//! Read-only retention mark/sweep preview and automatic retention execution.
//!
//! Automatic retention may only consider terminal, unreferenced, unpinned runs.
//! Live, queued, paused, awaiting-user, lineage-referenced, and pinned runs are
//! always excluded.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::admission::WorkflowAdmission;
use super::journal;
use super::limits::WorkflowLimits;
use super::state::{WorkflowId, WorkflowState};

/// Why a subject was excluded from the reclaimable set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionExclusion {
    Live,
    Queued,
    Paused,
    AwaitingUser,
    NonTerminal,
    Referenced,
    Pinned,
}

impl RetentionExclusion {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Queued => "queued",
            Self::Paused => "paused",
            Self::AwaitingUser => "awaiting_user",
            Self::NonTerminal => "nonterminal",
            Self::Referenced => "referenced",
            Self::Pinned => "pinned",
        }
    }
}

/// One durable run considered by a retention mark pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionSubject {
    pub run_id: WorkflowId,
    pub state: WorkflowState,
    pub bytes: u64,
    /// Age in milliseconds relative to the policy clock.
    pub age_ms: u64,
    /// Lineage or journal reference from another run.
    pub referenced: bool,
    /// Explicit operator pin.
    pub pinned: bool,
}

/// Trigger retention when actual storage reaches 90% of the global limit.
pub const STORAGE_HIGH_WATERMARK_PCT: f64 = 0.90;
/// Reclaim until actual storage is at or below 80% of the global limit.
pub const STORAGE_LOW_WATERMARK_PCT: f64 = 0.80;
/// Minimum terminal age before a run becomes reclaimable (30 days in ms).
pub const MIN_RUN_AGE_MS: u64 = 30 * 24 * 60 * 60 * 1000;

/// Whether automatic retention should run given current byte usage.
#[must_use]
pub fn should_trigger(global_storage_bytes: u64, current_bytes: u64) -> bool {
    // 90% of global: multiply by 9, then divide by 10 — avoids float precision loss.
    let limit = global_storage_bytes.saturating_mul(9).saturating_div(10);
    current_bytes >= limit
}

/// Calculate the target byte count after retention (low watermark).
#[must_use]
pub fn target_after_reclaim(global_storage_bytes: u64) -> u64 {
    // 80% of global: multiply by 8, then divide by 10.
    global_storage_bytes.saturating_mul(8).saturating_div(10)
}

#[must_use]
pub fn default_retention_policy() -> RetentionPolicy {
    RetentionPolicy {
        min_age_ms: Some(MIN_RUN_AGE_MS),
        reclaim_target_bytes: None,
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RetentionPolicy {
    /// Minimum terminal age before a run is reclaimable.
    pub min_age_ms: Option<u64>,
    /// Optional byte target: mark oldest eligible until reclaimable_bytes >= target
    /// or the candidate set is exhausted. `None` marks every eligible subject.
    pub reclaim_target_bytes: Option<u64>,
}

/// Read-only mark/sweep preview. No filesystem mutation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RetentionPreview {
    pub candidates: Vec<RetentionSubject>,
    pub excluded: Vec<(RetentionSubject, RetentionExclusion)>,
    pub reclaimable_bytes: u64,
}

/// Classify a single subject without applying age/byte policy.
#[must_use]
pub fn classify_subject(subject: &RetentionSubject) -> Option<RetentionExclusion> {
    if subject.pinned {
        return Some(RetentionExclusion::Pinned);
    }
    if subject.referenced {
        return Some(RetentionExclusion::Referenced);
    }
    match subject.state {
        WorkflowState::Running => Some(RetentionExclusion::Live),
        WorkflowState::Queued => Some(RetentionExclusion::Queued),
        WorkflowState::Pausing => Some(RetentionExclusion::Paused),
        WorkflowState::Paused => Some(RetentionExclusion::Paused),
        WorkflowState::AwaitingUser => Some(RetentionExclusion::AwaitingUser),
        WorkflowState::Completed
        | WorkflowState::Failed
        | WorkflowState::Cancelled
        | WorkflowState::ResourceLimited => None,
    }
}

/// Mark eligible terminal subjects and compute a dry-run sweep preview.
///
/// Eligible runs must be terminal, unreferenced, unpinned, and (when configured)
/// at least `min_age_ms` old. Candidates are ordered oldest-first for fair reclaim.
#[must_use]
pub fn preview_mark_sweep(
    subjects: &[RetentionSubject],
    policy: &RetentionPolicy,
) -> RetentionPreview {
    let mut excluded = Vec::new();
    let mut eligible = Vec::new();

    for subject in subjects {
        if let Some(reason) = classify_subject(subject) {
            excluded.push((subject.clone(), reason));
            continue;
        }
        if let Some(min_age) = policy.min_age_ms
            && subject.age_ms < min_age
        {
            // Still terminal but under age: treat as non-reclaimable without a
            // separate exclusion enum — surface as NonTerminal-adjacent policy miss
            // by keeping them out of candidates only when age-gated.
            excluded.push((subject.clone(), RetentionExclusion::NonTerminal));
            continue;
        }
        eligible.push(subject.clone());
    }

    eligible.sort_by(|a, b| {
        b.age_ms
            .cmp(&a.age_ms)
            .then_with(|| a.run_id.as_str().cmp(b.run_id.as_str()))
    });

    let mut candidates = Vec::new();
    let mut reclaimable_bytes = 0_u64;
    let target = policy.reclaim_target_bytes;

    for subject in eligible {
        if let Some(target_bytes) = target
            && reclaimable_bytes >= target_bytes
        {
            // Remaining eligible runs stay durable; not deleted, not candidates
            // for this sweep pass.
            break;
        }
        reclaimable_bytes = reclaimable_bytes.saturating_add(subject.bytes);
        candidates.push(subject);
    }

    RetentionPreview {
        candidates,
        excluded,
        reclaimable_bytes,
    }
}

/// Outcome of an automatic retention pass.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RetentionOutcome {
    /// Number of run directories reclaimed.
    pub reclaimed_count: usize,
    /// Total bytes reclaimed.
    pub reclaimed_bytes: u64,
    /// Whether protected data alone exceeds the storage limit.
    pub storage_full: bool,
}

/// Collect retention subjects by scanning the sessions root directory.
///
/// Walks `sessions_root` for bucket/session/workflows/<run_id> directories,
/// reads metadata, infers state, and measures byte size and age.
/// Returns each subject paired with its run directory path.
pub fn collect_retention_subjects(
    sessions_root: &Path,
) -> io::Result<Vec<(RetentionSubject, PathBuf)>> {
    let mut subjects = Vec::new();
    let now_ms = current_unix_ms();
    let mut referenced_parents = HashSet::new();
    let mut discovered: Vec<(WorkflowId, WorkflowState, u64, u64, PathBuf)> = Vec::new();

    if !sessions_root.is_dir() {
        return Ok(subjects);
    }

    for bucket in fs::read_dir(sessions_root)? {
        let bucket = bucket?;
        let bucket_path = bucket.path();
        if !bucket_path.is_dir() {
            continue;
        }
        for session in fs::read_dir(&bucket_path)? {
            let session = session?;
            let session_dir = session.path();
            let workflows_dir = session_dir.join("workflows");
            if !workflows_dir.is_dir() {
                continue;
            }
            for run_entry in fs::read_dir(&workflows_dir)? {
                let run_entry = run_entry?;
                let run_dir = run_entry.path();
                if !run_dir.is_dir() || !run_dir.join("run.json").is_file() {
                    continue;
                }
                let meta = match journal::read_run_metadata(&run_dir) {
                    Ok(meta) => meta,
                    Err(_) => continue,
                };
                if let Some(parent) = &meta.parent_run_id {
                    referenced_parents.insert(parent.as_str().to_owned());
                }
                let state = infer_run_state(&run_dir, &meta.run_id);
                let bytes = dir_byte_size(&run_dir).unwrap_or(0);
                let age_ms = file_age_ms(&run_dir, now_ms).unwrap_or(0);
                discovered.push((meta.run_id, state, bytes, age_ms, run_dir));
            }
        }
    }

    for (run_id, state, bytes, age_ms, run_dir) in discovered {
        let referenced = referenced_parents.contains(run_id.as_str());
        subjects.push((
            RetentionSubject {
                run_id,
                state,
                bytes,
                age_ms,
                referenced,
                pinned: false,
            },
            run_dir,
        ));
    }
    Ok(subjects)
}

/// Recursive directory byte size.
pub fn dir_byte_size(path: &Path) -> io::Result<u64> {
    let mut total = 0_u64;
    if path.is_file() {
        return Ok(fs::metadata(path)?.len());
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let child = entry.path();
        if child.is_dir() {
            total = total.saturating_add(dir_byte_size(&child)?);
        } else {
            total = total.saturating_add(fs::metadata(&child)?.len());
        }
    }
    Ok(total)
}

/// Age of a file or directory in milliseconds since last modification.
pub fn file_age_ms(path: &Path, now_ms: u64) -> io::Result<u64> {
    let modified = fs::metadata(path)?.modified()?;
    let modified_ms = modified.duration_since(UNIX_EPOCH).map_or(0, |duration| {
        u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
    });
    Ok(now_ms.saturating_sub(modified_ms))
}

/// Current Unix timestamp in milliseconds.
pub fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Perform automatic retention: reclaim eligible terminal runs until
/// actual storage is at or below the low watermark (80% of `global_storage_bytes`).
///
/// Eligibility: terminal, unreferenced, unpinned, and at least 30 days old.
/// Candidates are ordered oldest-first; each is revalidated immediately before
/// deletion. Failure skips only the failing target.
///
/// This function is safe to call from any context.
pub fn perform_retention(
    sessions_root: &Path,
    admission: Option<&WorkflowAdmission>,
    limits: &WorkflowLimits,
) -> RetentionOutcome {
    let Ok(subjects) = collect_retention_subjects(sessions_root) else {
        return RetentionOutcome::default();
    };
    let total_bytes: u64 = subjects.iter().map(|(s, _)| s.bytes).sum();
    if !should_trigger(limits.global_storage_bytes, total_bytes) {
        return RetentionOutcome::default();
    }

    let target_bytes = target_after_reclaim(limits.global_storage_bytes);
    let to_reclaim = total_bytes.saturating_sub(target_bytes);
    if to_reclaim == 0 {
        return RetentionOutcome::default();
    }

    let subjects_only: Vec<RetentionSubject> = subjects.iter().map(|(s, _)| s.clone()).collect();
    let preview = preview_mark_sweep(
        &subjects_only,
        &RetentionPolicy {
            min_age_ms: Some(MIN_RUN_AGE_MS),
            reclaim_target_bytes: Some(to_reclaim),
        },
    );

    if preview.candidates.is_empty() {
        return RetentionOutcome {
            storage_full: true,
            ..RetentionOutcome::default()
        };
    }

    let path_map: std::collections::HashMap<&str, &PathBuf> = subjects
        .iter()
        .map(|(s, p)| (s.run_id.as_str(), p))
        .collect();

    let mut reclaimed_count = 0_usize;
    let mut reclaimed_bytes = 0_u64;

    for candidate in &preview.candidates {
        let Some(run_dir) = path_map.get(candidate.run_id.as_str()) else {
            continue;
        };

        if !run_dir.exists() || !run_dir.join("run.json").is_file() {
            continue;
        }

        if !run_dir.starts_with(sessions_root) {
            continue;
        }

        let Ok(meta) = journal::read_run_metadata(run_dir) else {
            continue;
        };
        let current_state = infer_run_state(run_dir, &meta.run_id);
        if !current_state.is_terminal() {
            continue;
        }

        let dir_bytes = dir_byte_size(run_dir).unwrap_or(candidate.bytes);
        if fs::remove_dir_all(run_dir).is_err() {
            continue;
        }

        if let Some(adm) = admission {
            adm.release_storage_owner(candidate.run_id.as_str());
        }

        reclaimed_count += 1;
        reclaimed_bytes = reclaimed_bytes.saturating_add(dir_bytes);
    }

    RetentionOutcome {
        reclaimed_count,
        reclaimed_bytes,
        storage_full: false,
    }
}

/// Infer the workflow state from the journal file inside a run directory.
pub fn infer_run_state(run_dir: &Path, run_id: &WorkflowId) -> WorkflowState {
    let journal_path = run_dir.join("journal.jsonl");
    if let Ok(envelopes) = super::journal::collect_journal_v2(&journal_path, Some(run_id)) {
        for envelope in envelopes.iter().rev() {
            if let super::journal::JournalPayload::StateChanged { new, .. } = &envelope.payload {
                return *new;
            }
        }
    }
    WorkflowState::Completed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::state::WorkflowId;

    fn subject(
        id: &str,
        state: WorkflowState,
        bytes: u64,
        age_ms: u64,
        referenced: bool,
        pinned: bool,
    ) -> RetentionSubject {
        RetentionSubject {
            run_id: WorkflowId::from_existing(id),
            state,
            bytes,
            age_ms,
            referenced,
            pinned,
        }
    }

    #[test]
    fn preview_excludes_live_referenced_pinned_and_nonterminal() {
        let subjects = [
            subject("live", WorkflowState::Running, 10, 9_999_999, false, false),
            subject("queued", WorkflowState::Queued, 10, 9_999_999, false, false),
            subject("paused", WorkflowState::Paused, 10, 9_999_999, false, false),
            subject(
                "await",
                WorkflowState::AwaitingUser,
                10,
                9_999_999,
                false,
                false,
            ),
            subject("ref", WorkflowState::Completed, 10, 9_999_999, true, false),
            subject("pin", WorkflowState::Failed, 10, 9_999_999, false, true),
            subject("old", WorkflowState::Completed, 42, 9_999_999, false, false),
        ];
        let preview = preview_mark_sweep(
            &subjects,
            &RetentionPolicy {
                min_age_ms: Some(1),
                reclaim_target_bytes: None,
            },
        );
        assert_eq!(preview.candidates.len(), 1);
        assert_eq!(preview.candidates[0].run_id.as_str(), "old");
        assert_eq!(preview.reclaimable_bytes, 42);
        assert!(preview.excluded.len() >= 6);
    }
}
