use crate::ansi::{Color, Style, visible_width};
use crate::app::TuiTheme;
use crate::core::{Finalization, Line};
use crate::wrap_width;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEntry {
    Banner(String),
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
        Self::Banner(title.into())
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
            Self::Banner(title) => styled_wrap(title, inner_width, banner_style(theme)),
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
                    // Assistant body is rendered as markdown (headings, bold,
                    // inline code, code blocks with syntax highlighting, lists,
                    // tables, blockquotes). On finalization we prefix a single
                    // magenta `● ` bullet line as a turn separator; streaming
                    // content renders without a bullet.
                    if *finalized {
                        rows.push(Line::styled("●", Style::default().fg(theme.accent).bold()));
                    }
                    let md_width = if *finalized {
                        // finalized body sits flush under the bullet, no indent
                        inner_width
                    } else {
                        inner_width
                    };
                    rows.extend(crate::markdown::render_markdown(content, md_width, theme));
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

fn banner_style(theme: &TuiTheme) -> Style {
    Style::default().fg(theme.header).bold()
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
