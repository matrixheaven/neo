use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
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
    WorkflowRunMetadata, WorkflowState,
};
use crate::AgentTokenUsage;

#[path = "runtime_support.rs"]
mod support;
use support::{
    ReplayEntry, RunControl, add_usage, aggregate_usage, current_timestamp_ms, interrupted_outcome,
    last_state, recovered_phase, recovered_reports, replay_entries, resource_limited_outcome,
    usage_total,
};
pub use support::{ReplayPrefix, compute_replay_prefix};

type RunnerFuture = Pin<Box<dyn Future<Output = Result<(), WorkflowError>> + Send>>;
type Runner = dyn Fn(WorkflowHandle, WorkflowRunMetadata) -> RunnerFuture + Send + Sync;
type RecoveryFuture = Pin<Box<dyn Future<Output = Option<WorkflowInvocationOutcome>> + Send>>;
type RecoveryResolver = dyn Fn(Arc<IncompleteInvocation>) -> RecoveryFuture + Send + Sync;

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

#[derive(Debug, Clone)]
pub struct WorkflowRunSnapshot {
    pub run_id: WorkflowId,
    pub name: String,
    pub state: WorkflowState,
    pub current_phase: Option<String>,
    pub invocation_count: u64,
    pub failure_count: u64,
    pub actual_usage: Option<AgentTokenUsage>,
    pub terminal_reason: Option<String>,
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
    terminal_reason: Option<String>,
    reports: Vec<serde_json::Value>,
    run_dir: PathBuf,
    control: Arc<RunControl>,
    worker_active: bool,
    current_invocation: Option<String>,
    replay_entries: Vec<ReplayEntry>,
    replay_cursor: usize,
    replay_live: bool,
    journal_error: Option<String>,
}

impl RunState {
    fn snapshot(&self) -> WorkflowRunSnapshot {
        WorkflowRunSnapshot {
            run_id: self.metadata.run_id.clone(),
            name: self.metadata.name.clone(),
            state: self.state,
            current_phase: self.current_phase.clone(),
            invocation_count: self.invocation_count,
            failure_count: self.failure_count,
            actual_usage: self.actual_usage,
            terminal_reason: self.terminal_reason.clone(),
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
    runner: Arc<RwLock<Option<Arc<Runner>>>>,
    recovery_resolver: Arc<RwLock<Option<Arc<RecoveryResolver>>>>,
}

impl WorkflowRuntime {
    #[must_use]
    pub fn new(limits: WorkflowLimits) -> Self {
        Self {
            runs: Arc::new(Mutex::new(HashMap::new())),
            limits,
            runner: Arc::new(RwLock::new(None)),
            recovery_resolver: Arc::new(RwLock::new(None)),
        }
    }

    /// Bind the production worker supplied by the Lua/dispatch composition root.
    pub fn bind_runner<F, Fut>(&self, runner: F) -> Result<(), WorkflowError>
    where
        F: Fn(WorkflowHandle, WorkflowRunMetadata) -> Fut + Send + Sync + 'static,
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
        *slot = Some(Arc::new(move |handle, metadata| {
            Box::pin(runner(handle, metadata))
        }));
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
        if request.script.len() as u64 > self.limits.lua_source_bytes {
            return Err(WorkflowError::InvalidInput(format!(
                "script size {} exceeds limit {}",
                request.script.len(),
                self.limits.lua_source_bytes
            )));
        }

        let run_id = WorkflowId(format!("wf_{}", uuid::Uuid::new_v4().as_simple()));
        let script_sha256 = {
            use sha2::{Digest, Sha256};
            format!("{:x}", Sha256::digest(request.script.as_bytes()))
        };
        let metadata = WorkflowRunMetadata {
            run_id: run_id.clone(),
            parent_run_id: request.parent_run_id,
            name: request.name,
            description: request.description,
            phases: request.phases,
            script: request.script,
            script_sha256,
            args: request.args,
            launch_source: request.launch_source,
            journal_format_version: 1,
        };

        let run_dir = journal::run_dir(session_dir, &run_id);
        journal::write_run_metadata(&run_dir, &metadata, &self.limits)?;
        let mut writer = JournalWriter::open(&run_dir.join("journal.jsonl"))?;
        writer.append(
            &JournalRecord::StateChanged {
                seq: writer.next_seq(),
                timestamp_ms: current_timestamp_ms(),
                previous: WorkflowState::Running,
                new: WorkflowState::Running,
                reason: "launch".to_owned(),
                actor: WorkflowActor::Runtime,
            },
            &self.limits,
        )?;

        let control = Arc::new(RunControl::new());
        let state = Arc::new(Mutex::new(RunState {
            metadata,
            state: WorkflowState::Running,
            current_phase: None,
            invocation_count: 0,
            failure_count: 0,
            actual_usage: None,
            terminal_reason: None,
            reports: Vec::new(),
            run_dir,
            control: Arc::clone(&control),
            worker_active: false,
            current_invocation: None,
            replay_entries: Vec::new(),
            replay_cursor: 0,
            replay_live: false,
            journal_error: None,
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
        if self.bound_runner()?.is_some() {
            self.start_worker(&run_id).await?;
        }
        Ok(handle)
    }

    pub async fn start_worker(&self, run_id: &WorkflowId) -> Result<(), WorkflowError> {
        let runner = self.bound_runner()?.ok_or_else(|| {
            WorkflowError::InvalidInput("workflow runner is not bound".to_owned())
        })?;
        let state = self.run_state(run_id).await?;
        let (handle, metadata) = {
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
            (
                WorkflowHandle {
                    run_id: run_id.clone(),
                    control: Arc::clone(&guard.control),
                    runtime: self.clone(),
                },
                guard.metadata.clone(),
            )
        };
        let runtime = self.clone();
        let id = run_id.clone();
        tokio::spawn(async move {
            let result = runner(handle, metadata).await;
            let _ = runtime.finish_worker(&id, result).await;
        });
        Ok(())
    }

    pub async fn snapshot(
        &self,
        run_id: &WorkflowId,
    ) -> Result<WorkflowRunSnapshot, WorkflowError> {
        Ok(self.run_state(run_id).await?.lock().await.snapshot())
    }

    pub async fn output(&self, run_id: &WorkflowId) -> Result<WorkflowOutput, WorkflowError> {
        let state = self.run_state(run_id).await?;
        let guard = state.lock().await;
        let invocations = if guard.journal_error.is_some() {
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
            let run_dir = entry.path();
            if !run_dir.is_dir() {
                continue;
            }
            let fallback_id = WorkflowId(entry.file_name().to_string_lossy().into_owned());
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
                    continue;
                }
                Err(error) => {
                    handles.push(
                        self.insert_corrupt_run(run_dir, fallback_id, error.to_string())
                            .await,
                    );
                    continue;
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
                    continue;
                }
                Err(error) => {
                    handles.push(
                        self.insert_failed_run(
                            run_dir,
                            metadata,
                            format!("corrupt journal: {error}"),
                        )
                        .await,
                    );
                    continue;
                }
            };

            let incomplete = journal::find_incomplete_invocations(&records);
            if !incomplete.is_empty() {
                let resolver = self.bound_recovery_resolver()?;
                let mut writer = JournalWriter::open(&journal_path)?;
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
                let mut writer = JournalWriter::open(&journal_path)?;
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
            let terminal_reason = if final_state == WorkflowState::Paused {
                Some("host_exit".to_owned())
            } else if final_state.is_terminal() {
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
                )
                .await,
            );
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
            let mut writer = JournalWriter::open(&guard.journal_path())?;
            let started = JournalRecord::InvocationStarted {
                seq: writer.next_seq(),
                timestamp_ms: current_timestamp_ms(),
                invocation_id: invocation_id.clone(),
                call_index,
                kind,
                canonical_input,
                canonical_input_hash: input_hash,
            };
            if let Err(error) = writer.append(&started, &self.limits) {
                if matches!(error, WorkflowError::JournalTotalLimitExceeded) {
                    self.transition_locked(
                        &mut guard,
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

        let mut guard = state.lock().await;
        let mut writer = JournalWriter::open(&guard.journal_path())?;
        writer.append(
            &JournalRecord::InvocationFinished {
                seq: writer.next_seq(),
                timestamp_ms: current_timestamp_ms(),
                invocation_id,
                outcome: outcome.clone(),
            },
            &self.limits,
        )?;
        guard.current_invocation = None;
        observe_outcome(&mut guard, kind, &outcome);

        if capped {
            self.transition_locked(
                &mut guard,
                WorkflowState::ResourceLimited,
                "workflow actual token cap reached",
                WorkflowActor::Runtime,
            )?;
        } else if guard.control.stop_token.is_cancelled() {
            let stop_actor = guard.control.stop_actor()?;
            self.transition_locked(
                &mut guard,
                WorkflowState::Cancelled,
                "stopped by user/model",
                stop_actor,
            )?;
        } else if outcome.interruption
            == Some(super::WorkflowInterruptionReason::InstructionReplanRequired)
        {
            self.transition_locked(
                &mut guard,
                WorkflowState::Paused,
                "instruction_replan_required",
                WorkflowActor::Runtime,
            )?;
        }
        Ok(outcome)
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
        if guard.control.stop_token.is_cancelled() {
            let stop_actor = guard.control.stop_actor()?;
            return self.transition_locked(
                &mut guard,
                WorkflowState::Cancelled,
                "stopped by user/model",
                stop_actor,
            );
        }
        if guard.control.pause_requested.load(Ordering::Acquire) {
            let pause_actor = guard.control.pause_actor()?;
            return self.transition_locked(&mut guard, WorkflowState::Paused, "pause", pause_actor);
        }
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
            Err(error) => self.transition_locked(
                &mut guard,
                WorkflowState::Failed,
                &error.to_string(),
                WorkflowActor::Runtime,
            ),
        }
    }

    fn transition_locked(
        &self,
        state: &mut RunState,
        new_state: WorkflowState,
        reason: &str,
        actor: WorkflowActor,
    ) -> Result<(), WorkflowError> {
        if new_state.is_terminal()
            && !journal::find_incomplete_invocations(&journal::read_journal(&state.journal_path())?)
                .is_empty()
        {
            return Err(WorkflowError::InvalidInput(
                "cannot terminalize workflow with an incomplete invocation".to_owned(),
            ));
        }
        let previous = state.state;
        if previous == new_state {
            return Ok(());
        }
        let mut writer = JournalWriter::open(&state.journal_path())?;
        writer.append(
            &JournalRecord::StateChanged {
                seq: writer.next_seq(),
                timestamp_ms: current_timestamp_ms(),
                previous,
                new: new_state,
                reason: reason.to_owned(),
                actor,
            },
            &self.limits,
        )?;
        state.state = new_state;
        if new_state.is_terminal() || new_state == WorkflowState::Paused {
            state.terminal_reason = Some(reason.to_owned());
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

    async fn insert_rehydrated_run(
        &self,
        run_dir: PathBuf,
        metadata: WorkflowRunMetadata,
        records: Vec<JournalRecord>,
        state: WorkflowState,
        terminal_reason: Option<String>,
    ) -> WorkflowHandle {
        let replay_entries = replay_entries(&records);
        let control = Arc::new(RunControl::new());
        let run_id = metadata.run_id.clone();
        let run_state = RunState {
            current_phase: recovered_phase(&records),
            invocation_count: records
                .iter()
                .filter(|record| matches!(record, JournalRecord::InvocationStarted { .. }))
                .count() as u64,
            failure_count: records
                .iter()
                .filter(|record| matches!(record, JournalRecord::InvocationFinished { outcome, .. } if !outcome.ok))
                .count() as u64,
            actual_usage: aggregate_usage(&records),
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
            journal_error: None,
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
            terminal_reason: Some(reason.clone()),
            reports: Vec::new(),
            run_dir,
            control: Arc::clone(&control),
            worker_active: false,
            current_invocation: None,
            replay_entries: Vec::new(),
            replay_cursor: 0,
            replay_live: false,
            journal_error: Some(reason),
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
    pub async fn snapshot(&self) -> WorkflowRunSnapshot {
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
        WorkflowInvocationKind::Phase if outcome.ok => {
            state.current_phase = outcome
                .details
                .get("phase")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
        }
        WorkflowInvocationKind::Report if outcome.ok => {
            if let Some(report) = outcome.details.get("report") {
                state.reports.push(report.clone());
            }
        }
        _ => {}
    }
}
