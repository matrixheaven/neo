#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBrowserKind {
    Bash,
    Question,
    Delegate,
    DelegateSwarm,
}

impl TaskBrowserKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Question => "question",
            Self::Delegate => "delegate",
            Self::DelegateSwarm => "delegate-swarm",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBrowserStatus {
    Running,
    Waiting,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

impl TaskBrowserStatus {
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::Waiting)
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Completed => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed out",
        }
    }

    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Running => "●",
            Self::Waiting => "◼",
            Self::Completed => "✓",
            Self::Failed | Self::Cancelled | Self::TimedOut => "✕",
        }
    }

    #[must_use]
    pub const fn is_interrupted(self) -> bool {
        matches!(self, Self::Failed | Self::Cancelled | Self::TimedOut)
    }
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
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskBrowserSnapshot {
    items: Vec<TaskBrowserItem>,
}

impl TaskBrowserSnapshot {
    #[must_use]
    pub const fn new(items: Vec<TaskBrowserItem>) -> Self {
        Self { items }
    }

    #[must_use]
    pub fn items(&self) -> &[TaskBrowserItem] {
        &self.items
    }
}
