use std::collections::HashSet;
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
    next_seq: u64,
    bytes_written: u64,
    started_invocations: HashSet<String>,
    finished_invocations: HashSet<String>,
}

impl JournalWriter {
    pub fn open(path: &Path) -> Result<Self, WorkflowError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| WorkflowError::Journal(e.to_string()))?;
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
            sync_parent_directory(parent)?;
        }

        let records = read_journal(path)?;
        let next_seq = u64::try_from(records.len())
            .map_err(|_| WorkflowError::Journal("journal sequence overflow".to_owned()))?;
        let bytes_written = std::fs::metadata(path)
            .map_err(|e| WorkflowError::Journal(e.to_string()))?
            .len();
        let (started_invocations, finished_invocations) = invocation_ids(&records);
        let file = std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .map_err(|e| WorkflowError::Journal(e.to_string()))?;

        Ok(Self {
            file,
            next_seq,
            bytes_written,
            started_invocations,
            finished_invocations,
        })
    }

    pub fn append(
        &mut self,
        record: &JournalRecord,
        limits: &WorkflowLimits,
    ) -> Result<u64, WorkflowError> {
        validate_record(
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
            return Err(WorkflowError::Journal(format!(
                "record size {} exceeds limit {}",
                line_bytes, limits.journal_record_bytes
            )));
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
        observe_record(
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
        validate_record(
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

pub fn read_journal(path: &Path) -> Result<Vec<JournalRecord>, WorkflowError> {
    let file = std::fs::File::open(path).map_err(|e| WorkflowError::Journal(e.to_string()))?;
    let mut reader = std::io::BufReader::new(file);
    let mut records = Vec::new();
    let mut expected_seq = 0u64;
    let mut started_invocations = HashSet::new();
    let mut finished_invocations = HashSet::new();

    loop {
        let mut line = Vec::new();
        let bytes_read = reader
            .read_until(b'\n', &mut line)
            .map_err(|e| WorkflowError::Journal(e.to_string()))?;
        if bytes_read == 0 {
            break;
        }
        if line.last() != Some(&b'\n') {
            return Err(WorkflowError::Journal(
                "truncated record: journal does not end with a newline".to_owned(),
            ));
        }
        line.pop();
        if line.is_empty() {
            return Err(WorkflowError::Journal(
                "malformed record: empty journal line".to_owned(),
            ));
        }
        let record: JournalRecord = serde_json::from_slice(&line)
            .map_err(|e| WorkflowError::Journal(format!("malformed record: {e}")))?;
        validate_record(
            &record,
            expected_seq,
            &started_invocations,
            &finished_invocations,
        )?;
        observe_record(&record, &mut started_invocations, &mut finished_invocations);
        expected_seq = expected_seq
            .checked_add(1)
            .ok_or_else(|| WorkflowError::Journal("journal sequence overflow".to_owned()))?;
        records.push(record);
    }
    Ok(records)
}

fn validate_record(
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

fn observe_record(
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

fn invocation_ids(records: &[JournalRecord]) -> (HashSet<String>, HashSet<String>) {
    let mut started = HashSet::new();
    let mut finished = HashSet::new();
    for record in records {
        observe_record(record, &mut started, &mut finished);
    }
    (started, finished)
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

    let temporary_path = dir.join(format!(".run.json.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
            .map_err(|e| WorkflowError::Journal(e.to_string()))?;
        file.write_all(json.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|e| WorkflowError::Journal(e.to_string()))?;
        drop(file);

        std::fs::hard_link(&temporary_path, &path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                WorkflowError::Journal(format!("run metadata already exists: {}", path.display()))
            } else {
                WorkflowError::Journal(error.to_string())
            }
        })?;
        sync_parent_directory(dir)?;
        Ok(path.clone())
    })();
    let _ = std::fs::remove_file(&temporary_path);
    result
}

#[cfg(unix)]
fn sync_parent_directory(dir: &Path) -> Result<(), WorkflowError> {
    std::fs::File::open(dir)
        .and_then(|file| file.sync_all())
        .map_err(|e| WorkflowError::Journal(e.to_string()))
}

#[cfg(not(unix))]
fn sync_parent_directory(_dir: &Path) -> Result<(), WorkflowError> {
    Ok(())
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
