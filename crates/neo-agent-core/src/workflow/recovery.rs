//! Torn-tail journal recovery and typed effect reconciliation (design §18–19).
//!
//! Recovery mutates the journal only for a proven final EOF suffix:
//! - valid unterminated JSON → append newline and sync
//! - invalid non-newline suffix → hash-addressed quarantine, then truncate
//!
//! Newline-terminated invalid records, interior malformation, sequence gaps,
//! run-ID mismatch, and canonical hash mismatch fail closed without mutation.
//! Quarantine persistence failure leaves the journal byte-for-byte unchanged.
//!
//! Effect reconciliation never relaunches uncertain external work: the production
//! resolver may only adopt a proven terminal result, interrupt with host_exit,
//! or record a typed conflict.

use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::error::{WorkflowError, WorkflowErrorCode};
use super::journal::{
    JOURNAL_FORMAT_V2, JournalEnvelope, JournalPayload, JournalScanIndex, JournalV2Writer,
    scan_journal_v2, validate_v2_envelope,
};
use super::state::{WorkflowId, WorkflowInvocationOutcome};
use crate::session::atomic_file;

/// Directory name under a run dir for hash-addressed torn-tail suffixes.
pub const RECOVERY_QUARANTINE_DIR: &str = "recovery-quarantine";

/// Action taken (or not taken) by journal recovery on the final EOF suffix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalRecoveryAction {
    /// Journal already ended on a validated boundary; no mutation.
    None,
    /// Valid final JSON lacked a terminating newline; newline was appended and synced.
    NormalizedUnterminated { seq: u64 },
    /// Invalid non-newline EOF suffix was quarantined, then truncated.
    TornTailQuarantined {
        quarantine_sha256: String,
        quarantine_path: PathBuf,
        removed_bytes: u64,
        last_validated_offset: u64,
    },
}

/// Result of scanning and optionally repairing a V2 journal's final EOF suffix.
#[derive(Debug, Clone)]
pub struct JournalRecoveryReport {
    pub action: JournalRecoveryAction,
    pub index: JournalScanIndex,
    /// True when a recovery record was appended after quarantine truncation.
    pub recovery_record_appended: bool,
}

/// Production-resolver decision for a durable start without a durable finish.
///
/// Never implies re-dispatch or relaunch of the external effect.
#[derive(Debug, Clone, PartialEq)]
pub enum EffectReconciliation {
    /// Exactly one proven terminal result was found; adopt it as the finish.
    AdoptProven { outcome: WorkflowInvocationOutcome },
    /// No terminal result — durable interrupted(host_exit); do not relaunch.
    InterruptHostExit,
    /// Conflicting or unverifiable result — keep inspectable; do not choose heuristically.
    Conflict { detail: String },
}

/// Classify a read-only lookup into a typed reconciliation decision.
///
/// `conflict` wins over a single candidate. Callers must never relaunch when the
/// decision is [`EffectReconciliation::InterruptHostExit`] or
/// [`EffectReconciliation::Conflict`].
#[must_use]
pub fn reconcile_incomplete_effect(
    proven: Option<WorkflowInvocationOutcome>,
    conflict: bool,
    conflict_detail: impl Into<String>,
) -> EffectReconciliation {
    if conflict {
        return EffectReconciliation::Conflict {
            detail: conflict_detail.into(),
        };
    }
    match proven {
        Some(outcome) => EffectReconciliation::AdoptProven { outcome },
        None => EffectReconciliation::InterruptHostExit,
    }
}

/// Path of the recovery-quarantine directory for a run.
#[must_use]
pub fn recovery_quarantine_dir(run_dir: &Path) -> PathBuf {
    run_dir.join(RECOVERY_QUARANTINE_DIR)
}

/// Content-addressed quarantine file for a torn-tail suffix.
#[must_use]
pub fn quarantine_tail_path(run_dir: &Path, sha256_hex: &str) -> PathBuf {
    recovery_quarantine_dir(run_dir).join(format!("{sha256_hex}.tail"))
}

/// Recover a V2 journal's final EOF suffix, then return a validated scan index.
///
/// Scans only the final non-newline EOF suffix for normalize/quarantine. All
/// complete newline-terminated lines must already be valid; any interior or
/// newline-terminated failure is fail-closed corruption with no mutation.
pub fn recover_journal_v2(
    path: &Path,
    expected_run_id: Option<&WorkflowId>,
) -> Result<JournalRecoveryReport, WorkflowError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(JournalRecoveryReport {
                action: JournalRecoveryAction::None,
                index: JournalScanIndex::default(),
                recovery_record_appended: false,
            });
        }
        Err(error) => return Err(WorkflowError::Journal(error.to_string())),
    };

    if bytes.is_empty() {
        let mut index = JournalScanIndex::default();
        if let Some(run_id) = expected_run_id {
            index.run_id = Some(run_id.clone());
        }
        return Ok(JournalRecoveryReport {
            action: JournalRecoveryAction::None,
            index,
            recovery_record_appended: false,
        });
    }

    let prefix = analyze_prefix(&bytes, expected_run_id)?;
    let suffix = &bytes[prefix.last_validated_offset as usize..];

    if suffix.is_empty() {
        // Clean newline-terminated journal (or empty after validated lines).
        let index = scan_journal_v2(path, expected_run_id)?;
        return Ok(JournalRecoveryReport {
            action: JournalRecoveryAction::None,
            index,
            recovery_record_appended: false,
        });
    }

    // Final EOF suffix without a terminating newline.
    match try_parse_valid_suffix(suffix, &prefix, expected_run_id) {
        Ok(envelope) => {
            normalize_unterminated_newline(path)?;
            let index = scan_journal_v2(path, expected_run_id)?;
            Ok(JournalRecoveryReport {
                action: JournalRecoveryAction::NormalizedUnterminated { seq: envelope.seq },
                index,
                recovery_record_appended: false,
            })
        }
        Err(SuffixParseFailure::NotValidRecord) => {
            let report = quarantine_and_truncate(
                path,
                expected_run_id,
                prefix.last_validated_offset,
                suffix,
            )?;
            Ok(report)
        }
        Err(SuffixParseFailure::Corrupt(error)) => Err(error),
    }
}

/// Open a V2 journal after applying torn-tail recovery for `run_id`.
pub fn open_recovered_journal_v2(
    path: &Path,
    run_id: WorkflowId,
) -> Result<(JournalV2Writer, JournalRecoveryReport), WorkflowError> {
    let report = recover_journal_v2(path, Some(&run_id))?;
    let writer = JournalV2Writer::open_recovered(path, run_id, &report)?;
    Ok((writer, report))
}

// ---------------------------------------------------------------------------
// Prefix analysis (complete newline-terminated lines only)
// ---------------------------------------------------------------------------

struct PrefixAnalysis {
    last_validated_offset: u64,
    next_seq: u64,
    run_id: Option<WorkflowId>,
}

fn analyze_prefix(
    bytes: &[u8],
    expected_run_id: Option<&WorkflowId>,
) -> Result<PrefixAnalysis, WorkflowError> {
    let mut offset = 0usize;
    let mut expected_seq = 0u64;
    let mut run_id: Option<WorkflowId> = expected_run_id.cloned();

    while offset < bytes.len() {
        let rest = &bytes[offset..];
        let Some(rel_nl) = rest.iter().position(|&b| b == b'\n') else {
            // Remainder is the non-newline EOF suffix — stop; caller handles it.
            break;
        };
        let line = &rest[..rel_nl];
        let line_end = offset + rel_nl + 1;

        if line.is_empty() {
            return Err(journal_corrupt("malformed record: empty journal line"));
        }

        let envelope = parse_envelope_line(line).map_err(|e| {
            // Newline-terminated invalid JSON / unknown kind → fail closed.
            journal_corrupt(format!("malformed or unknown journal record: {e}"))
        })?;
        validate_envelope_in_sequence(&envelope, expected_seq, &mut run_id, expected_run_id)?;

        expected_seq = expected_seq
            .checked_add(1)
            .ok_or_else(|| journal_corrupt("journal sequence overflow"))?;
        offset = line_end;
    }

    Ok(PrefixAnalysis {
        last_validated_offset: u64::try_from(offset)
            .map_err(|_| journal_corrupt("journal offset overflow"))?,
        next_seq: expected_seq,
        run_id,
    })
}

enum SuffixParseFailure {
    /// Partial / non-JSON / wrong shape at EOF — eligible for quarantine.
    NotValidRecord,
    /// Parsed enough to know this is durable corruption (seq/run/hash/version).
    Corrupt(WorkflowError),
}

fn try_parse_valid_suffix(
    suffix: &[u8],
    prefix: &PrefixAnalysis,
    expected_run_id: Option<&WorkflowId>,
) -> Result<JournalEnvelope, SuffixParseFailure> {
    if suffix.is_empty() {
        return Err(SuffixParseFailure::NotValidRecord);
    }
    // Empty-looking whitespace-only tails are still invalid partials.
    if suffix.iter().all(u8::is_ascii_whitespace) {
        return Err(SuffixParseFailure::NotValidRecord);
    }

    let envelope = match parse_envelope_line(suffix) {
        Ok(envelope) => envelope,
        Err(_) => return Err(SuffixParseFailure::NotValidRecord),
    };

    let mut run_id = prefix.run_id.clone();
    match validate_envelope_in_sequence(&envelope, prefix.next_seq, &mut run_id, expected_run_id) {
        Ok(()) => Ok(envelope),
        // Sequence / run / hash / version mismatch on a complete-looking JSON
        // object at EOF without newline is still corruption (not a torn partial).
        Err(error) if error.code() == WorkflowErrorCode::JournalCorrupt => {
            Err(SuffixParseFailure::Corrupt(error))
        }
        Err(error) => Err(SuffixParseFailure::Corrupt(error)),
    }
}

fn parse_envelope_line(line: &[u8]) -> Result<JournalEnvelope, String> {
    serde_json::from_slice(line).map_err(|e| e.to_string())
}

fn validate_envelope_in_sequence(
    envelope: &JournalEnvelope,
    expected_seq: u64,
    run_id: &mut Option<WorkflowId>,
    expected_run_id: Option<&WorkflowId>,
) -> Result<(), WorkflowError> {
    if envelope.version != JOURNAL_FORMAT_V2 {
        return Err(journal_corrupt(format!(
            "unknown journal format version {}",
            envelope.version
        )));
    }
    if envelope.seq != expected_seq {
        return Err(journal_corrupt(format!(
            "sequence gap: expected {expected_seq}, got {}",
            envelope.seq
        )));
    }
    match run_id {
        None => *run_id = Some(envelope.run_id.clone()),
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
    validate_v2_envelope(envelope)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Mutations: normalize newline / quarantine + truncate
// ---------------------------------------------------------------------------

fn normalize_unterminated_newline(path: &Path) -> Result<(), WorkflowError> {
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|e| WorkflowError::Journal(e.to_string()))?;
    file.write_all(b"\n")
        .and_then(|()| file.sync_all())
        .map_err(|e| WorkflowError::Journal(e.to_string()))?;
    if let Some(parent) = path.parent() {
        let _ = atomic_file::sync_directory(parent);
    }
    Ok(())
}

fn quarantine_and_truncate(
    path: &Path,
    expected_run_id: Option<&WorkflowId>,
    last_validated_offset: u64,
    suffix: &[u8],
) -> Result<JournalRecoveryReport, WorkflowError> {
    let run_dir = path.parent().ok_or_else(|| {
        WorkflowError::Journal("journal path has no parent run directory".to_owned())
    })?;

    let sha = format!("{:x}", Sha256::digest(suffix));
    let quarantine_path = quarantine_tail_path(run_dir, &sha);
    let removed_bytes = u64::try_from(suffix.len())
        .map_err(|_| WorkflowError::Journal("torn tail size overflow".to_owned()))?;

    // Persist quarantine first. Any failure here leaves the journal untouched.
    write_quarantine_file(&quarantine_path, suffix)?;

    // Truncate only after durable quarantine.
    truncate_journal(path, last_validated_offset)?;

    // Append a recovery record when a valid run identity is known and a prefix exists
    // or the journal is empty-but-bound. Empty journals after total-suffix quarantine
    // still get a seq-0 recovery record when run_id is known.
    let mut recovery_record_appended = false;
    let index = if let Some(run_id) = expected_run_id {
        match append_torn_tail_recovery_record(
            path,
            run_id,
            &sha,
            removed_bytes,
            last_validated_offset,
        ) {
            Ok(()) => {
                recovery_record_appended = true;
                scan_journal_v2(path, Some(run_id))?
            }
            Err(error) => {
                // Quarantine + truncate already committed; surface scan/append issues.
                // Prefer returning the post-truncate index when scan works.
                match scan_journal_v2(path, Some(run_id)) {
                    Ok(index) => {
                        let _ = error;
                        index
                    }
                    Err(_) => return Err(error),
                }
            }
        }
    } else {
        scan_journal_v2(path, None)?
    };

    Ok(JournalRecoveryReport {
        action: JournalRecoveryAction::TornTailQuarantined {
            quarantine_sha256: sha,
            quarantine_path,
            removed_bytes,
            last_validated_offset,
        },
        index,
        recovery_record_appended,
    })
}

fn write_quarantine_file(path: &Path, suffix: &[u8]) -> Result<(), WorkflowError> {
    let parent = path.parent().ok_or_else(|| {
        WorkflowError::Journal("quarantine path has no parent directory".to_owned())
    })?;
    atomic_file::ensure_safe_directory_tree(parent)
        .map_err(|e| WorkflowError::Journal(format!("quarantine directory failed: {e}")))?;

    // create_new: identical content-addressed tails may already exist after a prior crash.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(suffix)
                .and_then(|()| file.sync_all())
                .map_err(|e| {
                    // Best-effort cleanup of partial quarantine file; journal still intact.
                    let _ = std::fs::remove_file(path);
                    WorkflowError::Journal(format!("quarantine write failed: {e}"))
                })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // Verify existing quarantine matches the suffix bytes.
            let existing = std::fs::read(path).map_err(|e| {
                WorkflowError::Journal(format!("quarantine read existing failed: {e}"))
            })?;
            if existing != suffix {
                return Err(WorkflowError::Journal(
                    "quarantine path exists with different contents".to_owned(),
                ));
            }
        }
        Err(error) => {
            return Err(WorkflowError::Journal(format!(
                "quarantine create failed: {error}"
            )));
        }
    }

    atomic_file::sync_directory(parent)
        .map_err(|e| WorkflowError::Journal(format!("quarantine directory sync failed: {e}")))?;
    Ok(())
}

fn truncate_journal(path: &Path, last_validated_offset: u64) -> Result<(), WorkflowError> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| WorkflowError::Journal(e.to_string()))?;
    file.set_len(last_validated_offset)
        .and_then(|()| file.sync_all())
        .map_err(|e| WorkflowError::Journal(e.to_string()))?;
    if let Some(parent) = path.parent() {
        let _ = atomic_file::sync_directory(parent);
    }
    Ok(())
}

fn append_torn_tail_recovery_record(
    path: &Path,
    run_id: &WorkflowId,
    quarantine_sha256: &str,
    removed_bytes: u64,
    last_validated_offset: u64,
) -> Result<(), WorkflowError> {
    // Determine next seq from the truncated prefix without going through open()
    // recovery recursion: scan the (now clean) file.
    let index = if last_validated_offset == 0 {
        let mut empty = JournalScanIndex::default();
        empty.run_id = Some(run_id.clone());
        empty
    } else {
        scan_journal_v2(path, Some(run_id))?
    };

    let envelope = JournalEnvelope::new(
        index.next_seq,
        // Wall-clock is fine for recovery bookkeeping; sequence is the commit order.
        current_timestamp_ms(),
        run_id.clone(),
        JournalPayload::RecoveryActionApplied {
            action: "torn_tail_quarantined".to_owned(),
            detail: Some(serde_json::json!({
                "last_validated_offset": last_validated_offset,
            })),
            quarantine_sha256: Some(quarantine_sha256.to_owned()),
            removed_bytes: Some(removed_bytes),
        },
    );

    // Append without re-entering recover_journal_v2.
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|e| WorkflowError::Journal(e.to_string()))?;
    let line =
        serde_json::to_string(&envelope).map_err(|e| WorkflowError::Journal(e.to_string()))?;
    file.write_all(line.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(|e| WorkflowError::Journal(e.to_string()))?;
    let _ = file;
    if let Some(parent) = path.parent() {
        let _ = atomic_file::sync_directory(parent);
    }
    Ok(())
}

fn current_timestamp_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn journal_corrupt(message: impl Into<String>) -> WorkflowError {
    WorkflowError::coded(WorkflowErrorCode::JournalCorrupt, message)
}
