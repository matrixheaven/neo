pub mod diff_preview;
pub mod entry;
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
pub use pane::{
    CHROME_GUTTER, ChromeRender, TranscriptPane, apply_gutter, frame_content_width,
    render_chrome_lines, render_chrome_lines_mut, render_footer_only_lines,
};
pub use plan_box::PlanBoxComponent;
pub use shell_run::{ShellRunComponent, ShellRunState};
pub use store::{TranscriptSelection, TranscriptStore, TranscriptViewport};
pub use tool_call::{ToolCallComponent, ToolCallState};
pub use tool_group::{ToolGroup, render_tool_group};
