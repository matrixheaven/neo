use crate::ansi::{Color, Style, paint, visible_width};
use crate::chrome::TuiTheme;
use crate::components::wrap_width;
use crate::core::Line;
use crate::image::{ImageRenderPolicy, ImageSource, InlineImage, TerminalImageCapabilities};
use crate::transcript::ToolCallComponent;
use crate::widgets::box_draw;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalPromptData {
    pub id: String,
    pub title: String,
    pub details: Vec<String>,
    pub queued_label: String,
    pub queued_count: usize,
    pub selected: usize,
    pub feedback_input: String,
    pub resolved: Option<String>,
    /// Dynamic label for the reusable session-approval option (Layer 1).
    /// `None` omits the option, keeping numeric shortcuts aligned.
    #[serde(default)]
    pub session_option_label: Option<String>,
    /// Dynamic label for the persistent prefix-approval option (Layer 2).
    /// `None` omits the option.
    #[serde(default)]
    pub prefix_option_label: Option<String>,
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
    /// A user message that was queued (Enter while busy) or steered (Ctrl+S)
    /// into a running turn. Rendered with a distinct prefix so the user can tell
    /// it apart from a normal delivered user message. `is_steer` selects the
    /// steer styling (↳) vs the follow-up styling (↪).
    QueuedMessage {
        text: String,
        is_steer: bool,
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
#[repr(usize)]
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
    pub fn queued_message(content: impl Into<String>, is_steer: bool) -> Self {
        Self::QueuedMessage {
            text: content.into(),
            is_steer,
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
        self.render_with_activity_frame(width, theme, 0)
    }

    #[must_use]
    pub fn render_with_activity_frame(
        &self,
        width: usize,
        theme: &TuiTheme,
        activity_frame: usize,
    ) -> Vec<Line> {
        // Every `Line` returned here MUST map to exactly one terminal row:
        // content is split on `\n` and soft-wrapped to `width` so no line ever
        // carries an embedded newline. The renderer's diff/scroll math treats
        // each `Vec<String>` entry as one screen row, so an un-split long line
        // would corrupt the coordinate model and garble streaming output.
        let inner_width = width.max(1);
        self.render_inner(inner_width, theme, activity_frame)
    }

    fn render_inner(
        &self,
        inner_width: usize,
        theme: &TuiTheme,
        activity_frame: usize,
    ) -> Vec<Line> {
        if let Some(lines) = self.render_message_entry(inner_width, theme, activity_frame) {
            return lines;
        }
        self.render_structured_entry(inner_width, theme)
    }

    fn render_message_entry(
        &self,
        inner_width: usize,
        theme: &TuiTheme,
        activity_frame: usize,
    ) -> Option<Vec<Line>> {
        let lines = match self {
            Self::Banner(data) => render_welcome_banner(data, inner_width, theme),
            Self::UserMessage(content) => render_user_message(content, inner_width, theme),
            Self::Status { text, severity } => render_status(text, *severity, inner_width, theme),
            Self::QueuedMessage { text, is_steer } => {
                render_queued_message(text, *is_steer, inner_width, theme)
            }
            Self::AssistantMessage { content } => {
                render_assistant_message(content, inner_width, theme)
            }
            Self::ThinkingBlock {
                content,
                phase,
                expanded,
            } => render_thinking_block(
                content,
                *phase,
                *expanded,
                inner_width,
                theme,
                activity_frame,
            ),
            _ => return None,
        };
        Some(lines)
    }

    fn render_structured_entry(&self, inner_width: usize, theme: &TuiTheme) -> Vec<Line> {
        match self {
            Self::ToolRun { component } => render_tool_run(component, inner_width, theme),
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
            Self::Banner(_)
            | Self::UserMessage(_)
            | Self::Status { .. }
            | Self::AssistantMessage { .. }
            | Self::ThinkingBlock { .. }
            | Self::QueuedMessage { .. } => unreachable!("message entries handled above"),
        }
    }

    #[must_use]
    pub fn copy_parts(&self) -> (&'static str, String) {
        if let Some(parts) = simple_copy_parts(self) {
            return parts;
        }
        complex_copy_parts(self)
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

fn complex_copy_parts(entry: &TranscriptEntry) -> (&'static str, String) {
    if let Some(parts) = utility_copy_parts(entry) {
        return parts;
    }
    card_copy_parts(entry)
}

fn utility_copy_parts(entry: &TranscriptEntry) -> Option<(&'static str, String)> {
    match entry {
        TranscriptEntry::Banner(data) => Some(("Banner", copy_banner(data))),
        TranscriptEntry::ToolRun { component } => Some(("Tool", copy_tool(component))),
        TranscriptEntry::Compaction {
            compacted_message_count,
            tokens_before,
            ..
        } => Some((
            "Compact",
            copy_compaction(*compacted_message_count, *tokens_before),
        )),
        _ => None,
    }
}

fn card_copy_parts(entry: &TranscriptEntry) -> (&'static str, String) {
    match entry {
        TranscriptEntry::GoalCard {
            kind,
            objective,
            detail,
            turns,
        } => (
            "Goal",
            copy_goal(*kind, objective, detail.as_deref(), *turns),
        ),
        TranscriptEntry::SkillActivation {
            name,
            description,
            args,
        } => (
            "Skill",
            copy_skill(name, description.as_deref(), args.as_deref()),
        ),
        TranscriptEntry::UserMessage(_)
        | TranscriptEntry::AssistantMessage { .. }
        | TranscriptEntry::ThinkingBlock { .. }
        | TranscriptEntry::ApprovalPrompt(_)
        | TranscriptEntry::Image { .. }
        | TranscriptEntry::Status { .. }
        | TranscriptEntry::QueuedMessage { .. } => unreachable!("simple copy parts handled above"),
        TranscriptEntry::Banner(_)
        | TranscriptEntry::ToolRun { .. }
        | TranscriptEntry::Compaction { .. } => unreachable!("utility copy parts handled above"),
    }
}

fn simple_copy_parts(entry: &TranscriptEntry) -> Option<(&'static str, String)> {
    text_copy_parts(entry)
        .or_else(|| status_copy_parts(entry))
        .or_else(|| media_copy_parts(entry))
}

fn text_copy_parts(entry: &TranscriptEntry) -> Option<(&'static str, String)> {
    match entry {
        TranscriptEntry::UserMessage(content) => Some(("You", content.clone())),
        TranscriptEntry::AssistantMessage { content } => Some(("Assistant", content.clone())),
        TranscriptEntry::ThinkingBlock { content, .. } => Some(("Thinking", content.clone())),
        TranscriptEntry::QueuedMessage { text, is_steer } => {
            let label = if *is_steer { "Steer" } else { "Queued" };
            Some((label, text.clone()))
        }
        _ => None,
    }
}

fn status_copy_parts(entry: &TranscriptEntry) -> Option<(&'static str, String)> {
    match entry {
        TranscriptEntry::Status { text, .. } => Some(("Status", text.clone())),
        TranscriptEntry::ApprovalPrompt(data) => Some(("Approval", data.title.clone())),
        _ => None,
    }
}

fn media_copy_parts(entry: &TranscriptEntry) -> Option<(&'static str, String)> {
    match entry {
        TranscriptEntry::Image { metadata, .. } => Some(("Image", metadata.clone())),
        _ => None,
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
    let prefix_width = visible_width(prefix);
    // BUGFIX: previously this wrapped at the full `width` without subtracting
    // the prefix, so the first rendered row was `prefix + width` columns wide
    // and overflowed the terminal. Long CJK prompts (each char is 2 columns)
    // hit this reliably and crashed the renderer's width invariant
    // (`renderer.rs` `check_line_widths`). The body budget must reserve space
    // for the prefix, mirroring `styled_wrap_with_prefix` and `render_markdown`.
    let body_width = width.saturating_sub(prefix_width).max(1);
    let indent = " ".repeat(prefix_width);
    let mut rows = Vec::new();
    for (i, line) in wrap_width(text, body_width).into_iter().enumerate() {
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
    // Build the option list dynamically. The session-approval (Layer 1) and
    // prefix-rule (Layer 2) options appear only when their labels are `Some`,
    // so numeric shortcuts and the feedback-input index track the visible list.
    let mut options: Vec<String> = vec!["Approve once".to_owned()];
    if let Some(label) = &data.session_option_label {
        options.push(label.clone());
    }
    if let Some(label) = &data.prefix_option_label {
        options.push(label.clone());
    }
    options.push("Reject".to_owned());
    options.push("Reject with feedback".to_owned());
    let revise_index = options.len() - 1;

    for (index, label) in options.iter().enumerate() {
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
    if data.selected == revise_index {
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
        "  ↑/↓ select · number keys choose · ↵ confirm",
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

fn render_user_message(content: &str, width: usize, theme: &TuiTheme) -> Vec<Line> {
    let style = Style::default().fg(theme.user_message);
    bulleted_wrap(content, width, "✨ ", style)
}

/// Render a queued/steered message. Steer uses `↳` (brand color) to signal an
/// immediate mid-turn injection; follow-up uses `↪` (muted) to signal a queued
/// turn that runs after the current one.
fn render_queued_message(text: &str, is_steer: bool, width: usize, theme: &TuiTheme) -> Vec<Line> {
    let (prefix, style) = if is_steer {
        ("↳ ", Style::default().fg(theme.brand).italic())
    } else {
        ("↪ ", Style::default().fg(theme.text_muted))
    };
    bulleted_wrap(text, width, prefix, style)
}

fn render_status(
    text: &str,
    severity: Option<StatusSeverity>,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    let Some(severity) = severity else {
        return styled_wrap(text, width, status_style(theme));
    };
    let style = Style::default().fg(severity_color(severity, theme)).bold();
    styled_wrap(text, width, style)
}

fn render_assistant_message(content: &str, width: usize, theme: &TuiTheme) -> Vec<Line> {
    if content.is_empty() {
        Vec::new()
    } else {
        crate::markdown::render_markdown(content, width, theme, "● ", "  ")
    }
}

fn render_thinking_block(
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

fn render_tool_run(component: &ToolCallComponent, width: usize, theme: &TuiTheme) -> Vec<Line> {
    let mut component = component.clone();
    component.render_with_theme(width, theme)
}

fn copy_banner(data: &BannerData) -> String {
    format!(
        "{}\nSession: {}\nModel: {}\nWorkspace: {}",
        data.title, data.session, data.model, data.directory
    )
}

fn copy_tool(component: &ToolCallComponent) -> String {
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
    format!("{} {} ({detail})", state.status.marker(), state.name)
}

fn copy_compaction(compacted_message_count: usize, tokens_before: usize) -> String {
    format!(
        "Compacted {compacted_message_count} messages · {} tokens before",
        format_token_count_usize(tokens_before)
    )
}

fn copy_goal(
    kind: GoalCardKind,
    objective: &str,
    detail: Option<&str>,
    turns: Option<u32>,
) -> String {
    format!(
        "{kind:?} goal: {objective}\n{}\n{}",
        detail.unwrap_or(""),
        turns.map_or_else(String::new, |turn| format!("Turns: {turn}"))
    )
}

fn copy_skill(name: &str, description: Option<&str>, args: Option<&str>) -> String {
    let body = skill_body(description, args).unwrap_or_default();
    if body.is_empty() {
        format!("Used Skill: {name}")
    } else {
        format!("Used Skill: {name}\n{body}")
    }
}

fn render_goal_card(
    kind: GoalCardKind,
    objective: &str,
    detail: Option<&str>,
    turns: Option<u32>,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    let chrome = GoalCardChrome::new(kind, theme);
    let content = goal_card_content(&chrome, objective, detail, turns);
    render_goal_card_rows(&content, width, &chrome, theme)
}

struct GoalCardChrome {
    icon: &'static str,
    label: &'static str,
    color: crate::ansi::Color,
}

impl GoalCardChrome {
    fn new(kind: GoalCardKind, theme: &TuiTheme) -> Self {
        Self {
            icon: goal_card_icon(kind),
            label: goal_card_label(kind),
            color: goal_card_color(kind, theme),
        }
    }

    fn header(&self) -> String {
        format!("{} {}", self.icon, self.label)
    }
}

const GOAL_CARD_ICONS: [&str; 5] = ["▶", "⏸", "▶", "⏹", "✓"];
const GOAL_CARD_LABELS: [&str; 5] = [
    "GOAL STARTED",
    "GOAL PAUSED",
    "GOAL RESUMED",
    "GOAL BLOCKED",
    "GOAL COMPLETE",
];

fn goal_card_icon(kind: GoalCardKind) -> &'static str {
    GOAL_CARD_ICONS[kind as usize]
}

fn goal_card_label(kind: GoalCardKind) -> &'static str {
    GOAL_CARD_LABELS[kind as usize]
}

fn goal_card_color(kind: GoalCardKind, theme: &TuiTheme) -> crate::ansi::Color {
    [
        theme.brand,
        theme.status_warn,
        theme.brand,
        theme.status_error,
        theme.status_ok,
    ][kind as usize]
}

fn goal_card_content(
    chrome: &GoalCardChrome,
    objective: &str,
    detail: Option<&str>,
    turns: Option<u32>,
) -> Vec<String> {
    let mut content: Vec<String> = Vec::new();
    content.push(chrome.header());
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
    content
}

fn render_goal_card_rows(
    content: &[String],
    width: usize,
    chrome: &GoalCardChrome,
    theme: &TuiTheme,
) -> Vec<Line> {
    let border_style = Style::default().fg(chrome.color);
    let header_style = Style::default().fg(chrome.color).bold();
    let body_style = Style::default().fg(theme.text_primary);
    let inner_width = width.saturating_sub(4).max(1);
    let mut rows: Vec<Line> = Vec::new();
    rows.push(Line::raw(box_draw::top_border(width, border_style)));
    for (idx, line) in content.iter().enumerate() {
        let style = if idx == 0 { header_style } else { body_style };
        rows.extend(render_goal_card_content_line(
            line,
            inner_width,
            width,
            border_style,
            style,
        ));
    }
    rows.push(Line::raw(box_draw::bottom_border(width, border_style)));
    rows.push(Line::raw(""));
    rows
}

fn render_goal_card_content_line(
    line: &str,
    inner_width: usize,
    width: usize,
    border_style: Style,
    style: Style,
) -> Vec<Line> {
    let wrapped = wrap_width(line, inner_width);
    if wrapped.is_empty() {
        return vec![Line::raw(paint(
            &box_draw::content_line("", width, border_style),
            style,
        ))];
    }
    wrapped
        .into_iter()
        .map(|part| {
            Line::raw(paint(
                &box_draw::content_line(&format!(" {part} "), width, border_style),
                style,
            ))
        })
        .collect()
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

    if let Some(body) = skill_body(description, args) {
        let indent = "   ";
        let body_width = width.saturating_sub(indent.len()).max(1);
        for line in wrap_width(&body, body_width) {
            rows.push(Line::styled(format!("{indent}{line}"), muted));
        }
    }

    rows.push(Line::raw(""));
    rows
}

fn skill_body(description: Option<&str>, args: Option<&str>) -> Option<String> {
    args.filter(|s| !s.trim().is_empty())
        .map(|a| format!("args: {a}"))
        .or_else(|| {
            description
                .filter(|s| !s.trim().is_empty())
                .map(std::borrow::ToOwned::to_owned)
        })
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
        .map(|line| line.text().clone())
        .collect::<Vec<_>>();
        let expanded = TranscriptEntry::ThinkingBlock {
            content: content.to_owned(),
            phase: ThinkingPhase::Complete,
            expanded: true,
        }
        .render(14, &TuiTheme::default())
        .into_iter()
        .map(|line| line.text().clone())
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
            .map(|l| l.text().clone())
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
            .map(|l| l.text().clone())
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
            .map(|l| l.text().clone())
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
            .map(|l| l.text().clone())
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
            .map(|l| l.text().clone())
            .collect::<Vec<_>>();
        assert!(
            lines
                .iter()
                .any(|l| l.contains("Execute a plan task-by-task."))
        );
        assert!(!lines.iter().any(|l| l.contains("args:")));
    }
}
