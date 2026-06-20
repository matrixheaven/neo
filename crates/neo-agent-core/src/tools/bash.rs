use std::{fmt::Write, process::Stdio, sync::Arc, time::Duration};

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
    ToolUpdateCallback, cap_output_details, cap_plain_output, output_from_buffers, parse_input,
    schema,
};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BashInput {
    /// The shell command to execute.
    command: String,
    /// The workspace-relative working directory in which to run the command.
    /// When omitted, the command runs in the session working directory.
    cwd: Option<String>,
    /// Optional timeout in seconds for the command to execute.
    /// Only applies to foreground commands; background commands run until they
    /// finish or are stopped. Defaults to the runtime bash timeout.
    timeout: Option<u64>,
    /// Whether to run the command as a background task.
    run_in_background: Option<bool>,
    /// A short description for the background task.
    /// Required when `run_in_background` is true.
    description: Option<String>,
    /// If true, do not apply a timeout to the command.
    /// Only applies when `run_in_background` is true.
    disable_timeout: Option<bool>,
    /// Maximum number of bytes of combined stdout/stderr to return.
    /// Defaults to the runtime output limit when omitted.
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
            let output = run_command(
                ctx,
                &input.command,
                input.cwd.as_deref(),
                Duration::from_millis(timeout_ms),
                max_output_bytes,
            )
            .await?;
            Ok(command_result(&output, max_output_bytes))
        })
    }
}

struct ManagedChild {
    child: Child,
    #[cfg(unix)]
    process_group: Option<Pid>,
}

const PIPE_DRAIN_TIMEOUT: Duration = Duration::from_millis(50);

fn command_result(output: &CommandOutput, max_output_bytes: usize) -> ToolResult {
    let (stdout_capped, stdout_truncated) = cap_plain_output(&output.stdout, max_output_bytes);
    let (stderr_capped, stderr_truncated) = cap_plain_output(&output.stderr, max_output_bytes);
    let stdout_details = cap_output_details(&output.stdout, max_output_bytes);
    let stderr_details = cap_output_details(&output.stderr, max_output_bytes);
    let truncated = stdout_truncated || stderr_truncated;
    let mut content = format!("{stdout_capped}{stderr_capped}");
    if output.exit_code != Some(0) {
        let exit_label = output
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
    let result = if output.exit_code == Some(0) {
        ToolResult::ok(content)
    } else {
        ToolResult::error(content)
    };
    result.with_details(json!({
        "exit_code": output.exit_code,
        "stdout": stdout_details,
        "stderr": stderr_details,
        "stdout_truncated": stdout_truncated,
        "stderr_truncated": stderr_truncated,
        "truncated": truncated,
    }))
}

async fn run_command(
    ctx: &ToolContext,
    command: &str,
    workdir: Option<&str>,
    timeout_duration: Duration,
    stream_max_bytes: usize,
) -> Result<CommandOutput, ToolError> {
    let mut process = spawn_bash_process(ctx, command, workdir)?;
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let stdout_task = spawn_streaming_output_reader(
        process.child.stdout.take().expect("stdout was piped"),
        stdout.clone(),
        ctx.tool_update.clone(),
        stream_max_bytes,
    );
    let stderr_task = spawn_streaming_output_reader(
        process.child.stderr.take().expect("stderr was piped"),
        stderr.clone(),
        ctx.tool_update.clone(),
        stream_max_bytes,
    );

    let status = tokio::select! {
        status = process.child.wait() => status?,
        () = tokio::time::sleep(timeout_duration) => {
            kill_child(&mut process).await;
            return Err(ToolError::CommandTimedOut {
                timeout_ms: u64::try_from(timeout_duration.as_millis()).unwrap_or(u64::MAX),
            });
        }
        () = ctx.cancel_token.cancelled() => {
            kill_child(&mut process).await;
            return Err(ToolError::Cancelled);
        }
    };

    drain_reader(stdout_task).await;
    drain_reader(stderr_task).await;
    Ok(output_from_buffers(status.code(), stdout, stderr).await)
}

fn spawn_bash_process(
    ctx: &ToolContext,
    command_text: &str,
    workdir: Option<&str>,
) -> Result<ManagedChild, ToolError> {
    let cwd = match workdir {
        Some(path) => ctx.resolve_workspace_path(std::path::Path::new(path))?,
        None => ctx.cwd.clone(),
    };
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
    let process = spawn_bash_process(ctx, command, workdir)?;
    let process = Arc::new(Mutex::new(process));

    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let mut locked_process = process.lock().await;
    let stdout_task = spawn_output_reader(
        locked_process
            .child
            .stdout
            .take()
            .expect("stdout was piped"),
        stdout.clone(),
    );
    let stderr_task = spawn_output_reader(
        locked_process
            .child
            .stderr
            .take()
            .expect("stderr was piped"),
        stderr.clone(),
    );
    drop(locked_process);

    let try_wait_process = Arc::clone(&process);
    let cleanup_process = Arc::clone(&process);
    let command = ManagedBackgroundCommand {
        stdout,
        stderr,
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
    };

    ctx.background_tasks
        .start_bash(description, command, max_output_bytes)
        .await
}

fn spawn_output_reader<R>(mut reader: R, buffer: Arc<Mutex<Vec<u8>>>) -> JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut local = [0_u8; 8192];
        loop {
            match reader.read(&mut local).await {
                Ok(0) | Err(_) => break,
                Ok(bytes_read) => buffer.lock().await.extend_from_slice(&local[..bytes_read]),
            }
        }
    })
}

fn spawn_streaming_output_reader<R>(
    mut reader: R,
    buffer: Arc<Mutex<Vec<u8>>>,
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
                    buffer.lock().await.extend_from_slice(chunk);
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
