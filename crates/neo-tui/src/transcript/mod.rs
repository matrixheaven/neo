mod approval_data;
mod browser;
mod child_activity;
pub mod chrome_render;
mod delegate_card;
mod delegate_group;
pub mod diff_preview;
mod edit_tool_presentation;
pub mod entry;
mod event_handler;
mod instruction_card;
pub mod pane;
pub mod partial_json;
pub mod plan_box;
mod presentation;
pub mod shell_run;
mod shell_tool_presentation;
pub mod store;
mod streaming_prefix;
mod swarm_card;
pub mod tool_call;
pub mod tool_group;
pub mod tool_renderers;
mod workflow_card;
mod write_tool_presentation;

use neo_agent_core::multi_agent::{AgentLifecycleState, AgentSnapshot};

pub use browser::TranscriptBrowserState;
pub(crate) use child_activity::{
    MAX_CHILD_TOOL_ROWS, can_detach, child_activity_view, child_tool_status_text, compact_chars,
    display_elapsed, format_cache_token_usage, format_elapsed, format_token_count, one_line,
    render_child_body, render_child_final, render_child_thinking, render_child_tool_row,
    role_badge_style, role_label,
};
pub use chrome_render::{
    CHROME_GUTTER, ChromeRender, apply_gutter, frame_content_width, render_chrome_lines,
    render_chrome_lines_mut, render_footer_only_lines,
};
pub use delegate_card::DelegateCardComponent;
pub use delegate_group::DelegateGroupComponent;
pub use entry::{
    ApprovalDisplayState, ApprovalPromptData, BannerData, InlineImageRender, McpStartupPhase,
    McpStartupStatusData, StatusSeverity, ThinkingPhase, TranscriptEntry,
    TranscriptImageAttachment,
};
pub use instruction_card::InstructionCardComponent;
pub use pane::TranscriptPane;
pub use plan_box::PlanBoxComponent;
pub use presentation::{
    FinalizedBlock, FinalizedBlockProof, TranscriptBlockId, TranscriptTerminalUpdate,
};
pub use shell_run::{ShellRunComponent, ShellRunState};
pub use store::{TranscriptEntryId, TranscriptSelection, TranscriptStore, TranscriptViewport};
pub use swarm_card::SwarmCardComponent;
pub use tool_call::{ToolCallComponent, ToolCallState};
pub use tool_group::{ToolGroup, render_tool_group};
pub use workflow_card::WorkflowCardComponent;

pub(crate) fn interrupt_agent_snapshot(snapshot: &mut AgentSnapshot) -> bool {
    if snapshot.state.is_terminal() {
        return false;
    }
    let previous = snapshot.state;
    snapshot.previous_status = Some(previous);
    snapshot.state = AgentLifecycleState::Interrupted;
    snapshot.terminal_at_ms = Some(snapshot.updated_at_ms);
    snapshot
        .terminal_status_history
        .push(AgentLifecycleState::Interrupted);
    true
}
