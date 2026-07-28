//! Bounded journal scanning and validation for record and envelope journals.
//!
//! The scanner is the sole owner of sequential journal validation. It produces
//! replay/index state and pages without requiring full-journal retention.

use std::collections::HashSet;
use std::io::BufRead;
use std::path::Path;

use super::super::error::{WorkflowError, WorkflowErrorCode};
use super::super::state::{WorkflowId, WorkflowState};
use super::{
    JournalEnvelope, JournalPayload, JournalRecord, canonical_input_hash, validate_envelope,
};

/// Streaming index built without retaining every record body.
#[derive(Debug, Clone, Default)]
pub struct JournalScanIndex {
    pub next_seq: u64,
    pub bytes_scanned: u64,
    pub last_validated_offset: u64,
    pub record_count: u64,
    pub run_id: Option<WorkflowId>,
    pub started_invocations: HashSet<String>,
    pub finished_invocations: HashSet<String>,
    pub open_swarm_items: HashSet<String>,
    pub finished_swarm_items: HashSet<String>,
    pub open_schema_repairs: HashSet<String>,
    pub finished_schema_repairs: HashSet<String>,
    pub open_user_inputs: HashSet<String>,
    pub answered_user_inputs: HashSet<String>,
    pub final_result_seq: Option<u64>,
    pub terminal_state: Option<WorkflowState>,
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
    pub fn has_incomplete_swarm_items(&self) -> bool {
        self.open_swarm_items
            .iter()
            .any(|id| !self.finished_swarm_items.contains(id))
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

/// Scan the canonical journal, validating sequence / run ID / hash / pairing invariants.
///
/// Does not retain record bodies — only index state.
pub fn scan_journal(
    path: &Path,
    expected_run_id: Option<&WorkflowId>,
) -> Result<JournalScanIndex, WorkflowError> {
    let mut index = JournalScanIndex::default();
    for_each_line(path, expected_run_id, |envelope, line_bytes, offset_end| {
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
    })?;
    finalize_index(&index)?;
    Ok(index)
}

/// Collect every validated envelope (test / small-journal helper only).
pub fn collect_journal(
    path: &Path,
    expected_run_id: Option<&WorkflowId>,
) -> Result<Vec<JournalEnvelope>, WorkflowError> {
    let mut out = Vec::new();
    let mut index = JournalScanIndex::default();
    for_each_line(path, expected_run_id, |envelope, line_bytes, offset_end| {
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
    })?;
    finalize_index(&index)?;
    Ok(out)
}

/// Return a bounded ascending page of envelopes starting at `from_seq`.
pub fn scan_journal_page(
    path: &Path,
    expected_run_id: Option<&WorkflowId>,
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

    for_each_line(path, expected_run_id, |envelope, line_bytes, offset_end| {
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
    })?;
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

/// Stream-validate a V1 journal and build an open index without full retention.
pub fn scan_journal_v1_index(path: &Path) -> Result<JournalScanIndex, WorkflowError> {
    let mut index = JournalScanIndex::default();
    for_each_v1_line(path, |record, line_bytes, offset_end| {
        validate_v1_against_index(&record, &index)?;
        observe_v1_record(&record, &mut index);
        index.record_count = index
            .record_count
            .checked_add(1)
            .ok_or_else(|| WorkflowError::Journal("journal record count overflow".to_owned()))?;
        index.bytes_scanned = index
            .bytes_scanned
            .checked_add(line_bytes)
            .ok_or_else(|| WorkflowError::Journal("journal size overflow".to_owned()))?;
        index.last_validated_offset = offset_end;
        index.next_seq = record
            .seq()
            .checked_add(1)
            .ok_or_else(|| WorkflowError::Journal("journal sequence overflow".to_owned()))?;
        Ok(())
    })?;
    Ok(index)
}

/// Collect V1 records via the streaming scanner (compatibility / small journals).
pub fn collect_journal_v1(path: &Path) -> Result<Vec<JournalRecord>, WorkflowError> {
    let mut records = Vec::new();
    let mut index = JournalScanIndex::default();
    for_each_v1_line(path, |record, line_bytes, offset_end| {
        validate_v1_against_index(&record, &index)?;
        observe_v1_record(&record, &mut index);
        index.next_seq = record
            .seq()
            .checked_add(1)
            .ok_or_else(|| WorkflowError::Journal("journal sequence overflow".to_owned()))?;
        index.bytes_scanned = index
            .bytes_scanned
            .checked_add(line_bytes)
            .ok_or_else(|| WorkflowError::Journal("journal size overflow".to_owned()))?;
        index.last_validated_offset = offset_end;
        records.push(record);
        Ok(())
    })?;
    Ok(records)
}

fn for_each_line(
    path: &Path,
    expected_run_id: Option<&WorkflowId>,
    mut on_record: impl FnMut(JournalEnvelope, u64, u64) -> Result<(), WorkflowError>,
) -> Result<(), WorkflowError> {
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
            .read_until(b'\n', &mut line)
            .map_err(|e| WorkflowError::Journal(e.to_string()))?;
        if bytes_read == 0 {
            break;
        }
        let line_bytes =
            u64::try_from(bytes_read).map_err(|_| journal_corrupt("journal line size overflow"))?;
        let offset_end = offset
            .checked_add(line_bytes)
            .ok_or_else(|| journal_corrupt("journal offset overflow"))?;

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
            // Unknown version/kind/fields surface as decode failure → fail closed.
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

fn for_each_v1_line(
    path: &Path,
    mut on_record: impl FnMut(JournalRecord, u64, u64) -> Result<(), WorkflowError>,
) -> Result<(), WorkflowError> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(WorkflowError::Journal(error.to_string())),
    };
    let mut reader = std::io::BufReader::new(file);
    let mut offset = 0u64;

    loop {
        let mut line = Vec::new();
        let bytes_read = reader
            .read_until(b'\n', &mut line)
            .map_err(|e| WorkflowError::Journal(e.to_string()))?;
        if bytes_read == 0 {
            break;
        }
        let line_bytes = u64::try_from(bytes_read)
            .map_err(|_| WorkflowError::Journal("journal line size overflow".to_owned()))?;
        let offset_end = offset
            .checked_add(line_bytes)
            .ok_or_else(|| WorkflowError::Journal("journal offset overflow".to_owned()))?;

        if line.last() != Some(&b'\n') {
            return Err(WorkflowError::Journal(
                "truncated record: journal does not end with a newline".to_owned(),
            ));
        }
        let content = &line[..line.len() - 1];
        if content.is_empty() {
            return Err(WorkflowError::Journal(
                "malformed record: empty journal line".to_owned(),
            ));
        }
        let record: JournalRecord = serde_json::from_slice(content)
            .map_err(|e| WorkflowError::Journal(format!("malformed record: {e}")))?;
        on_record(record, line_bytes, offset_end)?;
        offset = offset_end;
    }
    Ok(())
}

fn validate_v1_against_index(
    record: &JournalRecord,
    index: &JournalScanIndex,
) -> Result<(), WorkflowError> {
    if record.seq() != index.next_seq {
        return Err(WorkflowError::Journal(format!(
            "sequence gap: expected {}, got {}",
            index.next_seq,
            record.seq()
        )));
    }
    match record {
        JournalRecord::InvocationStarted {
            invocation_id,
            canonical_input,
            canonical_input_hash: recorded_hash,
            ..
        } => {
            let expected_hash = canonical_input_hash(canonical_input);
            if *recorded_hash != expected_hash {
                return Err(WorkflowError::Journal(format!(
                    "canonical input hash mismatch for invocation {invocation_id}"
                )));
            }
            if index.started_invocations.contains(invocation_id) {
                return Err(WorkflowError::Journal(format!(
                    "duplicate invocation_started for invocation {invocation_id}"
                )));
            }
        }
        JournalRecord::InvocationFinished { invocation_id, .. } => {
            if !index.started_invocations.contains(invocation_id) {
                return Err(WorkflowError::Journal(format!(
                    "invocation_finished without invocation_started for invocation {invocation_id}"
                )));
            }
            if index.finished_invocations.contains(invocation_id) {
                return Err(WorkflowError::Journal(format!(
                    "duplicate invocation_finished for invocation {invocation_id}"
                )));
            }
        }
        JournalRecord::StateChanged { .. } => {}
    }
    Ok(())
}

fn observe_v1_record(record: &JournalRecord, index: &mut JournalScanIndex) {
    match record {
        JournalRecord::InvocationStarted { invocation_id, .. } => {
            index.started_invocations.insert(invocation_id.clone());
        }
        JournalRecord::InvocationFinished { invocation_id, .. } => {
            index.finished_invocations.insert(invocation_id.clone());
        }
        JournalRecord::StateChanged { new, reason, .. } => {
            if new.is_terminal() {
                index.terminal_state = Some(*new);
                index.terminal_reason = Some(reason.clone());
            }
        }
    }
}

pub(super) fn observe_envelope(
    envelope: &JournalEnvelope,
    index: &mut JournalScanIndex,
) -> Result<(), WorkflowError> {
    if index.run_id.is_none() {
        index.run_id = Some(envelope.run_id.clone());
    }

    // Contiguous sequence already checked by the line scanner against expected_seq.
    match &envelope.payload {
        JournalPayload::RunCreated { .. } => {}
        JournalPayload::StateChanged {
            previous: _,
            new,
            reason,
            ..
        } => {
            if new.is_terminal() {
                // Terminal-child invariant: every started invocation/swarm item
                // must have finished before a terminal workflow state.
                if index.has_incomplete_invocations() {
                    return Err(journal_corrupt(
                        "terminal state with incomplete invocation (terminal-child invariant)",
                    ));
                }
                if index.has_incomplete_swarm_items() {
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
                index.terminal_reason = Some(reason.clone());
            }
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
        JournalPayload::SwarmItemQueued {
            swarm_id, item_id, ..
        }
        | JournalPayload::SwarmItemStarted {
            swarm_id, item_id, ..
        } => {
            let key = swarm_item_key(swarm_id, item_id);
            if matches!(envelope.payload, JournalPayload::SwarmItemStarted { .. })
                && index.finished_swarm_items.contains(&key)
            {
                return Err(journal_corrupt(format!(
                    "swarm item started after finish: {key}"
                )));
            }
            index.open_swarm_items.insert(key);
        }
        JournalPayload::SwarmItemFinished {
            swarm_id, item_id, ..
        } => {
            let key = swarm_item_key(swarm_id, item_id);
            if !index.open_swarm_items.contains(&key) {
                return Err(journal_corrupt(format!(
                    "swarm item finished without start/queue: {key}"
                )));
            }
            if index.finished_swarm_items.contains(&key) {
                return Err(journal_corrupt(format!(
                    "duplicate swarm item finished: {key}"
                )));
            }
            index.finished_swarm_items.insert(key);
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
        | JournalPayload::LineageSeedImported { .. }
        | JournalPayload::RecoveryActionApplied { .. }
        | JournalPayload::UsageRecorded { .. }
        | JournalPayload::ProvenanceRecorded { .. } => {}
        JournalPayload::ChildQueued { .. } => {}
        JournalPayload::ChildStarted { .. } => {}
        JournalPayload::ChildFinished { .. } => {}
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
    if index.terminal_state == Some(WorkflowState::Completed) && index.final_result_seq.is_none() {
        return Err(journal_corrupt(
            "completed state without final_result_recorded",
        ));
    }
    Ok(())
}

fn swarm_item_key(swarm_id: &str, item_id: &str) -> String {
    format!("{swarm_id}:{item_id}")
}

fn journal_corrupt(message: impl Into<String>) -> WorkflowError {
    WorkflowError::coded(WorkflowErrorCode::JournalCorrupt, message)
}
