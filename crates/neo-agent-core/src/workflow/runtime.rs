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
use super::artifacts::{ArtifactKind, ArtifactMetadata, ArtifactStore, ArtifactValue};
use super::error::{WorkflowError, WorkflowErrorCode};
use super::journal::{
    self, IncompleteInvocation, JournalEnvelope, JournalPayload, JournalRecord, JournalV2Writer,
    canonical_input_hash, find_incomplete_invocations_v2,
};
use super::limits::WorkflowLimits;
use super::output::{
    CanonicalFinalResult, PreparedFinalBody, prepare_final_body, reconstruct_canonical_final_result,
};
use super::schema::{
    CompiledSchema, StructuredOutputSource, accept_structured_output, validate_final_lua_result,
};
use super::state::{
    WorkflowActor, WorkflowFinalResultMetadata, WorkflowId, WorkflowInvocationKind,
    WorkflowInvocationOutcome, WorkflowOutcomeStatus, WorkflowPhase, WorkflowRevision,
    WorkflowRunMetadata, WorkflowSnapshot, WorkflowState,
};
use crate::AgentTokenUsage;
use crate::multi_agent::{
    AgentId, ChildRunOutput, ChildRuntimeDeps, MultiAgentRuntime, child_final_assistant_text,
};
use crate::runtime::{WorkflowNotification, WorkflowNotificationQueue};

#[path = "effect.rs"]
mod effect;
#[path = "lineage.rs"]
pub mod lineage;
#[path = "runtime_support.rs"]
mod support;
pub use lineage::{
    LineageSeedInvocation, SeedArtifactRef, VerifiedPrefix, compute_prefix_digest_v1,
    compute_prefix_digest_v2, extract_verified_prefix_v1, extract_verified_prefix_v2,
    import_seed_artifact, latest_eligible_sequence_v1, latest_eligible_sequence_v2,
    seed_pair_count_from_journal, split_usage_for_seed,
};
use support::{
    ReplayEntry, RunControl, add_usage, aggregate_usage, aggregate_usage_v2, bounded_summary,
    compact_resource_limited_outcome, current_timestamp_ms, failure_count_v2, final_result_v2,
    interrupted_outcome, invocation_count_v2, last_state, latest_log_summary,
    latest_report_summary, latest_report_summary_v2, projection_timestamps,
    projection_timestamps_v2, recovered_phase, recovered_phase_v2, recovered_reports,
    recovered_reports_v2, replay_entries, replay_entries_v2, report_summary,
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

/// Explicit linked-run / V2-upgrade launch (design §34). Requires fresh authorization.
#[derive(Debug, Clone)]
pub struct LinkedRunRequest {
    pub parent_run_id: WorkflowId,
    /// When `None`, imports the latest eligible completed checkpoint.
    pub checkpoint: Option<super::state::WorkflowCheckpoint>,
    pub link_reason: String,
    pub launch: WorkflowLaunchRequest,
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
    /// Usage imported from lineage seed; never charged to actual_usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherited_usage: Option<AgentTokenUsage>,
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
    /// Token usage inherited from lineage seed (display only).
    inherited_usage: Option<AgentTokenUsage>,
    /// Completed seed host-call pairs that must match before any new effect.
    seed_entry_count: usize,
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
    /// Run-scoped immutable artifact store (visibility requires journal commit).
    artifacts: ArtifactStore,
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
        let artifacts = ArtifactStore::open(&run_dir, run_id.clone()).map_err(|error| {
            self.admission.release_storage_owner(run_id.as_str());
            let _ = std::fs::remove_dir_all(&run_dir);
            error
        })?;
        let state = Arc::new(Mutex::new(RunState {
            metadata,
            state: WorkflowState::Queued,
            current_phase: None,
            invocation_count: 0,
            failure_count: 0,
            actual_usage: None,
            inherited_usage: None,
            seed_entry_count: 0,
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
            artifacts,
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

    /// Create a linked V2 run from a verified parent checkpoint (design §34).
    ///
    /// Imports the completed invocation prefix and referenced artifacts into the
    /// new journal. Requires a fresh capability reservation. Never mutates the
    /// parent run (terminal or V1 read-only).
    pub async fn create_linked_run(
        &self,
        session_dir: &Path,
        request: LinkedRunRequest,
        authorization: Option<super::capability::WorkflowCapabilityReservation>,
    ) -> Result<WorkflowHandle, WorkflowError> {
        let Some(authorization) = authorization else {
            return Err(WorkflowError::coded(
                super::error::WorkflowErrorCode::LaunchAuthorizationMissing,
                "linked run requires fresh launch authorization",
            ));
        };

        self.validate_launch_request(&request.launch)?;

        let parent_run_dir = journal::run_dir(session_dir, &request.parent_run_id);
        if !parent_run_dir.exists() {
            return Err(WorkflowError::NotFound(format!(
                "parent workflow run {} not found",
                request.parent_run_id.as_str()
            )));
        }

        // Snapshot parent durable bytes before any child work (immutability proof).
        let parent_meta_path = parent_run_dir.join("run.json");
        let parent_journal_path = parent_run_dir.join("journal.jsonl");
        let parent_meta_before = std::fs::read(&parent_meta_path)
            .map_err(|e| WorkflowError::Journal(format!("read parent run.json: {e}")))?;
        let parent_journal_before = std::fs::read(&parent_journal_path)
            .map_err(|e| WorkflowError::Journal(format!("read parent journal: {e}")))?;

        let parent_meta = journal::read_run_metadata(&parent_run_dir)?;
        if parent_meta.run_id != request.parent_run_id {
            return Err(WorkflowError::coded(
                super::error::WorkflowErrorCode::LineageMismatch,
                "parent run.json id does not match request parent_run_id",
            ));
        }

        let verified = if parent_meta.journal_format_version == journal::JOURNAL_FORMAT_V1 {
            let records = journal::read_journal(&parent_journal_path)?;
            lineage::extract_verified_prefix_v1(
                &parent_meta,
                &records,
                request.checkpoint.as_ref(),
                &request.link_reason,
            )?
        } else {
            let envelopes =
                journal::collect_journal_v2(&parent_journal_path, Some(&parent_meta.run_id))?;
            lineage::extract_verified_prefix_v2(
                &parent_meta,
                &parent_run_dir,
                &envelopes,
                request.checkpoint.as_ref(),
                &request.link_reason,
            )?
        };

        // Parent bytes must still match the pre-import snapshot (no parent mutation).
        let parent_meta_after = std::fs::read(&parent_meta_path)
            .map_err(|e| WorkflowError::Journal(format!("re-read parent run.json: {e}")))?;
        let parent_journal_after = std::fs::read(&parent_journal_path)
            .map_err(|e| WorkflowError::Journal(format!("re-read parent journal: {e}")))?;
        if parent_meta_before != parent_meta_after || parent_journal_before != parent_journal_after
        {
            return Err(WorkflowError::coded(
                super::error::WorkflowErrorCode::LineageMismatch,
                "parent run mutated during linked import",
            ));
        }

        // Optionally verify in-memory parent state is unchanged when loaded.
        if let Ok(parent_state) = self.run_state(&request.parent_run_id).await {
            let guard = parent_state.lock().await;
            let _parent_state_snapshot = (guard.state, guard.metadata.clone());
            drop(guard);
            // no writes to parent_state
            let _ = _parent_state_snapshot;
        }

        let mut launch = request.launch;
        launch.parent_run_id = Some(request.parent_run_id.clone());

        let (run_id, run_dir) = loop {
            let run_id = WorkflowId::generate();
            let run_dir = journal::run_dir(session_dir, &run_id);
            if !run_dir.exists() {
                break (run_id, run_dir);
            }
        };
        let metadata = metadata_for_request(run_id.clone(), launch);

        let storage_reservation = self
            .admission
            .try_reserve_storage(run_id.as_str(), self.limits.run_storage_reservation_bytes())?;
        storage_reservation.commit();

        let seed_entry_count = verified.seed_invocations.len();
        let inherited_usage = verified.inherited_usage;
        let replay_entries: Vec<ReplayEntry> = verified
            .seed_invocations
            .iter()
            .map(|seed| ReplayEntry {
                invocation_id: seed.invocation_id.clone(),
                call_index: seed.call_index,
                kind: seed.kind,
                canonical_input_hash: seed.canonical_input_hash.clone(),
                outcome: seed.outcome.clone(),
            })
            .collect();

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
            writer.append(&created, &self.limits)?;

            let seed_envelope = JournalEnvelope::new(
                writer.next_seq(),
                timestamp_ms,
                run_id.clone(),
                JournalPayload::LineageSeedImported {
                    lineage: verified.lineage.clone(),
                    prefix_digest: Some(verified.checkpoint.prefix_digest.clone()),
                },
            );
            writer.append(&seed_envelope, &self.limits)?;

            for seed in &verified.seed_invocations {
                let mut started = JournalEnvelope::new(
                    writer.next_seq(),
                    timestamp_ms,
                    run_id.clone(),
                    JournalPayload::InvocationStarted {
                        invocation_id: seed.invocation_id.clone(),
                        call_index: seed.call_index,
                        kind: seed.kind,
                        canonical_input: seed.canonical_input.clone(),
                    },
                );
                started = started.with_canonical_input_hash(seed.canonical_input_hash.clone());
                writer.append(&started, &self.limits)?;

                let finished = JournalEnvelope::new(
                    writer.next_seq(),
                    timestamp_ms,
                    run_id.clone(),
                    JournalPayload::InvocationFinished {
                        invocation_id: seed.invocation_id.clone(),
                        outcome: seed.outcome.clone(),
                    },
                );
                writer.append(&finished, &self.limits)?;
            }

            let mut store = ArtifactStore::open(&run_dir, run_id.clone())?;
            for artifact in &verified.artifacts {
                let staged = lineage::import_seed_artifact(&store, &self.limits, artifact)?;
                let commit = JournalEnvelope::new(
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
                );
                writer.append(&commit, &self.limits)?;
                store.mark_committed(staged.metadata())?;
            }

            let sequence = writer.next_seq().saturating_sub(1);
            Ok::<_, WorkflowError>((writer, store, sequence, timestamp_ms))
        })();

        let (writer, artifacts, projection_sequence, started_at_ms) = match durable_create {
            Ok(durable) => durable,
            Err(error) => {
                self.admission.release_storage_owner(run_id.as_str());
                let _ = std::fs::remove_dir_all(&run_dir);
                return Err(error);
            }
        };

        // Final parent immutability check after child durable write.
        let parent_meta_final = std::fs::read(&parent_meta_path).unwrap_or_default();
        let parent_journal_final = std::fs::read(&parent_journal_path).unwrap_or_default();
        if parent_meta_before != parent_meta_final || parent_journal_before != parent_journal_final
        {
            self.admission.release_storage_owner(run_id.as_str());
            let _ = std::fs::remove_dir_all(&run_dir);
            return Err(WorkflowError::coded(
                super::error::WorkflowErrorCode::LineageMismatch,
                "parent run mutated during linked create",
            ));
        }

        if !authorization.commit() {
            self.admission.release_storage_owner(run_id.as_str());
            let _ = std::fs::remove_dir_all(&run_dir);
            return Err(WorkflowError::coded(
                super::error::WorkflowErrorCode::LaunchAuthorizationMismatch,
                "launch authorization was revoked or already consumed",
            ));
        }

        let control = Arc::new(RunControl::new());
        let reports = recovered_reports_v2(
            &journal::collect_journal_v2(&run_dir.join("journal.jsonl"), Some(&run_id))
                .unwrap_or_default(),
        );
        let state = Arc::new(Mutex::new(RunState {
            metadata,
            state: WorkflowState::Queued,
            current_phase: recovered_phase_v2(
                &journal::collect_journal_v2(&run_dir.join("journal.jsonl"), Some(&run_id))
                    .unwrap_or_default(),
            ),
            invocation_count: seed_entry_count as u64,
            failure_count: 0,
            actual_usage: None,
            inherited_usage,
            seed_entry_count,
            projection_sequence: Some(projection_sequence),
            started_at_ms: Some(started_at_ms),
            updated_at_ms: Some(started_at_ms),
            latest_log_summary: latest_log_summary(&replay_entries),
            latest_report_summary: latest_report_summary_v2(
                &journal::collect_journal_v2(&run_dir.join("journal.jsonl"), Some(&run_id))
                    .unwrap_or_default(),
            ),
            terminal_reason: None,
            reports,
            run_dir,
            control: Arc::clone(&control),
            worker_active: false,
            worker_join: None,
            worker_permit: None,
            current_invocation: None,
            replay_entries,
            replay_cursor: 0,
            replay_live: false,
            journal: Some(Arc::new(StdMutex::new(writer))),
            final_result: None,
            artifacts,
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
            let session_dir = match guard.run_dir.parent().and_then(Path::parent) {
                Some(session_dir) => session_dir.to_path_buf(),
                None => {
                    guard.worker_active = false;
                    guard.current_invocation = None;
                    self.release_worker_admission_locked(&mut guard);
                    return Err(WorkflowError::Host(
                        "workflow run directory has no session parent".to_owned(),
                    ));
                }
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
        Ok(WorkflowOutput {
            metadata: guard.metadata.clone(),
            state: guard.state,
            current_phase: guard.current_phase.clone(),
            invocations,
            failure_count: guard.failure_count,
            actual_usage: guard.actual_usage,
            inherited_usage: guard.inherited_usage,
            terminal_reason: guard.terminal_reason.clone(),
            reports: guard.reports.clone(),
            final_result,
            artifacts: guard.artifacts.list_metadata().to_vec(),
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
            if guard.v1_read_only {
                return Err(WorkflowError::InvalidOperation(
                    "v1 workflow projections are read-only".to_owned(),
                ));
            }
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
            if guard.v1_read_only {
                return Err(WorkflowError::InvalidOperation(
                    "v1 workflow projections are read-only".to_owned(),
                ));
            }
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
        invocation_id: &str,
        multi_agent: &MultiAgentRuntime,
        deps: ChildRuntimeDeps,
        agent_id: &AgentId,
        schema: &CompiledSchema,
        first_output: &ChildRunOutput,
    ) -> Result<ChildSchemaAcceptResult, WorkflowError> {
        let first_raw = child_final_assistant_text(first_output);
        let first_usage = accumulate_child_usage(None, &first_output.events);
        let first_source = StructuredOutputSource::AssistantText(first_raw.clone());
        match accept_structured_output(schema, first_source) {
            Ok(value) => {
                return Ok(ChildSchemaAcceptResult {
                    ok: true,
                    value: Some(value),
                    error_code: None,
                    summary: "child output matched schema".to_owned(),
                    repair_attempted: false,
                    repair_id: None,
                    first_raw,
                    repair_raw: None,
                    actual_usage: first_usage,
                });
            }
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
        let envelopes = journal::collect_journal_v2(&path, Some(&run))?;
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
        let state = self.run_state(run_id).await?;
        let guard = state.lock().await;
        guard.artifacts.get(artifact_id)
    }

    /// Read a byte range of a journal-visible artifact.
    pub async fn read_artifact_range(
        &self,
        run_id: &WorkflowId,
        artifact_id: &super::state::WorkflowArtifactId,
        offset: u64,
        max_bytes: u64,
    ) -> Result<super::artifacts::ArtifactContentRange, WorkflowError> {
        let state = self.run_state(run_id).await?;
        let guard = state.lock().await;
        guard.artifacts.read_range(artifact_id, offset, max_bytes)
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

        // Reconcile durable starts without finishes via the production
        // (or test-injected) read-only resolver. Never relaunches effects.
        if let Err(error) = self
            .reconcile_incomplete_invocations_v2(&mut writer, &metadata, &journal_path)
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
        if let Err(error) = self.reconcile_open_schema_repairs_v2(&mut writer, &metadata) {
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
                let within_seed = guard.replay_cursor < guard.seed_entry_count;
                if let Some(entry) = guard.replay_entries.get(guard.replay_cursor) {
                    if entry.call_index == call_index
                        && entry.kind == kind
                        && entry.canonical_input_hash == input_hash
                    {
                        let outcome = entry.outcome.clone();
                        guard.replay_cursor += 1;
                        return Ok(outcome);
                    }
                    // Seed prefix must match exactly before any new external effect.
                    if within_seed {
                        return Err(WorkflowError::coded(
                            super::error::WorkflowErrorCode::LineageMismatch,
                            format!(
                                "lineage seed mismatch at call_index {call_index}: expected hash {}, got {input_hash}",
                                entry.canonical_input_hash
                            ),
                        ));
                    }
                } else if within_seed {
                    return Err(WorkflowError::coded(
                        super::error::WorkflowErrorCode::LineageMismatch,
                        format!(
                            "lineage seed exhausted early at call_index {call_index} (seed_entry_count={})",
                            guard.seed_entry_count
                        ),
                    ));
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
            guard.current_invocation = None;
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
                guard.current_invocation = None;
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

    /// Append finishes for durable starts lacking finishes using the bound
    /// read-only resolver. Adopts exactly one proven terminal result; zero /
    /// conflicting / unknown results become interrupted(host_exit). Never
    /// dispatches or auto-retries external effects.
    async fn reconcile_incomplete_invocations_v2(
        &self,
        writer: &mut JournalV2Writer,
        metadata: &WorkflowRunMetadata,
        journal_path: &Path,
    ) -> Result<(), WorkflowError> {
        let envelopes = journal::collect_journal_v2(journal_path, Some(&metadata.run_id))?;
        let incomplete = find_incomplete_invocations_v2(&envelopes);
        if incomplete.is_empty() {
            return Ok(());
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
    fn reconcile_open_schema_repairs_v2(
        &self,
        writer: &mut JournalV2Writer,
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
        let artifacts = ArtifactStore::empty(run_id.clone(), &run_dir);
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
            inherited_usage: None,
            seed_entry_count: 0,
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
            artifacts,
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
        let mut artifacts = ArtifactStore::open(&run_dir, run_id.clone())
            .unwrap_or_else(|_| ArtifactStore::empty(run_id.clone(), &run_dir));
        // Best-effort rehydrate: corrupt/missing files stay invisible and typed on get.
        let _ = artifacts.rehydrate_from_envelopes(&envelopes);
        let run_state = RunState {
            current_phase: recovered_phase_v2(&envelopes),
            invocation_count: invocation_count_v2(&envelopes),
            failure_count: failure_count_v2(&envelopes),
            actual_usage: {
                let seed_ids = lineage::seed_invocation_ids_from_journal(&envelopes);
                if seed_ids.is_empty() {
                    aggregate_usage_v2(&envelopes)
                } else {
                    lineage::split_usage_for_seed(&envelopes, &seed_ids).1
                }
            },
            inherited_usage: {
                let seed_ids = lineage::seed_invocation_ids_from_journal(&envelopes);
                if seed_ids.is_empty() {
                    None
                } else {
                    lineage::split_usage_for_seed(&envelopes, &seed_ids).0
                }
            },
            seed_entry_count: lineage::seed_pair_count_from_journal(&envelopes),
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
            artifacts,
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
        let artifacts = ArtifactStore::empty(run_id.clone(), &run_dir);
        let state = RunState {
            metadata,
            state: WorkflowState::Failed,
            current_phase: None,
            invocation_count: 0,
            failure_count: 1,
            actual_usage: None,
            inherited_usage: None,
            seed_entry_count: 0,
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
            artifacts,
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

    pub async fn accept_child_structured_output_with_repair(
        &self,
        invocation_id: &str,
        multi_agent: &MultiAgentRuntime,
        deps: ChildRuntimeDeps,
        agent_id: &AgentId,
        schema: &CompiledSchema,
        first_output: &ChildRunOutput,
    ) -> Result<ChildSchemaAcceptResult, WorkflowError> {
        self.runtime
            .accept_child_structured_output_with_repair(
                &self.run_id,
                invocation_id,
                multi_agent,
                deps,
                agent_id,
                schema,
                first_output,
            )
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
