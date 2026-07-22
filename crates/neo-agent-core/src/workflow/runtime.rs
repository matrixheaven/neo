use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::error::WorkflowError;
use super::journal::{
    self, IncompleteInvocation, JournalRecord, JournalWriter, canonical_input_hash,
};
use super::limits::WorkflowLimits;
use super::state::{
    WorkflowActor, WorkflowId, WorkflowInvocationKind, WorkflowInvocationOutcome,
    WorkflowOutcomeStatus, WorkflowPhase, WorkflowRunMetadata, WorkflowState,
};
use crate::AgentTokenUsage;

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

#[derive(Debug, Clone)]
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
    pause_token: CancellationToken,
    stop_token: CancellationToken,
    pause_requested: bool,
    session_dir: PathBuf,
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

    fn output(&self) -> WorkflowOutput {
        WorkflowOutput {
            metadata: self.metadata.clone(),
            state: self.state,
            current_phase: self.current_phase.clone(),
            invocations: Vec::new(),
            failure_count: self.failure_count,
            actual_usage: self.actual_usage,
            terminal_reason: self.terminal_reason.clone(),
            reports: self.reports.clone(),
        }
    }
}

#[derive(Clone)]
pub struct WorkflowRuntime {
    runs: Arc<Mutex<HashMap<String, Arc<Mutex<RunState>>>>>,
    limits: WorkflowLimits,
}

impl WorkflowRuntime {
    #[must_use]
    pub fn new(limits: WorkflowLimits) -> Self {
        Self {
            runs: Arc::new(Mutex::new(HashMap::new())),
            limits,
        }
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

        let rdir = journal::run_dir(session_dir, &run_id);
        journal::write_run_metadata(&rdir, &metadata, &self.limits)?;

        let jpath = journal::journal_path(session_dir, &run_id);
        let mut writer = JournalWriter::open(&jpath)?;
        let initial_record = JournalRecord::StateChanged {
            seq: 0,
            timestamp_ms: current_timestamp_ms(),
            previous: WorkflowState::Running,
            new: WorkflowState::Running,
            reason: "launch".to_owned(),
            actor: WorkflowActor::Runtime,
        };
        writer.append(&initial_record, &self.limits)?;

        let pause_token = CancellationToken::new();
        let stop_token = CancellationToken::new();

        let run_state = RunState {
            metadata: metadata.clone(),
            state: WorkflowState::Running,
            current_phase: None,
            invocation_count: 0,
            failure_count: 0,
            actual_usage: None,
            terminal_reason: None,
            reports: Vec::new(),
            pause_token: pause_token.clone(),
            stop_token: stop_token.clone(),
            pause_requested: false,
            session_dir: session_dir.to_path_buf(),
        };

        let state = Arc::new(Mutex::new(run_state));
        self.runs
            .lock()
            .await
            .insert(run_id.0.clone(), Arc::clone(&state));

        Ok(WorkflowHandle {
            run_id,
            state,
            pause_token,
            stop_token,
            runtime: self.clone(),
        })
    }

    pub async fn snapshot(
        &self,
        run_id: &WorkflowId,
    ) -> Result<WorkflowRunSnapshot, WorkflowError> {
        let runs = self.runs.lock().await;
        let state = runs
            .get(&run_id.0)
            .ok_or_else(|| WorkflowError::NotFound(run_id.0.clone()))?;
        Ok(state.lock().await.snapshot())
    }

    pub async fn output(&self, run_id: &WorkflowId) -> Result<WorkflowOutput, WorkflowError> {
        let runs = self.runs.lock().await;
        let state = runs
            .get(&run_id.0)
            .ok_or_else(|| WorkflowError::NotFound(run_id.0.clone()))?;
        let guard = state.lock().await;
        let mut output = guard.output();
        let jpath = journal::journal_path(&guard.session_dir, run_id);
        if jpath.exists() {
            output.invocations = journal::read_journal(&jpath).unwrap_or_default();
        }
        Ok(output)
    }

    pub async fn pause(
        &self,
        run_id: &WorkflowId,
        _actor: WorkflowActor,
    ) -> Result<(), WorkflowError> {
        let runs = self.runs.lock().await;
        let state = runs
            .get(&run_id.0)
            .ok_or_else(|| WorkflowError::NotFound(run_id.0.clone()))?;
        let mut guard = state.lock().await;
        if guard.state.is_terminal() {
            return Err(WorkflowError::InvalidInput(
                "cannot pause a terminal workflow".to_owned(),
            ));
        }
        guard.pause_requested = true;
        guard.pause_token.cancel();
        Ok(())
    }

    pub async fn resume(
        &self,
        run_id: &WorkflowId,
        _actor: WorkflowActor,
    ) -> Result<(), WorkflowError> {
        let runs = self.runs.lock().await;
        let state = runs
            .get(&run_id.0)
            .ok_or_else(|| WorkflowError::NotFound(run_id.0.clone()))?;
        let mut guard = state.lock().await;
        if guard.state != WorkflowState::Paused {
            return Err(WorkflowError::InvalidInput(
                "can only resume a paused workflow".to_owned(),
            ));
        }
        guard.state = WorkflowState::Running;
        guard.pause_requested = false;
        guard.pause_token = CancellationToken::new();
        guard.stop_token = CancellationToken::new();

        let jpath = journal::journal_path(&guard.session_dir, run_id);
        let mut writer = JournalWriter::open(&jpath)?;
        let record = JournalRecord::StateChanged {
            seq: writer.next_seq(),
            timestamp_ms: current_timestamp_ms(),
            previous: WorkflowState::Paused,
            new: WorkflowState::Running,
            reason: "resume".to_owned(),
            actor: _actor,
        };
        writer.append(&record, &self.limits)?;
        Ok(())
    }

    pub async fn stop(
        &self,
        run_id: &WorkflowId,
        actor: WorkflowActor,
    ) -> Result<(), WorkflowError> {
        let runs = self.runs.lock().await;
        let state = runs
            .get(&run_id.0)
            .ok_or_else(|| WorkflowError::NotFound(run_id.0.clone()))?;
        let mut guard = state.lock().await;
        if guard.state.is_terminal() {
            return Err(WorkflowError::InvalidInput(
                "cannot stop a terminal workflow".to_owned(),
            ));
        }
        guard.stop_token.cancel();
        guard.state = WorkflowState::Cancelled;
        guard.terminal_reason = Some("stopped by user/model".to_owned());

        let jpath = journal::journal_path(&guard.session_dir, run_id);
        let mut writer = JournalWriter::open(&jpath)?;
        let record = JournalRecord::StateChanged {
            seq: writer.next_seq(),
            timestamp_ms: current_timestamp_ms(),
            previous: WorkflowState::Running,
            new: WorkflowState::Cancelled,
            reason: "stop".to_owned(),
            actor,
        };
        writer.append(&record, &self.limits)?;
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

        let mut handles = Vec::new();
        let entries =
            std::fs::read_dir(&workflows_dir).map_err(|e| WorkflowError::Journal(e.to_string()))?;

        for entry in entries {
            let entry = entry.map_err(|e| WorkflowError::Journal(e.to_string()))?;
            let rdir = entry.path();
            if !rdir.is_dir() {
                continue;
            }

            let metadata = match journal::read_run_metadata(&rdir) {
                Ok(m) => m,
                Err(_) => continue,
            };

            let run_id = metadata.run_id.clone();
            let jpath = rdir.join("journal.jsonl");
            let records = if jpath.exists() {
                journal::read_journal(&jpath).unwrap_or_default()
            } else {
                Vec::new()
            };

            let last_state = records
                .iter()
                .rev()
                .find_map(|r| match r {
                    JournalRecord::StateChanged { new, .. } => Some(*new),
                    _ => None,
                })
                .unwrap_or(WorkflowState::Running);

            let needs_pause = last_state == WorkflowState::Running;

            let incomplete = journal::find_incomplete_invocations(&records);
            let invocation_count = records
                .iter()
                .filter(|r| matches!(r, JournalRecord::InvocationStarted { .. }))
                .count() as u64;
            let failure_count = records
                .iter()
                .filter(|r| {
                    matches!(r, JournalRecord::InvocationFinished { outcome, .. } if !outcome.ok)
                })
                .count() as u64;

            let final_state = if needs_pause {
                WorkflowState::Paused
            } else {
                last_state
            };

            let terminal_reason = if needs_pause {
                Some("host_exit".to_owned())
            } else {
                None
            };

            if needs_pause {
                let mut writer = JournalWriter::open(&jpath)?;
                for inv in &incomplete {
                    let finish = JournalRecord::InvocationFinished {
                        seq: writer.next_seq(),
                        timestamp_ms: current_timestamp_ms(),
                        invocation_id: inv.invocation_id.clone(),
                        outcome: WorkflowInvocationOutcome {
                            ok: false,
                            status: WorkflowOutcomeStatus::Interrupted,
                            summary: "interrupted by host exit".to_owned(),
                            details: serde_json::json!({"reason": "host_exit"}),
                            actual_usage: None,
                            child_refs: vec![],
                        },
                    };
                    writer.append(&finish, &self.limits)?;
                }

                let pause_record = JournalRecord::StateChanged {
                    seq: writer.next_seq(),
                    timestamp_ms: current_timestamp_ms(),
                    previous: WorkflowState::Running,
                    new: WorkflowState::Paused,
                    reason: "host_exit".to_owned(),
                    actor: WorkflowActor::Runtime,
                };
                writer.append(&pause_record, &self.limits)?;
            }

            let pause_token = CancellationToken::new();
            let stop_token = CancellationToken::new();

            let run_state = RunState {
                metadata: metadata.clone(),
                state: final_state,
                current_phase: None,
                invocation_count,
                failure_count,
                actual_usage: None,
                terminal_reason,
                reports: Vec::new(),
                pause_token: pause_token.clone(),
                stop_token: stop_token.clone(),
                pause_requested: false,
                session_dir: session_dir.to_path_buf(),
            };

            let state = Arc::new(Mutex::new(run_state));
            self.runs
                .lock()
                .await
                .insert(run_id.0.clone(), Arc::clone(&state));

            handles.push(WorkflowHandle {
                run_id,
                state,
                pause_token,
                stop_token,
                runtime: self.clone(),
            });
        }

        Ok(handles)
    }

    pub async fn transition_state(
        &self,
        run_id: &WorkflowId,
        new_state: WorkflowState,
        reason: &str,
        actor: WorkflowActor,
    ) -> Result<(), WorkflowError> {
        let runs = self.runs.lock().await;
        let state = runs
            .get(&run_id.0)
            .ok_or_else(|| WorkflowError::NotFound(run_id.0.clone()))?;
        let mut guard = state.lock().await;
        let previous = guard.state;
        guard.state = new_state;
        if new_state.is_terminal() {
            guard.terminal_reason = Some(reason.to_owned());
        }

        let jpath = journal::journal_path(&guard.session_dir, run_id);
        let mut writer = JournalWriter::open(&jpath)?;
        let record = JournalRecord::StateChanged {
            seq: writer.next_seq(),
            timestamp_ms: current_timestamp_ms(),
            previous,
            new: new_state,
            reason: reason.to_owned(),
            actor,
        };
        writer.append(&record, &self.limits)?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct WorkflowHandle {
    pub run_id: WorkflowId,
    state: Arc<Mutex<RunState>>,
    pause_token: CancellationToken,
    stop_token: CancellationToken,
    runtime: WorkflowRuntime,
}

impl WorkflowHandle {
    pub async fn snapshot(&self) -> WorkflowRunSnapshot {
        self.state.lock().await.snapshot()
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

    #[must_use]
    pub fn is_pause_requested(&self) -> bool {
        self.pause_token.is_cancelled()
    }

    #[must_use]
    pub fn is_stop_requested(&self) -> bool {
        self.stop_token.is_cancelled()
    }

    #[must_use]
    pub fn pause_token(&self) -> &CancellationToken {
        &self.pause_token
    }

    #[must_use]
    pub fn stop_token(&self) -> &CancellationToken {
        &self.stop_token
    }
}

pub struct ReplayPrefix {
    pub matched_records: Vec<JournalRecord>,
    pub first_live_call_index: u64,
    pub incomplete: Vec<IncompleteInvocation>,
}

pub fn compute_replay_prefix(
    records: &[JournalRecord],
    new_calls: &[(u64, WorkflowInvocationKind, serde_json::Value)],
) -> ReplayPrefix {
    let mut matched = Vec::new();
    let mut first_live = 0u64;

    let started: Vec<_> = records
        .iter()
        .filter_map(|r| match r {
            JournalRecord::InvocationStarted {
                call_index,
                kind,
                canonical_input_hash,
                ..
            } => Some((*call_index, *kind, canonical_input_hash.clone())),
            _ => None,
        })
        .collect();

    for (i, (call_index, kind, input)) in new_calls.iter().enumerate() {
        let hash = canonical_input_hash(input);
        if i < started.len() {
            let (s_idx, s_kind, s_hash) = &started[i];
            if s_idx == call_index && s_kind == kind && *s_hash == hash {
                if let Some(record) = records.iter().find(|r| {
                    matches!(r, JournalRecord::InvocationStarted { call_index: ci, .. } if ci == call_index)
                }) {
                    matched.push(record.clone());
                }
                if let Some(finish) = records.iter().find(|r| {
                    matches!(r, JournalRecord::InvocationFinished { invocation_id, .. }
                        if records.iter().any(|s| matches!(s,
                            JournalRecord::InvocationStarted { invocation_id: sid, call_index: ci, .. }
                            if sid == invocation_id && ci == call_index)))
                }) {
                    matched.push(finish.clone());
                }
                first_live = call_index + 1;
                continue;
            }
        }
        break;
    }

    let incomplete = journal::find_incomplete_invocations(records);
    ReplayPrefix {
        matched_records: matched,
        first_live_call_index: first_live,
        incomplete,
    }
}

fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
