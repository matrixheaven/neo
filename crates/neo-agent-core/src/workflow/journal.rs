use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::error::{WorkflowError, WorkflowErrorCode};
use super::limits::WorkflowLimits;
use super::state::{
    WorkflowActor, WorkflowArtifactId, WorkflowFinalResultMetadata, WorkflowId,
    WorkflowInvocationKind, WorkflowInvocationOutcome, WorkflowOutcomeStatus, WorkflowState,
};
use super::user_input::UserAnswerPolicy;
use crate::AgentTokenUsage;
use crate::session::atomic_file;

#[path = "journal_scan.rs"]
pub mod journal_scan;

pub use journal_scan::{
    JournalPage, JournalScanIndex, collect_journal, scan_journal, scan_journal_page,
};

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

/// Canonical journal envelope.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalEnvelope {
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

    /// Build a canonical envelope for `run_id` with the given payload.
    #[must_use]
    pub fn new(seq: u64, timestamp_ms: u64, run_id: WorkflowId, payload: JournalPayload) -> Self {
        Self {
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

/// Typed journal payloads. Unknown kinds fail closed at decode time.
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
        prompt: String,
        answer_schema: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        answer_policy: UserAnswerPolicy,
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
    /// Generic child queued: spec is durable before dispatch.
    ChildQueued {
        child_key: WorkflowChildKey,
        child_kind: WorkflowChildKind,
        invocation_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    /// Generic child started: binds runtime agent_id before live work.
    ChildStarted {
        child_key: WorkflowChildKey,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
    },
    /// Generic child finished: references the canonical outcome payload.
    ChildFinished {
        child_key: WorkflowChildKey,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        status: WorkflowOutcomeStatus,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actual_usage: Option<AgentTokenUsage>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

/// Validate envelope-local invariants.
pub fn validate_envelope(envelope: &JournalEnvelope) -> Result<(), WorkflowError> {
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

pub struct JournalWriter {
    file: std::fs::File,
    run_id: WorkflowId,
    next_seq: u64,
    bytes_written: u64,
    index: JournalScanIndex,
}

impl JournalWriter {
    /// Open or create the canonical journal bound to `run_id`.
    ///
    /// Applies torn-tail recovery (normalize valid unterminated final record or
    /// quarantine+truncate invalid EOF suffix) before indexing. Fail-closed
    /// corruption is not repaired.
    pub fn open(
        path: &Path,
        run_id: WorkflowId,
        limits: &WorkflowLimits,
    ) -> Result<Self, WorkflowError> {
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

        let index = crate::workflow::recovery::recover_journal(
            path,
            Some(&run_id),
            limits.journal_record_bytes,
            limits.journal_total_bytes,
        )?
        .index;

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
        validate_envelope(envelope)?;
        // Apply observe against a temporary index clone for pre-write checks.
        let mut probe = self.index.clone();
        journal_scan::observe_envelope(envelope, &mut probe)?;
        journal_scan::finalize_index(&probe)?;

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

#[derive(Debug)]
pub struct IncompleteInvocation {
    pub invocation_id: String,
    pub call_index: u64,
    pub kind: WorkflowInvocationKind,
    pub canonical_input_hash: String,
}

/// Durable invocation starts without a matching finish.
#[must_use]
pub fn find_incomplete_invocations(envelopes: &[JournalEnvelope]) -> Vec<IncompleteInvocation> {
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
