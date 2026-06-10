use std::{
    collections::HashMap,
    process::Stdio,
    sync::{Arc, LazyLock},
    time::Duration,
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
    ProcessKind, Tool, ToolContext, ToolError, ToolFuture, ToolResult, cap_output, parse_input,
    schema,
};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BashInput {
    mode: Option<BashMode>,
    command: Option<String>,
    handle: Option<String>,
    timeout_ms: Option<u64>,
    max_output_bytes: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum BashMode {
    Foreground,
    Start,
    Poll,
    Stop,
}

pub struct BashTool;

impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn description(&self) -> &'static str {
        "Run a shell command in the workspace, or start/poll/stop a compact background command."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<BashInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_shell_allowed()?;
            let input: BashInput = parse_input(self.name(), input)?;
            let max_output_bytes = input.max_output_bytes.unwrap_or(ctx.max_output_bytes);
            match input.mode.unwrap_or(BashMode::Foreground) {
                BashMode::Foreground => {
                    let command = required_field(self.name(), input.command, "command")?;
                    let timeout_ms = input.timeout_ms.unwrap_or_else(|| {
                        u64::try_from(ctx.bash_timeout.as_millis()).unwrap_or(u64::MAX)
                    });
                    let output =
                        run_command(ctx, &command, Duration::from_millis(timeout_ms)).await?;
                    Ok(command_result(&output, max_output_bytes))
                }
                BashMode::Start => {
                    let command = required_field(self.name(), input.command, "command")?;
                    start_background_command(ctx, &command, max_output_bytes).await
                }
                BashMode::Poll => {
                    let handle = required_field(self.name(), input.handle, "handle")?;
                    poll_background_command(ctx, self.name(), &handle, max_output_bytes).await
                }
                BashMode::Stop => {
                    reject_field(self.name(), input.command.is_some(), "command")?;
                    let handle = required_field(self.name(), input.handle, "handle")?;
                    stop_background_command(ctx, self.name(), &handle, max_output_bytes).await
                }
            }
        })
    }
}

static BACKGROUND_COMMANDS: LazyLock<Mutex<HashMap<String, BackgroundCommand>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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

fn required_field<T>(tool: &str, value: Option<T>, field: &'static str) -> Result<T, ToolError> {
    value.ok_or_else(|| ToolError::InvalidInput {
        tool: tool.to_owned(),
        message: format!("missing required field `{field}`"),
    })
}

fn reject_field(tool: &str, present: bool, field: &'static str) -> Result<(), ToolError> {
    if present {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: format!("field `{field}` is not supported for this mode"),
        });
    }
    Ok(())
}

fn command_result(output: &CommandOutput, max_output_bytes: usize) -> ToolResult {
    let (stdout_capped, stdout_truncated) = cap_output(&output.stdout, max_output_bytes);
    let (stderr_capped, stderr_truncated) = cap_output(&output.stderr, max_output_bytes);
    let stdout_details = cap_output_details(&output.stdout, max_output_bytes);
    let stderr_details = cap_output_details(&output.stderr, max_output_bytes);
    let truncated = stdout_truncated || stderr_truncated;
    let combined = format!(
        "exit_code: {:?}\nstdout:\n{}\nstderr:\n{}",
        output.exit_code, stdout_capped, stderr_capped
    );
    ToolResult::ok(format!("{combined}\ntruncated: {truncated}")).with_details(json!({
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
    timeout_duration: Duration,
) -> Result<CommandOutput, ToolError> {
    let mut process = spawn_bash_process(ctx, command)?;
    let mut stdout = process.child.stdout.take().expect("stdout was piped");
    let mut stderr = process.child.stderr.take().expect("stderr was piped");

    let stdout_task = tokio::spawn(async move {
        let mut buffer = Vec::new();
        stdout.read_to_end(&mut buffer).await?;
        Ok::<_, std::io::Error>(String::from_utf8_lossy(&buffer).into_owned())
    });
    let stderr_task = tokio::spawn(async move {
        let mut buffer = Vec::new();
        stderr.read_to_end(&mut buffer).await?;
        Ok::<_, std::io::Error>(String::from_utf8_lossy(&buffer).into_owned())
    });

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

    let stdout = stdout_task.await.map_err(std::io::Error::other)??;
    let stderr = stderr_task.await.map_err(std::io::Error::other)??;
    Ok(CommandOutput {
        exit_code: status.code(),
        stdout,
        stderr,
    })
}

fn spawn_bash_process(ctx: &ToolContext, command_text: &str) -> Result<ManagedChild, ToolError> {
    let mut process_command = Command::new("bash");
    process_command
        .arg("-lc")
        .arg(command_text)
        .current_dir(&ctx.cwd)
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
    max_output_bytes: usize,
) -> Result<ToolResult, ToolError> {
    let mut process = spawn_bash_process(ctx, command)?;

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

    let handle = Uuid::new_v4().to_string();
    BACKGROUND_COMMANDS.lock().await.insert(
        handle.clone(),
        BackgroundCommand {
            process,
            stdout,
            stderr,
            stdout_task,
            stderr_task,
        },
    );
    ctx.process_supervisor
        .register(handle.clone(), ProcessKind::BashBackground, |handle| {
            Box::pin(async move { cleanup_background_command(&handle).await })
        })
        .await;

    Ok(
        ToolResult::ok(format!("started background command: {handle}")).with_details(json!({
            "handle": handle,
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

async fn poll_background_command(
    ctx: &ToolContext,
    tool: &str,
    handle: &str,
    max_output_bytes: usize,
) -> Result<ToolResult, ToolError> {
    let mut commands = BACKGROUND_COMMANDS.lock().await;
    let command = commands
        .get_mut(handle)
        .ok_or_else(|| ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: format!("unknown background handle `{handle}`"),
        })?;

    let status = command.process.child.try_wait()?;
    if let Some(status) = status {
        let command = commands.remove(handle).expect("command existed");
        drop(commands);
        ctx.process_supervisor.unregister(handle).await;
        let _ = command.stdout_task.await;
        let _ = command.stderr_task.await;
        let output = output_from_buffers(status.code(), command.stdout, command.stderr).await;
        return Ok(background_command_result(
            handle,
            "exited",
            &output,
            max_output_bytes,
        ));
    }

    let stdout = command.stdout.clone();
    let stderr = command.stderr.clone();
    drop(commands);
    let stdout = stdout.lock_owned().await;
    let stderr = stderr.lock_owned().await;
    let output = output_from_locked_buffers(None, &stdout, &stderr);
    Ok(background_command_result(
        handle,
        "running",
        &output,
        max_output_bytes,
    ))
}

async fn stop_background_command(
    ctx: &ToolContext,
    tool: &str,
    handle: &str,
    max_output_bytes: usize,
) -> Result<ToolResult, ToolError> {
    let mut commands = BACKGROUND_COMMANDS.lock().await;
    let mut command = commands
        .remove(handle)
        .ok_or_else(|| ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: format!("unknown background handle `{handle}`"),
        })?;
    drop(commands);
    ctx.process_supervisor.unregister(handle).await;

    let exit_code = kill_child(&mut command.process).await;
    let _ = command.stdout_task.await;
    let _ = command.stderr_task.await;
    let output = output_from_buffers(exit_code, command.stdout, command.stderr).await;
    Ok(background_command_result(
        handle,
        "stopped",
        &output,
        max_output_bytes,
    ))
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
    handle: &str,
    status: &str,
    output: &CommandOutput,
    max_output_bytes: usize,
) -> ToolResult {
    let (stdout_capped, stdout_truncated) = cap_output(&output.stdout, max_output_bytes);
    let (stderr_capped, stderr_truncated) = cap_output(&output.stderr, max_output_bytes);
    let stdout_details = cap_output_details(&output.stdout, max_output_bytes);
    let stderr_details = cap_output_details(&output.stderr, max_output_bytes);
    let truncated = stdout_truncated || stderr_truncated;
    let content = format!(
        "handle: {handle}\nstatus: {status}\nexit_code: {:?}\nstdout:\n{}\nstderr:\n{}\ntruncated: {truncated}",
        output.exit_code, stdout_capped, stderr_capped
    );
    ToolResult::ok(content).with_details(json!({
        "handle": handle,
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
    let Some(mut command) = BACKGROUND_COMMANDS.lock().await.remove(handle) else {
        return;
    };
    let _ = kill_child(&mut command.process).await;
    let _ = command.stdout_task.await;
    let _ = command.stderr_task.await;
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
