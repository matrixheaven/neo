pub mod compaction;
pub mod events;
pub mod goal;
pub mod harness;
pub mod messages;
pub mod mode;
pub mod multi_agent;
pub mod oauth;
pub mod permissions;
pub mod rpc;
pub mod runtime;
pub mod session;
pub mod sidecar;
pub mod skills;
pub mod tools;
pub mod workflow;

pub use compaction::{
    CompactionError, CompactionSource, CompactionStrategy, can_split_after, compute_compact_count,
    generate_compaction_summary, render_messages_to_text,
};
pub use events::*;
pub use messages::*;
pub use mode::*;
pub use permissions::{
    ApprovalRuleStore, FileWriteApprovalOperation, PermissionApprovalDecision, PermissionMode,
    PermissionOperation, PrefixApprovalRule, SessionApprovalKey, SessionApprovalScope, ToolAccess,
    command_might_be_dangerous, is_known_safe_command,
};
pub use runtime::*;
pub use tools::*;

pub use neo_ai::NeoErrorInfo;
pub use neo_ai::error_info;
