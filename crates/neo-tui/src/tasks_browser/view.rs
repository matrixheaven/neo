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

/// Workflow projection fields for detail/list columns (TUI-only; never durable).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskBrowserWorkflowMeta {
    pub run_id: String,
    pub human_handle: Option<String>,
    pub definition_name: String,
    pub definition_revision: Option<String>,
    pub source_scope: Option<String>,
    pub current_phase: Option<String>,
    pub parent_run_id: Option<String>,
    pub admission_wait_reason: Option<String>,
    pub started_child_count: u64,
    pub queued_child_count: u64,
    pub terminal_child_count: u64,
    pub actual_usage_total: Option<u64>,
    pub has_final_result: bool,
    pub artifact_count: usize,
    pub pending_request_id: Option<String>,
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
        if let (Some(expected), Some(actual)) = (expected_query_hash, query_hash.as_deref()) {
            if expected != actual {
                return Err(
                    "list cursor query/filter does not match the active browser query".to_owned(),
                );
            }
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
