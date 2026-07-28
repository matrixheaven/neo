use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::error::{WorkflowError, WorkflowErrorCode};
use super::limits::WorkflowLimits;
use super::state::{
    WorkflowActor, WorkflowArtifactId, WorkflowFinalResultMetadata, WorkflowId,
    WorkflowInvocationKind, WorkflowInvocationOutcome, WorkflowLineageMetadata, WorkflowState,
};
use crate::AgentTokenUsage;
use crate::session::atomic_file;

/// Journal format version for legacy V1 records (unversioned wire shape).
pub const JOURNAL_FORMAT_V1: u32 = 1;
/// Journal format version for versioned V2 envelopes.
pub const JOURNAL_FORMAT_V2: u32 = 2;
/// Journal format version for V3 with generic child lifecycle events.
pub const JOURNAL_FORMAT_V3: u32 = 3;

#[path = "journal_scan.rs"]
pub mod journal_scan;

pub use journal_scan::{
    JournalPage, JournalScanIndex, collect_journal_v1, collect_journal_v2, scan_journal_v1_index,
    scan_journal_v2, scan_journal_v2_page,
};

// ---------------------------------------------------------------------------
// V1 wire records (read-only fixtures + current runtime writer path)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JournalRecord {
    StateChanged {
        seq: u64,
        timestamp_ms: u64,
        previous: WorkflowState,
        new: WorkflowState,
        reason: String,
        actor: WorkflowActor,
    },
    InvocationStarted {
        seq: u64,
        timestamp_ms: u64,
        invocation_id: String,
        call_index: u64,
        kind: WorkflowInvocationKind,
        canonical_input: serde_json::Value,
        canonical_input_hash: String,
    },
    InvocationFinished {
        seq: u64,
        timestamp_ms: u64,
        invocation_id: String,
        outcome: WorkflowInvocationOutcome,
    },
}

impl JournalRecord {
    #[must_use]
    pub fn seq(&self) -> u64 {
        match self {
            Self::StateChanged { seq, .. }
            | Self::InvocationStarted { seq, .. }
            | Self::InvocationFinished { seq, .. } => *seq,
        }
    }
}

// ---------------------------------------------------------------------------
// V2 versioned envelope + record families
// ---------------------------------------------------------------------------

/// Hash-addressed payload reference for large journal bodies.
///
/// Usage, terminal reason, and child/task references stay inline on their
/// owning records; only verbose details move behind refs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JournalPayloadRef {
    /// Logical role of the referenced bytes (`details`, `final_result`, …).
    pub role: String,
    pub artifact_id: WorkflowArtifactId,
    pub sha256: String,
    pub byte_len: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_name: Option<String>,
}

/// Stable child identity key (no random UUID).
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum WorkflowChildKey {
    #[serde(rename = "direct_delegate")]
    DirectDelegate { invocation_id: String },
    #[serde(rename = "swarm_item")]
    SwarmItem { swarm_id: String, item_id: String },
}

impl WorkflowChildKey {
    #[must_use]
    pub fn display_key(&self) -> String {
        match self {
            Self::DirectDelegate { invocation_id } => format!("delegate:{invocation_id}"),
            Self::SwarmItem { swarm_id, item_id } => format!("swarm:{swarm_id}:{item_id}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowChildKind {
    Delegate,
    SwarmItem,
}

/// Versioned journal envelope (design §17).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct JournalEnvelope {
    pub version: u32,
    pub seq: u64,
    pub timestamp_ms: u64,
    pub run_id: WorkflowId,
    pub payload: JournalPayload,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_input_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub payload_refs: Vec<JournalPayloadRef>,
}

impl JournalEnvelope {
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Build a V2 envelope for `run_id` with the given payload.
    #[must_use]
    pub fn new(seq: u64, timestamp_ms: u64, run_id: WorkflowId, payload: JournalPayload) -> Self {
        Self {
            version: JOURNAL_FORMAT_V2,
            seq,
            timestamp_ms,
            run_id,
            payload,
            canonical_input_hash: None,
            payload_refs: Vec::new(),
        }
    }

    /// Build a V3 envelope for `run_id` with the given payload.
    #[must_use]
    pub fn new_v3(
        seq: u64,
        timestamp_ms: u64,
        run_id: WorkflowId,
        payload: JournalPayload,
    ) -> Self {
        Self {
            version: JOURNAL_FORMAT_V3,
            seq,
            timestamp_ms,
            run_id,
            payload,
            canonical_input_hash: None,
            payload_refs: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_canonical_input_hash(mut self, hash: impl Into<String>) -> Self {
        self.canonical_input_hash = Some(hash.into());
        self
    }

    #[must_use]
    pub fn with_payload_refs(mut self, refs: Vec<JournalPayloadRef>) -> Self {
        self.payload_refs = refs;
        self
    }
}

/// Typed V2 journal payloads. Unknown kinds fail closed at decode time.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JournalPayload {
    RunCreated {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        launch_source: Option<String>,
    },
    StateChanged {
        previous: WorkflowState,
        new: WorkflowState,
        reason: String,
        actor: WorkflowActor,
    },
    InvocationStarted {
        invocation_id: String,
        call_index: u64,
        kind: WorkflowInvocationKind,
        /// Small inputs may remain inline; large ones use `payload_refs`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        canonical_input: Option<serde_json::Value>,
    },
    InvocationFinished {
        invocation_id: String,
        /// Outcome keeps usage, terminal status, and child_refs inline.
        outcome: WorkflowInvocationOutcome,
    },
    SwarmItemQueued {
        swarm_id: String,
        item_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        canonical_input: Option<serde_json::Value>,
    },
    SwarmItemStarted {
        swarm_id: String,
        item_id: String,
        invocation_id: String,
    },
    SwarmItemFinished {
        swarm_id: String,
        item_id: String,
        invocation_id: String,
        outcome: WorkflowInvocationOutcome,
    },
    SchemaRepairStarted {
        repair_id: String,
        invocation_id: String,
    },
    SchemaRepairFinished {
        repair_id: String,
        ok: bool,
        summary: String,
    },
    UserInputRequested {
        request_id: String,
        /// Request body: string prompt (legacy) or structured object with
        /// `prompt`, `answer_schema`, `default`, `title`, `answer_policy`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<serde_json::Value>,
    },
    UserInputAnswered {
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answer: Option<serde_json::Value>,
    },
    ArtifactCommitted {
        artifact_id: WorkflowArtifactId,
        sha256: String,
        byte_len: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        logical_name: Option<String>,
    },
    FinalResultRecorded {
        metadata: WorkflowFinalResultMetadata,
    },
    LineageSeedImported {
        lineage: WorkflowLineageMetadata,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prefix_digest: Option<String>,
    },
    RecoveryActionApplied {
        action: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quarantine_sha256: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        removed_bytes: Option<u64>,
    },
    /// Actual provider usage — always inline (never only behind a payload ref).
    UsageRecorded {
        usage: AgentTokenUsage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invocation_id: Option<String>,
    },
    ProvenanceRecorded {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        human_handle: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        definition_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        definition_revision: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invocation_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        swarm_item_id: Option<String>,
    },
    /// V3 generic child queued: spec is durable before dispatch.
    ChildQueued {
        child_key: WorkflowChildKey,
        child_kind: WorkflowChildKind,
        invocation_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase_id: Option<String>,
        spec_payload_ref: JournalPayloadRef,
    },
    /// V3 generic child started: binds runtime agent_id before live work.
    ChildStarted {
        child_key: WorkflowChildKey,
        agent_id: String,
    },
    /// V3 generic child finished: references the canonical outcome payload.
    ChildFinished {
        child_key: WorkflowChildKey,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        outcome_payload_ref: JournalPayloadRef,
    },
}

/// Validate envelope-local invariants (version, hash vs inline input).
pub fn validate_v2_envelope(envelope: &JournalEnvelope) -> Result<(), WorkflowError> {
    if envelope.version != JOURNAL_FORMAT_V2 && envelope.version != JOURNAL_FORMAT_V3 {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::JournalCorrupt,
            format!("unknown journal format version {}", envelope.version),
        ));
    }

    if let JournalPayload::InvocationStarted {
        invocation_id,
        canonical_input: Some(input),
        ..
    } = &envelope.payload
    {
        let Some(recorded) = envelope.canonical_input_hash.as_deref() else {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::JournalCorrupt,
                format!("canonical input hash missing for invocation {invocation_id}"),
            ));
        };
        let expected = canonical_input_hash(input);
        if recorded != expected {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::JournalCorrupt,
                format!("canonical input hash mismatch for invocation {invocation_id}"),
            ));
        }
    }

    // Payload refs must carry consistent sha256 identity.
    for pref in &envelope.payload_refs {
        if pref.sha256 != pref.artifact_id.as_content_sha256() {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::JournalCorrupt,
                format!(
                    "payload ref sha256 mismatch for role {}: ref={} artifact={}",
                    pref.role,
                    pref.sha256,
                    pref.artifact_id.as_content_sha256()
                ),
            ));
        }
    }

    Ok(())
}

#[must_use]
pub fn canonical_input_hash(input: &serde_json::Value) -> String {
    let canonical = canonicalize_json(input);
    let bytes = serde_json::to_vec(&canonical).expect("canonical json serializes");
    let hash = Sha256::digest(&bytes);
    format!("{hash:x}")
}

pub fn canonicalize_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            let sorted: serde_json::Map<String, serde_json::Value> = entries
                .into_iter()
                .map(|(k, v)| (k.clone(), canonicalize_json(v)))
                .collect();
            serde_json::Value::Object(sorted)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(canonicalize_json).collect())
        }
        other => other.clone(),
    }
}

// ---------------------------------------------------------------------------
// V1 writer (runtime path until V2 migration tasks land)
// ---------------------------------------------------------------------------

pub struct JournalWriter {
    file: std::fs::File,
    next_seq: u64,
    bytes_written: u64,
    started_invocations: HashSet<String>,
    finished_invocations: HashSet<String>,
}

impl JournalWriter {
    pub fn open(path: &Path) -> Result<Self, WorkflowError> {
        if let Some(parent) = path.parent() {
            atomic_file::ensure_safe_directory_tree(parent)
                .map_err(|e| WorkflowError::Journal(e.to_string()))?;
        }

        let created = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(file) => {
                file.sync_all()
                    .map_err(|e| WorkflowError::Journal(e.to_string()))?;
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
            Err(error) => return Err(WorkflowError::Journal(error.to_string())),
        };
        if created && let Some(parent) = path.parent() {
            atomic_file::sync_directory(parent)
                .map_err(|e| WorkflowError::Journal(e.to_string()))?;
        }

        // Bounded index scan — no full-journal Vec retention for open state.
        let index = if path.exists() {
            scan_journal_v1_index(path)?
        } else {
            JournalScanIndex::default()
        };
        let file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .map_err(|e| WorkflowError::Journal(e.to_string()))?;

        Ok(Self {
            file,
            next_seq: index.next_seq,
            bytes_written: index.bytes_scanned,
            started_invocations: index.started_invocations,
            finished_invocations: index.finished_invocations,
        })
    }

    pub fn append(
        &mut self,
        record: &JournalRecord,
        limits: &WorkflowLimits,
    ) -> Result<u64, WorkflowError> {
        validate_v1_record(
            record,
            self.next_seq,
            &self.started_invocations,
            &self.finished_invocations,
        )?;

        let line =
            serde_json::to_string(record).map_err(|e| WorkflowError::Journal(e.to_string()))?;
        let line_bytes = u64::try_from(line.len())
            .ok()
            .and_then(|bytes| bytes.checked_add(1))
            .ok_or_else(|| WorkflowError::Journal("record size overflow".to_owned()))?;

        if line_bytes > limits.journal_record_bytes {
            return Err(WorkflowError::JournalRecordLimitExceeded {
                observed: line_bytes,
                limit: limits.journal_record_bytes,
            });
        }

        if matches!(record, JournalRecord::InvocationStarted { .. })
            && !self.has_reservation_for_serialized_start(line_bytes, limits)
        {
            return Err(WorkflowError::JournalTotalLimitExceeded);
        }

        if self
            .bytes_written
            .checked_add(line_bytes)
            .is_none_or(|bytes| bytes > limits.journal_total_bytes)
        {
            return Err(WorkflowError::JournalTotalLimitExceeded);
        }

        self.file
            .write_all(line.as_bytes())
            .and_then(|()| self.file.write_all(b"\n"))
            .and_then(|()| self.file.sync_all())
            .map_err(|e| WorkflowError::Journal(e.to_string()))?;

        let seq = self.next_seq;
        observe_v1_record(
            record,
            &mut self.started_invocations,
            &mut self.finished_invocations,
        );
        self.next_seq = self
            .next_seq
            .checked_add(1)
            .ok_or_else(|| WorkflowError::Journal("journal sequence overflow".to_owned()))?;
        self.bytes_written = self
            .bytes_written
            .checked_add(line_bytes)
            .ok_or_else(|| WorkflowError::Journal("journal size overflow".to_owned()))?;
        Ok(seq)
    }

    pub fn has_reservation_for_invocation(
        &self,
        start: &JournalRecord,
        limits: &WorkflowLimits,
    ) -> Result<bool, WorkflowError> {
        validate_v1_record(
            start,
            self.next_seq,
            &self.started_invocations,
            &self.finished_invocations,
        )?;
        if !matches!(start, JournalRecord::InvocationStarted { .. }) {
            return Err(WorkflowError::Journal(
                "invocation reservation requires an invocation_started record".to_owned(),
            ));
        }

        let line =
            serde_json::to_string(start).map_err(|e| WorkflowError::Journal(e.to_string()))?;
        let line_bytes = u64::try_from(line.len())
            .ok()
            .and_then(|bytes| bytes.checked_add(1))
            .ok_or_else(|| WorkflowError::Journal("record size overflow".to_owned()))?;
        if line_bytes > limits.journal_record_bytes {
            return Ok(false);
        }
        Ok(self.has_reservation_for_serialized_start(line_bytes, limits))
    }

    fn has_reservation_for_serialized_start(
        &self,
        start_record_bytes: u64,
        limits: &WorkflowLimits,
    ) -> bool {
        limits
            .invocation_reservation_bytes(start_record_bytes)
            .and_then(|reservation| self.bytes_written.checked_add(reservation))
            .is_some_and(|bytes| bytes <= limits.journal_total_bytes)
    }

    #[must_use]
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    #[must_use]
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    #[must_use]
    pub fn has_incomplete_invocations(&self) -> bool {
        self.started_invocations
            .iter()
            .any(|id| !self.finished_invocations.contains(id))
    }
}

// ---------------------------------------------------------------------------
// V2 writer — sole owner of versioned appends
// ---------------------------------------------------------------------------

pub struct JournalV2Writer {
    file: std::fs::File,
    run_id: WorkflowId,
    next_seq: u64,
    bytes_written: u64,
    index: JournalScanIndex,
}

impl JournalV2Writer {
    /// Open or create a V2 journal bound to `run_id`.
    ///
    /// Applies torn-tail recovery (normalize valid unterminated final record or
    /// quarantine+truncate invalid EOF suffix) before indexing. Fail-closed
    /// corruption is not repaired.
    pub fn open(path: &Path, run_id: WorkflowId) -> Result<Self, WorkflowError> {
        if let Some(parent) = path.parent() {
            atomic_file::ensure_safe_directory_tree(parent)
                .map_err(|e| WorkflowError::Journal(e.to_string()))?;
        }

        let created = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(file) => {
                file.sync_all()
                    .map_err(|e| WorkflowError::Journal(e.to_string()))?;
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
            Err(error) => return Err(WorkflowError::Journal(error.to_string())),
        };
        if created && let Some(parent) = path.parent() {
            atomic_file::sync_directory(parent)
                .map_err(|e| WorkflowError::Journal(e.to_string()))?;
        }

        let report = if path.exists() && std::fs::metadata(path).map_or(0, |m| m.len()) > 0 {
            crate::workflow::recovery::recover_journal_v2(path, Some(&run_id))?
        } else {
            crate::workflow::recovery::JournalRecoveryReport {
                action: crate::workflow::recovery::JournalRecoveryAction::None,
                index: JournalScanIndex {
                    run_id: Some(run_id.clone()),
                    ..Default::default()
                },
                recovery_record_appended: false,
            }
        };
        Self::open_recovered(path, run_id, &report)
    }

    /// Open a journal after recovery has already been applied (no second recovery pass).
    pub fn open_recovered(
        path: &Path,
        run_id: WorkflowId,
        report: &crate::workflow::recovery::JournalRecoveryReport,
    ) -> Result<Self, WorkflowError> {
        let index = if report.index.run_id.is_some() || report.index.record_count > 0 {
            report.index.clone()
        } else if path.exists() && std::fs::metadata(path).map_or(0, |m| m.len()) > 0 {
            scan_journal_v2(path, Some(&run_id))?
        } else {
            JournalScanIndex {
                run_id: Some(run_id.clone()),
                ..Default::default()
            }
        };

        let file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .map_err(|e| WorkflowError::Journal(e.to_string()))?;

        Ok(Self {
            file,
            run_id,
            next_seq: index.next_seq,
            bytes_written: index.bytes_scanned,
            index,
        })
    }

    pub fn append(
        &mut self,
        envelope: &JournalEnvelope,
        limits: &WorkflowLimits,
    ) -> Result<u64, WorkflowError> {
        if envelope.version != JOURNAL_FORMAT_V2 {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::JournalCorrupt,
                format!("unknown journal format version {}", envelope.version),
            ));
        }
        if envelope.seq != self.next_seq {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::JournalCorrupt,
                format!(
                    "sequence gap: expected {}, got {}",
                    self.next_seq, envelope.seq
                ),
            ));
        }
        if envelope.run_id != self.run_id {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::JournalCorrupt,
                format!(
                    "run id mismatch: expected {}, got {}",
                    self.run_id.as_str(),
                    envelope.run_id.as_str()
                ),
            ));
        }
        validate_v2_envelope(envelope)?;
        // Apply observe against a temporary index clone for pre-write checks.
        let mut probe = self.index.clone();
        journal_scan::observe_v2_envelope(envelope, &mut probe)?;
        journal_scan::finalize_v2_index(&probe)?;

        let line =
            serde_json::to_string(envelope).map_err(|e| WorkflowError::Journal(e.to_string()))?;
        let line_bytes = u64::try_from(line.len())
            .ok()
            .and_then(|bytes| bytes.checked_add(1))
            .ok_or_else(|| WorkflowError::Journal("record size overflow".to_owned()))?;

        if line_bytes > limits.journal_record_bytes {
            return Err(WorkflowError::JournalRecordLimitExceeded {
                observed: line_bytes,
                limit: limits.journal_record_bytes,
            });
        }
        if self
            .bytes_written
            .checked_add(line_bytes)
            .is_none_or(|bytes| bytes > limits.journal_total_bytes)
        {
            return Err(WorkflowError::JournalTotalLimitExceeded);
        }

        self.file
            .write_all(line.as_bytes())
            .and_then(|()| self.file.write_all(b"\n"))
            .and_then(|()| self.file.sync_all())
            .map_err(|e| WorkflowError::Journal(e.to_string()))?;

        let seq = self.next_seq;
        self.index = probe;
        self.index.next_seq = self
            .next_seq
            .checked_add(1)
            .ok_or_else(|| WorkflowError::Journal("journal sequence overflow".to_owned()))?;
        self.index.bytes_scanned = self
            .bytes_written
            .checked_add(line_bytes)
            .ok_or_else(|| WorkflowError::Journal("journal size overflow".to_owned()))?;
        self.index.record_count = self.index.record_count.saturating_add(1);
        self.next_seq = self.index.next_seq;
        self.bytes_written = self.index.bytes_scanned;
        Ok(seq)
    }

    #[must_use]
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    #[must_use]
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    #[must_use]
    pub fn run_id(&self) -> &WorkflowId {
        &self.run_id
    }

    #[must_use]
    pub fn index(&self) -> &JournalScanIndex {
        &self.index
    }
}

/// Read a V1 journal via the streaming scanner (collects for small journals).
pub fn read_journal(path: &Path) -> Result<Vec<JournalRecord>, WorkflowError> {
    collect_journal_v1(path)
}

fn validate_v1_record(
    record: &JournalRecord,
    expected_seq: u64,
    started_invocations: &HashSet<String>,
    finished_invocations: &HashSet<String>,
) -> Result<(), WorkflowError> {
    if record.seq() != expected_seq {
        return Err(WorkflowError::Journal(format!(
            "sequence gap: expected {expected_seq}, got {}",
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
            if started_invocations.contains(invocation_id) {
                return Err(WorkflowError::Journal(format!(
                    "duplicate invocation_started for invocation {invocation_id}"
                )));
            }
        }
        JournalRecord::InvocationFinished { invocation_id, .. } => {
            if !started_invocations.contains(invocation_id) {
                return Err(WorkflowError::Journal(format!(
                    "invocation_finished without invocation_started for invocation {invocation_id}"
                )));
            }
            if finished_invocations.contains(invocation_id) {
                return Err(WorkflowError::Journal(format!(
                    "duplicate invocation_finished for invocation {invocation_id}"
                )));
            }
        }
        JournalRecord::StateChanged { .. } => {}
    }
    Ok(())
}

fn observe_v1_record(
    record: &JournalRecord,
    started_invocations: &mut HashSet<String>,
    finished_invocations: &mut HashSet<String>,
) {
    match record {
        JournalRecord::InvocationStarted { invocation_id, .. } => {
            started_invocations.insert(invocation_id.clone());
        }
        JournalRecord::InvocationFinished { invocation_id, .. } => {
            finished_invocations.insert(invocation_id.clone());
        }
        JournalRecord::StateChanged { .. } => {}
    }
}

#[derive(Debug)]
pub struct IncompleteInvocation {
    pub invocation_id: String,
    pub call_index: u64,
    pub kind: WorkflowInvocationKind,
    pub canonical_input_hash: String,
}

#[must_use]
pub fn find_incomplete_invocations(records: &[JournalRecord]) -> Vec<IncompleteInvocation> {
    let mut started: Vec<IncompleteInvocation> = Vec::new();
    let mut finished_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for record in records {
        match record {
            JournalRecord::InvocationFinished { invocation_id, .. } => {
                finished_ids.insert(invocation_id.as_str());
            }
            JournalRecord::InvocationStarted {
                invocation_id,
                call_index,
                kind,
                canonical_input_hash,
                ..
            } => {
                started.push(IncompleteInvocation {
                    invocation_id: invocation_id.clone(),
                    call_index: *call_index,
                    kind: *kind,
                    canonical_input_hash: canonical_input_hash.clone(),
                });
            }
            JournalRecord::StateChanged { .. } => {}
        }
    }

    started
        .into_iter()
        .filter(|inv| !finished_ids.contains(inv.invocation_id.as_str()))
        .collect()
}

/// Incomplete V2 starts (durable InvocationStarted without InvocationFinished).
#[must_use]
pub fn find_incomplete_invocations_v2(envelopes: &[JournalEnvelope]) -> Vec<IncompleteInvocation> {
    let mut started: Vec<IncompleteInvocation> = Vec::new();
    let mut finished_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for envelope in envelopes {
        match &envelope.payload {
            JournalPayload::InvocationFinished { invocation_id, .. } => {
                finished_ids.insert(invocation_id.as_str());
            }
            JournalPayload::InvocationStarted {
                invocation_id,
                call_index,
                kind,
                ..
            } => {
                started.push(IncompleteInvocation {
                    invocation_id: invocation_id.clone(),
                    call_index: *call_index,
                    kind: *kind,
                    canonical_input_hash: envelope.canonical_input_hash.clone().unwrap_or_default(),
                });
            }
            _ => {}
        }
    }

    started
        .into_iter()
        .filter(|inv| !finished_ids.contains(inv.invocation_id.as_str()))
        .collect()
}

pub fn write_run_metadata(
    dir: &Path,
    metadata: &super::state::WorkflowRunMetadata,
    limits: &WorkflowLimits,
) -> Result<PathBuf, WorkflowError> {
    atomic_file::ensure_safe_directory_tree(dir)
        .map_err(|e| WorkflowError::Journal(e.to_string()))?;
    let path = dir.join("run.json");
    let json = serde_json::to_string_pretty(metadata)
        .map_err(|e| WorkflowError::Journal(e.to_string()))?;

    if json.len() as u64 > limits.journal_record_bytes {
        return Err(WorkflowError::Journal(format!(
            "run.json size {} exceeds 16 MiB record limit",
            json.len()
        )));
    }

    match atomic_file::write_file_atomic_create_new(&path, json.as_bytes()) {
        Ok(atomic_file::AtomicWriteStatus::Durable) => Ok(path),
        Ok(atomic_file::AtomicWriteStatus::CommittedUnsynced(error)) => {
            Err(WorkflowError::Journal(error.to_string()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(
            WorkflowError::Journal(format!("run metadata already exists: {}", path.display())),
        ),
        Err(error) => Err(WorkflowError::Journal(error.to_string())),
    }
}

pub fn read_run_metadata(dir: &Path) -> Result<super::state::WorkflowRunMetadata, WorkflowError> {
    let path = dir.join("run.json");
    let content =
        std::fs::read_to_string(&path).map_err(|e| WorkflowError::Journal(e.to_string()))?;
    serde_json::from_str(&content).map_err(|e| WorkflowError::Journal(e.to_string()))
}

#[must_use]
pub fn run_dir(session_dir: &Path, run_id: &WorkflowId) -> PathBuf {
    session_dir.join("workflows").join(run_id.0.as_str())
}

#[must_use]
pub fn journal_path(session_dir: &Path, run_id: &WorkflowId) -> PathBuf {
    run_dir(session_dir, run_id).join("journal.jsonl")
}
