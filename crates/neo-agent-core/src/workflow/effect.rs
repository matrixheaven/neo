//! Durable external-effect protocol for workflow V2 (design §17.2).
//!
//! Ordering for every host-visible effect:
//!
//! ```text
//! validate + reserve
//! -> append and sync InvocationStarted
//! -> execute through the canonical owner (no runtime lock)
//! -> append and sync InvocationFinished
//! -> expose completion / optional terminal transition
//! ```
//!
//! This module owns pure preparation and reservation checks. Journal appends and
//! in-memory apply stay in [`super::runtime::WorkflowRuntime`] so that the
//! runtime remains the sole durable lifecycle owner. Callers must never hold the
//! async run-state mutex across journal I/O or the external await.

#![allow(dead_code)] // protocol fields are part of the durable effect surface

use crate::workflow::error::WorkflowError;
use crate::workflow::journal::{
    JournalEnvelope, JournalPayload, JournalWriter, canonical_input_hash,
};
use crate::workflow::limits::WorkflowLimits;
use crate::workflow::state::{
    WorkflowActor, WorkflowFinalResultMetadata, WorkflowId, WorkflowInvocationKind,
    WorkflowInvocationOutcome, WorkflowState,
};

/// Prepared, not-yet-durable invocation start.
#[derive(Debug, Clone)]
pub struct PreparedInvocationStart {
    pub envelope: JournalEnvelope,
    pub invocation_id: String,
    pub call_index: u64,
    pub kind: WorkflowInvocationKind,
    pub input_hash: String,
}

/// Prepared invocation finish (post-effect).
#[derive(Debug, Clone)]
pub struct PreparedInvocationFinish {
    pub envelope: JournalEnvelope,
    pub invocation_id: String,
    pub outcome: WorkflowInvocationOutcome,
}

/// Prepared final-result record (must precede `Completed`).
#[derive(Debug, Clone)]
pub struct PreparedFinalResult {
    pub envelope: JournalEnvelope,
    pub metadata: WorkflowFinalResultMetadata,
}

/// Prepared state transition envelope (table-validated by the caller).
#[derive(Debug, Clone)]
pub struct PreparedTransition {
    pub envelope: JournalEnvelope,
    pub previous: WorkflowState,
    pub new_state: WorkflowState,
    pub reason: String,
}

/// Whether the journal still has room for a start record plus a compact finish
/// and a terminal workflow state (design §17.2 reserve rule).
pub fn has_invocation_reservation(
    bytes_written: u64,
    start_envelope: &JournalEnvelope,
    limits: &WorkflowLimits,
) -> Result<bool, WorkflowError> {
    let line =
        serde_json::to_string(start_envelope).map_err(|e| WorkflowError::Journal(e.to_string()))?;
    let line_bytes = u64::try_from(line.len())
        .ok()
        .and_then(|bytes| bytes.checked_add(1))
        .ok_or_else(|| WorkflowError::Journal("record size overflow".to_owned()))?;
    if line_bytes > limits.journal_record_bytes {
        return Ok(false);
    }
    Ok(limits
        .invocation_reservation_bytes(line_bytes)
        .and_then(|reservation| bytes_written.checked_add(reservation))
        .is_some_and(|bytes| bytes <= limits.journal_total_bytes))
}

/// Build a synced-ready `InvocationStarted` envelope. Does not write.
pub fn prepare_invocation_start(
    writer: &JournalWriter,
    run_id: WorkflowId,
    invocation_id: String,
    call_index: u64,
    kind: WorkflowInvocationKind,
    canonical_input: serde_json::Value,
    timestamp_ms: u64,
) -> Result<PreparedInvocationStart, WorkflowError> {
    let input_hash = canonical_input_hash(&canonical_input);
    let envelope = JournalEnvelope::new(
        writer.next_seq(),
        timestamp_ms,
        run_id,
        JournalPayload::InvocationStarted {
            invocation_id: invocation_id.clone(),
            call_index,
            kind,
            canonical_input: Some(canonical_input),
        },
    )
    .with_canonical_input_hash(input_hash.clone());
    Ok(PreparedInvocationStart {
        envelope,
        invocation_id,
        call_index,
        kind,
        input_hash,
    })
}

/// Reserve capacity for the prepared start. Pure check — no I/O.
pub fn reserve_invocation_start(
    writer: &JournalWriter,
    prepared: &PreparedInvocationStart,
    limits: &WorkflowLimits,
) -> Result<(), WorkflowError> {
    if has_invocation_reservation(writer.bytes_written(), &prepared.envelope, limits)? {
        Ok(())
    } else {
        Err(WorkflowError::JournalTotalLimitExceeded)
    }
}

/// Append+sync `InvocationStarted`. Caller must not hold the async run mutex.
pub fn commit_invocation_start(
    writer: &mut JournalWriter,
    prepared: &PreparedInvocationStart,
    limits: &WorkflowLimits,
) -> Result<u64, WorkflowError> {
    reserve_invocation_start(writer, prepared, limits)?;
    writer.append(&prepared.envelope, limits)
}

/// Build a finish envelope after the external effect returns.
pub fn prepare_invocation_finish(
    writer: &JournalWriter,
    run_id: WorkflowId,
    invocation_id: String,
    outcome: WorkflowInvocationOutcome,
    timestamp_ms: u64,
) -> PreparedInvocationFinish {
    let envelope = JournalEnvelope::new(
        writer.next_seq(),
        timestamp_ms,
        run_id,
        JournalPayload::InvocationFinished {
            invocation_id: invocation_id.clone(),
            outcome: outcome.clone(),
        },
    );
    PreparedInvocationFinish {
        envelope,
        invocation_id,
        outcome,
    }
}

/// Append+sync `InvocationFinished`. Caller must not hold the async run mutex.
pub fn commit_invocation_finish(
    writer: &mut JournalWriter,
    prepared: &PreparedInvocationFinish,
    limits: &WorkflowLimits,
) -> Result<u64, WorkflowError> {
    writer.append(&prepared.envelope, limits)
}

/// Build `FinalResultRecorded` (must be durable before `Completed`).
pub fn prepare_final_result(
    writer: &JournalWriter,
    run_id: WorkflowId,
    metadata: WorkflowFinalResultMetadata,
    timestamp_ms: u64,
) -> PreparedFinalResult {
    let envelope = JournalEnvelope::new(
        writer.next_seq(),
        timestamp_ms,
        run_id,
        JournalPayload::FinalResultRecorded {
            metadata: metadata.clone(),
        },
    );
    PreparedFinalResult { envelope, metadata }
}

/// Append+sync `FinalResultRecorded`.
pub fn commit_final_result(
    writer: &mut JournalWriter,
    prepared: &PreparedFinalResult,
    limits: &WorkflowLimits,
) -> Result<u64, WorkflowError> {
    writer.append(&prepared.envelope, limits)
}

/// Build a table-validated state transition envelope.
pub fn prepare_transition(
    writer: &JournalWriter,
    run_id: WorkflowId,
    previous: WorkflowState,
    new_state: WorkflowState,
    reason: impl Into<String>,
    actor: WorkflowActor,
    timestamp_ms: u64,
) -> Result<PreparedTransition, WorkflowError> {
    let reason = reason.into();
    previous.require_transition_to(new_state)?;
    let envelope = JournalEnvelope::new(
        writer.next_seq(),
        timestamp_ms,
        run_id,
        JournalPayload::StateChanged {
            previous,
            new: new_state,
            reason: reason.clone(),
            actor,
        },
    );
    Ok(PreparedTransition {
        envelope,
        previous,
        new_state,
        reason,
    })
}

/// Append+sync a state transition.
pub fn commit_transition(
    writer: &mut JournalWriter,
    prepared: &PreparedTransition,
    limits: &WorkflowLimits,
) -> Result<u64, WorkflowError> {
    writer.append(&prepared.envelope, limits)
}

/// Build the durable create pair: `RunCreated` then implied `Queued` (no state
/// record required until the first real transition).
pub fn prepare_run_created(
    writer: &JournalWriter,
    run_id: WorkflowId,
    name: String,
    description: Option<String>,
    launch_source: Option<String>,
    timestamp_ms: u64,
) -> JournalEnvelope {
    JournalEnvelope::new(
        writer.next_seq(),
        timestamp_ms,
        run_id,
        JournalPayload::RunCreated {
            name,
            description,
            launch_source,
        },
    )
}
