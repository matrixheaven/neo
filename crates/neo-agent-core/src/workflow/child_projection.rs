//! Durable child rows projected from the canonical workflow journal.

use super::error::WorkflowError;
use super::journal::journal_scan::scan_journal_page;
use super::journal::{
    JournalEnvelope, JournalPayload, WorkflowChildKey, WorkflowChildKind, collect_journal,
};
use super::state::{WorkflowId, WorkflowOutcomeStatus};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowChildState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
    Recovering,
}

impl WorkflowChildState {
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowChildRow {
    pub key: WorkflowChildKey,
    pub child_kind: WorkflowChildKind,
    pub phase_id: Option<String>,
    pub agent_id: Option<String>,
    pub state: WorkflowChildState,
    pub title: Option<String>,
    pub role: Option<String>,
    pub queued_at_ms: Option<u64>,
    pub started_at_ms: Option<u64>,
    pub updated_at_ms: u64,
    pub terminal_at_ms: Option<u64>,
    pub terminal_summary: Option<String>,
    pub error_summary: Option<String>,
    pub actual_usage: Option<serde_json::Value>,
    pub latest_activity: Option<String>,
    pub generated_files: Vec<String>,
}

#[derive(Debug, Default)]
pub struct ChildProjection {
    pub rows: Vec<WorkflowChildRow>,
    pub duplicate_keys: Vec<WorkflowChildKey>,
}

/// Walk validated journal entries in bounded batches without retaining the journal.
pub(crate) fn for_each_journal_envelope(
    journal_path: &Path,
    expected_run_id: Option<&WorkflowId>,
    max_record_bytes: u64,
    max_total_bytes: u64,
    mut apply: impl FnMut(JournalEnvelope) -> Result<(), WorkflowError>,
) -> Result<(), WorkflowError> {
    const RECORDS_PER_BATCH: usize = 256;
    let mut from_seq = 0;
    loop {
        let page = scan_journal_page(
            journal_path,
            expected_run_id,
            max_record_bytes,
            max_total_bytes,
            from_seq,
            RECORDS_PER_BATCH,
            u64::MAX,
        )?;
        let mut last_seq = None;
        for envelope in page.envelopes {
            if last_seq == Some(envelope.seq) {
                continue;
            }
            last_seq = Some(envelope.seq);
            apply(envelope)?;
        }
        if !page.has_more {
            return Ok(());
        }
        from_seq = page.next_seq;
    }
}

/// Project queued, running, and terminal child facts without reading display text.
pub fn project_children(
    journal_path: &Path,
    expected_run_id: Option<&WorkflowId>,
    max_record_bytes: u64,
    max_total_bytes: u64,
) -> Result<ChildProjection, WorkflowError> {
    let envelopes = collect_journal(
        journal_path,
        expected_run_id,
        max_record_bytes,
        max_total_bytes,
    )?;
    let mut rows = BTreeMap::<WorkflowChildKey, WorkflowChildRow>::new();
    let mut duplicate_keys = Vec::new();
    let mut phase_id = None;

    for envelope in envelopes {
        match envelope.payload {
            JournalPayload::InvocationFinished {
                invocation_id: _,
                outcome,
            } => {
                if let Some(phase) = outcome
                    .details
                    .get("phase")
                    .and_then(serde_json::Value::as_str)
                {
                    phase_id = Some(phase.to_owned());
                }
            }
            JournalPayload::ChildQueued {
                child_key,
                child_kind,
                invocation_id: _,
                phase_id: child_phase,
                title,
                role,
            } => insert_row(
                &mut rows,
                &mut duplicate_keys,
                child_key,
                child_kind,
                child_phase.or_else(|| phase_id.clone()),
                title,
                role,
                envelope.timestamp_ms,
            ),
            JournalPayload::ChildStarted {
                child_key,
                agent_id,
            } => {
                if let Some(row) = rows.get_mut(&child_key) {
                    row.state = WorkflowChildState::Recovering;
                    row.agent_id = agent_id.or_else(|| row.agent_id.clone());
                    row.started_at_ms = Some(envelope.timestamp_ms);
                    row.updated_at_ms = envelope.timestamp_ms;
                }
            }
            JournalPayload::ChildFinished {
                child_key,
                agent_id,
                status,
                summary,
                actual_usage,
                error,
            } => {
                if let Some(row) = rows.get_mut(&child_key) {
                    if row.started_at_ms.is_none() && agent_id.is_some() {
                        row.agent_id = agent_id;
                    }
                    row.state = child_state(status);
                    row.terminal_at_ms = Some(envelope.timestamp_ms);
                    row.updated_at_ms = envelope.timestamp_ms;
                    row.terminal_summary = Some(summary);
                    row.error_summary = error;
                    row.actual_usage =
                        actual_usage.and_then(|usage| serde_json::to_value(usage).ok());
                }
            }
            _ => {}
        }
    }

    Ok(ChildProjection {
        rows: rows.into_values().collect(),
        duplicate_keys,
    })
}

fn insert_row(
    rows: &mut BTreeMap<WorkflowChildKey, WorkflowChildRow>,
    duplicate_keys: &mut Vec<WorkflowChildKey>,
    key: WorkflowChildKey,
    child_kind: WorkflowChildKind,
    phase_id: Option<String>,
    title: Option<String>,
    role: Option<String>,
    timestamp_ms: u64,
) {
    if rows.contains_key(&key) {
        duplicate_keys.push(key);
        return;
    }
    rows.insert(
        key.clone(),
        WorkflowChildRow {
            key,
            child_kind,
            phase_id,
            agent_id: None,
            state: WorkflowChildState::Queued,
            title,
            role,
            queued_at_ms: Some(timestamp_ms),
            started_at_ms: None,
            updated_at_ms: timestamp_ms,
            terminal_at_ms: None,
            terminal_summary: None,
            error_summary: None,
            actual_usage: None,
            latest_activity: None,
            generated_files: Vec::new(),
        },
    );
}

fn child_state(status: WorkflowOutcomeStatus) -> WorkflowChildState {
    match status {
        WorkflowOutcomeStatus::Completed => WorkflowChildState::Completed,
        WorkflowOutcomeStatus::Cancelled => WorkflowChildState::Cancelled,
        WorkflowOutcomeStatus::Interrupted => WorkflowChildState::Interrupted,
        WorkflowOutcomeStatus::Failed
        | WorkflowOutcomeStatus::Denied
        | WorkflowOutcomeStatus::ResourceLimited => WorkflowChildState::Failed,
    }
}
