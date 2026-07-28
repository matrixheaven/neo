//! Bounded child projection from workflow journal events (V2 + V3).
//! Stub: full implementation in Task 4.

use super::journal::{WorkflowChildKey, WorkflowChildKind};
use super::error::WorkflowError;
use super::state::WorkflowId;
use std::path::Path;

/// Observed child state after projecting journal events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowChildState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
    Recovering,
}

impl WorkflowChildState {
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}

/// A single child row projected from journal + live state.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowChildRow {
    pub key: WorkflowChildKey,
    pub child_kind: WorkflowChildKind,
    pub phase_id: Option<String>,
    pub agent_id: Option<String>,
    pub state: WorkflowChildState,
    pub title: Option<String>,
    pub role: Option<String>,
    pub queued_at_ms: Option<u64>,
    pub started_at_ms: Option<u64>,
    pub updated_at_ms: u64,
    pub terminal_at_ms: Option<u64>,
    pub terminal_summary: Option<String>,
    pub error_summary: Option<String>,
    pub actual_usage: Option<serde_json::Value>,
    pub latest_activity: Option<String>,
    pub generated_files: Vec<String>,
}

/// Ordered projection of all children from a journal.
#[derive(Debug, Default)]
pub struct ChildProjection {
    pub rows: Vec<WorkflowChildRow>,
    pub duplicate_keys: Vec<WorkflowChildKey>,
}

/// Project all child lifecycle events from a V2 or V3 journal.
/// Stub: returns empty projection. Full implementation in Task 4.
pub fn project_children(
    _journal_path: &Path,
    _expected_run_id: Option<&WorkflowId>,
) -> Result<ChildProjection, WorkflowError> {
    Ok(ChildProjection::default())
}
