use super::{Style, ThinkingPart, ThinkingPhase, TuiTheme};
use crate::primitive::{Line, wrap_width};
use neo_ai::ThinkingKind;

/// Number of thinking lines shown in the floating window (streaming) or as a
/// compact preview. Matches Neo's `THINKING_PREVIEW_LINES = 2`.
const THINKING_PREVIEW_LINES: usize = 2;

/// Total rows of a live, unexpanded thinking window: one spinner header plus
/// [`THINKING_PREVIEW_LINES`] tail rows.
const THINKING_WINDOW_ROWS: usize = 1 + THINKING_PREVIEW_LINES;

/// Render the thinking block as a fixed-height floating window.
///
/// - **Streaming Summary**: a braille-spinner title/status row plus reserved
///   placeholder rows. Summary body stays out of ordinary scrollback while the
///   part is live.
/// - **Streaming Full/Unknown**: a height-reserved window: a braille-spinner
///   header followed by the *last* [`THINKING_PREVIEW_LINES`] wrapped rows as a
///   scrolling tail. Rows not yet filled by streamed text stay as placeholders,
///   so the block keeps a constant height from the moment it goes live instead
///   of growing line by line and shifting the transcript.
/// - **Complete**: the *first* [`THINKING_PREVIEW_LINES`] rows prefixed with a
///   `●` bullet, followed by a `… N more lines (ctrl+o to expand)` hint when
///   the full text was longer. This keeps completed thinking compact instead
///   of unbounded.
fn render_thinking_parts(
    parts: &[ThinkingPart],
    width: usize,
    kind: ThinkingKind,
    phase: ThinkingPhase,
    expanded: bool,
    theme: &TuiTheme,
    activity_frame: usize,
) -> Vec<Line> {
    let style = thinking_style(theme);
    let body_width = width.max(1).saturating_sub(2).max(1);

    if kind == ThinkingKind::Summary {
        let summary = summary_projection(parts);
        if phase == ThinkingPhase::Streaming && !expanded {
            let label = summary.latest_title().map_or_else(
                || "thinking...".to_owned(),
                |title| format!("thinking · {title}"),
            );
            let mut rows = vec![Line::styled(
                format!("{} {label}", thinking_spinner(activity_frame)),
                style,
            )];
            reserve_streaming_rows(&mut rows, style);
            return rows;
        }

        let wrapped = summary.wrapped_lines(body_width);
        let total = wrapped.len();
        if total == 0 {
            return Vec::new();
        }

        let limit = if expanded {
            total
        } else {
            THINKING_PREVIEW_LINES.min(total)
        };
        let mut rows = Vec::new();
        for (index, line) in wrapped.iter().take(limit).enumerate() {
            let prefix = if index == 0 { "●" } else { "  " };
            rows.push(Line::styled(format!("{prefix} {line}"), style));
        }
        if !expanded && total > limit {
            rows.push(Line::styled(
                format!("  … {} more lines (ctrl+o to expand)", total - limit),
                Style::default().fg(theme.text_muted),
            ));
        }
        return rows;
    }

    let wrapped = wrap_thinking_parts(parts, body_width);
    let total = wrapped.len();

    if phase == ThinkingPhase::Streaming && !expanded {
        // Streaming: spinner + height-reserved tail window. The window keeps
        // its full height even before any text arrives so the transcript does
        // not jump while lines stream in.
        let mut rows = Vec::new();
        rows.push(Line::styled(
            format!("{} thinking...", thinking_spinner(activity_frame)),
            style,
        ));
        let start = total.saturating_sub(THINKING_PREVIEW_LINES);
        for line in wrapped.iter().skip(start) {
            rows.push(Line::styled(format!("  {line}"), style));
        }
        reserve_streaming_rows(&mut rows, style);
        return rows;
    }

    if total == 0 {
        return Vec::new();
    }

    if expanded {
        let mut rows = Vec::new();
        for (index, line) in wrapped.iter().enumerate() {
            let prefix = if index == 0 { "●" } else { "  " };
            rows.push(Line::styled(format!("{prefix} {line}"), style));
        }
        return rows;
    }

    // Complete: head window + collapse hint.
    let limit = THINKING_PREVIEW_LINES.min(total);
    let mut rows = Vec::new();
    for (index, line) in wrapped.iter().take(limit).enumerate() {
        let prefix = if index == 0 { "●" } else { "  " };
        rows.push(Line::styled(format!("{prefix} {line}"), style));
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

/// Pad a live streaming window with faint placeholder rows up to the reserved
/// height. Placeholders must carry a visible character: the pane trims blank
/// trailing rows from every entry block, so blank padding would collapse the
/// reservation and reintroduce height jitter.
fn reserve_streaming_rows(rows: &mut Vec<Line>, style: Style) {
    while rows.len() < THINKING_WINDOW_ROWS {
        rows.push(Line::styled("  ·", style));
    }
}

/// Wrap one renderer-local display stream while retaining canonical part boundaries.
fn wrap_thinking_parts(parts: &[ThinkingPart], width: usize) -> Vec<String> {
    let display_projection = parts
        .iter()
        .map(thinking_part_display_text)
        .filter(|text| !text.is_empty())
        .collect::<String>();
    if display_projection.is_empty() {
        Vec::new()
    } else {
        wrap_width(&display_projection, width)
    }
}

fn thinking_part_display_text(part: &ThinkingPart) -> &str {
    if part.text.is_empty() && part.redacted {
        super::REASONING_REDACTED_TEXT
    } else {
        &part.text
    }
}

pub(super) fn render_thinking_block(
    parts: &[ThinkingPart],
    kind: ThinkingKind,
    phase: ThinkingPhase,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
    activity_frame: usize,
) -> Vec<Line> {
    render_thinking_parts(parts, width, kind, phase, expanded, theme, activity_frame)
}

#[derive(Default)]
struct SummaryProjection {
    parts: Vec<SummaryPart>,
}

struct SummaryPart {
    title: Option<String>,
    body: String,
}

fn summary_projection(parts: &[ThinkingPart]) -> SummaryProjection {
    SummaryProjection {
        parts: parts
            .iter()
            .map(|part| parse_summary_part(thinking_part_display_text(part)))
            .collect(),
    }
}

fn parse_summary_part(text: &str) -> SummaryPart {
    let presentation = text.trim();
    let Some(after_open) = presentation.strip_prefix("**") else {
        return SummaryPart {
            title: None,
            body: summary_body(presentation),
        };
    };
    let Some(close_offset) = after_open.find("**") else {
        return SummaryPart {
            title: None,
            body: summary_body(presentation),
        };
    };

    let title = after_open[..close_offset].trim();
    if title.is_empty() {
        return SummaryPart {
            title: None,
            body: summary_body(presentation),
        };
    }

    SummaryPart {
        title: Some(title.to_owned()),
        body: summary_body(&after_open[close_offset + 2..]),
    }
}

fn summary_body(text: &str) -> String {
    let body = strip_summary_separator(text).trim_end();
    if body.trim() == "<!-- -->" {
        String::new()
    } else {
        body.to_owned()
    }
}

/// Remove the title/body separator without stripping indentation after a newline.
fn strip_summary_separator(text: &str) -> &str {
    let text = text.trim_start_matches([' ', '\t']);
    text.strip_prefix("\r\n")
        .or_else(|| text.strip_prefix('\n'))
        .or_else(|| text.strip_prefix('\r'))
        .unwrap_or(text)
}

impl SummaryProjection {
    fn latest_title(&self) -> Option<&str> {
        for part in self.parts.iter().rev() {
            if let Some(title) = part.title.as_deref() {
                return Some(title);
            }
            if !part.body.is_empty() {
                return None;
            }
        }
        None
    }

    fn wrapped_lines(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        let mut previous_title = None;

        for part in &self.parts {
            if let Some(title) = part.title.as_deref() {
                if previous_title != Some(title) {
                    lines.extend(wrap_width(title, width));
                }
                previous_title = Some(title);
            } else if !part.body.is_empty() {
                previous_title = None;
            }
            if !part.body.is_empty() {
                lines.extend(wrap_width(&part.body, width));
            }
        }

        lines
    }
}

fn thinking_spinner(activity_frame: usize) -> char {
    const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    SPINNER[activity_frame % SPINNER.len()]
}

pub(super) fn thinking_style(theme: &TuiTheme) -> Style {
    Style::default().fg(theme.text_muted).italic()
}
