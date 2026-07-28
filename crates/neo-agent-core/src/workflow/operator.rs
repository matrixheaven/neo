//! Workflow Operator projection types for the TUI Workflow Operator view.
//!
//! These types define the data contract between the runtime background task
//! manager and the /tasks Workflow Operator overlay. Full projection logic
//! is wired in Tasks 5-6.

/// Key that identifies a workflow step for paging.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkflowStepKey {
    pub phase_id: Option<String>,
    pub phase_marker_sequence: u64,
}

/// Paged child rows for one step.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowChildPage {
    pub items: Vec<super::child_projection::WorkflowChildRow>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub query_hash: String,
}

/// Operator query request from the TUI.
#[derive(Debug, Clone)]
pub struct WorkflowOperatorRequest {
    pub step: Option<WorkflowStepKey>,
    pub cursor: Option<String>,
    pub limit: usize,
}

/// Immutable operator snapshot consumed by the TUI.
#[derive(Debug, Clone)]
pub struct WorkflowOperatorSnapshot {
    pub task_id: String,
    pub run_id: super::state::WorkflowId,
    pub display_name: String,
    pub purpose: String,
    pub state: super::state::WorkflowState,
    pub elapsed_ms: u64,
    pub updated_at_ms: u64,
    pub current_step_key: Option<WorkflowStepKey>,
    pub child_counts: ChildCounts,
    pub steps: Vec<WorkflowStepRow>,
    pub pending_user: Option<PendingUserRequest>,
    pub final_summary: Option<String>,
    pub failure_reason: Option<String>,
    pub generated_files: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChildCounts {
    pub done: u64,
    pub working: u64,
    pub queued: u64,
    pub failed: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowStepRow {
    pub key: WorkflowStepKey,
    pub title: String,
    pub order: u64,
    pub state: StepRowState,
    pub done_count: u64,
    pub working_count: u64,
    pub queued_count: u64,
    pub failed_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepRowState {
    Pending,
    Active,
    Completed,
    Failed,
    Paused,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingUserRequest {
    pub request_id: String,
    pub prompt: String,
    pub answer_schema: Option<serde_json::Value>,
    pub default: Option<serde_json::Value>,
    pub title: Option<String>,
    pub answer_policy: String,
}
