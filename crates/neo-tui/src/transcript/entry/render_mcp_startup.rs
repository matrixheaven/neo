use super::{McpStartupPhase, McpStartupStatusData, Style, TuiTheme};
use crate::primitive::Line;

pub(super) fn render_mcp_startup_status(
    data: &McpStartupStatusData,
    width: usize,
    theme: &TuiTheme,
    activity_frame: usize,
) -> Vec<Line> {
    let style = Style::default().fg(match data.phase {
        McpStartupPhase::Connecting => theme.status_pending,
        McpStartupPhase::Connected { .. } => theme.status_ok,
        McpStartupPhase::NeedsAuth { .. } | McpStartupPhase::Cancelled => theme.status_warn,
        McpStartupPhase::Failed { .. } => theme.status_error,
        McpStartupPhase::Disabled => theme.text_muted,
    });
    let text = match data.phase {
        McpStartupPhase::Connecting => {
            format!("{} {}", spinner(activity_frame), data.message())
        }
        McpStartupPhase::Failed { .. } => format!("✗ {}", data.message()),
        _ => data.message(),
    };
    super::styled_wrap(&text, width, style)
}

fn spinner(activity_frame: usize) -> char {
    const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    SPINNER[activity_frame % SPINNER.len()]
}
