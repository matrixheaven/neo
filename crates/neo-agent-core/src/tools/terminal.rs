// `effective_cwd` / `effective_cmd` share an `effective_` prefix by design —
// they are the resolved Windows-vs-Unix pair after path translation.
#![allow(clippy::similar_names)]

use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::{Arc, LazyLock, Mutex as StdMutex},
    thread::JoinHandle as ThreadJoinHandle,
};

#[cfg(windows)]
use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tokio::{
    sync::Mutex,
    task,
    time::{Duration, Instant, sleep},
};
use uuid::Uuid;

use super::bash::resolved_shell;
use super::shell_env;
use super::terminal_process::TerminalProcessTree;
use super::{
    ProcessSupervisor, Tool, ToolContext, ToolError, ToolFuture, ToolResult, ToolUpdateCallback,
    cap_output, parse_input, schema,
};

const TERMINAL_READ_MAX_WAIT: Duration = Duration::from_secs(3);
const TERMINAL_READ_QUIET_PERIOD: Duration = Duration::from_millis(50);
const TERMINAL_READ_POLL_INTERVAL: Duration = Duration::from_millis(10);
const TERMINAL_READER_DRAIN_TIMEOUT: Duration = Duration::from_millis(300);
const TERMINAL_OUTPUT_BUFFER_CAP: usize = 1024 * 1024;

#[derive(Default)]
struct TerminalUtf8Decoder {
    pending: Vec<u8>,
}

impl TerminalUtf8Decoder {
    fn push(&mut self, chunk: &[u8]) -> String {
        self.pending.extend_from_slice(chunk);
        let (output, consumed) = decode_utf8_prefix(&self.pending);
        self.pending.drain(..consumed);
        output
    }

    fn finish(&mut self) -> String {
        let output = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        output
    }
}

fn decode_utf8_prefix(bytes: &[u8]) -> (String, usize) {
    let mut output = String::new();
    let mut consumed = 0;
    while consumed < bytes.len() {
        match std::str::from_utf8(&bytes[consumed..]) {
            Ok(text) => {
                output.push_str(text);
                consumed = bytes.len();
            }
            Err(error) => {
                let valid = error.valid_up_to();
                output.push_str(
                    std::str::from_utf8(&bytes[consumed..consumed + valid])
                        .expect("UTF-8 error valid prefix must be valid UTF-8"),
                );
                consumed += valid;
                let Some(invalid_len) = error.error_len() else {
                    break;
                };
                output.push('\u{FFFD}');
                consumed += invalid_len;
            }
        }
    }
    (output, consumed)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TerminalInput {
    #[schemars(
        description = "The operation to perform: `start`, `write`, `read`, `resize`, or `stop`."
    )]
    mode: TerminalMode,
    #[schemars(
        description = "The shell command to launch in the PTY. Required when mode is `start`."
    )]
    command: Option<String>,
    #[schemars(
        description = "The session handle returned by a previous `start` call. Required for `write`, `read`, `resize`, and `stop`."
    )]
    handle: Option<String>,
    #[schemars(
        description = "Text to send to the PTY. Required when mode is `write`. Newlines are translated to carriage returns as needed."
    )]
    input: Option<String>,
    #[schemars(
        description = "Terminal width in columns. Required when mode is `resize`; optional when mode is `start` (default 80)."
    )]
    cols: Option<u16>,
    #[schemars(
        description = "Terminal height in rows. Required when mode is `resize`; optional when mode is `start` (default 24)."
    )]
    rows: Option<u16>,
    #[schemars(
        description = "Maximum number of bytes of output to return for `read` and `stop`. Defaults to the runtime output limit when omitted."
    )]
    max_output_bytes: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum TerminalMode {
    /// Launch a new PTY session running `command`.
    Start,
    /// Send `input` to the PTY session identified by `handle`.
    Write,
    /// Read buffered output from the PTY session identified by `handle`.
    Read,
    /// Resize the PTY session identified by `handle` to `cols` x `rows`.
    Resize,
    /// Stop the PTY session identified by `handle` and collect remaining output.
    Stop,
}

const DESCRIPTION: &str = r"Operate a real PTY (pseudo-terminal) session with start/write/read/resize/stop modes.

Use `Terminal` for interactive or long-running programs that need a persistent terminal (e.g. REPLs, `htop`, `less`, interactive `ssh`, `npm` prompts, or a persistent shell). For one-shot commands, prefer `Bash`.

**Modes:**
- `start`: Launch a new PTY running the given `command`. Returns a `handle` that must be used for subsequent operations. Optional `cols` and `rows` set the terminal size (default 80x24).
- `write`: Send input to the PTY. Requires `handle` and `input`. Newlines in `input` are translated to carriage returns as needed.
- `read`: Read buffered output from the PTY. Requires `handle`. Returns the output produced since the last read, the current status (`running` or `exited`), and the exit code if the process has finished. Waits briefly for new output if the process is still running.
- `resize`: Change the PTY dimensions. Requires `handle`, `cols`, and `rows`.
- `stop`: Shut down the PTY, collect any remaining output, and release the handle. Requires `handle`.

**Parameters:**
- `mode` (required): One of `start`, `write`, `read`, `resize`, `stop`.
- `command`: The shell command to launch. Required when `mode=start`.
- `handle`: The session handle returned by a previous `start`. Required for `write`, `read`, `resize`, and `stop`.
- `input`: Text to send to the PTY. Required when `mode=write`.
- `cols`: Terminal width in columns. Required when `mode=resize`; optional when `mode=start` (default 80).
- `rows`: Terminal height in rows. Required when `mode=resize`; optional when `mode=start` (default 24).
- `max_output_bytes`: Maximum bytes of output to return for `read` and `stop`. Defaults to the runtime limit.

**Output:**
The tool returns a status block with `handle`, `status`, and mode-specific fields (e.g. `exit_code`, `output`, `cols`, `rows`). Output may be truncated; a `truncated: true` marker is appended when this happens.

**Security:**
- Avoid sending secrets to the terminal.
- Do not use the terminal to modify files outside the workspace unless explicitly instructed.";

pub struct TerminalTool;

impl Tool for TerminalTool {
    fn name(&self) -> &'static str {
        "Terminal"
    }

    fn description(&self) -> &'static str {
        DESCRIPTION
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
                    read_terminal(ctx, self.name(), &handle, max_output_bytes).await
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
    process: TerminalProcessTree,
    master: Box<dyn MasterPty + Send>,
    writer: Arc<StdMutex<Box<dyn Write + Send>>>,
    output: Arc<StdMutex<TerminalOutputBuffer>>,
    read_offset: usize,
    read_lock: Arc<Mutex<()>>,
    reader_thread: Option<ReaderThread>,
    cols: u16,
    rows: u16,
    stream_callback: Arc<StdMutex<Option<ToolUpdateCallback>>>,
    stream_max_bytes: Arc<StdMutex<usize>>,
    streamed: Arc<StdMutex<usize>>,
    #[cfg(windows)]
    _launch_barrier: WindowsLaunchBarrier,
}

#[cfg(windows)]
struct WindowsLaunchBarrier {
    path: PathBuf,
}

#[cfg(windows)]
impl WindowsLaunchBarrier {
    fn new(workspace: &Path) -> Self {
        Self {
            path: workspace.join(format!(".neo-terminal-ready-{}", Uuid::new_v4())),
        }
    }

    fn wait_command(&self) -> String {
        let command = format!(
            "if exist \"{}\" exit /b 0 else exit /b 1",
            self.path.display()
        );
        format!(
            "until cmd.exe /d /c {}; do sleep 0.01; done;",
            quote_posix_shell(&command)
        )
    }

    fn release(&self) -> std::io::Result<()> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.path)
            .map(|_| ())
    }
}

#[cfg(windows)]
impl Drop for WindowsLaunchBarrier {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(windows)]
fn quote_posix_shell(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

struct ReaderThread {
    handle: ThreadJoinHandle<()>,
    done: std::sync::mpsc::Receiver<()>,
}

impl ReaderThread {
    fn join_with_timeout(self, timeout: Duration) -> Result<(), ThreadJoinHandle<()>> {
        match self.done.recv_timeout(timeout) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let _ = self.handle.join();
                Ok(())
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(self.handle),
        }
    }
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
    let shell = resolved_shell()?;
    #[cfg(windows)]
    let launch_barrier = WindowsLaunchBarrier::new(&ctx.cwd);
    // Same Windows/POSIX handling as the Bash tool: Git Bash needs a POSIX cwd
    // (passed via `cd` inside the `-lc` script, since `.cwd(windows_path)` is
    // unreliable) and `>NUL` rewritten to `>/dev/null`. On Unix the cwd is set
    // directly on the builder.
    let (effective_cwd, effective_cmd) = if shell.is_windows {
        let cwd = shell_env::GitBashCwd::new(&ctx.cwd).map_err(|err| {
            ToolError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, err))
        })?;
        let quoted_path = cwd.shell_cd();
        (
            None,
            format!(
                "cd {quoted_path} && {}",
                shell_env::rewrite_windows_nul_redirect(command)
            ),
        )
    } else {
        (Some(ctx.cwd.as_path()), command.to_owned())
    };
    #[cfg(windows)]
    let effective_cmd = format!("{} {effective_cmd}", launch_barrier.wait_command());
    let mut builder = CommandBuilder::new(&shell.shell_path);
    if shell.is_windows {
        // A non-interactive Bash sources BASH_ENV before running its command
        // body. Clearing it keeps the Job Object barrier ahead of user code.
        builder.env("BASH_ENV", "");
        builder.arg("--noprofile");
        builder.arg("--norc");
        builder.arg("-c");
    } else {
        builder.arg("-lc");
    }
    builder.arg(&effective_cmd);
    if let Some(dir) = effective_cwd {
        builder.cwd(dir);
    }
    let child = pair
        .slave
        .spawn_command(builder)
        .map_err(|err| pty_error("spawn terminal command", err))?;
    drop(pair.slave);
    #[cfg(windows)]
    let mut process = TerminalProcessTree::new(child).map_err(ToolError::Io)?;
    #[cfg(not(windows))]
    let process = TerminalProcessTree::new(child).map_err(ToolError::Io)?;
    #[cfg(windows)]
    if let Err(error) = launch_barrier.release() {
        let _ = process.terminate_and_wait();
        return Err(ToolError::Io(error));
    }

    let output = Arc::new(StdMutex::new(TerminalOutputBuffer::new(
        TERMINAL_OUTPUT_BUFFER_CAP,
    )));
    let stream_callback: Arc<StdMutex<Option<ToolUpdateCallback>>> = Arc::new(StdMutex::new(None));
    let stream_max_bytes: Arc<StdMutex<usize>> = Arc::new(StdMutex::new(0));
    let streamed: Arc<StdMutex<usize>> = Arc::new(StdMutex::new(0));
    let reader_thread = spawn_reader_thread(
        reader,
        Arc::clone(&output),
        Arc::clone(&stream_callback),
        Arc::clone(&stream_max_bytes),
        Arc::clone(&streamed),
    );
    let handle = Uuid::new_v4().to_string();
    TERMINALS.lock().await.insert(
        handle.clone(),
        TerminalSession {
            process,
            master: pair.master,
            writer: Arc::new(StdMutex::new(writer)),
            output,
            read_offset: 0,
            read_lock: Arc::new(Mutex::new(())),
            reader_thread: Some(reader_thread),
            cols,
            rows,
            stream_callback,
            stream_max_bytes,
            streamed,
            #[cfg(windows)]
            _launch_barrier: launch_barrier,
        },
    );
    register_terminal_session(ctx.process_supervisor.clone(), handle.clone()).await;
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
    output: Arc<StdMutex<TerminalOutputBuffer>>,
    stream_callback: Arc<StdMutex<Option<ToolUpdateCallback>>>,
    stream_max_bytes: Arc<StdMutex<usize>>,
    streamed: Arc<StdMutex<usize>>,
) -> ReaderThread {
    let (done_tx, done_rx) = std::sync::mpsc::sync_channel(0);
    let handle = std::thread::spawn(move || {
        let mut local = [0_u8; 8192];
        let mut decoder = TerminalUtf8Decoder::default();
        loop {
            match reader.read(&mut local) {
                Ok(0) | Err(_) => break,
                Ok(bytes_read) => {
                    let chunk = &local[..bytes_read];
                    output
                        .lock()
                        .expect("terminal output lock poisoned")
                        .push(chunk);
                    let (max, mut already_streamed) = {
                        let max = *stream_max_bytes.lock().expect("stream max lock poisoned");
                        let already = *streamed.lock().expect("streamed lock poisoned");
                        (max, already)
                    };
                    if already_streamed < max {
                        let remaining = max - already_streamed;
                        let streamed_chunk = &chunk[..chunk.len().min(remaining)];
                        already_streamed += streamed_chunk.len();
                        *streamed.lock().expect("streamed lock poisoned") = already_streamed;
                        if let Some(callback) = stream_callback
                            .lock()
                            .expect("stream callback lock poisoned")
                            .as_ref()
                        {
                            let text = decoder.push(streamed_chunk);
                            if !text.is_empty() {
                                callback(&text);
                            }
                        }
                    }
                }
            }
        }
        if let Some(callback) = stream_callback
            .lock()
            .expect("stream callback lock poisoned")
            .as_ref()
        {
            let text = decoder.finish();
            if !text.is_empty() {
                callback(&text);
            }
        }
        // Best-effort completion signal; the consumer uses a timeout so a
        // blocking read (e.g. on Windows ConPTY) won't hang the stop path.
        let _ = done_tx.send(());
    });
    ReaderThread {
        handle,
        done: done_rx,
    }
}

async fn write_terminal(tool: &str, handle: &str, input: &str) -> Result<ToolResult, ToolError> {
    let writer = {
        let terminals = TERMINALS.lock().await;
        Arc::clone(
            &terminals
                .get(handle)
                .ok_or_else(|| unknown_terminal(tool, handle))?
                .writer,
        )
    };
    let input = normalize_terminal_input_newlines(input);
    task::spawn_blocking(move || {
        let mut writer = writer
            .lock()
            .map_err(|_| ToolError::Io(std::io::Error::other("terminal writer lock poisoned")))?;
        writer.write_all(input.as_bytes()).map_err(ToolError::Io)?;
        writer.flush().map_err(ToolError::Io)
    })
    .await
    .map_err(|error| {
        ToolError::Io(std::io::Error::other(format!(
            "terminal write task: {error}"
        )))
    })??;
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
    ctx: &ToolContext,
    tool: &str,
    handle: &str,
    max_output_bytes: usize,
) -> Result<ToolResult, ToolError> {
    let read_lock = {
        let terminals = TERMINALS.lock().await;
        Arc::clone(
            &terminals
                .get(handle)
                .ok_or_else(|| unknown_terminal(tool, handle))?
                .read_lock,
        )
    };
    let _read_guard = read_lock.lock().await;

    let (initial_status, output_buffer, initial_read_offset) = {
        let mut terminals = TERMINALS.lock().await;
        let session = terminals
            .get_mut(handle)
            .ok_or_else(|| unknown_terminal(tool, handle))?;
        let status = session.process.try_wait().map_err(ToolError::Io)?;
        session
            .stream_callback
            .lock()
            .expect("stream callback lock poisoned")
            .clone_from(&ctx.tool_update);
        *session
            .stream_max_bytes
            .lock()
            .expect("stream max lock poisoned") = max_output_bytes;
        (status, Arc::clone(&session.output), session.read_offset)
    };

    if initial_status.is_none() {
        wait_for_output_quiet_period(output_buffer, initial_read_offset).await;
    }

    let mut terminals = TERMINALS.lock().await;
    let session = terminals
        .get_mut(handle)
        .ok_or_else(|| unknown_terminal(tool, handle))?;
    let status = session
        .process
        .try_wait()
        .map_err(ToolError::Io)?
        .or(initial_status);
    let read_offset_before = session.read_offset;
    let (
        output,
        read_offset_after,
        total_output_bytes,
        unread_bytes_after,
        discarded_bytes_before_read,
    ) = {
        let output = session
            .output
            .lock()
            .expect("terminal output lock poisoned");
        let read = output.read_since_limited(read_offset_before, max_output_bytes);
        session.read_offset = read.next_offset;
        (
            read.output,
            read.next_offset,
            read.total_bytes,
            read.unread_bytes_after,
            read.discarded_bytes,
        )
    };
    let output_truncated = unread_bytes_after > 0;
    let output_details = output.clone();
    let truncated = output_truncated || discarded_bytes_before_read > 0;
    let output_content = format_terminal_output(&output, truncated);
    if let Some(callback) = ctx.tool_update.as_ref() {
        callback(&output_content);
    }
    *session.streamed.lock().expect("streamed lock poisoned") = total_output_bytes;
    let status_text = status.as_ref().map_or("running", |_| "exited");
    let exit_code = status;
    Ok(ToolResult::ok(format!(
        "handle: {handle}\nstatus: {status_text}\nexit_code: {exit_code:?}\noutput:\n{output_content}"
    ))
    .with_details(json!({
        "handle": handle,
        "status": status_text,
        "exit_code": exit_code,
        "output": output_details,
        "output_truncated": output_truncated,
        "truncated": truncated,
        "read_offset_before": read_offset_before,
        "read_offset_after": read_offset_after,
        "total_output_bytes": total_output_bytes,
        "unread_bytes_after": unread_bytes_after,
        "discarded_bytes_before_read": discarded_bytes_before_read,
        "cols": session.cols,
        "rows": session.rows,
    })))
}

async fn wait_for_output_quiet_period(
    output: Arc<StdMutex<TerminalOutputBuffer>>,
    read_offset: usize,
) {
    let deadline = Instant::now() + TERMINAL_READ_MAX_WAIT;
    let mut last_len = output_len(&output);
    let mut last_change = Instant::now();

    while Instant::now() < deadline {
        sleep(TERMINAL_READ_POLL_INTERVAL).await;
        let current_len = output_len(&output);
        if current_len != last_len {
            last_len = current_len;
            last_change = Instant::now();
            continue;
        }
        if current_len > read_offset && last_change.elapsed() >= TERMINAL_READ_QUIET_PERIOD {
            break;
        }
    }
}

fn output_len(output: &StdMutex<TerminalOutputBuffer>) -> usize {
    output
        .lock()
        .expect("terminal output lock poisoned")
        .total_bytes()
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
    match stop_session_blocking(
        handle.to_owned(),
        session,
        "cancelled",
        max_output_bytes,
        ctx.tool_update.clone(),
    )
    .await
    {
        Ok(result) => {
            ctx.process_supervisor.unregister(handle).await;
            Ok(result)
        }
        Err(StopSessionFailure::Recoverable { error, session }) => {
            TERMINALS.lock().await.insert(handle.to_owned(), *session);
            register_terminal_session(ctx.process_supervisor.clone(), handle.to_owned()).await;
            Err(error)
        }
        Err(StopSessionFailure::Unrecoverable(error)) => Err(error),
    }
}

async fn register_terminal_session(supervisor: ProcessSupervisor, handle: String) {
    supervisor
        .register(handle, |handle| {
            Box::pin(async move { cleanup_terminal_session(&handle).await })
        })
        .await;
}

async fn cleanup_terminal_session(handle: &str) {
    let Some(session) = TERMINALS.lock().await.remove(handle) else {
        return;
    };
    if let Err(StopSessionFailure::Recoverable { session, .. }) =
        stop_session_blocking(handle.to_owned(), session, "cancelled", 0, None).await
    {
        // Supervisor shutdown has no error channel. Dropping the retained tree
        // performs its last-resort platform cleanup instead of leaving it live.
        drop(session);
    }
}

enum StopSessionFailure {
    Recoverable {
        error: ToolError,
        session: Box<TerminalSession>,
    },
    Unrecoverable(ToolError),
}

async fn stop_session_blocking(
    handle: String,
    session: TerminalSession,
    status: &'static str,
    max_output_bytes: usize,
    stream_callback: Option<ToolUpdateCallback>,
) -> Result<ToolResult, StopSessionFailure> {
    task::spawn_blocking(move || {
        stop_session(&handle, session, status, max_output_bytes, stream_callback)
    })
    .await
    .map_err(|error| {
        StopSessionFailure::Unrecoverable(ToolError::Io(std::io::Error::other(format!(
            "terminal stop task: {error}"
        ))))
    })?
}

fn stop_session(
    handle: &str,
    mut session: TerminalSession,
    status: &'static str,
    max_output_bytes: usize,
    stream_callback: Option<ToolUpdateCallback>,
) -> Result<ToolResult, StopSessionFailure> {
    *session
        .stream_callback
        .lock()
        .expect("stream callback lock poisoned") = stream_callback;
    *session
        .stream_max_bytes
        .lock()
        .expect("stream max lock poisoned") = max_output_bytes;
    let exit_code = match session.process.terminate_and_wait() {
        Ok(exit_code) => exit_code,
        Err(error) => {
            return Err(StopSessionFailure::Recoverable {
                error: ToolError::Io(error),
                session: Box::new(session),
            });
        }
    };
    drop(session.master);
    drop(session.writer);

    let reader_drained = if let Some(reader) = session.reader_thread.take() {
        reader
            .join_with_timeout(TERMINAL_READER_DRAIN_TIMEOUT)
            .is_ok()
    } else {
        true
    };

    let output = session
        .output
        .lock()
        .expect("terminal output lock poisoned")
        .full_output();
    let discarded_bytes_before_stop = output.discarded_bytes;
    let (output_capped, output_truncated) = cap_terminal_output(&output.output, max_output_bytes);
    let output_details = cap_output_details(&output.output, max_output_bytes);
    let truncated = output_truncated || discarded_bytes_before_stop > 0;
    let output_content = format_terminal_output(&output_capped, truncated);
    Ok(ToolResult::ok(format!(
        "handle: {handle}\nstatus: {status}\nexit_code: {exit_code:?}\noutput:\n{output_content}"
    ))
    .with_details(json!({
        "handle": handle,
        "status": status,
        "exit_code": exit_code,
        "output": output_details,
        "output_truncated": output_truncated,
        "truncated": truncated,
        "discarded_bytes_before_stop": discarded_bytes_before_stop,
        "reader_drained": reader_drained,
    })))
}

#[derive(Debug)]
struct TerminalOutputBuffer {
    bytes: Vec<u8>,
    start_offset: usize,
    total_bytes: usize,
    cap: usize,
}

#[derive(Debug, PartialEq, Eq)]
struct TerminalOutputRead {
    output: String,
    next_offset: usize,
    total_bytes: usize,
    unread_bytes_after: usize,
    discarded_bytes: usize,
}

impl TerminalOutputBuffer {
    fn new(cap: usize) -> Self {
        Self {
            bytes: Vec::new(),
            start_offset: 0,
            total_bytes: 0,
            cap: cap.max(1),
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }

        self.total_bytes = self.total_bytes.saturating_add(chunk.len());
        if chunk.len() >= self.cap {
            self.bytes.clear();
            self.bytes
                .extend_from_slice(&chunk[chunk.len() - self.cap..]);
            self.start_offset = self.total_bytes - self.bytes.len();
            return;
        }

        self.bytes.extend_from_slice(chunk);
        if self.bytes.len() > self.cap {
            let excess = self.bytes.len() - self.cap;
            self.bytes.drain(..excess);
            self.start_offset = self.start_offset.saturating_add(excess);
        }
    }

    #[cfg(test)]
    fn read_since(&self, offset: usize) -> TerminalOutputRead {
        self.read_since_limited(offset, usize::MAX)
    }

    fn read_since_limited(&self, offset: usize, max_bytes: usize) -> TerminalOutputRead {
        let effective_offset = offset.max(self.start_offset).min(self.total_bytes);
        let start_index = effective_offset.saturating_sub(self.start_offset);
        let available = self.bytes.get(start_index..).unwrap_or_default();
        let end_index = available.len().min(max_bytes);
        let (output, consumed) = decode_utf8_prefix(&available[..end_index]);
        let next_offset = effective_offset.saturating_add(consumed);
        TerminalOutputRead {
            output,
            next_offset,
            total_bytes: self.total_bytes,
            unread_bytes_after: self.total_bytes.saturating_sub(next_offset),
            discarded_bytes: self.start_offset.saturating_sub(offset),
        }
    }

    fn full_output(&self) -> TerminalOutputRead {
        TerminalOutputRead {
            output: String::from_utf8_lossy(&self.bytes).into_owned(),
            next_offset: self.total_bytes,
            total_bytes: self.total_bytes,
            unread_bytes_after: 0,
            discarded_bytes: self.start_offset,
        }
    }

    fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    #[cfg(test)]
    fn retained_len(&self) -> usize {
        self.bytes.len()
    }
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

fn cap_terminal_output(content: &str, max_bytes: usize) -> (String, bool) {
    let (content, truncated) = cap_output(content, max_bytes);
    let content = content
        .strip_suffix("\ntruncated: true")
        .or_else(|| content.strip_suffix("\ntruncated: false"))
        .unwrap_or(&content)
        .to_owned();
    (content, truncated)
}

fn format_terminal_output(content: &str, truncated: bool) -> String {
    format!("{content}\ntruncated: {truncated}")
}

fn normalize_terminal_input_newlines(input: &str) -> String {
    let mut normalized = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                normalized.push('\r');
                if chars.peek() == Some(&'\n') {
                    let _ = chars.next();
                }
            }
            '\n' => normalized.push('\r'),
            _ => normalized.push(ch),
        }
    }
    normalized
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_decoder_preserves_utf8_split_across_chunks() {
        let mut decoder = TerminalUtf8Decoder::default();

        assert_eq!(decoder.push(&[0xE4, 0xBD]), "");
        assert_eq!(decoder.push(&[0xA0, b'!']), "你!");
        assert_eq!(decoder.finish(), "");
    }

    #[test]
    fn terminal_decoder_replaces_invalid_sequences_without_consuming_incomplete_suffix() {
        let mut decoder = TerminalUtf8Decoder::default();
        let invalid = vec![0xFF; 8 * 1024];

        assert_eq!(decoder.push(&invalid), "\u{FFFD}".repeat(invalid.len()));
        assert_eq!(decoder.push(&[b'!', 0xE4, 0xBD]), "!");
        assert_eq!(decoder.push(&[0xA0]), "你");
    }

    #[test]
    fn limited_read_does_not_advance_past_incomplete_utf8() {
        let mut buffer = TerminalOutputBuffer::new(64);
        buffer.push("你".as_bytes());

        let first = buffer.read_since_limited(0, 2);
        assert_eq!(first.output, "");
        assert_eq!(first.next_offset, 0);
        assert_eq!(buffer.read_since_limited(first.next_offset, 3).output, "你");
    }

    #[test]
    fn terminal_output_buffer_discards_old_bytes_without_growing_unbounded() {
        let mut buffer = TerminalOutputBuffer::new(5);

        buffer.push(b"abcdef");
        let read = buffer.read_since(0);

        assert_eq!(buffer.retained_len(), 5);
        assert_eq!(buffer.total_bytes(), 6);
        assert_eq!(read.discarded_bytes, 1);
        assert_eq!(read.output, "bcdef");
        assert_eq!(read.next_offset, 6);
    }

    #[test]
    fn terminal_output_buffer_reads_only_new_bytes_after_offset() {
        let mut buffer = TerminalOutputBuffer::new(5);

        buffer.push(b"abc");
        let first = buffer.read_since(0);
        buffer.push(b"de");
        let second = buffer.read_since(first.next_offset);

        assert_eq!(first.output, "abc");
        assert_eq!(second.output, "de");
        assert_eq!(second.discarded_bytes, 0);
        assert_eq!(second.next_offset, 5);
    }

    #[test]
    fn terminal_output_limited_read_advances_only_returned_bytes() {
        let mut buffer = TerminalOutputBuffer::new(32);

        buffer.push(b"abcdef");
        let read = buffer.read_since_limited(0, 4);

        assert_eq!(read.output, "abcd");
        assert_eq!(read.next_offset, 4);
        assert_eq!(read.total_bytes, 6);
        assert_eq!(read.unread_bytes_after, 2);
        assert_eq!(read.discarded_bytes, 0);
    }

    #[test]
    fn terminal_output_marker_uses_combined_truncation_state() {
        let (output, output_truncated) = cap_terminal_output("tail", 64);

        assert!(!output_truncated);
        assert_eq!(
            format_terminal_output(&output, true),
            "tail\ntruncated: true"
        );
    }

    #[test]
    fn terminal_input_newlines_collapse_crlf_to_single_carriage_return() {
        assert_eq!(
            normalize_terminal_input_newlines("one\r\ntwo\nthree\rfour"),
            "one\rtwo\rthree\rfour"
        );
    }
}
