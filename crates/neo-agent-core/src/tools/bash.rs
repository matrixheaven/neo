// `effective_cwd` / `effective_cmd` share an `effective_` prefix by design —
// they are the resolved Windows-vs-Unix pair after path translation.
#![allow(clippy::similar_names)]

use std::{path::PathBuf, sync::LazyLock, time::Duration};

use super::shell_env::{self, ShellEnv};
use super::shell_guard::{
    GuardStatusKind, GuardedCommandResult, GuardianClient, ShellAdmissionCallback,
    ShellAdmissionClass, ShellAdmissionEvent, ShellAdmissionRequest, ShellCommandPermit,
};
use super::{
    CommandOutput, ManagedBackgroundCommand, Tool, ToolContext, ToolError, ToolFuture, ToolResult,
    ToolUpdateCallback, cap_plain_output, parse_input, schema,
};
use crate::{
    BackgroundTaskManager, BackgroundTaskStatus, ShellCommandOrigin, ShellCommandOutcome,
    session::MAIN_AGENT_ID,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

/// Resolved POSIX shell, detected once and cached for the process lifetime
/// (it depends only on the platform / one-time path discovery). The `Result`
/// is cached so a missing shell does not trigger re-detection on every call —
/// detection is deterministic, so a retry would not help. On Windows this is
/// Git Bash.
static SHELL: LazyLock<Result<ShellEnv, shell_env::ShellEnvError>> =
    LazyLock::new(shell_env::detect_shell_env);

pub(crate) fn resolved_shell() -> Result<&'static ShellEnv, ToolError> {
    (*SHELL).as_ref().map_err(|err| {
        ToolError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            err.clone(),
        ))
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BashInput {
    #[schemars(description = "The shell command to execute.")]
    command: String,
    #[schemars(
        description = "The workspace-relative working directory in which to run the command. When omitted, the command runs in the session working directory. Supply it whenever the command works inside a nested project subtree: command text is never inspected for paths, so nested AGENTS.md instructions load only from this typed cwd."
    )]
    cwd: Option<String>,
    #[schemars(
        description = "Optional execution timeout in seconds. Omit this field to allow the command to run until it finishes or is cancelled. For potentially long-running work, prefer omission; if a limit is necessary, do not set it below 7200 seconds. Use shorter values only for commands that are explicitly expected to finish quickly.",
        range(min = 1)
    )]
    timeout_secs: Option<u64>,
    #[schemars(
        description = "Whether to run the command as a background task. When true, you must provide a description."
    )]
    run_in_background: Option<bool>,
    #[schemars(
        description = "A short description for the background task. Required when run_in_background is true."
    )]
    description: Option<String>,
    #[schemars(
        description = "Maximum number of bytes of combined stdout/stderr to return. Defaults to the runtime output limit when omitted."
    )]
    max_output_bytes: Option<usize>,
}

const DESCRIPTION: &str = r#"Execute a `bash` command. Use this for shell semantics — pipes, environment variables, processes, git, package managers, build/test runners, anything genuinely interactive or multi-step.

**Translate these to a dedicated tool instead:**
- `cat` / `head` / `tail` (known path) → `Read`
- `sed` / `awk` (in-place edit) → `Edit`
- `echo > file` / `cat <<EOF` → `Write`
- `find` / recursive `ls` to locate files by name pattern → `Glob` (plain `ls <known-directory>` is fine for listing a directory)
- `grep` / `rg` (search file contents) → `Grep`
- `echo` / `printf` (talk to the user) → just output text directly

The dedicated tools render in the per-tool permission UI and keep raw stdout out of the conversation; that is why they are worth reaching for whenever one fits.

**Output:**
The stdout and stderr will be combined and returned as a string. The output may be truncated if it is too long. If the command failed, the output will end with a line describing the failure: either `Command failed with exit code: N` for a non-zero exit, or `Command terminated by signal N (NAME) — hint` on Unix when the process was killed by a signal (e.g. SIGPIPE from a closed pipe).

If `run_in_background=true`, the command will be started as a background task and this tool will return a task ID instead of waiting for command completion. When doing that, you must provide a short `description`. Background commands are not subject to the foreground `timeout_secs`. You will be automatically notified when the task completes. Use `TaskOutput` with this task_id for a non-blocking status/output snapshot, and only set `block=true` when you explicitly want to wait for completion. Use `TaskStop` only if the task must be cancelled.

**Guidelines for safety and security:**
- Each shell tool call will be executed in a fresh shell environment. The shell variables, current working directory changes, and the shell history is not preserved between calls.
- The tool call will return after the command is finished. You shall not use this tool to execute an interactive command or a command that may run forever. For potentially long-running work, prefer omitting `timeout_secs`; if a limit is necessary, do not set it below 7200 seconds.
- Avoid using `..` to access files or directories outside the working directory.
- Avoid modifying files outside the working directory unless explicitly instructed to do so.
- Never run commands that require superuser privileges unless explicitly instructed to do so.
- When a command works inside a nested project subtree, set the `cwd` field to that subtree instead of embedding `cd <path> &&` in the command. The command string is never parsed for paths, so nested AGENTS.md instructions apply only when the typed `cwd` names the subtree.

**Guidelines for efficiency:**
- For multiple related commands, use `&&` to chain them in a single call, e.g. `cd /path && ls -la`
- Use `;` to run commands sequentially regardless of success/failure
- Use `||` for conditional execution (run second command only if first fails)
- Use pipe operations (`|`) and redirections (`>`, `>>`) to chain input and output between commands
- Always quote file paths containing spaces with double quotes (e.g., cd "/path with spaces/")
- Compose multi-step logic in a single call with `if` / `case` / `for` / `while` control flows
- Prefer `run_in_background=true` for long-running builds, tests, watchers, or servers when you need the conversation to continue before the command finishes.

**Commands available:**
The following common command categories are usually available. Availability still depends on the host, so when in doubt run `which <command>` first to confirm a command exists before relying on it.
- Navigation and inspection: `ls`, `pwd`, `cd`, `stat`, `file`, `du`, `df`, `tree`
- File and directory management: `cp`, `mv`, `rm`, `mkdir`, `touch`, `ln`, `chmod`, `chown`
- Text and data processing: `wc`, `sort`, `uniq`, `cut`, `tr`, `diff`, `xargs`
- Archives and compression: `tar`, `gzip`, `gunzip`, `zip`, `unzip`
- Networking and transfer: `curl`, `wget`, `ping`, `ssh`, `scp`
- Version control: `git`
- Process and system: `ps`, `kill`, `top`, `env`, `date`, `uname`, `whoami`
- Language and package toolchains: `node`, `npm`, `pnpm`, `yarn`, `python`, `pip` (use whichever the project actually relies on)"#;

pub struct BashTool;

pub struct ShellExecutionRequest {
    pub id: String,
    pub command: String,
    pub cwd: PathBuf,
    pub origin: ShellCommandOrigin,
    pub timeout: Option<Duration>,
    pub max_output_bytes: usize,
    pub cancel_token: tokio_util::sync::CancellationToken,
    pub stream_update: Option<ToolUpdateCallback>,
    pub background_tasks: Option<BackgroundTaskManager>,
    pub shell_runtime: super::ShellRuntime,
    pub admission: ShellAdmissionRequest,
    pub admission_callback: Option<ShellAdmissionCallback>,
}

fn admission_owner(ctx: &ToolContext) -> String {
    ctx.agent_id
        .clone()
        .unwrap_or_else(|| MAIN_AGENT_ID.to_owned())
}

fn model_admission(ctx: &ToolContext, class: ShellAdmissionClass) -> ShellAdmissionRequest {
    ShellAdmissionRequest {
        owner: admission_owner(ctx),
        class,
    }
}

async fn acquire_shell_permit(
    request: &ShellExecutionRequest,
) -> Result<ShellCommandPermit, ToolError> {
    let permit = tokio::select! {
        permit = request.shell_runtime.acquire(
            request.admission.clone(),
            request.admission_callback.clone(),
        ) => permit,
        () = request.cancel_token.cancelled() => return Err(ToolError::Cancelled),
    };
    if request.cancel_token.is_cancelled() {
        drop(permit);
        return Err(ToolError::Cancelled);
    }
    Ok(permit)
}

fn emit_admission_started(callback: &Option<ShellAdmissionCallback>) {
    if let Some(callback) = callback {
        callback(ShellAdmissionEvent::Started);
    }
}

/// Platform-aware termination info: `exit_code` from `ExitStatus::code()`,
/// plus `signal` on Unix (from `ExitStatus::signal()`). On Windows `signal` is
/// always `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellTermination {
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
}

pub struct ShellExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    /// Unix signal number when the process was killed by a signal (`None` on
    /// Windows or for normal exits).
    pub signal: Option<i32>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub truncated: bool,
    pub outcome: ShellCommandOutcome,
    pub foreground_task_id: Option<String>,
    pub resource_limit: Option<super::ResourceLimitDetail>,
}

impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "Bash"
    }

    fn description(&self) -> &'static str {
        DESCRIPTION
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<BashInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_shell_allowed()?;
            let input: BashInput = parse_input(self.name(), input)?;
            let max_output_bytes = input.max_output_bytes.unwrap_or(ctx.max_output_bytes);
            if input.run_in_background == Some(true) {
                if input.description.as_deref().unwrap_or("").trim().is_empty() {
                    return Err(ToolError::InvalidInput {
                        tool: self.name().to_owned(),
                        message: "description is required when run_in_background is true"
                            .to_owned(),
                    });
                }
                return start_background_command(
                    ctx,
                    &input.command,
                    input.cwd.as_deref(),
                    input.description.unwrap_or_default(),
                    max_output_bytes,
                )
                .await;
            }

            let timeout = parse_timeout_secs(self.name(), input.timeout_secs)?;
            let result = run_command(
                ctx,
                &input.command,
                input.cwd.as_deref(),
                timeout,
                max_output_bytes,
            )
            .await?;
            Ok(shell_command_result(&result))
        })
    }
}

fn shell_command_result(result: &ShellExecutionResult) -> ToolResult {
    let truncated = result.truncated || result.stdout_truncated || result.stderr_truncated;
    let mut content = format!("{}{}", result.stdout, result.stderr);
    if let Some(failure_msg) = shell_outcome_message(result) {
        if !content.ends_with('\n') && !content.is_empty() {
            content.push('\n');
        }
        content.push_str(&failure_msg);
    }
    if truncated {
        if !content.ends_with('\n') && !content.is_empty() {
            content.push('\n');
        }
        content.push_str("[output truncated]");
    }
    let tool_result = if shell_outcome_is_success(result) {
        ToolResult::ok(content)
    } else {
        ToolResult::error(content)
    };
    tool_result.with_details(shell_execution_details(result))
}

fn shell_outcome_is_success(result: &ShellExecutionResult) -> bool {
    match result.outcome {
        ShellCommandOutcome::Completed => result.exit_code == Some(0),
        ShellCommandOutcome::Backgrounded { .. } => true,
        ShellCommandOutcome::Cancelled
        | ShellCommandOutcome::TimedOut
        | ShellCommandOutcome::ResourceLimited => false,
    }
}

fn shell_outcome_message(result: &ShellExecutionResult) -> Option<String> {
    match result.outcome {
        ShellCommandOutcome::Completed => (result.exit_code != Some(0))
            .then(|| super::format_shell_failure(result.exit_code, result.signal)),
        ShellCommandOutcome::Cancelled => Some("Cancelled.".to_owned()),
        ShellCommandOutcome::TimedOut => Some("Timed out.".to_owned()),
        ShellCommandOutcome::ResourceLimited => {
            Some(super::format_resource_limit(result.resource_limit.as_ref()))
        }
        ShellCommandOutcome::Backgrounded { .. } => {
            Some("Moved to background. Use /tasks to view.".to_owned())
        }
    }
}

fn shell_execution_details(result: &ShellExecutionResult) -> serde_json::Value {
    let truncated = result.truncated || result.stdout_truncated || result.stderr_truncated;
    let mut details = json!({
        "exit_code": result.exit_code,
        "signal": result.signal,
        "stdout": result.stdout,
        "stderr": result.stderr,
        "stdout_truncated": result.stdout_truncated,
        "stderr_truncated": result.stderr_truncated,
        "truncated": truncated,
        "outcome": result.outcome.as_model_status(),
    });
    if let ShellCommandOutcome::Backgrounded { task_id } = &result.outcome {
        details["task_id"] = json!(task_id);
    }
    if let Some(task_id) = &result.foreground_task_id {
        details["foreground_task_id"] = json!(task_id);
    }
    if let Some(limit) = &result.resource_limit {
        details["resource_limit"] = json!(limit);
    }
    details
}

pub async fn execute_model_bash_for_runtime(
    ctx: &ToolContext,
    input: serde_json::Value,
) -> Result<ToolResult, ToolError> {
    ctx.ensure_shell_allowed()?;
    let input: BashInput = parse_input("Bash", input)?;
    let max_output_bytes = input.max_output_bytes.unwrap_or(ctx.max_output_bytes);
    if input.run_in_background == Some(true) {
        if input.description.as_deref().unwrap_or("").trim().is_empty() {
            return Err(ToolError::InvalidInput {
                tool: "Bash".to_owned(),
                message: "description is required when run_in_background is true".to_owned(),
            });
        }
        return start_background_command(
            ctx,
            &input.command,
            input.cwd.as_deref(),
            input.description.unwrap_or_default(),
            max_output_bytes,
        )
        .await;
    }

    let timeout = parse_timeout_secs("Bash", input.timeout_secs)?;
    let result = run_command_without_error_mapping(
        ctx,
        &input.command,
        input.cwd.as_deref(),
        timeout,
        max_output_bytes,
    )
    .await?;
    Ok(shell_command_result(&result))
}

fn parse_timeout_secs(tool: &str, timeout_secs: Option<u64>) -> Result<Option<Duration>, ToolError> {
    match timeout_secs {
        None => Ok(None),
        Some(0) => Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "timeout_secs must be positive".to_owned(),
        }),
        Some(secs) => Ok(Some(Duration::from_secs(secs))),
    }
}

async fn run_command(
    ctx: &ToolContext,
    command: &str,
    workdir: Option<&str>,
    timeout_duration: Option<Duration>,
    stream_max_bytes: usize,
) -> Result<ShellExecutionResult, ToolError> {
    let result = run_command_without_error_mapping(
        ctx,
        command,
        workdir,
        timeout_duration,
        stream_max_bytes,
    )
    .await?;
    match result.outcome {
        ShellCommandOutcome::TimedOut => Err(ToolError::CommandTimedOut {
            timeout_ms: timeout_duration
                .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
                .unwrap_or(0),
        }),
        ShellCommandOutcome::Cancelled => Err(ToolError::Cancelled),
        ShellCommandOutcome::ResourceLimited
        | ShellCommandOutcome::Completed
        | ShellCommandOutcome::Backgrounded { .. } => Ok(result),
    }
}

async fn run_command_without_error_mapping(
    ctx: &ToolContext,
    command: &str,
    workdir: Option<&str>,
    timeout_duration: Option<Duration>,
    stream_max_bytes: usize,
) -> Result<ShellExecutionResult, ToolError> {
    let cwd = match workdir {
        Some(path) => ctx.resolve_workspace_path(std::path::Path::new(path))?,
        None => ctx.cwd.clone(),
    };
    execute_shell_command(ShellExecutionRequest {
        id: "bash".to_owned(),
        command: command.to_owned(),
        cwd,
        origin: ShellCommandOrigin::ModelBashTool,
        timeout: timeout_duration,
        max_output_bytes: stream_max_bytes,
        cancel_token: ctx.cancel_token.clone(),
        stream_update: ctx.tool_update.clone(),
        background_tasks: None,
        shell_runtime: ctx.shell_runtime.clone(),
        admission: model_admission(ctx, ShellAdmissionClass::AgentForeground),
        admission_callback: ctx.shell_admission_callback.clone(),
    })
    .await
}

pub async fn execute_shell_command(
    request: ShellExecutionRequest,
) -> Result<ShellExecutionResult, ToolError> {
    if request.background_tasks.is_some()
        && matches!(request.origin, ShellCommandOrigin::UserShellMode)
    {
        return execute_manager_owned_shell_command(request).await;
    }
    let permit = acquire_shell_permit(&request).await?;
    if request.cancel_token.is_cancelled() {
        drop(permit);
        return Err(ToolError::Cancelled);
    }
    let max_output_bytes = request
        .shell_runtime
        .limits()
        .clamp_output_bytes(Some(request.max_output_bytes));
    emit_admission_started(&request.admission_callback);
    let client = GuardianClient::start_bash(
        &request.shell_runtime,
        BackgroundTaskManager::next_bash_task_id(),
        request.command,
        &request.cwd,
        request.shell_runtime.runtime_root(),
        request.timeout,
        max_output_bytes,
        request.stream_update,
        permit,
    )
    .await?;
    let result = tokio::select! {
        result = client.wait() => result,
        () = request.cancel_token.cancelled() => client.stop().await,
    };
    Ok(shell_result_from_guarded(
        result,
        None,
        max_output_bytes,
    ))
}

async fn execute_manager_owned_shell_command(
    request: ShellExecutionRequest,
) -> Result<ShellExecutionResult, ToolError> {
    let manager = request
        .background_tasks
        .clone()
        .expect("checked background task manager");
    let task_id = BackgroundTaskManager::next_bash_task_id();
    let permit = acquire_shell_permit(&request).await?;
    if request.cancel_token.is_cancelled() {
        drop(permit);
        return Err(ToolError::Cancelled);
    }
    let max_output_bytes = request
        .shell_runtime
        .limits()
        .clamp_output_bytes(Some(request.max_output_bytes));
    emit_admission_started(&request.admission_callback);
    let client = GuardianClient::start_bash(
        &request.shell_runtime,
        task_id.clone(),
        request.command.clone(),
        &request.cwd,
        manager
            .persistence_dir()
            .map_or(request.shell_runtime.runtime_root(), PathBuf::as_path),
        request.timeout,
        max_output_bytes,
        request.stream_update.clone(),
        permit,
    )
    .await?;
    manager
        .start_bash_foreground_with_task_id(
            task_id.clone(),
            request.command.clone(),
            ManagedBackgroundCommand::new(client),
        )
        .await?;
    loop {
        if manager.is_detached(&task_id).await {
            let snapshot = manager.snapshot(&task_id).await?;
            let output = snapshot.output.unwrap_or_else(empty_command_output);
            return Ok(shell_result_from_output(
                output,
                ShellCommandOutcome::Backgrounded {
                    task_id: task_id.clone().into(),
                },
                Some(task_id),
                max_output_bytes,
            ));
        }

        let snapshot = manager.snapshot(&task_id).await?;
        if !snapshot.status.is_active() {
            let output = snapshot.output.unwrap_or_else(empty_command_output);
            let outcome = match snapshot.status {
                BackgroundTaskStatus::TimedOut => ShellCommandOutcome::TimedOut,
                BackgroundTaskStatus::Cancelled => ShellCommandOutcome::Cancelled,
                BackgroundTaskStatus::ResourceLimited => ShellCommandOutcome::ResourceLimited,
                _ => ShellCommandOutcome::Completed,
            };
            return Ok(shell_result_from_output(
                output,
                outcome,
                Some(task_id.clone()),
                max_output_bytes,
            ));
        }

        tokio::select! {
            () = request.cancel_token.cancelled() => {
                let _ = manager.stop(&task_id, "Cancelled foreground shell command", max_output_bytes).await?;
                let snapshot = manager.snapshot(&task_id).await?;
                let output = snapshot.output.unwrap_or_else(empty_command_output);
                return Ok(shell_result_from_output(
                    output,
                    ShellCommandOutcome::Cancelled,
                    Some(task_id.clone()),
                    max_output_bytes,
                ));
            }
            () = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
    }
}

fn shell_result_from_guarded(
    result: GuardedCommandResult,
    foreground_task_id: Option<String>,
    max_output_bytes: usize,
) -> ShellExecutionResult {
    let outcome = match result.exit.status {
        GuardStatusKind::Completed | GuardStatusKind::Failed | GuardStatusKind::ParentExited => {
            ShellCommandOutcome::Completed
        }
        GuardStatusKind::Cancelled => ShellCommandOutcome::Cancelled,
        GuardStatusKind::TimedOut => ShellCommandOutcome::TimedOut,
        GuardStatusKind::ResourceLimited => ShellCommandOutcome::ResourceLimited,
    };
    let output = CommandOutput {
        exit_code: result.exit.exit_code,
        signal: result.exit.signal,
        stdout: String::from_utf8_lossy(&result.output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&result.output.stderr).into_owned(),
        stdout_truncated: result.exit.omitted_output_bytes > 0,
        stderr_truncated: result.exit.omitted_output_bytes > 0,
        resource_limit: result.exit.resource_limit.clone(),
    };
    let mut shell_result =
        shell_result_from_output(output, outcome, foreground_task_id, max_output_bytes);
    shell_result.resource_limit = result.exit.resource_limit;
    shell_result
}

fn empty_command_output() -> CommandOutput {
    CommandOutput {
        exit_code: None,
        signal: None,
        stdout: String::new(),
        stderr: String::new(),
        stdout_truncated: false,
        stderr_truncated: false,
        resource_limit: None,
    }
}

fn shell_result_from_output(
    output: CommandOutput,
    outcome: ShellCommandOutcome,
    foreground_task_id: Option<String>,
    max_output_bytes: usize,
) -> ShellExecutionResult {
    cap_shell_result_output(
        ShellExecutionResult {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.exit_code,
            signal: output.signal,
            stdout_truncated: output.stdout_truncated,
            stderr_truncated: output.stderr_truncated,
            truncated: output.stdout_truncated || output.stderr_truncated,
            outcome,
            foreground_task_id,
            resource_limit: output.resource_limit,
        },
        max_output_bytes,
    )
}

fn cap_shell_result_output(
    result: ShellExecutionResult,
    max_output_bytes: usize,
) -> ShellExecutionResult {
    let (stdout, stdout_truncated) = cap_plain_output(&result.stdout, max_output_bytes);
    let (stderr, stderr_truncated) = cap_plain_output(&result.stderr, max_output_bytes);
    ShellExecutionResult {
        stdout,
        stderr,
        exit_code: result.exit_code,
        signal: result.signal,
        stdout_truncated: result.stdout_truncated || stdout_truncated,
        stderr_truncated: result.stderr_truncated || stderr_truncated,
        truncated: result.truncated || stdout_truncated || stderr_truncated,
        outcome: result.outcome,
        foreground_task_id: result.foreground_task_id,
        resource_limit: result.resource_limit,
    }
}

async fn start_background_command(
    ctx: &ToolContext,
    command: &str,
    workdir: Option<&str>,
    description: String,
    max_output_bytes: usize,
) -> Result<ToolResult, ToolError> {
    // Validate path before waiting for admission.
    let _ = match workdir {
        Some(path) => ctx.resolve_workspace_path(std::path::Path::new(path))?,
        None => ctx.cwd.clone(),
    };
    let task_id = BackgroundTaskManager::next_bash_task_id();
    let admission = model_admission(ctx, ShellAdmissionClass::AgentBackground);
    let permit = tokio::select! {
        permit = ctx.shell_runtime.acquire(admission, ctx.shell_admission_callback.clone()) => permit,
        () = ctx.cancel_token.cancelled() => return Err(ToolError::Cancelled),
    };
    if ctx.cancel_token.is_cancelled() {
        drop(permit);
        return Err(ToolError::Cancelled);
    }
    ctx.ensure_shell_allowed()?;
    let cwd = match workdir {
        Some(path) => ctx.resolve_workspace_path(std::path::Path::new(path))?,
        None => ctx.cwd.clone(),
    };
    emit_admission_started(&ctx.shell_admission_callback);
    let max_output_bytes = ctx
        .shell_runtime
        .limits()
        .clamp_output_bytes(Some(max_output_bytes));
    let client = GuardianClient::start_bash(
        &ctx.shell_runtime,
        task_id.clone(),
        command.to_owned(),
        &cwd,
        ctx.background_tasks
            .persistence_dir()
            .map_or(ctx.shell_runtime.runtime_root(), PathBuf::as_path),
        None,
        max_output_bytes,
        ctx.tool_update.clone(),
        permit,
    )
    .await?;

    ctx.background_tasks
        .start_bash_with_task_id(
            task_id,
            description,
            ManagedBackgroundCommand::new(client),
            max_output_bytes,
        )
        .await
}
