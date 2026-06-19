use std::{
    collections::HashMap,
    process::Stdio,
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    sync::Mutex,
    task::JoinHandle,
};
use uuid::Uuid;

#[cfg(unix)]
use rustix::{
    io::Errno,
    process::{Pid, Signal, kill_process_group},
};

use super::{
    ProcessKind, Tool, ToolContext, ToolError, ToolFuture, ToolResult, ToolUpdateCallback,
    parse_input, schema,
};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BashInput {
    #[schemars(description = "The shell command to execute.")]
    command: String,
    #[schemars(
        description = "The workspace-relative working directory in which to run the command. When omitted, the command runs in the session working directory."
    )]
    cwd: Option<String>,
    #[schemars(description = "Optional timeout in seconds for the command to execute.")]
    timeout: Option<u64>,
    #[schemars(description = "Whether to run the command as a background task.")]
    run_in_background: Option<bool>,
    #[schemars(
        description = "A short description for the background task. Required when run_in_background is true."
    )]
    description: Option<String>,
    #[schemars(
        description = "If true, do not apply a timeout to the command. Only applies when run_in_background is true."
    )]
    disable_timeout: Option<bool>,
    max_output_bytes: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TaskOutputInput {
    task_id: String,
    block: Option<bool>,
    timeout: Option<u64>,
    max_output_bytes: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct TaskStopInput {
    task_id: String,
    reason: Option<String>,
    max_output_bytes: Option<usize>,
}

pub struct BashTool;

impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "Bash"
    }

    fn description(&self) -> &'static str {
        "Run a shell command in the workspace. Use cwd for a workspace-relative working directory and run_in_background for long-running commands."
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

pub struct TaskOutputTool;

impl Tool for TaskOutputTool {
    fn name(&self) -> &'static str {
        "TaskOutput"
    }

    fn description(&self) -> &'static str {
        "Read output from a background Bash task."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<TaskOutputInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: TaskOutputInput = parse_input(self.name(), input)?;
            let max_output_bytes = input.max_output_bytes.unwrap_or(ctx.max_output_bytes);
            task_output(
                ctx,
                self.name(),
                &input.task_id,
                input.block.unwrap_or(false),
                Duration::from_secs(input.timeout.unwrap_or(30)),
                max_output_bytes,
            )
            .await
        })
    }
}

pub struct TaskStopTool;

impl Tool for TaskStopTool {
    fn name(&self) -> &'static str {
        "TaskStop"
    }

    fn description(&self) -> &'static str {
        "Stop a running background Bash task."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<TaskStopInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_shell_allowed()?;
            let input: TaskStopInput = parse_input(self.name(), input)?;
            let max_output_bytes = input.max_output_bytes.unwrap_or(ctx.max_output_bytes);
            task_stop(ctx, self.name(), &input.task_id, max_output_bytes).await
        })
    }
}

static BACKGROUND_COMMANDS: LazyLock<Mutex<HashMap<String, BackgroundTask>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

enum BackgroundTask {
    Running(BackgroundCommand),
    Finished {
        status: &'static str,
        output: CommandOutput,
    },
}

struct BackgroundCommand {
    process: ManagedChild,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stdout_task: JoinHandle<()>,
    stderr_task: JoinHandle<()>,
}

struct ManagedChild {
    child: Child,
    #[cfg(unix)]
    process_group: Option<Pid>,
}

struct CommandOutput {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
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
        content.push_str(&format!("Command failed with exit code: {exit_label}."));
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
    max_output_bytes: usize,
) -> Result<ToolResult, ToolError> {
    let mut process = spawn_bash_process(ctx, command, workdir)?;

    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let stdout_task = spawn_output_reader(
        process.child.stdout.take().expect("stdout was piped"),
        stdout.clone(),
    );
    let stderr_task = spawn_output_reader(
        process.child.stderr.take().expect("stderr was piped"),
        stderr.clone(),
    );

    let handle = format!("bash-{}", Uuid::new_v4());
    BACKGROUND_COMMANDS.lock().await.insert(
        handle.clone(),
        BackgroundTask::Running(BackgroundCommand {
            process,
            stdout,
            stderr,
            stdout_task,
            stderr_task,
        }),
    );
    ctx.process_supervisor
        .register(handle.clone(), ProcessKind::BashBackground, |handle| {
            Box::pin(async move { cleanup_background_command(&handle).await })
        })
        .await;

    Ok(
        ToolResult::ok(format!("started background task: {handle}")).with_details(json!({
            "task_id": handle,
            "status": "running",
            "stdout": "",
            "stderr": "",
            "stdout_truncated": false,
            "stderr_truncated": false,
            "truncated": false,
            "max_output_bytes": max_output_bytes,
        })),
    )
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

async fn task_output(
    ctx: &ToolContext,
    tool: &str,
    task_id: &str,
    block: bool,
    timeout: Duration,
    max_output_bytes: usize,
) -> Result<ToolResult, ToolError> {
    let deadline = Instant::now() + timeout;
    loop {
        let snapshot = task_snapshot(ctx, tool, task_id, max_output_bytes).await?;
        let is_running = snapshot
            .details
            .as_ref()
            .and_then(|details| details.get("status"))
            .and_then(serde_json::Value::as_str)
            == Some("running");
        if !block || !is_running || Instant::now() >= deadline {
            return Ok(snapshot);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn task_stop(
    ctx: &ToolContext,
    tool: &str,
    task_id: &str,
    max_output_bytes: usize,
) -> Result<ToolResult, ToolError> {
    let mut commands = BACKGROUND_COMMANDS.lock().await;
    let Some(task) = commands.get_mut(task_id) else {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: format!("unknown background task `{task_id}`"),
        });
    };
    match task {
        BackgroundTask::Finished { status, output } => Ok(background_command_result(
            task_id,
            status,
            output,
            max_output_bytes,
        )),
        BackgroundTask::Running(_) => {
            let BackgroundTask::Running(mut command) =
                commands.remove(task_id).expect("task existed")
            else {
                unreachable!();
            };
            drop(commands);
            ctx.process_supervisor.unregister(task_id).await;

            let exit_code = kill_child(&mut command.process).await;
            drain_reader(command.stdout_task).await;
            drain_reader(command.stderr_task).await;
            let output = output_from_buffers(exit_code, command.stdout, command.stderr).await;
            let result = background_command_result(task_id, "stopped", &output, max_output_bytes);
            BACKGROUND_COMMANDS.lock().await.insert(
                task_id.to_owned(),
                BackgroundTask::Finished {
                    status: "stopped",
                    output,
                },
            );
            Ok(result)
        }
    }
}

async fn task_snapshot(
    ctx: &ToolContext,
    tool: &str,
    task_id: &str,
    max_output_bytes: usize,
) -> Result<ToolResult, ToolError> {
    let mut commands = BACKGROUND_COMMANDS.lock().await;
    let Some(task) = commands.get_mut(task_id) else {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: format!("unknown background task `{task_id}`"),
        });
    };

    match task {
        BackgroundTask::Finished { status, output } => Ok(background_command_result(
            task_id,
            status,
            output,
            max_output_bytes,
        )),
        BackgroundTask::Running(command) => {
            let status = command.process.child.try_wait()?;
            if let Some(status) = status {
                let BackgroundTask::Running(command) =
                    commands.remove(task_id).expect("task existed")
                else {
                    unreachable!();
                };
                drop(commands);
                ctx.process_supervisor.unregister(task_id).await;
                drain_reader(command.stdout_task).await;
                drain_reader(command.stderr_task).await;
                let output =
                    output_from_buffers(status.code(), command.stdout, command.stderr).await;
                let result =
                    background_command_result(task_id, "exited", &output, max_output_bytes);
                BACKGROUND_COMMANDS.lock().await.insert(
                    task_id.to_owned(),
                    BackgroundTask::Finished {
                        status: "exited",
                        output,
                    },
                );
                Ok(result)
            } else {
                let stdout = command.stdout.clone();
                let stderr = command.stderr.clone();
                drop(commands);
                let stdout = stdout.lock_owned().await;
                let stderr = stderr.lock_owned().await;
                let output = output_from_locked_buffers(None, &stdout, &stderr);
                Ok(background_command_result(
                    task_id,
                    "running",
                    &output,
                    max_output_bytes,
                ))
            }
        }
    }
}

async fn output_from_buffers(
    exit_code: Option<i32>,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
) -> CommandOutput {
    let stdout = stdout.lock_owned().await;
    let stderr = stderr.lock_owned().await;
    output_from_locked_buffers(exit_code, &stdout, &stderr)
}

fn output_from_locked_buffers(
    exit_code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
) -> CommandOutput {
    CommandOutput {
        exit_code,
        stdout: String::from_utf8_lossy(stdout).into_owned(),
        stderr: String::from_utf8_lossy(stderr).into_owned(),
    }
}

fn background_command_result(
    task_id: &str,
    status: &str,
    output: &CommandOutput,
    max_output_bytes: usize,
) -> ToolResult {
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
        content.push_str(&format!("Command failed with exit code: {exit_label}."));
    }
    if truncated {
        if !content.ends_with('\n') && !content.is_empty() {
            content.push('\n');
        }
        content.push_str("[output truncated]");
    }
    let result = if output.exit_code == Some(0) || status == "running" {
        ToolResult::ok(content)
    } else {
        ToolResult::error(content)
    };
    result.with_details(json!({
        "task_id": task_id,
        "status": status,
        "exit_code": output.exit_code,
        "stdout": stdout_details,
        "stderr": stderr_details,
        "stdout_truncated": stdout_truncated,
        "stderr_truncated": stderr_truncated,
        "truncated": truncated,
    }))
}

async fn cleanup_background_command(handle: &str) {
    let Some(task) = BACKGROUND_COMMANDS.lock().await.remove(handle) else {
        return;
    };
    if let BackgroundTask::Running(mut command) = task {
        let _ = kill_child(&mut command.process).await;
        drain_reader(command.stdout_task).await;
        drain_reader(command.stderr_task).await;
    }
}

fn cap_plain_output(content: &str, max_bytes: usize) -> (String, bool) {
    if content.len() <= max_bytes {
        return (content.to_owned(), false);
    }
    let mut capped = String::new();
    for character in content.chars() {
        let next_len = capped.len() + character.len_utf8();
        if next_len > max_bytes {
            break;
        }
        capped.push(character);
    }
    (capped, true)
}

fn cap_output_details(content: &str, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content.to_owned();
    }
    let mut capped = String::new();
    for character in content.chars() {
        let next_len = capped.len() + character.len_utf8();
        if next_len > max_bytes {
            break;
        }
        capped.push(character);
    }
    capped
}
