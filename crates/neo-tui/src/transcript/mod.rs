pub mod diff_preview;
pub mod entry;
pub mod chrome_render;
mod approval_data;
mod event_handler;
pub mod pane;
pub mod partial_json;
pub mod plan_box;
pub mod shell_run;
pub mod store;
pub mod tool_call;
pub mod tool_group;
pub mod tool_renderers;

pub use entry::{
    ApprovalPromptData, BannerData, InlineImageRender, StatusSeverity, ThinkingPhase,
    TranscriptEntry,
};
pub use chrome_render::{
    CHROME_GUTTER, ChromeRender, apply_gutter, frame_content_width, render_chrome_lines,
    render_chrome_lines_mut, render_footer_only_lines,
};
pub use pane::TranscriptPane;
pub use plan_box::PlanBoxComponent;
pub use shell_run::{ShellRunComponent, ShellRunState};
pub use store::{TranscriptSelection, TranscriptStore, TranscriptViewport};
pub use tool_call::{ToolCallComponent, ToolCallState};
pub use tool_group::{ToolGroup, render_tool_group};
