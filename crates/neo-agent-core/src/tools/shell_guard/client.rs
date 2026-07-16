use std::{
    collections::HashMap,
    io,
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio::{
    io::AsyncRead,
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{Mutex, oneshot, watch},
};

use super::{
    ShellCommandPermit, ShellRuntime,
    output::{TaggedHeadTailBuffer, TaggedOutput},
    protocol::{
        GuardRequest, GuardResponse, GuardResponsePart, GuardTaskKind, MAX_FRAME_BODY,
        MAX_TERMINAL_WRITE, ProtocolError, StartRequest, decode_fragmented_response, read_response,
        write_request,
    },
    status::{GuardExit, GuardStatusKind},
};
use crate::tools::{ToolError, ToolUpdateCallback};

const START_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub(crate) struct GuardedCommandResult {
    pub(crate) exit: GuardExit,
    pub(crate) output: TaggedOutput,
}

#[derive(Debug, Clone)]
pub(crate) struct TerminalSnapshot {
    pub(crate) offset: u64,
    pub(crate) total: u64,
    pub(crate) discarded: u64,
    pub(crate) data: Vec<u8>,
}

#[derive(Clone)]
pub(crate) struct GuardianClient {
    control: Arc<Mutex<ChildStdin>>,
    final_result: watch::Receiver<Option<GuardedCommandResult>>,
    output: Arc<Mutex<TaggedHeadTailBuffer>>,
    next_request_id: Arc<AtomicU64>,
    responses: Arc<Mutex<PendingResponses>>,
    pub(crate) guardian_pid: u32,
    pub(crate) command_pid: u32,
}

#[derive(Default)]
struct PendingResponses {
    closed: bool,
    pending: HashMap<u64, oneshot::Sender<GuardResponse>>,
}

impl PendingResponses {
    fn register(
        &mut self,
        request_id: u64,
        sender: oneshot::Sender<GuardResponse>,
    ) -> Result<(), oneshot::Sender<GuardResponse>> {
        if self.closed {
            return Err(sender);
        }
        self.pending.insert(request_id, sender);
        Ok(())
    }

    fn close(&mut self) -> HashMap<u64, oneshot::Sender<GuardResponse>> {
        self.closed = true;
        std::mem::take(&mut self.pending)
    }
}

impl std::fmt::Debug for GuardianClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GuardianClient")
            .field("guardian_pid", &self.guardian_pid)
            .field("command_pid", &self.command_pid)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TerminalClientSession {
    pub(crate) client: GuardianClient,
    pub(crate) state: Arc<Mutex<TerminalClientState>>,
    pub(crate) read_lock: Arc<Mutex<()>>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TerminalClientState {
    pub(crate) read_offset: u64,
    pub(crate) cols: u16,
    pub(crate) rows: u16,
}

impl GuardianClient {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn start_bash(
        runtime: &ShellRuntime,
        task_id: String,
        command_text: String,
        cwd: &Path,
        status_dir: &Path,
        timeout: Duration,
        max_output_bytes: usize,
        stream_update: Option<ToolUpdateCallback>,
    ) -> Result<Self, ToolError> {
        Self::start(
            runtime,
            task_id,
            GuardTaskKind::Bash,
            command_text,
            cwd,
            status_dir,
            timeout,
            max_output_bytes,
            None,
            None,
            stream_update,
        )
        .await
    }

    pub(crate) async fn start_terminal(
        runtime: &ShellRuntime,
        task_id: String,
        command_text: String,
        cwd: &Path,
        status_dir: &Path,
        cols: u16,
        rows: u16,
    ) -> Result<Self, ToolError> {
        Self::start(
            runtime,
            task_id,
            GuardTaskKind::Terminal,
            command_text,
            cwd,
            status_dir,
            Duration::from_secs(runtime.limits().background_timeout_secs),
            runtime.limits().max_output_bytes,
            Some(cols),
            Some(rows),
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn start(
        runtime: &ShellRuntime,
        task_id: String,
        kind: GuardTaskKind,
        command_text: String,
        cwd: &Path,
        status_dir: &Path,
        timeout: Duration,
        max_output_bytes: usize,
        cols: Option<u16>,
        rows: Option<u16>,
        stream_update: Option<ToolUpdateCallback>,
    ) -> Result<Self, ToolError> {
        let stream_limit = max_output_bytes.min(runtime.limits().max_output_bytes);
        let (control, response, child, permit, guardian_pid, command_pid, command_start_id) =
            spawn_guardian_and_handshake(GuardianStartArgs {
                runtime,
                task_id,
                kind,
                command_text,
                cwd,
                status_dir,
                timeout,
                max_output_bytes,
                cols,
                rows,
                stream_limit,
            })
            .await?;

        let output = Arc::new(Mutex::new(TaggedHeadTailBuffer::new(stream_limit)));
        let callback_tx = stream_update.map(build_callback_channel);
        let responses = Arc::new(Mutex::new(PendingResponses::default()));
        let (final_tx, final_result) = watch::channel(None);
        spawn_reader_task(ReaderTaskArgs {
            response,
            child,
            command_pid,
            command_start_id,
            output: Arc::clone(&output),
            callback_tx,
            responses: Arc::clone(&responses),
            final_tx,
            permit,
            stream_limit,
        });

        Ok(Self {
            control: Arc::new(Mutex::new(control)),
            final_result,
            output,
            next_request_id: Arc::new(AtomicU64::new(2)),
            responses,
            guardian_pid,
            command_pid,
        })
    }

    pub(crate) async fn stop(&self) -> GuardedCommandResult {
        let _ = self
            .send_control(|request_id| GuardRequest::Stop { request_id })
            .await;
        self.wait().await
    }

    pub(crate) async fn set_background_deadline(&self) -> Result<(), ToolError> {
        self.send_control(|request_id| GuardRequest::SetBackgroundDeadline { request_id })
            .await
    }

    pub(crate) async fn write_terminal(&self, data: &[u8]) -> Result<(), ToolError> {
        for chunk in data.chunks(MAX_TERMINAL_WRITE) {
            loop {
                match self
                    .request(|request_id| GuardRequest::Write {
                        request_id,
                        data: chunk.to_vec(),
                    })
                    .await?
                {
                    GuardResponse::Ack { .. } => break,
                    GuardResponse::Busy { .. } => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    GuardResponse::Error { message, .. } => {
                        return Err(ToolError::Io(io::Error::other(message)));
                    }
                    _ => return Err(unexpected_response("terminal Write")),
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn read_terminal(
        &self,
        offset: u64,
        max_bytes: usize,
    ) -> Result<TerminalSnapshot, ToolError> {
        match self
            .request(|request_id| GuardRequest::Read {
                request_id,
                offset,
                max_bytes,
            })
            .await?
        {
            GuardResponse::Snapshot {
                offset,
                total,
                discarded,
                data,
                ..
            } => Ok(TerminalSnapshot {
                offset,
                total,
                discarded,
                data,
            }),
            GuardResponse::Error { message, .. } => Err(ToolError::Io(io::Error::other(message))),
            _ => Err(unexpected_response("terminal Read")),
        }
    }

    pub(crate) async fn resize_terminal(&self, cols: u16, rows: u16) -> Result<(), ToolError> {
        match self
            .request(|request_id| GuardRequest::Resize {
                request_id,
                cols,
                rows,
            })
            .await?
        {
            GuardResponse::Ack { .. } => Ok(()),
            GuardResponse::Error { message, .. } => Err(ToolError::Io(io::Error::other(message))),
            _ => Err(unexpected_response("terminal Resize")),
        }
    }

    pub(crate) fn final_result(&self) -> Option<GuardedCommandResult> {
        self.final_result.borrow().clone()
    }

    pub(crate) async fn output(&self) -> TaggedOutput {
        self.output.lock().await.snapshot()
    }

    pub(crate) async fn wait(&self) -> GuardedCommandResult {
        let mut final_result = self.final_result.clone();
        loop {
            if let Some(result) = final_result.borrow().clone() {
                return result;
            }
            if final_result.changed().await.is_err() {
                return failed_result();
            }
        }
    }

    async fn send_control(
        &self,
        request: impl FnOnce(u64) -> GuardRequest,
    ) -> Result<(), ToolError> {
        if self.final_result().is_some() {
            return Ok(());
        }
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        write_request(&mut *self.control.lock().await, &request(request_id))
            .await
            .map_err(protocol_error)
    }

    async fn request(
        &self,
        request: impl FnOnce(u64) -> GuardRequest,
    ) -> Result<GuardResponse, ToolError> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let (response_tx, response_rx) = oneshot::channel();
        self.responses
            .lock()
            .await
            .register(request_id, response_tx)
            .map_err(|_| {
                ToolError::Io(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "guardian already exited",
                ))
            })?;
        if let Err(error) =
            write_request(&mut *self.control.lock().await, &request(request_id)).await
        {
            self.responses.lock().await.pending.remove(&request_id);
            return Err(protocol_error(error));
        }
        response_rx.await.map_err(|_| {
            ToolError::Io(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "guardian response channel closed",
            ))
        })
    }
}

struct GuardianStartArgs<'a> {
    runtime: &'a ShellRuntime,
    task_id: String,
    kind: GuardTaskKind,
    command_text: String,
    cwd: &'a Path,
    status_dir: &'a Path,
    timeout: Duration,
    max_output_bytes: usize,
    cols: Option<u16>,
    rows: Option<u16>,
    stream_limit: usize,
}

async fn spawn_guardian_and_handshake(
    args: GuardianStartArgs<'_>,
) -> Result<
    (
        ChildStdin,
        ChildStdout,
        Child,
        ShellCommandPermit,
        u32,
        u32,
        u64,
    ),
    ToolError,
> {
    let permit = args
        .runtime
        .try_acquire()
        .map_err(|cause| ToolError::ResourceLimited { cause })?;
    std::fs::create_dir_all(args.status_dir)?;
    let mut child = Command::new(args.runtime.guardian_executable())
        .arg("__process-guard")
        .current_dir(args.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut control = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("guardian stdin was not piped"))?;
    let mut response = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("guardian stdout was not piped"))?;
    write_request(
        &mut control,
        &GuardRequest::Start {
            request_id: 1,
            request: StartRequest {
                task_id: args.task_id,
                kind: args.kind,
                command: args.command_text,
                limits: args
                    .runtime
                    .guard_limits(args.timeout, args.max_output_bytes),
                status_dir: args.status_dir.to_path_buf(),
                cols: args.cols,
                rows: args.rows,
            },
        },
    )
    .await
    .map_err(protocol_error)?;
    let started = tokio::time::timeout(
        START_TIMEOUT,
        read_logical_response(&mut response, args.stream_limit),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "guardian start timed out"))?
    .map_err(protocol_error)?;
    let GuardResponse::Started {
        request_id: 1,
        guardian_pid,
        command_pid,
        command_start_id,
    } = started
    else {
        return Err(ToolError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "guardian did not acknowledge Start",
        )));
    };

    Ok((
        control,
        response,
        child,
        permit,
        guardian_pid,
        command_pid,
        command_start_id,
    ))
}

struct ReaderTaskArgs {
    response: ChildStdout,
    child: Child,
    command_pid: u32,
    command_start_id: u64,
    output: Arc<Mutex<TaggedHeadTailBuffer>>,
    callback_tx: Option<tokio::sync::mpsc::Sender<String>>,
    responses: Arc<Mutex<PendingResponses>>,
    final_tx: watch::Sender<Option<GuardedCommandResult>>,
    permit: ShellCommandPermit,
    stream_limit: usize,
}

fn spawn_reader_task(mut args: ReaderTaskArgs) {
    tokio::spawn(async move {
        let _permit = args.permit;
        let mut streamed = 0usize;
        let final_value = loop {
            match read_logical_response(&mut args.response, args.stream_limit).await {
                Ok(GuardResponse::Output { stream, data }) => {
                    args.output.lock().await.push(stream, &data);
                    if streamed < args.stream_limit {
                        let retained = &data[..data.len().min(args.stream_limit - streamed)];
                        streamed = streamed.saturating_add(retained.len());
                        if let Some(callback_tx) = &args.callback_tx {
                            let _ = callback_tx
                                .try_send(String::from_utf8_lossy(retained).into_owned());
                        }
                    }
                }
                Ok(GuardResponse::Exited {
                    exit,
                    stdout,
                    stderr,
                }) => {
                    let omitted_bytes = exit.omitted_output_bytes;
                    break GuardedCommandResult {
                        exit,
                        output: TaggedOutput {
                            stdout,
                            stderr,
                            omitted_bytes,
                        },
                    };
                }
                Ok(
                    reply @ (GuardResponse::Ack { request_id }
                    | GuardResponse::Busy { request_id }
                    | GuardResponse::Snapshot { request_id, .. }
                    | GuardResponse::Error { request_id, .. }),
                ) => {
                    if let Some(sender) = args.responses.lock().await.pending.remove(&request_id) {
                        let _ = sender.send(reply);
                    }
                }
                Ok(GuardResponse::Started { .. }) => {}
                Err(_) => {
                    emergency_cleanup(args.command_pid, args.command_start_id).await;
                    break failed_result();
                }
            }
        };
        let _ = args.final_tx.send(Some(final_value));
        let pending = args.responses.lock().await.close();
        for (request_id, sender) in pending {
            let _ = sender.send(GuardResponse::Error {
                request_id,
                message: "guardian exited before request completed".to_owned(),
            });
        }
        let _ = args.child.wait().await;
    });
}

fn build_callback_channel(callback: ToolUpdateCallback) -> tokio::sync::mpsc::Sender<String> {
    let (callback_tx, mut callback_rx) = tokio::sync::mpsc::channel::<String>(32);
    tokio::spawn(async move {
        while let Some(update) = callback_rx.recv().await {
            callback(&update);
        }
    });
    callback_tx
}

async fn read_logical_response<R>(
    reader: &mut R,
    max_output_bytes: usize,
) -> Result<GuardResponse, ProtocolError>
where
    R: AsyncRead + Unpin,
{
    let (kind, request_id, sequence, final_fragment, data) = match read_response(reader).await? {
        GuardResponsePart::Complete(response) => return Ok(response),
        GuardResponsePart::Fragment {
            kind,
            request_id,
            sequence,
            final_fragment,
            data,
        } => (kind, request_id, sequence, final_fragment, data),
    };

    if sequence != 0 {
        return Err(ProtocolError::Invalid(
            "fragment sequence must start at zero",
        ));
    }
    let logical_limit = max_output_bytes.saturating_add(MAX_FRAME_BODY);
    if data.len() > logical_limit {
        return Err(ProtocolError::Invalid(
            "logical response exceeds configured output limit",
        ));
    }
    let mut payload = data;
    if final_fragment {
        return decode_fragmented_response(kind, request_id, &payload);
    }

    let mut next_sequence = 1u32;
    loop {
        let part = match read_response(reader).await {
            Ok(part) => part,
            Err(ProtocolError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(ProtocolError::Truncated);
            }
            Err(error) => return Err(error),
        };
        let GuardResponsePart::Fragment {
            kind: next_kind,
            request_id: next_request_id,
            sequence,
            final_fragment,
            data,
        } = part
        else {
            return Err(ProtocolError::Invalid(
                "response fragments were interleaved",
            ));
        };
        if next_kind != kind {
            return Err(ProtocolError::Invalid("mixed response fragment kinds"));
        }
        if next_request_id != request_id {
            return Err(ProtocolError::Invalid(
                "mixed response fragment request ids",
            ));
        }
        if sequence != next_sequence {
            return Err(ProtocolError::Invalid(
                "response fragments are out of order",
            ));
        }
        let payload_len = payload
            .len()
            .checked_add(data.len())
            .ok_or(ProtocolError::Invalid("logical response length overflow"))?;
        if payload_len > logical_limit {
            return Err(ProtocolError::Invalid(
                "logical response exceeds configured output limit",
            ));
        }
        payload.extend_from_slice(&data);
        if final_fragment {
            return decode_fragmented_response(kind, request_id, &payload);
        }
        next_sequence = next_sequence.checked_add(1).ok_or(ProtocolError::Invalid(
            "response fragment sequence overflow",
        ))?;
    }
}

fn failed_result() -> GuardedCommandResult {
    GuardedCommandResult {
        exit: GuardExit {
            status: GuardStatusKind::Failed,
            exit_code: None,
            signal: None,
            resource_limit: None,
            omitted_output_bytes: 0,
            omitted_log_bytes: 0,
        },
        output: TaggedOutput {
            stdout: Vec::new(),
            stderr: Vec::new(),
            omitted_bytes: 0,
        },
    }
}

fn protocol_error(error: impl std::fmt::Display) -> ToolError {
    ToolError::Io(io::Error::new(
        io::ErrorKind::InvalidData,
        error.to_string(),
    ))
}

fn unexpected_response(operation: &str) -> ToolError {
    ToolError::Io(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("unexpected guardian response for {operation}"),
    ))
}

#[cfg(unix)]
async fn emergency_cleanup(command_pid: u32, command_start_id: u64) {
    if !identity_matches(command_pid, command_start_id) {
        return;
    }
    let Some(group) = i32::try_from(command_pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return;
    };
    let _ = rustix::process::kill_process_group(group, rustix::process::Signal::TERM);
    tokio::time::sleep(Duration::from_millis(500)).await;
    if identity_matches(command_pid, command_start_id) {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
}

#[cfg(unix)]
fn identity_matches(command_pid: u32, command_start_id: u64) -> bool {
    if command_start_id == 0 {
        return false;
    }
    let pid = sysinfo::Pid::from_u32(command_pid);
    let mut system = sysinfo::System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).map(sysinfo::Process::start_time) == Some(command_start_id)
}

#[cfg(not(unix))]
async fn emergency_cleanup(_command_pid: u32, _command_start_id: u64) {}

#[cfg(test)]
mod tests {
    use super::*;

    const SNAPSHOT_KIND: u8 = 105;
    const EXITED_KIND: u8 = 106;

    #[tokio::test]
    async fn logical_reader_round_trips_fragmented_snapshot_and_exited() {
        let responses = [
            GuardResponse::Snapshot {
                request_id: 41,
                offset: 7,
                total: 9,
                discarded: 2,
                data: vec![b's'; super::super::protocol::MAX_FRAME_BODY + 17],
            },
            GuardResponse::Exited {
                exit: GuardExit {
                    status: GuardStatusKind::Completed,
                    exit_code: Some(0),
                    signal: None,
                    resource_limit: None,
                    omitted_output_bytes: 0,
                    omitted_log_bytes: 0,
                },
                stdout: vec![b'o'; super::super::protocol::MAX_FRAME_BODY + 17],
                stderr: vec![b'e'; super::super::protocol::MAX_FRAME_BODY + 17],
            },
        ];

        for expected in responses {
            let mut bytes = Vec::new();
            super::super::protocol::write_response(&mut bytes, &expected)
                .await
                .unwrap();
            assert_eq!(
                read_logical_response(&mut bytes.as_slice(), usize::MAX)
                    .await
                    .unwrap(),
                expected
            );
        }
    }

    #[tokio::test]
    async fn logical_reader_rejects_truncated_out_of_order_and_mixed_fragments() {
        let payload = [0u8; 24];
        let cases = [
            (
                vec![fragment_frame(SNAPSHOT_KIND, 7, 0, false, &payload[..12])],
                None,
            ),
            (
                vec![
                    fragment_frame(SNAPSHOT_KIND, 7, 0, false, &payload[..12]),
                    fragment_frame(SNAPSHOT_KIND, 7, 2, true, &payload[12..]),
                ],
                Some("response fragments are out of order"),
            ),
            (
                vec![
                    fragment_frame(SNAPSHOT_KIND, 7, 0, false, &payload[..12]),
                    fragment_frame(SNAPSHOT_KIND, 8, 1, true, &payload[12..]),
                ],
                Some("mixed response fragment request ids"),
            ),
            (
                vec![
                    fragment_frame(SNAPSHOT_KIND, 7, 0, false, &payload[..12]),
                    fragment_frame(EXITED_KIND, 7, 1, true, &payload[12..]),
                ],
                Some("mixed response fragment kinds"),
            ),
        ];

        for (frames, expected) in cases {
            let bytes = frames.concat();
            let error = read_logical_response(&mut bytes.as_slice(), usize::MAX)
                .await
                .expect_err("invalid fragment sequence was accepted");
            match expected {
                Some(message) => {
                    assert!(matches!(error, ProtocolError::Invalid(actual) if actual == message));
                }
                None => assert!(matches!(error, ProtocolError::Truncated)),
            }
        }
    }

    #[tokio::test]
    async fn logical_reader_rejects_fragments_over_configured_output_limit() {
        let first = vec![0u8; super::super::protocol::MAX_FRAME_BODY - 32];
        let frames = [
            fragment_frame(SNAPSHOT_KIND, 7, 0, false, &first),
            fragment_frame(SNAPSHOT_KIND, 7, 1, false, &[0u8; 64]),
        ];
        let bytes = frames.concat();

        let error = read_logical_response(&mut bytes.as_slice(), 1)
            .await
            .expect_err("oversized logical response was accepted");

        assert!(matches!(
            error,
            ProtocolError::Invalid("logical response exceeds configured output limit")
        ));
    }

    fn fragment_frame(
        kind: u8,
        request_id: u64,
        sequence: u32,
        final_fragment: bool,
        data: &[u8],
    ) -> Vec<u8> {
        let body_len = 9 + 5 + data.len();
        let mut frame = Vec::with_capacity(4 + body_len);
        frame.extend_from_slice(&u32::try_from(body_len).unwrap().to_be_bytes());
        frame.push(kind);
        frame.extend_from_slice(&request_id.to_be_bytes());
        frame.extend_from_slice(&sequence.to_be_bytes());
        frame.push(u8::from(final_fragment));
        frame.extend_from_slice(data);
        frame
    }

    #[tokio::test]
    async fn reader_close_and_late_request_registration_are_atomic() {
        let state = Arc::new(Mutex::new(PendingResponses::default()));
        let closer_state = Arc::clone(&state);
        let (locked_tx, locked_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let closer = tokio::spawn(async move {
            let mut state = closer_state.lock().await;
            let _ = locked_tx.send(());
            let _ = release_rx.await;
            state.close()
        });
        locked_rx.await.unwrap();

        let register_state = Arc::clone(&state);
        let registration = tokio::spawn(async move {
            let (response_tx, _response_rx) = oneshot::channel();
            register_state.lock().await.register(7, response_tx)
        });
        tokio::task::yield_now().await;
        let _ = release_tx.send(());

        assert!(closer.await.unwrap().is_empty());
        assert!(registration.await.unwrap().is_err());
    }
}
