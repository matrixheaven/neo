use std::{
    io::{self, Read, Write},
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures::StreamExt as _;
use portable_pty::{CommandBuilder, MasterPty, PtySize, PtySystem, native_pty_system};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
    task::JoinHandle,
};

#[cfg(windows)]
use super::process_tree::WindowsLaunchBarrier;
use super::{
    ResourceLimitDetail,
    guardian::{
        ProcessSampler, ResourceTick, check_resource_tick, process_start_id, try_send_response,
    },
    process_tree::TerminalProcessTree,
    protocol::{GuardRequest, GuardResponse, StartRequest, request_stream, write_response},
    status::{
        FinalStatusGuard, GuardExit, GuardStatus, GuardStatusKind, RunningStatus,
        require_durable_running_write,
    },
};
use crate::{
    session::atomic_file::{AtomicWriteStatus, write_file_atomic_status},
    tools::{bash::resolved_shell, shell_env},
};

const RESPONSE_QUEUE_CAPACITY: usize = 32;
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);
const RESOURCE_POLL_INTERVAL: Duration = Duration::from_millis(250);
const OUTPUT_DRAIN_GRACE: Duration = Duration::from_secs(2);

type TerminalSupervisionBreak = (
    GuardStatusKind,
    Option<i32>,
    Option<ResourceLimitDetail>,
    Option<String>,
);

#[derive(Debug)]
struct TerminalSupervisionResult {
    status: GuardStatusKind,
    exit_code: Option<i32>,
    resource_limit: Option<ResourceLimitDetail>,
    response_error: Option<String>,
    response_open: bool,
}

struct TerminalGuardState {
    response_tx: mpsc::Sender<GuardResponse>,
    response_writer: JoinHandle<io::Result<()>>,
    terminal: GuardedTerminal,
    sampler: ProcessSampler,
    final_status_guard: FinalStatusGuard,
    started_at_ms: u64,
}

pub(super) async fn run_terminal_guard<R, W>(
    mut control: R,
    response: W,
    start_request_id: u64,
    start: StartRequest,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let mut state = start_terminal_guard(response, start_request_id, &start)?;
    let result = run_terminal_supervision_loop(
        &mut control,
        &mut state.terminal,
        &mut state.sampler,
        &mut state.response_writer,
        &state.response_tx,
        &start,
    )
    .await?;
    finalize_terminal(state, result, &start).await
}

fn start_terminal_guard(
    response: impl AsyncWrite + Unpin + Send + 'static,
    start_request_id: u64,
    start: &StartRequest,
) -> io::Result<TerminalGuardState> {
    std::fs::create_dir_all(&start.status_dir)?;
    let started_at_ms = unix_time_ms();
    let final_status_path = start
        .status_dir
        .join(format!("{}.status.json", start.task_id));
    let final_status_guard = FinalStatusGuard::after_running_write(
        final_status_path,
        start.task_id.clone(),
        started_at_ms,
        write_running_status(start, started_at_ms, None),
    )?;
    let (response_tx, mut response_rx) = mpsc::channel(RESPONSE_QUEUE_CAPACITY);
    let response_writer = tokio::spawn(async move {
        let mut response = response;
        while let Some(message) = response_rx.recv().await {
            write_response(&mut response, &message)
                .await
                .map_err(protocol_io_error)?;
        }
        Ok::<(), io::Error>(())
    });
    let cols = start.cols.unwrap_or(80).max(1);
    let rows = start.rows.unwrap_or(24).max(1);
    let mut terminal = GuardedTerminal::spawn(start, cols, rows, response_tx.clone())?;
    let command_pid = terminal.process_id();
    let sampler = ProcessSampler::new(command_pid);
    let command_start_id = process_start_id(command_pid);
    if command_start_id == 0 {
        let _ = terminal.stop();
        return Err(io::Error::other(
            "cannot establish Terminal process identity",
        ));
    }
    require_durable_running_write(write_running_status(
        start,
        started_at_ms,
        Some((command_pid, command_start_id)),
    )?)?;
    try_send_response(
        &response_tx,
        GuardResponse::Started {
            request_id: start_request_id,
            guardian_pid: std::process::id(),
            command_pid,
            command_start_id,
        },
    )?;
    Ok(TerminalGuardState {
        response_tx,
        response_writer,
        terminal,
        sampler,
        final_status_guard,
        started_at_ms,
    })
}

async fn run_terminal_supervision_loop<R>(
    control: &mut R,
    terminal: &mut GuardedTerminal,
    sampler: &mut ProcessSampler,
    mut response_writer: &mut JoinHandle<io::Result<()>>,
    response_tx: &mpsc::Sender<GuardResponse>,
    start: &StartRequest,
) -> io::Result<TerminalSupervisionResult>
where
    R: AsyncRead + Unpin,
{
    let mut deadline = super::command_deadline(start.limits.timeout_ms);
    let mut poll = tokio::time::interval(PROCESS_POLL_INTERVAL);
    let mut resource_poll = tokio::time::interval(RESOURCE_POLL_INTERVAL);
    let mut writer_poll = tokio::time::interval(Duration::from_millis(10));
    let requests = request_stream(&mut *control);
    tokio::pin!(requests);
    let mut response_open = true;
    let result = 'supervision: loop {
        tokio::select! {
            writer_result = &mut response_writer => {
                response_open = false;
                let error = match writer_result {
                    Ok(Ok(())) => "terminal response writer closed".to_owned(),
                    Ok(Err(error)) => error.to_string(),
                    Err(error) => format!("join terminal response writer: {error}"),
                };
                break (GuardStatusKind::ParentExited, None, None, Some(error));
            }
            request = requests.next() => {
                match request.expect("guardian request stream does not terminate") {
                    Ok(request) => {
                        if let Some(tuple) = handle_terminal_request(
                            &request,
                            terminal,
                            response_tx,
                            start,
                            &mut response_open,
                        ) {
                            break tuple;
                        }
                    }
                    Err(error) => break handle_terminal_request_error(&error, terminal)?,
                }
            }
            () = super::wait_for_deadline(&mut deadline) => {
                break (GuardStatusKind::TimedOut, None, None, None)
            }
            _ = poll.tick() => {
                if let Some(code) = terminal.try_wait()? {
                    break (status_kind_for_exit_code(code), Some(code), None, None);
                }
            }
            _ = resource_poll.tick() => {
                match check_resource_tick(
                    || terminal.try_wait(),
                    || sampler.exceeded_limit(start),
                )? {
                    ResourceTick::Exited(code) => {
                        break (status_kind_for_exit_code(code), Some(code), None, None)
                    }
                    ResourceTick::Limited(limit) => {
                        break (GuardStatusKind::ResourceLimited, None, Some(limit), None)
                    }
                    ResourceTick::Running => {}
                }
            }
            _ = writer_poll.tick() => {
                for response in terminal.take_write_responses() {
                    if let Err(error) = try_send_response(response_tx, response) {
                        response_open = false;
                        break 'supervision (GuardStatusKind::ParentExited, None, None, Some(error.to_string()));
                    }
                }
            }
        }
    };
    Ok(TerminalSupervisionResult {
        status: result.0,
        exit_code: result.1,
        resource_limit: result.2,
        response_error: result.3,
        response_open,
    })
}

fn handle_terminal_request(
    request: &GuardRequest,
    terminal: &mut GuardedTerminal,
    response_tx: &mpsc::Sender<GuardResponse>,
    start: &StartRequest,
    response_open: &mut bool,
) -> Option<TerminalSupervisionBreak> {
    match request {
        GuardRequest::Write { request_id, data } => {
            if let Err(error) = terminal.enqueue_write(*request_id, data.clone(), response_tx) {
                *response_open = false;
                return Some((
                    GuardStatusKind::ParentExited,
                    None,
                    None,
                    Some(error.to_string()),
                ));
            }
            None
        }
        GuardRequest::Read {
            request_id,
            offset,
            max_bytes,
        } => {
            let snapshot = terminal.read(*offset, (*max_bytes).min(start.limits.max_output_bytes));
            let error = send_response_or_close(
                response_tx,
                GuardResponse::Snapshot {
                    request_id: *request_id,
                    offset: snapshot.next_offset,
                    total: snapshot.total,
                    discarded: snapshot.discarded,
                    data: snapshot.data,
                },
                response_open,
            );
            error.map(|error| (GuardStatusKind::ParentExited, None, None, Some(error)))
        }
        GuardRequest::Resize {
            request_id,
            cols,
            rows,
        } => {
            let response = match terminal.resize((*cols).max(1), (*rows).max(1)) {
                Ok(()) => GuardResponse::Ack {
                    request_id: *request_id,
                },
                Err(error) => GuardResponse::Error {
                    request_id: *request_id,
                    message: error.to_string(),
                },
            };
            let error = send_response_or_close(response_tx, response, response_open);
            error.map(|error| (GuardStatusKind::ParentExited, None, None, Some(error)))
        }
        GuardRequest::Stop { request_id } => {
            let error = send_response_or_close(
                response_tx,
                GuardResponse::Ack {
                    request_id: *request_id,
                },
                response_open,
            );
            let status = if error.is_some() {
                GuardStatusKind::ParentExited
            } else {
                GuardStatusKind::Cancelled
            };
            Some((status, None, None, error))
        }
        GuardRequest::Start { .. } => None,
    }
}

fn handle_terminal_request_error(
    error: &super::protocol::ProtocolError,
    terminal: &mut GuardedTerminal,
) -> io::Result<TerminalSupervisionBreak> {
    if let Some(code) = terminal.try_wait()? {
        return Ok((status_kind_for_exit_code(code), Some(code), None, None));
    }
    let status = if protocol_is_eof(error) {
        GuardStatusKind::ParentExited
    } else {
        GuardStatusKind::Failed
    };
    Ok((status, None, None, None))
}

fn status_kind_for_exit_code(code: i32) -> GuardStatusKind {
    if code == 0 {
        GuardStatusKind::Completed
    } else {
        GuardStatusKind::Failed
    }
}

fn send_response_or_close(
    response_tx: &mpsc::Sender<GuardResponse>,
    response: GuardResponse,
    response_open: &mut bool,
) -> Option<String> {
    try_send_response(response_tx, response).err().map(|error| {
        *response_open = false;
        error.to_string()
    })
}

async fn finalize_terminal(
    mut state: TerminalGuardState,
    result: TerminalSupervisionResult,
    start: &StartRequest,
) -> io::Result<()> {
    if !result.response_open {
        state.response_writer.abort();
    }
    let mut cleanup_errors = result.response_error.into_iter().collect::<Vec<_>>();
    #[cfg(unix)]
    {
        state.sampler.refresh_descendants();
        if let Err(error) = super::guardian::signal_descendants(
            &state.sampler.descendants(),
            rustix::process::Signal::TERM,
        ) {
            cleanup_errors.push(error.to_string());
        }
    }
    let cleanup_exit = match state.terminal.stop() {
        Ok(exit) => exit,
        Err(error) => {
            cleanup_errors.push(error.to_string());
            None
        }
    };
    #[cfg(unix)]
    {
        state.sampler.refresh_descendants();
        if let Err(error) = super::guardian::signal_descendants(
            &state.sampler.descendants(),
            rustix::process::Signal::KILL,
        ) {
            cleanup_errors.push(error.to_string());
        }
    }
    let exit_code = result.exit_code.or(cleanup_exit);
    let output = state.terminal.full_output(start.limits.max_output_bytes);
    let exit = GuardExit {
        status: result.status,
        exit_code,
        signal: None,
        resource_limit: result.resource_limit,
        omitted_output_bytes: output.omitted,
        omitted_log_bytes: 0,
    };
    let final_write = state
        .final_status_guard
        .write(&GuardStatus {
            schema_version: 1,
            task_id: start.task_id.clone(),
            started_at_ms: state.started_at_ms,
            finished_at_ms: unix_time_ms(),
            exit: exit.clone(),
            cleanup_errors,
        })
        .map_err(|error| io::Error::other(error.to_string()))?;
    if result.response_open {
        let _ = state
            .response_tx
            .send(GuardResponse::Exited {
                exit,
                stdout: output.data,
                stderr: Vec::new(),
            })
            .await;
    }
    drop(state.response_tx);
    if result.response_open {
        state.response_writer.await.map_err(|error| {
            io::Error::other(format!("join terminal response writer: {error}"))
        })??;
    }
    match final_write {
        AtomicWriteStatus::Durable => Ok(()),
        AtomicWriteStatus::CommittedUnsynced(error) => Err(error),
    }
}

struct GuardedTerminal {
    process: TerminalProcessTree,
    master: Box<dyn MasterPty + Send>,
    output: Arc<StdMutex<TerminalOutputBuffer>>,
    write_tx: Option<std::sync::mpsc::SyncSender<(u64, Vec<u8>)>>,
    write_responses: std::sync::mpsc::Receiver<GuardResponse>,
    reader_done: std::sync::mpsc::Receiver<()>,
    reader_thread: Option<std::thread::JoinHandle<()>>,
    #[cfg(windows)]
    _launch_barrier: WindowsLaunchBarrier,
}

impl GuardedTerminal {
    fn spawn(
        start: &StartRequest,
        cols: u16,
        rows: u16,
        _response_tx: mpsc::Sender<GuardResponse>,
    ) -> io::Result<Self> {
        let pair = native_pty_system()
            .openpty(pty_size(cols, rows))
            .map_err(pty_error)?;
        let reader = pair.master.try_clone_reader().map_err(pty_error)?;
        let writer = pair.master.take_writer().map_err(pty_error)?;
        let shell = resolved_shell().map_err(|error| io::Error::other(error.to_string()))?;
        let cwd = std::env::current_dir()?;
        #[cfg(windows)]
        let launch_barrier = WindowsLaunchBarrier::new(&start.status_dir);
        let (effective_cwd, command) = if shell.is_windows {
            let cwd = shell_env::GitBashCwd::new(&cwd)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
            (
                None,
                format!(
                    "cd {} && {}",
                    cwd.shell_cd(),
                    shell_env::rewrite_windows_nul_redirect(&start.command)
                ),
            )
        } else {
            (Some(cwd), start.command.clone())
        };
        #[cfg(windows)]
        let command = format!("{} {command}", launch_barrier.wait_command());
        let mut builder = CommandBuilder::new(&shell.shell_path);
        if shell.is_windows {
            builder.env("BASH_ENV", "");
            builder.arg("--noprofile");
            builder.arg("--norc");
            builder.arg("-c");
        } else {
            builder.arg("-lc");
        }
        builder.env("NO_COLOR", "1");
        builder.env("TERM", "dumb");
        builder.env("SHELL", &shell.shell_path);
        if std::env::var_os("GIT_TERMINAL_PROMPT").is_none() {
            builder.env("GIT_TERMINAL_PROMPT", "0");
        }
        for name in [
            "CARGO_BUILD_JOBS",
            "NEXTEST_TEST_THREADS",
            "RAYON_NUM_THREADS",
        ] {
            if std::env::var_os(name).is_none() {
                builder.env(name, start.limits.max_command_parallelism.to_string());
            }
        }
        builder.arg(command);
        if let Some(cwd) = effective_cwd {
            builder.cwd(cwd);
        }
        let child = pair.slave.spawn_command(builder).map_err(pty_error)?;
        drop(pair.slave);
        #[cfg(windows)]
        let mut process = TerminalProcessTree::new(child)?;
        #[cfg(not(windows))]
        let process = TerminalProcessTree::new(child)?;
        #[cfg(windows)]
        if let Err(error) = launch_barrier.release() {
            let _ = process.terminate_and_wait();
            return Err(error);
        }
        let output = Arc::new(StdMutex::new(TerminalOutputBuffer::new(
            start.limits.max_output_bytes,
        )));
        let reader_output = Arc::clone(&output);
        let (reader_thread, reader_done) = Self::spawn_terminal_reader(reader, reader_output);
        let (write_tx, write_responses) = Self::spawn_terminal_writer(writer);
        Ok(Self {
            process,
            master: pair.master,
            output,
            write_tx: Some(write_tx),
            write_responses,
            reader_done,
            reader_thread: Some(reader_thread),
            #[cfg(windows)]
            _launch_barrier: launch_barrier,
        })
    }

    fn spawn_terminal_reader(
        mut reader: Box<dyn Read + Send>,
        output: Arc<StdMutex<TerminalOutputBuffer>>,
    ) -> (std::thread::JoinHandle<()>, std::sync::mpsc::Receiver<()>) {
        let (reader_done_tx, reader_done) = std::sync::mpsc::sync_channel(0);
        let thread = std::thread::spawn(move || {
            let mut chunk = [0u8; 8 * 1024];
            while let Ok(read) = reader.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                output
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(&chunk[..read]);
            }
            let _ = reader_done_tx.send(());
        });
        (thread, reader_done)
    }

    fn spawn_terminal_writer(
        mut writer: Box<dyn Write + Send>,
    ) -> (
        std::sync::mpsc::SyncSender<(u64, Vec<u8>)>,
        std::sync::mpsc::Receiver<GuardResponse>,
    ) {
        let (write_tx, write_rx) = std::sync::mpsc::sync_channel::<(u64, Vec<u8>)>(1);
        let (write_response_tx, write_responses) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            while let Ok((request_id, data)) = write_rx.recv() {
                let response = match writer.write_all(&data).and_then(|()| writer.flush()) {
                    Ok(()) => GuardResponse::Ack { request_id },
                    Err(error) => GuardResponse::Error {
                        request_id,
                        message: error.to_string(),
                    },
                };
                let _ = write_response_tx.send(response);
            }
        });
        (write_tx, write_responses)
    }

    fn process_id(&self) -> u32 {
        self.process.process_id().unwrap_or_default()
    }

    fn try_wait(&mut self) -> io::Result<Option<i32>> {
        self.process.try_wait()
    }

    fn enqueue_write(
        &self,
        request_id: u64,
        data: Vec<u8>,
        response_tx: &mpsc::Sender<GuardResponse>,
    ) -> io::Result<()> {
        let Some(write_tx) = &self.write_tx else {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "terminal writer closed",
            ));
        };
        match write_tx.try_send((request_id, data)) {
            Ok(()) => Ok(()),
            Err(std::sync::mpsc::TrySendError::Full(_)) => {
                try_send_response(response_tx, GuardResponse::Busy { request_id })
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => try_send_response(
                response_tx,
                GuardResponse::Error {
                    request_id,
                    message: "terminal writer closed".to_owned(),
                },
            ),
        }
    }

    fn take_write_responses(&self) -> Vec<GuardResponse> {
        self.write_responses.try_iter().collect()
    }

    fn read(&self, offset: u64, max_bytes: usize) -> TerminalSnapshot {
        self.output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .read(offset, max_bytes)
    }

    fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        self.master.resize(pty_size(cols, rows)).map_err(pty_error)
    }

    fn stop(&mut self) -> io::Result<Option<i32>> {
        let exit = self.process.terminate_and_wait()?;
        self.write_tx.take();
        if self.reader_done.recv_timeout(OUTPUT_DRAIN_GRACE).is_ok()
            && let Some(thread) = self.reader_thread.take()
        {
            let _ = thread.join();
        }
        Ok(exit)
    }

    fn full_output(&self, max_bytes: usize) -> TerminalSnapshot {
        let output = self
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        output.read(output.start, max_bytes)
    }
}

struct TerminalOutputBuffer {
    bytes: Vec<u8>,
    start: u64,
    total: u64,
    capacity: usize,
}

struct TerminalSnapshot {
    data: Vec<u8>,
    next_offset: u64,
    total: u64,
    discarded: u64,
    omitted: u64,
}

impl TerminalOutputBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
            start: 0,
            total: 0,
            capacity,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.total = self
            .total
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        self.bytes.extend_from_slice(bytes);
        if self.bytes.len() > self.capacity {
            let excess = self.bytes.len() - self.capacity;
            self.bytes.drain(..excess);
            self.start = self
                .start
                .saturating_add(u64::try_from(excess).unwrap_or(u64::MAX));
        }
    }

    fn read(&self, offset: u64, max_bytes: usize) -> TerminalSnapshot {
        let effective = offset.max(self.start).min(self.total);
        let index = usize::try_from(effective.saturating_sub(self.start)).unwrap_or(usize::MAX);
        let available = self.bytes.get(index..).unwrap_or_default();
        let candidate = &available[..available.len().min(max_bytes)];
        let consumed = utf8_prefix_len(candidate);
        let next_offset = effective.saturating_add(u64::try_from(consumed).unwrap_or(u64::MAX));
        TerminalSnapshot {
            data: candidate[..consumed].to_vec(),
            next_offset,
            total: self.total,
            discarded: self.start.saturating_sub(offset),
            omitted: self
                .start
                .saturating_add(self.total.saturating_sub(next_offset)),
        }
    }
}

fn utf8_prefix_len(bytes: &[u8]) -> usize {
    let mut consumed = 0;
    while consumed < bytes.len() {
        match std::str::from_utf8(&bytes[consumed..]) {
            Ok(_) => return bytes.len(),
            Err(error) => {
                consumed += error.valid_up_to();
                let Some(invalid) = error.error_len() else {
                    break;
                };
                consumed += invalid;
            }
        }
    }
    consumed
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn pty_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

fn write_running_status(
    start: &StartRequest,
    started_at_ms: u64,
    command: Option<(u32, u64)>,
) -> io::Result<AtomicWriteStatus> {
    let status = command.map_or_else(
        || RunningStatus::new(&start.task_id, started_at_ms),
        |(pid, start_id)| {
            RunningStatus::new(&start.task_id, started_at_ms).with_command(pid, start_id)
        },
    );
    let content = serde_json::to_vec(&status)?;
    write_file_atomic_status(
        &start
            .status_dir
            .join(format!("{}.running.json", start.task_id)),
        &content,
    )
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn protocol_is_eof(error: &super::protocol::ProtocolError) -> bool {
    matches!(error, super::protocol::ProtocolError::Io(error) if error.kind() == io::ErrorKind::UnexpectedEof)
}

fn protocol_io_error(error: super::protocol::ProtocolError) -> io::Error {
    match error {
        super::protocol::ProtocolError::Io(error) => error,
        other => io::Error::new(io::ErrorKind::InvalidData, other),
    }
}
