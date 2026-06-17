use crate::ansi::{Color, Style, paint, visible_width};
use crate::app::TuiTheme;
use crate::core::{Finalization, Line};
use crate::widgets::box_draw;
use crate::wrap_width;

/// Rich welcome-banner content rendered as a rounded box (matching kimi-code).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BannerData {
    pub title: String,
    pub subtitle: String,
    pub directory: String,
    pub session: String,
    pub model: String,
    pub version: String,
    pub mcp: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEntry {
    Banner(BannerData),
    User(String),
    Assistant {
        thinking: String,
        content: String,
        finalized: bool,
    },
    ToolCallRunning {
        name: String,
        detail: String,
    },
    ToolCallFinished {
        name: String,
        detail: String,
    },
    Notice {
        text: String,
        /// When set, the notice renders as a bold title (in the severity
        /// color) for system/error notices. Plain notices stay a single dim
        /// line with no prefix.
        severity: Option<NoticeSeverity>,
    },
}

/// Severity for an emphasized system notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeSeverity {
    Info,
    Warning,
    Error,
}

impl TranscriptEntry {
    #[must_use]
    pub fn banner(title: impl Into<String>) -> Self {
        Self::Banner(BannerData {
            title: title.into(),
            ..BannerData::default()
        })
    }

    #[must_use]
    pub fn welcome_banner(data: BannerData) -> Self {
        Self::Banner(data)
    }

    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self::User(content.into())
    }

    #[must_use]
    pub fn assistant_live(content: impl Into<String>) -> Self {
        Self::Assistant {
            thinking: String::new(),
            content: content.into(),
            finalized: false,
        }
    }

    #[must_use]
    pub fn assistant_final(content: impl Into<String>) -> Self {
        Self::Assistant {
            thinking: String::new(),
            content: content.into(),
            finalized: true,
        }
    }

    #[must_use]
    pub fn tool_call_running(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::ToolCallRunning {
            name: name.into(),
            detail: detail.into(),
        }
    }

    #[must_use]
    pub fn tool_call_finished(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::ToolCallFinished {
            name: name.into(),
            detail: detail.into(),
        }
    }

    #[must_use]
    pub fn notice(content: impl Into<String>) -> Self {
        Self::Notice {
            text: content.into(),
            severity: None,
        }
    }

    #[must_use]
    pub fn notice_severity(content: impl Into<String>, severity: NoticeSeverity) -> Self {
        Self::Notice {
            text: content.into(),
            severity: Some(severity),
        }
    }

    #[must_use]
    pub fn finalization(&self) -> Finalization {
        match self {
            Self::Banner(_)
            | Self::User(_)
            | Self::Notice { .. }
            | Self::ToolCallFinished { .. } => Finalization::Finalized,
            Self::Assistant { finalized, .. } if *finalized => Finalization::Finalized,
            Self::Assistant { .. } | Self::ToolCallRunning { .. } => Finalization::Live,
        }
    }

    #[must_use]
    pub fn render(&self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        // Every `Line` returned here MUST map to exactly one terminal row:
        // content is split on `\n` and soft-wrapped to `width` so no line ever
        // carries an embedded newline. The renderer's diff/scroll math treats
        // each `Vec<String>` entry as one screen row, so an un-split long line
        // would corrupt the coordinate model and garble streaming output.
        let inner_width = width.max(1);
        match self {
            Self::Banner(data) => render_welcome_banner(data, inner_width, theme),
            // User: no "You" label — a sparkle bullet (roleUser amber) on the
            // first line, continuation lines indented to align after the
            // bullet (kimi-code style).
            Self::User(content) => {
                let style = Style::default().fg(theme.user);
                bulleted_wrap(content, inner_width, "✨ ", style)
            }
            // Notice: plain dim single-line for routine notices; a bold
            // severity-colored title for system/error notices.
            Self::Notice { text, severity } => match severity {
                None => styled_wrap(text, inner_width, notice_style(theme)),
                Some(sev) => {
                    let style = Style::default().fg(severity_color(*sev, theme)).bold();
                    styled_wrap(text, inner_width, style)
                }
            },
            Self::Assistant {
                thinking,
                content,
                finalized,
            } => {
                let mut rows = Vec::new();
                if *finalized && content.is_empty() && thinking.is_empty() {
                    return rows;
                }
                if !thinking.is_empty() {
                    rows.extend(render_thinking(thinking, inner_width, *finalized, theme));
                }
                if !content.is_empty() {
                    // Assistant body is rendered as markdown. On finalization
                    // the first line carries a magenta `● ` bullet and every
                    // continuation line is indented to align under the body
                    // (not under the bullet glyph) — matching kimi-code.
                    if *finalized {
                        rows.extend(crate::markdown::render_markdown(
                            content,
                            inner_width,
                            theme,
                            "● ",
                            "  ",
                        ));
                    } else {
                        // Streaming: no bullet, plain indent.
                        rows.extend(crate::markdown::render_markdown(
                            content,
                            inner_width,
                            theme,
                            "",
                            "",
                        ));
                    }
                }
                rows
            }
            Self::ToolCallRunning { name, detail } => styled_wrap(
                &format!("● Using {name} ({detail})"),
                inner_width,
                tool_running_style(theme),
            ),
            Self::ToolCallFinished { name, detail } => styled_wrap(
                &format!("● Used {name} ({detail})"),
                inner_width,
                tool_finished_style(theme),
            ),
        }
    }
}

/// Wrap `text` and apply a bullet prefix to the first row, indenting the rest
/// to align under the body (prefix width of spaces). This is the kimi-code
/// "bullet + indented continuation" layout used by user/assistant messages.
fn bulleted_wrap(text: &str, width: usize, prefix: &str, style: Style) -> Vec<Line> {
    let indent = " ".repeat(visible_width(prefix));
    let mut rows = Vec::new();
    for (i, line) in wrap_width(text, width.max(1)).into_iter().enumerate() {
        if i == 0 {
            rows.push(Line::styled(format!("{prefix}{line}"), style));
        } else {
            rows.push(Line::styled(format!("{indent}{line}"), style));
        }
    }
    if rows.is_empty() {
        rows.push(Line::styled(prefix.to_owned(), style));
    }
    rows
}

#[allow(clippy::needless_pass_by_value)]
fn severity_color(severity: NoticeSeverity, theme: &TuiTheme) -> Color {
    match severity {
        NoticeSeverity::Info => theme.accent,
        NoticeSeverity::Warning => theme.warning,
        NoticeSeverity::Error => theme.danger,
    }
}

/// Number of thinking lines shown in the floating window (live) or as a
/// finalized preview. Matches kimi-code's `THINKING_PREVIEW_LINES = 2`.
const THINKING_PREVIEW_LINES: usize = 2;

/// Render the thinking block as a fixed-height floating window.
///
/// - **Live** (`finalized == false`): a braille-spinner header line
///   `⠋ thinking...` followed by the *last* `THINKING_PREVIEW_LINES` wrapped
///   rows. As new content streams in the window shows the tail, giving the
///   impression of text scrolling up within a fixed 2-line height.
/// - **Finalized**: the *first* `THINKING_PREVIEW_LINES` rows prefixed with a
///   `●` bullet, followed by a `… N more lines (ctrl+o to expand)` hint when
///   the full text was longer. This keeps finalized thinking compact instead
///   of unbounded.
fn render_thinking(thinking: &str, width: usize, finalized: bool, theme: &TuiTheme) -> Vec<Line> {
    let style = thinking_style(theme);
    let body_width = width.max(1).saturating_sub(2).max(1);
    let wrapped = wrap_width(thinking, body_width);
    let total = wrapped.len();
    let mut rows = Vec::new();

    if !finalized {
        // Live: spinner + tail window.
        rows.push(Line::styled("⠋ thinking...", style));
        let start = total.saturating_sub(THINKING_PREVIEW_LINES);
        for line in &wrapped[start..] {
            rows.push(Line::styled(format!("  {line}"), style));
        }
        return rows;
    }

    // Finalized: head window + collapse hint.
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
            Style::default().fg(theme.muted),
        ));
    }
    rows
}

/// Wrap `text` to `width` and emit each wrapped row as a styled [`Line`].
fn styled_wrap(text: &str, width: usize, style: Style) -> Vec<Line> {
    wrap_width(text, width.max(1))
        .into_iter()
        .map(|line| Line::styled(line, style))
        .collect()
}

/// Render the welcome banner as a rounded box with an ASCII-art logo and
/// aligned metadata, matching kimi-code's `welcome.ts`.
///
/// Layout:
/// ```text
/// ╭──────╮
/// │      │
/// │  ▐█▛█▛█▌  Welcome to Neo!
/// │  ▐█████▌  Send /help for help.
/// │      │
/// │  Directory: /path
/// │  Session:   abc
/// │  Model:     GLM
/// │  ...
/// │      │
/// ╰──────╯
/// ```
fn render_welcome_banner(data: &BannerData, width: usize, theme: &TuiTheme) -> Vec<Line> {
    use std::fmt::Write as _;
    let logo = [
        "\u{2590}\u{2588}\u{259b}\u{2588}\u{259b}\u{2588}\u{258c}",
        "\u{2590}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{258c}",
    ];
    let gap = "  ";

    // Build the content lines (plain text with ANSI via paint, to be padded).
    let logo_color = Style::default().fg(theme.accent);
    let title_style = Style::default().fg(theme.accent).bold();
    let subtitle_style = Style::default().fg(theme.muted);
    let label_style = Style::default().fg(theme.muted).bold();
    let value_style = Style::default().fg(theme.header);

    let mut content: Vec<String> = Vec::new();
    // blank line at top of box
    content.push(String::new());
    // logo + title / subtitle
    let mut line0 = String::new();
    let _ = write!(line0, "{}{}", paint(logo[0], logo_color), gap);
    let mut rest0 = String::new();
    if !data.title.is_empty() {
        rest0.push_str(&paint(&data.title, title_style));
    }
    content.push(format!("{line0}{rest0}"));
    let mut line1 = String::new();
    let _ = write!(line1, "{}{}", paint(logo[1], logo_color), gap);
    let mut rest1 = String::new();
    if !data.subtitle.is_empty() {
        rest1.push_str(&paint(&data.subtitle, subtitle_style));
    }
    content.push(format!("{line1}{rest1}"));
    // blank line between logo and metadata
    content.push(String::new());

    // Metadata rows: label padded to a fixed width so colons align.
    let label_w = 11usize;
    let mut meta: Vec<(&str, &str)> = Vec::new();
    if !data.directory.is_empty() {
        meta.push(("Directory:", data.directory.as_str()));
    }
    if !data.session.is_empty() {
        meta.push(("Session:", data.session.as_str()));
    }
    if !data.model.is_empty() {
        meta.push(("Model:", data.model.as_str()));
    }
    if !data.version.is_empty() {
        meta.push(("Version:", data.version.as_str()));
    }
    if let Some(m) = &data.mcp {
        meta.push(("MCP:", m.as_str()));
    }
    for (label, value) in &meta {
        let mut label_padded = label.to_string();
        while visible_width(&label_padded) < label_w {
            label_padded.push(' ');
        }
        let mut row = String::new();
        let _ = write!(
            row,
            "{} {}",
            paint(&label_padded, label_style),
            paint(value, value_style)
        );
        content.push(row);
    }
    // blank line at bottom of box
    content.push(String::new());

    let border_style = Style::default().fg(theme.accent);
    let mut rows = Vec::new();
    rows.push(Line::raw(box_draw::top_border(width, border_style)));
    for cline in &content {
        rows.push(Line::raw(box_draw::content_line(
            &format!(" {cline} "),
            width,
            border_style,
        )));
    }
    rows.push(Line::raw(box_draw::bottom_border(width, border_style)));
    rows.push(Line::raw(""));
    rows
}

fn notice_style(theme: &TuiTheme) -> Style {
    Style::default().fg(theme.notice)
}

fn thinking_style(theme: &TuiTheme) -> Style {
    Style::default().fg(theme.thinking).italic()
}

fn tool_running_style(theme: &TuiTheme) -> Style {
    Style::default().fg(theme.accent)
}

#[allow(clippy::needless_pass_by_value)]
fn tool_finished_style(theme: &TuiTheme) -> Style {
    // Finished tool rows use the success accent; failures are surfaced via the
    // tool card status symbol rather than recoloring the whole row here.
    Style::default().fg(theme.success)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::TuiTheme;

    #[test]
    fn welcome_banner_has_correct_width_and_logo() {
        let data = BannerData {
            title: "Welcome to Neo!".to_owned(),
            subtitle: "Send /help for help information.".to_owned(),
            directory: "/tmp/neo".to_owned(),
            session: "test".to_owned(),
            model: "deepseek/deepseek-v4-pro".to_owned(),
            version: "0.1.0".to_owned(),
            mcp: None,
        };
        let lines = render_welcome_banner(&data, 60, &TuiTheme::default());
        for line in &lines {
            let width = crate::ansi::visible_width(&line.to_ansi());
            assert!(
                width == 60 || width == 0,
                "line width mismatch: {:?}",
                line.text()
            );
        }
        // The right edge of both logo rows should use the left half-block
        // glyph, not the square corner glyph '┐'.
        for logo_line in [&lines[2], &lines[3]] {
            assert!(!logo_line.text().contains('┐'));
        }
    }
}
