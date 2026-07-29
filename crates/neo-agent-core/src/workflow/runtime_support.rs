use std::collections::HashMap;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;

use tokio_util::sync::CancellationToken;

use super::super::error::WorkflowError;
use super::super::journal::{IncompleteInvocation, JournalEnvelope, JournalPayload};
use super::super::state::{
    WorkflowActor, WorkflowFinalResultMetadata, WorkflowInvocationKind, WorkflowInvocationOutcome,
    WorkflowOutcomeStatus, WorkflowState,
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
    pub(super) call_index: u64,
    pub(super) kind: WorkflowInvocationKind,
    pub(super) canonical_input_hash: String,
    pub(super) outcome: WorkflowInvocationOutcome,
}

pub(super) fn interrupted_outcome(invocation: &IncompleteInvocation) -> WorkflowInvocationOutcome {
    WorkflowInvocationOutcome {
        ok: false,
        status: WorkflowOutcomeStatus::Interrupted,
        summary: "interrupted by host exit".to_owned(),
        interruption: None,
        details: serde_json::json!({
            "reason": "host_exit",
            "call_index": invocation.call_index,
            "canonical_input_hash": invocation.canonical_input_hash,
            "side_effect_occurred": true,
        }),
        actual_usage: None,
        child_refs: Vec::new(),
    }
}

pub(super) fn bounded_resource_limited_outcome(
    reason: &str,
    original: &WorkflowInvocationOutcome,
) -> WorkflowInvocationOutcome {
    WorkflowInvocationOutcome {
        ok: false,
        status: WorkflowOutcomeStatus::ResourceLimited,
        summary: reason.to_owned(),
        interruption: None,
        details: serde_json::json!({"reason": reason}),
        actual_usage: original.actual_usage,
        child_refs: original.child_refs.clone(),
    }
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

pub(super) fn latest_log_summary(entries: &[ReplayEntry]) -> Option<String> {
    entries.iter().rev().find_map(|entry| {
        if entry.kind != WorkflowInvocationKind::Log || !entry.outcome.ok {
            return None;
        }
        entry
            .outcome
            .details
            .get("message")
            .and_then(serde_json::Value::as_str)
            .map(bounded_summary)
    })
}

pub(super) fn report_summary(value: &serde_json::Value) -> Option<String> {
    let text = value
        .as_str()
        .map(str::to_owned)
        .or_else(|| serde_json::to_string(value).ok())?;
    Some(bounded_summary(&text))
}

pub(super) fn bounded_summary(value: &str) -> String {
    const MAX_CHARS: usize = 160;
    value
        .chars()
        .map(|character| {
            if character.is_whitespace() {
                ' '
            } else {
                character
            }
        })
        .take(MAX_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

pub(super) fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

pub(super) fn replay_entries(envelopes: &[JournalEnvelope]) -> Vec<ReplayEntry> {
    let finished: HashMap<_, _> = envelopes
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            JournalPayload::InvocationFinished {
                invocation_id,
                outcome,
            } => Some((invocation_id.as_str(), outcome)),
            _ => None,
        })
        .collect();
    let mut replay = Vec::new();
    for entry in envelopes
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            JournalPayload::InvocationStarted {
                invocation_id,
                call_index,
                kind,
                ..
            } => {
                let hash = envelope.canonical_input_hash.clone().unwrap_or_default();
                finished
                    .get(invocation_id.as_str())
                    .map(|outcome| ReplayEntry {
                        call_index: *call_index,
                        kind: *kind,
                        canonical_input_hash: hash,
                        outcome: (*outcome).clone(),
                    })
            }
            _ => None,
        })
    {
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

pub(super) fn last_state(envelopes: &[JournalEnvelope]) -> (WorkflowState, Option<String>) {
    let mut state = WorkflowState::Queued;
    let mut reason: Option<String> = None;
    let mut saw_state = false;
    for envelope in envelopes {
        if let JournalPayload::StateChanged { new, reason: r, .. } = &envelope.payload {
            state = *new;
            reason = Some(r.clone());
            saw_state = true;
        }
    }
    if saw_state {
        (state, reason)
    } else if envelopes
        .iter()
        .any(|e| matches!(e.payload, JournalPayload::RunCreated { .. }))
    {
        (WorkflowState::Queued, Some("launch".to_owned()))
    } else {
        (
            WorkflowState::Failed,
            Some("missing workflow state".to_owned()),
        )
    }
}

pub(super) fn final_result(envelopes: &[JournalEnvelope]) -> Option<WorkflowFinalResultMetadata> {
    envelopes
        .iter()
        .rev()
        .find_map(|envelope| match &envelope.payload {
            JournalPayload::FinalResultRecorded { metadata } => Some(metadata.clone()),
            _ => None,
        })
}

pub(super) fn aggregate_usage(envelopes: &[JournalEnvelope]) -> Option<AgentTokenUsage> {
    envelopes
        .iter()
        .fold(None, |total, envelope| match &envelope.payload {
            JournalPayload::InvocationFinished {
                outcome:
                    WorkflowInvocationOutcome {
                        actual_usage: Some(usage),
                        ..
                    },
                ..
            } => Some(add_usage(total, *usage)),
            JournalPayload::UsageRecorded { usage, .. } => Some(add_usage(total, *usage)),
            _ => total,
        })
}

pub(super) fn recovered_phase(envelopes: &[JournalEnvelope]) -> Option<String> {
    envelopes
        .iter()
        .rev()
        .find_map(|envelope| match &envelope.payload {
            JournalPayload::InvocationFinished { outcome, .. } if outcome.ok => outcome
                .details
                .get("phase")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            _ => None,
        })
}

pub(super) fn recovered_reports(envelopes: &[JournalEnvelope]) -> Vec<serde_json::Value> {
    envelopes
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            JournalPayload::InvocationFinished { outcome, .. } if outcome.ok => {
                outcome.details.get("report").cloned()
            }
            _ => None,
        })
        .collect()
}

pub(super) fn latest_report_summary(envelopes: &[JournalEnvelope]) -> Option<String> {
    envelopes
        .iter()
        .rev()
        .find_map(|envelope| match &envelope.payload {
            JournalPayload::InvocationFinished { outcome, .. } if outcome.ok => {
                outcome.details.get("report").and_then(report_summary)
            }
            _ => None,
        })
}

pub(super) fn projection_timestamps(envelopes: &[JournalEnvelope]) -> (Option<u64>, Option<u64>) {
    (
        envelopes.first().map(|e| e.timestamp_ms),
        envelopes.last().map(|e| e.timestamp_ms),
    )
}

pub(super) fn invocation_count(envelopes: &[JournalEnvelope]) -> u64 {
    envelopes
        .iter()
        .filter(|e| matches!(e.payload, JournalPayload::InvocationStarted { .. }))
        .count()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub(super) fn failure_count(envelopes: &[JournalEnvelope]) -> u64 {
    envelopes
        .iter()
        .filter(|e| {
            matches!(
                &e.payload,
                JournalPayload::InvocationFinished { outcome, .. } if !outcome.ok
            )
        })
        .count()
        .try_into()
        .unwrap_or(u64::MAX)
}
