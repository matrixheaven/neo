//! Tool behavior: bash/task execution, file tools, tool names, output capture,
//! permission gating, schema descriptions, shell messages, and skills.

#[path = "tool_behavior/bash.rs"]
mod bash;
#[path = "tool_behavior/files.rs"]
mod files;
#[path = "tool_behavior/names.rs"]
mod names;
#[path = "tool_behavior/output_capture.rs"]
mod output_capture;
#[path = "tool_behavior/permissions.rs"]
mod permissions;
#[path = "tool_behavior/schema.rs"]
mod schema;
#[path = "tool_behavior/shell_messages.rs"]
mod shell_messages;
#[path = "tool_behavior/skills.rs"]
mod skills;
