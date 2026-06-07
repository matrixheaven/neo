use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum PermissionDecision {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionPolicy {
    pub file_read: PermissionDecision,
    pub file_write: PermissionDecision,
    pub shell: PermissionDecision,
}

impl PermissionPolicy {
    #[must_use]
    pub const fn read_only() -> Self {
        Self {
            file_read: PermissionDecision::Allow,
            file_write: PermissionDecision::Deny,
            shell: PermissionDecision::Deny,
        }
    }

    #[must_use]
    pub const fn allow_all() -> Self {
        Self {
            file_read: PermissionDecision::Allow,
            file_write: PermissionDecision::Allow,
            shell: PermissionDecision::Allow,
        }
    }

    #[must_use]
    pub const fn deny_all() -> Self {
        Self {
            file_read: PermissionDecision::Deny,
            file_write: PermissionDecision::Deny,
            shell: PermissionDecision::Deny,
        }
    }

    #[must_use]
    pub const fn can_read_files(&self) -> bool {
        matches!(self.file_read, PermissionDecision::Allow)
    }

    #[must_use]
    pub const fn can_write_files(&self) -> bool {
        matches!(self.file_write, PermissionDecision::Allow)
    }

    #[must_use]
    pub const fn can_run_shell(&self) -> bool {
        matches!(self.shell, PermissionDecision::Allow)
    }
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        Self {
            file_read: PermissionDecision::Allow,
            file_write: PermissionDecision::Ask,
            shell: PermissionDecision::Ask,
        }
    }
}
