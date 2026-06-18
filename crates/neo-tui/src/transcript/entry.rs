use crate::ansi::{Color, Style, paint, visible_width};
use crate::chrome::TuiTheme;
use crate::components::wrap_width;
use crate::core::Line;
use crate::image::{ImageRenderPolicy, ImageSource, InlineImage, TerminalImageCapabilities};
use crate::transcript::ToolCallComponent;
use crate::widgets::box_draw;

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
    UserMessage(String),
    AssistantMessage {
        content: String,
    },
    ThinkingBlock {
        content: String,
        phase: ThinkingPhase,
    },
    ToolRun {
        component: ToolCallComponent,
    },
    Image {
        id: String,
        mime_type: String,
        size_bytes: Option<usize>,
        alt: Option<String>,
        source: ImageSource,
        metadata: String,
        payload: Option<Vec<u8>>,
    },
    Compaction {
        phase: Option<neo_agent_core::CompactionPhase>,
        percent: u8,
        compacted_message_count: usize,
        tokens_before: usize,
    },
    Status {
        text: String,
        /// When set, the status renders as a bold title (in the severity
        /// color) for system/error statuses. Plain statuses stay a single dim
        /// line with no prefix.
        severity: Option<StatusSeverity>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingPhase {
    Streaming,
    Complete,
}

/// Severity for an emphasized system notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusSeverity {
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
    pub fn user_message(content: impl Into<String>) -> Self {
        Self::UserMessage(content.into())
    }

    #[must_use]
    pub fn assistant_message(content: impl Into<String>) -> Self {
        Self::AssistantMessage {
            content: content.into(),
        }
    }

    #[must_use]
    pub fn thinking_streaming(content: impl Into<String>) -> Self {
        Self::ThinkingBlock {
            content: content.into(),
            phase: ThinkingPhase::Streaming,
        }
    }

    #[must_use]
    pub fn thinking_complete(content: impl Into<String>) -> Self {
        Self::ThinkingBlock {
            content: content.into(),
            phase: ThinkingPhase::Complete,
        }
    }

    #[must_use]
    pub fn tool_run(component: ToolCallComponent) -> Self {
        Self::ToolRun { component }
    }

    #[must_use]
    pub fn image(
        id: impl Into<String>,
        mime_type: impl Into<String>,
        size_bytes: Option<usize>,
        alt: Option<impl Into<String>>,
        source: ImageSource,
        metadata: impl Into<String>,
        payload: Option<Vec<u8>>,
    ) -> Self {
        Self::Image {
            id: id.into(),
            mime_type: mime_type.into(),
            size_bytes,
            alt: alt.map(Into::into),
            source,
            metadata: metadata.into(),
            payload,
        }
    }

    #[must_use]
    pub const fn compaction(compacted_message_count: usize, tokens_before: usize) -> Self {
        Self::Compaction {
            phase: Some(neo_agent_core::CompactionPhase::Applying),
            percent: 100,
            compacted_message_count,
            tokens_before,
        }
    }

    #[must_use]
    pub fn status(content: impl Into<String>) -> Self {
        Self::Status {
            text: content.into(),
            severity: None,
        }
    }

    #[must_use]
    pub fn status_severity(content: impl Into<String>, severity: StatusSeverity) -> Self {
        Self::Status {
            text: content.into(),
            severity: Some(severity),
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
            Self::UserMessage(content) => {
                let style = Style::default().fg(theme.user_message);
                bulleted_wrap(content, inner_width, "✨ ", style)
            }
            // Status: plain dim single-line for routine statuses; a bold
            // severity-colored title for system/error statuses.
            Self::Status { text, severity } => match severity {
                None => styled_wrap(text, inner_width, status_style(theme)),
                Some(sev) => {
                    let style = Style::default().fg(severity_color(*sev, theme)).bold();
                    styled_wrap(text, inner_width, style)
                }
            },
            Self::AssistantMessage { content } => {
                if content.is_empty() {
                    return Vec::new();
                }
                crate::markdown::render_markdown(content, inner_width, theme, "● ", "  ")
            }
            Self::ThinkingBlock { content, phase } => {
                if content.is_empty() {
                    Vec::new()
                } else {
                    render_thinking(content, inner_width, *phase, theme)
                }
            }
            Self::ToolRun { component } => {
                let mut component = component.clone();
                component.render_with_theme(inner_width, theme)
            }
            Self::Image { metadata, .. } => styled_wrap(metadata, inner_width, status_style(theme)),
            Self::Compaction {
                compacted_message_count,
                tokens_before,
                ..
            } => styled_wrap(
                &format!(
                    "Compacted {compacted_message_count} messages · {} tokens before",
                    format_token_count_usize(*tokens_before)
                ),
                inner_width,
                status_style(theme),
            ),
        }
    }

    #[must_use]
    pub fn copy_parts(&self) -> (&'static str, String) {
        match self {
            Self::Banner(data) => (
                "Banner",
                format!(
                    "{}\nSession: {}\nModel: {}\nWorkspace: {}",
                    data.title, data.session, data.model, data.directory
                ),
            ),
            Self::UserMessage(content) => ("You", content.clone()),
            Self::AssistantMessage { content } => ("Assistant", content.clone()),
            Self::ThinkingBlock { content, .. } => ("Thinking", content.clone()),
            Self::ToolRun { component } => {
                let state = component.state();
                let detail = state
                    .result
                    .as_ref()
                    .filter(|result| !result.is_empty())
                    .or_else(|| {
                        state
                            .arguments
                            .as_ref()
                            .filter(|arguments| !arguments.is_empty())
                    })
                    .cloned()
                    .unwrap_or_default();
                (
                    "Tool",
                    format!("{} {} ({detail})", state.status.marker(), state.name),
                )
            }
            Self::Image { metadata, .. } => ("Image", metadata.clone()),
            Self::Compaction {
                compacted_message_count,
                tokens_before,
                ..
            } => (
                "Compact",
                format!(
                    "Compacted {compacted_message_count} messages · {} tokens before",
                    format_token_count_usize(*tokens_before)
                ),
            ),
            Self::Status { text, .. } => ("Status", text.clone()),
        }
    }

    #[must_use]
    pub fn inline_image_render(
        &self,
        image_render_policy: ImageRenderPolicy,
        image_capabilities: TerminalImageCapabilities,
    ) -> Option<InlineImageRender> {
        let Self::Image {
            id,
            mime_type,
            alt,
            source,
            payload,
            ..
        } = self
        else {
            return None;
        };
        let payload = payload.as_ref()?;
        let inline = InlineImage::bytes(
            id.clone(),
            mime_type.clone(),
            payload.clone(),
            alt.clone(),
            *source,
        );
        image_render_policy
            .render_inline_image(&inline, image_capabilities)
            .escape_sequence
            .map(|escape_sequence| InlineImageRender {
                id: id.clone(),
                escape_sequence,
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineImageRender {
    pub id: String,
    pub escape_sequence: String,
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
fn severity_color(severity: StatusSeverity, theme: &TuiTheme) -> Color {
    match severity {
        StatusSeverity::Info => theme.brand,
        StatusSeverity::Warning => theme.status_warn,
        StatusSeverity::Error => theme.status_error,
    }
}

fn format_token_count_usize(tokens: usize) -> String {
    if tokens >= 1_000_000 {
        format!("{}m", tokens / 1_000_000)
    } else if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

/// Number of thinking lines shown in the floating window (streaming) or as a
/// compact preview. Matches kimi-code's `THINKING_PREVIEW_LINES = 2`.
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
fn render_thinking(
    thinking: &str,
    width: usize,
    phase: ThinkingPhase,
    theme: &TuiTheme,
) -> Vec<Line> {
    let style = thinking_style(theme);
    let body_width = width.max(1).saturating_sub(2).max(1);
    let wrapped = wrap_width(thinking, body_width);
    let total = wrapped.len();
    let mut rows = Vec::new();

    if phase == ThinkingPhase::Streaming {
        // Streaming: spinner + tail window.
        rows.push(Line::styled("⠋ thinking...", style));
        let start = total.saturating_sub(THINKING_PREVIEW_LINES);
        for line in &wrapped[start..] {
            rows.push(Line::styled(format!("  {line}"), style));
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
    let logo_color = Style::default().fg(theme.brand);
    let title_style = Style::default().fg(theme.brand).bold();
    let subtitle_style = Style::default().fg(theme.text_muted);
    let label_style = Style::default().fg(theme.text_muted).bold();
    let value_style = Style::default().fg(theme.text_primary);

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

    let border_style = Style::default().fg(theme.brand);
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

fn status_style(theme: &TuiTheme) -> Style {
    Style::default().fg(theme.text_muted)
}

fn thinking_style(theme: &TuiTheme) -> Style {
    Style::default().fg(theme.text_muted).italic()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::TuiTheme;

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
