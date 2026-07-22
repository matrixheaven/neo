use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::error::WorkflowError;
use super::limits::WorkflowLimits;
use super::state::{
    WorkflowActor, WorkflowId, WorkflowInvocationKind, WorkflowInvocationOutcome, WorkflowState,
};

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

#[must_use]
pub fn canonical_input_hash(input: &serde_json::Value) -> String {
    let canonical = canonicalize_json(input);
    let bytes = serde_json::to_vec(&canonical).expect("canonical json serializes");
    let hash = Sha256::digest(&bytes);
    format!("{hash:x}")
}

fn canonicalize_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|(a, _), (b, _)| a.cmp(b));
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
    path: PathBuf,
    next_seq: u64,
    bytes_written: u64,
}

impl JournalWriter {
    pub fn open(path: &Path) -> Result<Self, WorkflowError> {
        let (next_seq, bytes_written) = if path.exists() {
            let records = read_journal(path)?;
            let next = records.last().map_or(0, |r| r.seq() + 1);
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            (next, size)
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| WorkflowError::Journal(e.to_string()))?;
            }
            std::fs::File::create(path).map_err(|e| WorkflowError::Journal(e.to_string()))?;
            (0, 0)
        };
        Ok(Self {
            path: path.to_path_buf(),
            next_seq,
            bytes_written,
        })
    }

    pub fn append(
        &mut self,
        record: &JournalRecord,
        limits: &WorkflowLimits,
    ) -> Result<u64, WorkflowError> {
        let line =
            serde_json::to_string(record).map_err(|e| WorkflowError::Journal(e.to_string()))?;
        let line_bytes = line.len() as u64 + 1; // +1 for newline

        if line_bytes > limits.journal_record_bytes {
            return Err(WorkflowError::Journal(format!(
                "record size {} exceeds limit {}",
                line_bytes, limits.journal_record_bytes
            )));
        }

        if self.bytes_written + line_bytes > limits.journal_total_bytes {
            return Err(WorkflowError::JournalTotalLimitExceeded);
        }

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.path)
            .map_err(|e| WorkflowError::Journal(e.to_string()))?;
        writeln!(file, "{line}").map_err(|e| WorkflowError::Journal(e.to_string()))?;
        file.flush()
            .map_err(|e| WorkflowError::Journal(e.to_string()))?;

        let seq = self.next_seq;
        self.next_seq += 1;
        self.bytes_written += line_bytes;
        Ok(seq)
    }

    pub fn has_reservation_for_invocation(&self, limits: &WorkflowLimits) -> bool {
        let reservation = limits.invocation_reservation_bytes();
        self.bytes_written + reservation <= limits.journal_total_bytes
    }

    #[must_use]
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    #[must_use]
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }
}

pub fn read_journal(path: &Path) -> Result<Vec<JournalRecord>, WorkflowError> {
    let file = std::fs::File::open(path).map_err(|e| WorkflowError::Journal(e.to_string()))?;
    let reader = std::io::BufReader::new(file);
    let mut records = Vec::new();
    let mut expected_seq = 0u64;

    for line in reader.lines() {
        let line = line.map_err(|e| WorkflowError::Journal(e.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        let record: JournalRecord = serde_json::from_str(&line)
            .map_err(|e| WorkflowError::Journal(format!("malformed record: {e}")))?;
        if record.seq() != expected_seq {
            return Err(WorkflowError::Journal(format!(
                "sequence gap: expected {expected_seq}, got {}",
                record.seq()
            )));
        }
        expected_seq += 1;
        records.push(record);
    }
    Ok(records)
}

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
    std::fs::create_dir_all(dir).map_err(|e| WorkflowError::Journal(e.to_string()))?;
    let path = dir.join("run.json");
    let json = serde_json::to_string_pretty(metadata)
        .map_err(|e| WorkflowError::Journal(e.to_string()))?;

    if json.len() as u64 > limits.journal_record_bytes {
        return Err(WorkflowError::Journal(format!(
            "run.json size {} exceeds 16 MiB record limit",
            json.len()
        )));
    }

    std::fs::write(&path, &json).map_err(|e| WorkflowError::Journal(e.to_string()))?;
    Ok(path)
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
