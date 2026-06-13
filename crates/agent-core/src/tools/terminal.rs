use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::{Arc, LazyLock, Mutex as StdMutex},
    thread::JoinHandle as ThreadJoinHandle,
};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tokio::{
    sync::Mutex,
    task,
    time::{Duration, Instant, sleep},
};
use uuid::Uuid;

use super::{
    ProcessKind, Tool, ToolContext, ToolError, ToolFuture, ToolResult, cap_output, parse_input,
    schema,
};

const TERMINAL_READ_SETTLE_TIMEOUT: Duration = Duration::from_millis(100);
const TERMINAL_READ_SETTLE_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TerminalInput {
    mode: TerminalMode,
    command: Option<String>,
    handle: Option<String>,
    input: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
    max_output_bytes: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum TerminalMode {
    Start,
    Write,
    Read,
    Resize,
    Stop,
}

pub struct TerminalTool;

impl Tool for TerminalTool {
    fn name(&self) -> &'static str {
        "terminal"
    }

    fn description(&self) -> &'static str {
        "Operate a real PTY terminal session with start/write/read/resize/stop modes."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<TerminalInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_shell_allowed()?;
            let input: TerminalInput = parse_input(self.name(), input)?;
            let max_output_bytes = input.max_output_bytes.unwrap_or(ctx.max_output_bytes);
            match input.mode {
                TerminalMode::Start => {
                    let command = required_field(self.name(), input.command, "command")?;
                    start_terminal(ctx, &command, input.cols, input.rows).await
                }
                TerminalMode::Write => {
                    let handle = required_field(self.name(), input.handle, "handle")?;
                    let input_text = required_field(self.name(), input.input, "input")?;
                    write_terminal(self.name(), &handle, &input_text).await
                }
                TerminalMode::Read => {
                    let handle = required_field(self.name(), input.handle, "handle")?;
                    read_terminal(self.name(), &handle, max_output_bytes).await
                }
                TerminalMode::Resize => {
                    let handle = required_field(self.name(), input.handle, "handle")?;
                    let cols = required_field(self.name(), input.cols, "cols")?;
                    let rows = required_field(self.name(), input.rows, "rows")?;
                    resize_terminal(self.name(), &handle, cols, rows).await
                }
                TerminalMode::Stop => {
                    let handle = required_field(self.name(), input.handle, "handle")?;
                    stop_terminal(ctx, self.name(), &handle, max_output_bytes).await
                }
            }
        })
    }
}

static TERMINALS: LazyLock<Mutex<HashMap<String, TerminalSession>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct TerminalSession {
    child: Box<dyn Child + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    output: Arc<StdMutex<Vec<u8>>>,
    read_offset: usize,
    reader_thread: Option<ThreadJoinHandle<()>>,
    cols: u16,
    rows: u16,
}

fn required_field<T>(tool: &str, value: Option<T>, field: &'static str) -> Result<T, ToolError> {
    value.ok_or_else(|| ToolError::InvalidInput {
        tool: tool.to_owned(),
        message: format!("missing required field `{field}`"),
    })
}

async fn start_terminal(
    ctx: &ToolContext,
    command: &str,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<ToolResult, ToolError> {
    let cols = cols.unwrap_or(80).max(1);
    let rows = rows.unwrap_or(24).max(1);
    let size = pty_size(cols, rows);
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(size)
        .map_err(|err| pty_error("open terminal PTY", err))?;
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|err| pty_error("clone terminal reader", err))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|err| pty_error("open terminal writer", err))?;
    let mut builder = CommandBuilder::new("bash");
    builder.args(["-lc", command]);
    builder.cwd(ctx.cwd.as_os_str());
    let child = pair
        .slave
        .spawn_command(builder)
        .map_err(|err| pty_error("spawn terminal command", err))?;
    drop(pair.slave);

    let output = Arc::new(StdMutex::new(Vec::new()));
    let reader_thread = spawn_reader_thread(reader, Arc::clone(&output));
    let handle = Uuid::new_v4().to_string();
    TERMINALS.lock().await.insert(
        handle.clone(),
        TerminalSession {
            child,
            master: pair.master,
            writer,
            output,
            read_offset: 0,
            reader_thread: Some(reader_thread),
            cols,
            rows,
        },
    );
    ctx.process_supervisor
        .register(handle.clone(), ProcessKind::Terminal, |handle| {
            Box::pin(async move { cleanup_terminal_session(&handle).await })
        })
        .await;
    Ok(ToolResult::ok(format!(
        "handle: {handle}\nstatus: running\ncommand: {command}\ncols: {cols}\nrows: {rows}"
    ))
    .with_details(json!({
        "handle": handle,
        "status": "running",
        "command": command,
        "cols": cols,
        "rows": rows,
    })))
}

fn spawn_reader_thread(
    mut reader: Box<dyn Read + Send>,
    output: Arc<StdMutex<Vec<u8>>>,
) -> ThreadJoinHandle<()> {
    std::thread::spawn(move || {
        let mut local = [0_u8; 8192];
        loop {
            match reader.read(&mut local) {
                Ok(0) | Err(_) => break,
                Ok(bytes_read) => output
                    .lock()
                    .expect("terminal output lock poisoned")
                    .extend_from_slice(&local[..bytes_read]),
            }
        }
    })
}

async fn write_terminal(tool: &str, handle: &str, input: &str) -> Result<ToolResult, ToolError> {
    let mut terminals = TERMINALS.lock().await;
    let session = terminals
        .get_mut(handle)
        .ok_or_else(|| unknown_terminal(tool, handle))?;
    let input = input.replace('\n', "\r");
    session
        .writer
        .write_all(input.as_bytes())
        .map_err(ToolError::Io)?;
    session.writer.flush().map_err(ToolError::Io)?;
    Ok(
        ToolResult::ok(format!("handle: {handle}\nstatus: running\nwritten: true")).with_details(
            json!({
                "handle": handle,
                "status": "running",
                "written": true,
            }),
        ),
    )
}

async fn read_terminal(
    tool: &str,
    handle: &str,
    max_output_bytes: usize,
) -> Result<ToolResult, ToolError> {
    let mut terminals = TERMINALS.lock().await;
    let session = terminals
        .get_mut(handle)
        .ok_or_else(|| unknown_terminal(tool, handle))?;
    let status = session.child.try_wait().map_err(ToolError::Io)?;
    if status.is_none() {
        wait_for_fresh_output(Arc::clone(&session.output), session.read_offset).await;
    }
    let output = {
        let output = session
            .output
            .lock()
            .expect("terminal output lock poisoned");
        let output_slice = output
            .get(session.read_offset..)
            .ok_or_else(|| unknown_terminal(tool, handle))?;
        session.read_offset = output.len();
        String::from_utf8_lossy(output_slice).into_owned()
    };
    let (output_capped, output_truncated) = cap_output(&output, max_output_bytes);
    let output_details = cap_output_details(&output, max_output_bytes);
    let status_text = status.as_ref().map_or("running", |_| "exited");
    let exit_code = status.map(|status| i32::try_from(status.exit_code()).unwrap_or(i32::MAX));
    Ok(ToolResult::ok(format!(
        "handle: {handle}\nstatus: {status_text}\nexit_code: {exit_code:?}\noutput:\n{output_capped}"
    ))
    .with_details(json!({
        "handle": handle,
        "status": status_text,
        "exit_code": exit_code,
        "output": output_details,
        "output_truncated": output_truncated,
        "truncated": output_truncated,
    })))
}

async fn wait_for_fresh_output(output: Arc<StdMutex<Vec<u8>>>, read_offset: usize) {
    if has_fresh_output(&output, read_offset) {
        return;
    }
    let deadline = Instant::now() + TERMINAL_READ_SETTLE_TIMEOUT;
    while Instant::now() < deadline {
        sleep(TERMINAL_READ_SETTLE_INTERVAL).await;
        if has_fresh_output(&output, read_offset) {
            break;
        }
    }
}

fn has_fresh_output(output: &StdMutex<Vec<u8>>, read_offset: usize) -> bool {
    output.lock().expect("terminal output lock poisoned").len() > read_offset
}

async fn resize_terminal(
    tool: &str,
    handle: &str,
    cols: u16,
    rows: u16,
) -> Result<ToolResult, ToolError> {
    let cols = cols.max(1);
    let rows = rows.max(1);
    let mut terminals = TERMINALS.lock().await;
    let session = terminals
        .get_mut(handle)
        .ok_or_else(|| unknown_terminal(tool, handle))?;
    session
        .master
        .resize(pty_size(cols, rows))
        .map_err(|err| pty_error("resize terminal PTY", err))?;
    session.cols = cols;
    session.rows = rows;
    Ok(ToolResult::ok(format!(
        "handle: {handle}\nstatus: running\ncols: {cols}\nrows: {rows}"
    ))
    .with_details(json!({
        "handle": handle,
        "status": "running",
        "cols": cols,
        "rows": rows,
    })))
}

async fn stop_terminal(
    ctx: &ToolContext,
    tool: &str,
    handle: &str,
    max_output_bytes: usize,
) -> Result<ToolResult, ToolError> {
    let session = TERMINALS
        .lock()
        .await
        .remove(handle)
        .ok_or_else(|| unknown_terminal(tool, handle))?;
    ctx.process_supervisor.unregister(handle).await;
    Ok(stop_session_blocking(handle.to_owned(), session, "stopped", max_output_bytes).await)
}

async fn cleanup_terminal_session(handle: &str) {
    let Some(session) = TERMINALS.lock().await.remove(handle) else {
        return;
    };
    let _ = stop_session_blocking(handle.to_owned(), session, "stopped", 0).await;
}

async fn stop_session_blocking(
    handle: String,
    session: TerminalSession,
    status: &'static str,
    max_output_bytes: usize,
) -> ToolResult {
    task::spawn_blocking(move || stop_session(handle, session, status, max_output_bytes))
        .await
        .expect("terminal stop blocking task should not panic")
}

fn stop_session(
    handle: String,
    mut session: TerminalSession,
    status: &'static str,
    max_output_bytes: usize,
) -> ToolResult {
    drop(session.writer);
    drop(session.master);
    let _ = session.child.kill();
    let exit_status = session.child.wait().ok();
    if let Some(reader_thread) = session.reader_thread.take() {
        let _ = reader_thread.join();
    }
    let output = session
        .output
        .lock()
        .expect("terminal output lock poisoned")
        .clone();
    let output = String::from_utf8_lossy(&output).into_owned();
    let (output_capped, output_truncated) = cap_output(&output, max_output_bytes);
    let output_details = cap_output_details(&output, max_output_bytes);
    let exit_code = exit_status.map(|status| i32::try_from(status.exit_code()).unwrap_or(i32::MAX));
    ToolResult::ok(format!(
        "handle: {handle}\nstatus: {status}\nexit_code: {exit_code:?}\noutput:\n{output_capped}"
    ))
    .with_details(json!({
        "handle": handle,
        "status": status,
        "exit_code": exit_code,
        "output": output_details,
        "output_truncated": output_truncated,
        "truncated": output_truncated,
    }))
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

fn unknown_terminal(tool: &str, handle: &str) -> ToolError {
    ToolError::InvalidInput {
        tool: tool.to_owned(),
        message: format!("unknown terminal handle `{handle}`"),
    }
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn pty_error(operation: &str, err: impl std::fmt::Display) -> ToolError {
    ToolError::Io(std::io::Error::other(format!("{operation}: {err}")))
}
