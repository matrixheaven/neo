use std::collections::HashMap;

use futures::StreamExt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex as StdMutex, RwLock};

use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::admission::{AdmitOutcome, WorkerPermit, WorkflowAdmission};
use super::artifacts::{ArtifactKind, ArtifactMetadata, ArtifactStore, ArtifactValue};
use super::error::{WorkflowError, WorkflowErrorCode};
use super::journal::{
    self, IncompleteInvocation, JournalEnvelope, JournalPayload, JournalWriter, WorkflowChildKey,
    WorkflowChildKind, canonical_input_hash, find_incomplete_invocations,
};
use super::limits::WorkflowLimits;
use super::output::{
    CanonicalFinalResult, PreparedFinalBody, TaskOutputMaterials, TaskOutputPage,
    TaskOutputRequest, prepare_final_body, reconstruct_canonical_final_result,
    render_task_output_page, validate_pending_user_input_projection,
};
use super::schema::{
    CompiledSchema, StructuredOutputSource, accept_structured_output, validate_final_lua_result,
};
use super::state::{
    WorkflowActor, WorkflowExecutionOrigin, WorkflowFinalResultMetadata, WorkflowId,
    WorkflowInvocationKind, WorkflowInvocationOutcome, WorkflowOutcomeStatus, WorkflowPhase,
    WorkflowRevision, WorkflowRunMetadata, WorkflowSnapshot, WorkflowSourceOrigin, WorkflowState,
};
use super::user_input::{AwaitUserInput, PendingUserInput, request_id_for_call_index};
use super::{RetentionOutcome, perform_retention};
use crate::AgentTokenUsage;
use crate::multi_agent::{
    AgentId, AgentRole, AgentRunMode, ChildPlan, ChildRunOutput, ChildRuntimeDeps,
    MultiAgentRuntime, child_final_assistant_text,
};
use crate::runtime::{WorkflowNotification, WorkflowNotificationQueue};

#[path = "effect.rs"]
mod effect;
#[path = "lineage.rs"]
pub mod lineage;
#[path = "runtime_support.rs"]
mod support;
pub use lineage::{
    ChildIsolationRequest, ParentChildAuthority, ResolvedChildContext, ResolvedChildIsolation,
    ResolvedWorktreeBinding, child_isolation_provenance, cleanup_isolated_worktree,
    host_bounded_context_summary, permission_rank, resolve_child_context, resolve_child_isolation,
    resolve_child_model, resolve_child_permission, resolve_child_tool_ceiling,
    resolve_child_worktree,
};
use support::{
    ReplayEntry, RunControl, add_usage, aggregate_usage, bounded_resource_limited_outcome,
    bounded_summary, current_timestamp_ms, failure_count, final_result, interrupted_outcome,
    invocation_count, latest_log_summary, latest_report_summary, projection_timestamps,
    recovered_phase, recovered_reports, replay_entries, report_summary,
};

type RunnerFuture = Pin<Box<dyn Future<Output = Result<(), WorkflowError>> + Send>>;
type Runner = dyn Fn(WorkflowHandle, WorkflowRunMetadata, PathBuf) -> RunnerFuture + Send + Sync;
type RecoveryFuture = Pin<Box<dyn Future<Output = Option<WorkflowInvocationOutcome>> + Send>>;
type RecoveryResolver = dyn Fn(Arc<IncompleteInvocation>) -> RecoveryFuture + Send + Sync;
type ProjectionEmitter = dyn Fn(&Path, WorkflowProjectionStage, WorkflowSnapshot) + Send + Sync;
type SharedJournal = Arc<StdMutex<JournalWriter>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowProjectionStage {
    Started,
    Updated,
    Finished,
}

/// Host acceptance of a child structured output with at most one tools-disabled repair.
#[derive(Debug, Clone)]
pub struct ChildSchemaAcceptResult {
    pub ok: bool,
    pub value: Option<serde_json::Value>,
    pub error_code: Option<WorkflowErrorCode>,
    pub summary: String,
    pub repair_attempted: bool,
    pub repair_id: Option<String>,
    pub first_raw: String,
    pub repair_raw: Option<String>,
    pub actual_usage: Option<AgentTokenUsage>,
}

/// Inputs for child structured-output validation plus one tools-disabled repair.
#[derive(Debug, Clone, Copy)]
pub struct ChildSchemaRepairRequest<'a> {
    pub invocation_id: &'a str,
    pub agent_id: &'a AgentId,
    pub schema: &'a CompiledSchema,
    pub first_output: &'a ChildRunOutput,
}

/// Heterogeneous swarm batch parameters (durable per-item journal records).
#[derive(Debug, Clone)]
pub struct SwarmBatchRequest {
    pub call_index: u64,
    pub canonical_input: serde_json::Value,
    pub description: String,
    pub role: AgentRole,
    pub max_concurrency: usize,
    pub plans: Vec<ChildPlan>,
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
    /// Pinned final `output_schema` JSON for the production Lua runner.
    ///
    /// Required for definition-backed launches; optional only for fixture/harness
    /// paths that attach schemas via [`LuaWorkflowRunner::with_final_schema`].
    pub output_schema: Option<serde_json::Value>,
    /// Optional user-facing display name for the Operator and completion delivery.
    pub display_name: Option<String>,
    /// Input schema JSON pinned at launch for Operator display and Save reconstruction.
    pub input_schema: Option<serde_json::Value>,
    /// Pinned definition origin so completion and Save can show the source kind.
    pub definition_origin: Option<WorkflowSourceOrigin>,
    /// Whether this is an inline (unsaved) run eligible for contextual Save.
    pub inline_unsaved: bool,
}

fn metadata_for_request(run_id: WorkflowId, request: WorkflowLaunchRequest) -> WorkflowRunMetadata {
    use sha2::{Digest, Sha256};

    let script_sha256 = format!("{:x}", Sha256::digest(request.script.as_bytes()));
    WorkflowRunMetadata {
        run_id,
        name: request.name,
        description: request.description,
        phases: request.phases,
        script: request.script,
        script_sha256,
        args: request.args,
        launch_source: request.launch_source,
        output_schema: request.output_schema,
        display_name: request.display_name,
        input_schema: request.input_schema,
        definition_origin: request.definition_origin,
        inline_unsaved: request.inline_unsaved,
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkflowOutput {
    pub metadata: WorkflowRunMetadata,
    pub state: WorkflowState,
    pub current_phase: Option<String>,
    pub failure_count: u64,
    pub actual_usage: Option<AgentTokenUsage>,
    pub terminal_reason: Option<String>,
    pub reports: Vec<serde_json::Value>,
    /// Canonical final result (inline or artifact-backed). Never synthesized from reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_result: Option<CanonicalFinalResult>,
    /// Bounded metadata for journal-committed artifacts only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactMetadata>,
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
    current_invocation_kind: Option<WorkflowInvocationKind>,
    replay_entries: Vec<ReplayEntry>,
    replay_cursor: usize,
    replay_live: bool,
    /// Journal writer. Taken out of this field for the duration of blocking
    /// journal I/O so the async run mutex never crosses file sync.
    journal: Option<SharedJournal>,
    final_result: Option<WorkflowFinalResultMetadata>,
    /// Current or most recently answered durable user-input request projection.
    pending_user_input: Option<PendingUserInput>,
    /// Run-scoped immutable artifact store (visibility requires journal commit).
    artifacts: ArtifactStore,
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
            display_name: self
                .metadata
                .display_name
                .clone()
                .unwrap_or_else(|| self.metadata.name.clone()),
            purpose: self.metadata.description.clone(),
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
    /// When set, TaskOutput I/O sleeps this long after releasing the run lock.
    /// Integration tests inject delay to prove I/O does not hold the run mutex.
    output_io_delay_ms: Arc<std::sync::atomic::AtomicU64>,
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
            output_io_delay_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
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

    /// Run automatic retention on the given sessions root.
    ///
    /// Deletes eligible terminal runs older than 30 days until global storage
    /// is at or below 80% of the configured limit. Returns the outcome
    /// (count and bytes reclaimed).
    pub fn try_auto_retention(&self, sessions_root: &Path) -> RetentionOutcome {
        perform_retention(sessions_root, Some(&self.admission), &self.limits)
    }

    /// Validate every pure launch boundary before durable creation.
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

    /// Bind the production recovery resolver once. Repeated composition calls
    /// are harmless; the resolver never dispatches or mutates child stores.
    pub fn bind_recovery_resolver_if_unbound<F, Fut>(
        &self,
        resolver: F,
    ) -> Result<(), WorkflowError>
    where
        F: Fn(Arc<IncompleteInvocation>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<WorkflowInvocationOutcome>> + Send + 'static,
    {
        let mut slot = self
            .recovery_resolver
            .write()
            .map_err(|_| WorkflowError::Host("workflow recovery lock poisoned".to_owned()))?;
        if slot.is_none() {
            *slot = Some(Arc::new(move |invocation| Box::pin(resolver(invocation))));
        }
        Ok(())
    }

    /// Create a run: durable `run.json` + `RunCreated` + `Queued` before any
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

        // Attempt storage reservation with one retry after auto-retention.
        let storage_reservation = match self
            .admission
            .try_reserve_storage(run_id.as_str(), self.limits.run_storage_reservation_bytes())
        {
            Ok(reservation) => reservation,
            Err(e) if e.code() == WorkflowErrorCode::StorageAdmissionDenied => {
                // Try auto-retention before final denial.
                if let Some(sessions_root) = session_dir.parent().and_then(Path::parent) {
                    let outcome =
                        perform_retention(sessions_root, Some(&self.admission), &self.limits);
                    if outcome.reclaimed_count > 0 {
                        tracing::info!(
                            "auto-retention before run create: reclaimed {} runs ({} bytes)",
                            outcome.reclaimed_count,
                            outcome.reclaimed_bytes
                        );
                    }
                }
                self.admission.try_reserve_storage(
                    run_id.as_str(),
                    self.limits.run_storage_reservation_bytes(),
                )?
            }
            Err(e) => return Err(e),
        };
        // Commit so create holds storage for the durable run lifetime.
        storage_reservation.commit();

        let durable_create = (|| {
            journal::write_run_metadata(&run_dir, &metadata, &self.limits)?;
            let mut writer =
                JournalWriter::open(&run_dir.join("journal.jsonl"), run_id.clone(), &self.limits)?;
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
        let artifacts = match ArtifactStore::open(&run_dir, run_id.clone()) {
            Ok(artifacts) => artifacts,
            Err(error) => {
                self.admission.release_storage_owner(run_id.as_str());
                let _ = std::fs::remove_dir_all(&run_dir);
                return Err(error);
            }
        };
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
            current_invocation_kind: None,
            replay_entries: Vec::new(),
            replay_cursor: 0,
            replay_live: false,
            journal: Some(Arc::new(StdMutex::new(writer))),
            final_result: None,
            pending_user_input: None,
            artifacts,
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
        std::fs::remove_dir_all(&run_dir)
            .map_err(|error| WorkflowError::Journal(error.to_string()))?;
        self.admission.release_storage_owner(run_id.as_str());
        self.admission.dequeue_worker(run_id);
        self.admission.release_run_occupancy(run_id);
        self.runs.lock().await.remove(&run_id.0);
        Ok(())
    }

    /// Persist a terminal failure if worker startup fails after durable
    /// creation. The registered task remains available through `TaskOutput`.
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
            let session_dir =
                if let Some(session_dir) = guard.run_dir.parent().and_then(Path::parent) {
                    session_dir.to_path_buf()
                } else {
                    guard.worker_active = false;
                    guard.current_invocation = None;
                    guard.current_invocation_kind = None;
                    self.release_worker_admission_locked(&mut guard);
                    return Err(WorkflowError::Host(
                        "workflow run directory has no session parent".to_owned(),
                    ));
                };
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

    pub async fn operator_snapshot(
        &self,
        run_id: &WorkflowId,
        task_id: &str,
    ) -> Result<super::WorkflowOperatorSnapshot, WorkflowError> {
        let state = self.run_state(run_id).await?;
        let guard = state.lock().await;
        let pending_user = guard.pending_user_input.clone().filter(|pending| {
            guard.state == WorkflowState::AwaitingUser && pending.answer.is_none()
        });
        let snapshot = guard.snapshot();
        let metadata = guard.metadata.clone();
        let journal_path = guard.journal_path();
        drop(guard);
        super::operator::project_snapshot(
            task_id,
            &snapshot,
            &metadata,
            pending_user,
            &journal_path,
            self.limits.journal_record_bytes,
            self.limits.journal_total_bytes,
        )
    }

    pub async fn operator_child_page(
        &self,
        run_id: &WorkflowId,
        request: &super::WorkflowOperatorRequest,
    ) -> Result<super::WorkflowChildPage, WorkflowError> {
        let state = self.run_state(run_id).await?;
        let guard = state.lock().await;
        let metadata = guard.metadata.clone();
        let journal_path = guard.journal_path();
        drop(guard);
        super::operator::project_child_page(
            run_id,
            &metadata,
            request,
            &journal_path,
            self.limits.journal_record_bytes,
            self.limits.journal_total_bytes,
        )
    }

    pub async fn output(&self, run_id: &WorkflowId) -> Result<WorkflowOutput, WorkflowError> {
        // Bounded projection only: never load or return a complete journal here.
        // Use `task_output` for paged journal/artifact views (design §35).
        let materials = self.task_output_materials(run_id).await?;
        Ok(WorkflowOutput {
            metadata: materials.metadata,
            state: materials.state,
            current_phase: materials.current_phase,
            failure_count: materials.failure_count,
            actual_usage: materials.actual_usage,
            terminal_reason: materials.terminal_reason,
            reports: materials.reports,
            final_result: materials.final_result,
            artifacts: materials.artifacts.list_metadata().to_vec(),
        })
    }

    /// Copy lock-free TaskOutput materials under the run mutex (no I/O).
    pub async fn task_output_materials(
        &self,
        run_id: &WorkflowId,
    ) -> Result<TaskOutputMaterials, WorkflowError> {
        let state = self.run_state(run_id).await?;
        let guard = state.lock().await;
        let final_result = match &guard.final_result {
            Some(meta) => {
                let artifact = meta
                    .artifact_id
                    .as_ref()
                    .and_then(|id| guard.artifacts.find_by_id(id).cloned());
                Some(reconstruct_canonical_final_result(
                    meta,
                    artifact.as_ref(),
                    guard.actual_usage,
                    Vec::new(),
                    guard.terminal_reason.clone(),
                )?)
            }
            None => None,
        };
        let admission_wait_reason = if guard.state == WorkflowState::Queued {
            Some("waiting_for_worker_permit".to_owned())
        } else {
            None
        };
        let journal = guard.journal.clone();
        let mut materials = TaskOutputMaterials {
            run_id: guard.metadata.run_id.clone(),
            journal_path: guard.journal_path(),
            journal_record_bytes: self.limits.journal_record_bytes,
            journal_total_bytes: self.limits.journal_total_bytes,
            metadata: guard.metadata.clone(),
            state: guard.state,
            current_phase: guard.current_phase.clone(),
            human_handle: None,
            invocation_count: guard.invocation_count,
            failure_count: guard.failure_count,
            actual_usage: guard.actual_usage,
            terminal_reason: guard.terminal_reason.clone(),
            latest_log_summary: guard.latest_log_summary.clone(),
            latest_report_summary: guard.latest_report_summary.clone(),
            reports: guard.reports.clone(),
            final_result,
            artifacts: guard.artifacts.clone(),
            pending_user: guard.pending_user_input.clone().filter(|pending| {
                guard.state == WorkflowState::AwaitingUser && pending.answer.is_none()
            }),
            started_child_count: 0,
            queued_child_count: 0,
            terminal_child_count: 0,
            admission_wait_reason,
        };
        drop(guard);

        if let Some(journal) = journal {
            let writer = journal
                .lock()
                .map_err(|_| WorkflowError::Host("workflow journal lock poisoned".to_owned()))?;
            let index = writer.index();
            materials.started_child_count =
                index.started_children.len().try_into().unwrap_or(u64::MAX);
            materials.terminal_child_count =
                index.finished_children.len().try_into().unwrap_or(u64::MAX);
            materials.queued_child_count = index
                .queued_children
                .iter()
                .filter(|key| {
                    !index.started_children.contains(*key)
                        && !index.finished_children.contains(*key)
                })
                .count()
                .try_into()
                .unwrap_or(u64::MAX);
        }
        Ok(materials)
    }

    /// Bounded TaskOutput page. Journal/artifact I/O runs outside the run lock.
    pub async fn task_output(
        &self,
        run_id: &WorkflowId,
        request: TaskOutputRequest,
    ) -> Result<TaskOutputPage, WorkflowError> {
        let materials = self.task_output_materials(run_id).await?;

        let delay_ms = self
            .output_io_delay_ms
            .load(std::sync::atomic::Ordering::SeqCst);

        let request_for_io = request.clone();
        let materials_for_io = materials.clone();
        let page = tokio::task::spawn_blocking(move || {
            if delay_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            }
            render_task_output_page(&materials_for_io, &request_for_io)
        })
        .await
        .map_err(|e| WorkflowError::Host(format!("TaskOutput worker join failed: {e}")))??;
        Ok(page)
    }

    /// Inject slow TaskOutput I/O after the run lock is released (test hook).
    #[doc(hidden)]
    pub fn set_output_io_delay_ms_for_test(&self, delay_ms: u64) {
        self.output_io_delay_ms
            .store(delay_ms, std::sync::atomic::Ordering::SeqCst);
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
            if let Ok(envelopes) = journal::collect_journal(
                &guard.journal_path(),
                Some(&guard.metadata.run_id),
                self.limits.journal_record_bytes,
                self.limits.journal_total_bytes,
            ) {
                guard.replay_entries = replay_entries(&envelopes);
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

    /// Durable `neo.await_user` host boundary (design §29).
    ///
    /// Compiles schema/default before any journal effect. On first live call:
    /// appends `UserInputRequested`, transitions `running -> awaiting_user`,
    /// releases worker/VM admission, and returns a coded `AwaitingUser` error so
    /// the worker exits without failing the run. On replay after an answer,
    /// returns the journaled JSON value.
    pub async fn await_user(
        &self,
        run_id: &WorkflowId,
        call_index: u64,
        input: AwaitUserInput,
    ) -> Result<serde_json::Value, WorkflowError> {
        let prepared = input.prepare()?;
        let request_id = request_id_for_call_index(call_index);

        // Replay path: answered request returns durable JSON without re-effect.
        if let Some(pending) = self.user_input_by_id(run_id, &request_id).await? {
            if let Some(answer) = pending.answer {
                return Ok(answer);
            }
            // Open request while somehow still running: stay fail-closed.
            return Err(WorkflowError::coded(
                WorkflowErrorCode::AwaitingUser,
                format!("user input {request_id} is still open"),
            ));
        }

        let state = self.run_state(run_id).await?;
        {
            let guard = state.lock().await;
            if guard.state != WorkflowState::Running {
                return Err(WorkflowError::InvalidInput(
                    "await_user requires running state".to_owned(),
                ));
            }
            if guard.control.stop_token.is_cancelled() {
                return Err(WorkflowError::Cancelled(
                    "workflow stop requested".to_owned(),
                ));
            }
        }

        let pending = PendingUserInput {
            request_id: request_id.clone(),
            prompt: prepared.prompt.clone(),
            answer_schema: prepared.answer_schema.clone(),
            default: prepared.default.clone(),
            title: prepared.title.clone(),
            answer_policy: prepared.answer_policy,
            answer: None,
        };
        validate_pending_user_input_projection(&pending, self.limits.task_output_page_bytes)?;

        // Append+sync UserInputRequested before state transition.
        let (journal, run_id_owned) = {
            let guard = state.lock().await;
            (
                guard.journal.clone().ok_or_else(|| {
                    WorkflowError::Journal("workflow journal is unavailable".to_owned())
                })?,
                guard.metadata.run_id.clone(),
            )
        };
        let timestamp_ms = current_timestamp_ms();
        let envelope = {
            let writer = journal
                .lock()
                .map_err(|_| WorkflowError::Host("workflow journal lock poisoned".to_owned()))?;
            // Reject duplicate open request ids.
            if writer.index().open_user_inputs.contains(&request_id)
                && !writer.index().answered_user_inputs.contains(&request_id)
            {
                return Err(WorkflowError::coded(
                    WorkflowErrorCode::StaleUserRequest,
                    format!("user input {request_id} is already open"),
                ));
            }
            JournalEnvelope::new(
                writer.next_seq(),
                timestamp_ms,
                run_id_owned,
                JournalPayload::UserInputRequested {
                    request_id: request_id.clone(),
                    prompt: prepared.prompt.clone(),
                    answer_schema: prepared.answer_schema.clone(),
                    default: prepared.default.clone(),
                    title: prepared.title.clone(),
                    answer_policy: prepared.answer_policy,
                },
            )
        };
        let sequence =
            self.journal_io(&journal, |writer| writer.append(&envelope, &self.limits))?;
        {
            let mut guard = state.lock().await;
            guard.projection_sequence = Some(sequence);
            guard.updated_at_ms = Some(timestamp_ms);
            guard.pending_user_input = Some(pending);
            self.emit_projection(&guard, WorkflowProjectionStage::Updated);
        }

        // Durable state before worker release.
        self.transition(
            &state,
            WorkflowState::AwaitingUser,
            "user_input",
            WorkflowActor::Runtime,
        )
        .await?;

        Err(WorkflowError::coded(
            WorkflowErrorCode::AwaitingUser,
            format!("awaiting user input {request_id}"),
        ))
    }

    /// Single runtime answer control path (design §29.3).
    ///
    /// Validates state, request id, answer policy, and schema, then appends
    /// `UserInputAnswered` and transitions `awaiting_user -> queued`. Stale,
    /// duplicate conflicting, wrong-run, wrong-schema, and human-only model
    /// answers are rejected without changing state.
    pub async fn answer(
        &self,
        run_id: &WorkflowId,
        request_id: &str,
        value: serde_json::Value,
        actor: WorkflowActor,
    ) -> Result<(), WorkflowError> {
        let state = self.run_state(run_id).await?;
        let pending = self
            .user_input_by_id(run_id, request_id)
            .await?
            .ok_or_else(|| {
                WorkflowError::coded(
                    WorkflowErrorCode::StaleUserRequest,
                    format!("unknown user input request {request_id}"),
                )
            })?;

        // Idempotent success only when the durable answer already matches.
        if let Some(existing) = &pending.answer {
            if existing == &value {
                return Ok(());
            }
            return Err(WorkflowError::coded(
                WorkflowErrorCode::StaleUserRequest,
                format!("user input {request_id} already answered with a different value"),
            ));
        }

        {
            let guard = state.lock().await;
            if guard.state != WorkflowState::AwaitingUser {
                return Err(WorkflowError::coded(
                    WorkflowErrorCode::InvalidOperation,
                    format!(
                        "answer requires awaiting_user state; current is {}",
                        guard.state.as_str()
                    ),
                ));
            }
        }

        // Validate policy + schema before any durable write.
        pending.validate_answer(&value, actor)?;

        let (journal, run_id_owned) = {
            let guard = state.lock().await;
            (
                guard.journal.clone().ok_or_else(|| {
                    WorkflowError::Journal("workflow journal is unavailable".to_owned())
                })?,
                guard.metadata.run_id.clone(),
            )
        };
        let timestamp_ms = current_timestamp_ms();
        let envelope = {
            let writer = journal
                .lock()
                .map_err(|_| WorkflowError::Host("workflow journal lock poisoned".to_owned()))?;
            if !writer.index().open_user_inputs.contains(request_id) {
                return Err(WorkflowError::coded(
                    WorkflowErrorCode::StaleUserRequest,
                    format!("user input {request_id} is not open"),
                ));
            }
            if writer.index().answered_user_inputs.contains(request_id) {
                return Err(WorkflowError::coded(
                    WorkflowErrorCode::StaleUserRequest,
                    format!("user input {request_id} already answered"),
                ));
            }
            JournalEnvelope::new(
                writer.next_seq(),
                timestamp_ms,
                run_id_owned,
                JournalPayload::UserInputAnswered {
                    request_id: request_id.to_owned(),
                    answer: Some(value.clone()),
                },
            )
        };
        let sequence =
            self.journal_io(&journal, |writer| writer.append(&envelope, &self.limits))?;
        {
            let mut guard = state.lock().await;
            guard.projection_sequence = Some(sequence);
            guard.updated_at_ms = Some(timestamp_ms);
            if let Some(pending) = guard
                .pending_user_input
                .as_mut()
                .filter(|pending| pending.request_id == request_id)
            {
                pending.answer = Some(value.clone());
            }
            // Reset replay so the next worker pass can return the journaled answer.
            if let Ok(envelopes) = journal::collect_journal(
                &guard.journal_path(),
                Some(&guard.metadata.run_id),
                self.limits.journal_record_bytes,
                self.limits.journal_total_bytes,
            ) {
                guard.replay_entries = replay_entries(&envelopes);
            }
            guard.replay_cursor = 0;
            guard.replay_live = false;
            self.emit_projection(&guard, WorkflowProjectionStage::Updated);
        }

        self.transition(&state, WorkflowState::Queued, "user_input_answered", actor)
            .await?;

        // Admission may promote to running when a runner is bound.
        if self.bound_runner()?.is_some() {
            let _ = self.start_worker(run_id).await;
        }
        Ok(())
    }

    /// Rehydrate the open (or historical) user-input request for a run.
    pub async fn pending_user_input(
        &self,
        run_id: &WorkflowId,
    ) -> Result<Option<PendingUserInput>, WorkflowError> {
        let state = self.run_state(run_id).await?;
        Ok(state.lock().await.pending_user_input.clone())
    }

    async fn user_input_by_id(
        &self,
        run_id: &WorkflowId,
        request_id: &str,
    ) -> Result<Option<PendingUserInput>, WorkflowError> {
        let state = self.run_state(run_id).await?;
        let journal_path = {
            let guard = state.lock().await;
            guard.journal_path()
        };
        let envelopes = journal::collect_journal(
            &journal_path,
            Some(run_id),
            self.limits.journal_record_bytes,
            self.limits.journal_total_bytes,
        )?;
        Ok(user_input_from_envelopes(&envelopes, request_id))
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

    /// Stage immutable artifact bytes, append `ArtifactCommitted` last, then expose metadata.
    ///
    /// Ordering: serialize → limits → temp/sync/hash/rename/dir-sync → journal append → mark visible.
    pub async fn commit_artifact(
        &self,
        run_id: &WorkflowId,
        logical_name: &str,
        kind: ArtifactKind,
        value: ArtifactValue,
        media_type: Option<&str>,
    ) -> Result<ArtifactMetadata, WorkflowError> {
        let state = self.run_state(run_id).await?;
        // Artifact I/O must not hold the async run mutex (design §23 / §35).
        let (journal, store) = {
            let guard = state.lock().await;
            if guard.state.is_terminal() {
                return Err(WorkflowError::InvalidInput(
                    "cannot commit artifact on terminal run".to_owned(),
                ));
            }
            let journal = guard.journal.clone().ok_or_else(|| {
                WorkflowError::Journal("workflow journal is unavailable".to_owned())
            })?;
            (journal, guard.artifacts.clone())
        };

        let staged = store.stage(&self.limits, logical_name, kind, &value, media_type)?;

        let timestamp_ms = current_timestamp_ms();
        let envelope = {
            let writer = journal
                .lock()
                .map_err(|_| WorkflowError::Host("workflow journal lock poisoned".to_owned()))?;
            JournalEnvelope::new(
                writer.next_seq(),
                timestamp_ms,
                run_id.clone(),
                JournalPayload::ArtifactCommitted {
                    artifact_id: staged.artifact_id.clone(),
                    sha256: staged.sha256.clone(),
                    byte_len: staged.byte_len,
                    media_type: Some(staged.media_type.clone()),
                    logical_name: Some(staged.logical_name.clone()),
                },
            )
        };
        let sequence =
            self.journal_io(&journal, |writer| writer.append(&envelope, &self.limits))?;

        let mut guard = state.lock().await;
        guard.artifacts.mark_committed(staged.metadata())?;
        let meta = guard
            .artifacts
            .find_by_id(&staged.artifact_id)
            .cloned()
            .ok_or_else(|| {
                WorkflowError::coded(
                    super::error::WorkflowErrorCode::ArtifactMissing,
                    "artifact missing after commit",
                )
            })?;
        guard.projection_sequence = Some(sequence);
        guard.updated_at_ms = Some(timestamp_ms);
        self.emit_projection(&guard, WorkflowProjectionStage::Updated);
        Ok(meta)
    }

    /// Persist exactly one top-level Lua return as the canonical final result.
    ///
    /// Oversized values are content-addressed artifacts; usage/terminal reason stay on output.
    pub async fn persist_canonical_final_result(
        &self,
        run_id: &WorkflowId,
        value: serde_json::Value,
        schema_revision: Option<WorkflowRevision>,
    ) -> Result<CanonicalFinalResult, WorkflowError> {
        let prepared = prepare_final_body(value, &self.limits)?;
        let metadata = match prepared {
            PreparedFinalBody::Inline(value) => WorkflowFinalResultMetadata {
                value: Some(value),
                artifact_id: None,
                schema_revision: schema_revision.clone(),
            },
            PreparedFinalBody::NeedsArtifact {
                logical_name,
                kind,
                value,
                media_type,
                ..
            } => {
                let artifact = self
                    .commit_artifact(run_id, &logical_name, kind, value, Some(&media_type))
                    .await?;
                WorkflowFinalResultMetadata {
                    value: None,
                    artifact_id: Some(artifact.artifact_id.clone()),
                    schema_revision: schema_revision.clone(),
                }
            }
        };
        self.record_final_result(run_id, metadata.clone()).await?;

        let state = self.run_state(run_id).await?;
        let guard = state.lock().await;
        let artifact = metadata
            .artifact_id
            .as_ref()
            .and_then(|id| guard.artifacts.find_by_id(id).cloned());
        reconstruct_canonical_final_result(
            &metadata,
            artifact.as_ref(),
            guard.actual_usage,
            Vec::new(),
            guard.terminal_reason.clone(),
        )
    }

    /// Validate (optional) final output schema then persist the Lua return.
    ///
    /// Schema failures are typed `schema_invalid` with message prefix
    /// `schema_invalid_final_result` and never trigger a model call or repair turn.
    pub async fn accept_final_lua_result(
        &self,
        run_id: &WorkflowId,
        value: serde_json::Value,
        schema: Option<&CompiledSchema>,
        schema_revision: Option<WorkflowRevision>,
    ) -> Result<CanonicalFinalResult, WorkflowError> {
        if let Some(schema) = schema {
            validate_final_lua_result(schema, &value).map_err(|err| {
                WorkflowError::coded(WorkflowErrorCode::SchemaInvalid, err.message)
            })?;
        }
        self.persist_canonical_final_result(run_id, value, schema_revision)
            .await
    }

    /// Append `SchemaRepairStarted` before a tools-disabled corrective model call.
    ///
    /// Returns the durable `repair_id`. Callers must not dispatch the repair model
    /// effect until this append has synced. A second start for the same
    /// `invocation_id` is rejected so crash recovery never repeats the model effect.
    pub async fn start_schema_repair(
        &self,
        run_id: &WorkflowId,
        invocation_id: &str,
    ) -> Result<String, WorkflowError> {
        if self
            .schema_repair_already_started(run_id, invocation_id)
            .await?
        {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InterruptedHostExit,
                format!(
                    "schema repair already started for invocation {invocation_id}; not repeating model effect"
                ),
            ));
        }
        let state = self.run_state(run_id).await?;
        let (journal, run_id_owned) = {
            let guard = state.lock().await;
            (
                guard.journal.clone().ok_or_else(|| {
                    WorkflowError::Journal("workflow journal is unavailable".to_owned())
                })?,
                guard.metadata.run_id.clone(),
            )
        };
        let repair_id = format!("repair_{}", uuid::Uuid::new_v4().as_simple());
        let timestamp_ms = current_timestamp_ms();
        let envelope = {
            let writer = journal
                .lock()
                .map_err(|_| WorkflowError::Host("workflow journal lock poisoned".to_owned()))?;
            JournalEnvelope::new(
                writer.next_seq(),
                timestamp_ms,
                run_id_owned,
                JournalPayload::SchemaRepairStarted {
                    repair_id: repair_id.clone(),
                    invocation_id: invocation_id.to_owned(),
                },
            )
        };
        let sequence =
            self.journal_io(&journal, |writer| writer.append(&envelope, &self.limits))?;
        let mut guard = state.lock().await;
        guard.projection_sequence = Some(sequence);
        guard.updated_at_ms = Some(timestamp_ms);
        self.emit_projection(&guard, WorkflowProjectionStage::Updated);
        Ok(repair_id)
    }

    /// Append `SchemaRepairFinished` after the single corrective attempt settles.
    pub async fn finish_schema_repair(
        &self,
        run_id: &WorkflowId,
        repair_id: &str,
        ok: bool,
        summary: impl Into<String>,
    ) -> Result<(), WorkflowError> {
        let summary = summary.into();
        let state = self.run_state(run_id).await?;
        let (journal, run_id_owned) = {
            let guard = state.lock().await;
            (
                guard.journal.clone().ok_or_else(|| {
                    WorkflowError::Journal("workflow journal is unavailable".to_owned())
                })?,
                guard.metadata.run_id.clone(),
            )
        };
        let timestamp_ms = current_timestamp_ms();
        let envelope = {
            let writer = journal
                .lock()
                .map_err(|_| WorkflowError::Host("workflow journal lock poisoned".to_owned()))?;
            if !writer.index().open_schema_repairs.contains(repair_id) {
                return Err(WorkflowError::coded(
                    WorkflowErrorCode::JournalCorrupt,
                    format!("schema_repair_finished without start for {repair_id}"),
                ));
            }
            if writer.index().finished_schema_repairs.contains(repair_id) {
                return Err(WorkflowError::coded(
                    WorkflowErrorCode::JournalCorrupt,
                    format!("duplicate schema_repair_finished for {repair_id}"),
                ));
            }
            JournalEnvelope::new(
                writer.next_seq(),
                timestamp_ms,
                run_id_owned,
                JournalPayload::SchemaRepairFinished {
                    repair_id: repair_id.to_owned(),
                    ok,
                    summary: summary.clone(),
                },
            )
        };
        let sequence =
            self.journal_io(&journal, |writer| writer.append(&envelope, &self.limits))?;
        let mut guard = state.lock().await;
        guard.projection_sequence = Some(sequence);
        guard.updated_at_ms = Some(timestamp_ms);
        self.emit_projection(&guard, WorkflowProjectionStage::Updated);
        Ok(())
    }

    /// Durable heterogeneous swarm batch: one outer Swarm invocation plus one
    /// generic child lifecycle for every item.
    ///
    /// Children are created solely through [`MultiAgentRuntime::prepare_swarm_batch`].
    /// Completed items (durable finished) are never replayed; pause blocks new starts;
    /// stop cancels active children through the multi-agent owner.
    pub async fn invoke_swarm_batch(
        &self,
        run_id: &WorkflowId,
        request: SwarmBatchRequest,
        multi_agent: MultiAgentRuntime,
        deps: ChildRuntimeDeps,
    ) -> Result<WorkflowInvocationOutcome, WorkflowError> {
        let multi_agent_for_effect = multi_agent;
        let deps_for_effect = deps;
        let max_concurrency_for_effect = request.max_concurrency.max(1);
        let runtime = self.clone();
        let run_id_for_effect = run_id.clone();
        let effect_request = SwarmBatchRequest {
            call_index: request.call_index,
            canonical_input: request.canonical_input.clone(),
            description: request.description,
            role: request.role,
            max_concurrency: max_concurrency_for_effect,
            plans: request.plans,
        };
        self.invoke(
            run_id,
            request.call_index,
            WorkflowInvocationKind::Swarm,
            request.canonical_input,
            true,
            move |invocation| {
                let runtime = runtime;
                let run_id = run_id_for_effect;
                let multi_agent = multi_agent_for_effect;
                let deps = deps_for_effect;
                let request = effect_request;
                async move {
                    match runtime
                        .run_swarm_batch_effect(
                            &run_id,
                            &invocation.invocation_id,
                            request,
                            multi_agent,
                            deps,
                        )
                        .await
                    {
                        Ok(outcome) => outcome,
                        Err(error) => WorkflowInvocationOutcome {
                            ok: false,
                            status: WorkflowOutcomeStatus::Failed,
                            summary: error.to_string(),
                            details: serde_json::json!({"error": error.to_string()}),
                            actual_usage: None,
                            child_refs: Vec::new(),
                            interruption: None,
                        },
                    }
                }
            },
        )
        .await
    }

    async fn run_swarm_batch_effect(
        &self,
        run_id: &WorkflowId,
        parent_invocation_id: &str,
        request: SwarmBatchRequest,
        multi_agent: MultiAgentRuntime,
        deps: ChildRuntimeDeps,
    ) -> Result<WorkflowInvocationOutcome, WorkflowError> {
        let SwarmBatchRequest {
            description,
            role,
            max_concurrency,
            plans,
            ..
        } = request;
        let swarm_id = multi_agent.new_swarm_id();
        let phase_id = self.current_phase(run_id).await?;
        // Queue all items durably before any dispatch.
        for plan in &plans {
            self.append_child_event(
                run_id,
                JournalPayload::ChildQueued {
                    child_key: WorkflowChildKey::SwarmItem {
                        swarm_id: swarm_id.clone(),
                        item_id: plan.item_id.clone(),
                    },
                    child_kind: WorkflowChildKind::SwarmItem,
                    invocation_id: parent_invocation_id.to_owned(),
                    phase_id: phase_id.clone(),
                    title: plan.title.clone().or_else(|| Some(plan.item_label.clone())),
                    role: plan.role.map(|role| role.as_str().to_owned()),
                },
            )
            .await?;
        }

        // Skip items that already finished in the journal (never replay completed).
        let finished = self.finished_swarm_item_ids(run_id, &swarm_id).await?;
        let active_plans: Vec<ChildPlan> = plans
            .iter()
            .filter(|plan| !finished.contains(&plan.item_id))
            .cloned()
            .collect();

        let snapshot = multi_agent
            .prepare_swarm_batch(
                &swarm_id,
                &description,
                role,
                AgentRunMode::Foreground,
                Some(max_concurrency),
                &active_plans,
            )
            .map_err(WorkflowError::InvalidInput)?;

        // Map item_id -> child snapshot for ordered results.
        let mut item_outcomes: Vec<(String, WorkflowInvocationOutcome)> = Vec::new();
        // Run with bounded concurrency, pausing before new starts when requested.
        let mut next = 0usize;
        let mut in_flight = futures::stream::FuturesUnordered::new();
        let children = snapshot.children.clone();

        loop {
            // Fill concurrency slots unless pause/stop requested.
            while in_flight.len() < max_concurrency && next < children.len() {
                if self.is_stop_requested(run_id).await {
                    let _ = multi_agent.cancel_swarm(&swarm_id);
                    break;
                }
                if self.is_pause_requested(run_id).await {
                    break;
                }
                let child = children[next].clone();
                let plan = active_plans.get(next).cloned();
                next += 1;
                let Some(plan) = plan else {
                    continue;
                };
                // Skip already-terminal children (completed siblings never re-run).
                if child.agent.state.is_terminal() {
                    let outcome = child_agent_to_outcome(&child.agent);
                    item_outcomes.push((plan.item_id.clone(), outcome));
                    continue;
                }
                let item_id = plan.item_id.clone();
                // Resolve context/ceiling/worktree before any child start or
                // Durable child start. Unsupported isolation
                // fails closed; tool_allow may only reduce; permission cannot escalate.
                let workspace = deps
                    .config
                    .workspace_root
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("."));
                let parent_authority = lineage::ParentChildAuthority {
                    permission_mode: deps.config.permission_mode,
                    model: deps.config.model.clone(),
                    model_aliases: std::collections::BTreeMap::new(),
                    provider_ids: {
                        let mut set = std::collections::HashSet::new();
                        set.insert(deps.config.model.provider.0.clone());
                        set
                    },
                    tools: deps.tools.for_workflow_child(None),
                    workspace_root: workspace.clone(),
                    parent_messages: Vec::new(),
                };
                // Prefer shared path without manager; isolated requires a configured manager later.
                // Until a session-scoped WorktreeManager is injected, isolated fails before start.
                let mut isolation_request = lineage::ChildIsolationRequest::from_child_plan(&plan);
                // Model/provider aliases require a bound catalog. Until the host
                // injects one, clear overrides so resolution does not invent
                // providers — worktree/permission/tool ceilings still apply.
                if parent_authority.model_aliases.is_empty() {
                    isolation_request.model = None;
                    isolation_request.provider = None;
                }
                let worktree_manager = None::<&crate::worktree::WorktreeManager>;
                match lineage::resolve_child_isolation(
                    &parent_authority,
                    &isolation_request,
                    worktree_manager,
                ) {
                    Ok(resolved) => {
                        // Apply resolved permission/model/workspace to child deps.
                        let mut deps = deps.clone().with_role(plan.role.unwrap_or(role));
                        deps.config = deps.config.with_permission_mode(resolved.permission_mode);
                        deps.config.model = resolved.model;
                        if let Ok(cfg) = deps
                            .config
                            .clone()
                            .with_workspace_root(resolved.worktree.workspace_root())
                        {
                            deps.config = cfg;
                        }
                        deps.config.instruction_inheritance =
                            resolved.context.instruction_inheritance;
                        deps.tools = std::sync::Arc::new(
                            deps.tools.for_workflow_child(plan.tool_allow.as_deref()),
                        );
                        let invocation_id =
                            format!("swarm_item_{}", uuid::Uuid::new_v4().as_simple());
                        self.append_child_event(
                            run_id,
                            JournalPayload::ChildStarted {
                                child_key: WorkflowChildKey::SwarmItem {
                                    swarm_id: swarm_id.clone(),
                                    item_id: item_id.clone(),
                                },
                                agent_id: Some(child.agent.id.as_str().to_owned()),
                            },
                        )
                        .await?;
                        let runtime = multi_agent.clone();
                        let swarm_id_run = swarm_id.clone();
                        let item_label = child.item.clone();
                        let child_context = resolved.context.mode;
                        let repair_deps = deps.clone();
                        in_flight.push(async move {
                            let output = runtime
                                .run_started_swarm_child_turn_with_schema(
                                    deps,
                                    child.agent,
                                    (&swarm_id_run, &item_label),
                                    child_context,
                                    plan.output_schema.as_ref(),
                                    |_| {},
                                )
                                .await;
                            (item_id, invocation_id, output, repair_deps)
                        });
                    }
                    Err(error) => {
                        // Fail before child start: durable item finish with isolation error.
                        let outcome = WorkflowInvocationOutcome {
                            ok: false,
                            status: WorkflowOutcomeStatus::Failed,
                            summary: error.to_string(),
                            interruption: None,
                            details: serde_json::json!({
                                "error": error.to_string(),
                                "error_code": error.code().as_str(),
                                "isolation_failed_before_start": true,
                            }),
                            actual_usage: None,
                            child_refs: Vec::new(),
                        };
                        self.append_child_event(
                            run_id,
                            JournalPayload::ChildStarted {
                                child_key: WorkflowChildKey::SwarmItem {
                                    swarm_id: swarm_id.clone(),
                                    item_id: item_id.clone(),
                                },
                                agent_id: Some(child.agent.id.as_str().to_owned()),
                            },
                        )
                        .await?;
                        self.append_child_event(
                            run_id,
                            JournalPayload::ChildFinished {
                                child_key: WorkflowChildKey::SwarmItem {
                                    swarm_id: swarm_id.clone(),
                                    item_id: item_id.clone(),
                                },
                                agent_id: Some(child.agent.id.as_str().to_owned()),
                                status: outcome.status,
                                summary: outcome.summary.clone(),
                                actual_usage: outcome.actual_usage,
                                error: (!outcome.ok).then(|| outcome.summary.clone()),
                            },
                        )
                        .await?;
                        item_outcomes.push((item_id, outcome));
                    }
                }
            }

            if in_flight.is_empty() {
                break;
            }

            if let Some((item_id, invocation_id, output, repair_deps)) = in_flight.next().await {
                let plan = plans
                    .iter()
                    .find(|plan| plan.item_id == item_id)
                    .expect("in-flight swarm item must have a plan");
                let outcome = self
                    .child_run_to_outcome_with_schema(
                        run_id,
                        &multi_agent,
                        repair_deps,
                        plan,
                        &invocation_id,
                        &output,
                    )
                    .await?;
                self.append_child_event(
                    run_id,
                    JournalPayload::ChildFinished {
                        child_key: WorkflowChildKey::SwarmItem {
                            swarm_id: swarm_id.clone(),
                            item_id: item_id.clone(),
                        },
                        agent_id: Some(output.snapshot.id.as_str().to_owned()),
                        status: outcome.status,
                        summary: outcome.summary.clone(),
                        actual_usage: outcome.actual_usage,
                        error: (!outcome.ok).then(|| outcome.summary.clone()),
                    },
                )
                .await?;
                item_outcomes.push((item_id, outcome));
            }

            if self.is_stop_requested(run_id).await {
                let _ = multi_agent.cancel_swarm(&swarm_id);
                // Drain in-flight to terminal without starting new ones.
                while let Some((item_id, invocation_id, output, repair_deps)) =
                    in_flight.next().await
                {
                    let plan = plans
                        .iter()
                        .find(|plan| plan.item_id == item_id)
                        .expect("in-flight swarm item must have a plan");
                    let outcome = self
                        .child_run_to_outcome_with_schema(
                            run_id,
                            &multi_agent,
                            repair_deps,
                            plan,
                            &invocation_id,
                            &output,
                        )
                        .await
                        .unwrap_or_else(|error| WorkflowInvocationOutcome {
                            ok: false,
                            status: WorkflowOutcomeStatus::Failed,
                            summary: error.to_string(),
                            details: serde_json::json!({"error": error.to_string()}),
                            actual_usage: None,
                            child_refs: Vec::new(),
                            interruption: None,
                        });
                    let _ = self
                        .append_child_event(
                            run_id,
                            JournalPayload::ChildFinished {
                                child_key: WorkflowChildKey::SwarmItem {
                                    swarm_id: swarm_id.clone(),
                                    item_id: item_id.clone(),
                                },
                                agent_id: Some(output.snapshot.id.as_str().to_owned()),
                                status: outcome.status,
                                summary: outcome.summary.clone(),
                                actual_usage: outcome.actual_usage,
                                error: (!outcome.ok).then(|| outcome.summary.clone()),
                            },
                        )
                        .await;
                    item_outcomes.push((item_id, outcome));
                }
                break;
            }
            if self.is_pause_requested(run_id).await && next < children.len() {
                // Let active finish, then stop starting new ones.
                while let Some((item_id, invocation_id, output, repair_deps)) =
                    in_flight.next().await
                {
                    let plan = plans
                        .iter()
                        .find(|plan| plan.item_id == item_id)
                        .expect("in-flight swarm item must have a plan");
                    let outcome = self
                        .child_run_to_outcome_with_schema(
                            run_id,
                            &multi_agent,
                            repair_deps,
                            plan,
                            &invocation_id,
                            &output,
                        )
                        .await
                        .unwrap_or_else(|error| WorkflowInvocationOutcome {
                            ok: false,
                            status: WorkflowOutcomeStatus::Failed,
                            summary: error.to_string(),
                            details: serde_json::json!({"error": error.to_string()}),
                            actual_usage: None,
                            child_refs: Vec::new(),
                            interruption: None,
                        });
                    let _ = self
                        .append_child_event(
                            run_id,
                            JournalPayload::ChildFinished {
                                child_key: WorkflowChildKey::SwarmItem {
                                    swarm_id: swarm_id.clone(),
                                    item_id: item_id.clone(),
                                },
                                agent_id: Some(output.snapshot.id.as_str().to_owned()),
                                status: outcome.status,
                                summary: outcome.summary.clone(),
                                actual_usage: outcome.actual_usage,
                                error: (!outcome.ok).then(|| outcome.summary.clone()),
                            },
                        )
                        .await;
                    item_outcomes.push((item_id, outcome));
                }
                break;
            }
        }

        let stopped = self.is_stop_requested(run_id).await;
        let paused = self.is_pause_requested(run_id).await;
        if stopped || paused {
            for plan in &plans {
                if finished.contains(&plan.item_id)
                    || item_outcomes
                        .iter()
                        .any(|(item_id, _)| item_id == &plan.item_id)
                {
                    continue;
                }
                let status = if stopped {
                    WorkflowOutcomeStatus::Cancelled
                } else {
                    WorkflowOutcomeStatus::Interrupted
                };
                let summary = if stopped {
                    "cancelled before start"
                } else {
                    "paused before start"
                };
                let outcome = WorkflowInvocationOutcome {
                    ok: false,
                    status,
                    summary: summary.to_owned(),
                    details: serde_json::json!({"not_started": true}),
                    actual_usage: None,
                    child_refs: Vec::new(),
                    interruption: None,
                };
                let agent_id = active_plans
                    .iter()
                    .position(|candidate| candidate.item_id == plan.item_id)
                    .and_then(|index| children.get(index))
                    .map(|child| child.agent.id.as_str().to_owned());
                self.append_child_event(
                    run_id,
                    JournalPayload::ChildFinished {
                        child_key: WorkflowChildKey::SwarmItem {
                            swarm_id: swarm_id.clone(),
                            item_id: plan.item_id.clone(),
                        },
                        agent_id,
                        status,
                        summary: summary.to_owned(),
                        actual_usage: None,
                        error: Some(summary.to_owned()),
                    },
                )
                .await?;
                item_outcomes.push((plan.item_id.clone(), outcome));
            }
        }

        // Preserve input order in the aggregate result.
        let mut ordered = Vec::with_capacity(plans.len());
        for plan in &plans {
            if let Some((_, outcome)) = item_outcomes.iter().find(|(id, _)| id == &plan.item_id) {
                let mut item = serde_json::json!({
                    "item_id": plan.item_id,
                    "ok": outcome.ok,
                    "status": outcome.status,
                    "summary": outcome.summary,
                });
                if let Some(structured_output) = outcome.details.get("structured_output") {
                    item["structured_output"] = structured_output.clone();
                }
                ordered.push(item);
            } else if finished.contains(&plan.item_id) {
                ordered.push(serde_json::json!({
                    "item_id": plan.item_id,
                    "ok": true,
                    "status": "completed",
                    "summary": "already finished; not replayed",
                }));
            } else {
                ordered.push(serde_json::json!({
                    "item_id": plan.item_id,
                    "ok": false,
                    "status": "queued",
                    "summary": "not started",
                }));
            }
        }

        let final_snapshot = multi_agent.swarm_snapshot(&swarm_id);
        let all_terminal = final_snapshot
            .as_ref()
            .is_some_and(|s| s.children.iter().all(|c| c.agent.state.is_terminal()));
        let any_failed = item_outcomes.iter().any(|(_, o)| !o.ok);
        let actual_usage = item_outcomes.iter().fold(None, |total, (_, outcome)| {
            outcome
                .actual_usage
                .map_or(total, |usage| Some(add_usage(total, usage)))
        });
        Ok(WorkflowInvocationOutcome {
            ok: all_terminal && !any_failed,
            status: if all_terminal && !any_failed {
                WorkflowOutcomeStatus::Completed
            } else if any_failed {
                WorkflowOutcomeStatus::Failed
            } else {
                WorkflowOutcomeStatus::Interrupted
            },
            summary: format!(
                "swarm {} items={} finished={}",
                swarm_id,
                plans.len(),
                item_outcomes.len()
            ),
            details: serde_json::json!({
                "kind": "delegate_swarm",
                "swarm_id": swarm_id,
                "items": ordered,
                "swarm": final_snapshot,
            }),
            actual_usage,
            child_refs: Vec::new(),
            interruption: None,
        })
    }

    async fn child_run_to_outcome_with_schema(
        &self,
        run_id: &WorkflowId,
        multi_agent: &MultiAgentRuntime,
        deps: ChildRuntimeDeps,
        plan: &ChildPlan,
        invocation_id: &str,
        output: &ChildRunOutput,
    ) -> Result<WorkflowInvocationOutcome, WorkflowError> {
        let mut outcome = child_run_to_outcome(output);
        // A failed child turn (provider, auth, rate-limit, cancellation, or
        // runtime) is returned unchanged with only observed usage. It never
        // enters schema compilation, schema-repair journal events, or a second
        // model request, so the original actionable error survives.
        if !outcome.ok {
            return Ok(outcome);
        }
        let Some(schema_doc) = plan.output_schema.as_ref() else {
            return Ok(outcome);
        };
        let schema = CompiledSchema::compile(schema_doc).map_err(|error| {
            WorkflowError::InvalidInput(format!(
                "{} output_schema compile failed: {error}",
                plan.item_id
            ))
        })?;
        let accepted = self
            .accept_child_structured_output_with_repair(
                run_id,
                multi_agent,
                deps,
                ChildSchemaRepairRequest {
                    invocation_id,
                    agent_id: &output.snapshot.id,
                    schema: &schema,
                    first_output: output,
                },
            )
            .await?;
        let details = outcome.details.as_object_mut().ok_or_else(|| {
            WorkflowError::Host("child outcome details must be an object".to_owned())
        })?;
        details.insert(
            "schema_repair_attempted".to_owned(),
            serde_json::json!(accepted.repair_attempted),
        );
        if let Some(repair_id) = accepted.repair_id {
            details.insert("repair_id".to_owned(), serde_json::json!(repair_id));
        }
        if accepted.ok {
            details.insert(
                "structured_output".to_owned(),
                accepted.value.unwrap_or(serde_json::Value::Null),
            );
            outcome.actual_usage = accepted.actual_usage;
        } else {
            outcome.ok = false;
            outcome.status = WorkflowOutcomeStatus::Failed;
            outcome.summary = accepted.summary.clone();
            details.insert(
                "schema_error".to_owned(),
                serde_json::json!(accepted.summary),
            );
            if let Some(code) = accepted.error_code {
                details.insert(
                    "schema_error_code".to_owned(),
                    serde_json::json!(code.as_str()),
                );
            }
            outcome.actual_usage = accepted.actual_usage;
        }
        Ok(outcome)
    }

    async fn append_child_event(
        &self,
        run_id: &WorkflowId,
        payload: JournalPayload,
    ) -> Result<(), WorkflowError> {
        let state = self.run_state(run_id).await?;
        let (journal, run_id_owned) = {
            let guard = state.lock().await;
            (
                guard.journal.clone().ok_or_else(|| {
                    WorkflowError::Journal("workflow journal is unavailable".to_owned())
                })?,
                guard.metadata.run_id.clone(),
            )
        };
        let timestamp_ms = current_timestamp_ms();
        let envelope = {
            let writer = journal
                .lock()
                .map_err(|_| WorkflowError::Host("workflow journal lock poisoned".to_owned()))?;
            JournalEnvelope::new(writer.next_seq(), timestamp_ms, run_id_owned, payload)
        };
        let sequence =
            self.journal_io(&journal, |writer| writer.append(&envelope, &self.limits))?;
        let mut guard = state.lock().await;
        guard.projection_sequence = Some(sequence);
        guard.updated_at_ms = Some(timestamp_ms);
        self.emit_projection(&guard, WorkflowProjectionStage::Updated);
        Ok(())
    }

    /// Durably bind a workflow-hosted direct delegate to its real agent id.
    pub async fn bind_direct_delegate_agent(
        &self,
        origin: &WorkflowExecutionOrigin,
        agent_id: &crate::multi_agent::AgentId,
    ) -> Result<(), WorkflowError> {
        let invocation_id = origin.invocation_id.as_deref().ok_or_else(|| {
            WorkflowError::InvalidInput(
                "direct delegate binding requires a workflow invocation id".to_owned(),
            )
        })?;
        if origin.swarm_item_id.is_some() {
            return Err(WorkflowError::InvalidInput(
                "direct delegate binding cannot target a swarm item".to_owned(),
            ));
        }
        let state = self.run_state(&origin.run_id).await?;
        {
            let guard = state.lock().await;
            if guard.current_invocation.as_deref() != Some(invocation_id)
                || guard.current_invocation_kind != Some(WorkflowInvocationKind::Delegate)
            {
                return Err(WorkflowError::InvalidInput(
                    "direct delegate binding does not match the active invocation".to_owned(),
                ));
            }
        }
        self.append_child_event(
            &origin.run_id,
            JournalPayload::ChildStarted {
                child_key: WorkflowChildKey::DirectDelegate {
                    invocation_id: invocation_id.to_owned(),
                },
                agent_id: Some(agent_id.as_str().to_owned()),
            },
        )
        .await
    }

    async fn finished_swarm_item_ids(
        &self,
        run_id: &WorkflowId,
        swarm_id: &str,
    ) -> Result<std::collections::HashSet<String>, WorkflowError> {
        let state = self.run_state(run_id).await?;
        let journal = {
            let guard = state.lock().await;
            guard.journal.clone().ok_or_else(|| {
                WorkflowError::Journal("workflow journal is unavailable".to_owned())
            })?
        };
        let writer = journal
            .lock()
            .map_err(|_| WorkflowError::Host("workflow journal lock poisoned".to_owned()))?;
        let prefix = format!("swarm:{swarm_id}:");
        Ok(writer
            .index()
            .finished_children
            .iter()
            .filter_map(|key| key.strip_prefix(&prefix).map(str::to_owned))
            .collect())
    }

    async fn is_pause_requested(&self, run_id: &WorkflowId) -> bool {
        let Ok(state) = self.run_state(run_id).await else {
            return false;
        };
        let guard = state.lock().await;
        guard.control.pause_requested.load(Ordering::Acquire)
    }

    async fn current_phase(&self, run_id: &WorkflowId) -> Result<Option<String>, WorkflowError> {
        let state = self.run_state(run_id).await?;
        Ok(state.lock().await.current_phase.clone())
    }

    async fn is_stop_requested(&self, run_id: &WorkflowId) -> bool {
        let Ok(state) = self.run_state(run_id).await else {
            return false;
        };
        let guard = state.lock().await;
        guard.control.stop_token.is_cancelled()
    }

    /// Validate a child output against `schema`, with exactly one tools-disabled repair.
    ///
    /// Ordering:
    /// 1. validate first provider-native/assistant value;
    /// 2. on failure, append `SchemaRepairStarted` before any corrective model call;
    /// 3. continue the same child session with tools disabled;
    /// 4. reject repair tool attempts as `schema_repair_tool_forbidden`;
    /// 5. append `SchemaRepairFinished` and aggregate both attempts' actual usage.
    ///
    /// Crash after start and before finish never re-dispatches the corrective model
    /// effect: recovery finishes the open repair as interrupted without a model call.
    pub async fn accept_child_structured_output_with_repair(
        &self,
        run_id: &WorkflowId,
        multi_agent: &MultiAgentRuntime,
        deps: ChildRuntimeDeps,
        request: ChildSchemaRepairRequest<'_>,
    ) -> Result<ChildSchemaAcceptResult, WorkflowError> {
        let ChildSchemaRepairRequest {
            invocation_id,
            agent_id,
            schema,
            first_output,
        } = request;
        let first_raw = child_final_assistant_text(first_output);
        let first_usage = accumulate_child_usage(None, &first_output.events);
        let first_source = StructuredOutputSource::AssistantText(first_raw.clone());
        match accept_structured_output(schema, first_source) {
            Ok(value) => Ok(ChildSchemaAcceptResult {
                ok: true,
                value: Some(value),
                error_code: None,
                summary: "child output matched schema".to_owned(),
                repair_attempted: false,
                repair_id: None,
                first_raw,
                repair_raw: None,
                actual_usage: first_usage,
            }),
            Err(first_err) => {
                // If a repair was already journaled for this invocation (crash mid-repair),
                // never repeat the corrective model effect.
                if self
                    .schema_repair_already_started(run_id, invocation_id)
                    .await?
                {
                    return Ok(ChildSchemaAcceptResult {
                        ok: false,
                        value: None,
                        error_code: Some(WorkflowErrorCode::InterruptedHostExit),
                        summary: format!(
                            "schema repair already started for {invocation_id}; not repeating model effect"
                        ),
                        repair_attempted: true,
                        repair_id: None,
                        first_raw,
                        repair_raw: None,
                        actual_usage: first_usage,
                    });
                }

                let repair_id = self.start_schema_repair(run_id, invocation_id).await?;

                let repair = multi_agent
                    .run_tools_disabled_schema_repair_turn(
                        deps,
                        agent_id,
                        &first_err.to_string(),
                        schema.schema(),
                    )
                    .await
                    .map_err(|e| WorkflowError::Host(format!("schema repair turn failed: {e}")))?;

                let repair_raw = repair.latest_text.clone().unwrap_or_default();
                let repair_usage = accumulate_child_usage(first_usage, &repair.events);
                let actual_usage = repair_usage;

                if repair.tool_attempted {
                    let summary = "schema_repair_tool_forbidden".to_owned();
                    self.finish_schema_repair(run_id, &repair_id, false, &summary)
                        .await?;
                    return Ok(ChildSchemaAcceptResult {
                        ok: false,
                        value: None,
                        error_code: Some(WorkflowErrorCode::SchemaRepairToolForbidden),
                        summary,
                        repair_attempted: true,
                        repair_id: Some(repair_id),
                        first_raw,
                        repair_raw: Some(repair_raw),
                        actual_usage,
                    });
                }

                let second_source = StructuredOutputSource::AssistantText(repair_raw.clone());
                match accept_structured_output(schema, second_source) {
                    Ok(value) => {
                        let summary = "child schema repaired".to_owned();
                        self.finish_schema_repair(run_id, &repair_id, true, &summary)
                            .await?;
                        Ok(ChildSchemaAcceptResult {
                            ok: true,
                            value: Some(value),
                            error_code: None,
                            summary,
                            repair_attempted: true,
                            repair_id: Some(repair_id),
                            first_raw,
                            repair_raw: Some(repair_raw),
                            actual_usage,
                        })
                    }
                    Err(second_err) => {
                        let summary = format!("schema_invalid after repair: {second_err}");
                        self.finish_schema_repair(run_id, &repair_id, false, &summary)
                            .await?;
                        Ok(ChildSchemaAcceptResult {
                            ok: false,
                            value: None,
                            error_code: Some(WorkflowErrorCode::SchemaInvalid),
                            summary,
                            repair_attempted: true,
                            repair_id: Some(repair_id),
                            first_raw,
                            repair_raw: Some(repair_raw),
                            actual_usage,
                        })
                    }
                }
            }
        }
    }

    /// Whether a schema repair was already journaled for `invocation_id`.
    pub async fn schema_repair_already_started(
        &self,
        run_id: &WorkflowId,
        invocation_id: &str,
    ) -> Result<bool, WorkflowError> {
        let state = self.run_state(run_id).await?;
        let guard = state.lock().await;
        let Some(journal) = guard.journal.as_ref() else {
            return Ok(false);
        };
        let writer = journal
            .lock()
            .map_err(|_| WorkflowError::Host("workflow journal lock poisoned".to_owned()))?;
        // Index tracks repair_ids; scan path for invocation linkage.
        drop(writer);
        let path = guard.journal_path();
        let run = guard.metadata.run_id.clone();
        drop(guard);
        let envelopes = journal::collect_journal(
            &path,
            Some(&run),
            self.limits.journal_record_bytes,
            self.limits.journal_total_bytes,
        )?;
        Ok(envelopes.iter().any(|env| {
            matches!(
                &env.payload,
                JournalPayload::SchemaRepairStarted {
                    invocation_id: inv,
                    ..
                } if inv == invocation_id
            )
        }))
    }

    /// Active (run_id, invocation_id) if exactly one run has a live invocation.
    pub async fn find_active_invocation(&self) -> Option<(WorkflowId, String)> {
        let runs = self.runs.lock().await;
        let mut found = None;
        for state in runs.values() {
            let guard = state.lock().await;
            if let Some(inv) = guard.current_invocation.clone() {
                if found.is_some() {
                    return None;
                }
                found = Some((guard.metadata.run_id.clone(), inv));
            }
        }
        found
    }

    /// Transition `Queued -> Running` without spawning a supervised worker.
    ///
    /// Production workers use [`Self::start_worker`]. Direct `LuaWorkflowRunner`
    /// execution (host-bound scripts and unit fixtures) call this so durable
    /// host APIs observe a Running run.
    pub async fn enter_running_without_worker(
        &self,
        run_id: &WorkflowId,
    ) -> Result<(), WorkflowError> {
        let state = self.run_state(run_id).await?;
        {
            let guard = state.lock().await;
            if guard.state != WorkflowState::Queued {
                return Err(WorkflowError::InvalidInput(
                    "enter_running_without_worker requires queued state".to_owned(),
                ));
            }
            if guard.worker_active {
                return Err(WorkflowError::InvalidInput(
                    "worker already active".to_owned(),
                ));
            }
        }
        self.transition(
            &state,
            WorkflowState::Running,
            "direct_execution",
            WorkflowActor::Runtime,
        )
        .await
    }

    /// List journal-visible artifact metadata for a run.
    pub async fn list_artifacts(
        &self,
        run_id: &WorkflowId,
    ) -> Result<Vec<ArtifactMetadata>, WorkflowError> {
        let state = self.run_state(run_id).await?;
        let guard = state.lock().await;
        Ok(guard.artifacts.list_metadata().to_vec())
    }

    /// Read a journal-visible artifact with integrity revalidation.
    pub async fn get_artifact(
        &self,
        run_id: &WorkflowId,
        artifact_id: &super::state::WorkflowArtifactId,
    ) -> Result<super::artifacts::ArtifactContent, WorkflowError> {
        let store = {
            let state = self.run_state(run_id).await?;
            let guard = state.lock().await;
            guard.artifacts.clone()
        };
        let artifact_id = artifact_id.clone();
        tokio::task::spawn_blocking(move || store.get(&artifact_id))
            .await
            .map_err(|e| WorkflowError::Host(format!("artifact read join failed: {e}")))?
    }

    /// Read a byte range of a journal-visible artifact (outside the run lock).
    pub async fn read_artifact_range(
        &self,
        run_id: &WorkflowId,
        artifact_id: &super::state::WorkflowArtifactId,
        offset: u64,
        max_bytes: u64,
    ) -> Result<super::artifacts::ArtifactContentRange, WorkflowError> {
        let store = {
            let state = self.run_state(run_id).await?;
            let guard = state.lock().await;
            guard.artifacts.clone()
        };
        let artifact_id = artifact_id.clone();
        tokio::task::spawn_blocking(move || store.read_range(&artifact_id, offset, max_bytes))
            .await
            .map_err(|e| WorkflowError::Host(format!("artifact range join failed: {e}")))?
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
        let mut writer =
            match JournalWriter::open(&journal_path, metadata.run_id.clone(), &self.limits) {
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

        // Reconcile durable starts without finishes via the production
        // (or test-injected) read-only resolver. Never relaunches effects.
        if let Err(error) = self
            .reconcile_incomplete_invocations(&mut writer, &metadata, &journal_path)
            .await
        {
            handles.push(
                self.insert_failed_run(
                    run_dir,
                    metadata,
                    format!("recovery append failed: {error}"),
                )
                .await,
            );
            return Ok(());
        }
        // Open schema repairs never re-dispatch the corrective model effect.
        if let Err(error) = self.reconcile_open_schema_repairs(&mut writer, &metadata) {
            handles.push(
                self.insert_failed_run(
                    run_dir,
                    metadata,
                    format!("schema repair recovery failed: {error}"),
                )
                .await,
            );
            return Ok(());
        }

        // Crash after FinalResultRecorded / before Completed: append only the
        // missing terminal state. Never re-execute Lua or rewrite the result.
        if writer.index().final_result_seq.is_some() && writer.index().terminal_state.is_none() {
            let timestamp_ms = current_timestamp_ms();
            let prepared = writer
                .index()
                .current_state
                .ok_or_else(|| {
                    WorkflowError::Journal(
                        "final-result recovery requires a durable current state".to_owned(),
                    )
                })
                .and_then(|previous| {
                    effect::prepare_transition(
                        &writer,
                        metadata.run_id.clone(),
                        previous,
                        WorkflowState::Completed,
                        "recover_final_result",
                        WorkflowActor::Runtime,
                        timestamp_ms,
                    )
                });
            match prepared {
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

        let mut envelopes = match journal::collect_journal(
            &journal_path,
            Some(&metadata.run_id),
            self.limits.journal_record_bytes,
            self.limits.journal_total_bytes,
        ) {
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
        // Durable host-exit: Queued/Running runs paused by process exit must
        // leave a journaled state transition so projection sequences advance
        // and session rehydrate can emit WorkflowUpdated exactly once.
        let (last_state, _) = support::last_state(&envelopes);
        if last_state.rehydrates_as_paused_host_exit() {
            let timestamp_ms = current_timestamp_ms();
            match effect::prepare_transition(
                &writer,
                metadata.run_id.clone(),
                last_state,
                WorkflowState::Paused,
                "host_exit",
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
                                format!("host_exit recovery failed: {error}"),
                            )
                            .await,
                        );
                        return Ok(());
                    }
                    envelopes = match journal::collect_journal(
                        &journal_path,
                        Some(&metadata.run_id),
                        self.limits.journal_record_bytes,
                        self.limits.journal_total_bytes,
                    ) {
                        Ok(records) if !records.is_empty() => records,
                        Ok(_) => {
                            handles.push(
                                self.insert_failed_run(
                                    run_dir,
                                    metadata,
                                    "host_exit recovery cleared journal".to_owned(),
                                )
                                .await,
                            );
                            return Ok(());
                        }
                        Err(error) => {
                            handles.push(
                                self.insert_failed_run(
                                    run_dir,
                                    metadata,
                                    format!("host_exit recovery reread failed: {error}"),
                                )
                                .await,
                            );
                            return Ok(());
                        }
                    };
                }
                Err(error) => {
                    handles.push(
                        self.insert_failed_run(
                            run_dir,
                            metadata,
                            format!("host_exit recovery transition rejected: {error}"),
                        )
                        .await,
                    );
                    return Ok(());
                }
            }
        }
        let (final_state, terminal_reason) = projection_state(&envelopes);
        handles.push(
            self.insert_rehydrated(
                run_dir,
                metadata,
                envelopes,
                final_state,
                terminal_reason,
                Some(Arc::new(StdMutex::new(writer))),
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
                let display_name = snapshot.display_name.clone();
                let purpose = snapshot.purpose.clone();
                let _ = self.notifications.enqueue(WorkflowNotification::new(
                    session_dir,
                    snapshot.id,
                    snapshot.state,
                    snapshot
                        .terminal_reason
                        .unwrap_or_else(|| "terminal".to_owned()),
                    display_name,
                    purpose,
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
            let phase_id = guard.current_phase.clone();
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
                phase_id,
                timestamp_ms,
            }
        };

        // --- reserve + durable InvocationStarted (no async run lock) ---
        let start_result = self.journal_io(&prepared.journal, |writer| {
            let mut sequence =
                effect::commit_invocation_start(writer, &prepared.prepared_start, &self.limits)?;
            if prepared.prepared_start.kind == WorkflowInvocationKind::Delegate {
                let input = match &prepared.prepared_start.envelope.payload {
                    JournalPayload::InvocationStarted {
                        canonical_input: Some(input),
                        ..
                    } => input,
                    _ => unreachable!("prepared delegate start must carry canonical input"),
                };
                let (title, role) = child_spec(input);
                let child_key = WorkflowChildKey::DirectDelegate {
                    invocation_id: prepared.prepared_start.invocation_id.clone(),
                };
                let queued = JournalEnvelope::new(
                    writer.next_seq(),
                    prepared.timestamp_ms,
                    prepared.prepared_start.envelope.run_id.clone(),
                    JournalPayload::ChildQueued {
                        child_key: child_key.clone(),
                        child_kind: WorkflowChildKind::Delegate,
                        invocation_id: prepared.prepared_start.invocation_id.clone(),
                        phase_id: prepared.phase_id.clone(),
                        title,
                        role,
                    },
                );
                sequence = writer.append(&queued, &self.limits)?;
            }
            Ok(sequence)
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
            guard.current_invocation_kind = Some(kind);
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

        if kind == WorkflowInvocationKind::Delegate {
            self.journal_io(&journal, |writer| {
                append_child_finished(
                    writer,
                    &invocation_id,
                    &outcome,
                    None,
                    timestamp_ms,
                    &self.limits,
                )
            })?;
        }

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
                let bounded = bounded_resource_limited_outcome(&reason, &outcome);
                let prepared = {
                    let writer = journal.lock().map_err(|_| {
                        WorkflowError::Host("workflow journal lock poisoned".to_owned())
                    })?;
                    effect::prepare_invocation_finish(
                        &writer,
                        run_id.clone(),
                        invocation_id.clone(),
                        bounded.clone(),
                        timestamp_ms,
                    )
                };
                let sequence = self.journal_io(&journal, |writer| {
                    effect::commit_invocation_finish(writer, &prepared, &self.limits)
                })?;
                (sequence, bounded, Some(reason))
            }
            Err(WorkflowError::JournalTotalLimitExceeded) => {
                let reason = "workflow journal total limit reached".to_owned();
                let bounded = bounded_resource_limited_outcome(&reason, &outcome);
                let prepared = {
                    let writer = journal.lock().map_err(|_| {
                        WorkflowError::Host("workflow journal lock poisoned".to_owned())
                    })?;
                    effect::prepare_invocation_finish(
                        &writer,
                        run_id.clone(),
                        invocation_id.clone(),
                        bounded.clone(),
                        timestamp_ms,
                    )
                };
                let sequence = self.journal_io(&journal, |writer| {
                    effect::commit_invocation_finish(writer, &prepared, &self.limits)
                })?;
                (sequence, bounded, Some(reason))
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
            guard.current_invocation_kind = None;
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
            guard.current_invocation = None;
            guard.current_invocation_kind = None;
            self.release_worker_admission_locked(&mut guard);
            if guard.state.is_terminal()
                || guard.state == WorkflowState::Paused
                || guard.state == WorkflowState::AwaitingUser
            {
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
                Err(error) if error.code() == WorkflowErrorCode::AwaitingUser => {
                    // await_user already transitioned to AwaitingUser and released permits.
                    let current = state.lock().await.state;
                    if current == WorkflowState::AwaitingUser {
                        Ok(())
                    } else {
                        self.transition(
                            &state,
                            WorkflowState::AwaitingUser,
                            "user_input",
                            WorkflowActor::Runtime,
                        )
                        .await
                    }
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
                guard.current_invocation = None;
                guard.current_invocation_kind = None;
                return Ok(());
            }
        }

        let current = {
            let guard = state.lock().await;
            guard
                .current_invocation
                .clone()
                .map(|invocation_id| (invocation_id, guard.current_invocation_kind))
        };
        if let Some((invocation_id, invocation_kind)) = current {
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
                    let mut writer = journal.lock().map_err(|_| {
                        WorkflowError::Host("workflow journal lock poisoned".to_owned())
                    })?;
                    if invocation_kind == Some(WorkflowInvocationKind::Delegate) {
                        let child_key = WorkflowChildKey::DirectDelegate {
                            invocation_id: invocation_id.clone(),
                        };
                        let agent_id = writer
                            .index()
                            .child_agent_ids
                            .get(&child_key.display_key())
                            .and_then(Clone::clone);
                        append_child_finished(
                            &mut writer,
                            &invocation_id,
                            &outcome,
                            agent_id.as_deref(),
                            timestamp_ms,
                            &self.limits,
                        )?;
                    }
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
                        guard.current_invocation_kind = None;
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
        state.current_invocation_kind = None;
        // Unsequenced recovery-failure projection must not leak occupancy.
        self.release_worker_admission_locked(state);
        state.state = WorkflowState::Failed;
        state.failure_count = state.failure_count.saturating_add(1);
        state.projection_sequence = None;
        state.updated_at_ms = Some(current_timestamp_ms());
        state.terminal_reason = Some(reason.to_owned());
        state.journal = None;
        self.emit_projection(state, WorkflowProjectionStage::Finished);
        if let Some(session_dir) = state.run_dir.parent().and_then(Path::parent) {
            let display_name = state
                .metadata
                .display_name
                .as_deref()
                .unwrap_or(&state.metadata.name)
                .to_owned();
            let purpose = state.metadata.description.clone();
            let _ = self.notifications.enqueue(WorkflowNotification::new(
                session_dir,
                state.metadata.run_id.clone(),
                WorkflowState::Failed,
                reason,
                display_name,
                purpose,
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
                let display_name = guard
                    .metadata
                    .display_name
                    .as_deref()
                    .unwrap_or(&guard.metadata.name)
                    .to_owned();
                let purpose = guard.metadata.description.clone();
                let _ = self.notifications.enqueue(WorkflowNotification::new(
                    session_dir,
                    guard.metadata.run_id.clone(),
                    new_state,
                    reason,
                    display_name,
                    purpose,
                ));
            }
        }
        Ok(())
    }

    /// Run blocking journal I/O without holding the async run-state mutex.
    fn journal_io<R>(
        &self,
        journal: &SharedJournal,
        f: impl FnOnce(&mut JournalWriter) -> Result<R, WorkflowError>,
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

    /// Append finishes for durable starts lacking finishes using the bound
    /// read-only resolver. Adopts exactly one proven terminal result; zero /
    /// conflicting / unknown results become interrupted(host_exit). Never
    /// dispatches or auto-retries external effects.
    async fn reconcile_incomplete_invocations(
        &self,
        writer: &mut JournalWriter,
        metadata: &WorkflowRunMetadata,
        journal_path: &Path,
    ) -> Result<(), WorkflowError> {
        let envelopes = journal::collect_journal(
            journal_path,
            Some(&metadata.run_id),
            self.limits.journal_record_bytes,
            self.limits.journal_total_bytes,
        )?;
        let incomplete = find_incomplete_invocations(&envelopes);
        if incomplete.is_empty() {
            return Ok(());
        }

        let mut open_children = HashMap::<WorkflowChildKey, (String, Option<String>)>::new();
        for envelope in &envelopes {
            match &envelope.payload {
                JournalPayload::ChildQueued {
                    child_key,
                    invocation_id,
                    ..
                } => {
                    open_children.insert(child_key.clone(), (invocation_id.clone(), None));
                }
                JournalPayload::ChildStarted {
                    child_key,
                    agent_id,
                } => {
                    if let Some((_, bound_agent_id)) = open_children.get_mut(child_key) {
                        bound_agent_id.clone_from(agent_id);
                    }
                }
                JournalPayload::ChildFinished { child_key, .. } => {
                    open_children.remove(child_key);
                }
                _ => {}
            }
        }

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

            let timestamp_ms = current_timestamp_ms();
            if invocation.kind == WorkflowInvocationKind::Delegate {
                let child_key = WorkflowChildKey::DirectDelegate {
                    invocation_id: invocation.invocation_id.clone(),
                };
                let agent_id = open_children
                    .get(&child_key)
                    .and_then(|(_, agent_id)| agent_id.as_deref());
                if writer
                    .index()
                    .queued_children
                    .contains(&child_key.display_key())
                {
                    append_child_finished(
                        writer,
                        &invocation.invocation_id,
                        &outcome,
                        agent_id,
                        timestamp_ms,
                        &self.limits,
                    )?;
                    open_children.remove(&child_key);
                }
            }
            let interrupted_children = open_children
                .iter()
                .filter(|(_, (parent_invocation_id, _))| {
                    parent_invocation_id == &invocation.invocation_id
                })
                .map(|(child_key, (_, agent_id))| (child_key.clone(), agent_id.clone()))
                .collect::<Vec<_>>();
            for (child_key, agent_id) in interrupted_children {
                let summary = "interrupted(host_exit); child effect was not repeated".to_owned();
                let envelope = JournalEnvelope::new(
                    writer.next_seq(),
                    timestamp_ms,
                    metadata.run_id.clone(),
                    JournalPayload::ChildFinished {
                        child_key: child_key.clone(),
                        agent_id,
                        status: WorkflowOutcomeStatus::Interrupted,
                        summary: summary.clone(),
                        actual_usage: None,
                        error: Some(summary),
                    },
                );
                writer.append(&envelope, &self.limits)?;
                open_children.remove(&child_key);
            }
            let prepared = effect::prepare_invocation_finish(
                writer,
                metadata.run_id.clone(),
                invocation.invocation_id.clone(),
                outcome,
                timestamp_ms,
            );
            effect::commit_invocation_finish(writer, &prepared, &self.limits)?;
        }
        Ok(())
    }

    /// Finish open schema repairs as interrupted without re-dispatching the model.
    fn reconcile_open_schema_repairs(
        &self,
        writer: &mut JournalWriter,
        metadata: &WorkflowRunMetadata,
    ) -> Result<(), WorkflowError> {
        let open: Vec<String> = writer
            .index()
            .open_schema_repairs
            .iter()
            .filter(|id| !writer.index().finished_schema_repairs.contains(*id))
            .cloned()
            .collect();
        for repair_id in open {
            let timestamp_ms = current_timestamp_ms();
            let envelope = JournalEnvelope::new(
                writer.next_seq(),
                timestamp_ms,
                metadata.run_id.clone(),
                JournalPayload::SchemaRepairFinished {
                    repair_id,
                    ok: false,
                    summary: "interrupted(host_exit); schema repair not repeated".to_owned(),
                },
            );
            writer.append(&envelope, &self.limits)?;
        }
        Ok(())
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

    async fn insert_rehydrated(
        &self,
        run_dir: PathBuf,
        metadata: WorkflowRunMetadata,
        envelopes: Vec<JournalEnvelope>,
        state: WorkflowState,
        terminal_reason: Option<String>,
        writer: Option<SharedJournal>,
    ) -> WorkflowHandle {
        let replay_entries = replay_entries(&envelopes);
        let projection_sequence = envelopes.last().map(JournalEnvelope::seq);
        let (started_at_ms, updated_at_ms) = projection_timestamps(&envelopes);
        let control = Arc::new(RunControl::new());
        let run_id = metadata.run_id.clone();
        let final_result = final_result(&envelopes);
        let pending_user_input =
            latest_open_user_input(&envelopes).or_else(|| latest_user_input(&envelopes));
        let mut artifacts = ArtifactStore::open(&run_dir, run_id.clone())
            .unwrap_or_else(|_| ArtifactStore::empty(run_id.clone(), &run_dir));
        // Best-effort rehydrate: corrupt/missing files stay invisible and typed on get.
        let _ = artifacts.rehydrate_from_envelopes(&envelopes);
        let run_state = RunState {
            current_phase: recovered_phase(&envelopes),
            invocation_count: invocation_count(&envelopes),
            failure_count: failure_count(&envelopes),
            actual_usage: aggregate_usage(&envelopes),
            projection_sequence,
            started_at_ms,
            updated_at_ms,
            latest_log_summary: latest_log_summary(&replay_entries),
            latest_report_summary: latest_report_summary(&envelopes),
            reports: recovered_reports(&envelopes),
            metadata,
            state,
            terminal_reason,
            run_dir,
            control: Arc::clone(&control),
            worker_active: false,
            worker_join: None,
            worker_permit: None,
            current_invocation: None,
            current_invocation_kind: None,
            replay_entries,
            replay_cursor: 0,
            replay_live: false,
            journal: writer,
            final_result,
            pending_user_input,
            artifacts,
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
            name: "corrupt workflow".to_owned(),
            description: String::new(),
            phases: Vec::new(),
            script: String::new(),
            script_sha256: String::new(),
            args: serde_json::json!({}),
            launch_source: "rehydrate".to_owned(),
            output_schema: None,
            display_name: None,
            input_schema: None,
            definition_origin: None,
            inline_unsaved: false,
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
        let artifacts = ArtifactStore::empty(run_id.clone(), &run_dir);
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
            current_invocation_kind: None,
            replay_entries: Vec::new(),
            replay_cursor: 0,
            replay_live: false,
            journal: None,
            final_result: None,
            pending_user_input: None,
            artifacts,
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
    phase_id: Option<String>,
    timestamp_ms: u64,
}

fn projection_state(envelopes: &[JournalEnvelope]) -> (WorkflowState, Option<String>) {
    let (state, reason) = support::last_state(envelopes);
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

    /// Bounded TaskOutput page (summary/journal/result/artifacts/artifact_content).
    pub async fn task_output(
        &self,
        request: TaskOutputRequest,
    ) -> Result<TaskOutputPage, WorkflowError> {
        self.runtime.task_output(&self.run_id, request).await
    }

    /// Build typed provenance for a host-dispatched tool or child effect.
    pub async fn execution_origin(&self, swarm_item_id: Option<String>) -> WorkflowExecutionOrigin {
        let output = self.output().await.ok();
        let (definition_name, definition_revision, phase_id) = match output.as_ref() {
            Some(out) => (
                out.metadata.name.clone(),
                Some(out.metadata.script_sha256.clone()),
                out.current_phase.clone(),
            ),
            None => (String::new(), None, None),
        };
        WorkflowExecutionOrigin {
            run_id: self.run_id.clone(),
            human_handle: None,
            definition_name,
            definition_revision,
            phase_id,
            invocation_id: None,
            swarm_item_id,
        }
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

    pub async fn commit_artifact(
        &self,
        logical_name: &str,
        kind: ArtifactKind,
        value: ArtifactValue,
        media_type: Option<&str>,
    ) -> Result<ArtifactMetadata, WorkflowError> {
        self.runtime
            .commit_artifact(&self.run_id, logical_name, kind, value, media_type)
            .await
    }

    pub async fn persist_canonical_final_result(
        &self,
        value: serde_json::Value,
        schema_revision: Option<WorkflowRevision>,
    ) -> Result<CanonicalFinalResult, WorkflowError> {
        self.runtime
            .persist_canonical_final_result(&self.run_id, value, schema_revision)
            .await
    }

    /// Validate optional final schema then persist. Never invokes a model.
    pub async fn accept_final_lua_result(
        &self,
        value: serde_json::Value,
        schema: Option<&CompiledSchema>,
        schema_revision: Option<WorkflowRevision>,
    ) -> Result<CanonicalFinalResult, WorkflowError> {
        self.runtime
            .accept_final_lua_result(&self.run_id, value, schema, schema_revision)
            .await
    }

    pub async fn start_schema_repair(&self, invocation_id: &str) -> Result<String, WorkflowError> {
        self.runtime
            .start_schema_repair(&self.run_id, invocation_id)
            .await
    }

    pub async fn finish_schema_repair(
        &self,
        repair_id: &str,
        ok: bool,
        summary: impl Into<String>,
    ) -> Result<(), WorkflowError> {
        self.runtime
            .finish_schema_repair(&self.run_id, repair_id, ok, summary)
            .await
    }

    pub async fn await_user(
        &self,
        call_index: u64,
        input: AwaitUserInput,
    ) -> Result<serde_json::Value, WorkflowError> {
        self.runtime
            .await_user(&self.run_id, call_index, input)
            .await
    }

    pub async fn answer(
        &self,
        request_id: &str,
        value: serde_json::Value,
        actor: WorkflowActor,
    ) -> Result<(), WorkflowError> {
        self.runtime
            .answer(&self.run_id, request_id, value, actor)
            .await
    }

    pub async fn pending_user_input(&self) -> Result<Option<PendingUserInput>, WorkflowError> {
        self.runtime.pending_user_input(&self.run_id).await
    }

    pub async fn operator_snapshot(
        &self,
        task_id: &str,
    ) -> Result<super::WorkflowOperatorSnapshot, WorkflowError> {
        self.runtime.operator_snapshot(&self.run_id, task_id).await
    }

    pub async fn operator_child_page(
        &self,
        request: &super::WorkflowOperatorRequest,
    ) -> Result<super::WorkflowChildPage, WorkflowError> {
        self.runtime
            .operator_child_page(&self.run_id, request)
            .await
    }

    pub async fn accept_child_structured_output_with_repair(
        &self,
        multi_agent: &MultiAgentRuntime,
        deps: ChildRuntimeDeps,
        request: ChildSchemaRepairRequest<'_>,
    ) -> Result<ChildSchemaAcceptResult, WorkflowError> {
        self.runtime
            .accept_child_structured_output_with_repair(&self.run_id, multi_agent, deps, request)
            .await
    }

    pub async fn schema_repair_already_started(
        &self,
        invocation_id: &str,
    ) -> Result<bool, WorkflowError> {
        self.runtime
            .schema_repair_already_started(&self.run_id, invocation_id)
            .await
    }

    /// Put this run into Running without a supervised worker (direct Lua execution).
    pub async fn enter_running_for_direct_execution(&self) -> Result<(), WorkflowError> {
        self.runtime
            .enter_running_without_worker(&self.run_id)
            .await
    }

    pub async fn list_artifacts(&self) -> Result<Vec<ArtifactMetadata>, WorkflowError> {
        self.runtime.list_artifacts(&self.run_id).await
    }

    pub async fn get_artifact(
        &self,
        artifact_id: &super::state::WorkflowArtifactId,
    ) -> Result<super::artifacts::ArtifactContent, WorkflowError> {
        self.runtime.get_artifact(&self.run_id, artifact_id).await
    }

    pub async fn read_artifact_range(
        &self,
        artifact_id: &super::state::WorkflowArtifactId,
        offset: u64,
        max_bytes: u64,
    ) -> Result<super::artifacts::ArtifactContentRange, WorkflowError> {
        self.runtime
            .read_artifact_range(&self.run_id, artifact_id, offset, max_bytes)
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

    /// Heterogeneous `neo.swarm` entry: lowers to [`ChildPlan`]s and runs through
    /// the multi-agent batch owner with durable per-item journal records.
    pub async fn invoke_swarm_batch(
        &self,
        request: SwarmBatchRequest,
        multi_agent: MultiAgentRuntime,
        deps: ChildRuntimeDeps,
    ) -> Result<WorkflowInvocationOutcome, WorkflowError> {
        self.runtime
            .invoke_swarm_batch(&self.run_id, request, multi_agent, deps)
            .await
    }

    pub async fn child_projection(&self) -> Result<super::ChildProjection, WorkflowError> {
        let state = self.runtime.run_state(&self.run_id).await?;
        let journal_path = state.lock().await.journal_path();
        super::project_children(
            &journal_path,
            Some(&self.run_id),
            self.runtime.limits.journal_record_bytes,
            self.runtime.limits.journal_total_bytes,
        )
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

fn child_run_to_outcome(output: &ChildRunOutput) -> WorkflowInvocationOutcome {
    let mut outcome = child_agent_to_outcome(&output.snapshot);
    outcome.actual_usage = accumulate_child_usage(None, &output.events);
    outcome
}

fn child_agent_to_outcome(agent: &crate::multi_agent::AgentSnapshot) -> WorkflowInvocationOutcome {
    let summary = agent.outcome.as_ref().map_or_else(
        || agent.state.as_str().to_owned(),
        |outcome| outcome.summary.clone(),
    );
    let is_error = agent.outcome.as_ref().map_or_else(
        || agent.state != crate::multi_agent::AgentLifecycleState::Completed,
        |outcome| outcome.is_error,
    );
    WorkflowInvocationOutcome {
        ok: !is_error && agent.state == crate::multi_agent::AgentLifecycleState::Completed,
        status: match agent.state {
            crate::multi_agent::AgentLifecycleState::Completed => WorkflowOutcomeStatus::Completed,
            crate::multi_agent::AgentLifecycleState::Cancelled => WorkflowOutcomeStatus::Cancelled,
            crate::multi_agent::AgentLifecycleState::Failed
            | crate::multi_agent::AgentLifecycleState::TimedOut
            | crate::multi_agent::AgentLifecycleState::Interrupted => WorkflowOutcomeStatus::Failed,
            _ => WorkflowOutcomeStatus::Interrupted,
        },
        summary,
        details: serde_json::json!({
            "agent_id": agent.id.as_str(),
            "status": agent.state.as_str(),
        }),
        actual_usage: None,
        child_refs: Vec::new(),
        interruption: None,
    }
}

fn append_child_finished(
    writer: &mut JournalWriter,
    invocation_id: &str,
    outcome: &WorkflowInvocationOutcome,
    agent_id_hint: Option<&str>,
    timestamp_ms: u64,
    limits: &WorkflowLimits,
) -> Result<u64, WorkflowError> {
    let child_key = WorkflowChildKey::DirectDelegate {
        invocation_id: invocation_id.to_owned(),
    };
    if writer
        .index()
        .finished_children
        .contains(&child_key.display_key())
    {
        return Ok(writer.next_seq().saturating_sub(1));
    }
    let agent_id = outcome
        .details
        .get("agent_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            outcome
                .child_refs
                .iter()
                .find(|reference| reference.kind == "delegate")
                .map(|reference| reference.id.clone())
        })
        .or_else(|| agent_id_hint.map(str::to_owned));
    let summary = bounded_summary(&outcome.summary);
    let envelope = JournalEnvelope::new(
        writer.next_seq(),
        timestamp_ms,
        writer.run_id().clone(),
        JournalPayload::ChildFinished {
            child_key,
            agent_id,
            status: outcome.status,
            summary: summary.clone(),
            actual_usage: outcome.actual_usage,
            error: (!outcome.ok).then_some(summary),
        },
    );
    writer.append(&envelope, limits)
}

fn child_spec(input: &serde_json::Value) -> (Option<String>, Option<String>) {
    let title = input
        .get("title")
        .or_else(|| input.get("task"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned);
    let role = input
        .get("role")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned);
    (title, role)
}

fn accumulate_child_usage(
    total: Option<AgentTokenUsage>,
    events: &[crate::AgentEvent],
) -> Option<AgentTokenUsage> {
    events.iter().fold(total, |total, event| {
        let crate::AgentEvent::TokenUsage { usage, .. } = event else {
            return total;
        };
        Some(add_usage(total, *usage))
    })
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

fn user_input_from_envelopes(
    envelopes: &[JournalEnvelope],
    request_id: &str,
) -> Option<PendingUserInput> {
    let mut pending: Option<PendingUserInput> = None;
    for envelope in envelopes {
        match &envelope.payload {
            JournalPayload::UserInputRequested {
                request_id: rid,
                prompt,
                answer_schema,
                default,
                title,
                answer_policy,
            } if rid == request_id => {
                pending = Some(PendingUserInput {
                    request_id: rid.clone(),
                    prompt: prompt.clone(),
                    answer_schema: answer_schema.clone(),
                    default: default.clone(),
                    title: title.clone(),
                    answer_policy: *answer_policy,
                    answer: None,
                });
            }
            JournalPayload::UserInputAnswered {
                request_id: rid,
                answer,
                ..
            } if rid == request_id => {
                if let Some(entry) = pending.as_mut() {
                    entry.answer = answer.clone();
                }
            }
            _ => {}
        }
    }
    pending
}

fn latest_open_user_input(envelopes: &[JournalEnvelope]) -> Option<PendingUserInput> {
    let mut open: Option<PendingUserInput> = None;
    let mut answered = std::collections::HashSet::new();
    for envelope in envelopes {
        match &envelope.payload {
            JournalPayload::UserInputRequested {
                request_id,
                prompt,
                answer_schema,
                default,
                title,
                answer_policy,
            } => {
                open = Some(PendingUserInput {
                    request_id: request_id.clone(),
                    prompt: prompt.clone(),
                    answer_schema: answer_schema.clone(),
                    default: default.clone(),
                    title: title.clone(),
                    answer_policy: *answer_policy,
                    answer: None,
                });
            }
            JournalPayload::UserInputAnswered { request_id, .. } => {
                answered.insert(request_id.clone());
                if open.as_ref().is_some_and(|p| p.request_id == *request_id) {
                    open = None;
                }
            }
            _ => {}
        }
    }
    open.filter(|p| !answered.contains(&p.request_id))
}

fn latest_user_input(envelopes: &[JournalEnvelope]) -> Option<PendingUserInput> {
    let mut last_id: Option<String> = None;
    for envelope in envelopes {
        if let JournalPayload::UserInputRequested { request_id, .. } = &envelope.payload {
            last_id = Some(request_id.clone());
        }
    }
    last_id.and_then(|id| user_input_from_envelopes(envelopes, &id))
}
