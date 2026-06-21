use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    Manual,
    Auto,
    Yolo,
}

impl PermissionMode {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Auto => "auto",
            Self::Yolo => "yolo",
        }
    }
}

#[allow(clippy::derivable_impls)]
impl Default for PermissionMode {
    fn default() -> Self {
        Self::Manual
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionApprovalDecision {
    AllowOnce,
    AllowForSession,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum PermissionOperation {
    FileRead,
    FileWrite,
    Shell,
    Tool,
    UserQuestion,
    PlanTransition,
    GoalTransition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct ToolAccess {
    pub file_read: bool,
    pub file_write: bool,
    pub shell: bool,
    pub tool: bool,
    pub user_question: bool,
}

impl ToolAccess {
    #[must_use]
    pub const fn none() -> Self {
        Self {
            file_read: false,
            file_write: false,
            shell: false,
            tool: false,
            user_question: false,
        }
    }

    #[must_use]
    pub const fn all() -> Self {
        Self {
            file_read: true,
            file_write: true,
            shell: true,
            tool: true,
            user_question: true,
        }
    }
}
