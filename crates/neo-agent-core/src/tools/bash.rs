use std::{fmt::Write, path::PathBuf, process::Stdio, sync::Arc, time::Duration};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    sync::Mutex,
    task::JoinHandle,
};

#[cfg(unix)]
use rustix::{
    io::Errno,
    process::{Pid, Signal, kill_process_group},
};

use super::{
    CommandOutput, ManagedBackgroundCommand, Tool, ToolContext, ToolError, ToolFuture, ToolResult,
    ToolUpdateCallback, cap_plain_output, output_from_buffers, parse_input, schema,
};
use crate::{BackgroundTaskManager, BackgroundTaskStatus, ShellCommandOrigin, ShellCommandOutcome};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BashInput {
    #[schemars(description = "The shell command to execute.")]
    command: String,
    #[schemars(
        description = "The workspace-relative working directory in which to run the command. When omitted, the command runs in the session working directory."
    )]
    cwd: Option<String>,
    #[schemars(
        description = "Optional timeout in seconds for foreground commands. Background commands run until finished or stopped. Defaults to the runtime bash timeout."
    )]
    timeout: Option<u64>,
    #[schemars(
        description = "Whether to run the command as a background task. When true, you must provide a description."
    )]
    run_in_background: Option<bool>,
    #[schemars(
        description = "A short description for the background task. Required when run_in_background is true."
    )]
    description: Option<String>,
    #[schemars(
        description = "If true, do not apply a timeout to the command. Only applies when run_in_background is true."
    )]
    disable_timeout: Option<bool>,
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
The stdout and stderr will be combined and returned as a string. The output may be truncated if it is too long. If the command failed, the output will end with a `Command failed with exit code: N` line stating the non-zero exit code.

If `run_in_background=true`, the command will be started as a background task and this tool will return a task ID instead of waiting for command completion. When doing that, you must provide a short `description`. Background commands are not subject to the foreground `timeout`. You will be automatically notified when the task completes. Use `TaskOutput` with this task_id for a non-blocking status/output snapshot, and only set `block=true` when you explicitly want to wait for completion. Use `TaskStop` only if the task must be cancelled.

**Guidelines for safety and security:**
- Each shell tool call will be executed in a fresh shell environment. The shell variables, current working directory changes, and the shell history is not preserved between calls.
- The tool call will return after the command is finished. You shall not use this tool to execute an interactive command or a command that may run forever. For possibly long-running foreground commands, set the `timeout` argument in seconds. The foreground timeout defaults to the runtime bash timeout (currently 10 minutes).
- Avoid using `..` to access files or directories outside the working directory.
- Avoid modifying files outside the working directory unless explicitly instructed to do so.
- Never run commands that require superuser privileges unless explicitly instructed to do so.

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
    pub foreground_timeout: Duration,
    pub background_timeout: Duration,
    pub max_output_bytes: usize,
    pub cancel_token: tokio_util::sync::CancellationToken,
    pub stream_update: Option<ToolUpdateCallback>,
    pub background_tasks: Option<BackgroundTaskManager>,
}

pub struct ShellExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub truncated: bool,
    pub outcome: ShellCommandOutcome,
    pub foreground_task_id: Option<String>,
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
            let _disable_timeout = input.disable_timeout.unwrap_or(false);
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

            let timeout_ms = input.timeout.map_or_else(
                || u64::try_from(ctx.bash_timeout.as_millis()).unwrap_or(u64::MAX),
                |s| s.saturating_mul(1000),
            );
            let result = run_command(
                ctx,
                &input.command,
                input.cwd.as_deref(),
                Duration::from_millis(timeout_ms),
                max_output_bytes,
            )
            .await?;
            Ok(shell_command_result(&result))
        })
    }
}

struct ManagedChild {
    child: Child,
    #[cfg(unix)]
    process_group: Option<Pid>,
}

const PIPE_DRAIN_TIMEOUT: Duration = Duration::from_millis(50);

fn shell_command_result(result: &ShellExecutionResult) -> ToolResult {
    let truncated = result.truncated || result.stdout_truncated || result.stderr_truncated;
    let mut content = format!("{}{}", result.stdout, result.stderr);
    if result.exit_code != Some(0) {
        let exit_label = result
            .exit_code
            .map_or_else(|| "signal".to_owned(), |code| code.to_string());
        if !content.ends_with('\n') && !content.is_empty() {
            content.push('\n');
        }
        let _ = write!(content, "Command failed with exit code: {exit_label}.");
    }
    if truncated {
        if !content.ends_with('\n') && !content.is_empty() {
            content.push('\n');
        }
        content.push_str("[output truncated]");
    }
    let tool_result = if result.exit_code == Some(0) {
        ToolResult::ok(content)
    } else {
        ToolResult::error(content)
    };
    tool_result.with_details(shell_execution_details(result))
}

fn shell_execution_details(result: &ShellExecutionResult) -> serde_json::Value {
    let truncated = result.truncated || result.stdout_truncated || result.stderr_truncated;
    let mut details = json!({
        "exit_code": result.exit_code,
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
    details
}

pub async fn execute_model_bash_for_runtime(
    ctx: &ToolContext,
    input: serde_json::Value,
) -> Result<ToolResult, ToolError> {
    ctx.ensure_shell_allowed()?;
    let input: BashInput = parse_input("Bash", input)?;
    let max_output_bytes = input.max_output_bytes.unwrap_or(ctx.max_output_bytes);
    let _disable_timeout = input.disable_timeout.unwrap_or(false);
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

    let timeout_ms = input.timeout.map_or_else(
        || u64::try_from(ctx.bash_timeout.as_millis()).unwrap_or(u64::MAX),
        |s| s.saturating_mul(1000),
    );
    let result = run_command_without_error_mapping(
        ctx,
        &input.command,
        input.cwd.as_deref(),
        Duration::from_millis(timeout_ms),
        max_output_bytes,
    )
    .await?;
    Ok(shell_command_result(&result))
}

async fn run_command(
    ctx: &ToolContext,
    command: &str,
    workdir: Option<&str>,
    timeout_duration: Duration,
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
            timeout_ms: u64::try_from(timeout_duration.as_millis()).unwrap_or(u64::MAX),
        }),
        ShellCommandOutcome::Cancelled => Err(ToolError::Cancelled),
        ShellCommandOutcome::Completed | ShellCommandOutcome::Backgrounded { .. } => Ok(result),
    }
}

async fn run_command_without_error_mapping(
    ctx: &ToolContext,
    command: &str,
    workdir: Option<&str>,
    timeout_duration: Duration,
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
        foreground_timeout: timeout_duration,
        background_timeout: Duration::from_secs(10 * 60),
        max_output_bytes: stream_max_bytes,
        cancel_token: ctx.cancel_token.clone(),
        stream_update: ctx.tool_update.clone(),
        background_tasks: None,
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
    let mut process = spawn_bash_process_at(&request.command, &request.cwd)?;
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let stdout_truncated = Arc::new(Mutex::new(false));
    let stderr_truncated = Arc::new(Mutex::new(false));
    let stdout_task = spawn_streaming_output_reader(
        process.child.stdout.take().expect("stdout was piped"),
        stdout.clone(),
        stdout_truncated.clone(),
        request.stream_update.clone(),
        request.max_output_bytes,
    );
    let stderr_task = spawn_streaming_output_reader(
        process.child.stderr.take().expect("stderr was piped"),
        stderr.clone(),
        stderr_truncated.clone(),
        request.stream_update.clone(),
        request.max_output_bytes,
    );

    tokio::select! {
        status = process.child.wait() => {
            let status = status?;
            let output = finish_shell_process(
                status.code(),
                stdout,
                stderr,
                stdout_truncated,
                stderr_truncated,
                stdout_task,
                stderr_task,
            ).await;
            Ok(shell_result_from_output(
                output,
                ShellCommandOutcome::Completed,
                None,
                request.max_output_bytes,
            ))
        }
        () = tokio::time::sleep(request.foreground_timeout) => {
            let exit_code = kill_child(&mut process).await;
            (drain_reader)(stdout_task).await;
            (drain_reader)(stderr_task).await;
            let output =
                output_from_bounded_buffers(exit_code, stdout, stderr, stdout_truncated, stderr_truncated).await;
            Ok(shell_result_from_output(
                output,
                ShellCommandOutcome::TimedOut,
                None,
                request.max_output_bytes,
            ))
        }
        () = request.cancel_token.cancelled() => {
            let exit_code = kill_child(&mut process).await;
            (drain_reader)(stdout_task).await;
            (drain_reader)(stderr_task).await;
            let output =
                output_from_bounded_buffers(exit_code, stdout, stderr, stdout_truncated, stderr_truncated).await;
            Ok(shell_result_from_output(
                output,
                ShellCommandOutcome::Cancelled,
                None,
                request.max_output_bytes,
            ))
        }
    }
}

async fn execute_manager_owned_shell_command(
    request: ShellExecutionRequest,
) -> Result<ShellExecutionResult, ToolError> {
    let manager = request
        .background_tasks
        .clone()
        .expect("checked background task manager");
    let command = spawn_managed_background_command_at_with_stream(
        &request.command,
        &request.cwd,
        request.stream_update.clone(),
        request.max_output_bytes,
    )?;
    let task_id = manager
        .start_bash_foreground(
            request.command.clone(),
            command,
            request.max_output_bytes,
            request.background_timeout,
        )
        .await?;
    let started = tokio::time::Instant::now();
    loop {
        if manager.is_detached(&task_id).await {
            let snapshot = manager.snapshot(&task_id).await?;
            let output = snapshot.output.unwrap_or_else(empty_command_output);
            return Ok(shell_result_from_output(
                output,
                ShellCommandOutcome::Backgrounded {
                    task_id: task_id.clone(),
                },
                Some(task_id),
                request.max_output_bytes,
            ));
        }

        let snapshot = manager.snapshot(&task_id).await?;
        if !snapshot.status.is_active() {
            let output = snapshot.output.unwrap_or_else(empty_command_output);
            let outcome = match snapshot.status {
                BackgroundTaskStatus::TimedOut => ShellCommandOutcome::TimedOut,
                BackgroundTaskStatus::Stopped => ShellCommandOutcome::Cancelled,
                _ => ShellCommandOutcome::Completed,
            };
            return Ok(shell_result_from_output(
                output,
                outcome,
                Some(task_id.clone()),
                request.max_output_bytes,
            ));
        }

        tokio::select! {
            () = request.cancel_token.cancelled() => {
                let _ = manager.stop(&task_id, "Cancelled foreground shell command", request.max_output_bytes).await?;
                let snapshot = manager.snapshot(&task_id).await?;
                let output = snapshot.output.unwrap_or_else(empty_command_output);
                return Ok(shell_result_from_output(
                    output,
                    ShellCommandOutcome::Cancelled,
                    Some(task_id.clone()),
                    request.max_output_bytes,
                ));
            }
            () = tokio::time::sleep_until(started + request.foreground_timeout) => {
                let _ = manager.stop(&task_id, "Foreground shell command timed out", request.max_output_bytes).await?;
                let snapshot = manager.snapshot(&task_id).await?;
                let output = snapshot.output.unwrap_or_else(empty_command_output);
                return Ok(shell_result_from_output(
                    output,
                    ShellCommandOutcome::TimedOut,
                    Some(task_id.clone()),
                    request.max_output_bytes,
                ));
            }
            () = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
    }
}

fn empty_command_output() -> CommandOutput {
    CommandOutput {
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        stdout_truncated: false,
        stderr_truncated: false,
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
            stdout_truncated: output.stdout_truncated,
            stderr_truncated: output.stderr_truncated,
            truncated: output.stdout_truncated || output.stderr_truncated,
            outcome,
            foreground_task_id,
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
        stdout_truncated: result.stdout_truncated || stdout_truncated,
        stderr_truncated: result.stderr_truncated || stderr_truncated,
        truncated: result.truncated || stdout_truncated || stderr_truncated,
        outcome: result.outcome,
        foreground_task_id: result.foreground_task_id,
    }
}

fn spawn_bash_process_at(
    command_text: &str,
    cwd: &std::path::Path,
) -> Result<ManagedChild, ToolError> {
    let mut process_command = Command::new("bash");
    process_command
        .arg("-lc")
        .arg(command_text)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    process_command.process_group(0);

    let child = process_command.spawn()?;
    Ok(ManagedChild {
        #[cfg(unix)]
        process_group: child_process_group(&child),
        child,
    })
}

async fn finish_shell_process(
    exit_code: Option<i32>,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stdout_truncated: Arc<Mutex<bool>>,
    stderr_truncated: Arc<Mutex<bool>>,
    stdout_task: JoinHandle<()>,
    stderr_task: JoinHandle<()>,
) -> CommandOutput {
    drain_reader(stdout_task).await;
    drain_reader(stderr_task).await;
    output_from_bounded_buffers(
        exit_code,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
    )
    .await
}

async fn output_from_bounded_buffers(
    exit_code: Option<i32>,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stdout_truncated: Arc<Mutex<bool>>,
    stderr_truncated: Arc<Mutex<bool>>,
) -> CommandOutput {
    let mut output = output_from_buffers(exit_code, stdout, stderr).await;
    output.stdout_truncated = *stdout_truncated.lock().await;
    output.stderr_truncated = *stderr_truncated.lock().await;
    output
}

async fn drain_reader(task: JoinHandle<()>) {
    let mut task = task;
    tokio::select! {
        () = tokio::time::sleep(PIPE_DRAIN_TIMEOUT) => task.abort(),
        _ = &mut task => {}
    }
}

#[cfg(unix)]
fn child_process_group(child: &Child) -> Option<Pid> {
    child
        .id()
        .and_then(|pid| i32::try_from(pid).ok())
        .and_then(Pid::from_raw)
}

async fn kill_child(process: &mut ManagedChild) -> Option<i32> {
    kill_process_group_if_available(process);
    let _ = process.child.start_kill();
    process
        .child
        .wait()
        .await
        .ok()
        .and_then(|status| status.code())
}

fn kill_process_group_if_available(process: &ManagedChild) {
    #[cfg(unix)]
    if let Some(process_group) = process.process_group {
        let _ = kill_process_group(process_group, Signal::KILL)
            .or_else(|err| if err == Errno::SRCH { Ok(()) } else { Err(err) });
    }
}

async fn start_background_command(
    ctx: &ToolContext,
    command: &str,
    workdir: Option<&str>,
    description: String,
    max_output_bytes: usize,
) -> Result<ToolResult, ToolError> {
    let cwd = match workdir {
        Some(path) => ctx.resolve_workspace_path(std::path::Path::new(path))?,
        None => ctx.cwd.clone(),
    };
    let command =
        spawn_managed_background_command_at_with_stream(command, &cwd, None, max_output_bytes)?;

    ctx.background_tasks
        .start_bash(description, command, max_output_bytes)
        .await
}

fn spawn_managed_background_command_at_with_stream(
    command: &str,
    cwd: &std::path::Path,
    callback: Option<ToolUpdateCallback>,
    stream_max_bytes: usize,
) -> Result<ManagedBackgroundCommand, ToolError> {
    let process = spawn_bash_process_at(command, cwd)?;
    let process = Arc::new(Mutex::new(process));

    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let stdout_truncated = Arc::new(Mutex::new(false));
    let stderr_truncated = Arc::new(Mutex::new(false));
    let mut locked_process = process.try_lock().expect("new process lock available");
    let stdout_task = spawn_streaming_output_reader(
        locked_process
            .child
            .stdout
            .take()
            .expect("stdout was piped"),
        stdout.clone(),
        stdout_truncated.clone(),
        callback.clone(),
        stream_max_bytes,
    );
    let stderr_task = spawn_streaming_output_reader(
        locked_process
            .child
            .stderr
            .take()
            .expect("stderr was piped"),
        stderr.clone(),
        stderr_truncated.clone(),
        callback,
        stream_max_bytes,
    );
    drop(locked_process);

    let try_wait_process = Arc::clone(&process);
    let cleanup_process = Arc::clone(&process);
    Ok(ManagedBackgroundCommand {
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
        stdout_task,
        stderr_task,
        try_wait: Arc::new(move || {
            let process = Arc::clone(&try_wait_process);
            Box::pin(async move {
                let mut process = process.lock().await;
                process
                    .child
                    .try_wait()
                    .map(|status| status.and_then(|s| s.code()))
            })
        }),
        cleanup: Arc::new(move || {
            let process = Arc::clone(&cleanup_process);
            Box::pin(async move {
                let mut process = process.lock().await;
                kill_child(&mut process).await
            })
        }),
        drain: Arc::new(|task| Box::pin(async move { drain_reader(task).await })),
    })
}

fn spawn_streaming_output_reader<R>(
    mut reader: R,
    buffer: Arc<Mutex<Vec<u8>>>,
    truncated: Arc<Mutex<bool>>,
    callback: Option<ToolUpdateCallback>,
    stream_max_bytes: usize,
) -> JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut local = [0_u8; 8192];
        let mut streamed = 0;
        loop {
            match reader.read(&mut local).await {
                Ok(0) | Err(_) => break,
                Ok(bytes_read) => {
                    let chunk = &local[..bytes_read];
                    {
                        let mut buffer = buffer.lock().await;
                        if buffer.len() < stream_max_bytes {
                            let remaining = stream_max_bytes - buffer.len();
                            let buffered = &chunk[..chunk.len().min(remaining)];
                            buffer.extend_from_slice(buffered);
                            if buffered.len() < chunk.len() {
                                *truncated.lock().await = true;
                            }
                        } else {
                            *truncated.lock().await = true;
                        }
                    }
                    if streamed < stream_max_bytes {
                        let remaining = stream_max_bytes - streamed;
                        let streamed_chunk = &chunk[..chunk.len().min(remaining)];
                        streamed += streamed_chunk.len();
                        if let Some(callback) = &callback {
                            callback(&String::from_utf8_lossy(streamed_chunk));
                        }
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt as _;

    #[tokio::test]
    async fn streaming_output_reader_bounds_buffer_to_max_output_bytes() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let truncated = Arc::new(Mutex::new(false));
        let handle =
            spawn_streaming_output_reader(reader, buffer.clone(), truncated.clone(), None, 4);

        writer
            .write_all(b"keep-secret-leak-tail")
            .await
            .expect("write output");
        drop(writer);
        handle.await.expect("reader task");

        assert_eq!(&*buffer.lock().await, b"keep");
        assert!(*truncated.lock().await);
    }
}
