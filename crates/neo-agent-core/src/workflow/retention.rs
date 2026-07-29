//! Read-only retention mark/sweep preview and automatic retention execution.
//!
//! Automatic retention may only consider terminal runs. Live, queued, paused,
//! and awaiting-user runs are always excluded.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::admission::WorkflowAdmission;
use super::journal;
use super::limits::WorkflowLimits;
use super::state::{WorkflowId, WorkflowState};
use crate::session::atomic_file;

/// Why a subject was excluded from the reclaimable set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionExclusion {
    Live,
    Queued,
    Paused,
    AwaitingUser,
    NonTerminal,
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
        }
    }
}

/// One durable run considered by a retention mark pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionSubject {
    pub run_id: WorkflowId,
    /// Validated journal state. `None` means the journal cannot prove a state.
    pub state: Option<WorkflowState>,
    pub bytes: u64,
    /// Exact durable terminal transition selected by the mark pass.
    pub terminal_timestamp_ms: Option<u64>,
    /// Age of the durable terminal transition; zero without terminal proof.
    pub age_ms: u64,
}

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
    match subject.state {
        Some(WorkflowState::Running) => Some(RetentionExclusion::Live),
        Some(WorkflowState::Queued) => Some(RetentionExclusion::Queued),
        Some(WorkflowState::Pausing | WorkflowState::Paused) => Some(RetentionExclusion::Paused),
        Some(WorkflowState::AwaitingUser) => Some(RetentionExclusion::AwaitingUser),
        Some(
            WorkflowState::Completed
            | WorkflowState::Failed
            | WorkflowState::Cancelled
            | WorkflowState::ResourceLimited,
        ) if subject.terminal_timestamp_ms.is_some() => None,
        Some(
            WorkflowState::Completed
            | WorkflowState::Failed
            | WorkflowState::Cancelled
            | WorkflowState::ResourceLimited,
        ) => Some(RetentionExclusion::NonTerminal),
        None => Some(RetentionExclusion::NonTerminal),
    }
}

/// Mark eligible terminal subjects and compute a dry-run sweep preview.
///
/// Eligible runs must be terminal and, when configured, at least `min_age_ms`
/// old. Candidates are ordered oldest-first for fair reclaim.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerminalProof {
    state: WorkflowState,
    timestamp_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct JournalStateProof {
    current_state: WorkflowState,
    terminal: Option<TerminalProof>,
}

#[derive(Debug, PartialEq, Eq)]
enum DirectoryIdentity {
    #[cfg(unix)]
    Unix { device: u64, inode: u64 },
    #[cfg(windows)]
    Windows(same_file::Handle),
    #[cfg(not(any(unix, windows)))]
    Portable(PathBuf),
}

fn journal_state_proof(
    run_dir: &Path,
    run_id: &WorkflowId,
    max_record_bytes: u64,
    max_total_bytes: u64,
) -> Option<JournalStateProof> {
    let journal_path = run_dir.join("journal.jsonl");
    if !is_plain_file(&journal_path) {
        return None;
    }
    if fs::symlink_metadata(&journal_path).ok()?.len() > max_total_bytes {
        return None;
    }
    let index = journal::scan_journal(
        &journal_path,
        Some(run_id),
        max_record_bytes,
        max_total_bytes,
    )
    .ok()?;
    let current_state = index.current_state?;
    let terminal = match (index.terminal_state, index.terminal_timestamp_ms) {
        (Some(state), Some(timestamp_ms)) if state.is_terminal() && state == current_state => {
            Some(TerminalProof {
                state,
                timestamp_ms,
            })
        }
        (None, None) if !current_state.is_terminal() => None,
        _ => return None,
    };
    Some(JournalStateProof {
        current_state,
        terminal,
    })
}

fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    atomic_file::is_reparse_or_symlink(metadata)
}

fn is_plain_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.file_type().is_dir() && !is_link_or_reparse(&metadata))
}

fn is_plain_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.file_type().is_file() && !is_link_or_reparse(&metadata))
}

fn canonical_retention_target(root: &Path, target: &Path) -> Option<PathBuf> {
    let relative = target.strip_prefix(root).ok()?;
    if relative.as_os_str().is_empty() {
        return None;
    }

    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return None;
        };
        current.push(name);
        if !is_plain_directory(&current) {
            return None;
        }
    }

    let canonical = fs::canonicalize(&current).ok()?;
    (canonical != root && canonical.starts_with(root)).then_some(canonical)
}

#[cfg(unix)]
fn directory_identity(path: &Path) -> Option<DirectoryIdentity> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_dir() || is_link_or_reparse(&metadata) {
        return None;
    }
    Some(DirectoryIdentity::Unix {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(windows)]
fn directory_identity(path: &Path) -> Option<DirectoryIdentity> {
    use std::os::windows::fs::OpenOptionsExt;
    use winapi::um::winbase::{FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT};
    use winapi::um::winnt::{FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE};

    let directory = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .ok()?;
    let metadata = directory.metadata().ok()?;
    if !metadata.is_dir() || is_link_or_reparse(&metadata) {
        return None;
    }
    Some(DirectoryIdentity::Windows(
        same_file::Handle::from_file(directory).ok()?,
    ))
}

#[cfg(not(any(unix, windows)))]
fn directory_identity(path: &Path) -> Option<DirectoryIdentity> {
    Some(DirectoryIdentity::Portable(fs::canonicalize(path).ok()?))
}

fn collect_retention_entries_at(
    sessions_root: &Path,
    max_record_bytes: u64,
    max_total_bytes: u64,
    now_ms: u64,
) -> io::Result<(Vec<(RetentionSubject, PathBuf, DirectoryIdentity)>, u64)> {
    let mut subjects = Vec::new();
    let mut total_bytes = 0_u64;

    if !is_plain_directory(sessions_root) {
        return Ok((subjects, total_bytes));
    }
    let sessions_root = fs::canonicalize(sessions_root)?;
    let quarantine_root = sessions_root.join(".workflow-retention-trash");
    if is_plain_directory(&quarantine_root) {
        total_bytes = total_bytes.saturating_add(dir_byte_size(&quarantine_root)?);
    }

    for bucket in fs::read_dir(&sessions_root)? {
        let bucket = bucket?;
        let bucket_path = bucket.path();
        if !is_plain_directory(&bucket_path) {
            continue;
        }
        for session in fs::read_dir(&bucket_path)? {
            let session = session?;
            let session_dir = session.path();
            if !is_plain_directory(&session_dir) {
                continue;
            }
            let workflows_dir = session_dir.join("workflows");
            if !is_plain_directory(&workflows_dir) {
                continue;
            }
            for run_entry in fs::read_dir(&workflows_dir)? {
                let run_entry = run_entry?;
                let run_dir = run_entry.path();
                let Some(run_dir) = canonical_retention_target(&sessions_root, &run_dir) else {
                    continue;
                };
                let bytes = dir_byte_size(&run_dir)?;
                total_bytes = total_bytes.saturating_add(bytes);
                let Some(identity) = directory_identity(&run_dir) else {
                    continue;
                };
                if !is_plain_file(&run_dir.join("run.json")) {
                    continue;
                }
                let meta = match journal::read_run_metadata(&run_dir) {
                    Ok(meta) => meta,
                    Err(_) => continue,
                };
                if run_dir.file_name().and_then(|name| name.to_str()) != Some(meta.run_id.as_str())
                {
                    continue;
                }
                let proof =
                    journal_state_proof(&run_dir, &meta.run_id, max_record_bytes, max_total_bytes);
                let state = proof.map(|proof| proof.current_state);
                let terminal_timestamp_ms = proof
                    .and_then(|proof| proof.terminal)
                    .map(|terminal| terminal.timestamp_ms);
                let age_ms = terminal_timestamp_ms
                    .map_or(0, |timestamp_ms| now_ms.saturating_sub(timestamp_ms));
                subjects.push((
                    RetentionSubject {
                        run_id: meta.run_id,
                        state,
                        bytes,
                        terminal_timestamp_ms,
                        age_ms,
                    },
                    run_dir,
                    identity,
                ));
            }
        }
    }
    Ok((subjects, total_bytes))
}

/// Recursive directory byte size.
pub fn dir_byte_size(path: &Path) -> io::Result<u64> {
    let metadata = fs::symlink_metadata(path)?;
    if is_link_or_reparse(&metadata) || metadata.file_type().is_file() {
        return Ok(metadata.len());
    }
    let mut total = 0_u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let child = entry.path();
        total = total.saturating_add(dir_byte_size(&child)?);
    }
    Ok(total)
}

/// Current Unix timestamp in milliseconds.
pub fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

fn revalidated_retention_target(
    sessions_root: &Path,
    discovered_path: &Path,
    discovered_identity: &DirectoryIdentity,
    candidate: &RetentionSubject,
    max_record_bytes: u64,
    max_total_bytes: u64,
) -> Option<PathBuf> {
    let run_dir = canonical_retention_target(sessions_root, discovered_path)?;
    if run_dir != discovered_path
        || directory_identity(&run_dir).as_ref() != Some(discovered_identity)
        || !is_plain_file(&run_dir.join("run.json"))
    {
        return None;
    }
    let meta = journal::read_run_metadata(&run_dir).ok()?;
    if meta.run_id != candidate.run_id
        || run_dir.file_name().and_then(|name| name.to_str()) != Some(candidate.run_id.as_str())
    {
        return None;
    }
    let terminal =
        journal_state_proof(&run_dir, &meta.run_id, max_record_bytes, max_total_bytes)?.terminal?;
    if candidate.state != Some(terminal.state)
        || candidate.terminal_timestamp_ms != Some(terminal.timestamp_ms)
        || current_unix_ms().saturating_sub(terminal.timestamp_ms) < MIN_RUN_AGE_MS
    {
        return None;
    }
    Some(run_dir)
}

fn quarantined_target_still_matches(
    run_dir: &Path,
    discovered_identity: &DirectoryIdentity,
    candidate: &RetentionSubject,
    limits: &WorkflowLimits,
) -> bool {
    if directory_identity(run_dir).as_ref() != Some(discovered_identity)
        || !is_plain_file(&run_dir.join("run.json"))
    {
        return false;
    }
    let Ok(meta) = journal::read_run_metadata(run_dir) else {
        return false;
    };
    if meta.run_id != candidate.run_id {
        return false;
    }
    let Some(terminal) = journal_state_proof(
        run_dir,
        &meta.run_id,
        limits.journal_record_bytes,
        limits.journal_total_bytes,
    )
    .and_then(|proof| proof.terminal) else {
        return false;
    };
    candidate.state == Some(terminal.state)
        && candidate.terminal_timestamp_ms == Some(terminal.timestamp_ms)
        && current_unix_ms().saturating_sub(terminal.timestamp_ms) >= MIN_RUN_AGE_MS
}

fn restore_quarantined_target(
    sessions_root: &Path,
    quarantine_path: &Path,
    original_path: &Path,
    expected_identity: &DirectoryIdentity,
) -> bool {
    let Some(original_parent) = original_path.parent() else {
        return false;
    };
    if original_path.exists()
        || !is_plain_directory(original_parent)
        || fs::canonicalize(original_parent)
            .ok()
            .is_none_or(|parent| !parent.starts_with(sessions_root))
        || fs::rename(quarantine_path, original_path).is_err()
    {
        return false;
    }
    if directory_identity(original_path).as_ref() != Some(expected_identity) {
        return false;
    }
    let _ = atomic_file::sync_directory(original_parent);
    true
}

fn cleanup_quarantine_root(
    quarantine_root: &Path,
    admission: Option<&WorkflowAdmission>,
) -> RetentionOutcome {
    if !is_plain_directory(quarantine_root)
        || fs::canonicalize(quarantine_root).ok().as_deref() != Some(quarantine_root)
    {
        return RetentionOutcome::default();
    }

    let mut outcome = RetentionOutcome::default();
    let Ok(entries) = fs::read_dir(quarantine_root) else {
        return outcome;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_plain_directory(&path) {
            continue;
        }
        let bytes = dir_byte_size(&path).unwrap_or(0);
        if fs::remove_dir_all(&path).is_err() {
            continue;
        }
        if let Some(run_id) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.split_once('.').map(|(run_id, _)| run_id))
            && let Some(admission) = admission
        {
            admission.release_storage_owner(run_id);
        }
        outcome.reclaimed_count = outcome.reclaimed_count.saturating_add(1);
        outcome.reclaimed_bytes = outcome.reclaimed_bytes.saturating_add(bytes);
    }
    let _ = atomic_file::sync_directory(quarantine_root);
    let _ = fs::remove_dir(quarantine_root);
    outcome
}

/// Perform automatic retention: reclaim eligible terminal runs until
/// actual storage is at or below the low watermark (80% of `global_storage_bytes`).
///
/// Eligibility: terminal and at least 30 days old.
/// Candidates are ordered oldest-first; each is revalidated immediately before
/// deletion. Failure skips only the failing target.
///
/// This function is safe to call from any context.
pub fn perform_retention(
    sessions_root: &Path,
    admission: Option<&WorkflowAdmission>,
    limits: &WorkflowLimits,
) -> RetentionOutcome {
    perform_retention_with_hook(sessions_root, admission, limits, |_, _| {})
}

fn perform_retention_with_hook(
    sessions_root: &Path,
    admission: Option<&WorkflowAdmission>,
    limits: &WorkflowLimits,
    mut before_revalidate: impl FnMut(&RetentionSubject, &Path),
) -> RetentionOutcome {
    if !is_plain_directory(sessions_root) {
        return RetentionOutcome::default();
    }
    let Ok(sessions_root) = fs::canonicalize(sessions_root) else {
        return RetentionOutcome::default();
    };
    let quarantine_root = sessions_root.join(".workflow-retention-trash");
    let mut outcome = cleanup_quarantine_root(&quarantine_root, admission);
    let Ok((subjects, total_bytes)) = collect_retention_entries_at(
        &sessions_root,
        limits.journal_record_bytes,
        limits.journal_total_bytes,
        current_unix_ms(),
    ) else {
        return outcome;
    };
    if !should_trigger(limits.global_storage_bytes, total_bytes) {
        outcome.storage_full = total_bytes > limits.global_storage_bytes;
        return outcome;
    }

    let target_bytes = target_after_reclaim(limits.global_storage_bytes);
    let to_reclaim = total_bytes.saturating_sub(target_bytes);
    if to_reclaim == 0 {
        return outcome;
    }

    let subjects_only: Vec<RetentionSubject> = subjects
        .iter()
        .map(|(subject, _, _)| subject.clone())
        .collect();
    let preview = preview_mark_sweep(
        &subjects_only,
        &RetentionPolicy {
            min_age_ms: Some(MIN_RUN_AGE_MS),
            reclaim_target_bytes: Some(to_reclaim),
        },
    );

    if preview.candidates.is_empty() {
        outcome.storage_full = total_bytes > limits.global_storage_bytes;
        return outcome;
    }

    let path_map: std::collections::HashMap<&str, (&PathBuf, &DirectoryIdentity)> = subjects
        .iter()
        .map(|(subject, path, identity)| (subject.run_id.as_str(), (path, identity)))
        .collect();

    for (candidate_index, candidate) in preview.candidates.iter().enumerate() {
        let Some((run_dir, discovered_identity)) = path_map.get(candidate.run_id.as_str()) else {
            continue;
        };
        before_revalidate(candidate, run_dir);

        let Some(run_dir) = revalidated_retention_target(
            &sessions_root,
            run_dir,
            discovered_identity,
            candidate,
            limits.journal_record_bytes,
            limits.journal_total_bytes,
        ) else {
            continue;
        };

        if fs::create_dir_all(&quarantine_root).is_err()
            || !is_plain_directory(&quarantine_root)
            || fs::canonicalize(&quarantine_root).ok().as_deref() != Some(quarantine_root.as_path())
        {
            continue;
        }
        let quarantine_path = quarantine_root.join(format!(
            "{}.{}.{}",
            candidate.run_id.as_str(),
            current_unix_ms(),
            candidate_index
        ));
        if quarantine_path.exists() || fs::rename(&run_dir, &quarantine_path).is_err() {
            continue;
        }
        if let Some(source_parent) = run_dir.parent() {
            let _ = atomic_file::sync_directory(source_parent);
        }
        let _ = atomic_file::sync_directory(&quarantine_root);

        if !quarantined_target_still_matches(
            &quarantine_path,
            discovered_identity,
            candidate,
            limits,
        ) {
            if directory_identity(&quarantine_path).as_ref() == Some(discovered_identity) {
                let _ = restore_quarantined_target(
                    &sessions_root,
                    &quarantine_path,
                    &run_dir,
                    discovered_identity,
                );
            }
            continue;
        }

        let dir_bytes = dir_byte_size(&quarantine_path).unwrap_or(candidate.bytes);
        if fs::remove_dir_all(&quarantine_path).is_err() {
            continue;
        }
        let _ = atomic_file::sync_directory(&quarantine_root);

        if let Some(adm) = admission {
            adm.release_storage_owner(candidate.run_id.as_str());
        }

        outcome.reclaimed_count = outcome.reclaimed_count.saturating_add(1);
        outcome.reclaimed_bytes = outcome.reclaimed_bytes.saturating_add(dir_bytes);
    }
    let _ = fs::remove_dir(&quarantine_root);

    let remaining_bytes = collect_retention_entries_at(
        &sessions_root,
        limits.journal_record_bytes,
        limits.journal_total_bytes,
        current_unix_ms(),
    )
    .map_or(total_bytes, |(_, bytes)| bytes);

    outcome.storage_full = remaining_bytes > limits.global_storage_bytes;
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::state::WorkflowId;

    #[cfg(unix)]
    fn create_directory_link(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).expect("create directory symlink");
    }

    #[cfg(windows)]
    fn create_directory_link(target: &Path, link: &Path) {
        std::os::windows::fs::symlink_dir(target, link).expect("create directory link");
    }

    fn subject(id: &str, state: WorkflowState, bytes: u64, age_ms: u64) -> RetentionSubject {
        RetentionSubject {
            run_id: WorkflowId::from_existing(id),
            state: Some(state),
            bytes,
            terminal_timestamp_ms: state.is_terminal().then_some(0),
            age_ms,
        }
    }

    #[test]
    fn preview_excludes_nonterminal_runs() {
        let subjects = [
            subject("live", WorkflowState::Running, 10, 9_999_999),
            subject("queued", WorkflowState::Queued, 10, 9_999_999),
            subject("paused", WorkflowState::Paused, 10, 9_999_999),
            subject("await", WorkflowState::AwaitingUser, 10, 9_999_999),
            subject("old", WorkflowState::Completed, 42, 9_999_999),
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
        assert_eq!(preview.excluded.len(), 4);
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn canonical_retention_target_rejects_intermediate_directory_link_escape() {
        let sessions = tempfile::tempdir().expect("sessions");
        let outside = tempfile::tempdir().expect("outside");
        let root = fs::canonicalize(sessions.path()).expect("canonical sessions");
        let outside_bucket = outside.path().join("bucket");
        let outside_run = outside_bucket
            .join("session")
            .join("workflows")
            .join("outside-run");
        fs::create_dir_all(&outside_run).expect("create outside run");
        let linked_bucket = root.join("linked-bucket");
        create_directory_link(&outside_bucket, &linked_bucket);

        let escaped = linked_bucket
            .join("session")
            .join("workflows")
            .join("outside-run");
        assert!(canonical_retention_target(&root, &escaped).is_none());
        assert!(outside_run.exists());
    }

    #[test]
    fn retention_delete_loop_revalidates_directory_identity_and_terminal_proof() {
        use crate::workflow::journal::{JournalEnvelope, JournalPayload, JournalWriter};
        use crate::workflow::{WorkflowActor, WorkflowRunMetadata};

        let sessions = tempfile::tempdir().expect("sessions");
        let root = fs::canonicalize(sessions.path()).expect("canonical sessions");
        let run_id = WorkflowId::from_existing("run-a");
        let run_dir = root
            .join("bucket")
            .join("session")
            .join("workflows")
            .join(run_id.as_str());
        fs::create_dir_all(&run_dir).expect("create run");
        let limits = WorkflowLimits {
            global_storage_bytes: 1,
            ..WorkflowLimits::default()
        };
        journal::write_run_metadata(
            &run_dir,
            &WorkflowRunMetadata {
                run_id: run_id.clone(),
                name: "retention".to_owned(),
                description: String::new(),
                phases: Vec::new(),
                script: String::new(),
                script_sha256: "abc".to_owned(),
                args: serde_json::json!({}),
                launch_source: "test".to_owned(),
                output_schema: None,
                display_name: None,
                input_schema: None,
                definition_origin: None,
                inline_unsaved: false,
            },
            &limits,
        )
        .expect("write metadata");
        let terminal_timestamp_ms = MIN_RUN_AGE_MS + 1;
        let mut writer =
            JournalWriter::open(&run_dir.join("journal.jsonl"), run_id.clone(), &limits)
                .expect("open journal");
        writer
            .append(
                &JournalEnvelope::new(
                    0,
                    1,
                    run_id.clone(),
                    JournalPayload::RunCreated {
                        name: "retention".to_owned(),
                        description: None,
                        launch_source: None,
                    },
                ),
                &limits,
            )
            .expect("append created");
        writer
            .append(
                &JournalEnvelope::new(
                    1,
                    terminal_timestamp_ms,
                    run_id.clone(),
                    JournalPayload::StateChanged {
                        previous: WorkflowState::Queued,
                        new: WorkflowState::Cancelled,
                        reason: "test".to_owned(),
                        actor: WorkflowActor::Runtime,
                    },
                ),
                &limits,
            )
            .expect("append terminal");
        drop(writer);

        #[cfg(unix)]
        fs::File::open(&run_dir)
            .expect("open run directory")
            .set_times(
                fs::FileTimes::new().set_modified(UNIX_EPOCH + std::time::Duration::from_millis(1)),
            )
            .expect("age run directory");
        let (just_terminalized, _) = collect_retention_entries_at(
            &root,
            limits.journal_record_bytes,
            limits.journal_total_bytes,
            terminal_timestamp_ms,
        )
        .expect("collect just-terminalized run");
        assert_eq!(just_terminalized.len(), 1);
        assert_eq!(just_terminalized[0].0.age_ms, 0);
        assert!(
            preview_mark_sweep(
                &[just_terminalized[0].0.clone()],
                &RetentionPolicy {
                    min_age_ms: Some(MIN_RUN_AGE_MS),
                    reclaim_target_bytes: None,
                },
            )
            .candidates
            .is_empty()
        );

        let candidate = RetentionSubject {
            run_id: run_id.clone(),
            state: Some(WorkflowState::Cancelled),
            bytes: 0,
            terminal_timestamp_ms: Some(terminal_timestamp_ms),
            age_ms: MIN_RUN_AGE_MS,
        };
        let identity = directory_identity(&run_dir).expect("directory identity");
        assert!(
            revalidated_retention_target(
                &root,
                &run_dir,
                &identity,
                &candidate,
                limits.journal_record_bytes,
                limits.journal_total_bytes,
            )
            .is_some()
        );

        let mut replaced = candidate.clone();
        replaced.terminal_timestamp_ms = Some(terminal_timestamp_ms + 1);
        assert!(
            revalidated_retention_target(
                &root,
                &run_dir,
                &identity,
                &replaced,
                limits.journal_record_bytes,
                limits.journal_total_bytes,
            )
            .is_none()
        );
        replaced = candidate;
        replaced.run_id = WorkflowId::from_existing("run-b");
        assert!(
            revalidated_retention_target(
                &root,
                &run_dir,
                &identity,
                &replaced,
                limits.journal_record_bytes,
                limits.journal_total_bytes,
            )
            .is_none()
        );

        let moved = root.join("collected-run");
        let hook_called = std::cell::Cell::new(false);
        let outcome = perform_retention_with_hook(&root, None, &limits, |subject, path| {
            assert_eq!(subject.run_id, run_id);
            assert_eq!(path, run_dir);
            hook_called.set(true);
            fs::rename(path, &moved).expect("move collected run");
            fs::create_dir(path).expect("replace run directory");
            fs::copy(moved.join("run.json"), path.join("run.json")).expect("copy metadata");
            fs::copy(moved.join("journal.jsonl"), path.join("journal.jsonl"))
                .expect("copy journal");
        });
        assert!(hook_called.get(), "candidate must enter the delete loop");
        assert_eq!(outcome.reclaimed_count, 0);
        assert!(run_dir.exists(), "replacement must fail closed");
        assert!(moved.exists(), "collected directory must not be deleted");
    }

    #[test]
    fn restore_quarantined_target_restores_matching_identity() {
        let temp = tempfile::tempdir().expect("root");
        let root = fs::canonicalize(temp.path()).expect("canonical root");
        let original = root.join("bucket/session/workflows/run-a");
        fs::create_dir_all(&original).expect("create run directory");
        fs::write(original.join("payload.bin"), b"payload").expect("write payload");
        let identity = directory_identity(&original).expect("directory identity");
        let quarantine = root.join(".workflow-retention-trash/run-a");
        fs::create_dir_all(quarantine.parent().expect("quarantine parent"))
            .expect("create quarantine");
        fs::rename(&original, &quarantine).expect("quarantine run");

        assert!(restore_quarantined_target(
            &root,
            &quarantine,
            &original,
            &identity,
        ));
        assert_eq!(directory_identity(&original), Some(identity));
        assert!(original.join("payload.bin").exists());
        assert!(!quarantine.exists());
    }

    #[cfg(windows)]
    #[test]
    fn directory_identity_accepts_non_utf8_windows_paths() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;

        let temp = tempfile::tempdir().expect("root");
        let path = temp.path().join(OsString::from_wide(&[0xd800]));
        fs::create_dir(&path).expect("create non-UTF-8 directory");

        assert!(directory_identity(&path).is_some());
    }
}
