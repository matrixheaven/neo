use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::error::WorkflowError;
use super::journal::{
    self, IncompleteInvocation, JournalRecord, JournalWriter, canonical_input_hash,
};
use super::limits::WorkflowLimits;
use super::state::{
    WorkflowActor, WorkflowId, WorkflowInvocationKind, WorkflowInvocationOutcome, WorkflowPhase,
    WorkflowRunMetadata, WorkflowSnapshot, WorkflowState,
};
use crate::AgentTokenUsage;
use crate::runtime::{WorkflowNotification, WorkflowNotificationQueue};

#[path = "runtime_support.rs"]
mod support;
use support::{
    ReplayEntry, RunControl, add_usage, aggregate_usage, bounded_summary,
    compact_resource_limited_outcome, current_timestamp_ms, interrupted_outcome, last_state,
    latest_log_summary, latest_report_summary, projection_timestamps, recovered_phase,
    recovered_reports, replay_entries, report_summary, resource_limited_outcome, usage_total,
};
pub use support::{ReplayPrefix, compute_replay_prefix};

type RunnerFuture = Pin<Box<dyn Future<Output = Result<(), WorkflowError>> + Send>>;
type Runner = dyn Fn(WorkflowHandle, WorkflowRunMetadata, PathBuf) -> RunnerFuture + Send + Sync;
type RecoveryFuture = Pin<Box<dyn Future<Output = Option<WorkflowInvocationOutcome>> + Send>>;
type RecoveryResolver = dyn Fn(Arc<IncompleteInvocation>) -> RecoveryFuture + Send + Sync;
type ProjectionEmitter = dyn Fn(&Path, WorkflowProjectionStage, WorkflowSnapshot) + Send + Sync;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowProjectionStage {
    Started,
    Updated,
    Finished,
}

#[derive(Debug, Clone)]
pub struct WorkflowInvocationContext {
    pub invocation_id: String,
    pub cancel_token: CancellationToken,
}

#[derive(Debug, Clone)]
pub struct WorkflowLaunchRequest {
    pub name: String,
    pub description: String,
    pub phases: Vec<WorkflowPhase>,
    pub script: String,
    pub args: serde_json::Value,
    pub launch_source: String,
    pub parent_run_id: Option<WorkflowId>,
}

fn metadata_for_request(run_id: WorkflowId, request: WorkflowLaunchRequest) -> WorkflowRunMetadata {
    use sha2::{Digest, Sha256};

    let script_sha256 = format!("{:x}", Sha256::digest(request.script.as_bytes()));
    WorkflowRunMetadata {
        run_id,
        parent_run_id: request.parent_run_id,
        name: request.name,
        description: request.description,
        phases: request.phases,
        script: request.script,
        script_sha256,
        args: request.args,
        launch_source: request.launch_source,
        journal_format_version: 1,
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkflowOutput {
    pub metadata: WorkflowRunMetadata,
    pub state: WorkflowState,
    pub current_phase: Option<String>,
    pub invocations: Vec<JournalRecord>,
    pub failure_count: u64,
    pub actual_usage: Option<AgentTokenUsage>,
    pub terminal_reason: Option<String>,
    pub reports: Vec<serde_json::Value>,
}

struct RunState {
    metadata: WorkflowRunMetadata,
    state: WorkflowState,
    current_phase: Option<String>,
    invocation_count: u64,
    failure_count: u64,
    actual_usage: Option<AgentTokenUsage>,
    projection_sequence: Option<u64>,
    started_at_ms: Option<u64>,
    updated_at_ms: Option<u64>,
    latest_log_summary: Option<String>,
    latest_report_summary: Option<String>,
    terminal_reason: Option<String>,
    reports: Vec<serde_json::Value>,
    run_dir: PathBuf,
    control: Arc<RunControl>,
    worker_active: bool,
    current_invocation: Option<String>,
    replay_entries: Vec<ReplayEntry>,
    replay_cursor: usize,
    replay_live: bool,
    journal: Option<JournalWriter>,
}

impl RunState {
    fn snapshot(&self) -> WorkflowSnapshot {
        WorkflowSnapshot {
            id: self.metadata.run_id.clone(),
            title: self.metadata.name.clone(),
            state: self.state,
            current_phase: self.current_phase.clone(),
            projection_sequence: self.projection_sequence,
            recovery_failure: self.journal.is_none(),
            started_at_ms: self.started_at_ms,
            updated_at_ms: self.updated_at_ms,
            invocation_count: self.invocation_count,
            failure_count: self.failure_count,
            actual_usage: self.actual_usage,
            latest_log_summary: self.latest_log_summary.clone(),
            latest_report_summary: self.latest_report_summary.clone(),
            terminal_reason: self.terminal_reason.clone(),
            steps: Vec::new(),
        }
    }

    fn journal_path(&self) -> PathBuf {
        self.run_dir.join("journal.jsonl")
    }
}

#[derive(Clone)]
pub struct WorkflowRuntime {
    runs: Arc<Mutex<HashMap<String, Arc<Mutex<RunState>>>>>,
    limits: WorkflowLimits,
    notifications: WorkflowNotificationQueue,
    runner: Arc<RwLock<Option<Arc<Runner>>>>,
    recovery_resolver: Arc<RwLock<Option<Arc<RecoveryResolver>>>>,
    projection_emitter: Arc<RwLock<Option<Arc<ProjectionEmitter>>>>,
    #[cfg(test)]
    rollback_remove_failure: Arc<AtomicBool>,
}

impl std::fmt::Debug for WorkflowRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkflowRuntime")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl Default for WorkflowRuntime {
    fn default() -> Self {
        Self::new(WorkflowLimits::default())
    }
}

impl WorkflowRuntime {
    #[must_use]
    pub fn new(limits: WorkflowLimits) -> Self {
        Self {
            runs: Arc::new(Mutex::new(HashMap::new())),
            limits,
            notifications: WorkflowNotificationQueue::default(),
            runner: Arc::new(RwLock::new(None)),
            recovery_resolver: Arc::new(RwLock::new(None)),
            projection_emitter: Arc::new(RwLock::new(None)),
            #[cfg(test)]
            rollback_remove_failure: Arc::new(AtomicBool::new(false)),
        }
    }

    #[must_use]
    pub fn notification_queue(&self) -> WorkflowNotificationQueue {
        self.notifications.clone()
    }

    /// Bind the production worker supplied by the Lua/dispatch composition root.
    pub fn bind_runner<F, Fut>(&self, runner: F) -> Result<(), WorkflowError>
    where
        F: Fn(WorkflowHandle, WorkflowRunMetadata, PathBuf) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), WorkflowError>> + Send + 'static,
    {
        let mut slot = self
            .runner
            .write()
            .map_err(|_| WorkflowError::Host("workflow runner lock poisoned".to_owned()))?;
        if slot.is_some() {
            return Err(WorkflowError::InvalidInput(
                "workflow runner is already bound".to_owned(),
            ));
        }
        *slot = Some(Arc::new(move |handle, metadata, session_dir| {
            Box::pin(runner(handle, metadata, session_dir))
        }));
        Ok(())
    }

    /// Bind the shared production runner once. Repeated calls are harmless;
    /// the runner resolves live dependencies per session when a worker starts.
    pub fn bind_runner_if_unbound<F, Fut>(&self, runner: F) -> Result<(), WorkflowError>
    where
        F: Fn(WorkflowHandle, WorkflowRunMetadata, PathBuf) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), WorkflowError>> + Send + 'static,
    {
        let mut slot = self
            .runner
            .write()
            .map_err(|_| WorkflowError::Host("workflow runner lock poisoned".to_owned()))?;
        if slot.is_none() {
            *slot = Some(Arc::new(move |handle, metadata, session_dir| {
                Box::pin(runner(handle, metadata, session_dir))
            }));
        }
        Ok(())
    }

    pub fn bind_projection_emitter_if_unbound<F>(&self, emitter: F) -> Result<(), WorkflowError>
    where
        F: Fn(&Path, WorkflowProjectionStage, WorkflowSnapshot) + Send + Sync + 'static,
    {
        let mut slot = self.projection_emitter.write().map_err(|_| {
            WorkflowError::Host("workflow projection emitter lock poisoned".to_owned())
        })?;
        if slot.is_none() {
            *slot = Some(Arc::new(emitter));
        }
        Ok(())
    }

    #[must_use]
    pub fn limits(&self) -> WorkflowLimits {
        self.limits.clone()
    }

    /// Validate every pure launch boundary before capability reservation.
    pub fn validate_launch_request(
        &self,
        request: &WorkflowLaunchRequest,
    ) -> Result<(), WorkflowError> {
        if u64::try_from(request.script.len()).unwrap_or(u64::MAX) > self.limits.lua_source_bytes {
            return Err(WorkflowError::InvalidInput(format!(
                "script size {} exceeds limit {}",
                request.script.len(),
                self.limits.lua_source_bytes
            )));
        }
        let metadata = metadata_for_request(
            WorkflowId(format!("wf_{}", "0".repeat(32))),
            request.clone(),
        );
        let bytes = u64::try_from(
            serde_json::to_vec_pretty(&metadata)
                .map_err(|error| WorkflowError::InvalidInput(error.to_string()))?
                .len(),
        )
        .unwrap_or(u64::MAX);
        if bytes > self.limits.journal_record_bytes {
            return Err(WorkflowError::InvalidInput(format!(
                "run.json size {bytes} exceeds 16 MiB record limit"
            )));
        }
        Ok(())
    }

    /// Bind a read-only child-result lookup used only during host-exit recovery.
    pub fn bind_recovery_resolver<F, Fut>(&self, resolver: F) -> Result<(), WorkflowError>
    where
        F: Fn(Arc<IncompleteInvocation>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<WorkflowInvocationOutcome>> + Send + 'static,
    {
        let mut slot = self
            .recovery_resolver
            .write()
            .map_err(|_| WorkflowError::Host("workflow recovery lock poisoned".to_owned()))?;
        if slot.is_some() {
            return Err(WorkflowError::InvalidInput(
                "workflow recovery resolver is already bound".to_owned(),
            ));
        }
        *slot = Some(Arc::new(move |invocation| Box::pin(resolver(invocation))));
        Ok(())
    }

    pub async fn create_run(
        &self,
        session_dir: &Path,
        request: WorkflowLaunchRequest,
    ) -> Result<WorkflowHandle, WorkflowError> {
        self.validate_launch_request(&request)?;

        let (run_id, run_dir) = loop {
            let run_id = WorkflowId(format!("wf_{}", uuid::Uuid::new_v4().as_simple()));
            let run_dir = journal::run_dir(session_dir, &run_id);
            if !run_dir.exists() {
                break (run_id, run_dir);
            }
        };
        let metadata = metadata_for_request(run_id.clone(), request);

        let durable_create = (|| {
            journal::write_run_metadata(&run_dir, &metadata, &self.limits)?;
            let mut writer = JournalWriter::open(&run_dir.join("journal.jsonl"))?;
            let timestamp_ms = current_timestamp_ms();
            let sequence = writer.append(
                &JournalRecord::StateChanged {
                    seq: writer.next_seq(),
                    timestamp_ms,
                    previous: WorkflowState::Running,
                    new: WorkflowState::Running,
                    reason: "launch".to_owned(),
                    actor: WorkflowActor::Runtime,
                },
                &self.limits,
            )?;
            Ok::<_, WorkflowError>((writer, sequence, timestamp_ms))
        })();
        let (writer, projection_sequence, started_at_ms) = match durable_create {
            Ok(durable) => durable,
            Err(error) => {
                return match std::fs::remove_dir_all(&run_dir) {
                    Ok(()) => Err(error),
                    Err(cleanup) if cleanup.kind() == std::io::ErrorKind::NotFound => Err(error),
                    Err(cleanup) => Err(WorkflowError::Journal(format!(
                        "{error}; failed to clean incomplete run {}: {cleanup}",
                        run_dir.display()
                    ))),
                };
            }
        };

        let control = Arc::new(RunControl::new());
        let state = Arc::new(Mutex::new(RunState {
            metadata,
            state: WorkflowState::Running,
            current_phase: None,
            invocation_count: 0,
            failure_count: 0,
            actual_usage: None,
            projection_sequence: Some(projection_sequence),
            started_at_ms: Some(started_at_ms),
            updated_at_ms: Some(started_at_ms),
            latest_log_summary: None,
            latest_report_summary: None,
            terminal_reason: None,
            reports: Vec::new(),
            run_dir,
            control: Arc::clone(&control),
            worker_active: false,
            current_invocation: None,
            replay_entries: Vec::new(),
            replay_cursor: 0,
            replay_live: false,
            journal: Some(writer),
        }));
        self.runs
            .lock()
            .await
            .insert(run_id.0.clone(), Arc::clone(&state));

        let handle = WorkflowHandle {
            run_id: run_id.clone(),
            control,
            runtime: self.clone(),
        };
        Ok(handle)
    }

    pub async fn emit_started(&self, run_id: &WorkflowId) -> Result<(), WorkflowError> {
        let state = self.run_state(run_id).await?;
        let guard = state.lock().await;
        self.emit_projection(&guard, WorkflowProjectionStage::Started);
        Ok(())
    }

    /// Remove a just-created, never-started run when task registration fails.
    pub async fn rollback_created_run(&self, run_id: &WorkflowId) -> Result<(), WorkflowError> {
        let state = self.run_state(run_id).await?;
        let run_dir = {
            let guard = state.lock().await;
            if guard.worker_active {
                return Err(WorkflowError::InvalidInput(
                    "cannot roll back a started workflow".to_owned(),
                ));
            }
            guard.run_dir.clone()
        };
        #[cfg(test)]
        if self.rollback_remove_failure.load(Ordering::Acquire) {
            return Err(WorkflowError::Journal(
                "injected rollback removal failure".to_owned(),
            ));
        }
        std::fs::remove_dir_all(&run_dir)
            .map_err(|error| WorkflowError::Journal(error.to_string()))?;
        self.runs.lock().await.remove(&run_id.0);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn inject_rollback_remove_failure(&self) {
        self.rollback_remove_failure.store(true, Ordering::Release);
    }

    /// Persist a terminal failure if worker startup fails after capability
    /// commit. The registered task remains inspectable through `TaskOutput`.
    pub async fn fail_worker_start(
        &self,
        run_id: &WorkflowId,
        error: &WorkflowError,
    ) -> Result<(), WorkflowError> {
        self.finish_worker(run_id, Err(WorkflowError::Host(error.to_string())))
            .await
    }

    pub async fn start_worker(&self, run_id: &WorkflowId) -> Result<(), WorkflowError> {
        let runner = self.bound_runner()?.ok_or_else(|| {
            WorkflowError::InvalidInput("workflow runner is not bound".to_owned())
        })?;
        let state = self.run_state(run_id).await?;
        let (handle, metadata, session_dir) = {
            let mut guard = state.lock().await;
            if guard.state != WorkflowState::Running {
                return Err(WorkflowError::InvalidInput(
                    "worker can only start for a running workflow".to_owned(),
                ));
            }
            if guard.worker_active {
                return Err(WorkflowError::InvalidInput(
                    "workflow worker is already active".to_owned(),
                ));
            }
            guard.worker_active = true;
            let session_dir = guard
                .run_dir
                .parent()
                .and_then(Path::parent)
                .ok_or_else(|| {
                    WorkflowError::Host("workflow run directory has no session parent".to_owned())
                })?
                .to_path_buf();
            (
                WorkflowHandle {
                    run_id: run_id.clone(),
                    control: Arc::clone(&guard.control),
                    runtime: self.clone(),
                },
                guard.metadata.clone(),
                session_dir,
            )
        };
        let runtime = self.clone();
        let id = run_id.clone();
        tokio::spawn(async move {
            let result = runner(handle, metadata, session_dir).await;
            let _ = runtime.finish_worker(&id, result).await;
        });
        Ok(())
    }

    pub async fn snapshot(&self, run_id: &WorkflowId) -> Result<WorkflowSnapshot, WorkflowError> {
        Ok(self.run_state(run_id).await?.lock().await.snapshot())
    }

    pub async fn output(&self, run_id: &WorkflowId) -> Result<WorkflowOutput, WorkflowError> {
        let state = self.run_state(run_id).await?;
        let guard = state.lock().await;
        let invocations = if guard.journal.is_none() {
            Vec::new()
        } else {
            journal::read_journal(&guard.journal_path())?
        };
        Ok(WorkflowOutput {
            metadata: guard.metadata.clone(),
            state: guard.state,
            current_phase: guard.current_phase.clone(),
            invocations,
            failure_count: guard.failure_count,
            actual_usage: guard.actual_usage,
            terminal_reason: guard.terminal_reason.clone(),
            reports: guard.reports.clone(),
        })
    }

    pub async fn pause(
        &self,
        run_id: &WorkflowId,
        actor: WorkflowActor,
    ) -> Result<(), WorkflowError> {
        let state = self.run_state(run_id).await?;
        let mut guard = state.lock().await;
        if guard.state.is_terminal() {
            return Err(WorkflowError::InvalidInput(
                "cannot pause a terminal workflow".to_owned(),
            ));
        }
        guard.control.request_pause(actor)?;
        if !guard.worker_active {
            let pause_actor = guard.control.pause_actor()?;
            self.transition_locked(&mut guard, WorkflowState::Paused, "pause", pause_actor)?;
        }
        Ok(())
    }

    pub async fn resume(
        &self,
        run_id: &WorkflowId,
        actor: WorkflowActor,
    ) -> Result<(), WorkflowError> {
        if self.bound_runner()?.is_none() {
            return Err(WorkflowError::InvalidInput(
                "workflow runner is not bound".to_owned(),
            ));
        }
        let state = self.run_state(run_id).await?;
        {
            let mut guard = state.lock().await;
            if guard.state != WorkflowState::Paused {
                return Err(WorkflowError::InvalidInput(
                    "can only resume a paused workflow".to_owned(),
                ));
            }
            guard.control.clear_pause()?;
            guard.replay_entries = replay_entries(&journal::read_journal(&guard.journal_path())?);
            guard.replay_cursor = 0;
            guard.replay_live = false;
            self.transition_locked(&mut guard, WorkflowState::Running, "resume", actor)?;
        }
        self.start_worker(run_id).await
    }

    pub async fn stop(
        &self,
        run_id: &WorkflowId,
        actor: WorkflowActor,
    ) -> Result<(), WorkflowError> {
        let state = self.run_state(run_id).await?;
        let mut guard = state.lock().await;
        if guard.state.is_terminal() {
            return Err(WorkflowError::InvalidInput(
                "cannot stop a terminal workflow".to_owned(),
            ));
        }
        guard.control.request_stop(actor)?;
        if !guard.worker_active && guard.current_invocation.is_none() {
            let stop_actor = guard.control.stop_actor()?;
            self.transition_locked(
                &mut guard,
                WorkflowState::Cancelled,
                "stopped by user/model",
                stop_actor,
            )?;
        }
        Ok(())
    }

    async fn rehydrate_run_entry(
        &self,
        entry: std::fs::DirEntry,
        handles: &mut Vec<WorkflowHandle>,
    ) -> Result<(), WorkflowError> {
        let run_dir = entry.path();
        if !run_dir.is_dir() {
            return Ok(());
        }
        let fallback_id = WorkflowId(entry.file_name().to_string_lossy().into_owned());
        let existing = self.runs.lock().await.get(&fallback_id.0).cloned();
        if let Some(existing) = existing {
            let guard = existing.lock().await;
            if guard.run_dir != run_dir {
                return Err(WorkflowError::Journal(format!(
                    "workflow {} is already registered from {} instead of {}",
                    fallback_id,
                    guard.run_dir.display(),
                    run_dir.display()
                )));
            }
            handles.push(WorkflowHandle {
                run_id: fallback_id,
                control: Arc::clone(&guard.control),
                runtime: self.clone(),
            });
            return Ok(());
        }
        let metadata = match journal::read_run_metadata(&run_dir) {
            Ok(metadata) if metadata.run_id == fallback_id => metadata,
            Ok(_) => {
                handles.push(
                    self.insert_corrupt_run(
                        run_dir,
                        fallback_id,
                        "run metadata id does not match directory".to_owned(),
                    )
                    .await,
                );
                return Ok(());
            }
            Err(error) => {
                handles.push(
                    self.insert_corrupt_run(run_dir, fallback_id, error.to_string())
                        .await,
                );
                return Ok(());
            }
        };
        let journal_path = run_dir.join("journal.jsonl");
        let mut records = match journal::read_journal(&journal_path) {
            Ok(records) if !records.is_empty() => records,
            Ok(_) => {
                handles.push(
                    self.insert_failed_run(
                        run_dir,
                        metadata,
                        "corrupt journal: missing initial state".to_owned(),
                    )
                    .await,
                );
                return Ok(());
            }
            Err(error) => {
                handles.push(
                    self.insert_failed_run(run_dir, metadata, format!("corrupt journal: {error}"))
                        .await,
                );
                return Ok(());
            }
        };
        let mut writer = JournalWriter::open(&journal_path)?;
        let incomplete = journal::find_incomplete_invocations(&records);
        if !incomplete.is_empty() {
            let resolver = self.bound_recovery_resolver()?;
            for invocation in incomplete {
                let invocation = Arc::new(invocation);
                let outcome = if let Some(resolver) = resolver.as_ref() {
                    resolver(Arc::clone(&invocation))
                        .await
                        .unwrap_or_else(|| interrupted_outcome(&invocation))
                } else {
                    interrupted_outcome(&invocation)
                };
                let record = JournalRecord::InvocationFinished {
                    seq: writer.next_seq(),
                    timestamp_ms: current_timestamp_ms(),
                    invocation_id: invocation.invocation_id.clone(),
                    outcome,
                };
                writer.append(&record, &self.limits)?;
                records.push(record);
            }
        }

        let (last_state, last_reason) = last_state(&records);
        let final_state = if last_state == WorkflowState::Running {
            let record = JournalRecord::StateChanged {
                seq: writer.next_seq(),
                timestamp_ms: current_timestamp_ms(),
                previous: WorkflowState::Running,
                new: WorkflowState::Paused,
                reason: "host_exit".to_owned(),
                actor: WorkflowActor::Runtime,
            };
            writer.append(&record, &self.limits)?;
            records.push(record);
            WorkflowState::Paused
        } else {
            last_state
        };
        let terminal_reason = if last_state == WorkflowState::Running {
            Some("host_exit".to_owned())
        } else if final_state == WorkflowState::Paused || final_state.is_terminal() {
            last_reason
        } else {
            None
        };
        handles.push(
            self.insert_rehydrated_run(
                run_dir,
                metadata,
                records,
                final_state,
                terminal_reason,
                writer,
            )
            .await,
        );
        Ok(())
    }

    pub async fn rehydrate(
        &self,
        session_dir: &Path,
    ) -> Result<Vec<WorkflowHandle>, WorkflowError> {
        let workflows_dir = session_dir.join("workflows");
        if !workflows_dir.exists() {
            return Ok(Vec::new());
        }

        let mut entries = std::fs::read_dir(&workflows_dir)
            .map_err(|error| WorkflowError::Journal(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| WorkflowError::Journal(error.to_string()))?;
        entries.sort_by_key(std::fs::DirEntry::file_name);

        let mut handles = Vec::new();
        for entry in entries {
            self.rehydrate_run_entry(entry, &mut handles).await?;
        }
        for handle in &handles {
            let snapshot = handle.snapshot().await;
            if snapshot.state.is_terminal()
                || (snapshot.state == WorkflowState::Paused
                    && snapshot.terminal_reason.as_deref() == Some("host_exit"))
            {
                let _ = self.notifications.enqueue(WorkflowNotification::new(
                    session_dir,
                    snapshot.id,
                    snapshot.state,
                    snapshot
                        .terminal_reason
                        .unwrap_or_else(|| "terminal".to_owned()),
                ));
            }
        }
        Ok(handles)
    }

    async fn invoke<F, Fut>(
        &self,
        run_id: &WorkflowId,
        call_index: u64,
        kind: WorkflowInvocationKind,
        canonical_input: serde_json::Value,
        provider_backed: bool,
        effect: F,
    ) -> Result<WorkflowInvocationOutcome, WorkflowError>
    where
        F: FnOnce(WorkflowInvocationContext) -> Fut + Send,
        Fut: Future<Output = WorkflowInvocationOutcome> + Send,
    {
        let state = self.run_state(run_id).await?;
        let input_hash = canonical_input_hash(&canonical_input);
        let (invocation_id, control, capped) = {
            let mut guard = state.lock().await;
            if guard.state != WorkflowState::Running {
                return Err(WorkflowError::InvalidInput(
                    "workflow invocation requires running state".to_owned(),
                ));
            }
            if guard.control.pause_requested.load(Ordering::Acquire) {
                return Err(WorkflowError::InvalidInput(
                    "workflow paused at invocation boundary".to_owned(),
                ));
            }
            if guard.control.stop_token.is_cancelled() {
                return Err(WorkflowError::InvalidInput(
                    "workflow stop requested".to_owned(),
                ));
            }
            if !guard.replay_live {
                if let Some(entry) = guard.replay_entries.get(guard.replay_cursor)
                    && entry.call_index == call_index
                    && entry.kind == kind
                    && entry.canonical_input_hash == input_hash
                {
                    let outcome = entry.outcome.clone();
                    guard.replay_cursor += 1;
                    return Ok(outcome);
                }
                guard.replay_live = true;
            }

            let capped = provider_backed
                && self
                    .limits
                    .token_cap
                    .is_some_and(|cap| usage_total(guard.actual_usage) >= cap);
            let invocation_id = format!("inv_{}", uuid::Uuid::new_v4().as_simple());
            self.write_invocation_started(
                &mut guard,
                invocation_id.clone(),
                call_index,
                kind,
                canonical_input,
                input_hash,
            )?;
            guard.invocation_count = guard.invocation_count.saturating_add(1);
            guard.current_invocation = Some(invocation_id.clone());
            (invocation_id, Arc::clone(&guard.control), capped)
        };

        let outcome = if capped {
            resource_limited_outcome("workflow actual token cap reached")
        } else {
            effect(WorkflowInvocationContext {
                invocation_id: invocation_id.clone(),
                cancel_token: control.stop_token.clone(),
            })
            .await
        };

        self.finalize_invocation_locked(state, invocation_id, kind, outcome, capped)
            .await
    }

    fn write_invocation_started(
        &self,
        guard: &mut RunState,
        invocation_id: String,
        call_index: u64,
        kind: WorkflowInvocationKind,
        canonical_input: serde_json::Value,
        canonical_input_hash: String,
    ) -> Result<(), WorkflowError> {
        let writer = guard
            .journal
            .as_mut()
            .ok_or_else(|| WorkflowError::Journal("workflow journal is unavailable".to_owned()))?;
        let timestamp_ms = current_timestamp_ms();
        let started = JournalRecord::InvocationStarted {
            seq: writer.next_seq(),
            timestamp_ms,
            invocation_id,
            call_index,
            kind,
            canonical_input,
            canonical_input_hash,
        };
        let sequence = match writer.append(&started, &self.limits) {
            Ok(sequence) => sequence,
            Err(error) => {
                if matches!(error, WorkflowError::JournalTotalLimitExceeded) {
                    self.transition_locked(
                        guard,
                        WorkflowState::ResourceLimited,
                        "journal limit reached",
                        WorkflowActor::Runtime,
                    )?;
                    return Err(WorkflowError::ResourceLimited(
                        "journal limit reached".to_owned(),
                    ));
                }
                return Err(error);
            }
        };
        guard.projection_sequence = Some(sequence);
        guard.updated_at_ms = Some(timestamp_ms);
        self.emit_projection(guard, WorkflowProjectionStage::Updated);
        Ok(())
    }

    async fn finalize_invocation_locked(
        &self,
        state: Arc<Mutex<RunState>>,
        invocation_id: String,
        kind: WorkflowInvocationKind,
        outcome: WorkflowInvocationOutcome,
        capped: bool,
    ) -> Result<WorkflowInvocationOutcome, WorkflowError> {
        let mut guard = state.lock().await;
        let (outcome, resource_limit_reason) =
            match self.finish_invocation_locked(&mut guard, invocation_id, kind, outcome, capped) {
                Ok(finished) => finished,
                Err(error) => {
                    self.mark_recovery_failure_locked(
                        &mut guard,
                        &format!("workflow invocation finalization failed: {error}"),
                    );
                    return Err(error);
                }
            };

        let transition = if let Some(reason) = resource_limit_reason {
            self.transition_locked(
                &mut guard,
                WorkflowState::ResourceLimited,
                &reason,
                WorkflowActor::Runtime,
            )
        } else if guard.control.stop_token.is_cancelled() {
            let stop_actor = guard.control.stop_actor()?;
            self.transition_locked(
                &mut guard,
                WorkflowState::Cancelled,
                "stopped by user/model",
                stop_actor,
            )
        } else if outcome.interruption
            == Some(super::WorkflowInterruptionReason::InstructionReplanRequired)
        {
            self.transition_locked(
                &mut guard,
                WorkflowState::Paused,
                "instruction_replan_required",
                WorkflowActor::Runtime,
            )
        } else {
            Ok(())
        };
        if let Err(error) = transition {
            self.mark_recovery_failure_locked(
                &mut guard,
                &format!("workflow state finalization failed: {error}"),
            );
            return Err(error);
        }
        Ok(outcome)
    }

    fn finish_invocation_locked(
        &self,
        state: &mut RunState,
        invocation_id: String,
        kind: WorkflowInvocationKind,
        outcome: WorkflowInvocationOutcome,
        token_capped: bool,
    ) -> Result<(WorkflowInvocationOutcome, Option<String>), WorkflowError> {
        let timestamp_ms = current_timestamp_ms();
        let append_result = {
            let writer = state.journal.as_mut().ok_or_else(|| {
                WorkflowError::Journal("workflow journal is unavailable".to_owned())
            })?;
            writer.append(
                &JournalRecord::InvocationFinished {
                    seq: writer.next_seq(),
                    timestamp_ms,
                    invocation_id: invocation_id.clone(),
                    outcome: outcome.clone(),
                },
                &self.limits,
            )
        };

        let (sequence, outcome, resource_limit_reason) = match append_result {
            Ok(sequence) => {
                let reason = token_capped.then(|| "workflow actual token cap reached".to_owned());
                (sequence, outcome, reason)
            }
            Err(WorkflowError::JournalRecordLimitExceeded { .. }) => {
                let reason = "workflow invocation result exceeds journal record limit".to_owned();
                let outcome = compact_resource_limited_outcome(&reason, &outcome);
                let sequence = self.append_small_invocation_finish_locked(
                    state,
                    invocation_id,
                    timestamp_ms,
                    &outcome,
                )?;
                (sequence, outcome, Some(reason))
            }
            Err(WorkflowError::JournalTotalLimitExceeded) => {
                let reason = "workflow journal total limit reached".to_owned();
                let outcome = compact_resource_limited_outcome(&reason, &outcome);
                let sequence = self.append_small_invocation_finish_locked(
                    state,
                    invocation_id,
                    timestamp_ms,
                    &outcome,
                )?;
                (sequence, outcome, Some(reason))
            }
            Err(error) => return Err(error),
        };

        state.current_invocation = None;
        observe_outcome(state, kind, &outcome);
        state.projection_sequence = Some(sequence);
        state.updated_at_ms = Some(timestamp_ms);
        self.emit_projection(state, WorkflowProjectionStage::Updated);
        Ok((outcome, resource_limit_reason))
    }

    fn append_small_invocation_finish_locked(
        &self,
        state: &mut RunState,
        invocation_id: String,
        timestamp_ms: u64,
        outcome: &WorkflowInvocationOutcome,
    ) -> Result<u64, WorkflowError> {
        let writer = state
            .journal
            .as_mut()
            .ok_or_else(|| WorkflowError::Journal("workflow journal is unavailable".to_owned()))?;
        writer.append(
            &JournalRecord::InvocationFinished {
                seq: writer.next_seq(),
                timestamp_ms,
                invocation_id,
                outcome: outcome.clone(),
            },
            &self.limits,
        )
    }

    async fn finish_worker(
        &self,
        run_id: &WorkflowId,
        result: Result<(), WorkflowError>,
    ) -> Result<(), WorkflowError> {
        let state = self.run_state(run_id).await?;
        let mut guard = state.lock().await;
        guard.worker_active = false;
        if guard.state.is_terminal() || guard.state == WorkflowState::Paused {
            return Ok(());
        }
        let completion = if guard.control.stop_token.is_cancelled() {
            let stop_actor = guard.control.stop_actor()?;
            self.transition_locked(
                &mut guard,
                WorkflowState::Cancelled,
                "stopped by user/model",
                stop_actor,
            )
        } else if guard.control.pause_requested.load(Ordering::Acquire) {
            let pause_actor = guard.control.pause_actor()?;
            self.transition_locked(&mut guard, WorkflowState::Paused, "pause", pause_actor)
        } else {
            match result {
                Ok(()) => self.transition_locked(
                    &mut guard,
                    WorkflowState::Completed,
                    "worker completed",
                    WorkflowActor::Runtime,
                ),
                Err(WorkflowError::ResourceLimited(reason)) => self.transition_locked(
                    &mut guard,
                    WorkflowState::ResourceLimited,
                    &reason,
                    WorkflowActor::Runtime,
                ),
                Err(WorkflowError::Paused(reason)) => self.transition_locked(
                    &mut guard,
                    WorkflowState::Paused,
                    &reason,
                    WorkflowActor::Runtime,
                ),
                Err(error) => self.transition_locked(
                    &mut guard,
                    WorkflowState::Failed,
                    &error.to_string(),
                    WorkflowActor::Runtime,
                ),
            }
        };
        if let Err(error) = completion {
            self.mark_recovery_failure_locked(
                &mut guard,
                &format!("workflow worker finalization failed: {error}"),
            );
        }
        Ok(())
    }

    fn mark_recovery_failure_locked(&self, state: &mut RunState, reason: &str) {
        state.worker_active = false;
        state.current_invocation = None;
        state.state = WorkflowState::Failed;
        state.failure_count = state.failure_count.saturating_add(1);
        state.projection_sequence = None;
        state.updated_at_ms = Some(current_timestamp_ms());
        state.terminal_reason = Some(reason.to_owned());
        state.journal = None;
        self.emit_projection(state, WorkflowProjectionStage::Finished);
        if let Some(session_dir) = state.run_dir.parent().and_then(Path::parent) {
            let _ = self.notifications.enqueue(WorkflowNotification::new(
                session_dir,
                state.metadata.run_id.clone(),
                WorkflowState::Failed,
                reason,
            ));
        }
    }

    fn transition_locked(
        &self,
        state: &mut RunState,
        new_state: WorkflowState,
        reason: &str,
        actor: WorkflowActor,
    ) -> Result<(), WorkflowError> {
        let writer = state
            .journal
            .as_mut()
            .ok_or_else(|| WorkflowError::Journal("workflow journal is unavailable".to_owned()))?;
        if new_state.is_terminal() && writer.has_incomplete_invocations() {
            return Err(WorkflowError::InvalidInput(
                "cannot terminalize workflow with an incomplete invocation".to_owned(),
            ));
        }
        let previous = state.state;
        if previous == new_state {
            return Ok(());
        }
        let timestamp_ms = current_timestamp_ms();
        let sequence = writer.append(
            &JournalRecord::StateChanged {
                seq: writer.next_seq(),
                timestamp_ms,
                previous,
                new: new_state,
                reason: reason.to_owned(),
                actor,
            },
            &self.limits,
        )?;
        state.state = new_state;
        state.projection_sequence = Some(sequence);
        state.updated_at_ms = Some(timestamp_ms);
        if new_state.is_terminal() || new_state == WorkflowState::Paused {
            state.terminal_reason = Some(reason.to_owned());
        } else {
            state.terminal_reason = None;
        }
        self.emit_projection(
            state,
            if new_state.is_terminal() {
                WorkflowProjectionStage::Finished
            } else {
                WorkflowProjectionStage::Updated
            },
        );
        if new_state.is_terminal()
            && let Some(session_dir) = state.run_dir.parent().and_then(Path::parent)
        {
            let _ = self.notifications.enqueue(WorkflowNotification::new(
                session_dir,
                state.metadata.run_id.clone(),
                new_state,
                reason,
            ));
        }
        Ok(())
    }

    async fn run_state(&self, run_id: &WorkflowId) -> Result<Arc<Mutex<RunState>>, WorkflowError> {
        self.runs
            .lock()
            .await
            .get(&run_id.0)
            .cloned()
            .ok_or_else(|| WorkflowError::NotFound(run_id.0.clone()))
    }

    fn bound_runner(&self) -> Result<Option<Arc<Runner>>, WorkflowError> {
        self.runner
            .read()
            .map(|slot| slot.clone())
            .map_err(|_| WorkflowError::Host("workflow runner lock poisoned".to_owned()))
    }

    fn bound_recovery_resolver(&self) -> Result<Option<Arc<RecoveryResolver>>, WorkflowError> {
        self.recovery_resolver
            .read()
            .map(|slot| slot.clone())
            .map_err(|_| WorkflowError::Host("workflow recovery lock poisoned".to_owned()))
    }

    fn emit_projection(&self, state: &RunState, projection_stage: WorkflowProjectionStage) {
        let Ok(emitter) = self
            .projection_emitter
            .read()
            .map(|slot| slot.as_ref().map(Arc::clone))
        else {
            return;
        };
        let Some(emitter) = emitter else {
            return;
        };
        let Some(session_dir) = state.run_dir.parent().and_then(Path::parent) else {
            return;
        };
        emitter(session_dir, projection_stage, state.snapshot());
    }

    async fn insert_rehydrated_run(
        &self,
        run_dir: PathBuf,
        metadata: WorkflowRunMetadata,
        records: Vec<JournalRecord>,
        state: WorkflowState,
        terminal_reason: Option<String>,
        writer: JournalWriter,
    ) -> WorkflowHandle {
        let replay_entries = replay_entries(&records);
        let projection_sequence = records.last().map(JournalRecord::seq);
        let (started_at_ms, updated_at_ms) = projection_timestamps(&records);
        let control = Arc::new(RunControl::new());
        let run_id = metadata.run_id.clone();
        let run_state = RunState {
            current_phase: recovered_phase(&records),
            invocation_count: records
                .iter()
                .filter(|record| matches!(record, JournalRecord::InvocationStarted { .. }))
                .count()
                .try_into()
                .unwrap_or(u64::MAX),
            failure_count: records
                .iter()
                .filter(|record| matches!(record, JournalRecord::InvocationFinished { outcome, .. } if !outcome.ok))
                .count()
                .try_into()
                .unwrap_or(u64::MAX),
            actual_usage: aggregate_usage(&records),
            projection_sequence,
            started_at_ms,
            updated_at_ms,
            latest_log_summary: latest_log_summary(&replay_entries),
            latest_report_summary: latest_report_summary(&records),
            reports: recovered_reports(&records),
            metadata,
            state,
            terminal_reason,
            run_dir,
            control: Arc::clone(&control),
            worker_active: false,
            current_invocation: None,
            replay_entries,
            replay_cursor: 0,
            replay_live: false,
            journal: Some(writer),
        };
        self.runs
            .lock()
            .await
            .insert(run_id.0.clone(), Arc::new(Mutex::new(run_state)));
        WorkflowHandle {
            run_id,
            control,
            runtime: self.clone(),
        }
    }

    async fn insert_corrupt_run(
        &self,
        run_dir: PathBuf,
        run_id: WorkflowId,
        error: String,
    ) -> WorkflowHandle {
        let metadata = WorkflowRunMetadata {
            run_id,
            parent_run_id: None,
            name: "corrupt workflow".to_owned(),
            description: String::new(),
            phases: Vec::new(),
            script: String::new(),
            script_sha256: String::new(),
            args: serde_json::json!({}),
            launch_source: "rehydrate".to_owned(),
            journal_format_version: 1,
        };
        self.insert_failed_run(run_dir, metadata, format!("corrupt run metadata: {error}"))
            .await
    }

    async fn insert_failed_run(
        &self,
        run_dir: PathBuf,
        metadata: WorkflowRunMetadata,
        reason: String,
    ) -> WorkflowHandle {
        let control = Arc::new(RunControl::new());
        let run_id = metadata.run_id.clone();
        let state = RunState {
            metadata,
            state: WorkflowState::Failed,
            current_phase: None,
            invocation_count: 0,
            failure_count: 1,
            actual_usage: None,
            projection_sequence: None,
            started_at_ms: None,
            updated_at_ms: None,
            latest_log_summary: None,
            latest_report_summary: None,
            terminal_reason: Some(reason.clone()),
            reports: Vec::new(),
            run_dir,
            control: Arc::clone(&control),
            worker_active: false,
            current_invocation: None,
            replay_entries: Vec::new(),
            replay_cursor: 0,
            replay_live: false,
            journal: None,
        };
        self.runs
            .lock()
            .await
            .insert(run_id.0.clone(), Arc::new(Mutex::new(state)));
        WorkflowHandle {
            run_id,
            control,
            runtime: self.clone(),
        }
    }
}

#[derive(Clone)]
pub struct WorkflowHandle {
    pub run_id: WorkflowId,
    control: Arc<RunControl>,
    runtime: WorkflowRuntime,
}

impl WorkflowHandle {
    pub async fn snapshot(&self) -> WorkflowSnapshot {
        self.runtime
            .snapshot(&self.run_id)
            .await
            .expect("workflow handle refers to a registered run")
    }

    pub async fn output(&self) -> Result<WorkflowOutput, WorkflowError> {
        self.runtime.output(&self.run_id).await
    }

    pub async fn pause(&self, actor: WorkflowActor) -> Result<(), WorkflowError> {
        self.runtime.pause(&self.run_id, actor).await
    }

    pub async fn resume(&self, actor: WorkflowActor) -> Result<(), WorkflowError> {
        self.runtime.resume(&self.run_id, actor).await
    }

    pub async fn stop(&self, actor: WorkflowActor) -> Result<(), WorkflowError> {
        self.runtime.stop(&self.run_id, actor).await
    }

    pub async fn invoke<F, Fut>(
        &self,
        call_index: u64,
        kind: WorkflowInvocationKind,
        canonical_input: serde_json::Value,
        provider_backed: bool,
        effect: F,
    ) -> Result<WorkflowInvocationOutcome, WorkflowError>
    where
        F: FnOnce(WorkflowInvocationContext) -> Fut + Send,
        Fut: Future<Output = WorkflowInvocationOutcome> + Send,
    {
        self.runtime
            .invoke(
                &self.run_id,
                call_index,
                kind,
                canonical_input,
                provider_backed,
                effect,
            )
            .await
    }

    #[must_use]
    pub fn is_pause_requested(&self) -> bool {
        self.control.pause_requested.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn is_stop_requested(&self) -> bool {
        self.control.stop_token.is_cancelled()
    }

    #[must_use]
    pub fn stop_token(&self) -> &CancellationToken {
        &self.control.stop_token
    }
}

fn observe_outcome(
    state: &mut RunState,
    kind: WorkflowInvocationKind,
    outcome: &WorkflowInvocationOutcome,
) {
    if !outcome.ok {
        state.failure_count = state.failure_count.saturating_add(1);
    }
    if let Some(usage) = outcome.actual_usage {
        state.actual_usage = Some(add_usage(state.actual_usage, usage));
    }
    match kind {
        WorkflowInvocationKind::Log if outcome.ok => {
            state.latest_log_summary = outcome
                .details
                .get("message")
                .and_then(serde_json::Value::as_str)
                .map(bounded_summary);
        }
        WorkflowInvocationKind::Phase if outcome.ok => {
            state.current_phase = outcome
                .details
                .get("phase")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
        }
        WorkflowInvocationKind::Report if outcome.ok => {
            if let Some(report) = outcome.details.get("report") {
                state.latest_report_summary = report_summary(report);
                state.reports.push(report.clone());
            }
        }
        _ => {}
    }
}
