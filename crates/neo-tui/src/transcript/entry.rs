use crate::ansi::{Color, Style, paint, visible_width};
use crate::chrome::TuiTheme;
use crate::components::wrap_width;
use crate::core::Line;
use crate::image::{ImageRenderPolicy, ImageSource, InlineImage, TerminalImageCapabilities};
use crate::transcript::ToolCallComponent;
use crate::widgets::box_draw;

/// Rich welcome-banner content rendered as a rounded box (matching Neo).
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
pub struct ApprovalPromptData {
    pub id: String,
    pub title: String,
    pub details: Vec<String>,
    pub queued_label: String,
    pub queued_count: usize,
    pub selected: usize,
    pub feedback_input: String,
    pub resolved: Option<String>,
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
        expanded: bool,
    },
    ToolRun {
        component: ToolCallComponent,
    },
    ApprovalPrompt(ApprovalPromptData),
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
    GoalCard {
        kind: GoalCardKind,
        objective: String,
        detail: Option<String>,
        turns: Option<u32>,
    },
    SkillActivation {
        name: String,
        description: Option<String>,
        args: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalCardKind {
    Started,
    Paused,
    Resumed,
    Blocked,
    Finished,
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
            expanded: false,
        }
    }

    #[must_use]
    pub fn thinking_complete(content: impl Into<String>) -> Self {
        Self::ThinkingBlock {
            content: content.into(),
            phase: ThinkingPhase::Complete,
            expanded: false,
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
    pub fn skill_activated(
        name: impl Into<String>,
        description: Option<impl Into<String>>,
        args: Option<impl Into<String>>,
    ) -> Self {
        Self::SkillActivation {
            name: name.into(),
            description: description.map(Into::into),
            args: args.map(Into::into),
        }
    }

    #[must_use]
    pub fn goal_card(
        kind: GoalCardKind,
        objective: impl Into<String>,
        detail: Option<impl Into<String>>,
        turns: Option<u32>,
    ) -> Self {
        Self::GoalCard {
            kind,
            objective: objective.into(),
            detail: detail.map(Into::into),
            turns,
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
            // bullet (Neo style).
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
            Self::ThinkingBlock {
                content,
                phase,
                expanded,
            } => {
                if content.is_empty() {
                    Vec::new()
                } else {
                    render_thinking(content, inner_width, *phase, *expanded, theme)
                }
            }
            Self::ToolRun { component } => {
                let mut component = component.clone();
                component.render_with_theme(inner_width, theme)
            }
            Self::ApprovalPrompt(data) => render_approval_prompt(data, inner_width, theme),
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
            Self::GoalCard {
                kind,
                objective,
                detail,
                turns,
            } => render_goal_card(
                *kind,
                objective,
                detail.as_deref(),
                *turns,
                inner_width,
                theme,
            ),
            Self::SkillActivation {
                name,
                description,
                args,
            } => render_skill_used(
                name,
                description.as_deref(),
                args.as_deref(),
                inner_width,
                theme,
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
            Self::ApprovalPrompt(data) => ("Approval", data.title.clone()),
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
            Self::GoalCard {
                kind,
                objective,
                detail,
                turns,
            } => (
                "Goal",
                format!(
                    "{:?} goal: {objective}\n{}\n{}",
                    kind,
                    detail.as_deref().unwrap_or(""),
                    turns.map_or_else(String::new, |t| format!("Turns: {t}"))
                ),
            ),
            Self::SkillActivation {
                name,
                description,
                args,
            } => {
                let body = args
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .map(|a| format!("args: {a}"))
                    .or_else(|| {
                        description
                            .as_deref()
                            .filter(|s| !s.trim().is_empty())
                            .map(std::borrow::ToOwned::to_owned)
                    })
                    .unwrap_or_default();
                let text = if body.is_empty() {
                    format!("Used Skill: {name}")
                } else {
                    format!("Used Skill: {name}\n{body}")
                };
                ("Skill", text)
            }
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
/// to align under the body (prefix width of spaces). This is the Neo
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
fn render_thinking(
    thinking: &str,
    width: usize,
    phase: ThinkingPhase,
    expanded: bool,
    theme: &TuiTheme,
) -> Vec<Line> {
    let style = thinking_style(theme);
    let body_width = width.max(1).saturating_sub(2).max(1);
    let wrapped = wrap_width(thinking, body_width);
    let total = wrapped.len();
    let mut rows = Vec::new();

    if phase == ThinkingPhase::Streaming && !expanded {
        // Streaming: spinner + tail window.
        rows.push(Line::styled("⠋ thinking...", style));
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

/// Wrap `text` to `width` and emit each wrapped row as a styled [`Line`].
fn styled_wrap(text: &str, width: usize, style: Style) -> Vec<Line> {
    wrap_width(text, width.max(1))
        .into_iter()
        .map(|line| Line::styled(line, style))
        .collect()
}

fn render_approval_prompt(data: &ApprovalPromptData, width: usize, theme: &TuiTheme) -> Vec<Line> {
    let border = Style::default().fg(theme.status_warn);
    let title = Style::default().fg(theme.status_warn).bold();
    let body = Style::default().fg(theme.text_primary);
    let muted = Style::default().fg(theme.text_muted);
    let selected = Style::default().fg(theme.status_ok).bold();
    if let Some(resolved) = &data.resolved {
        return vec![Line::styled(format!("approval: {resolved}"), muted)];
    }

    let line = "\u{2500}".repeat(width.max(1));
    let mut rows = vec![Line::styled(line.clone(), border)];
    rows.extend(styled_wrap_with_indent(
        &format!("▶ {}", data.title),
        width,
        2,
        2,
        title,
    ));
    rows.push(Line::raw(""));
    for detail in &data.details {
        rows.extend(styled_wrap_with_indent(detail, width, 2, 4, body));
    }
    rows.push(Line::raw(""));
    for (index, label) in [
        "Approve once",
        "Approve for this session",
        "Reject",
        "Reject with feedback",
    ]
    .iter()
    .enumerate()
    {
        let prefix = if data.selected == index {
            "  ▶ "
        } else {
            "    "
        };
        let style = if data.selected == index {
            selected
        } else {
            body
        };
        rows.extend(styled_wrap_with_prefix(
            &format!("{}. {label}", index + 1),
            width,
            prefix,
            "     ",
            style,
        ));
    }
    rows.push(Line::raw(""));
    if data.selected == 3 {
        let feedback = if data.feedback_input.is_empty() {
            "feedback: ▌".to_owned()
        } else {
            format!("feedback: {}▌", data.feedback_input)
        };
        rows.extend(styled_wrap_with_indent(&feedback, width, 2, 4, selected));
        rows.push(Line::raw(""));
    }
    if data.queued_count > 0 {
        let suffix = if data.queued_count == 1 {
            "approval"
        } else {
            "approvals"
        };
        let queued_label = data.queued_label.trim();
        let label = if queued_label.is_empty() {
            suffix.to_owned()
        } else {
            format!("{queued_label} {suffix}")
        };
        rows.extend(styled_wrap_with_indent(
            &format!("queued: {} {label} waiting", data.queued_count),
            width,
            2,
            2,
            muted,
        ));
        rows.push(Line::raw(""));
    }
    rows.extend(styled_wrap_with_indent(
        "  ↑/↓ select · 1/2/3/4 choose · ↵ confirm",
        width,
        0,
        2,
        muted,
    ));
    rows.push(Line::styled(line, border));
    rows
}

fn styled_wrap_with_indent(
    text: &str,
    width: usize,
    first_indent: usize,
    continuation_indent: usize,
    style: Style,
) -> Vec<Line> {
    styled_wrap_with_prefix(
        text,
        width,
        &" ".repeat(first_indent),
        &" ".repeat(continuation_indent),
        style,
    )
}

fn styled_wrap_with_prefix(
    text: &str,
    width: usize,
    first_prefix: &str,
    continuation_prefix: &str,
    style: Style,
) -> Vec<Line> {
    let first_width = width.saturating_sub(visible_width(first_prefix)).max(1);
    let next_width = width
        .saturating_sub(visible_width(continuation_prefix))
        .max(1);
    let wrapped = wrap_width(text, first_width);
    let mut rows = Vec::with_capacity(wrapped.len());
    for (index, line) in wrapped.into_iter().enumerate() {
        if index == 0 {
            rows.push(Line::styled(format!("{first_prefix}{line}"), style));
        } else {
            for continued in wrap_width(&line, next_width) {
                rows.push(Line::styled(
                    format!("{continuation_prefix}{continued}"),
                    style,
                ));
            }
        }
    }
    rows
}

/// Render the welcome banner as a rounded box with an ASCII-art logo and
/// aligned metadata, matching Neo's `welcome.ts`.
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

fn render_goal_card(
    kind: GoalCardKind,
    objective: &str,
    detail: Option<&str>,
    turns: Option<u32>,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    let (icon, label, color) = match kind {
        GoalCardKind::Started => ("▶", "GOAL STARTED", theme.brand),
        GoalCardKind::Paused => ("⏸", "GOAL PAUSED", theme.status_warn),
        GoalCardKind::Resumed => ("▶", "GOAL RESUMED", theme.brand),
        GoalCardKind::Blocked => ("⏹", "GOAL BLOCKED", theme.status_error),
        GoalCardKind::Finished => ("✓", "GOAL COMPLETE", theme.status_ok),
    };

    let mut content: Vec<String> = Vec::new();
    content.push(format!("{icon} {label}"));
    content.push(String::new());
    content.push(objective.to_owned());
    if let Some(detail) = detail {
        content.push(String::new());
        content.push(detail.to_owned());
    }
    if let Some(turns) = turns {
        content.push(String::new());
        content.push(format!("Turns used: {turns}"));
    }

    let border_style = Style::default().fg(color);
    let header_style = Style::default().fg(color).bold();
    let body_style = Style::default().fg(theme.text_primary);

    let inner_width = width.saturating_sub(4).max(1);
    let mut rows: Vec<Line> = Vec::new();
    rows.push(Line::raw(box_draw::top_border(width, border_style)));
    for (idx, line) in content.iter().enumerate() {
        let wrapped = wrap_width(line, inner_width);
        let style = if idx == 0 { header_style } else { body_style };
        if wrapped.is_empty() {
            rows.push(Line::raw(paint(
                &box_draw::content_line("", width, border_style),
                style,
            )));
        } else {
            for part in wrapped {
                rows.push(Line::raw(paint(
                    &box_draw::content_line(&format!(" {part} "), width, border_style),
                    style,
                )));
            }
        }
    }
    rows.push(Line::raw(box_draw::bottom_border(width, border_style)));
    rows.push(Line::raw(""));
    rows
}

fn render_skill_used(
    name: &str,
    description: Option<&str>,
    args: Option<&str>,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    let brand = Style::default().fg(theme.brand).bold();
    let muted = Style::default().fg(theme.text_muted);

    let mut rows = Vec::new();
    rows.extend(styled_wrap_with_prefix(
        name,
        width,
        "✦ Used Skill: ",
        &" ".repeat(visible_width("✦ Used Skill: ")),
        brand,
    ));

    let body = args
        .filter(|s| !s.trim().is_empty())
        .map(|a| format!("args: {a}"))
        .or_else(|| {
            description
                .filter(|s| !s.trim().is_empty())
                .map(std::borrow::ToOwned::to_owned)
        });

    if let Some(body) = body {
        let indent = "   ";
        let body_width = width.saturating_sub(indent.len()).max(1);
        for line in wrap_width(&body, body_width) {
            rows.push(Line::styled(format!("{indent}{line}"), muted));
        }
    }

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

    #[test]
    fn thinking_block_expands_full_text() {
        let content = "one two three four five six seven eight nine ten eleven twelve";
        let collapsed = TranscriptEntry::ThinkingBlock {
            content: content.to_owned(),
            phase: ThinkingPhase::Complete,
            expanded: false,
        }
        .render(14, &TuiTheme::default())
        .into_iter()
        .map(|line| line.text().to_owned())
        .collect::<Vec<_>>();
        let expanded = TranscriptEntry::ThinkingBlock {
            content: content.to_owned(),
            phase: ThinkingPhase::Complete,
            expanded: true,
        }
        .render(14, &TuiTheme::default())
        .into_iter()
        .map(|line| line.text().to_owned())
        .collect::<Vec<_>>();

        assert!(
            collapsed
                .iter()
                .any(|line| line.contains("ctrl+o to expand"))
        );
        assert!(
            !expanded
                .iter()
                .any(|line| line.contains("ctrl+o to expand"))
        );
        assert!(expanded.len() > collapsed.len());
    }

    #[test]
    fn skill_used_renders_header_and_description() {
        let entry = TranscriptEntry::skill_activated(
            "executing-plans",
            Some("Execute a plan task-by-task."),
            None::<String>,
        );
        let lines = entry
            .render(60, &TuiTheme::default())
            .into_iter()
            .map(|l| l.text().to_owned())
            .collect::<Vec<_>>();
        assert!(
            lines
                .iter()
                .any(|l| l.contains("✦ Used Skill: executing-plans"))
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("Execute a plan task-by-task."))
        );
    }

    #[test]
    fn skill_used_prefers_args_over_description() {
        let entry = TranscriptEntry::skill_activated(
            "sub-skill",
            Some("Decompose into sub-skills."),
            Some("refactor auth module".to_owned()),
        );
        let lines = entry
            .render(60, &TuiTheme::default())
            .into_iter()
            .map(|l| l.text().to_owned())
            .collect::<Vec<_>>();
        assert!(
            lines
                .iter()
                .any(|l| l.contains("args: refactor auth module"))
        );
        assert!(
            !lines
                .iter()
                .any(|l| l.contains("Decompose into sub-skills."))
        );
    }

    #[test]
    fn skill_used_wraps_long_description() {
        let entry = TranscriptEntry::skill_activated(
            "brainstorming",
            Some("Explore user intent, requirements and design before implementation."),
            None::<String>,
        );
        let lines = entry
            .render(40, &TuiTheme::default())
            .into_iter()
            .map(|l| l.text().to_owned())
            .collect::<Vec<_>>();
        assert!(lines.len() >= 3); // header + 2+ wrapped body lines + blank
    }

    #[test]
    fn skill_used_renders_header_only_when_no_body() {
        let entry =
            TranscriptEntry::skill_activated("executing-plans", None::<String>, None::<String>);
        let lines = entry
            .render(60, &TuiTheme::default())
            .into_iter()
            .map(|l| l.text().to_owned())
            .collect::<Vec<_>>();
        assert!(
            lines
                .iter()
                .any(|l| l.contains("✦ Used Skill: executing-plans"))
        );
        assert!(!lines.iter().any(|l| l.contains("args:")));
        assert!(!lines.iter().any(|l| l.contains("Execute")));
    }

    #[test]
    fn skill_used_falls_back_to_description_when_args_are_whitespace() {
        let entry = TranscriptEntry::skill_activated(
            "executing-plans",
            Some("Execute a plan task-by-task.".to_owned()),
            Some("   ".to_owned()),
        );
        let lines = entry
            .render(60, &TuiTheme::default())
            .into_iter()
            .map(|l| l.text().to_owned())
            .collect::<Vec<_>>();
        assert!(
            lines
                .iter()
                .any(|l| l.contains("Execute a plan task-by-task."))
        );
        assert!(!lines.iter().any(|l| l.contains("args:")));
    }
}
