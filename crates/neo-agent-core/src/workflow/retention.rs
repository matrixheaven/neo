//! Read-only retention mark/sweep preview.
//!
//! Automatic retention may only consider terminal, unreferenced, unpinned runs.
//! Live, queued, paused, awaiting-user, lineage-referenced, and pinned runs are
//! always excluded. This module never deletes; it only previews reclaimable work.

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

/// Host-configured automatic retention policy (user-global only).
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
