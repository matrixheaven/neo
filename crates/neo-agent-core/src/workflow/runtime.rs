use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex as StdMutex, RwLock};

use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::admission::{AdmitOutcome, WorkerPermit, WorkflowAdmission};
use super::error::WorkflowError;
use super::journal::{
    self, IncompleteInvocation, JournalEnvelope, JournalRecord, JournalV2Writer,
    canonical_input_hash,
};
use super::limits::WorkflowLimits;
use super::state::{
    WorkflowActor, WorkflowFinalResultMetadata, WorkflowId, WorkflowInvocationKind,
    WorkflowInvocationOutcome, WorkflowOutcomeStatus, WorkflowPhase, WorkflowRunMetadata,
    WorkflowSnapshot, WorkflowState,
};
use crate::AgentTokenUsage;
use crate::runtime::{WorkflowNotification, WorkflowNotificationQueue};

#[path = "effect.rs"]
mod effect;
#[path = "runtime_support.rs"]
mod support;
use support::{
    ReplayEntry, RunControl, add_usage, aggregate_usage, aggregate_usage_v2, bounded_summary,
    compact_resource_limited_outcome, current_timestamp_ms, failure_count_v2, final_result_v2,
    invocation_count_v2, last_state, latest_log_summary, latest_report_summary,
    latest_report_summary_v2, projection_timestamps, projection_timestamps_v2, recovered_phase,
    recovered_phase_v2, recovered_reports, recovered_reports_v2, replay_entries, replay_entries_v2,
    report_summary,
};
pub use support::{ReplayPrefix, compute_replay_prefix};

type RunnerFuture = Pin<Box<dyn Future<Output = Result<(), WorkflowError>> + Send>>;
type Runner = dyn Fn(WorkflowHandle, WorkflowRunMetadata, PathBuf) -> RunnerFuture + Send + Sync;
type RecoveryFuture = Pin<Box<dyn Future<Output = Option<WorkflowInvocationOutcome>> + Send>>;
type RecoveryResolver = dyn Fn(Arc<IncompleteInvocation>) -> RecoveryFuture + Send + Sync;
type ProjectionEmitter = dyn Fn(&Path, WorkflowProjectionStage, WorkflowSnapshot) + Send + Sync;
type SharedJournal = Arc<StdMutex<JournalV2Writer>>;

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
        journal_format_version: journal::JOURNAL_FORMAT_V2,
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkflowOutput {
    pub metadata: WorkflowRunMetadata,
    pub state: WorkflowState,
    pub current_phase: Option<String>,
    /// V1 journal projection only; V2 runs leave this empty (use journal scan APIs).
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
    /// Supervisor task that awaits the runner `JoinHandle` and terminalizes panics.
    worker_join: Option<JoinHandle<()>>,
    /// Active worker+VM admission permit; released on every exit path.
    worker_permit: Option<WorkerPermit>,
    current_invocation: Option<String>,
    replay_entries: Vec<ReplayEntry>,
    replay_cursor: usize,
    replay_live: bool,
    /// V2 journal writer. Taken out of this field for the duration of blocking
    /// journal I/O so the async run mutex never crosses file sync.
    journal: Option<SharedJournal>,
    final_result: Option<WorkflowFinalResultMetadata>,
    /// V1 durable artifacts are inspectable projections only; no append path.
    v1_read_only: bool,
}

impl RunState {
    fn snapshot(&self) -> WorkflowSnapshot {
        WorkflowSnapshot {
            id: self.metadata.run_id.clone(),
            title: self.metadata.name.clone(),
            state: self.state,
            current_phase: self.current_phase.clone(),
            projection_sequence: self.projection_sequence,
            recovery_failure: self.journal.is_none() && !self.v1_read_only,
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
    admission: WorkflowAdmission,
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
        let admission = WorkflowAdmission::new(limits.clone());
        Self {
            runs: Arc::new(Mutex::new(HashMap::new())),
            limits,
            admission,
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

    /// Host-owned global admission controller (permits, not lifecycle).
    #[must_use]
    pub fn admission(&self) -> &WorkflowAdmission {
        &self.admission
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

    /// Create a V2 run: durable `run.json` + `RunCreated` + `Queued` before any
    /// task registration or worker start. Failure rolls back only a never-started
    /// directory (nothing is registered in the runtime map until durability).
    pub async fn create_run(
        &self,
        session_dir: &Path,
        request: WorkflowLaunchRequest,
    ) -> Result<WorkflowHandle, WorkflowError> {
        self.validate_launch_request(&request)?;

        let (run_id, run_dir) = loop {
            let run_id = WorkflowId::generate();
            let run_dir = journal::run_dir(session_dir, &run_id);
            if !run_dir.exists() {
                break (run_id, run_dir);
            }
        };
        let metadata = metadata_for_request(run_id.clone(), request);

        let storage_reservation = self
            .admission
            .try_reserve_storage(run_id.as_str(), self.limits.run_storage_reservation_bytes())?;
        // Commit so create holds storage for the durable run lifetime.
        storage_reservation.commit();

        let durable_create = (|| {
            journal::write_run_metadata(&run_dir, &metadata, &self.limits)?;
            let mut writer = JournalV2Writer::open(&run_dir.join("journal.jsonl"), run_id.clone())?;
            let timestamp_ms = current_timestamp_ms();
            let created = effect::prepare_run_created(
                &writer,
                run_id.clone(),
                metadata.name.clone(),
                Some(metadata.description.clone()).filter(|s| !s.is_empty()),
                Some(metadata.launch_source.clone()),
                timestamp_ms,
            );
            let sequence = writer.append(&created, &self.limits)?;
            Ok::<_, WorkflowError>((writer, sequence, timestamp_ms))
        })();
        let (writer, projection_sequence, started_at_ms) = match durable_create {
            Ok(durable) => durable,
            Err(error) => {
                self.admission.release_storage_owner(run_id.as_str());
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
            state: WorkflowState::Queued,
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
            worker_join: None,
            worker_permit: None,
            current_invocation: None,
            replay_entries: Vec::new(),
            replay_cursor: 0,
            replay_live: false,
            journal: Some(Arc::new(StdMutex::new(writer))),
            final_result: None,
            v1_read_only: false,
        }));
        self.runs
            .lock()
            .await
            .insert(run_id.0.clone(), Arc::clone(&state));

        Ok(WorkflowHandle {
            run_id,
            control,
            runtime: self.clone(),
        })
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
            if guard.worker_active || guard.state != WorkflowState::Queued {
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
        self.admission.release_storage_owner(run_id.as_str());
        self.admission.dequeue_worker(run_id);
        self.admission.release_run_occupancy(run_id);
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
        {
            let guard = state.lock().await;
            if guard.state != WorkflowState::Queued {
                return Err(WorkflowError::InvalidInput(
                    "worker can only start for a queued workflow".to_owned(),
                ));
            }
            if guard.worker_active {
                return Err(WorkflowError::InvalidInput(
                    "workflow worker is already active".to_owned(),
                ));
            }
        }

        // Fair FIFO occupancy: unavailable permits leave the run durably queued.
        let permit = match self.admission.try_admit_worker(run_id) {
            AdmitOutcome::Granted(permit) => permit,
            AdmitOutcome::Queued { .. } => {
                return Ok(());
            }
        };

        if let Err(error) = self
            .transition(
                &state,
                WorkflowState::Running,
                "worker_start",
                WorkflowActor::Runtime,
            )
            .await
        {
            drop(permit);
            return Err(error);
        }

        let (handle, metadata, session_dir) = {
            let mut guard = state.lock().await;
            if guard.state != WorkflowState::Running {
                drop(permit);
                return Err(WorkflowError::InvalidInput(
                    "worker start lost running state".to_owned(),
                ));
            }
            guard.worker_active = true;
            guard.worker_permit = Some(permit);
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
        let runner_task: JoinHandle<Result<(), WorkflowError>> =
            tokio::spawn(async move { runner(handle, metadata, session_dir).await });
        let supervisor = tokio::spawn(async move {
            match runner_task.await {
                Ok(result) => {
                    let _ = runtime.finish_worker(&id, result).await;
                }
                Err(join_error) if join_error.is_panic() => {
                    let _ = runtime.finish_worker_panicked(&id).await;
                }
                Err(_) => {
                    let _ = runtime
                        .finish_worker(
                            &id,
                            Err(WorkflowError::Host(
                                "workflow worker task cancelled".to_owned(),
                            )),
                        )
                        .await;
                }
            }
        });
        {
            let mut guard = state.lock().await;
            guard.worker_join = Some(supervisor);
        }
        Ok(())
    }

    pub async fn snapshot(&self, run_id: &WorkflowId) -> Result<WorkflowSnapshot, WorkflowError> {
        Ok(self.run_state(run_id).await?.lock().await.snapshot())
    }

    pub async fn output(&self, run_id: &WorkflowId) -> Result<WorkflowOutput, WorkflowError> {
        let state = self.run_state(run_id).await?;
        let guard = state.lock().await;
        let invocations = if guard.metadata.journal_format_version < journal::JOURNAL_FORMAT_V2
            && (guard.v1_read_only || guard.journal.is_some())
        {
            journal::read_journal(&guard.journal_path()).unwrap_or_default()
        } else {
            Vec::new()
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
        {
            let guard = state.lock().await;
            if guard.state.is_terminal() {
                return Err(WorkflowError::InvalidInput(
                    "cannot pause a terminal workflow".to_owned(),
                ));
            }
            if guard.state != WorkflowState::Paused {
                guard.state.require_transition_to(WorkflowState::Paused)?;
            }
            guard.control.request_pause(actor)?;
            if guard.worker_active {
                return Ok(());
            }
        }
        let pause_actor = {
            let guard = state.lock().await;
            guard.control.pause_actor()?
        };
        self.transition(&state, WorkflowState::Paused, "pause", pause_actor)
            .await
    }

    pub async fn resume(
        &self,
        run_id: &WorkflowId,
        actor: WorkflowActor,
    ) -> Result<(), WorkflowError> {
        let state = self.run_state(run_id).await?;
        {
            let guard = state.lock().await;
            if guard.v1_read_only {
                return Err(WorkflowError::InvalidOperation(
                    "linked_upgrade_required".to_owned(),
                ));
            }
            // Ordinary resume cannot bypass durable AwaitingUser.
            if guard.state == WorkflowState::AwaitingUser {
                return Err(WorkflowError::coded(
                    super::error::WorkflowErrorCode::AwaitingUser,
                    "ordinary resume cannot bypass awaiting_user; answer the request first",
                ));
            }
            if !guard.state.allows_ordinary_resume() {
                return Err(WorkflowError::InvalidInput(
                    "can only resume a paused workflow".to_owned(),
                ));
            }
        }
        if self.bound_runner()?.is_none() {
            return Err(WorkflowError::InvalidInput(
                "workflow runner is not bound".to_owned(),
            ));
        }
        {
            let mut guard = state.lock().await;
            guard.control.clear_pause()?;
            if let Ok(envelopes) =
                journal::collect_journal_v2(&guard.journal_path(), Some(&guard.metadata.run_id))
            {
                guard.replay_entries = replay_entries_v2(&envelopes);
            }
            guard.replay_cursor = 0;
            guard.replay_live = false;
        }
        // Design: paused -> queued; start_worker then queued -> running.
        self.transition(&state, WorkflowState::Queued, "resume", actor)
            .await?;
        self.start_worker(run_id).await
    }

    pub async fn stop(
        &self,
        run_id: &WorkflowId,
        actor: WorkflowActor,
    ) -> Result<(), WorkflowError> {
        let state = self.run_state(run_id).await?;
        let (worker_active, has_invocation, stop_now) = {
            let guard = state.lock().await;
            if guard.state.is_terminal() {
                return Err(WorkflowError::InvalidInput(
                    "cannot stop a terminal workflow".to_owned(),
                ));
            }
            // request_stop needs &self on control; reborrow as mut via interior mutability
            drop(guard);
            let guard = state.lock().await;
            guard.control.request_stop(actor)?;
            let stop_now = !guard.worker_active && guard.current_invocation.is_none();
            (
                guard.worker_active,
                guard.current_invocation.is_some(),
                stop_now,
            )
        };
        if stop_now && !worker_active && !has_invocation {
            let stop_actor = {
                let guard = state.lock().await;
                guard.control.stop_actor()?
            };
            self.transition(
                &state,
                WorkflowState::Cancelled,
                "stopped by user/model",
                stop_actor,
            )
            .await?;
        }
        Ok(())
    }

    /// Persist the canonical final result before a `Completed` transition.
    pub async fn record_final_result(
        &self,
        run_id: &WorkflowId,
        metadata: WorkflowFinalResultMetadata,
    ) -> Result<(), WorkflowError> {
        let state = self.run_state(run_id).await?;
        let (journal, run_id_owned) = {
            let guard = state.lock().await;
            if guard.state != WorkflowState::Running {
                return Err(WorkflowError::InvalidInput(
                    "final result requires running state".to_owned(),
                ));
            }
            if guard.final_result.is_some() {
                return Err(WorkflowError::InvalidOperation(
                    "final result already recorded".to_owned(),
                ));
            }
            (
                guard.journal.clone().ok_or_else(|| {
                    WorkflowError::Journal("workflow journal is unavailable".to_owned())
                })?,
                guard.metadata.run_id.clone(),
            )
        };
        let timestamp_ms = current_timestamp_ms();
        let prepared = {
            let writer = journal
                .lock()
                .map_err(|_| WorkflowError::Host("workflow journal lock poisoned".to_owned()))?;
            effect::prepare_final_result(&writer, run_id_owned, metadata, timestamp_ms)
        };
        let sequence = self.journal_io(&journal, |writer| {
            effect::commit_final_result(writer, &prepared, &self.limits)
        })?;
        let mut guard = state.lock().await;
        guard.final_result = Some(prepared.metadata);
        guard.projection_sequence = Some(sequence);
        guard.updated_at_ms = Some(timestamp_ms);
        self.emit_projection(&guard, WorkflowProjectionStage::Updated);
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
        // V1 durable artifacts rehydrate as inspectable read-only projections.
        if metadata.journal_format_version < journal::JOURNAL_FORMAT_V2 {
            self.rehydrate_v1_readonly(run_dir, metadata, handles)
                .await?;
            return Ok(());
        }

        let journal_path = run_dir.join("journal.jsonl");
        let recovery = match crate::workflow::recovery::recover_journal_v2(
            &journal_path,
            Some(&metadata.run_id),
        ) {
            Ok(report) => report,
            Err(error) => {
                handles.push(
                    self.insert_failed_run(
                        run_dir,
                        metadata,
                        format!("journal recovery failed: {error}"),
                    )
                    .await,
                );
                return Ok(());
            }
        };

        let mut writer = match JournalV2Writer::open_recovered(
            &journal_path,
            metadata.run_id.clone(),
            &recovery,
        ) {
            Ok(writer) => writer,
            Err(error) => {
                handles.push(
                    self.insert_failed_run(
                        run_dir,
                        metadata,
                        format!("journal open failed: {error}"),
                    )
                    .await,
                );
                return Ok(());
            }
        };

        // Crash after FinalResultRecorded / before Completed: append only the
        // missing terminal state. Never re-execute Lua or rewrite the result.
        if writer.index().final_result_seq.is_some() && writer.index().terminal_state.is_none() {
            let envelopes = match journal::collect_journal_v2(&journal_path, Some(&metadata.run_id))
            {
                Ok(envelopes) => envelopes,
                Err(error) => {
                    handles.push(
                        self.insert_failed_run(
                            run_dir,
                            metadata,
                            format!("corrupt journal: {error}"),
                        )
                        .await,
                    );
                    return Ok(());
                }
            };
            let (previous, _) = support::last_state_v2(&envelopes);
            let previous = if previous.is_terminal() {
                WorkflowState::Running
            } else {
                previous
            };
            // Completed is only legal from Running; if durable state drifted,
            // still close only when a final result is present and nonterminal.
            let target_previous = if previous.can_transition_to(WorkflowState::Completed) {
                previous
            } else {
                WorkflowState::Running
            };
            let timestamp_ms = current_timestamp_ms();
            match effect::prepare_transition(
                &writer,
                metadata.run_id.clone(),
                target_previous,
                WorkflowState::Completed,
                "recover_final_result",
                WorkflowActor::Runtime,
                timestamp_ms,
            ) {
                Ok(prepared) => {
                    if let Err(error) =
                        effect::commit_transition(&mut writer, &prepared, &self.limits)
                    {
                        handles.push(
                            self.insert_failed_run(
                                run_dir,
                                metadata,
                                format!("final-result recovery failed: {error}"),
                            )
                            .await,
                        );
                        return Ok(());
                    }
                }
                Err(error) => {
                    handles.push(
                        self.insert_failed_run(
                            run_dir,
                            metadata,
                            format!("final-result recovery transition rejected: {error}"),
                        )
                        .await,
                    );
                    return Ok(());
                }
            }
        }

        // Completed without a valid final result fails closed at scan time.
        if writer.index().terminal_state == Some(WorkflowState::Completed)
            && writer.index().final_result_seq.is_none()
        {
            handles.push(
                self.insert_failed_run(
                    run_dir,
                    metadata,
                    "completed state without final_result_recorded".to_owned(),
                )
                .await,
            );
            return Ok(());
        }

        let records_v2 = match journal::collect_journal_v2(&journal_path, Some(&metadata.run_id)) {
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
        let (final_state, terminal_reason) = v2_projection_state(&records_v2);
        handles.push(
            self.insert_rehydrated_v2(
                run_dir,
                metadata,
                records_v2,
                final_state,
                terminal_reason,
                Some(Arc::new(StdMutex::new(writer))),
            )
            .await,
        );
        Ok(())
    }

    /// Rehydrate a V1 run as a read-only projection (no journal mutation).
    async fn rehydrate_v1_readonly(
        &self,
        run_dir: PathBuf,
        metadata: WorkflowRunMetadata,
        handles: &mut Vec<WorkflowHandle>,
    ) -> Result<(), WorkflowError> {
        let journal_path = run_dir.join("journal.jsonl");
        let records = match journal::read_journal(&journal_path) {
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

        let (last_state, last_reason) = last_state(&records);
        let final_state = if last_state.rehydrates_as_paused_host_exit() {
            WorkflowState::Paused
        } else {
            last_state
        };
        let terminal_reason = if last_state.rehydrates_as_paused_host_exit() {
            Some("host_exit".to_owned())
        } else if final_state == WorkflowState::Paused || final_state.is_terminal() {
            last_reason
        } else {
            None
        };

        handles.push(
            self.insert_rehydrated_v1(run_dir, metadata, records, final_state, terminal_reason)
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
        _provider_backed: bool,
        effect_fn: F,
    ) -> Result<WorkflowInvocationOutcome, WorkflowError>
    where
        F: FnOnce(WorkflowInvocationContext) -> Fut + Send,
        Fut: Future<Output = WorkflowInvocationOutcome> + Send,
    {
        let state = self.run_state(run_id).await?;
        let input_hash = canonical_input_hash(&canonical_input);

        // --- prepare (async lock only; no I/O) ---
        let prepared = {
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

            let journal = guard.journal.clone().ok_or_else(|| {
                WorkflowError::Journal("workflow journal is unavailable".to_owned())
            })?;
            let run_id_owned = guard.metadata.run_id.clone();
            let control = Arc::clone(&guard.control);
            let invocation_id = format!("inv_{}", uuid::Uuid::new_v4().as_simple());
            let timestamp_ms = current_timestamp_ms();
            // Build envelope under journal std lock (no async lock held after this block ends).
            drop(guard);
            let prepared_start = {
                let writer = journal.lock().map_err(|_| {
                    WorkflowError::Host("workflow journal lock poisoned".to_owned())
                })?;
                effect::prepare_invocation_start(
                    &writer,
                    run_id_owned,
                    invocation_id,
                    call_index,
                    kind,
                    canonical_input,
                    timestamp_ms,
                )?
            };
            PreparedInvoke {
                journal,
                prepared_start,
                control,
                timestamp_ms,
            }
        };

        // --- reserve + durable InvocationStarted (no async run lock) ---
        let start_result = self.journal_io(&prepared.journal, |writer| {
            effect::commit_invocation_start(writer, &prepared.prepared_start, &self.limits)
        });
        let sequence = match start_result {
            Ok(sequence) => sequence,
            Err(WorkflowError::JournalTotalLimitExceeded) => {
                if let Err(error) = self
                    .transition(
                        &state,
                        WorkflowState::ResourceLimited,
                        "journal limit reached",
                        WorkflowActor::Runtime,
                    )
                    .await
                {
                    let mut guard = state.lock().await;
                    self.mark_recovery_failure_locked(
                        &mut guard,
                        &format!("journal limit reached; terminalization failed: {error}"),
                    );
                }
                return Err(WorkflowError::ResourceLimited(
                    "journal limit reached".to_owned(),
                ));
            }
            Err(error) => return Err(error),
        };

        // Apply start to memory (async lock; no I/O).
        {
            let mut guard = state.lock().await;
            guard.invocation_count = guard.invocation_count.saturating_add(1);
            guard.current_invocation = Some(prepared.prepared_start.invocation_id.clone());
            guard.projection_sequence = Some(sequence);
            guard.updated_at_ms = Some(prepared.timestamp_ms);
            self.emit_projection(&guard, WorkflowProjectionStage::Updated);
        }

        // --- external effect: no runtime locks ---
        let outcome = effect_fn(WorkflowInvocationContext {
            invocation_id: prepared.prepared_start.invocation_id.clone(),
            cancel_token: prepared.control.stop_token.clone(),
        })
        .await;

        self.finalize_invocation(
            state,
            prepared.journal,
            prepared.prepared_start.invocation_id,
            kind,
            outcome,
        )
        .await
    }

    async fn finalize_invocation(
        &self,
        state: Arc<Mutex<RunState>>,
        journal: SharedJournal,
        invocation_id: String,
        kind: WorkflowInvocationKind,
        outcome: WorkflowInvocationOutcome,
    ) -> Result<WorkflowInvocationOutcome, WorkflowError> {
        let run_id = {
            let guard = state.lock().await;
            guard.metadata.run_id.clone()
        };
        let timestamp_ms = current_timestamp_ms();

        let prepared_finish = {
            let writer = journal
                .lock()
                .map_err(|_| WorkflowError::Host("workflow journal lock poisoned".to_owned()))?;
            effect::prepare_invocation_finish(
                &writer,
                run_id.clone(),
                invocation_id.clone(),
                outcome.clone(),
                timestamp_ms,
            )
        };

        let append_result = self.journal_io(&journal, |writer| {
            effect::commit_invocation_finish(writer, &prepared_finish, &self.limits)
        });

        let (sequence, outcome, resource_limit_reason) = match append_result {
            Ok(sequence) => {
                let reason = None;
                (sequence, prepared_finish.outcome, reason)
            }
            Err(WorkflowError::JournalRecordLimitExceeded { .. }) => {
                let reason = "workflow invocation result exceeds journal record limit".to_owned();
                let compact = compact_resource_limited_outcome(&reason, &outcome);
                let prepared = {
                    let writer = journal.lock().map_err(|_| {
                        WorkflowError::Host("workflow journal lock poisoned".to_owned())
                    })?;
                    effect::prepare_invocation_finish(
                        &writer,
                        run_id.clone(),
                        invocation_id.clone(),
                        compact.clone(),
                        timestamp_ms,
                    )
                };
                let sequence = self.journal_io(&journal, |writer| {
                    effect::commit_invocation_finish(writer, &prepared, &self.limits)
                })?;
                (sequence, compact, Some(reason))
            }
            Err(WorkflowError::JournalTotalLimitExceeded) => {
                let reason = "workflow journal total limit reached".to_owned();
                let compact = compact_resource_limited_outcome(&reason, &outcome);
                let prepared = {
                    let writer = journal.lock().map_err(|_| {
                        WorkflowError::Host("workflow journal lock poisoned".to_owned())
                    })?;
                    effect::prepare_invocation_finish(
                        &writer,
                        run_id.clone(),
                        invocation_id.clone(),
                        compact.clone(),
                        timestamp_ms,
                    )
                };
                let sequence = self.journal_io(&journal, |writer| {
                    effect::commit_invocation_finish(writer, &prepared, &self.limits)
                })?;
                (sequence, compact, Some(reason))
            }
            Err(error) => {
                let mut guard = state.lock().await;
                self.mark_recovery_failure_locked(
                    &mut guard,
                    &format!("workflow invocation finalization failed: {error}"),
                );
                return Err(error);
            }
        };

        {
            let mut guard = state.lock().await;
            guard.current_invocation = None;
            observe_outcome(&mut guard, kind, &outcome);
            guard.projection_sequence = Some(sequence);
            guard.updated_at_ms = Some(timestamp_ms);
            self.emit_projection(&guard, WorkflowProjectionStage::Updated);
        }

        let transition = if let Some(reason) = resource_limit_reason.as_deref() {
            self.transition(
                &state,
                WorkflowState::ResourceLimited,
                reason,
                WorkflowActor::Runtime,
            )
            .await
        } else {
            let stop = {
                let guard = state.lock().await;
                guard.control.stop_token.is_cancelled()
            };
            if stop {
                let stop_actor = {
                    let guard = state.lock().await;
                    guard.control.stop_actor()?
                };
                self.transition(
                    &state,
                    WorkflowState::Cancelled,
                    "stopped by user/model",
                    stop_actor,
                )
                .await
            } else if outcome.interruption
                == Some(super::WorkflowInterruptionReason::InstructionReplanRequired)
            {
                self.transition(
                    &state,
                    WorkflowState::Paused,
                    "instruction_replan_required",
                    WorkflowActor::Runtime,
                )
                .await
            } else {
                Ok(())
            }
        };
        if let Err(error) = transition {
            let mut guard = state.lock().await;
            self.mark_recovery_failure_locked(
                &mut guard,
                &format!("workflow state finalization failed: {error}"),
            );
            return Err(error);
        }
        Ok(outcome)
    }

    async fn finish_worker(
        &self,
        run_id: &WorkflowId,
        result: Result<(), WorkflowError>,
    ) -> Result<(), WorkflowError> {
        let state = self.run_state(run_id).await?;
        {
            let mut guard = state.lock().await;
            guard.worker_active = false;
            guard.worker_join = None;
            self.release_worker_admission_locked(&mut guard);
            if guard.state.is_terminal() || guard.state == WorkflowState::Paused {
                return Ok(());
            }
        }

        let stop = {
            let guard = state.lock().await;
            guard.control.stop_token.is_cancelled()
        };
        let pause = {
            let guard = state.lock().await;
            guard.control.pause_requested.load(Ordering::Acquire)
        };

        let completion = if stop {
            let stop_actor = {
                let guard = state.lock().await;
                guard.control.stop_actor()?
            };
            self.transition(
                &state,
                WorkflowState::Cancelled,
                "stopped by user/model",
                stop_actor,
            )
            .await
        } else if pause {
            let pause_actor = {
                let guard = state.lock().await;
                guard.control.pause_actor()?
            };
            self.transition(&state, WorkflowState::Paused, "pause", pause_actor)
                .await
        } else {
            match result {
                Ok(()) => {
                    let has_final = {
                        let guard = state.lock().await;
                        guard.final_result.is_some()
                            || guard
                                .journal
                                .as_ref()
                                .and_then(|j| j.lock().ok())
                                .is_some_and(|w| w.index().final_result_seq.is_some())
                    };
                    if has_final {
                        self.transition(
                            &state,
                            WorkflowState::Completed,
                            "worker completed",
                            WorkflowActor::Runtime,
                        )
                        .await
                    } else {
                        self.transition(
                            &state,
                            WorkflowState::Failed,
                            "missing_final_result",
                            WorkflowActor::Runtime,
                        )
                        .await
                    }
                }
                Err(WorkflowError::ResourceLimited(reason)) => {
                    self.transition(
                        &state,
                        WorkflowState::ResourceLimited,
                        &reason,
                        WorkflowActor::Runtime,
                    )
                    .await
                }
                Err(WorkflowError::Paused(reason)) => {
                    self.transition(
                        &state,
                        WorkflowState::Paused,
                        &reason,
                        WorkflowActor::Runtime,
                    )
                    .await
                }
                Err(error) => {
                    self.transition(
                        &state,
                        WorkflowState::Failed,
                        &error.to_string(),
                        WorkflowActor::Runtime,
                    )
                    .await
                }
            }
        };
        if let Err(error) = completion {
            let mut guard = state.lock().await;
            self.mark_recovery_failure_locked(
                &mut guard,
                &format!("workflow worker finalization failed: {error}"),
            );
        }
        Ok(())
    }

    /// Terminalize a panicking worker: durable interrupted outcome first, then Failed.
    async fn finish_worker_panicked(&self, run_id: &WorkflowId) -> Result<(), WorkflowError> {
        let state = self.run_state(run_id).await?;
        {
            let mut guard = state.lock().await;
            guard.worker_active = false;
            guard.worker_join = None;
            self.release_worker_admission_locked(&mut guard);
            if guard.state.is_terminal() || guard.state == WorkflowState::Paused {
                return Ok(());
            }
        }

        let current = {
            let guard = state.lock().await;
            guard.current_invocation.clone()
        };
        if let Some(invocation_id) = current {
            let journal = {
                let guard = state.lock().await;
                guard.journal.clone()
            };
            if let Some(journal) = journal {
                let run_id_owned = {
                    let guard = state.lock().await;
                    guard.metadata.run_id.clone()
                };
                let outcome = WorkflowInvocationOutcome {
                    ok: false,
                    status: WorkflowOutcomeStatus::Interrupted,
                    summary: "workflow worker panicked".to_owned(),
                    interruption: None,
                    details: serde_json::json!({"reason": "worker_panicked"}),
                    actual_usage: None,
                    child_refs: Vec::new(),
                };
                let timestamp_ms = current_timestamp_ms();
                let prepared = {
                    let writer = journal.lock().map_err(|_| {
                        WorkflowError::Host("workflow journal lock poisoned".to_owned())
                    })?;
                    effect::prepare_invocation_finish(
                        &writer,
                        run_id_owned,
                        invocation_id,
                        outcome,
                        timestamp_ms,
                    )
                };
                match self.journal_io(&journal, |writer| {
                    effect::commit_invocation_finish(writer, &prepared, &self.limits)
                }) {
                    Ok(sequence) => {
                        let mut guard = state.lock().await;
                        guard.current_invocation = None;
                        guard.failure_count = guard.failure_count.saturating_add(1);
                        guard.projection_sequence = Some(sequence);
                        guard.updated_at_ms = Some(timestamp_ms);
                        self.emit_projection(&guard, WorkflowProjectionStage::Updated);
                    }
                    Err(error) => {
                        let mut guard = state.lock().await;
                        self.mark_recovery_failure_locked(
                            &mut guard,
                            &format!(
                                "workflow worker panic invocation finalization failed: {error}"
                            ),
                        );
                        return Ok(());
                    }
                }
            } else {
                let mut guard = state.lock().await;
                self.mark_recovery_failure_locked(
                    &mut guard,
                    "workflow worker panicked with unavailable journal",
                );
                return Ok(());
            }
        }

        if let Err(error) = self
            .transition(
                &state,
                WorkflowState::Failed,
                "worker_panicked",
                WorkflowActor::Runtime,
            )
            .await
        {
            let mut guard = state.lock().await;
            self.mark_recovery_failure_locked(
                &mut guard,
                &format!("workflow worker panic finalization failed: {error}"),
            );
        }
        Ok(())
    }

    fn release_worker_admission_locked(&self, state: &mut RunState) {
        state.worker_permit = None;
        self.admission.release_run_occupancy(&state.metadata.run_id);
    }

    fn mark_recovery_failure_locked(&self, state: &mut RunState, reason: &str) {
        state.worker_active = false;
        state.worker_join = None;
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

    /// Table-validated durable transition. Journal I/O runs without the async
    /// run-state mutex; memory is applied only after a synced append.
    async fn transition(
        &self,
        state: &Arc<Mutex<RunState>>,
        new_state: WorkflowState,
        reason: &str,
        actor: WorkflowActor,
    ) -> Result<(), WorkflowError> {
        let (previous, journal, run_id) = {
            let guard = state.lock().await;
            if guard.state == new_state {
                return Ok(());
            }
            guard.state.require_transition_to(new_state)?;
            if new_state == WorkflowState::Completed && guard.final_result.is_none() {
                let has_final = guard
                    .journal
                    .as_ref()
                    .and_then(|j| j.lock().ok())
                    .is_some_and(|w| w.index().final_result_seq.is_some());
                if !has_final {
                    return Err(WorkflowError::InvalidOperation(
                        "completed requires final_result_recorded".to_owned(),
                    ));
                }
            }
            let incomplete = guard
                .journal
                .as_ref()
                .and_then(|j| j.lock().ok())
                .is_some_and(|w| w.index().has_incomplete_invocations());
            if new_state.is_terminal() && incomplete {
                return Err(WorkflowError::InvalidInput(
                    "cannot terminalize workflow with an incomplete invocation".to_owned(),
                ));
            }
            (
                guard.state,
                guard.journal.clone().ok_or_else(|| {
                    WorkflowError::Journal("workflow journal is unavailable".to_owned())
                })?,
                guard.metadata.run_id.clone(),
            )
        };

        let timestamp_ms = current_timestamp_ms();
        let prepared = {
            let writer = journal
                .lock()
                .map_err(|_| WorkflowError::Host("workflow journal lock poisoned".to_owned()))?;
            effect::prepare_transition(
                &writer,
                run_id,
                previous,
                new_state,
                reason,
                actor,
                timestamp_ms,
            )?
        };
        let sequence = self.journal_io(&journal, |writer| {
            effect::commit_transition(writer, &prepared, &self.limits)
        })?;

        {
            let mut guard = state.lock().await;
            // Journal is source of truth after a successful sync.
            guard.state = new_state;
            guard.projection_sequence = Some(sequence);
            guard.updated_at_ms = Some(timestamp_ms);
            if new_state.is_terminal() || new_state == WorkflowState::Paused {
                guard.terminal_reason = Some(reason.to_owned());
            } else {
                guard.terminal_reason = None;
            }
            self.emit_projection(
                &guard,
                if new_state.is_terminal() {
                    WorkflowProjectionStage::Finished
                } else {
                    WorkflowProjectionStage::Updated
                },
            );
            if new_state.is_terminal()
                || new_state == WorkflowState::Paused
                || new_state == WorkflowState::AwaitingUser
            {
                self.release_worker_admission_locked(&mut guard);
            }
            if new_state.is_terminal()
                && let Some(session_dir) = guard.run_dir.parent().and_then(Path::parent)
            {
                let _ = self.notifications.enqueue(WorkflowNotification::new(
                    session_dir,
                    guard.metadata.run_id.clone(),
                    new_state,
                    reason,
                ));
            }
        }
        Ok(())
    }

    /// Run blocking journal I/O without holding the async run-state mutex.
    fn journal_io<R>(
        &self,
        journal: &SharedJournal,
        f: impl FnOnce(&mut JournalV2Writer) -> Result<R, WorkflowError>,
    ) -> Result<R, WorkflowError> {
        let mut writer = journal
            .lock()
            .map_err(|_| WorkflowError::Host("workflow journal lock poisoned".to_owned()))?;
        f(&mut writer)
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

    #[allow(dead_code)] // rebound by Task 5 production recovery path for V2 incomplete effects
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

    async fn insert_rehydrated_v1(
        &self,
        run_dir: PathBuf,
        metadata: WorkflowRunMetadata,
        records: Vec<JournalRecord>,
        state: WorkflowState,
        terminal_reason: Option<String>,
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
                .filter(|record| {
                    matches!(
                        record,
                        JournalRecord::InvocationFinished { outcome, .. } if !outcome.ok
                    )
                })
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
            worker_join: None,
            worker_permit: None,
            current_invocation: None,
            replay_entries,
            replay_cursor: 0,
            replay_live: false,
            journal: None,
            final_result: None,
            v1_read_only: true,
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

    async fn insert_rehydrated_v2(
        &self,
        run_dir: PathBuf,
        metadata: WorkflowRunMetadata,
        envelopes: Vec<JournalEnvelope>,
        state: WorkflowState,
        terminal_reason: Option<String>,
        writer: Option<SharedJournal>,
    ) -> WorkflowHandle {
        let replay_entries = replay_entries_v2(&envelopes);
        let projection_sequence = envelopes.last().map(JournalEnvelope::seq);
        let (started_at_ms, updated_at_ms) = projection_timestamps_v2(&envelopes);
        let control = Arc::new(RunControl::new());
        let run_id = metadata.run_id.clone();
        let final_result = final_result_v2(&envelopes);
        let run_state = RunState {
            current_phase: recovered_phase_v2(&envelopes),
            invocation_count: invocation_count_v2(&envelopes),
            failure_count: failure_count_v2(&envelopes),
            actual_usage: aggregate_usage_v2(&envelopes),
            projection_sequence,
            started_at_ms,
            updated_at_ms,
            latest_log_summary: latest_log_summary(&replay_entries),
            latest_report_summary: latest_report_summary_v2(&envelopes),
            reports: recovered_reports_v2(&envelopes),
            metadata,
            state,
            terminal_reason,
            run_dir,
            control: Arc::clone(&control),
            worker_active: false,
            worker_join: None,
            worker_permit: None,
            current_invocation: None,
            replay_entries,
            replay_cursor: 0,
            replay_live: false,
            journal: writer,
            final_result,
            v1_read_only: false,
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
            journal_format_version: journal::JOURNAL_FORMAT_V1,
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
            worker_join: None,
            worker_permit: None,
            current_invocation: None,
            replay_entries: Vec::new(),
            replay_cursor: 0,
            replay_live: false,
            journal: None,
            final_result: None,
            v1_read_only: false,
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

struct PreparedInvoke {
    journal: SharedJournal,
    prepared_start: effect::PreparedInvocationStart,
    control: Arc<RunControl>,
    timestamp_ms: u64,
}

fn v2_projection_state(envelopes: &[JournalEnvelope]) -> (WorkflowState, Option<String>) {
    let (state, reason) = support::last_state_v2(envelopes);
    if state.rehydrates_as_paused_host_exit() {
        (WorkflowState::Paused, Some("host_exit".to_owned()))
    } else if state == WorkflowState::Paused || state.is_terminal() {
        (state, reason)
    } else {
        (state, None)
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

    pub async fn record_final_result(
        &self,
        metadata: WorkflowFinalResultMetadata,
    ) -> Result<(), WorkflowError> {
        self.runtime
            .record_final_result(&self.run_id, metadata)
            .await
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
