//! Torn-tail journal recovery and typed effect reconciliation (design §18–19).
//!
//! Recovery mutates the journal only for a proven final EOF suffix:
//! - valid unterminated JSON → append newline and sync
//! - invalid non-newline suffix → hash-addressed quarantine, then atomic replacement
//!
//! Newline-terminated invalid records, interior malformation, sequence gaps,
//! run-ID mismatch, and canonical hash mismatch fail closed without mutation.
//! Quarantine persistence failure leaves the journal byte-for-byte unchanged.
//!
//! Effect reconciliation never relaunches uncertain external work: the production
//! resolver may only adopt a proven terminal result, interrupt with host_exit,
//! or record a typed conflict.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::error::WorkflowError;
use super::journal::{
    JournalEnvelope, JournalPayload, JournalScanIndex,
    journal_scan::{JournalRecoveryPrefix, scan_recovery_prefix},
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

/// Result of scanning and optionally repairing the canonical journal's final EOF suffix.
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
    /// Conflicting or unverifiable result — preserve for diagnosis; do not choose heuristically.
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

/// Recover a journal's final EOF suffix, then return a validated scan index.
///
/// Scans only the final non-newline EOF suffix for normalize/quarantine. All
/// complete newline-terminated lines must already be valid; any interior or
/// newline-terminated failure is fail-closed corruption with no mutation.
pub fn recover_journal(
    path: &Path,
    expected_run_id: Option<&WorkflowId>,
    max_record_bytes: u64,
    max_total_bytes: u64,
) -> Result<JournalRecoveryReport, WorkflowError> {
    let JournalRecoveryPrefix {
        index,
        last_validated_offset,
        suffix,
        valid_suffix_seq,
    } = scan_recovery_prefix(path, expected_run_id, max_record_bytes, max_total_bytes)?;

    if suffix.is_empty() {
        return Ok(JournalRecoveryReport {
            action: JournalRecoveryAction::None,
            index,
            recovery_record_appended: false,
        });
    }

    // Final EOF suffix without a terminating newline.
    if let Some(seq) = valid_suffix_seq {
        normalize_unterminated_newline(path)?;
        return Ok(JournalRecoveryReport {
            action: JournalRecoveryAction::NormalizedUnterminated { seq },
            index,
            recovery_record_appended: false,
        });
    }

    quarantine_and_replace(
        path,
        expected_run_id,
        index,
        last_validated_offset,
        &suffix,
        max_record_bytes,
        max_total_bytes,
    )
}

// ---------------------------------------------------------------------------
// Mutations: normalize newline / quarantine + atomic replacement
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

fn quarantine_and_replace(
    path: &Path,
    expected_run_id: Option<&WorkflowId>,
    prefix_index: JournalScanIndex,
    last_validated_offset: u64,
    suffix: &[u8],
    max_record_bytes: u64,
    max_total_bytes: u64,
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

    let recovery_line = expected_run_id
        .filter(|_| prefix_index.run_created)
        .map(|run_id| {
            torn_tail_recovery_record_line(
                run_id,
                &sha,
                removed_bytes,
                last_validated_offset,
                prefix_index.next_seq,
                max_record_bytes,
            )
        })
        .transpose()?;
    let recovery_record_appended = recovery_line.is_some();

    let replacement = atomic_file::replace_existing_file_atomic_with_status(path, |temp| {
        let source = std::fs::File::open(path)?;
        let copied = io::copy(&mut source.take(last_validated_offset), temp)?;
        if copied != last_validated_offset {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!(
                    "journal ended at {copied} bytes while copying validated prefix of {last_validated_offset} bytes"
                ),
            ));
        }
        if let Some(line) = &recovery_line {
            temp.write_all(line)?;
            temp.write_all(b"\n")?;
        }
        Ok(())
    })
    .map_err(|error| WorkflowError::Journal(error.to_string()))?;
    if let atomic_file::AtomicWriteStatus::CommittedUnsynced(error) = replacement {
        return Err(WorkflowError::Journal(format!(
            "journal recovery committed but parent directory sync failed: {error}"
        )));
    };

    let index =
        scan_recovery_prefix(path, expected_run_id, max_record_bytes, max_total_bytes)?.index;

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

fn torn_tail_recovery_record_line(
    run_id: &WorkflowId,
    quarantine_sha256: &str,
    removed_bytes: u64,
    last_validated_offset: u64,
    next_seq: u64,
    max_record_bytes: u64,
) -> Result<Vec<u8>, WorkflowError> {
    let envelope = JournalEnvelope::new(
        next_seq,
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

    let line =
        serde_json::to_string(&envelope).map_err(|e| WorkflowError::Journal(e.to_string()))?;
    let line_bytes = u64::try_from(line.len())
        .ok()
        .and_then(|bytes| bytes.checked_add(1))
        .ok_or_else(|| WorkflowError::Journal("recovery record size overflow".to_owned()))?;
    if line_bytes > max_record_bytes {
        return Err(WorkflowError::JournalRecordLimitExceeded {
            observed: line_bytes,
            limit: max_record_bytes,
        });
    }
    Ok(line.into_bytes())
}

fn current_timestamp_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}
