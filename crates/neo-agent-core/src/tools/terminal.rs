use std::{sync::Arc, time::Duration};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tokio::{sync::Mutex, time::Instant};
use uuid::Uuid;

use super::shell_guard::{
    GuardStatusKind, GuardedCommandResult, GuardianClient, ShellAdmissionClass,
    ShellAdmissionEvent, ShellAdmissionRequest, TerminalClientSession, TerminalClientState,
};
use super::{
    Tool, ToolContext, ToolError, ToolFuture, ToolResult, format_exit_code, parse_input,
    parse_shell_timeout_secs, schema,
};
use crate::session::MAIN_AGENT_ID;

const TERMINAL_START_WRITE_YIELD: Duration = Duration::from_millis(250);
const TERMINAL_READ_YIELD: Duration = Duration::from_secs(3);
const TERMINAL_MAX_YIELD_MS: u64 = 30_000;
const TERMINAL_READ_QUIET_PERIOD: Duration = Duration::from_millis(50);
const TERMINAL_READ_POLL_INTERVAL: Duration = Duration::from_millis(10);
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TerminalInput {
    #[schemars(description = "The operation: start, write, read, resize, or stop.")]
    mode: TerminalMode,
    #[schemars(description = "Command to launch. Required for start.")]
    command: Option<String>,
    #[schemars(description = "Terminal handle. Required except for start.")]
    handle: Option<String>,
    #[schemars(
        description = "Input text. Required for write. Control keys must decode to one control character: Ctrl+C is U+0003, Ctrl+D is U+0004, and Ctrl+Z is U+001A. Do not send printable escape text such as backslash-u-0-0-0-3. These are raw PTY inputs, not portable signal guarantees."
    )]
    input: Option<String>,
    #[schemars(
        description = "Working directory for the launched process. Only valid for start; rejected for other modes. Relative paths resolve against the session working directory. Supply it whenever the command works inside a nested project subtree: command text is never inspected for paths, so nested AGENTS.md instructions load only from this typed cwd."
    )]
    cwd: Option<String>,
    #[schemars(description = "Terminal columns. Defaults to 80 for start.")]
    cols: Option<u16>,
    #[schemars(description = "Terminal rows. Defaults to 24 for start.")]
    rows: Option<u16>,
    #[schemars(
        description = "Optional execution timeout in seconds. Omit this field to allow the command to run until it finishes or is cancelled. When set, use a value from 300 seconds (5 minutes) to 3600 seconds (1 hour). For long-running or uncertain-duration work, prefer omission instead of guessing a deadline. Valid only for mode=start.",
        range(min = 300, max = 3600)
    )]
    timeout_secs: Option<u64>,
    #[schemars(
        description = "Wait for incremental PTY output after start/write or while reading. The clock starts only after admission and operation readiness; expiry returns current output with status running and never stops the command. Defaults: 250 ms for start/write, 3000 ms for read. Valid only for start/write/read. Range 0..=30000; 0 snapshots immediately after the operation is ready.",
        range(min = 0, max = 30000)
    )]
    yield_time_ms: Option<u64>,
    #[schemars(description = "Maximum output bytes for start, write, read, and stop.")]
    max_output_bytes: Option<usize>,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum TerminalMode {
    Start,
    Write,
    Read,
    Resize,
    Stop,
}

fn terminal_yield(mode: TerminalMode, requested: Option<u64>) -> Duration {
    requested.map_or_else(
        || match mode {
            TerminalMode::Start | TerminalMode::Write => TERMINAL_START_WRITE_YIELD,
            TerminalMode::Read => TERMINAL_READ_YIELD,
            TerminalMode::Resize | TerminalMode::Stop => Duration::ZERO,
        },
        Duration::from_millis,
    )
}

const DESCRIPTION: &str = r"Operate a real PTY session with start/write/read/resize/stop modes.

Use Terminal for interactive or persistent commands; use Bash for one-shot commands. Start returns a handle plus any incremental raw PTY output collected during a short yield window. Write sends input, including single control characters such as Ctrl+C (U+0003), Ctrl+D (U+0004), and Ctrl+Z (U+001A); printable escape text is sent literally, and control input has no portable signal guarantee. Read returns output since the prior observation. Resize changes PTY dimensions, and Stop terminates the full process tree. Newlines sent by Write are translated to carriage returns. Output is raw PTY bytes (echo, ANSI, CR, backspace, cursor control are not filtered) and is bounded by the runtime limit.

`yield_time_ms` is valid only for start/write/read (defaults 250/250/3000 ms, range 0..=30000). The yield clock starts only after admission and operation readiness; expiry returns current output with status running and never stops the command. Omit `timeout_secs` for unlimited command lifetime.

`cwd` is accepted only for start and sets the launched process's working directory. When the command works inside a nested project subtree, you must set `cwd` to that subtree: the command string is never parsed for paths, so nested AGENTS.md instructions apply only when the typed `cwd` (or path) argument names the subtree.";

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
            if input.cwd.is_some() && input.mode != TerminalMode::Start {
                return Err(ToolError::InvalidInput {
                    tool: self.name().to_owned(),
                    message: "`cwd` is only valid for mode `start`".to_owned(),
                });
            }
            if input.timeout_secs.is_some() && input.mode != TerminalMode::Start {
                return Err(ToolError::InvalidInput {
                    tool: self.name().to_owned(),
                    message: "timeout_secs is valid only for start".to_owned(),
                });
            }
            if let Some(yield_time_ms) = input.yield_time_ms {
                if !matches!(
                    input.mode,
                    TerminalMode::Start | TerminalMode::Write | TerminalMode::Read
                ) {
                    return Err(ToolError::InvalidInput {
                        tool: self.name().to_owned(),
                        message: "yield_time_ms is valid only for start, write, and read"
                            .to_owned(),
                    });
                }
                if yield_time_ms > TERMINAL_MAX_YIELD_MS {
                    return Err(ToolError::InvalidInput {
                        tool: self.name().to_owned(),
                        message: format!(
                            "yield_time_ms must be between 0 and {TERMINAL_MAX_YIELD_MS}"
                        ),
                    });
                }
            }
            let timeout = parse_shell_timeout_secs(self.name(), input.timeout_secs)?;
            let max_output_bytes = input
                .max_output_bytes
                .unwrap_or(ctx.max_output_bytes)
                .min(ctx.shell_runtime.limits().max_output_bytes);
            let yield_for = terminal_yield(input.mode, input.yield_time_ms);
            match input.mode {
                TerminalMode::Start => {
                    start_terminal(
                        ctx,
                        &required_field(self.name(), input.command, "command")?,
                        input.cwd.as_deref(),
                        input.cols,
                        input.rows,
                        timeout,
                        max_output_bytes,
                        yield_for,
                    )
                    .await
                }
                TerminalMode::Write => {
                    write_terminal(
                        ctx,
                        self.name(),
                        &required_field(self.name(), input.handle, "handle")?,
                        &required_field(self.name(), input.input, "input")?,
                        max_output_bytes,
                        yield_for,
                    )
                    .await
                }
                TerminalMode::Read => {
                    read_terminal(
                        ctx,
                        self.name(),
                        &required_field(self.name(), input.handle, "handle")?,
                        max_output_bytes,
                        yield_for,
                    )
                    .await
                }
                TerminalMode::Resize => {
                    resize_terminal(
                        ctx,
                        self.name(),
                        &required_field(self.name(), input.handle, "handle")?,
                        required_field(self.name(), input.cols, "cols")?,
                        required_field(self.name(), input.rows, "rows")?,
                    )
                    .await
                }
                TerminalMode::Stop => {
                    stop_terminal(
                        ctx,
                        self.name(),
                        &required_field(self.name(), input.handle, "handle")?,
                        max_output_bytes,
                    )
                    .await
                }
            }
        })
    }
}

async fn start_terminal(
    ctx: &ToolContext,
    command: &str,
    cwd: Option<&str>,
    cols: Option<u16>,
    rows: Option<u16>,
    timeout: Option<Duration>,
    max_output_bytes: usize,
    yield_for: Duration,
) -> Result<ToolResult, ToolError> {
    let cols = cols.unwrap_or(80).max(1);
    let rows = rows.unwrap_or(24).max(1);
    let handle = Uuid::new_v4().to_string();
    let task_id = format!("terminal-{handle}");
    let status_dir = ctx.background_tasks.persistence_dir().map_or(
        ctx.shell_runtime.runtime_root(),
        std::path::PathBuf::as_path,
    );
    // Pre-resolve so invalid paths fail before admission wait.
    let _ = match cwd {
        Some(path) => ctx.resolve_workspace_path(std::path::Path::new(path))?,
        None => ctx.cwd.clone(),
    };
    let admission = ShellAdmissionRequest {
        owner: ctx
            .agent_id
            .clone()
            .unwrap_or_else(|| MAIN_AGENT_ID.to_owned()),
        class: ShellAdmissionClass::AgentBackground,
    };
    let permit = tokio::select! {
        permit = ctx.shell_runtime.acquire(admission, ctx.shell_admission_callback.clone()) => permit,
        () = ctx.cancel_token.cancelled() => return Err(ToolError::Cancelled),
    };
    if ctx.cancel_token.is_cancelled() {
        drop(permit);
        return Err(ToolError::Cancelled);
    }
    ctx.ensure_shell_allowed()?;
    // Re-resolve after grant so containment cannot drift during queue wait.
    let cwd = match cwd {
        Some(path) => ctx.resolve_workspace_path(std::path::Path::new(path))?,
        None => ctx.cwd.clone(),
    };
    if let Some(callback) = &ctx.shell_admission_callback {
        callback(ShellAdmissionEvent::Started);
    }
    let client = GuardianClient::start_terminal(
        &ctx.shell_runtime,
        task_id,
        command.to_owned(),
        &cwd,
        status_dir,
        cols,
        rows,
        timeout,
        permit,
    )
    .await?;
    let guardian_pid = client.guardian_pid;
    let command_pid = client.command_pid;
    let session = TerminalClientSession {
        client,
        state: Arc::new(Mutex::new(TerminalClientState {
            read_offset: 0,
            cols,
            rows,
        })),
        read_lock: Arc::new(Mutex::new(())),
    };
    ctx.shell_runtime
        .insert_terminal(handle.clone(), session.clone())
        .await;
    let collected = collect_terminal_output(
        ctx,
        TerminalMode::Start,
        &handle,
        &session,
        max_output_bytes,
        yield_for,
    )
    .await;
    match collected {
        Ok(mut result) => {
            if let Some(details) = result.details.as_mut() {
                details["command"] = json!(command);
                details["guardian_pid"] = json!(guardian_pid);
                details["command_pid"] = json!(command_pid);
            }
            Ok(result)
        }
        Err(error) => {
            if let Some(session) = ctx.shell_runtime.remove_terminal(&handle).await {
                let _ = session.client.stop().await;
            }
            Err(error)
        }
    }
}

async fn write_terminal(
    ctx: &ToolContext,
    tool: &str,
    handle: &str,
    input: &str,
    max_output_bytes: usize,
    yield_for: Duration,
) -> Result<ToolResult, ToolError> {
    let session = terminal_session(ctx, tool, handle).await?;
    session
        .client
        .write_terminal(normalize_terminal_input_newlines(input).as_bytes())
        .await?;
    let mut result = collect_terminal_output(
        ctx,
        TerminalMode::Write,
        handle,
        &session,
        max_output_bytes,
        yield_for,
    )
    .await?;
    if let Some(details) = result.details.as_mut() {
        details["written"] = json!(true);
    }
    Ok(result)
}

async fn read_terminal(
    ctx: &ToolContext,
    tool: &str,
    handle: &str,
    max_output_bytes: usize,
    yield_for: Duration,
) -> Result<ToolResult, ToolError> {
    let session = terminal_session(ctx, tool, handle).await?;
    collect_terminal_output(
        ctx,
        TerminalMode::Read,
        handle,
        &session,
        max_output_bytes,
        yield_for,
    )
    .await
}

async fn collect_terminal_output(
    ctx: &ToolContext,
    mode: TerminalMode,
    handle: &str,
    session: &TerminalClientSession,
    max_output_bytes: usize,
    yield_for: Duration,
) -> Result<ToolResult, ToolError> {
    let _read = session.read_lock.lock().await;
    let read_offset = session.state.lock().await.read_offset;

    if !yield_for.is_zero() && session.client.final_result().is_none() {
        let started_at = Instant::now();
        let deadline = started_at + yield_for;
        // Raw PTY bootstrap bytes can precede child output on ConPTY.
        let quiet_not_before = started_at
            + if mode == TerminalMode::Start {
                yield_for.min(TERMINAL_START_WRITE_YIELD)
            } else {
                Duration::ZERO
            };
        let mut last_total = 0;
        let mut last_change = started_at;
        while Instant::now() < deadline {
            if ctx.cancel_token.is_cancelled() {
                return Err(ToolError::Cancelled);
            }
            let snapshot = match session.client.read_terminal(read_offset, 0).await {
                Ok(snapshot) => snapshot,
                Err(_) if session.client.final_result().is_some() => break,
                Err(error) => return Err(error),
            };
            if snapshot.total != last_total {
                last_total = snapshot.total;
                last_change = Instant::now();
            } else if snapshot.total > read_offset
                && last_change.elapsed() >= TERMINAL_READ_QUIET_PERIOD
                && Instant::now() >= quiet_not_before
            {
                break;
            }
            if session.client.final_result().is_some() {
                break;
            }
            tokio::select! {
                () = ctx.cancel_token.cancelled() => return Err(ToolError::Cancelled),
                () = tokio::time::sleep(TERMINAL_READ_POLL_INTERVAL) => {}
            }
        }
    }

    if ctx.cancel_token.is_cancelled() {
        return Err(ToolError::Cancelled);
    }

    let (snapshot, final_result) = if let Some(result) = session.client.final_result() {
        (
            terminal_snapshot_from_final(&result, read_offset, max_output_bytes),
            Some(result),
        )
    } else {
        match session
            .client
            .read_terminal(read_offset, max_output_bytes)
            .await
        {
            Ok(snapshot) => (snapshot, session.client.final_result()),
            Err(error) => {
                let Some(result) = session.client.final_result() else {
                    return Err(error);
                };
                (
                    terminal_snapshot_from_final(&result, read_offset, max_output_bytes),
                    Some(result),
                )
            }
        }
    };
    let mut state = session.state.lock().await;
    state.read_offset = snapshot.offset;
    let cols = state.cols;
    let rows = state.rows;
    drop(state);
    let output = String::from_utf8_lossy(&snapshot.data).into_owned();
    let unread = snapshot.total.saturating_sub(snapshot.offset);
    let truncated = unread > 0 || snapshot.discarded > 0;
    let content = format_terminal_output(&output, truncated);
    if let Some(callback) = &ctx.tool_update {
        callback(&content);
    }
    let status = final_result
        .as_ref()
        .map_or("running", |result| guard_status_text(result.exit.status));
    let exit_code = final_result
        .as_ref()
        .and_then(|result| result.exit.exit_code);
    let signal = final_result.as_ref().and_then(|result| result.exit.signal);
    let exit_code_text = format_exit_code(exit_code, signal);
    let resource_limit = final_result
        .as_ref()
        .and_then(|result| result.exit.resource_limit.as_ref());
    let mut details = json!({
        "handle": handle,
        "status": status,
        "exit_code": exit_code,
        "output": output,
        "output_truncated": unread > 0,
        "truncated": truncated,
        "read_offset_before": read_offset,
        "read_offset_after": snapshot.offset,
        "total_output_bytes": snapshot.total,
        "unread_bytes_after": unread,
        "discarded_bytes_before_read": snapshot.discarded,
        "cols": cols,
        "rows": rows,
    });
    if let Some(limit) = resource_limit {
        details["resource_limit"] = json!(limit);
    }
    Ok(ToolResult::ok(format!(
        "handle: {handle}\nstatus: {status}\nexit_code: {exit_code_text}\noutput:\n{content}"
    ))
    .with_details(details))
}

async fn resize_terminal(
    ctx: &ToolContext,
    tool: &str,
    handle: &str,
    cols: u16,
    rows: u16,
) -> Result<ToolResult, ToolError> {
    let session = terminal_session(ctx, tool, handle).await?;
    let cols = cols.max(1);
    let rows = rows.max(1);
    session.client.resize_terminal(cols, rows).await?;
    let mut state = session.state.lock().await;
    state.cols = cols;
    state.rows = rows;
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
    let session = ctx
        .shell_runtime
        .remove_terminal(handle)
        .await
        .ok_or_else(|| unknown_terminal(tool, handle))?;
    let result = session.client.stop().await;
    let status = guard_status_text(result.exit.status);
    let output = String::from_utf8_lossy(&result.output.stdout).into_owned();
    let (output, capped) = cap_utf8(&output, max_output_bytes);
    let truncated = capped || result.exit.omitted_output_bytes > 0;
    let content = format_terminal_output(&output, truncated);
    if let Some(callback) = &ctx.tool_update {
        callback(&content);
    }
    let exit_code_text = format_exit_code(result.exit.exit_code, result.exit.signal);
    Ok(ToolResult::ok(format!(
        "handle: {handle}\nstatus: {status}\nexit_code: {exit_code_text}\noutput:\n{content}"
    ))
    .with_details(json!({
        "handle": handle,
        "status": status,
        "exit_code": result.exit.exit_code,
        "output": output,
        "output_truncated": capped,
        "truncated": truncated,
        "reader_drained": true,
    })))
}

fn terminal_snapshot_from_final(
    result: &GuardedCommandResult,
    offset: u64,
    max_bytes: usize,
) -> super::shell_guard::TerminalSnapshot {
    let start = result.exit.omitted_output_bytes;
    let total = start.saturating_add(u64::try_from(result.output.stdout.len()).unwrap_or(u64::MAX));
    let effective = offset.max(start).min(total);
    let index = usize::try_from(effective.saturating_sub(start)).unwrap_or(usize::MAX);
    let available = result.output.stdout.get(index..).unwrap_or_default();
    let retained = available.len().min(max_bytes);
    let mut retained = retained;
    while retained > 0 && std::str::from_utf8(&available[..retained]).is_err() {
        retained -= 1;
    }
    let next = effective.saturating_add(u64::try_from(retained).unwrap_or(u64::MAX));
    super::shell_guard::TerminalSnapshot {
        offset: next,
        total,
        discarded: start.saturating_sub(offset),
        data: available[..retained].to_vec(),
    }
}

const fn guard_status_text(status: GuardStatusKind) -> &'static str {
    match status {
        GuardStatusKind::Completed => "completed",
        GuardStatusKind::Failed => "failed",
        GuardStatusKind::Cancelled => "cancelled",
        GuardStatusKind::TimedOut => "timed_out",
        GuardStatusKind::ResourceLimited => "resource_limited",
        GuardStatusKind::ParentExited => "parent_exited",
    }
}

async fn terminal_session(
    ctx: &ToolContext,
    tool: &str,
    handle: &str,
) -> Result<TerminalClientSession, ToolError> {
    ctx.shell_runtime
        .terminal(handle)
        .await
        .ok_or_else(|| unknown_terminal(tool, handle))
}

fn required_field<T>(tool: &str, value: Option<T>, field: &'static str) -> Result<T, ToolError> {
    value.ok_or_else(|| ToolError::InvalidInput {
        tool: tool.to_owned(),
        message: format!("missing required field `{field}`"),
    })
}

fn unknown_terminal(tool: &str, handle: &str) -> ToolError {
    ToolError::InvalidInput {
        tool: tool.to_owned(),
        message: format!("unknown terminal handle `{handle}`"),
    }
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

fn cap_utf8(content: &str, max_bytes: usize) -> (String, bool) {
    if content.len() <= max_bytes {
        return (content.to_owned(), false);
    }
    let mut end = max_bytes.min(content.len());
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    (content[..end].to_owned(), true)
}

fn format_terminal_output(content: &str, truncated: bool) -> String {
    format!("{content}\ntruncated: {truncated}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn terminal_input_preserves_control_bytes_and_normalizes_line_endings() {
        assert_eq!(
            normalize_terminal_input_newlines("\0\u{1}\u{3}\u{4}\t\n\r\n\u{1a}\u{1b}\u{7f}")
                .as_bytes(),
            b"\0\x01\x03\x04\t\r\r\x1a\x1b\x7f"
        );
        assert_eq!(normalize_terminal_input_newlines(r"\u0003"), r"\u0003");
    }

    #[tokio::test]
    async fn terminal_yield_is_bounded_and_mode_scoped() {
        assert_eq!(
            terminal_yield(TerminalMode::Start, None),
            Duration::from_millis(250)
        );
        assert_eq!(
            terminal_yield(TerminalMode::Write, None),
            Duration::from_millis(250)
        );
        assert_eq!(
            terminal_yield(TerminalMode::Read, None),
            Duration::from_secs(3)
        );
        assert_eq!(terminal_yield(TerminalMode::Read, Some(0)), Duration::ZERO);

        let temp = tempfile::tempdir().expect("tempdir");
        let context = ToolContext::new(temp.path())
            .expect("tool context")
            .with_access(crate::ToolAccess::all());
        let over_range = TerminalTool
            .execute(
                &context,
                json!({
                    "mode": "read",
                    "handle": "missing",
                    "yield_time_ms": 30001
                }),
            )
            .await
            .expect_err("out-of-range yield_time_ms was accepted");
        assert!(
            over_range.to_string().contains("yield_time_ms"),
            "error should name yield_time_ms: {over_range}"
        );
        let resize_yield = TerminalTool
            .execute(
                &context,
                json!({
                    "mode": "resize",
                    "handle": "missing",
                    "cols": 80,
                    "rows": 24,
                    "yield_time_ms": 1
                }),
            )
            .await
            .expect_err("yield_time_ms on resize was accepted");
        assert!(
            resize_yield.to_string().contains("yield_time_ms"),
            "error should name yield_time_ms: {resize_yield}"
        );
    }

    #[tokio::test]
    async fn terminal_timeout_is_valid_only_for_start() {
        let schema = TerminalTool.input_schema();
        let schema = schema.get("schema").unwrap_or(&schema);
        let properties = schema["properties"].as_object().expect("properties");
        let timeout = &properties["timeout_secs"];
        assert_eq!(timeout["minimum"], 300);
        assert_eq!(timeout["maximum"], 3_600);
        let timeout_schema = timeout.to_string();
        assert!(!timeout_schema.to_lowercase().contains("rust"));
        assert!(!timeout_schema.to_lowercase().contains("cargo"));
        let temp = tempfile::tempdir().expect("tempdir");
        let context = ToolContext::new(temp.path())
            .expect("tool context")
            .with_access(crate::ToolAccess::all());
        let error = TerminalTool
            .execute(
                &context,
                json!({"mode": "read", "handle": "missing", "timeout_secs": 5}),
            )
            .await
            .expect_err("non-start timeout was accepted");
        assert!(
            error
                .to_string()
                .contains("timeout_secs is valid only for start")
        );
        for timeout_secs in [299, 3_601] {
            let error = TerminalTool
                .execute(
                    &context,
                    json!({
                        "mode": "start",
                        "command": "printf ready",
                        "timeout_secs": timeout_secs,
                    }),
                )
                .await
                .expect_err("out-of-range start timeout was accepted");
            assert!(error.to_string().contains("between 300 and 3600"));
        }
    }
}
