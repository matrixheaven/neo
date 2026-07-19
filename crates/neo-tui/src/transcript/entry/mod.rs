use crate::primitive::theme::TuiTheme;
use crate::primitive::wrap_width;
use crate::primitive::{Color, Component, Expandable, Finalization, Style, visible_width};
use crate::primitive::{Line, Span};
use crate::terminal_image::{
    ImageDisplayOptions, ImageRenderPolicy, ImageSource, TerminalImageCapabilities,
};
use crate::transcript::DelegateCardComponent;
use crate::transcript::DelegateGroupComponent;
use crate::transcript::InstructionCardComponent;
use crate::transcript::PlanBoxComponent;
use crate::transcript::ShellRunComponent;
use crate::transcript::SwarmCardComponent;
use crate::transcript::ToolCallComponent;
use crate::transcript::WorkflowCardComponent;
use neo_agent_core::{
    ApprovalAction, ApprovalPresentation, ApprovalRequest, ApprovalResolution,
    SkillInvocationOutcome, SkillInvocationSource,
};
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

/// Transcript display lifecycle for one approval request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApprovalDisplayState {
    Pending,
    Resolved(ApprovalResolution),
    Abandoned,
}

/// Transcript entry for a canonical approval request.
///
/// Holds the immutable runtime-owned request plus mutable view state only.
/// Labels are presentation-only; option actions are never reconstructed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalPromptData {
    pub request: ApprovalRequest,
    pub selected: usize,
    pub feedback_input: String,
    #[serde(default)]
    pub feedback_active: bool,
    pub state: ApprovalDisplayState,
    /// UI-only queue badge for additional pending approvals waiting behind this
    /// active prompt. Not part of the protocol request/response.
    #[serde(default)]
    pub queued_count: usize,
}

impl ApprovalPromptData {
    #[must_use]
    pub fn id(&self) -> &str {
        self.request.id.as_str()
    }

    #[must_use]
    pub fn title(&self) -> &str {
        match &self.request.presentation {
            ApprovalPresentation::Command { title, .. }
            | ApprovalPresentation::Tool { title, .. }
            | ApprovalPresentation::Plan { title, .. }
            | ApprovalPresentation::Goal { title, .. } => title.as_str(),
        }
    }

    #[must_use]
    pub fn is_pending(&self) -> bool {
        matches!(self.state, ApprovalDisplayState::Pending)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptImageAttachment {
    pub id: String,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    pub placeholder: String,
    pub payload: Vec<u8>,
}

impl TranscriptImageAttachment {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        mime_type: impl Into<String>,
        width: u32,
        height: u32,
        placeholder: impl Into<String>,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            id: id.into(),
            mime_type: mime_type.into(),
            width,
            height,
            placeholder: placeholder.into(),
            payload,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEntry {
    Banner(BannerData),
    UserMessage {
        content: String,
        images: Vec<TranscriptImageAttachment>,
    },
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
    RetryStatus {
        data: RetryStatusData,
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
        source: SkillInvocationSource,
        outcome: SkillInvocationOutcome,
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
    /// Finalized metadata-only card for one instruction epoch. Task 7 owns
    /// insertion/routing; the entry never renders instruction bodies.
    InstructionEpoch {
        component: InstructionCardComponent,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryPhase {
    Waiting,
    Connecting,
    Exhausted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryStatusData {
    pub turn: u32,
    pub retry: u32,
    pub max_retries: u32,
    pub phase: RetryPhase,
    pub delay_ms: u64,
    pub started_at_ms: u64,
    pub error_code: String,
    pub message: String,
}

pub(crate) fn monotonic_time_ms() -> u64 {
    static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    ORIGIN
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
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
            McpStartupPhase::Cancelled => format!(
                "MCP server \"{}\" startup interrupted ({})",
                self.id, self.transport
            ),
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
    Cancelled,
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
        Self::UserMessage {
            content: content.into(),
            images: Vec::new(),
        }
    }

    #[must_use]
    pub fn user_message_with_images(
        content: impl Into<String>,
        images: Vec<TranscriptImageAttachment>,
    ) -> Self {
        Self::UserMessage {
            content: content.into(),
            images,
        }
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
    pub const fn instruction_epoch(component: InstructionCardComponent) -> Self {
        Self::InstructionEpoch { component }
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
    pub const fn retry_status(data: RetryStatusData) -> Self {
        Self::RetryStatus { data }
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
    pub fn skill_invocation(
        names: Vec<String>,
        source: SkillInvocationSource,
        outcome: SkillInvocationOutcome,
        body: impl Into<String>,
    ) -> Self {
        Self::SkillActivation {
            names,
            source,
            outcome,
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
    pub fn is_expandable(&self) -> bool {
        matches!(
            self,
            Self::ToolRun { .. }
                | Self::ThinkingBlock { .. }
                | Self::SkillActivation { .. }
                | Self::DelegateSwarm { .. }
                | Self::InstructionEpoch { .. }
        )
    }

    pub fn set_expanded(&mut self, expanded: bool) -> bool {
        match self {
            Self::ToolRun { component } => {
                if component.is_expanded() == expanded {
                    return false;
                }
                component.set_expanded(expanded);
                true
            }
            Self::ThinkingBlock {
                expanded: current, ..
            }
            | Self::SkillActivation {
                expanded: current, ..
            } => {
                if *current == expanded {
                    return false;
                }
                *current = expanded;
                true
            }
            Self::DelegateSwarm { component } => {
                if component.is_expanded() == expanded {
                    return false;
                }
                component.set_expanded(expanded);
                true
            }
            Self::InstructionEpoch { component } => {
                if component.is_expanded() == expanded {
                    return false;
                }
                component.set_expanded(expanded);
                true
            }
            _ => false,
        }
    }

    #[must_use]
    pub fn finalization(&self) -> Finalization {
        match self {
            Self::ThinkingBlock { phase, .. } => match phase {
                ThinkingPhase::Streaming => Finalization::Live,
                ThinkingPhase::Complete => Finalization::Finalized,
            },
            Self::ToolRun { component } => component.finalization(),
            Self::ShellRun { component } => component.finalization(),
            Self::ApprovalPrompt(data) => {
                if data.is_pending() {
                    Finalization::Live
                } else {
                    Finalization::Finalized
                }
            }
            Self::Compaction { phase, percent, .. } => {
                if *phase == Some(neo_agent_core::CompactionPhase::Applying) && *percent >= 100 {
                    Finalization::Finalized
                } else {
                    Finalization::Live
                }
            }
            Self::RetryStatus { data } => {
                if data.phase == RetryPhase::Exhausted {
                    Finalization::Finalized
                } else {
                    Finalization::Live
                }
            }
            Self::McpStartupStatus { data } => {
                if matches!(data.phase, McpStartupPhase::Connecting) {
                    Finalization::Live
                } else {
                    Finalization::Finalized
                }
            }
            Self::QueuedMessage { .. } => Finalization::Live,
            Self::Delegate { component } => component.finalization(),
            Self::DelegateGroup { component } => component.finalization(),
            Self::DelegateSwarm { component } => component.finalization(),
            Self::Workflow { component } => component.finalization(),
            Self::InstructionEpoch { component } => component.finalization(),
            Self::Banner(_)
            | Self::UserMessage { .. }
            | Self::AssistantMessage { .. }
            | Self::Image { .. }
            | Self::Status { .. }
            | Self::GoalCard { .. }
            | Self::SkillActivation { .. } => Finalization::Finalized,
        }
    }

    pub fn interrupt(&mut self) -> bool {
        if self.finalization() == Finalization::Finalized {
            return false;
        }
        match self {
            Self::ThinkingBlock { phase, .. } => {
                *phase = ThinkingPhase::Complete;
                true
            }
            Self::ToolRun { component } => component.set_terminal_status(
                crate::shell::ToolStatusKind::Cancelled,
                Some("Interrupted when terminal exited".to_owned()),
            ),
            Self::ShellRun { component } => component.interrupt(),
            Self::ApprovalPrompt(data) => {
                if data.is_pending() {
                    data.state = ApprovalDisplayState::Abandoned;
                    true
                } else {
                    false
                }
            }
            Self::Compaction { percent, .. } => {
                let percent = *percent;
                *self = Self::status(format!(
                    "Compaction interrupted at {percent}% when terminal exited"
                ));
                true
            }
            Self::RetryStatus { data } => {
                let retry = data.retry;
                *self = Self::status(format!("Reconnect interrupted during attempt {retry}"));
                true
            }
            Self::McpStartupStatus { data } => {
                data.phase = McpStartupPhase::Cancelled;
                true
            }
            Self::QueuedMessage { text, .. } => {
                let text = text.clone();
                *self = Self::status(format!("Queued message not sent before exit: {text}"));
                true
            }
            Self::Delegate { component } => component.interrupt(),
            Self::DelegateGroup { component } => component.interrupt(),
            Self::DelegateSwarm { component } => component.interrupt(),
            Self::Workflow { component } => component.interrupt(),
            _ => false,
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
        self.render_with_image_context(
            width,
            theme,
            activity_frame,
            ImageRenderPolicy::default(),
            TerminalImageCapabilities::default(),
        )
    }

    #[must_use]
    pub fn render_with_image_context(
        &self,
        width: usize,
        theme: &TuiTheme,
        activity_frame: usize,
        image_render_policy: ImageRenderPolicy,
        image_capabilities: TerminalImageCapabilities,
    ) -> Vec<Line> {
        // Every `Line` returned here MUST map to exactly one terminal row:
        // content is split on `\n` and soft-wrapped to `width` so no line ever
        // carries an embedded newline. The renderer's diff/scroll math treats
        // each `Vec<String>` entry as one screen row, so an un-split long line
        // would corrupt the coordinate model and garble streaming output.
        let inner_width = width.max(1);
        self.render_inner(
            inner_width,
            theme,
            activity_frame,
            image_render_policy,
            image_capabilities,
        )
    }

    fn render_inner(
        &self,
        inner_width: usize,
        theme: &TuiTheme,
        activity_frame: usize,
        image_render_policy: ImageRenderPolicy,
        image_capabilities: TerminalImageCapabilities,
    ) -> Vec<Line> {
        if let Some(lines) = self.render_message_entry(
            inner_width,
            theme,
            activity_frame,
            image_render_policy,
            image_capabilities,
        ) {
            return lines;
        }
        self.render_structured_entry(
            inner_width,
            theme,
            activity_frame,
            image_render_policy,
            image_capabilities,
        )
    }

    fn render_message_entry(
        &self,
        inner_width: usize,
        theme: &TuiTheme,
        activity_frame: usize,
        image_render_policy: ImageRenderPolicy,
        image_capabilities: TerminalImageCapabilities,
    ) -> Option<Vec<Line>> {
        let lines = match self {
            Self::Banner(data) => render_banner::render_welcome_banner(data, inner_width, theme),
            Self::UserMessage { content, images } => render_banner::render_user_message(
                content,
                images,
                inner_width,
                theme,
                image_render_policy,
                image_capabilities,
            ),
            Self::Status { text, severity } => {
                render_status::render_status(text, *severity, inner_width, theme)
            }
            Self::RetryStatus { data } => {
                render_status::render_retry_status(data, inner_width, theme, activity_frame)
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
        image_render_policy: ImageRenderPolicy,
        image_capabilities: TerminalImageCapabilities,
    ) -> Vec<Line> {
        match self {
            Self::ToolRun { component } => render_tool_run(component, inner_width, theme),
            Self::ShellRun { component } => component.render(inner_width, theme),
            Self::ApprovalPrompt(data) => render_approval_prompt(data, inner_width, theme),
            Self::Image {
                id,
                mime_type,
                metadata,
                payload,
                ..
            } => render_image_entry(
                id,
                mime_type,
                metadata,
                payload.as_deref(),
                inner_width,
                theme,
                image_render_policy,
                image_capabilities,
            ),
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
                source,
                outcome,
                body,
                expanded,
            } => render_skill_activation(
                names,
                *source,
                *outcome,
                body,
                *expanded,
                inner_width,
                theme,
            ),
            Self::Delegate { component } => render_delegate_card(component, inner_width, theme),
            Self::DelegateGroup { component } => component.render_with_theme(inner_width, theme),
            Self::DelegateSwarm { component } => render_swarm_card(component, inner_width, theme),
            Self::Workflow { component } => render_workflow_card(component, inner_width, theme),
            Self::InstructionEpoch { component } => component.render_with_theme(inner_width, theme),
            Self::Banner(_)
            | Self::UserMessage { .. }
            | Self::Status { .. }
            | Self::RetryStatus { .. }
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
            Self::RetryStatus { data } => {
                matches!(data.phase, RetryPhase::Waiting | RetryPhase::Connecting)
            }
            _ => false,
        }
    }

    #[must_use]
    pub fn has_visible_animation(&self) -> bool {
        match self {
            Self::ThinkingBlock {
                phase: ThinkingPhase::Streaming,
                ..
            }
            | Self::McpStartupStatus {
                data:
                    McpStartupStatusData {
                        phase: McpStartupPhase::Connecting,
                        ..
                    },
            }
            | Self::RetryStatus {
                data:
                    RetryStatusData {
                        phase: RetryPhase::Waiting | RetryPhase::Connecting,
                        ..
                    },
            }
            | Self::Compaction { .. } => true,
            Self::ToolRun { component } => component.has_visible_animation(),
            Self::ShellRun { component } => component.has_visible_animation(),
            Self::Delegate { component } => {
                Component::finalization(component) == Finalization::Live
            }
            Self::DelegateGroup { component } => {
                Component::finalization(component) == Finalization::Live
            }
            Self::DelegateSwarm { component } => {
                Component::finalization(component) == Finalization::Live
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
    #[allow(clippy::match_same_arms)]
    pub fn is_render_cacheable(&self) -> bool {
        match self {
            // MCP startup status uses activity_frame spinner when connecting.
            Self::McpStartupStatus { data } => !matches!(data.phase, McpStartupPhase::Connecting),
            Self::RetryStatus { data } => data.phase == RetryPhase::Exhausted,
            // ThinkingBlock uses activity_frame spinner when streaming.
            Self::ThinkingBlock { phase, .. } => *phase == ThinkingPhase::Complete,
            // Image rendering depends on terminal image capabilities and render policy.
            Self::UserMessage { images, .. } if !images.is_empty() => false,
            Self::Image { .. } => false,
            // Live entries (per-tick animation) and ToolRun (group rendering)
            // are never cached individually.
            Self::Delegate { .. }
            | Self::DelegateGroup { .. }
            | Self::DelegateSwarm { .. }
            | Self::Compaction { .. }
            | Self::ToolRun { .. } => false,
            // All other entries (Banner, text-only UserMessage, AssistantMessage, Status,
            // QueuedMessage, ShellRun, ApprovalPrompt, GoalCard,
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
            metadata,
            payload,
            ..
        } = self
        else {
            return None;
        };
        let payload = payload.as_ref()?;
        image_render_policy
            .render_inline_image_bytes(
                id,
                mime_type,
                payload,
                metadata.clone(),
                image_capabilities,
                &ImageDisplayOptions::bounded(1, 1),
            )
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

#[allow(clippy::too_many_lines)]
fn render_approval_prompt(data: &ApprovalPromptData, width: usize, theme: &TuiTheme) -> Vec<Line> {
    let border = Style::default().fg(theme.status_warn);
    let title = Style::default().fg(theme.status_warn).bold();
    let body = Style::default().fg(theme.text_primary);
    let muted = Style::default().fg(theme.text_muted);
    let selected = Style::default().fg(theme.status_ok).bold();
    match &data.state {
        ApprovalDisplayState::Resolved(resolution) => {
            let label = resolution_display_label(resolution);
            return vec![Line::styled(format!("approval: {label}"), muted)];
        }
        ApprovalDisplayState::Abandoned => {
            return vec![Line::styled("approval: Abandoned", muted)];
        }
        ApprovalDisplayState::Pending => {}
    }

    let line = "\u{2500}".repeat(width.max(1));
    let mut rows = vec![Line::styled(line.clone(), border)];
    rows.extend(styled_wrap_with_indent(
        &format!("▶ {}", data.title()),
        width,
        2,
        2,
        title,
    ));
    rows.push(Line::raw(""));
    for detail in presentation_detail_lines(&data.request.presentation) {
        rows.extend(styled_wrap_with_indent(&detail, width, 2, 4, body));
    }
    rows.push(Line::raw(""));
    if let ApprovalPresentation::Plan { markdown, path, .. } = &data.request.presentation
        && !markdown.trim().is_empty()
    {
        let plan_path = path.as_ref().map(|p| p.display().to_string());
        let plan_box = PlanBoxComponent::new(markdown.clone(), plan_path);
        rows.extend(plan_box.render(width, theme));
        rows.push(Line::raw(""));
    }

    for (index, option) in data.request.options.iter().enumerate() {
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
            &format!("{}. {}", index + 1, option.label),
            width,
            prefix,
            "     ",
            style,
        ));
        if let Some(description) = &option.description {
            rows.extend(styled_wrap_with_indent(description, width, 7, 7, muted));
        }
    }
    rows.push(Line::raw(""));
    let revise_selected = data
        .request
        .options
        .get(data.selected)
        .is_some_and(|option| {
            matches!(
                option.action,
                ApprovalAction::RevisePlan { .. } | ApprovalAction::ReviseGoal { .. }
            )
        });
    if data.feedback_active && revise_selected {
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
        rows.extend(styled_wrap_with_indent(
            &format!("queued: {} {suffix} waiting", data.queued_count),
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

fn presentation_detail_lines(presentation: &ApprovalPresentation) -> Vec<String> {
    match presentation {
        ApprovalPresentation::Command { command, cwd, .. } => {
            let mut lines = Vec::new();
            if let Some(cwd) = cwd {
                lines.push(format!("cwd: {}", cwd.display()));
            }
            lines.push(format!("$ {command}"));
            lines
        }
        ApprovalPresentation::Tool { details, .. } => details.clone(),
        ApprovalPresentation::Plan { summary, .. } => summary
            .as_ref()
            .filter(|s| !s.trim().is_empty())
            .cloned()
            .into_iter()
            .collect(),
        ApprovalPresentation::Goal {
            objective,
            completion_criterion,
            phases,
            ..
        } => {
            let mut lines = vec![objective.clone()];
            if let Some(criterion) = completion_criterion {
                lines.push(criterion.clone());
            }
            lines.extend(phases.iter().cloned());
            lines
        }
    }
}

fn resolution_display_label(resolution: &ApprovalResolution) -> String {
    match resolution {
        // Pure reject actions render a stable past-tense status word. Other
        // selected actions keep the event's canonical option label.
        ApprovalResolution::Selected {
            action: ApprovalAction::Reject | ApprovalAction::RejectPlan | ApprovalAction::RejectGoal,
            ..
        } => "Rejected".to_owned(),
        ApprovalResolution::Selected { label, .. } => label.clone(),
        ApprovalResolution::Cancelled { reason } => format!("cancelled ({reason:?})"),
    }
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

#[allow(clippy::too_many_arguments)]
fn render_image_entry(
    id: &str,
    mime_type: &str,
    metadata: &str,
    payload: Option<&[u8]>,
    width: usize,
    theme: &TuiTheme,
    image_render_policy: ImageRenderPolicy,
    image_capabilities: TerminalImageCapabilities,
) -> Vec<Line> {
    let Some(payload) = payload else {
        return styled_wrap(metadata, width, render_status::status_style(theme));
    };
    let Some((image_width, image_height)) =
        crate::terminal_image::detect_image_dimensions(payload, mime_type)
    else {
        return styled_wrap(metadata, width, render_status::status_style(theme));
    };
    let placeholder = format!("[image ({image_width}x{image_height})]");
    let display = ImageDisplayOptions::thumbnail(image_width, image_height, placeholder)
        .with_max_cols(thumbnail_cols(width));
    image_render_policy
        .render_inline_image_bytes(
            id,
            mime_type,
            payload,
            metadata.to_owned(),
            image_capabilities,
            &display,
        )
        .lines
        .into_iter()
        .map(Line::raw)
        .collect()
}

fn thumbnail_cols(width: usize) -> u32 {
    u32::try_from(
        width
            .saturating_sub(2)
            .min(ImageDisplayOptions::DEFAULT_MAX_COLS as usize),
    )
    .unwrap_or(ImageDisplayOptions::DEFAULT_MAX_COLS)
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
    source: SkillInvocationSource,
    outcome: SkillInvocationOutcome,
    body: &str,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    let (marker, label, status_style) = match outcome {
        SkillInvocationOutcome::Activated => (
            "✦",
            "Skill activated: ",
            Style::default().fg(theme.status_warn).bold(),
        ),
        SkillInvocationOutcome::Failed => (
            "✕",
            "Skill failed: ",
            Style::default().fg(theme.status_error).bold(),
        ),
    };
    let skill_name = Style::default().fg(theme.brand).bold();
    let thinking = render_thinking::thinking_style(theme);
    let muted = Style::default().fg(theme.text_muted);
    let error = Style::default().fg(theme.status_error);
    let name_list = names.join(", ");
    let source = match source {
        SkillInvocationSource::Auto => "auto",
        SkillInvocationSource::Manual => "manual",
    };
    let suffix = format!(" · {source}");
    let full_prefix = format!("{marker} {label}");
    let prefix = if visible_width(&full_prefix) + visible_width(&suffix) < width {
        full_prefix
    } else {
        format!("{marker} ")
    };
    let name_width = width.saturating_sub(visible_width(&prefix) + visible_width(&suffix));
    let visible_name = crate::primitive::truncate_to_width(&name_list, name_width);

    let mut rows = Vec::new();
    rows.push(
        Line::from_spans(vec![
            Span::styled(prefix, status_style),
            Span::styled(visible_name, skill_name),
            Span::styled(suffix, muted),
        ])
        .truncate_to_width(width),
    );

    let body = body.trim();
    if body.is_empty() {
        rows.push(Line::raw(""));
        return rows;
    }
    if outcome == SkillInvocationOutcome::Activated {
        rows.push(Line::styled("━".repeat(width.max(1)), status_style));
    }

    let body_width = if outcome == SkillInvocationOutcome::Failed {
        width.saturating_sub(2).max(1)
    } else {
        width.max(1)
    };
    let body_lines = skill_body_lines(body, body_width);
    let visible_count = if expanded {
        body_lines.len()
    } else {
        body_lines.len().min(SKILL_ACTIVATION_PREVIEW_LINES)
    };
    for line in body_lines.iter().take(visible_count) {
        let line = if outcome == SkillInvocationOutcome::Failed {
            format!("  {line}")
        } else {
            line.clone()
        };
        rows.push(Line::styled(
            line,
            if outcome == SkillInvocationOutcome::Failed {
                error
            } else {
                thinking
            },
        ));
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
        let entry = TranscriptEntry::skill_invocation(
            vec!["skill_one".to_owned(), "skill_two".to_owned()],
            SkillInvocationSource::Manual,
            SkillInvocationOutcome::Activated,
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

        assert_eq!(text[0], "✦ Skill activated: skill_one, skill_two · manual");
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
        let entry = TranscriptEntry::skill_invocation(
            vec!["skill_one".to_owned(), "skill_two".to_owned()],
            SkillInvocationSource::Manual,
            SkillInvocationOutcome::Activated,
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

        assert_eq!(lines[0], "✦ Skill activated: skill_one, skill_two · manual");
        assert!(lines.contains(&"bonjour".to_owned()));
        assert!(lines.contains(&"amigo".to_owned()));
        assert!(!lines.iter().any(|l| l.contains("ctrl+o to expand")));
    }

    #[test]
    fn skill_activation_preserves_source_at_narrow_width() {
        let entry = TranscriptEntry::skill_invocation(
            vec!["using-aegis".to_owned()],
            SkillInvocationSource::Auto,
            SkillInvocationOutcome::Activated,
            "",
        );

        let header = entry.render(24, &TuiTheme::default())[0].text().clone();

        assert!(
            header.contains("· auto"),
            "source should remain visible: {header}"
        );
        assert!(visible_width(&header) <= 24, "header should fit: {header}");
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

    fn plan_prompt_data(
        selected: usize,
        feedback_active: bool,
        feedback_input: String,
    ) -> ApprovalPromptData {
        use neo_agent_core::{ApprovalAction, ApprovalOption, PermissionOperation};
        ApprovalPromptData {
            request: ApprovalRequest {
                turn: 1,
                id: "test-id".to_owned(),
                operation: PermissionOperation::PlanTransition,
                presentation: ApprovalPresentation::Plan {
                    title: "Plan Review".to_owned(),
                    path: None,
                    markdown: String::new(),
                    summary: Some("Ready?".to_owned()),
                },
                options: vec![
                    ApprovalOption {
                        label: "Approve".to_owned(),
                        description: None,
                        action: ApprovalAction::ApprovePlan { selection: None },
                    },
                    ApprovalOption {
                        label: "Suggestion: Keep 85% window".to_owned(),
                        description: Some("Keep compaction window at 85%.".to_owned()),
                        action: ApprovalAction::RevisePlan {
                            preset_feedback: Some("Keep compaction at 85%.".to_owned()),
                        },
                    },
                    ApprovalOption {
                        label: "Reject".to_owned(),
                        description: None,
                        action: ApprovalAction::RejectPlan,
                    },
                    ApprovalOption {
                        label: "Reject with feedback".to_owned(),
                        description: None,
                        action: ApprovalAction::RevisePlan {
                            preset_feedback: None,
                        },
                    },
                ],
            },
            selected,
            feedback_input,
            feedback_active,
            state: ApprovalDisplayState::Pending,
            queued_count: 0,
        }
    }

    #[test]
    fn approval_prompt_renders_canonical_options() {
        let data = plan_prompt_data(0, false, String::new());
        let lines = TranscriptEntry::ApprovalPrompt(data)
            .render(80, &TuiTheme::default())
            .into_iter()
            .map(|l| l.text().clone())
            .collect::<Vec<_>>();
        let text = lines.join("\n");
        assert!(text.contains("1. Approve"), "{text}");
        assert!(text.contains("2. Suggestion: Keep 85% window"), "{text}");
        assert!(text.contains("Keep compaction window at 85%."), "{text}");
        assert!(text.contains("3. Reject"), "{text}");
    }

    #[test]
    fn approval_prompt_highlights_selected_revision_feedback() {
        let data = plan_prompt_data(1, true, "Keep compaction at 85%.".to_owned());
        let lines = TranscriptEntry::ApprovalPrompt(data)
            .render(80, &TuiTheme::default())
            .into_iter()
            .map(|l| l.text().clone())
            .collect::<Vec<_>>();
        let text = lines.join("\n");
        assert!(text.contains("feedback: Keep compaction at 85%."), "{text}");
    }

    #[test]
    fn approval_prompt_hides_feedback_until_input_is_active() {
        let data = plan_prompt_data(3, false, String::new());
        let lines = TranscriptEntry::ApprovalPrompt(data)
            .render(80, &TuiTheme::default())
            .into_iter()
            .map(|l| l.text().clone())
            .collect::<Vec<_>>();
        let text = lines.join("\n");
        assert!(!text.contains("feedback:"), "{text}");
    }

    #[test]
    fn approval_prompt_shows_feedback_when_input_is_active() {
        let data = plan_prompt_data(3, true, String::new());
        let lines = TranscriptEntry::ApprovalPrompt(data)
            .render(80, &TuiTheme::default())
            .into_iter()
            .map(|l| l.text().clone())
            .collect::<Vec<_>>();
        let text = lines.join("\n");
        assert!(text.contains("feedback: ▌"), "{text}");
    }
}
