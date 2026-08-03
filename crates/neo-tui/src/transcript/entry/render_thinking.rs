use super::{Style, ThinkingPart, ThinkingPhase, TuiTheme};
use crate::primitive::{Line, wrap_width};
use neo_ai::ThinkingKind;

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
    let wrapped = wrap_thinking_parts(parts, body_width);
    let total = wrapped.len();
    let summary = (kind == ThinkingKind::Summary).then(|| summary_projection(parts));
    let mut rows = Vec::new();

    if total == 0 {
        return rows;
    }

    if phase == ThinkingPhase::Streaming && !expanded {
        if let Some(summary) = summary.as_ref() {
            let label = summary.titles.last().map_or_else(
                || "thinking...".to_owned(),
                |title| format!("thinking · {title}"),
            );
            rows.push(Line::styled(
                format!("{} {label}", thinking_spinner(activity_frame)),
                style,
            ));
            return rows;
        }

        // Streaming: spinner + tail window.
        rows.push(Line::styled(
            format!("{} thinking...", thinking_spinner(activity_frame)),
            style,
        ));
        let start = total.saturating_sub(THINKING_PREVIEW_LINES);
        for line in wrapped.iter().skip(start) {
            rows.push(Line::styled(format!("  {line}"), style));
        }
        return rows;
    }

    if let Some(summary) = summary.as_ref()
        && !summary.titles.is_empty()
    {
        let wrapped_titles = summary
            .titles
            .iter()
            .map(|title| wrap_width(title, body_width))
            .collect::<Vec<_>>();
        let total = wrapped_titles.iter().map(Vec::len).sum::<usize>();
        let limit = if expanded {
            total
        } else {
            THINKING_PREVIEW_LINES.min(total)
        };
        let mut emitted = 0;
        for lines in &wrapped_titles {
            for line in lines {
                if emitted >= limit {
                    break;
                }
                let prefix = if emitted == 0 { "●" } else { "  " };
                rows.push(Line::styled(format!("{prefix} {line}"), style));
                emitted += 1;
            }
        }
        if !expanded && total > limit {
            rows.push(Line::styled(
                format!("  … {} more lines (ctrl+o to expand)", total - limit),
                Style::default().fg(theme.text_muted),
            ));
        }
        return rows;
    }

    if expanded {
        for (index, line) in wrapped.iter().enumerate() {
            let prefix = if index == 0 { "●" } else { "  " };
            rows.push(Line::styled(format!("{prefix} {line}"), style));
        }
        return rows;
    }

    // Complete: head window + collapse hint.
    let limit = THINKING_PREVIEW_LINES.min(total);
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
    titles: Vec<String>,
}

fn summary_projection(parts: &[ThinkingPart]) -> SummaryProjection {
    let mut parser = SummaryParser::default();
    for part in parts {
        parser.push(thinking_part_display_text(part));
    }
    parser.finish()
}

#[derive(Default)]
struct SummaryParser {
    titles: Vec<String>,
    fallback: Option<String>,
    line: String,
    in_title: bool,
    pending_star: bool,
    title: String,
}

impl SummaryParser {
    fn push(&mut self, text: &str) {
        self.push_fallback(text);
        for character in text.chars() {
            self.push_title_character(character);
        }
    }

    fn push_fallback(&mut self, text: &str) {
        if self.fallback.is_some() {
            return;
        }
        for character in text.chars() {
            if character == '\n' {
                self.finish_line();
                if self.fallback.is_some() {
                    break;
                }
            } else {
                self.line.push(character);
            }
        }
    }

    fn finish_line(&mut self) {
        if self.fallback.is_none() {
            let line = self.line.trim();
            if !line.is_empty() {
                self.fallback = Some(line.trim_matches('*').trim().to_owned());
            }
        }
        self.line.clear();
    }

    fn push_title_character(&mut self, character: char) {
        if self.in_title {
            if character == '*' {
                if self.pending_star {
                    self.add_closed_title();
                    self.in_title = false;
                    self.pending_star = false;
                } else {
                    self.pending_star = true;
                }
            } else {
                if self.pending_star {
                    self.title.push('*');
                    self.pending_star = false;
                }
                self.title.push(character);
            }
        } else if character == '*' {
            if self.pending_star {
                self.in_title = true;
                self.pending_star = false;
                self.title.clear();
            } else {
                self.pending_star = true;
            }
        } else {
            self.pending_star = false;
        }
    }

    fn add_closed_title(&mut self) {
        let title = self.title.trim();
        if !title.is_empty() && !self.titles.iter().any(|known| known == title) {
            self.titles.push(title.to_owned());
        }
        self.title.clear();
    }

    fn finish(mut self) -> SummaryProjection {
        if self.in_title {
            if self.pending_star {
                self.title.push('*');
            }
            self.add_closed_title();
        }
        self.finish_line();
        if self.titles.is_empty()
            && let Some(fallback) = self.fallback
        {
            self.titles.push(fallback);
        }
        SummaryProjection {
            titles: self.titles,
        }
    }
}

fn thinking_spinner(activity_frame: usize) -> char {
    const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    SPINNER[activity_frame % SPINNER.len()]
}

pub(super) fn thinking_style(theme: &TuiTheme) -> Style {
    Style::default().fg(theme.text_muted).italic()
}
