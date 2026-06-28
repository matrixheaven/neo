use super::{Style, ThinkingPhase, TuiTheme};
use crate::primitive::{Line, wrap_width};

/// Number of thinking lines shown in the floating window (streaming) or as a
/// compact preview. Matches Neo's `THINKING_PREVIEW_LINES = 2`.
const THINKING_PREVIEW_LINES: usize = 2;

/// Render the thinking block as a fixed-height floating window.
///
/// - **Streaming**: a braille-spinner header line
///   `⠋ thinking...` followed by the *last* `THINKING_PREVIEW_LINES` wrapped
///   rows. As new content streams in the window shows the tail, giving the
///   impression of text scrolling up within a fixed 2-line height.
/// - **Complete**: the *first* `THINKING_PREVIEW_LINES` rows prefixed with a
///   `●` bullet, followed by a `… N more lines (ctrl+o to expand)` hint when
///   the full text was longer. This keeps completed thinking compact instead
///   of unbounded.
pub(super) fn render_thinking(
    thinking: &str,
    width: usize,
    phase: ThinkingPhase,
    expanded: bool,
    theme: &TuiTheme,
    activity_frame: usize,
) -> Vec<Line> {
    let style = thinking_style(theme);
    let body_width = width.max(1).saturating_sub(2).max(1);
    let wrapped = wrap_width(thinking, body_width);
    let total = wrapped.len();
    let mut rows = Vec::new();

    if phase == ThinkingPhase::Streaming && !expanded {
        // Streaming: spinner + tail window.
        rows.push(Line::styled(
            format!("{} thinking...", thinking_spinner(activity_frame)),
            style,
        ));
        let start = total.saturating_sub(THINKING_PREVIEW_LINES);
        for line in &wrapped[start..] {
            rows.push(Line::styled(format!("  {line}"), style));
        }
        return rows;
    }

    if expanded {
        for (i, line) in wrapped.iter().enumerate() {
            if i == 0 {
                rows.push(Line::styled(format!("● {line}"), style));
            } else {
                rows.push(Line::styled(format!("  {line}"), style));
            }
        }
        return rows;
    }

    // Complete: head window + collapse hint.
    let limit = THINKING_PREVIEW_LINES.min(total);
    for (i, line) in wrapped.iter().take(limit).enumerate() {
        if i == 0 {
            rows.push(Line::styled(format!("● {line}"), style));
        } else {
            rows.push(Line::styled(format!("  {line}"), style));
        }
    }
    if total > limit {
        let remaining = total - limit;
        rows.push(Line::styled(
            format!("  … {remaining} more lines (ctrl+o to expand)"),
            Style::default().fg(theme.text_muted),
        ));
    }
    rows
}

pub(super) fn render_thinking_block(
    content: &str,
    phase: ThinkingPhase,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
    activity_frame: usize,
) -> Vec<Line> {
    if content.is_empty() {
        Vec::new()
    } else {
        render_thinking(content, width, phase, expanded, theme, activity_frame)
    }
}

fn thinking_spinner(activity_frame: usize) -> char {
    const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    SPINNER[activity_frame % SPINNER.len()]
}

pub(super) fn thinking_style(theme: &TuiTheme) -> Style {
    Style::default().fg(theme.text_muted).italic()
}
