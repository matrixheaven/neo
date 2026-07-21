use std::{
    collections::HashSet,
    io,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures::StreamExt as _;
use sysinfo::{Pid as SystemPid, ProcessesToUpdate, System};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    sync::{Mutex, mpsc},
    task::JoinHandle,
};

#[cfg(windows)]
use super::process_tree::WindowsLaunchBarrier;
use super::{
    ResourceLimitCause, ResourceLimitDetail,
    output::{StreamKind, TaggedHeadTailBuffer, TaggedOutput},
    protocol::{
        GuardRequest, GuardResponse, GuardTaskKind, ProtocolError, StartRequest, read_request,
        request_stream, write_response,
    },
    status::{
        FinalStatusGuard, GuardExit, GuardStatus, GuardStatusKind, RunningStatus,
        require_durable_running_write,
    },
};
use crate::{
    session::atomic_file::{AtomicWriteStatus, write_file_atomic_status},
    tools::{bash::resolved_shell, shell_env},
};

const OUTPUT_CHUNK_BYTES: usize = 8 * 1024;
const RESPONSE_QUEUE_CAPACITY: usize = 32;
const LOG_QUEUE_CAPACITY: usize = 32;
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);
const RESOURCE_POLL_INTERVAL: Duration = Duration::from_millis(250);
const TERMINATION_GRACE: Duration = Duration::from_millis(500);
const OUTPUT_DRAIN_GRACE: Duration = Duration::from_secs(2);

pub(super) fn try_send_response(
    sender: &mpsc::Sender<GuardResponse>,
    response: GuardResponse,
) -> io::Result<()> {
    sender.try_send(response).map_err(|error| match error {
        mpsc::error::TrySendError::Full(_) => {
            io::Error::new(io::ErrorKind::WouldBlock, "guardian response queue is full")
        }
        mpsc::error::TrySendError::Closed(_) => {
            io::Error::new(io::ErrorKind::BrokenPipe, "guardian response closed")
        }
    })
}

pub async fn run_process_guard() -> io::Result<()> {
    run_process_guard_io(tokio::io::stdin(), tokio::io::stdout()).await
}

type SupervisionBreak = (
    GuardStatusKind,
    Option<std::process::ExitStatus>,
    Option<ResourceLimitDetail>,
    Option<String>,
);

#[derive(Debug)]
struct SupervisionResult {
    status_kind: GuardStatusKind,
    exit_status: Option<std::process::ExitStatus>,
    resource_limit: Option<ResourceLimitDetail>,
    response_error: Option<String>,
    response_open: bool,
}

struct OutputTasks {
    output: Arc<Mutex<TaggedHeadTailBuffer>>,
    log_tx: mpsc::Sender<Vec<u8>>,
    log_task: JoinHandle<io::Result<u64>>,
    stdout_task: Option<JoinHandle<io::Result<()>>>,
    stderr_task: Option<JoinHandle<io::Result<()>>>,
    dropped_log_bytes: Arc<AtomicU64>,
}

async fn run_process_guard_io<R, W>(mut control: R, response: W) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let first = read_request(&mut control)
        .await
        .map_err(protocol_io_error)?;
    let GuardRequest::Start {
        request_id: start_request_id,
        request: start,
    } = first
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "first guardian frame must be Start",
        ));
    };
    if start.kind == GuardTaskKind::Terminal {
        return super::terminal_guard::run_terminal_guard(
            control,
            response,
            start_request_id,
            start,
        )
        .await;
    }

    std::fs::create_dir_all(&start.status_dir)?;
    let started_at_ms = unix_time_ms();
    let mut final_status_guard = FinalStatusGuard::after_running_write(
        final_status_path(&start),
        start.task_id.clone(),
        started_at_ms,
        write_running_status(&start, started_at_ms, None),
    )?;

    let (mut writer, response_tx) = spawn_response_writer(response);
    let (mut process, mut sampler) =
        start_bash_process(&start, &response_tx, start_request_id, started_at_ms).await?;
    let output_tasks = spawn_output_tasks(&mut process, &start, &response_tx).await?;

    let result = run_supervision_loop(
        &mut process,
        &mut sampler,
        &mut control,
        &mut writer,
        &response_tx,
        &start,
    )
    .await?;
    if !result.response_open {
        writer.abort();
    }

    let (exit_status, mut cleanup_errors) = terminate_process(
        &mut process,
        &mut sampler,
        result.exit_status,
        result.response_error,
    )
    .await;
    let (retained, omitted_log_bytes, mut errors) = await_output_and_log(output_tasks).await;
    cleanup_errors.append(&mut errors);

    let exit = guard_exit(
        result.status_kind,
        exit_status,
        &retained,
        result.resource_limit,
        omitted_log_bytes,
    );
    let final_status = build_final_status(&start, started_at_ms, exit.clone(), cleanup_errors);
    let final_write = final_status_guard
        .write(&final_status)
        .map_err(|error| io::Error::other(error.to_string()))?;

    if result.response_open {
        let _ = response_tx
            .send(GuardResponse::Exited {
                exit,
                stdout: retained.stdout,
                stderr: retained.stderr,
            })
            .await;
    }
    drop(response_tx);
    if result.response_open {
        writer
            .await
            .map_err(|error| io::Error::other(format!("join guardian writer: {error}")))??;
    }
    match final_write {
        AtomicWriteStatus::Durable => Ok(()),
        AtomicWriteStatus::CommittedUnsynced(error) => Err(error),
    }
}

fn spawn_response_writer<W>(
    response: W,
) -> (JoinHandle<io::Result<()>>, mpsc::Sender<GuardResponse>)
where
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (response_tx, mut response_rx) = mpsc::channel(RESPONSE_QUEUE_CAPACITY);
    let writer = tokio::spawn(async move {
        let mut response = response;
        while let Some(message) = response_rx.recv().await {
            write_response(&mut response, &message)
                .await
                .map_err(protocol_io_error)?;
        }
        Ok::<(), io::Error>(())
    });
    (writer, response_tx)
}

async fn start_bash_process(
    start: &StartRequest,
    response_tx: &mpsc::Sender<GuardResponse>,
    start_request_id: u64,
    started_at_ms: u64,
) -> io::Result<(GuardedBashProcess, ProcessSampler)> {
    let mut process = GuardedBashProcess::spawn(start)?;
    let command_pid = process.child.id().unwrap_or_default();
    let mut sampler = ProcessSampler::new(command_pid);
    let command_start_id = process_start_id(command_pid);
    if command_start_id == 0 {
        let _ = process.terminate_and_wait(&mut sampler).await;
        return Err(io::Error::other("cannot establish Bash process identity"));
    }
    require_durable_running_write(write_running_status(
        start,
        started_at_ms,
        Some((command_pid, command_start_id)),
    )?)?;
    try_send_response(
        response_tx,
        GuardResponse::Started {
            request_id: start_request_id,
            guardian_pid: std::process::id(),
            command_pid,
            command_start_id,
        },
    )?;
    Ok((process, sampler))
}

async fn spawn_output_tasks(
    process: &mut GuardedBashProcess,
    start: &StartRequest,
    response_tx: &mpsc::Sender<GuardResponse>,
) -> io::Result<OutputTasks> {
    let output = Arc::new(Mutex::new(TaggedHeadTailBuffer::new(
        start.limits.max_output_bytes,
    )));
    let log_file = tokio::fs::File::create(log_path(start)).await?;
    let (log_tx, log_rx) = mpsc::channel(LOG_QUEUE_CAPACITY);
    let dropped_log_bytes = Arc::new(AtomicU64::new(0));
    let log_truncated = Arc::new(AtomicBool::new(false));
    let log_task = spawn_log_writer(log_file, log_rx, start.limits.max_background_log_bytes);
    let stdout_task = process.child.stdout.take().map(|stdout| {
        spawn_output_drain(
            stdout,
            StreamKind::Stdout,
            Arc::clone(&output),
            response_tx.clone(),
            log_tx.clone(),
            Arc::clone(&dropped_log_bytes),
            Arc::clone(&log_truncated),
        )
    });
    let stderr_task = process.child.stderr.take().map(|stderr| {
        spawn_output_drain(
            stderr,
            StreamKind::Stderr,
            Arc::clone(&output),
            response_tx.clone(),
            log_tx.clone(),
            Arc::clone(&dropped_log_bytes),
            Arc::clone(&log_truncated),
        )
    });
    Ok(OutputTasks {
        output,
        log_tx,
        log_task,
        stdout_task,
        stderr_task,
        dropped_log_bytes,
    })
}

async fn run_supervision_loop<R>(
    process: &mut GuardedBashProcess,
    sampler: &mut ProcessSampler,
    control: &mut R,
    mut writer: &mut JoinHandle<io::Result<()>>,
    response_tx: &mpsc::Sender<GuardResponse>,
    start: &StartRequest,
) -> io::Result<SupervisionResult>
where
    R: AsyncRead + Unpin,
{
    let mut deadline = super::command_deadline(start.limits.timeout_ms);
    let mut poll = tokio::time::interval(PROCESS_POLL_INTERVAL);
    let mut resource_poll = tokio::time::interval(RESOURCE_POLL_INTERVAL);
    let requests = request_stream(&mut *control);
    tokio::pin!(requests);
    let mut response_open = true;
    let result = loop {
        tokio::select! {
            writer_result = &mut writer => {
                response_open = false;
                let error = match writer_result {
                    Ok(Ok(())) => "guardian response writer closed".to_owned(),
                    Ok(Err(error)) => error.to_string(),
                    Err(error) => format!("join guardian response writer: {error}"),
                };
                break (GuardStatusKind::ParentExited, None, None, Some(error));
            }
            request = requests.next() => {
                match request.expect("guardian request stream does not terminate") {
                    Ok(request) => {
                        if let Some(tuple) = handle_request(
                            &request,
                            response_tx,
                            &mut response_open,
                        ) {
                            break tuple;
                        }
                    }
                    Err(error) => break handle_request_error(&error, process)?,
                }
            }
            () = super::wait_for_deadline(&mut deadline) => {
                break (GuardStatusKind::TimedOut, None, None, None)
            }
            _ = poll.tick() => {
                if let Some(status) = process.child.try_wait()? {
                    break (status_kind_for_exit(status), Some(status), None, None);
                }
            }
            _ = resource_poll.tick() => {
                match check_resource_tick(
                    || process.child.try_wait(),
                    || sampler.exceeded_limit(start),
                )? {
                    ResourceTick::Exited(status) => {
                        break (status_kind_for_exit(status), Some(status), None, None)
                    }
                    ResourceTick::Limited(limit) => {
                        break (GuardStatusKind::ResourceLimited, None, Some(limit), None)
                    }
                    ResourceTick::Running => {}
                }
            }
        }
    };
    Ok(SupervisionResult {
        status_kind: result.0,
        exit_status: result.1,
        resource_limit: result.2,
        response_error: result.3,
        response_open,
    })
}

fn handle_request(
    request: &GuardRequest,
    response_tx: &mpsc::Sender<GuardResponse>,
    response_open: &mut bool,
) -> Option<SupervisionBreak> {
    if let GuardRequest::Stop { request_id } = request {
        let error = send_response_or_close(
            response_tx,
            GuardResponse::Ack {
                request_id: *request_id,
            },
            response_open,
        );
        let status_kind = if error.is_some() {
            GuardStatusKind::ParentExited
        } else {
            GuardStatusKind::Cancelled
        };
        Some((status_kind, None, None, error))
    } else {
        let error = send_response_or_close(
            response_tx,
            GuardResponse::Error {
                request_id: 0,
                message: "request is not valid for Bash".to_owned(),
            },
            response_open,
        );
        error.map(|error| (GuardStatusKind::ParentExited, None, None, Some(error)))
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

fn handle_request_error(
    error: &ProtocolError,
    process: &mut GuardedBashProcess,
) -> io::Result<SupervisionBreak> {
    if let Some(status) = process.child.try_wait()? {
        return Ok((status_kind_for_exit(status), Some(status), None, None));
    }
    let status_kind = if protocol_is_eof(error) {
        GuardStatusKind::ParentExited
    } else {
        GuardStatusKind::Failed
    };
    Ok((status_kind, None, None, None))
}

fn status_kind_for_exit(status: std::process::ExitStatus) -> GuardStatusKind {
    if status.success() {
        GuardStatusKind::Completed
    } else {
        GuardStatusKind::Failed
    }
}

async fn terminate_process(
    process: &mut GuardedBashProcess,
    sampler: &mut ProcessSampler,
    exit_status: Option<std::process::ExitStatus>,
    response_error: Option<String>,
) -> (Option<std::process::ExitStatus>, Vec<String>) {
    let mut cleanup_errors = response_error.into_iter().collect::<Vec<_>>();
    let exit_status = match exit_status {
        Some(status) => {
            if let Err(error) = process.terminate_remaining_group(sampler).await {
                cleanup_errors.push(error.to_string());
            }
            Some(status)
        }
        None => match process.terminate_and_wait(sampler).await {
            Ok(status) => Some(status),
            Err(error) => {
                cleanup_errors.push(error.to_string());
                let _ = process.force_termination();
                None
            }
        },
    };
    (exit_status, cleanup_errors)
}

async fn await_output_and_log(tasks: OutputTasks) -> (TaggedOutput, u64, Vec<String>) {
    let OutputTasks {
        output,
        log_tx,
        log_task,
        stdout_task,
        stderr_task,
        dropped_log_bytes,
    } = tasks;
    let mut errors = Vec::new();
    tokio::join!(
        drain_output_task(stdout_task),
        drain_output_task(stderr_task)
    );
    drop(log_tx);
    let omitted_log_bytes = match log_task.await {
        Ok(Ok(omitted)) => omitted,
        Ok(Err(error)) => {
            errors.push(error.to_string());
            0
        }
        Err(error) => {
            errors.push(format!("join guardian log writer: {error}"));
            0
        }
    }
    .saturating_add(dropped_log_bytes.load(Ordering::Relaxed));
    let retained = {
        let mut output = output.lock().await;
        std::mem::replace(&mut *output, TaggedHeadTailBuffer::new(0)).finish()
    };
    (retained, omitted_log_bytes, errors)
}

fn build_final_status(
    start: &StartRequest,
    started_at_ms: u64,
    exit: GuardExit,
    cleanup_errors: Vec<String>,
) -> GuardStatus {
    GuardStatus {
        schema_version: 1,
        task_id: start.task_id.clone(),
        started_at_ms,
        finished_at_ms: unix_time_ms(),
        exit,
        cleanup_errors,
    }
}

struct GuardedBashProcess {
    child: Child,
    #[cfg(unix)]
    process_group: rustix::process::Pid,
    #[cfg(unix)]
    descendants: Vec<ProcessIdentity>,
    #[cfg(windows)]
    job: Option<win32job::Job>,
    #[cfg(windows)]
    _launch_barrier: WindowsLaunchBarrier,
}

impl Drop for GuardedBashProcess {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = signal_group(self.process_group, rustix::process::Signal::KILL);
            let _ = signal_descendants(&self.descendants, rustix::process::Signal::KILL);
        }
        #[cfg(windows)]
        drop(self.job.take());
    }
}

impl GuardedBashProcess {
    fn spawn(start: &StartRequest) -> io::Result<Self> {
        let shell = resolved_shell().map_err(|error| io::Error::other(error.to_string()))?;
        let cwd = std::env::current_dir()?;
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
        let launch_barrier = WindowsLaunchBarrier::new(&start.status_dir);
        #[cfg(windows)]
        let command = format!("{} {command}", launch_barrier.wait_command());

        let mut command_builder = Command::new(&shell.shell_path);
        command_builder
            .arg("-lc")
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env("NO_COLOR", "1")
            .env("TERM", "dumb")
            .env("SHELL", &shell.shell_path);
        if std::env::var_os("GIT_TERMINAL_PROMPT").is_none() {
            command_builder.env("GIT_TERMINAL_PROMPT", "0");
        }
        for name in [
            "CARGO_BUILD_JOBS",
            "NEXTEST_TEST_THREADS",
            "RAYON_NUM_THREADS",
        ] {
            if std::env::var_os(name).is_none() {
                command_builder.env(name, start.limits.max_command_parallelism.to_string());
            }
        }
        if let Some(cwd) = effective_cwd {
            command_builder.current_dir(cwd);
        }
        #[cfg(unix)]
        command_builder.process_group(0);

        #[cfg(windows)]
        let mut child = command_builder.spawn()?;
        #[cfg(not(windows))]
        let child = command_builder.spawn()?;
        #[cfg(unix)]
        let process_group = child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .and_then(rustix::process::Pid::from_raw)
            .ok_or_else(|| io::Error::other("spawned shell has no process group"))?;
        #[cfg(windows)]
        let job = match create_windows_job(&child) {
            Ok(job) => job,
            Err(error) => {
                let _ = child.start_kill();
                return Err(error);
            }
        };
        #[cfg(windows)]
        if let Err(error) = launch_barrier.release() {
            drop(job);
            let _ = child.start_kill();
            return Err(error);
        }
        Ok(Self {
            child,
            #[cfg(unix)]
            process_group,
            #[cfg(unix)]
            descendants: Vec::new(),
            #[cfg(windows)]
            job: Some(job),
            #[cfg(windows)]
            _launch_barrier: launch_barrier,
        })
    }

    async fn terminate_and_wait(
        &mut self,
        sampler: &mut ProcessSampler,
    ) -> io::Result<std::process::ExitStatus> {
        self.refresh_descendants(sampler);
        self.request_termination()?;
        let wait = tokio::time::sleep(TERMINATION_GRACE);
        tokio::pin!(wait);
        let mut exit_status = None;
        loop {
            tokio::select! {
                () = &mut wait => break,
                () = tokio::time::sleep(PROCESS_POLL_INTERVAL), if exit_status.is_none() => {
                    if let Some(status) = self.child.try_wait()? {
                        exit_status = Some(status);
                    }
                }
            }
        }
        self.refresh_descendants(sampler);
        self.force_termination()?;
        match exit_status {
            Some(status) => Ok(status),
            None => self.child.wait().await,
        }
    }

    async fn terminate_remaining_group(&mut self, sampler: &mut ProcessSampler) -> io::Result<()> {
        self.refresh_descendants(sampler);
        self.request_termination()?;
        tokio::time::sleep(TERMINATION_GRACE).await;
        self.refresh_descendants(sampler);
        self.force_termination()
    }

    fn refresh_descendants(&mut self, sampler: &mut ProcessSampler) {
        #[cfg(unix)]
        {
            sampler.refresh_descendants();
            self.set_descendants(sampler.descendants());
        }
        #[cfg(not(unix))]
        let _ = sampler;
    }

    fn request_termination(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        {
            let group = signal_group(self.process_group, rustix::process::Signal::TERM);
            let descendants = signal_descendants(&self.descendants, rustix::process::Signal::TERM);
            group.and(descendants)
        }

        #[cfg(not(unix))]
        {
            #[cfg(windows)]
            drop(self.job.take());
            Ok(())
        }
    }

    fn force_termination(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        {
            let group = signal_group(self.process_group, rustix::process::Signal::KILL);
            let descendants = signal_descendants(&self.descendants, rustix::process::Signal::KILL);
            group.and(descendants)
        }

        #[cfg(not(unix))]
        self.child.start_kill()
    }

    #[cfg(unix)]
    fn set_descendants(&mut self, descendants: Vec<ProcessIdentity>) {
        self.descendants = descendants;
    }
}

#[cfg(windows)]
fn create_windows_job(child: &Child) -> io::Result<win32job::Job> {
    let mut limits = win32job::ExtendedLimitInfo::new();
    limits.limit_kill_on_job_close();
    let job = win32job::Job::create_with_limit_info(&limits)
        .map_err(|error| io::Error::other(format!("create Bash Job Object: {error}")))?;
    let handle = child
        .raw_handle()
        .ok_or_else(|| io::Error::other("spawned Bash process has no handle"))?;
    job.assign_process(handle as isize)
        .map_err(|error| io::Error::other(format!("assign Bash Job Object: {error}")))?;
    Ok(job)
}

#[cfg(unix)]
fn signal_group(group: rustix::process::Pid, signal: rustix::process::Signal) -> io::Result<()> {
    match rustix::process::kill_process_group(group, signal) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        #[cfg(target_os = "macos")]
        Err(rustix::io::Errno::PERM) if signal == rustix::process::Signal::KILL => Ok(()),
        Err(error) => Err(io::Error::other(format!(
            "signal guarded process group with {signal:?}: {error}"
        ))),
    }
}

fn spawn_output_drain<R>(
    mut reader: R,
    stream: StreamKind,
    output: Arc<Mutex<TaggedHeadTailBuffer>>,
    response_tx: mpsc::Sender<GuardResponse>,
    log_tx: mpsc::Sender<Vec<u8>>,
    dropped_log_bytes: Arc<AtomicU64>,
    log_truncated: Arc<AtomicBool>,
) -> JoinHandle<io::Result<()>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut chunk = vec![0; OUTPUT_CHUNK_BYTES];
        loop {
            let read = reader.read(&mut chunk).await?;
            if read == 0 {
                return Ok(());
            }
            output.lock().await.push(stream, &chunk[..read]);
            if log_truncated.load(Ordering::Relaxed) {
                dropped_log_bytes
                    .fetch_add(u64::try_from(read).unwrap_or(u64::MAX), Ordering::Relaxed);
            } else if let Err(mpsc::error::TrySendError::Full(dropped)) =
                log_tx.try_send(chunk[..read].to_vec())
            {
                log_truncated.store(true, Ordering::Relaxed);
                dropped_log_bytes.fetch_add(
                    u64::try_from(dropped.len()).unwrap_or(u64::MAX),
                    Ordering::Relaxed,
                );
            }
            if response_tx.capacity() == response_tx.max_capacity() {
                let _ = response_tx.try_send(GuardResponse::Output {
                    stream,
                    data: chunk[..read].to_vec(),
                });
            }
        }
    })
}

fn spawn_log_writer(
    mut file: tokio::fs::File,
    mut chunks: mpsc::Receiver<Vec<u8>>,
    max_bytes: u64,
) -> JoinHandle<io::Result<u64>> {
    tokio::spawn(async move {
        let mut written = 0u64;
        let mut omitted = 0u64;
        while let Some(chunk) = chunks.recv().await {
            let remaining = max_bytes.saturating_sub(written);
            let retained = chunk
                .len()
                .min(usize::try_from(remaining).unwrap_or(usize::MAX));
            if retained > 0 {
                file.write_all(&chunk[..retained]).await?;
                written = written.saturating_add(u64::try_from(retained).unwrap_or(u64::MAX));
            }
            omitted = omitted.saturating_add(
                u64::try_from(chunk.len().saturating_sub(retained)).unwrap_or(u64::MAX),
            );
        }
        file.flush().await?;
        Ok(omitted)
    })
}

async fn drain_output_task(task: Option<JoinHandle<io::Result<()>>>) {
    let Some(mut task) = task else {
        return;
    };
    if tokio::time::timeout(OUTPUT_DRAIN_GRACE, &mut task)
        .await
        .is_err()
    {
        task.abort();
    }
}

fn guard_exit(
    status: GuardStatusKind,
    exit_status: Option<std::process::ExitStatus>,
    output: &TaggedOutput,
    resource_limit: Option<ResourceLimitDetail>,
    omitted_log_bytes: u64,
) -> GuardExit {
    GuardExit {
        status,
        exit_code: exit_status
            .as_ref()
            .and_then(std::process::ExitStatus::code),
        #[cfg(unix)]
        signal: exit_status
            .as_ref()
            .and_then(std::os::unix::process::ExitStatusExt::signal),
        #[cfg(not(unix))]
        signal: None,
        resource_limit,
        omitted_output_bytes: output.omitted_bytes,
        omitted_log_bytes,
    }
}

pub(super) enum ResourceTick<T> {
    Exited(T),
    Limited(ResourceLimitDetail),
    Running,
}

pub(super) fn check_resource_tick<T>(
    mut try_wait: impl FnMut() -> io::Result<Option<T>>,
    sample: impl FnOnce() -> Option<ResourceLimitDetail>,
) -> io::Result<ResourceTick<T>> {
    if let Some(status) = try_wait()? {
        return Ok(ResourceTick::Exited(status));
    }
    let tick = match sample() {
        Some(limit) if limit.cause == ResourceLimitCause::SamplerUnavailable => {
            if let Some(status) = try_wait()? {
                return Ok(ResourceTick::Exited(status));
            }
            ResourceTick::Limited(limit)
        }
        Some(limit) => ResourceTick::Limited(limit),
        None => ResourceTick::Running,
    };
    Ok(tick)
}

pub(super) struct ProcessSampler {
    root: SystemPid,
    system: System,
    descendants: Vec<ProcessIdentity>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ProcessIdentity {
    pid: SystemPid,
    start_time: u64,
}

#[derive(Debug, Clone, Copy)]
struct ProcessRecord {
    pid: SystemPid,
    start_time: u64,
    parent: Option<SystemPid>,
}

#[derive(Debug, Clone, Copy)]
struct ProcessSample {
    root_present: bool,
    descendants: usize,
    total_memory: u64,
    resident_memory: u64,
}

pub(super) fn process_start_id(pid: u32) -> u64 {
    let pid = SystemPid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).map_or(0, sysinfo::Process::start_time)
}

impl ProcessSampler {
    pub(super) fn new(root: u32) -> Self {
        Self {
            root: SystemPid::from_u32(root),
            system: System::new(),
            descendants: Vec::new(),
        }
    }

    pub(super) fn exceeded_limit(&mut self, start: &StartRequest) -> Option<ResourceLimitDetail> {
        let snapshot_available = self.refresh_descendants();
        self.system.refresh_memory();
        let root_present = self.system.process(self.root).is_some();
        let descendants = self.descendants.len();
        let total_memory = self.system.total_memory();
        let resident_memory = self.descendants.iter().fold(
            self.system
                .process(self.root)
                .map_or(0, sysinfo::Process::memory),
            |total, identity| {
                total.saturating_add(
                    self.system
                        .process(identity.pid)
                        .map_or(0, sysinfo::Process::memory),
                )
            },
        );
        let sample = snapshot_available.then_some(ProcessSample {
            root_present,
            descendants,
            total_memory,
            resident_memory,
        });
        resource_limit_for_sample(&start.limits, sample)
    }

    pub(super) fn refresh_descendants(&mut self) -> bool {
        let refreshed = self.system.refresh_processes(ProcessesToUpdate::All, true);
        let snapshot = self
            .system
            .processes()
            .iter()
            .map(|(pid, process)| ProcessRecord {
                pid: *pid,
                start_time: process.start_time(),
                parent: process.parent(),
            })
            .collect::<Vec<_>>();
        self.descendants = tracked_descendants(self.root, &self.descendants, &snapshot);
        refreshed > 0
    }

    #[cfg(unix)]
    pub(super) fn descendants(&self) -> Vec<ProcessIdentity> {
        self.descendants.clone()
    }
}

fn tracked_descendants(
    root: SystemPid,
    known: &[ProcessIdentity],
    snapshot: &[ProcessRecord],
) -> Vec<ProcessIdentity> {
    let mut tree = HashSet::new();
    if snapshot.iter().any(|process| process.pid == root) {
        tree.insert(root);
    }
    for identity in known {
        if snapshot
            .iter()
            .any(|process| process.pid == identity.pid && process.start_time == identity.start_time)
        {
            tree.insert(identity.pid);
        }
    }
    loop {
        let before = tree.len();
        for process in snapshot {
            if process.parent.is_some_and(|parent| tree.contains(&parent)) {
                tree.insert(process.pid);
            }
        }
        if tree.len() == before {
            break;
        }
    }
    snapshot
        .iter()
        .filter(|process| process.pid != root && tree.contains(&process.pid))
        .map(|process| ProcessIdentity {
            pid: process.pid,
            start_time: process.start_time,
        })
        .collect()
}

#[cfg(unix)]
pub(super) fn signal_descendants(
    descendants: &[ProcessIdentity],
    signal: rustix::process::Signal,
) -> io::Result<()> {
    if descendants.is_empty() {
        return Ok(());
    }
    let pids = descendants.iter().map(|item| item.pid).collect::<Vec<_>>();
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&pids), true);
    for identity in descendants {
        if system
            .process(identity.pid)
            .is_none_or(|process| process.start_time() != identity.start_time)
        {
            continue;
        }
        let Some(pid) = i32::try_from(identity.pid.as_u32())
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        else {
            continue;
        };
        match rustix::process::kill_process(pid, signal) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => {}
            #[cfg(target_os = "macos")]
            Err(rustix::io::Errno::PERM) if signal == rustix::process::Signal::KILL => {}
            Err(error) => {
                return Err(io::Error::other(format!(
                    "signal sampled descendant {} with {signal:?}: {error}",
                    identity.pid
                )));
            }
        }
    }
    Ok(())
}

fn resource_limit_for_sample(
    limits: &super::GuardLimits,
    sample: Option<ProcessSample>,
) -> Option<ResourceLimitDetail> {
    let Some(sample) = sample else {
        return Some(ResourceLimitDetail {
            cause: ResourceLimitCause::SamplerUnavailable,
            configured: None,
            observed: None,
        });
    };
    if !sample.root_present || sample.total_memory == 0 {
        return Some(ResourceLimitDetail {
            cause: ResourceLimitCause::SamplerUnavailable,
            configured: None,
            observed: None,
        });
    }
    if sample.descendants > limits.max_command_descendant_processes {
        return Some(ResourceLimitDetail {
            cause: ResourceLimitCause::ProcessCount,
            configured: Some(
                u64::try_from(limits.max_command_descendant_processes).unwrap_or(u64::MAX),
            ),
            observed: Some(u64::try_from(sample.descendants).unwrap_or(u64::MAX)),
        });
    }
    let memory_percent =
        u64::try_from((u128::from(sample.resident_memory) * 100) / u128::from(sample.total_memory))
            .unwrap_or(u64::MAX);
    (memory_percent > u64::from(limits.max_command_memory_percent)).then_some(ResourceLimitDetail {
        cause: ResourceLimitCause::TreeMemory,
        configured: Some(u64::from(limits.max_command_memory_percent)),
        observed: Some(memory_percent),
    })
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
    write_file_atomic_status(&running_status_path(start), &content)
}

fn running_status_path(start: &StartRequest) -> PathBuf {
    start
        .status_dir
        .join(format!("{}.running.json", start.task_id))
}

fn final_status_path(start: &StartRequest) -> PathBuf {
    start
        .status_dir
        .join(format!("{}.status.json", start.task_id))
}

fn log_path(start: &StartRequest) -> PathBuf {
    start.status_dir.join(format!("{}.log", start.task_id))
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn protocol_is_eof(error: &ProtocolError) -> bool {
    matches!(error, ProtocolError::Io(error) if error.kind() == io::ErrorKind::UnexpectedEof)
}

fn protocol_io_error(error: ProtocolError) -> io::Error {
    match error {
        ProtocolError::Io(error) => error,
        other => io::Error::new(io::ErrorKind::InvalidData, other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_tick_checks_exit_before_sampling() {
        let sampled = std::cell::Cell::new(false);

        let result = check_resource_tick(
            || Ok(Some(0)),
            || {
                sampled.set(true);
                None
            },
        )
        .unwrap();

        assert!(matches!(result, ResourceTick::Exited(0)));
        assert!(!sampled.get());
    }

    #[test]
    fn resource_tick_rechecks_exit_after_sampling_loses_root() {
        let waits = std::cell::Cell::new(0);

        let result = check_resource_tick(
            || {
                let call = waits.get();
                waits.set(call + 1);
                Ok((call == 1).then_some(0))
            },
            || {
                Some(ResourceLimitDetail {
                    cause: ResourceLimitCause::SamplerUnavailable,
                    configured: None,
                    observed: None,
                })
            },
        )
        .unwrap();

        assert!(matches!(result, ResourceTick::Exited(0)));
        assert_eq!(waits.get(), 2);
    }

    #[test]
    fn full_response_queue_reports_busy_delivery_failure() {
        let (response_tx, _response_rx) = mpsc::channel(1);
        response_tx
            .try_send(GuardResponse::Ack { request_id: 1 })
            .unwrap();

        let error = try_send_response(&response_tx, GuardResponse::Busy { request_id: 2 })
            .expect_err("Busy must not be silently dropped");

        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    }

    #[cfg(unix)]
    struct BrokenWriter;

    #[cfg(unix)]
    impl tokio::io::AsyncWrite for BrokenWriter {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &[u8],
        ) -> std::task::Poll<io::Result<usize>> {
            std::task::Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "response reader closed",
            )))
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn response_writer_break_terminates_quiet_command() {
        let workspace = tempfile::tempdir().unwrap();
        let start = StartRequest {
            task_id: "quiet-writer-break".to_owned(),
            kind: GuardTaskKind::Bash,
            command: "sleep 30".to_owned(),
            limits: super::super::ShellRuntime::default()
                .guard_limits(Some(Duration::from_secs(30)), 1024),
            status_dir: workspace.path().to_path_buf(),
            cols: None,
            rows: None,
        };
        let (mut request_writer, request_reader) = tokio::io::duplex(4096);
        super::super::protocol::write_request(
            &mut request_writer,
            &GuardRequest::Start {
                request_id: 1,
                request: start,
            },
        )
        .await
        .unwrap();

        tokio::time::timeout(
            Duration::from_secs(3),
            run_process_guard_io(request_reader, BrokenWriter),
        )
        .await
        .expect("writer failure must terminate a quiet command")
        .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn descendant_refresh_survives_missing_leader_and_finds_late_fork() {
        let root = SystemPid::from_u32(10);
        let known = vec![
            ProcessIdentity {
                pid: SystemPid::from_u32(20),
                start_time: 2,
            },
            ProcessIdentity {
                pid: SystemPid::from_u32(40),
                start_time: 4,
            },
        ];
        let snapshot = vec![
            ProcessRecord {
                pid: SystemPid::from_u32(20),
                start_time: 2,
                parent: Some(SystemPid::from_u32(1)),
            },
            ProcessRecord {
                pid: SystemPid::from_u32(30),
                start_time: 3,
                parent: Some(SystemPid::from_u32(20)),
            },
            ProcessRecord {
                pid: SystemPid::from_u32(40),
                start_time: 400,
                parent: Some(SystemPid::from_u32(1)),
            },
            ProcessRecord {
                pid: SystemPid::from_u32(50),
                start_time: 5,
                parent: Some(SystemPid::from_u32(40)),
            },
        ];

        let mut tracked = tracked_descendants(root, &known, &snapshot)
            .into_iter()
            .map(|identity| identity.pid.as_u32())
            .collect::<Vec<_>>();
        tracked.sort_unstable();

        assert_eq!(tracked, vec![20, 30]);
    }

    #[test]
    fn sampler_snapshot_root_and_memory_unavailable_are_fail_closed() {
        let limits =
            super::super::ShellRuntime::default().guard_limits(Some(Duration::from_secs(1)), 1);
        let unavailable = Some(ResourceLimitDetail {
            cause: ResourceLimitCause::SamplerUnavailable,
            configured: None,
            observed: None,
        });

        assert_eq!(resource_limit_for_sample(&limits, None), unavailable);
        assert_eq!(
            resource_limit_for_sample(
                &limits,
                Some(ProcessSample {
                    root_present: false,
                    descendants: 0,
                    total_memory: 1,
                    resident_memory: 0,
                }),
            ),
            unavailable
        );
        assert_eq!(
            resource_limit_for_sample(
                &limits,
                Some(ProcessSample {
                    root_present: true,
                    descendants: 0,
                    total_memory: 0,
                    resident_memory: 0,
                }),
            ),
            unavailable
        );
    }
}
