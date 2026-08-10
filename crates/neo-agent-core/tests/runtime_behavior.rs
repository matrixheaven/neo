//! Runtime behavior: turn execution, context/cache-prefix append-only, stream
//! assembly, thinking, tool dispatch, permissions, compaction, retry, and
//! plan/goal mode.

#[path = "runtime_behavior/compaction.rs"]
mod compaction;
#[path = "runtime_behavior/compaction_rehydration.rs"]
mod compaction_rehydration;
#[path = "runtime_behavior/context.rs"]
mod context;
#[path = "runtime_behavior/context_preflight.rs"]
mod context_preflight;
#[path = "runtime_behavior/fake_harness.rs"]
mod fake_harness;
#[path = "runtime_behavior/goal_mode.rs"]
mod goal_mode;
#[path = "runtime_behavior/model_switch.rs"]
mod model_switch;
#[path = "runtime_behavior/permissions.rs"]
mod permissions;
#[path = "runtime_behavior/permissions_mode.rs"]
mod permissions_mode;
#[path = "runtime_behavior/permissions_scope.rs"]
mod permissions_scope;
#[path = "runtime_behavior/plan_and_goal.rs"]
mod plan_and_goal;
#[path = "runtime_behavior/retry.rs"]
mod retry;
#[path = "runtime_behavior/streaming.rs"]
mod streaming;
#[path = "runtime_behavior/thinking.rs"]
mod thinking;
#[path = "runtime_behavior/tool_dispatch.rs"]
mod tool_dispatch;
#[path = "runtime_behavior/tool_dispatch_cancel.rs"]
mod tool_dispatch_cancel;
#[path = "runtime_behavior/tool_dispatch_edit.rs"]
mod tool_dispatch_edit;
#[path = "runtime_behavior/tool_dispatch_shell.rs"]
mod tool_dispatch_shell;
