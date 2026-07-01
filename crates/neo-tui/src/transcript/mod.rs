mod approval_data;
mod child_activity;
pub mod chrome_render;
mod delegate_card;
mod delegate_group;
pub mod diff_preview;
pub mod entry;
mod event_handler;
pub mod pane;
pub mod partial_json;
pub mod plan_box;
pub mod shell_run;
pub mod store;
mod swarm_card;
pub mod tool_call;
pub mod tool_group;
pub mod tool_renderers;
mod workflow_card;

pub(crate) use child_activity::{
    MAX_CHILD_TOOL_ROWS, can_detach, child_activity_view, compact_chars, display_elapsed,
    format_cache_token_usage, format_elapsed, format_token_count, one_line, render_child_body,
    render_child_final, render_child_thinking, render_child_tool_row, role_label,
};
pub use chrome_render::{
    CHROME_GUTTER, ChromeRender, apply_gutter, frame_content_width, render_chrome_lines,
    render_chrome_lines_mut, render_footer_only_lines,
};
pub use delegate_card::DelegateCardComponent;
pub use delegate_group::DelegateGroupComponent;
pub use entry::{
    ApprovalPromptData, BannerData, InlineImageRender, StatusSeverity, ThinkingPhase,
    TranscriptEntry,
};
pub use pane::TranscriptPane;
pub use plan_box::PlanBoxComponent;
pub use shell_run::{ShellRunComponent, ShellRunState};
pub use store::{TranscriptSelection, TranscriptStore, TranscriptViewport};
pub use swarm_card::SwarmCardComponent;
pub use tool_call::{ToolCallComponent, ToolCallState};
pub use tool_group::{ToolGroup, render_tool_group};
pub use workflow_card::WorkflowCardComponent;
