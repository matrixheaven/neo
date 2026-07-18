use neo_agent_core::StopReason;

use crate::dialogs::QuestionDisplayData;
use crate::widgets::TodoDisplayItem;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamUpdate {
    AssistantStarted {
        id: String,
    },
    TextDelta {
        text: String,
    },
    ToolStarted {
        id: String,
        name: String,
        detail: String,
    },
    ToolUpdated {
        id: String,
        detail: String,
    },
    ToolFinished {
        id: String,
        detail: String,
        success: bool,
        details: Option<serde_json::Value>,
    },
    ThinkingStarted,
    ThinkingDelta {
        text: String,
    },
    ThinkingFinished,
    Error {
        text: String,
    },
    TurnFinished,
    RunFinished {
        turn: u32,
        stop_reason: StopReason,
    },
    PlanModeChanged {
        active: bool,
    },
    TodoUpdated {
        todos: Vec<TodoDisplayItem>,
    },
    QuestionRequested {
        id: String,
        questions: Vec<QuestionDisplayData>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum ToolStatusKind {
    Pending,
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl ToolStatusKind {
    #[must_use]
    pub fn label(self) -> &'static str {
        ["pending", "queued", "running", "succeeded", "failed", "cancelled"][self as usize]
    }

    #[must_use]
    pub fn marker(self) -> &'static str {
        ["-", "q", "*", "+", "!", "x"][self as usize]
    }
}
