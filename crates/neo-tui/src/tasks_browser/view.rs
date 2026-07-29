#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBrowserKind {
    Bash,
    Question,
    Delegate,
    DelegateSwarm,
    Workflow,
}

impl TaskBrowserKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Question => "question",
            Self::Delegate => "delegate",
            Self::DelegateSwarm => "delegate-swarm",
            Self::Workflow => "workflow",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBrowserStatus {
    Running,
    Waiting,
    Paused,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    ResourceLimited,
    ParentExited,
}

impl TaskBrowserStatus {
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::Waiting | Self::Paused)
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Paused => "paused",
            Self::Completed => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed out",
            Self::ResourceLimited => "resource limited",
            Self::ParentExited => "owner exited",
        }
    }

    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Running => "●",
            Self::Waiting => "◼",
            Self::Paused => "Ⅱ",
            Self::Completed => "✓",
            Self::Failed
            | Self::Cancelled
            | Self::TimedOut
            | Self::ResourceLimited
            | Self::ParentExited => "✕",
        }
    }

    #[must_use]
    pub const fn is_interrupted(self) -> bool {
        matches!(
            self,
            Self::Failed
                | Self::Cancelled
                | Self::TimedOut
                | Self::ResourceLimited
                | Self::ParentExited
        )
    }
}

/// Immutable Workflow Operator data supplied by the runtime projection.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskBrowserWorkflowMeta {
    pub run_id: String,
    pub display_name: String,
    pub purpose: String,
    pub elapsed_ms: u64,
    pub current_step_key: Option<WorkflowStepKey>,
    pub steps: Vec<TaskBrowserWorkflowStep>,
    pub child_page: TaskBrowserWorkflowChildPage,
    pub pending_user: Option<TaskBrowserPendingUserRequest>,
    pub inline_unsaved: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBrowserWorkflowRowState {
    Pending,
    Working,
    Completed,
    Failed,
    Paused,
    Recovering,
}

impl TaskBrowserWorkflowRowState {
    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Pending => "·",
            Self::Working => "›",
            Self::Completed => "✓",
            Self::Failed => "✕",
            Self::Paused => "Ⅱ",
            Self::Recovering => "↻",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskBrowserWorkflowStep {
    pub key: WorkflowStepKey,
    pub title: String,
    pub state: TaskBrowserWorkflowRowState,
    pub done_count: u64,
    pub working_count: u64,
    pub queued_count: u64,
    pub failed_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskBrowserWorkflowChildPage {
    pub items: Vec<TaskBrowserWorkflowChild>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub query_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskBrowserWorkflowChild {
    pub key: WorkflowChildKey,
    pub title: String,
    pub role: Option<String>,
    pub state: TaskBrowserWorkflowRowState,
    pub elapsed: String,
    pub actual_usage: Option<serde_json::Value>,
    pub latest_activity: Option<String>,
    pub terminal_summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskBrowserPendingUserRequest {
    pub request_id: String,
    pub prompt: String,
    pub answer_schema: serde_json::Value,
    pub default: Option<serde_json::Value>,
    pub title: Option<String>,
    pub answer_policy: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskBrowserItem {
    pub id: String,
    pub kind: TaskBrowserKind,
    pub status: TaskBrowserStatus,
    pub title: String,
    pub description: String,
    pub elapsed: String,
    pub detail_lines: Vec<String>,
    pub preview_lines: Vec<String>,
    pub can_stop: bool,
    pub human_handle: Option<String>,
    /// Opaque list cursor associated with this page (projection only).
    pub list_cursor: Option<String>,
    pub workflow: Option<TaskBrowserWorkflowMeta>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskBrowserSnapshot {
    pub items: Vec<TaskBrowserItem>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub query_hash: Option<String>,
    pub total_matched: Option<usize>,
}

impl TaskBrowserSnapshot {
    #[must_use]
    pub fn new(items: Vec<TaskBrowserItem>) -> Self {
        Self {
            items,
            next_cursor: None,
            has_more: false,
            query_hash: None,
            total_matched: None,
        }
    }

    #[must_use]
    pub fn items(&self) -> &[TaskBrowserItem] {
        &self.items
    }

    /// Apply a host-provided page while binding cursor to the query hash.
    ///
    /// Returns `Err` when a non-empty cursor is supplied with a mismatched
    /// `query_hash` (query-bound cursor rules, design §38.1).
    pub fn with_page(
        items: Vec<TaskBrowserItem>,
        next_cursor: Option<String>,
        has_more: bool,
        query_hash: Option<String>,
        total_matched: Option<usize>,
        expected_query_hash: Option<&str>,
    ) -> Result<Self, String> {
        if let (Some(expected), Some(actual)) = (expected_query_hash, query_hash.as_deref())
            && expected != actual
        {
            return Err(
                "list cursor query/filter does not match the active browser query".to_owned(),
            );
        }
        Ok(Self {
            items,
            next_cursor,
            has_more,
            query_hash,
            total_matched,
        })
    }
}
use neo_agent_core::workflow::{WorkflowChildKey, WorkflowStepKey};
