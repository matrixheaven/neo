use crate::primitive::theme::TuiTheme;
use crate::primitive::wrap_width;
use crate::primitive::{Color, Style, visible_width};
use crate::primitive::{Line, Span};
use crate::terminal_image::{
    ImageRenderPolicy, ImageSource, InlineImage, TerminalImageCapabilities,
};
use crate::transcript::DelegateCardComponent;
use crate::transcript::DelegateGroupComponent;
use crate::transcript::PlanBoxComponent;
use crate::transcript::ShellRunComponent;
use crate::transcript::SwarmCardComponent;
use crate::transcript::ToolCallComponent;
use crate::transcript::WorkflowCardComponent;
use neo_agent_core::PlanSuggestion;
use serde::{Deserialize, Serialize};

mod copy;
mod render_banner;
mod render_goal;
mod render_mcp_startup;
mod render_status;
mod render_thinking;

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
    /// Plan file content to render inside the approval dialog (`PlanTransition` only).
    #[serde(default)]
    pub plan_content: Option<String>,
    /// Plan file path for the box title (`PlanTransition` only).
    #[serde(default)]
    pub plan_path: Option<String>,
    /// Model-supplied option labels for plan review (`PlanTransition` only).
    /// When non-empty, these replace the generic "Approve once" option so
    /// that the rendered option list matches the chrome's one-to-one.
    #[serde(default)]
    pub plan_option_labels: Vec<String>,
    /// Preset revision suggestions for plan review (`PlanTransition` only).
    #[serde(default)]
    pub suggestions: Vec<PlanSuggestion>,
    /// Index of the currently selected preset suggestion, if any. When set,
    /// the corresponding suggestion's feedback text is populated into
    /// [`Self::feedback_input`] and the option selection moves to
    /// "Reject with feedback".
    #[serde(default)]
    pub selected_suggestion: Option<usize>,
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
    ShellRun {
        component: ShellRunComponent,
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
        tokens_after: usize,
    },
    Status {
        text: String,
        /// When set, the status renders as a bold title (in the severity
        /// color) for system/error statuses. Plain statuses stay a single dim
        /// line with no prefix.
        severity: Option<StatusSeverity>,
    },
    McpStartupStatus {
        data: McpStartupStatusData,
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
        names: Vec<String>,
        body: String,
        expanded: bool,
    },
    Delegate {
        component: DelegateCardComponent,
    },
    DelegateGroup {
        component: DelegateGroupComponent,
    },
    DelegateSwarm {
        component: SwarmCardComponent,
    },
    Workflow {
        component: WorkflowCardComponent,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpStartupStatusData {
    pub id: String,
    pub transport: String,
    pub phase: McpStartupPhase,
}

impl McpStartupStatusData {
    #[must_use]
    pub fn message(&self) -> String {
        match &self.phase {
            McpStartupPhase::Connecting => {
                format!(
                    "MCP server \"{}\" connecting... ({})",
                    self.id, self.transport
                )
            }
            McpStartupPhase::Connected { tool_count } => format!(
                "MCP server \"{}\" connected · {} tools ({})",
                self.id, tool_count, self.transport
            ),
            McpStartupPhase::NeedsAuth { hint } => {
                format!("MCP server \"{}\" needs OAuth · {hint}", self.id)
            }
            McpStartupPhase::Failed { message } => {
                format!("MCP server \"{}\" failed · {message}", self.id)
            }
            McpStartupPhase::Disabled => {
                format!("MCP server \"{}\" disabled ({})", self.id, self.transport)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpStartupPhase {
    Connecting,
    Connected { tool_count: usize },
    NeedsAuth { hint: String },
    Failed { message: String },
    Disabled,
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
    pub fn shell_run(component: ShellRunComponent) -> Self {
        Self::ShellRun { component }
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
    pub const fn compaction(
        compacted_message_count: usize,
        tokens_before: usize,
        tokens_after: usize,
    ) -> Self {
        Self::Compaction {
            phase: Some(neo_agent_core::CompactionPhase::Applying),
            percent: 100,
            compacted_message_count,
            tokens_before,
            tokens_after,
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
    pub const fn mcp_startup_status(data: McpStartupStatusData) -> Self {
        Self::McpStartupStatus { data }
    }

    #[must_use]
    pub fn queued_message(content: impl Into<String>, is_steer: bool) -> Self {
        Self::QueuedMessage {
            text: content.into(),
            is_steer,
        }
    }

    #[must_use]
    pub fn skill_activated(names: Vec<String>, body: impl Into<String>) -> Self {
        Self::SkillActivation {
            names,
            body: body.into(),
            expanded: false,
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
        self.render_structured_entry(inner_width, theme, activity_frame)
    }

    fn render_message_entry(
        &self,
        inner_width: usize,
        theme: &TuiTheme,
        activity_frame: usize,
    ) -> Option<Vec<Line>> {
        let lines = match self {
            Self::Banner(data) => render_banner::render_welcome_banner(data, inner_width, theme),
            Self::UserMessage(content) => {
                render_banner::render_user_message(content, inner_width, theme)
            }
            Self::Status { text, severity } => {
                render_status::render_status(text, *severity, inner_width, theme)
            }
            Self::McpStartupStatus { data } => render_mcp_startup::render_mcp_startup_status(
                data,
                inner_width,
                theme,
                activity_frame,
            ),
            Self::QueuedMessage { text, is_steer } => {
                render_banner::render_queued_message(text, *is_steer, inner_width, theme)
            }
            Self::AssistantMessage { content } => {
                render_banner::render_assistant_message(content, inner_width, theme)
            }
            Self::ThinkingBlock {
                content,
                phase,
                expanded,
            } => render_thinking::render_thinking_block(
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

    fn render_structured_entry(
        &self,
        inner_width: usize,
        theme: &TuiTheme,
        activity_frame: usize,
    ) -> Vec<Line> {
        match self {
            Self::ToolRun { component } => render_tool_run(component, inner_width, theme),
            Self::ShellRun { component } => component.render(inner_width, theme),
            Self::ApprovalPrompt(data) => render_approval_prompt(data, inner_width, theme),
            Self::Image { metadata, .. } => {
                styled_wrap(metadata, inner_width, render_status::status_style(theme))
            }
            Self::Compaction {
                phase,
                percent,
                compacted_message_count,
                tokens_before,
                tokens_after,
            } => render_status::render_compaction(
                *phase,
                *percent,
                *compacted_message_count,
                *tokens_before,
                *tokens_after,
                inner_width,
                theme,
                activity_frame,
            ),
            Self::GoalCard {
                kind,
                objective,
                detail,
                turns,
            } => render_goal::render_goal_card(
                *kind,
                objective,
                detail.as_deref(),
                *turns,
                inner_width,
                theme,
            ),
            Self::SkillActivation {
                names,
                body,
                expanded,
            } => render_skill_activation(names, body, *expanded, inner_width, theme),
            Self::Delegate { component } => render_delegate_card(component, inner_width, theme),
            Self::DelegateGroup { component } => component.render_with_theme(inner_width, theme),
            Self::DelegateSwarm { component } => render_swarm_card(component, inner_width, theme),
            Self::Workflow { component } => render_workflow_card(component, inner_width, theme),
            Self::Banner(_)
            | Self::UserMessage(_)
            | Self::Status { .. }
            | Self::McpStartupStatus { .. }
            | Self::AssistantMessage { .. }
            | Self::ThinkingBlock { .. }
            | Self::QueuedMessage { .. } => unreachable!("message entries handled above"),
        }
    }

    pub fn on_render_tick(&mut self, now_ms: u64) -> bool {
        match self {
            Self::Delegate { component } => component.on_render_tick(now_ms),
            Self::DelegateGroup { component } => component.on_render_tick(now_ms),
            Self::DelegateSwarm { component } => component.on_render_tick(now_ms),
            Self::McpStartupStatus { data } => {
                matches!(data.phase, McpStartupPhase::Connecting)
            }
            _ => false,
        }
    }

    /// Whether this entry's rendered output is static — does not depend on
    /// `activity_frame` or per-tick internal animation. Static entries can be
    /// render-cached; live entries must be re-rendered every frame.
    ///
    /// `ToolRun` entries are excluded because they go through group rendering
    /// (`render_ordered_tools`), not the per-entry cache path.
    #[must_use]
    pub fn is_render_cacheable(&self) -> bool {
        match self {
            // MCP startup status uses activity_frame spinner when connecting.
            Self::McpStartupStatus { data } => !matches!(data.phase, McpStartupPhase::Connecting),
            // ThinkingBlock uses activity_frame spinner when streaming.
            Self::ThinkingBlock { phase, .. } => *phase == ThinkingPhase::Complete,
            // Live entries (per-tick animation) and ToolRun (group rendering)
            // are never cached individually.
            Self::Delegate { .. }
            | Self::DelegateGroup { .. }
            | Self::DelegateSwarm { .. }
            | Self::Compaction { .. }
            | Self::ToolRun { .. } => false,
            // All other entries (Banner, UserMessage, AssistantMessage, Status,
            // QueuedMessage, ShellRun, ApprovalPrompt, Image, GoalCard,
            // SkillActivation, Workflow) are static.
            _ => true,
        }
    }

    #[must_use]
    pub fn copy_parts(&self) -> (&'static str, String) {
        if let Some(parts) = copy::simple_copy_parts(self) {
            return parts;
        }
        copy::complex_copy_parts(self)
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

pub(super) fn format_token_count_usize(tokens: usize) -> String {
    if tokens >= 1_000_000 {
        format!("{}m", tokens / 1_000_000)
    } else if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

/// Wrap `text` to `width` and emit each wrapped row as a styled [`Line`].
pub(super) fn styled_wrap(text: &str, width: usize, style: Style) -> Vec<Line> {
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
    // Render the plan content box (PlanTransition only).
    if let Some(plan_content) = &data.plan_content {
        let plan_box = PlanBoxComponent::new(plan_content.clone(), data.plan_path.clone());
        let box_lines = plan_box.render(width, theme);
        for line in box_lines {
            rows.push(line);
        }
        rows.push(Line::raw(""));
    }
    // Render preset revision suggestions (PlanTransition only).
    if !data.suggestions.is_empty() {
        rows.push(Line::styled("  Suggestions:", title));
        for (index, suggestion) in data.suggestions.iter().enumerate() {
            let number = index + 1;
            let is_selected = data.selected_suggestion == Some(index);
            let prefix = if is_selected { "  ▶ " } else { "    " };
            let style = if is_selected { selected } else { body };
            rows.extend(styled_wrap_with_prefix(
                &format!("{}. {}", number, suggestion.label),
                width,
                prefix,
                "     ",
                style,
            ));
            if !suggestion.description.is_empty() {
                rows.extend(styled_wrap_with_indent(
                    &suggestion.description,
                    width,
                    7,
                    7,
                    muted,
                ));
            }
        }
        rows.push(Line::raw(""));
    }
    // Build the option list dynamically. For plan reviews with custom
    // options, use the model-supplied labels so the list matches the
    // chrome one-to-one. Otherwise fall back to a single "Approve once".
    // The session-approval (Layer 1) and prefix-rule (Layer 2) options
    // appear only when their labels are `Some`, so numeric shortcuts and
    // the feedback-input index track the visible list.
    let mut options: Vec<String> = if data.plan_option_labels.is_empty() {
        vec!["Approve once".to_owned()]
    } else {
        data.plan_option_labels
            .iter()
            .map(|label| format!("Approach: {label}"))
            .collect()
    };
    if let Some(label) = &data.session_option_label {
        options.push(label.clone());
    }
    if let Some(label) = &data.prefix_option_label {
        options.push(label.clone());
    }
    for suggestion in &data.suggestions {
        options.push(format!("Suggestion: {}", suggestion.label));
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

fn render_tool_run(component: &ToolCallComponent, width: usize, theme: &TuiTheme) -> Vec<Line> {
    let mut component = component.clone();
    component.render_with_theme(width, theme)
}

fn render_delegate_card(
    component: &DelegateCardComponent,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    component.render_with_theme(width, theme)
}

fn render_swarm_card(component: &SwarmCardComponent, width: usize, theme: &TuiTheme) -> Vec<Line> {
    component.render_with_theme(width, theme)
}

fn render_workflow_card(
    component: &WorkflowCardComponent,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    component.render_with_theme(width, theme)
}

const SKILL_ACTIVATION_PREVIEW_LINES: usize = 3;

fn render_skill_activation(
    names: &[String],
    body: &str,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    let activation = Style::default().fg(theme.status_warn).bold();
    let skill_name = Style::default().fg(theme.brand).bold();
    let thinking = render_thinking::thinking_style(theme);
    let muted = Style::default().fg(theme.text_muted);
    let name_list = names.join(", ");

    let mut rows = Vec::new();
    rows.push(
        Line::from_spans(vec![
            Span::styled("✦ Skill activated: ", activation),
            Span::styled(name_list, skill_name),
        ])
        .truncate_to_width(width),
    );
    rows.push(Line::styled("━".repeat(width.max(1)), activation));

    let body_lines = skill_body_lines(body, width.max(1));
    let visible_count = if expanded {
        body_lines.len()
    } else {
        body_lines.len().min(SKILL_ACTIVATION_PREVIEW_LINES)
    };
    for line in body_lines.iter().take(visible_count) {
        rows.push(Line::styled(line.clone(), thinking));
    }
    if !expanded && body_lines.len() > visible_count {
        let remaining = body_lines.len() - visible_count;
        rows.push(Line::styled(
            format!("… {remaining} more lines (ctrl+o to expand)"),
            muted,
        ));
    }

    rows.push(Line::raw(""));
    rows
}

fn skill_body_lines(body: &str, width: usize) -> Vec<String> {
    body.lines()
        .flat_map(|line| {
            if line.is_empty() {
                vec![String::new()]
            } else {
                wrap_width(line, width)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitive::theme::TuiTheme;

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
        let lines = render_banner::render_welcome_banner(&data, 60, &TuiTheme::default());
        for line in &lines {
            let width = crate::primitive::visible_width(&line.to_ansi());
            assert!(
                width == 60 || width == 0,
                "line width mismatch: {:?}",
                line.text()
            );
        }
        // The right edge of both logo rows should use the left half-block
        // glyph, not the square corner glyph '┐'.
        for logo_line in [&lines[2], &lines[3], &lines[4]] {
            assert!(!logo_line.text().contains('┐'));
        }
        assert!(
            lines[2]
                .text()
                .contains("\u{2590}\u{2588}\u{259b}  \u{2588}\u{258c}  Welcome to Neo!")
        );
        assert!(lines[3].text().contains(
            "\u{2590}\u{2588} \u{2588} \u{2588}\u{258c}  Send /help for help information."
        ));
        assert!(
            lines[4]
                .text()
                .contains("\u{2590}\u{2588}  \u{2599}\u{2588}\u{258c}")
        );
        let ansi = lines[2].to_ansi();
        assert!(ansi.contains("\x1b[38;2;63;247;255m"));
        assert!(ansi.contains("\x1b[38;2;255;79;216m"));
        assert!(ansi.contains("\x1b[38;2;138;92;255m"));
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
    fn skill_activation_renders_aggregate_collapsed_preview() {
        let entry = TranscriptEntry::skill_activated(
            vec!["skill_one".to_owned(), "skill_two".to_owned()],
            "\
foo
bar
test test test
bonjour
hello
test test test test
hola
amigo",
        );
        let lines = entry
            .render(60, &TuiTheme::default())
            .into_iter()
            .collect::<Vec<_>>();
        let text = lines.iter().map(Line::text).collect::<Vec<_>>();

        assert_eq!(text[0], "✦ Skill activated: skill_one, skill_two");
        assert!(text[1].starts_with("━"));
        assert_eq!(text[2], "foo");
        assert_eq!(text[3], "bar");
        assert_eq!(text[4], "test test test");
        assert_eq!(text[5], "… 5 more lines (ctrl+o to expand)");
        assert!(
            !text.iter().any(|line| line.contains("/skill:")),
            "{text:?}"
        );

        let header_spans = lines[0].spans();
        assert_eq!(header_spans[0].text(), "✦ Skill activated: ");
        assert_eq!(
            header_spans[0].style().fg,
            Some(TuiTheme::default().status_warn)
        );
        assert_eq!(header_spans[1].text(), "skill_one, skill_two");
        assert_eq!(header_spans[1].style().fg, Some(TuiTheme::default().brand));
        assert_eq!(
            lines[2].spans()[0].style().fg,
            Some(TuiTheme::default().text_muted)
        );
        assert!(lines[2].spans()[0].style().italic);
    }

    #[test]
    fn skill_activation_expands_full_body() {
        let entry = TranscriptEntry::skill_activated(
            vec!["skill_one".to_owned(), "skill_two".to_owned()],
            "foo\nbar\ntest test test\nbonjour\nhello\ntest test test test\nhola\namigo",
        );
        let mut entry = entry;
        if let TranscriptEntry::SkillActivation { expanded, .. } = &mut entry {
            *expanded = true;
        }
        let lines = entry
            .render(60, &TuiTheme::default())
            .into_iter()
            .map(|l| l.text().clone())
            .collect::<Vec<_>>();

        assert_eq!(lines[0], "✦ Skill activated: skill_one, skill_two");
        assert!(lines.contains(&"bonjour".to_owned()));
        assert!(lines.contains(&"amigo".to_owned()));
        assert!(!lines.iter().any(|l| l.contains("ctrl+o to expand")));
    }

    #[test]
    fn compaction_in_progress_renders_spinner_phase_and_progress_bar() {
        let entry = TranscriptEntry::Compaction {
            phase: Some(neo_agent_core::CompactionPhase::Summarizing),
            percent: 70,
            compacted_message_count: 0,
            tokens_before: 0,
            tokens_after: 0,
        };
        let lines = entry
            .render_with_activity_frame(80, &TuiTheme::default(), 0)
            .into_iter()
            .map(|l| l.text().clone())
            .collect::<Vec<_>>();
        let text = lines.join("");
        assert!(text.contains("Compacting context"), "{text}");
        assert!(text.contains("Summarizing"), "{text}");
        assert!(text.contains("70%"), "{text}");
        assert!(text.contains('█'), "{text}");
    }

    #[test]
    fn compaction_complete_renders_token_reduction() {
        let entry = TranscriptEntry::Compaction {
            phase: Some(neo_agent_core::CompactionPhase::Applying),
            percent: 100,
            compacted_message_count: 852,
            tokens_before: 192_000,
            tokens_after: 24_000,
        };
        let lines = entry
            .render_with_activity_frame(80, &TuiTheme::default(), 0)
            .into_iter()
            .map(|l| l.text().clone())
            .collect::<Vec<_>>();
        let text = lines.join("");
        assert!(text.contains("Compaction complete"), "{text}");
        assert!(text.contains("852"), "{text}");
        assert!(text.contains("192k"), "{text}");
        assert!(text.contains("24k"), "{text}");
    }

    #[test]
    fn approval_prompt_renders_suggestions() {
        let data = ApprovalPromptData {
            id: "test-id".to_owned(),
            title: "Plan Review".to_owned(),
            details: vec!["Ready to build with this plan?".to_owned()],
            queued_label: String::new(),
            queued_count: 0,
            selected: 0,
            feedback_input: String::new(),
            resolved: None,
            session_option_label: None,
            prefix_option_label: None,
            plan_content: None,
            plan_path: None,
            plan_option_labels: Vec::new(),
            suggestions: vec![
                PlanSuggestion {
                    label: "Keep 85% window".to_owned(),
                    description: "Keep compaction window at 85%.".to_owned(),
                    feedback: Some("Keep compaction at 85%.".to_owned()),
                },
                PlanSuggestion {
                    label: "Accept as-is".to_owned(),
                    description: "No changes needed.".to_owned(),
                    feedback: Some("No changes.".to_owned()),
                },
            ],
            selected_suggestion: None,
        };
        let lines = TranscriptEntry::ApprovalPrompt(data)
            .render(80, &TuiTheme::default())
            .into_iter()
            .map(|l| l.text().clone())
            .collect::<Vec<_>>();
        let text = lines.join("\n");
        assert!(text.contains("Suggestions:"), "{text}");
        assert!(text.contains("1. Keep 85% window"), "{text}");
        assert!(text.contains("Keep compaction window at 85%."), "{text}");
        assert!(text.contains("2. Accept as-is"), "{text}");
    }

    #[test]
    fn approval_prompt_highlights_selected_suggestion() {
        let data = ApprovalPromptData {
            id: "test-id".to_owned(),
            title: "Plan Review".to_owned(),
            details: vec!["Ready?".to_owned()],
            queued_label: String::new(),
            queued_count: 0,
            // Options: [0] Approve once, [1] Suggestion, [2] Reject, [3] Revise.
            selected: 3,
            feedback_input: "Keep compaction at 85%.".to_owned(),
            resolved: None,
            session_option_label: None,
            prefix_option_label: None,
            plan_content: None,
            plan_path: None,
            plan_option_labels: Vec::new(),
            suggestions: vec![PlanSuggestion {
                label: "Keep 85% window".to_owned(),
                description: "Keep compaction window at 85%.".to_owned(),
                feedback: Some("Keep compaction at 85%.".to_owned()),
            }],
            selected_suggestion: Some(0),
        };
        let lines = TranscriptEntry::ApprovalPrompt(data)
            .render(80, &TuiTheme::default())
            .into_iter()
            .map(|l| l.text().clone())
            .collect::<Vec<_>>();
        let text = lines.join("\n");
        assert!(text.contains("feedback: Keep compaction at 85%."), "{text}");
    }
}
