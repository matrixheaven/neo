//! Bounded journal scanning and validation for record and envelope journals.
//!
//! The scanner is the sole owner of sequential journal validation. It produces
//! replay/index state and pages without requiring full-journal retention.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Read};
use std::path::Path;

use super::super::error::{WorkflowError, WorkflowErrorCode};
use super::super::state::{WorkflowId, WorkflowState};
use super::{JournalEnvelope, JournalPayload, validate_envelope};

/// Streaming index built without retaining every record body.
#[derive(Debug, Clone, Default)]
pub struct JournalScanIndex {
    pub next_seq: u64,
    pub bytes_scanned: u64,
    pub last_validated_offset: u64,
    pub record_count: u64,
    pub run_id: Option<WorkflowId>,
    pub run_created: bool,
    pub current_state: Option<WorkflowState>,
    pub started_invocations: HashSet<String>,
    pub finished_invocations: HashSet<String>,
    pub queued_children: HashSet<String>,
    pub started_children: HashSet<String>,
    pub finished_children: HashSet<String>,
    pub child_agent_ids: HashMap<String, Option<String>>,
    pub open_schema_repairs: HashSet<String>,
    pub finished_schema_repairs: HashSet<String>,
    pub open_user_inputs: HashSet<String>,
    pub answered_user_inputs: HashSet<String>,
    pub final_result_seq: Option<u64>,
    pub terminal_state: Option<WorkflowState>,
    pub terminal_timestamp_ms: Option<u64>,
    pub terminal_reason: Option<String>,
    /// Child refs observed on finished outcomes that still need a terminal parent.
    pub open_child_refs: HashSet<String>,
}

impl JournalScanIndex {
    #[must_use]
    pub fn has_incomplete_invocations(&self) -> bool {
        self.started_invocations
            .iter()
            .any(|id| !self.finished_invocations.contains(id))
    }

    #[must_use]
    pub fn has_incomplete_children(&self) -> bool {
        self.queued_children
            .iter()
            .any(|id| !self.finished_children.contains(id))
    }
}

/// One bounded page of envelopes (never a full multi-gigabyte journal).
#[derive(Debug, Clone)]
pub struct JournalPage {
    pub envelopes: Vec<JournalEnvelope>,
    pub first_seq: Option<u64>,
    pub last_seq: Option<u64>,
    pub has_more: bool,
    pub returned_bytes: u64,
    pub next_seq: u64,
}

#[derive(Debug)]
pub(crate) struct JournalRecoveryPrefix {
    /// Index for the complete journal after normalizing `suffix` when
    /// `valid_suffix_seq` is present; otherwise the validated prefix index.
    pub index: JournalScanIndex,
    pub last_validated_offset: u64,
    pub suffix: Vec<u8>,
    pub valid_suffix_seq: Option<u64>,
}

/// Stream the canonical journal through the same invariant validator while
/// retaining only one bounded non-newline EOF suffix for recovery.
pub(crate) fn scan_recovery_prefix(
    path: &Path,
    expected_run_id: Option<&WorkflowId>,
    max_record_bytes: u64,
    max_total_bytes: u64,
) -> Result<JournalRecoveryPrefix, WorkflowError> {
    if max_record_bytes == 0 {
        return Err(journal_corrupt(
            "journal record limit must be greater than zero",
        ));
    }
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(JournalRecoveryPrefix {
                index: JournalScanIndex {
                    run_id: expected_run_id.cloned(),
                    ..Default::default()
                },
                last_validated_offset: 0,
                suffix: Vec::new(),
                valid_suffix_seq: None,
            });
        }
        Err(error) => return Err(WorkflowError::Journal(error.to_string())),
    };
    let read_limit = max_record_bytes
        .checked_add(1)
        .ok_or_else(|| journal_corrupt("journal record limit overflow"))?;
    let mut reader = std::io::BufReader::new(file);
    let mut index = JournalScanIndex {
        run_id: expected_run_id.cloned(),
        ..Default::default()
    };
    let mut offset = 0u64;

    loop {
        let mut line = Vec::new();
        let bytes_read = reader
            .by_ref()
            .take(read_limit)
            .read_until(b'\n', &mut line)
            .map_err(|error| WorkflowError::Journal(error.to_string()))?;
        if bytes_read == 0 {
            if index.record_count > 0 {
                finalize_index(&index)?;
            }
            return Ok(JournalRecoveryPrefix {
                index,
                last_validated_offset: offset,
                suffix: Vec::new(),
                valid_suffix_seq: None,
            });
        }
        let line_bytes =
            u64::try_from(bytes_read).map_err(|_| journal_corrupt("journal line size overflow"))?;
        let offset_end = offset
            .checked_add(line_bytes)
            .ok_or_else(|| journal_corrupt("journal offset overflow"))?;
        if offset_end > max_total_bytes {
            return Err(journal_corrupt(format!(
                "journal exceeds configured limit of {max_total_bytes} bytes"
            )));
        }
        if line.last() != Some(&b'\n') {
            if line_bytes > max_record_bytes {
                return Err(journal_corrupt(format!(
                    "journal EOF record exceeds configured limit of {max_record_bytes} bytes"
                )));
            }
            let valid_suffix_seq = match serde_json::from_slice::<JournalEnvelope>(&line) {
                Ok(envelope) => {
                    let normalized_line_bytes = line_bytes
                        .checked_add(1)
                        .ok_or_else(|| journal_corrupt("journal line size overflow"))?;
                    if normalized_line_bytes > max_record_bytes {
                        return Err(journal_corrupt(format!(
                            "journal record exceeds configured limit of {max_record_bytes} bytes"
                        )));
                    }
                    validate_recovery_envelope(
                        &envelope,
                        &mut index,
                        expected_run_id,
                        normalized_line_bytes,
                        offset
                            .checked_add(normalized_line_bytes)
                            .filter(|end| *end <= max_total_bytes)
                            .ok_or_else(|| {
                                journal_corrupt(format!(
                                    "journal exceeds configured limit of {max_total_bytes} bytes"
                                ))
                            })?,
                    )?;
                    finalize_index(&index)?;
                    Some(envelope.seq)
                }
                Err(_) => None,
            };
            return Ok(JournalRecoveryPrefix {
                index,
                last_validated_offset: offset,
                suffix: line,
                valid_suffix_seq,
            });
        }
        if line_bytes > max_record_bytes {
            return Err(journal_corrupt(format!(
                "journal record exceeds configured limit of {max_record_bytes} bytes"
            )));
        }
        let content = &line[..line.len() - 1];
        if content.is_empty() {
            return Err(journal_corrupt("malformed record: empty journal line"));
        }
        let envelope: JournalEnvelope = serde_json::from_slice(content).map_err(|error| {
            journal_corrupt(format!("malformed or unknown journal record: {error}"))
        })?;
        validate_recovery_envelope(
            &envelope,
            &mut index,
            expected_run_id,
            line_bytes,
            offset_end,
        )?;
        offset = offset_end;
    }
}

fn validate_recovery_envelope(
    envelope: &JournalEnvelope,
    index: &mut JournalScanIndex,
    expected_run_id: Option<&WorkflowId>,
    line_bytes: u64,
    offset_end: u64,
) -> Result<(), WorkflowError> {
    if envelope.seq != index.next_seq {
        return Err(journal_corrupt(format!(
            "sequence gap: expected {}, got {}",
            index.next_seq, envelope.seq
        )));
    }
    match &index.run_id {
        None => index.run_id = Some(envelope.run_id.clone()),
        Some(expected) if expected != &envelope.run_id => {
            return Err(journal_corrupt(format!(
                "run id mismatch: expected {}, got {}",
                expected.as_str(),
                envelope.run_id.as_str()
            )));
        }
        Some(_) => {}
    }
    if let Some(expected) = expected_run_id
        && expected != &envelope.run_id
    {
        return Err(journal_corrupt(format!(
            "run id mismatch: expected {}, got {}",
            expected.as_str(),
            envelope.run_id.as_str()
        )));
    }
    validate_envelope(envelope)?;
    observe_envelope(envelope, index)?;
    index.record_count = index
        .record_count
        .checked_add(1)
        .ok_or_else(|| journal_corrupt("journal record count overflow"))?;
    index.bytes_scanned = index
        .bytes_scanned
        .checked_add(line_bytes)
        .ok_or_else(|| journal_corrupt("journal size overflow"))?;
    index.last_validated_offset = offset_end;
    index.next_seq = envelope
        .seq
        .checked_add(1)
        .ok_or_else(|| journal_corrupt("journal sequence overflow"))?;
    Ok(())
}

/// Scan the canonical journal, validating sequence / run ID / hash / pairing invariants.
///
/// Does not retain record bodies — only index state.
pub fn scan_journal(
    path: &Path,
    expected_run_id: Option<&WorkflowId>,
    max_record_bytes: u64,
    max_total_bytes: u64,
) -> Result<JournalScanIndex, WorkflowError> {
    let mut index = JournalScanIndex::default();
    for_each_line(
        path,
        expected_run_id,
        max_record_bytes,
        max_total_bytes,
        |envelope, line_bytes, offset_end| {
            observe_envelope(&envelope, &mut index)?;
            index.record_count = index
                .record_count
                .checked_add(1)
                .ok_or_else(|| journal_corrupt("journal record count overflow"))?;
            index.bytes_scanned = index
                .bytes_scanned
                .checked_add(line_bytes)
                .ok_or_else(|| journal_corrupt("journal size overflow"))?;
            index.last_validated_offset = offset_end;
            index.next_seq = envelope
                .seq
                .checked_add(1)
                .ok_or_else(|| journal_corrupt("journal sequence overflow"))?;
            Ok(())
        },
    )?;
    finalize_index(&index)?;
    Ok(index)
}

/// Collect every validated envelope (test / small-journal helper only).
pub fn collect_journal(
    path: &Path,
    expected_run_id: Option<&WorkflowId>,
    max_record_bytes: u64,
    max_total_bytes: u64,
) -> Result<Vec<JournalEnvelope>, WorkflowError> {
    let mut out = Vec::new();
    let mut index = JournalScanIndex::default();
    for_each_line(
        path,
        expected_run_id,
        max_record_bytes,
        max_total_bytes,
        |envelope, line_bytes, offset_end| {
            observe_envelope(&envelope, &mut index)?;
            index.record_count = index
                .record_count
                .checked_add(1)
                .ok_or_else(|| journal_corrupt("journal record count overflow"))?;
            index.bytes_scanned = index
                .bytes_scanned
                .checked_add(line_bytes)
                .ok_or_else(|| journal_corrupt("journal size overflow"))?;
            index.last_validated_offset = offset_end;
            index.next_seq = envelope
                .seq
                .checked_add(1)
                .ok_or_else(|| journal_corrupt("journal sequence overflow"))?;
            out.push(envelope);
            Ok(())
        },
    )?;
    finalize_index(&index)?;
    Ok(out)
}

/// Return a bounded ascending page of envelopes starting at `from_seq`.
pub fn scan_journal_page(
    path: &Path,
    expected_run_id: Option<&WorkflowId>,
    max_record_bytes: u64,
    max_total_bytes: u64,
    from_seq: u64,
    max_records: usize,
    max_bytes: u64,
) -> Result<JournalPage, WorkflowError> {
    if max_records == 0 || max_bytes == 0 {
        return Err(WorkflowError::InvalidInput(
            "journal page limits must be greater than zero".to_owned(),
        ));
    }

    let mut index = JournalScanIndex::default();
    let mut envelopes = Vec::new();
    let mut returned_bytes = 0u64;
    let mut has_more = false;
    let mut page_started = false;

    for_each_line(
        path,
        expected_run_id,
        max_record_bytes,
        max_total_bytes,
        |envelope, line_bytes, offset_end| {
            observe_envelope(&envelope, &mut index)?;
            index.record_count = index
                .record_count
                .checked_add(1)
                .ok_or_else(|| journal_corrupt("journal record count overflow"))?;
            index.bytes_scanned = index
                .bytes_scanned
                .checked_add(line_bytes)
                .ok_or_else(|| journal_corrupt("journal size overflow"))?;
            index.last_validated_offset = offset_end;
            index.next_seq = envelope
                .seq
                .checked_add(1)
                .ok_or_else(|| journal_corrupt("journal sequence overflow"))?;

            if envelope.seq < from_seq {
                return Ok(());
            }
            if page_started
                && (envelopes.len() >= max_records
                    || returned_bytes.saturating_add(line_bytes) > max_bytes)
            {
                has_more = true;
                return Ok(());
            }
            // Once has_more is set we still must validate the remainder without
            // retaining bodies — skip push only.
            if has_more {
                return Ok(());
            }
            page_started = true;
            returned_bytes = returned_bytes
                .checked_add(line_bytes)
                .ok_or_else(|| journal_corrupt("journal page size overflow"))?;
            envelopes.push(envelope);
            Ok(())
        },
    )?;
    finalize_index(&index)?;

    let first_seq = envelopes.first().map(|e| e.seq);
    let last_seq = envelopes.last().map(|e| e.seq);
    let next_seq = last_seq.map_or(from_seq, |s| s.saturating_add(1));
    Ok(JournalPage {
        envelopes,
        first_seq,
        last_seq,
        has_more,
        returned_bytes,
        next_seq,
    })
}

fn for_each_line(
    path: &Path,
    expected_run_id: Option<&WorkflowId>,
    max_record_bytes: u64,
    max_total_bytes: u64,
    mut on_record: impl FnMut(JournalEnvelope, u64, u64) -> Result<(), WorkflowError>,
) -> Result<(), WorkflowError> {
    if max_record_bytes == 0 {
        return Err(journal_corrupt(
            "journal record limit must be greater than zero",
        ));
    }
    let read_limit = max_record_bytes
        .checked_add(1)
        .ok_or_else(|| journal_corrupt("journal record limit overflow"))?;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(WorkflowError::Journal(error.to_string())),
    };
    let mut reader = std::io::BufReader::new(file);
    let mut offset = 0u64;
    let mut expected_seq = 0u64;
    let mut run_id: Option<WorkflowId> = expected_run_id.cloned();

    loop {
        let mut line = Vec::new();
        let bytes_read = reader
            .by_ref()
            .take(read_limit)
            .read_until(b'\n', &mut line)
            .map_err(|e| WorkflowError::Journal(e.to_string()))?;
        if bytes_read == 0 {
            break;
        }
        let line_bytes =
            u64::try_from(bytes_read).map_err(|_| journal_corrupt("journal line size overflow"))?;
        if line_bytes > max_record_bytes {
            return Err(journal_corrupt(format!(
                "journal record exceeds configured limit of {max_record_bytes} bytes"
            )));
        }
        let offset_end = offset
            .checked_add(line_bytes)
            .ok_or_else(|| journal_corrupt("journal offset overflow"))?;
        if offset_end > max_total_bytes {
            return Err(journal_corrupt(format!(
                "journal exceeds configured limit of {max_total_bytes} bytes"
            )));
        }

        if line.last() != Some(&b'\n') {
            return Err(journal_corrupt(
                "truncated record: journal does not end with a newline",
            ));
        }
        let content = &line[..line.len() - 1];
        if content.is_empty() {
            return Err(journal_corrupt("malformed record: empty journal line"));
        }

        let envelope: JournalEnvelope = serde_json::from_slice(content).map_err(|e| {
            // Unknown kinds or fields surface as decode failure and fail closed.
            journal_corrupt(format!("malformed or unknown journal record: {e}"))
        })?;

        if envelope.seq != expected_seq {
            return Err(journal_corrupt(format!(
                "sequence gap: expected {expected_seq}, got {}",
                envelope.seq
            )));
        }
        match &run_id {
            None => run_id = Some(envelope.run_id.clone()),
            Some(expected) if expected != &envelope.run_id => {
                return Err(journal_corrupt(format!(
                    "run id mismatch: expected {}, got {}",
                    expected.as_str(),
                    envelope.run_id.as_str()
                )));
            }
            Some(_) => {}
        }
        if let Some(expected) = expected_run_id
            && expected != &envelope.run_id
        {
            return Err(journal_corrupt(format!(
                "run id mismatch: expected {}, got {}",
                expected.as_str(),
                envelope.run_id.as_str()
            )));
        }

        validate_envelope(&envelope)?;

        on_record(envelope, line_bytes, offset_end)?;
        expected_seq = expected_seq
            .checked_add(1)
            .ok_or_else(|| journal_corrupt("journal sequence overflow"))?;
        offset = offset_end;
    }
    Ok(())
}

pub(super) fn observe_envelope(
    envelope: &JournalEnvelope,
    index: &mut JournalScanIndex,
) -> Result<(), WorkflowError> {
    if index.run_id.is_none() {
        index.run_id = Some(envelope.run_id.clone());
    }

    if !matches!(envelope.payload, JournalPayload::RunCreated { .. }) && !index.run_created {
        return Err(journal_corrupt("journal record appears before run_created"));
    }

    // Contiguous sequence already checked by the line scanner against expected_seq.
    match &envelope.payload {
        JournalPayload::RunCreated { .. } => {
            if index.run_created || index.record_count != 0 {
                return Err(journal_corrupt(
                    "run_created must be the unique first record",
                ));
            }
            index.run_created = true;
            index.current_state = Some(WorkflowState::Queued);
        }
        JournalPayload::StateChanged {
            previous,
            new,
            reason,
            ..
        } => {
            let current = index
                .current_state
                .ok_or_else(|| journal_corrupt("state change before run_created"))?;
            if *previous != current {
                return Err(journal_corrupt(format!(
                    "state transition previous mismatch: journal says {}, current is {}",
                    previous.as_str(),
                    current.as_str()
                )));
            }
            if !previous.can_transition_to(*new) {
                return Err(journal_corrupt(format!(
                    "illegal workflow transition {} -> {}",
                    previous.as_str(),
                    new.as_str()
                )));
            }
            if new.is_terminal() {
                // Terminal-child invariant: every started invocation/swarm item
                // must have finished before a terminal workflow state.
                if index.has_incomplete_invocations() {
                    return Err(journal_corrupt(
                        "terminal state with incomplete invocation (terminal-child invariant)",
                    ));
                }
                if index.has_incomplete_children() {
                    return Err(journal_corrupt(
                        "terminal state with incomplete swarm item (terminal-child invariant)",
                    ));
                }
                // Final-result ordering: Completed requires a prior FinalResultRecorded.
                if *new == WorkflowState::Completed && index.final_result_seq.is_none() {
                    return Err(journal_corrupt(
                        "completed state without final_result_recorded",
                    ));
                }
                index.terminal_state = Some(*new);
                index.terminal_timestamp_ms = Some(envelope.timestamp_ms);
                index.terminal_reason = Some(reason.clone());
            }
            index.current_state = Some(*new);
        }
        JournalPayload::InvocationStarted { invocation_id, .. } => {
            if index.started_invocations.contains(invocation_id) {
                return Err(journal_corrupt(format!(
                    "duplicate invocation_started for invocation {invocation_id}"
                )));
            }
            index.started_invocations.insert(invocation_id.clone());
        }
        JournalPayload::InvocationFinished {
            invocation_id,
            outcome,
        } => {
            if !index.started_invocations.contains(invocation_id) {
                return Err(journal_corrupt(format!(
                    "invocation_finished without invocation_started for invocation {invocation_id}"
                )));
            }
            if index.finished_invocations.contains(invocation_id) {
                return Err(journal_corrupt(format!(
                    "duplicate invocation_finished for invocation {invocation_id}"
                )));
            }
            index.finished_invocations.insert(invocation_id.clone());
            for child in &outcome.child_refs {
                let key = format!("{}:{}", child.kind, child.id);
                // Track child refs as observed terminal outcomes from host.
                // Open children are those started via swarm/invocation that are
                // not yet finished; child_refs on finish are the durable refs.
                let _ = key;
            }
        }
        JournalPayload::SchemaRepairStarted { repair_id, .. } => {
            if index.open_schema_repairs.contains(repair_id)
                && !index.finished_schema_repairs.contains(repair_id)
            {
                return Err(journal_corrupt(format!(
                    "duplicate schema_repair_started for {repair_id}"
                )));
            }
            index.open_schema_repairs.insert(repair_id.clone());
        }
        JournalPayload::SchemaRepairFinished { repair_id, .. } => {
            if !index.open_schema_repairs.contains(repair_id) {
                return Err(journal_corrupt(format!(
                    "schema_repair_finished without start for {repair_id}"
                )));
            }
            if index.finished_schema_repairs.contains(repair_id) {
                return Err(journal_corrupt(format!(
                    "duplicate schema_repair_finished for {repair_id}"
                )));
            }
            index.finished_schema_repairs.insert(repair_id.clone());
        }
        JournalPayload::UserInputRequested { request_id, .. } => {
            if index.open_user_inputs.contains(request_id)
                && !index.answered_user_inputs.contains(request_id)
            {
                return Err(journal_corrupt(format!(
                    "duplicate user_input_requested for {request_id}"
                )));
            }
            index.open_user_inputs.insert(request_id.clone());
        }
        JournalPayload::UserInputAnswered { request_id, .. } => {
            if !index.open_user_inputs.contains(request_id) {
                return Err(journal_corrupt(format!(
                    "user_input_answered without request for {request_id}"
                )));
            }
            if index.answered_user_inputs.contains(request_id) {
                return Err(journal_corrupt(format!(
                    "duplicate user_input_answered for {request_id}"
                )));
            }
            index.answered_user_inputs.insert(request_id.clone());
        }
        JournalPayload::ArtifactCommitted { .. }
        | JournalPayload::RecoveryActionApplied { .. }
        | JournalPayload::UsageRecorded { .. }
        | JournalPayload::ProvenanceRecorded { .. } => {}
        JournalPayload::ChildQueued { child_key, .. } => {
            let key = child_key.display_key();
            if !index.queued_children.insert(key.clone()) {
                return Err(journal_corrupt(format!("duplicate child queued: {key}")));
            }
        }
        JournalPayload::ChildStarted {
            child_key,
            agent_id,
        } => {
            let key = child_key.display_key();
            if !index.queued_children.contains(&key) {
                return Err(journal_corrupt(format!(
                    "child started without queue: {key}"
                )));
            }
            if index.finished_children.contains(&key) {
                return Err(journal_corrupt(format!(
                    "child started after finish: {key}"
                )));
            }
            if index.started_children.contains(&key) {
                return Err(journal_corrupt(format!("duplicate child started: {key}")));
            }
            index.started_children.insert(key.clone());
            index.child_agent_ids.insert(key, agent_id.clone());
        }
        JournalPayload::ChildFinished {
            child_key,
            agent_id,
            ..
        } => {
            let key = child_key.display_key();
            if !index.queued_children.contains(&key) {
                return Err(journal_corrupt(format!(
                    "child finished without queue: {key}"
                )));
            }
            if index.finished_children.contains(&key) {
                return Err(journal_corrupt(format!("duplicate child finished: {key}")));
            }
            if index.started_children.contains(&key)
                && let Some(started_agent_id) = index.child_agent_ids.get(&key)
                && started_agent_id != agent_id
            {
                return Err(journal_corrupt(format!(
                    "child agent id mismatch for {key}: started {}, finished {}",
                    started_agent_id.as_deref().unwrap_or("<none>"),
                    agent_id.as_deref().unwrap_or("<none>")
                )));
            }
            index.finished_children.insert(key.clone());
            index
                .child_agent_ids
                .entry(key)
                .or_insert_with(|| agent_id.clone());
        }
        JournalPayload::FinalResultRecorded { .. } => {
            if index.final_result_seq.is_some() {
                return Err(journal_corrupt("duplicate final_result_recorded"));
            }
            if index.terminal_state == Some(WorkflowState::Completed) {
                return Err(journal_corrupt(
                    "final_result_recorded after completed state",
                ));
            }
            index.final_result_seq = Some(envelope.seq);
        }
    }
    Ok(())
}

pub(super) fn finalize_index(index: &JournalScanIndex) -> Result<(), WorkflowError> {
    if !index.run_created {
        return Err(journal_corrupt("journal is missing run_created"));
    }
    if index.terminal_state == Some(WorkflowState::Completed) && index.final_result_seq.is_none() {
        return Err(journal_corrupt(
            "completed state without final_result_recorded",
        ));
    }
    Ok(())
}

fn journal_corrupt(message: impl Into<String>) -> WorkflowError {
    WorkflowError::coded(WorkflowErrorCode::JournalCorrupt, message)
}
