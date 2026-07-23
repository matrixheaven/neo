use std::collections::HashMap;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;

use tokio_util::sync::CancellationToken;

use super::super::error::WorkflowError;
use super::super::journal::{self, IncompleteInvocation, JournalRecord, canonical_input_hash};
use super::super::state::{
    WorkflowActor, WorkflowInvocationKind, WorkflowInvocationOutcome, WorkflowOutcomeStatus,
    WorkflowState,
};
use crate::AgentTokenUsage;

pub(super) struct RunControl {
    pub(super) pause_requested: AtomicBool,
    pub(super) stop_token: CancellationToken,
    pause_actor: RwLock<Option<WorkflowActor>>,
    stop_actor: RwLock<Option<WorkflowActor>>,
}

impl RunControl {
    pub(super) fn new() -> Self {
        Self {
            pause_requested: AtomicBool::new(false),
            stop_token: CancellationToken::new(),
            pause_actor: RwLock::new(None),
            stop_actor: RwLock::new(None),
        }
    }

    pub(super) fn request_pause(&self, actor: WorkflowActor) -> Result<(), WorkflowError> {
        self.pause_requested
            .store(true, std::sync::atomic::Ordering::Release);
        let mut requester = self
            .pause_actor
            .write()
            .map_err(|_| WorkflowError::Host("workflow pause actor lock poisoned".to_owned()))?;
        if requester.is_none() {
            *requester = Some(actor);
        }
        Ok(())
    }

    pub(super) fn clear_pause(&self) -> Result<(), WorkflowError> {
        self.pause_requested
            .store(false, std::sync::atomic::Ordering::Release);
        *self
            .pause_actor
            .write()
            .map_err(|_| WorkflowError::Host("workflow pause actor lock poisoned".to_owned()))? =
            None;
        Ok(())
    }

    pub(super) fn pause_actor(&self) -> Result<WorkflowActor, WorkflowError> {
        self.pause_actor
            .read()
            .map(|actor| actor.unwrap_or(WorkflowActor::Runtime))
            .map_err(|_| WorkflowError::Host("workflow pause actor lock poisoned".to_owned()))
    }

    pub(super) fn request_stop(&self, actor: WorkflowActor) -> Result<(), WorkflowError> {
        let mut requester = self
            .stop_actor
            .write()
            .map_err(|_| WorkflowError::Host("workflow stop actor lock poisoned".to_owned()))?;
        if requester.is_none() {
            *requester = Some(actor);
        }
        self.stop_token.cancel();
        Ok(())
    }

    pub(super) fn stop_actor(&self) -> Result<WorkflowActor, WorkflowError> {
        self.stop_actor
            .read()
            .map(|actor| actor.unwrap_or(WorkflowActor::Runtime))
            .map_err(|_| WorkflowError::Host("workflow stop actor lock poisoned".to_owned()))
    }
}

#[derive(Clone)]
pub(super) struct ReplayEntry {
    pub(super) invocation_id: String,
    pub(super) call_index: u64,
    pub(super) kind: WorkflowInvocationKind,
    pub(super) canonical_input_hash: String,
    pub(super) outcome: WorkflowInvocationOutcome,
}

pub struct ReplayPrefix {
    pub matched_records: Vec<JournalRecord>,
    pub first_live_call_index: u64,
    pub incomplete: Vec<IncompleteInvocation>,
}

#[must_use]
pub fn compute_replay_prefix(
    records: &[JournalRecord],
    new_calls: &[(u64, WorkflowInvocationKind, serde_json::Value)],
) -> ReplayPrefix {
    let entries = replay_entries(records);
    let mut matched_records = Vec::new();
    let mut first_live_call_index = 0;
    for (entry, (call_index, kind, input)) in entries.iter().zip(new_calls) {
        if entry.call_index != *call_index
            || entry.kind != *kind
            || entry.canonical_input_hash != canonical_input_hash(input)
        {
            break;
        }
        matched_records.extend(
            records
                .iter()
                .filter(|record| match record {
                    JournalRecord::InvocationStarted { invocation_id, .. }
                    | JournalRecord::InvocationFinished { invocation_id, .. } => {
                        invocation_id == &entry.invocation_id
                    }
                    JournalRecord::StateChanged { .. } => false,
                })
                .cloned(),
        );
        first_live_call_index = call_index.saturating_add(1);
    }
    ReplayPrefix {
        matched_records,
        first_live_call_index,
        incomplete: journal::find_incomplete_invocations(records),
    }
}

pub(super) fn replay_entries(records: &[JournalRecord]) -> Vec<ReplayEntry> {
    let finished: HashMap<_, _> = records
        .iter()
        .filter_map(|record| match record {
            JournalRecord::InvocationFinished {
                invocation_id,
                outcome,
                ..
            } => Some((invocation_id.as_str(), outcome)),
            _ => None,
        })
        .collect();
    let mut replay = Vec::new();
    for entry in records.iter().filter_map(|record| match record {
        JournalRecord::InvocationStarted {
            invocation_id,
            call_index,
            kind,
            canonical_input_hash,
            ..
        } => finished
            .get(invocation_id.as_str())
            .map(|outcome| ReplayEntry {
                invocation_id: invocation_id.clone(),
                call_index: *call_index,
                kind: *kind,
                canonical_input_hash: canonical_input_hash.clone(),
                outcome: (*outcome).clone(),
            }),
        _ => None,
    }) {
        let Ok(index) = usize::try_from(entry.call_index) else {
            continue;
        };
        if index > replay.len() {
            continue;
        }
        replay.truncate(index);
        replay.push(entry);
    }
    replay
}

pub(super) fn last_state(records: &[JournalRecord]) -> (WorkflowState, Option<String>) {
    records
        .iter()
        .rev()
        .find_map(|record| match record {
            JournalRecord::StateChanged { new, reason, .. } => Some((*new, Some(reason.clone()))),
            _ => None,
        })
        .unwrap_or((
            WorkflowState::Failed,
            Some("missing workflow state".to_owned()),
        ))
}

pub(super) fn interrupted_outcome(invocation: &IncompleteInvocation) -> WorkflowInvocationOutcome {
    WorkflowInvocationOutcome {
        ok: false,
        status: WorkflowOutcomeStatus::Interrupted,
        summary: "interrupted by host exit".to_owned(),
        interruption: None,
        details: serde_json::json!({"reason": "host_exit", "call_index": invocation.call_index}),
        actual_usage: None,
        child_refs: Vec::new(),
    }
}

pub(super) fn resource_limited_outcome(reason: &str) -> WorkflowInvocationOutcome {
    WorkflowInvocationOutcome {
        ok: false,
        status: WorkflowOutcomeStatus::ResourceLimited,
        summary: reason.to_owned(),
        interruption: None,
        details: serde_json::json!({"reason": reason}),
        actual_usage: None,
        child_refs: Vec::new(),
    }
}

pub(super) fn aggregate_usage(records: &[JournalRecord]) -> Option<AgentTokenUsage> {
    records.iter().fold(None, |total, record| match record {
        JournalRecord::InvocationFinished {
            outcome:
                WorkflowInvocationOutcome {
                    actual_usage: Some(usage),
                    ..
                },
            ..
        } => Some(add_usage(total, *usage)),
        _ => total,
    })
}

pub(super) fn add_usage(total: Option<AgentTokenUsage>, usage: AgentTokenUsage) -> AgentTokenUsage {
    let total = total.unwrap_or(AgentTokenUsage {
        input_tokens: 0,
        output_tokens: 0,
        input_cache_read_tokens: 0,
        input_cache_write_tokens: 0,
    });
    total.saturating_add(usage)
}

pub(super) fn usage_total(usage: Option<AgentTokenUsage>) -> u64 {
    usage.map_or(0, |usage| {
        u64::from(usage.input_tokens) + u64::from(usage.output_tokens)
    })
}

pub(super) fn recovered_phase(records: &[JournalRecord]) -> Option<String> {
    records.iter().rev().find_map(|record| match record {
        JournalRecord::InvocationFinished { outcome, .. } if outcome.ok => outcome
            .details
            .get("phase")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        _ => None,
    })
}

pub(super) fn recovered_reports(records: &[JournalRecord]) -> Vec<serde_json::Value> {
    records
        .iter()
        .filter_map(|record| match record {
            JournalRecord::InvocationFinished { outcome, .. } if outcome.ok => {
                outcome.details.get("report").cloned()
            }
            _ => None,
        })
        .collect()
}

pub(super) fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}
