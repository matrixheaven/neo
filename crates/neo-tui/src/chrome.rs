use std::{
    collections::{BTreeMap, VecDeque},
    ops::Range,
    path::{Path, PathBuf},
    time::SystemTime,
};

use neo_agent_core::{AgentEvent, PermissionMode, PermissionOperation};

use crate::{
    ansi::Color,
    components::{truncate_width, visible_width},
    core::InputResult,
    dialogs::{
        ApiKeyInputState, ChoicePickerState, CustomRegistryImportState, ModelSelectorState,
        ProviderManagerState, TabbedModelSelectorState,
    },
    image::{ImageRenderPolicy, TerminalImageCapabilities},
    input::{InputEvent, KeybindingAction},
    widgets::{
        QuestionDialogAction, QuestionDisplayData, QuestionDisplayOption, QuestionResult,
        QuestionStateMachine, TodoDisplayItem, TodoDisplayStatus,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TuiTheme {
    pub background: Color,
    pub surface: Color,
    pub surface_border: Color,
    pub brand: Color,
    pub status_ok: Color,
    pub status_error: Color,
    pub status_warn: Color,
    pub text_muted: Color,
    pub text_primary: Color,
    pub prompt: Color,
    pub composer_bg: Color,
    pub user_message: Color,
    pub user_bg: Color,
    pub diff_added: Color,
    pub diff_removed: Color,
    pub diff_hunk: Color,
    pub diff_context: Color,
    pub selection_bg: Color,
    pub status_pending: Color,
    pub status_cancelled: Color,
    pub approval_bg: Color,
    pub approval_border: Color,
    pub approval_title: Color,
    pub selected_fg: Color,
    pub selected_bg: Color,
    pub overlay_border: Color,
    pub footer_permission_allow: Color,
    pub footer_permission_ask: Color,
    pub footer_permission_deny: Color,
    pub footer_working: Color,
    pub footer_context_ok: Color,
    pub footer_context_warn: Color,
    pub footer_context_critical: Color,
}

impl Default for TuiTheme {
    fn default() -> Self {
        Self {
            background: Color::Reset,
            surface: Color::Rgb(31, 35, 43),
            surface_border: Color::Rgb(75, 88, 104),
            brand: Color::Rgb(198, 120, 221),
            status_ok: Color::Rgb(78, 200, 126),
            status_error: Color::Rgb(232, 84, 84),
            status_warn: Color::Rgb(232, 168, 56),
            text_muted: Color::Rgb(139, 148, 158),
            // Soft white body text instead of pure terminal white.
            text_primary: Color::Rgb(198, 208, 245),
            prompt: Color::Rgb(198, 208, 245),
            composer_bg: Color::Reset,
            user_message: Color::Rgb(229, 200, 144),
            user_bg: Color::Reset,
            diff_added: Color::Rgb(78, 200, 126),
            diff_removed: Color::Rgb(232, 84, 84),
            diff_hunk: Color::Rgb(232, 168, 56),
            diff_context: Color::Rgb(139, 148, 158),
            selection_bg: Color::DarkGray,
            status_pending: Color::Rgb(139, 148, 158),
            status_cancelled: Color::DarkGray,
            approval_bg: Color::Reset,
            approval_border: Color::Rgb(75, 88, 104),
            approval_title: Color::Rgb(232, 168, 56),
            selected_fg: Color::Black,
            // Selection / overlay track the magenta brand color.
            selected_bg: Color::Rgb(198, 120, 221),
            overlay_border: Color::Rgb(198, 120, 221),
            footer_permission_allow: Color::Rgb(78, 200, 126),
            footer_permission_ask: Color::Rgb(198, 120, 221),
            footer_permission_deny: Color::Rgb(232, 84, 84),
            footer_working: Color::Rgb(198, 120, 221),
            footer_context_ok: Color::Rgb(139, 148, 158),
            footer_context_warn: Color::Rgb(232, 168, 56),
            footer_context_critical: Color::Rgb(232, 84, 84),
        }
    }
}

impl TuiTheme {
    #[must_use]
    pub const fn with_background(mut self, color: Color) -> Self {
        self.background = color;
        self
    }

    #[must_use]
    pub const fn with_surface(mut self, color: Color) -> Self {
        self.surface = color;
        self.composer_bg = color;
        self.approval_bg = color;
        self
    }

    #[must_use]
    pub const fn with_surface_border(mut self, color: Color) -> Self {
        self.surface_border = color;
        self.overlay_border = color;
        self.approval_border = color;
        self
    }

    #[must_use]
    pub const fn with_brand(mut self, color: Color) -> Self {
        self.brand = color;
        self.overlay_border = color;
        self
    }

    #[must_use]
    pub const fn with_status_ok(mut self, color: Color) -> Self {
        self.status_ok = color;
        self
    }

    #[must_use]
    pub const fn with_status_error(mut self, color: Color) -> Self {
        self.status_error = color;
        self
    }

    #[must_use]
    pub const fn with_status_warn(mut self, color: Color) -> Self {
        self.status_warn = color;
        self.approval_title = color;
        self
    }

    #[must_use]
    pub const fn with_text_muted(mut self, color: Color) -> Self {
        self.text_muted = color;
        self
    }

    #[must_use]
    pub const fn with_text_primary(mut self, color: Color) -> Self {
        self.text_primary = color;
        self
    }

    #[must_use]
    pub const fn with_prompt(mut self, color: Color) -> Self {
        self.prompt = color;
        self
    }

    #[must_use]
    pub const fn with_composer_bg(mut self, color: Color) -> Self {
        self.composer_bg = color;
        self
    }

    #[must_use]
    pub const fn with_user_message(mut self, color: Color) -> Self {
        self.user_message = color;
        self
    }

    #[must_use]
    pub const fn with_footer_permission_allow(mut self, color: Color) -> Self {
        self.footer_permission_allow = color;
        self
    }

    #[must_use]
    pub const fn with_footer_permission_ask(mut self, color: Color) -> Self {
        self.footer_permission_ask = color;
        self
    }

    #[must_use]
    pub const fn with_footer_permission_deny(mut self, color: Color) -> Self {
        self.footer_permission_deny = color;
        self
    }

    #[must_use]
    pub const fn with_footer_working(mut self, color: Color) -> Self {
        self.footer_working = color;
        self
    }

    #[must_use]
    pub const fn with_footer_context_ok(mut self, color: Color) -> Self {
        self.footer_context_ok = color;
        self
    }

    #[must_use]
    pub const fn with_footer_context_warn(mut self, color: Color) -> Self {
        self.footer_context_warn = color;
        self
    }

    #[must_use]
    pub const fn with_footer_context_critical(mut self, color: Color) -> Self {
        self.footer_context_critical = color;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromeMode {
    Editing,
    Streaming,
    Overlay,
    Approval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DevelopmentMode {
    #[default]
    Normal,
    Plan,
    Goal(GoalModeStatus),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GoalModeStatus {
    #[default]
    Pending,
    Active,
    Paused,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextWindow {
    pub used_tokens: Option<u32>,
    pub max_tokens: u32,
}

impl ContextWindow {
    #[must_use]
    pub const fn new(max_tokens: u32) -> Self {
        Self {
            used_tokens: None,
            max_tokens,
        }
    }

    #[must_use]
    pub const fn with_used_tokens(mut self, used_tokens: u32) -> Self {
        self.used_tokens = Some(used_tokens);
        self
    }

    #[must_use]
    pub fn label(self) -> String {
        let used = self
            .used_tokens
            .map_or_else(|| "--".to_owned(), format_token_count);
        format!("ctx {used}/{}", format_token_count(self.max_tokens))
    }
}

fn format_token_count(tokens: u32) -> String {
    if tokens >= 1_000_000 {
        format!("{}m", tokens / 1_000_000)
    } else if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

fn review_title(operation: PermissionOperation) -> &'static str {
    match operation {
        PermissionOperation::GoalTransition => "Goal Review",
        _ => "Plan Review",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeoChromeState {
    title: String,
    session_label: String,
    model_label: String,
    workspace_root: PathBuf,
    context_window: Option<ContextWindow>,
    activity_frame: usize,
    prompt: PromptState,
    copy_buffer: Option<String>,
    mode: ChromeMode,
    overlays: Vec<Overlay>,
    next_overlay_id: OverlayId,
    focused_overlay: Option<OverlayId>,
    pending_approvals: VecDeque<ApprovalRequestModal>,
    pending_question_result: Option<QuestionResult>,
    image_render_policy: ImageRenderPolicy,
    image_capabilities: TerminalImageCapabilities,
    theme: TuiTheme,
    permission_mode: PermissionMode,
    /// Current development mode indicator (for footer display).
    plan_mode_active: bool,
    development_mode: DevelopmentMode,
    /// Current todo list for the `TodoPanel`.
    todo_items: Vec<TodoDisplayItem>,
    /// Optional custom label shown in the footer as a working indicator.
    custom_working_label: Option<String>,
    /// Whether the current model has thinking enabled (shown in the footer).
    thinking_enabled: bool,
    /// Optional persistent exit-confirmation message shown in the footer.
    exit_confirmation_label: Option<String>,
    /// Formatted git branch/status badge shown after the workspace path.
    git_status_label: Option<String>,
}

impl NeoChromeState {
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        session_label: impl Into<String>,
        model_label: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            title: title.into(),
            session_label: session_label.into(),
            model_label: model_label.into(),
            workspace_root: workspace_root.into(),
            context_window: None,
            activity_frame: 0,
            prompt: PromptState::default(),
            copy_buffer: None,
            mode: ChromeMode::Editing,
            overlays: Vec::new(),
            next_overlay_id: OverlayId::default(),
            focused_overlay: None,
            pending_approvals: VecDeque::new(),
            pending_question_result: None,
            image_render_policy: ImageRenderPolicy::default(),
            image_capabilities: TerminalImageCapabilities::default(),
            theme: TuiTheme::default(),
            permission_mode: PermissionMode::default(),
            plan_mode_active: false,
            development_mode: DevelopmentMode::Normal,
            todo_items: Vec::new(),
            custom_working_label: None,
            thinking_enabled: false,
            exit_confirmation_label: None,
            git_status_label: None,
        }
    }

    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    #[must_use]
    pub fn session_label(&self) -> &str {
        &self.session_label
    }

    #[must_use]
    pub fn model_label(&self) -> &str {
        &self.model_label
    }

    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        self.workspace_root.as_path()
    }

    #[must_use]
    pub const fn context_window(&self) -> Option<ContextWindow> {
        self.context_window
    }

    pub const fn set_context_window(&mut self, context_window: Option<ContextWindow>) {
        self.context_window = context_window;
    }

    #[must_use]
    pub fn context_window_label(&self) -> Option<String> {
        self.context_window.map(ContextWindow::label)
    }

    #[must_use]
    pub fn working_label(&self) -> Option<String> {
        if let Some(label) = &self.custom_working_label {
            return Some(label.clone());
        }
        matches!(self.mode, ChromeMode::Streaming).then(|| "working · esc interrupt".to_owned())
    }

    /// Set a custom footer working label. Pass `None` to clear it.
    pub fn set_custom_working_label(&mut self, label: Option<String>) {
        self.custom_working_label = label;
    }

    #[must_use]
    pub fn thinking_enabled(&self) -> bool {
        self.thinking_enabled
    }

    /// Toggle the thinking-enabled indicator shown in the footer.
    pub fn set_thinking_enabled(&mut self, enabled: bool) {
        self.thinking_enabled = enabled;
    }

    #[must_use]
    pub fn exit_confirmation_label(&self) -> Option<&str> {
        self.exit_confirmation_label.as_deref()
    }

    /// Set a persistent exit-confirmation label in the footer. Pass `None` to clear.
    pub fn set_exit_confirmation_label(&mut self, label: Option<String>) {
        self.exit_confirmation_label = label;
    }

    #[must_use]
    pub fn git_status_label(&self) -> Option<&str> {
        self.git_status_label.as_deref()
    }

    pub fn set_git_status_label(&mut self, label: Option<String>) {
        self.git_status_label = label;
    }

    #[must_use]
    pub const fn permission_mode(&self) -> PermissionMode {
        self.permission_mode
    }

    pub fn set_permission_mode(&mut self, mode: PermissionMode) {
        self.permission_mode = mode;
    }

    #[must_use]
    pub fn permission_badge(&self) -> (&'static str, Color) {
        match self.permission_mode {
            PermissionMode::Manual => ("manual", self.theme().footer_permission_ask),
            PermissionMode::Auto => ("auto", self.theme().footer_permission_allow),
            PermissionMode::Yolo => ("yolo", self.theme().footer_permission_deny),
        }
    }

    #[must_use]
    pub fn cwd_label(&self) -> String {
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            if let Ok(rest) = self.workspace_root.strip_prefix(&home) {
                if rest.as_os_str().is_empty() {
                    return "~".to_owned();
                }
                return format!("~/{}", rest.display());
            }
        }
        self.workspace_root.display().to_string()
    }

    #[must_use]
    pub fn context_color(&self) -> Color {
        let Some(context) = self.context_window else {
            return self.theme().footer_context_ok;
        };
        if context.max_tokens == 0 {
            return self.theme().footer_context_ok;
        }
        let Some(used) = context.used_tokens else {
            return self.theme().footer_context_ok;
        };
        let ratio = f64::from(used) / f64::from(context.max_tokens);
        if ratio >= 0.9 {
            self.theme().footer_context_critical
        } else if ratio >= 0.7 {
            self.theme().footer_context_warn
        } else {
            self.theme().footer_context_ok
        }
    }

    #[must_use]
    pub const fn activity_frame(&self) -> usize {
        self.activity_frame
    }

    pub fn advance_activity_frame(&mut self) {
        self.activity_frame = self.activity_frame.wrapping_add(1);
    }

    #[must_use]
    pub const fn mode(&self) -> ChromeMode {
        self.mode
    }

    pub fn set_plan_mode(&mut self, active: bool) {
        self.plan_mode_active = active;
        self.development_mode = if active {
            DevelopmentMode::Plan
        } else if matches!(self.development_mode, DevelopmentMode::Plan) {
            DevelopmentMode::Normal
        } else {
            self.development_mode
        };
    }

    #[must_use]
    pub const fn is_plan_mode(&self) -> bool {
        self.plan_mode_active
    }

    #[must_use]
    pub const fn development_mode(&self) -> DevelopmentMode {
        self.development_mode
    }

    pub fn set_development_mode(&mut self, mode: DevelopmentMode) {
        self.development_mode = mode;
        self.plan_mode_active = matches!(mode, DevelopmentMode::Plan);
    }

    #[must_use]
    pub fn todo_items(&self) -> &[TodoDisplayItem] {
        &self.todo_items
    }

    pub fn set_todo_items(&mut self, items: Vec<TodoDisplayItem>) {
        self.todo_items = items;
    }

    #[must_use]
    pub fn has_todos(&self) -> bool {
        !self.todo_items.is_empty()
    }

    /// Clear the todo panel (e.g. when all items are done).
    pub fn clear_todos(&mut self) {
        self.todo_items.clear();
    }

    #[must_use]
    pub const fn theme(&self) -> TuiTheme {
        self.theme
    }

    pub const fn set_theme(&mut self, theme: TuiTheme) {
        self.theme = theme;
    }

    #[must_use]
    pub const fn image_render_policy(&self) -> ImageRenderPolicy {
        self.image_render_policy
    }

    pub const fn set_image_render_policy(&mut self, policy: ImageRenderPolicy) {
        self.image_render_policy = policy;
    }

    #[must_use]
    pub const fn image_capabilities(&self) -> TerminalImageCapabilities {
        self.image_capabilities
    }

    pub const fn set_image_capabilities(&mut self, capabilities: TerminalImageCapabilities) {
        self.image_capabilities = capabilities;
    }

    #[must_use]
    pub const fn prompt(&self) -> &PromptState {
        &self.prompt
    }

    pub fn prompt_mut(&mut self) -> &mut PromptState {
        &mut self.prompt
    }

    #[must_use]
    pub fn copy_buffer(&self) -> Option<&str> {
        self.copy_buffer.as_deref()
    }

    pub fn copy_prompt_text(&mut self) -> Option<String> {
        let copied = self.prompt.copy_text()?;
        self.copy_buffer = Some(copied.clone());
        Some(copied)
    }

    pub fn set_session_label(&mut self, session_label: impl Into<String>) {
        self.session_label = session_label.into();
    }

    pub fn set_model_label(&mut self, model_label: impl Into<String>) {
        self.model_label = model_label.into();
    }

    #[must_use]
    pub fn overlays(&self) -> &[Overlay] {
        &self.overlays
    }

    #[must_use]
    pub const fn focused_overlay_id(&self) -> Option<OverlayId> {
        self.focused_overlay
    }

    #[must_use]
    pub fn focused_overlay(&self) -> Option<&Overlay> {
        self.focused_overlay
            .and_then(|id| self.overlays.iter().find(|overlay| overlay.id == id))
    }

    pub fn focused_overlay_mut(&mut self) -> Option<&mut Overlay> {
        self.focused_overlay
            .and_then(|id| self.overlays.iter_mut().find(|overlay| overlay.id == id))
    }

    pub fn submit_prompt(&mut self) -> Option<String> {
        let submitted = self.prompt.text.trim_end().to_owned();
        if submitted.trim().is_empty() {
            return None;
        }

        self.prompt.remember_history(submitted.clone());
        self.prompt.clear_after_submit();
        self.mode = ChromeMode::Streaming;
        Some(submitted)
    }

    pub fn apply_stream_update(&mut self, update: StreamUpdate) {
        match update {
            StreamUpdate::AssistantStarted { .. }
            | StreamUpdate::TextDelta { .. }
            | StreamUpdate::ToolStarted { .. }
            | StreamUpdate::ToolUpdated { .. }
            | StreamUpdate::ToolFinished { .. }
            | StreamUpdate::ThinkingStarted
            | StreamUpdate::ThinkingDelta { .. } => {
                self.mode = ChromeMode::Streaming;
            }
            StreamUpdate::Error { text } => {
                let _ = text;
                self.mode = self.overlay_mode();
            }
            StreamUpdate::TurnFinished | StreamUpdate::RunFinished { .. } => {
                self.mode = self.overlay_mode();
            }
            StreamUpdate::PlanModeChanged { active } => self.set_plan_mode(active),
            StreamUpdate::TodoUpdated { todos } => {
                self.todo_items = todos;
            }
            StreamUpdate::QuestionRequested { id, questions } => {
                self.push_question_overlay(id, questions);
            }
            StreamUpdate::ThinkingFinished | StreamUpdate::SkillActivated { .. } => {}
        }
    }

    #[allow(clippy::too_many_lines)]
    pub fn apply_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::MessageStarted { .. }
            | AgentEvent::TextDelta { .. }
            | AgentEvent::ThinkingStarted { .. }
            | AgentEvent::ThinkingDelta { .. }
            | AgentEvent::ThinkingFinished { .. }
            | AgentEvent::ToolCallStarted { .. }
            | AgentEvent::ToolCallArgumentsDelta { .. }
            | AgentEvent::ToolCallFinished { .. }
            | AgentEvent::ToolExecutionStarted { .. }
            | AgentEvent::ToolExecutionUpdate { .. }
            | AgentEvent::ToolExecutionFinished { .. }
            | AgentEvent::ShellCommandStarted { .. }
            | AgentEvent::ShellCommandFinished { .. } => {
                self.mode = ChromeMode::Streaming;
            }
            AgentEvent::ApprovalRequested {
                id,
                operation,
                subject,
                arguments,
                ..
            } => {
                let is_review = matches!(
                    operation,
                    PermissionOperation::PlanTransition | PermissionOperation::GoalTransition
                );
                let body = if arguments.is_null() {
                    subject
                } else {
                    format!("{subject}\n{arguments}")
                };
                self.pending_approvals.push_back(if is_review {
                    ApprovalRequestModal::new_review(id, review_title(operation), body)
                } else {
                    ApprovalRequestModal::new(id, format!("{operation:?} approval"), body)
                });
                self.focused_overlay = None;
                self.mode = ChromeMode::Approval;
            }
            AgentEvent::ContextWindowUpdated { used_tokens, .. } => {
                if let Some(context_window) = &mut self.context_window {
                    *context_window = context_window.with_used_tokens(used_tokens);
                }
            }
            AgentEvent::TurnFinished { .. } => {
                self.apply_stream_update(StreamUpdate::TurnFinished);
            }
            AgentEvent::Error { message, .. } => {
                self.apply_stream_update(StreamUpdate::Error { text: message });
            }
            AgentEvent::RunFinished { turn, stop_reason } => {
                self.apply_stream_update(StreamUpdate::RunFinished { turn, stop_reason });
            }
            AgentEvent::SteeringQueued { .. }
            | AgentEvent::FollowUpQueued { .. }
            | AgentEvent::QueueDrained { .. }
            | AgentEvent::CompactionStarted { .. }
            | AgentEvent::CompactionProgress { .. }
            | AgentEvent::CompactionApplied { .. }
            | AgentEvent::MessageAppended { .. }
            | AgentEvent::RunStarted { .. }
            | AgentEvent::TurnStarted { .. }
            | AgentEvent::MessageFinished { .. }
            | AgentEvent::TokenUsage { .. }
            | AgentEvent::TerminalSessionStarted { .. }
            | AgentEvent::TerminalSessionOutput { .. }
            | AgentEvent::TerminalSessionFinished { .. }
            | AgentEvent::SkillActivated { .. } => {}
            AgentEvent::GoalStarted { .. } | AgentEvent::GoalResumed { .. } => {
                self.set_development_mode(DevelopmentMode::Goal(GoalModeStatus::Active));
            }
            AgentEvent::GoalPaused { .. } => {
                self.set_development_mode(DevelopmentMode::Goal(GoalModeStatus::Paused));
            }
            AgentEvent::GoalBlocked { .. } => {
                self.set_development_mode(DevelopmentMode::Goal(GoalModeStatus::Blocked));
            }
            AgentEvent::GoalFinished { .. } => {
                self.set_development_mode(DevelopmentMode::Normal);
            }
            AgentEvent::PlanModeEntered { .. } => self.set_plan_mode(true),
            AgentEvent::PlanModeExited { .. } | AgentEvent::PlanModeCancelled { .. } => {
                self.set_plan_mode(false);
            }
            AgentEvent::PlanUpdated { enabled, .. } => self.set_plan_mode(enabled),
            AgentEvent::TodoUpdated { todos, .. } => {
                let display: Vec<TodoDisplayItem> = todos
                    .iter()
                    .map(|t| TodoDisplayItem {
                        title: t.title.clone(),
                        status: match t.status.as_str() {
                            "in_progress" => TodoDisplayStatus::InProgress,
                            "done" => TodoDisplayStatus::Done,
                            _ => TodoDisplayStatus::Pending,
                        },
                    })
                    .collect();
                self.todo_items = display;
            }
            AgentEvent::QuestionRequested { id, questions, .. } => {
                let display: Vec<QuestionDisplayData> = questions
                    .iter()
                    .map(|q| QuestionDisplayData {
                        question: q.question.clone(),
                        header: q.header.clone(),
                        body: q.body.clone(),
                        options: q
                            .options
                            .iter()
                            .map(|o| QuestionDisplayOption {
                                label: o.label.clone(),
                                description: o.description.clone(),
                            })
                            .collect(),
                        multi_select: q.multi_select,
                    })
                    .collect();
                self.push_question_overlay(id, display);
            }
        }
    }

    pub fn push_overlay(&mut self, mut overlay: Overlay) -> OverlayId {
        self.next_overlay_id = self.next_overlay_id.next();
        overlay.id = self.next_overlay_id;
        let id = overlay.id;
        self.overlays.push(overlay);
        self.focused_overlay = Some(id);
        self.mode = self.overlay_mode();
        id
    }

    pub fn focus_overlay(&mut self, id: OverlayId) -> bool {
        if self.overlays.iter().any(|overlay| overlay.id == id) {
            self.focused_overlay = Some(id);
            self.mode = self.overlay_mode();
            true
        } else {
            false
        }
    }

    pub fn close_overlay(&mut self, id: OverlayId) -> Option<Overlay> {
        let index = self.overlays.iter().position(|overlay| overlay.id == id)?;
        let overlay = self.overlays.remove(index);
        if self.focused_overlay == Some(id) {
            self.focused_overlay = self.overlays.last().map(|overlay| overlay.id);
        }
        self.mode = self.overlay_mode();
        Some(overlay)
    }

    pub fn close_focused_overlay(&mut self) -> Option<Overlay> {
        self.focused_overlay.and_then(|id| self.close_overlay(id))
    }

    pub fn close_question_overlay(&mut self, question_id: &str) -> Option<Overlay> {
        let id = self.overlays.iter().find_map(|overlay| {
            let OverlayKind::QuestionDialog(state) = &overlay.kind else {
                return None;
            };
            (state.id == question_id).then_some(overlay.id)
        })?;
        self.close_overlay(id)
    }

    pub fn clear_interrupted_turn_state(&mut self) -> Vec<String> {
        let mut question_ids = Vec::new();
        self.overlays.retain(|overlay| {
            let OverlayKind::QuestionDialog(state) = &overlay.kind else {
                return true;
            };
            question_ids.push(state.id.clone());
            false
        });
        if self
            .focused_overlay
            .is_some_and(|id| !self.overlays.iter().any(|overlay| overlay.id == id))
        {
            self.focused_overlay = self.overlays.last().map(|overlay| overlay.id);
        }
        self.mode = self.overlay_mode();
        question_ids
    }

    pub fn request_approval(
        &mut self,
        request_id: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> OverlayId {
        self.pending_approvals
            .push_back(ApprovalRequestModal::new(request_id, title, body));
        self.focused_overlay = None;
        self.mode = ChromeMode::Approval;
        OverlayId::default()
    }

    pub fn open_command_palette(
        &mut self,
        commands: impl IntoIterator<Item = CommandSpec>,
    ) -> OverlayId {
        self.push_overlay(Overlay::new(
            "commands",
            OverlayKind::CommandPalette(CommandPaletteState::new(commands)),
        ))
    }

    #[must_use]
    pub fn selected_command(&self) -> Option<CommandSpec> {
        let OverlayKind::CommandPalette(palette) = &self.focused_overlay()?.kind else {
            return None;
        };
        palette.confirm()
    }

    pub fn confirm_command_palette(&mut self) -> Option<CommandSpec> {
        let id = self.focused_overlay;
        let selected = self.selected_command()?;
        if let Some(id) = id {
            let _ = self.close_overlay(id);
        }
        Some(selected)
    }

    pub fn open_session_picker(
        &mut self,
        current_session_id: &str,
        scope: SessionPickerScope,
        items: impl IntoIterator<Item = SessionPickerItem>,
    ) -> OverlayId {
        self.push_overlay(Overlay::new(
            "sessions",
            OverlayKind::SessionPicker(SessionPickerState::new(
                items,
                current_session_id,
                scope,
                4,
            )),
        ))
    }

    #[must_use]
    pub fn selected_session(&self) -> Option<SessionPickerItem> {
        let OverlayKind::SessionPicker(picker) = &self.focused_overlay()?.kind else {
            return None;
        };
        picker.confirm()
    }

    pub fn confirm_session_picker(&mut self) -> Option<SessionPickerItem> {
        let id = self.focused_overlay;
        let selected = self.selected_session()?;
        if let Some(id) = id {
            let _ = self.close_overlay(id);
        }
        Some(selected)
    }

    /// Render the focused overlay as ANSI lines, if any.
    #[must_use]
    pub fn render_focused_overlay(&self, width: usize) -> Option<Vec<String>> {
        self.focused_overlay()?
            .render_standalone_lines(width, &self.theme)
    }

    pub fn open_model_picker(&mut self, items: impl IntoIterator<Item = PickerItem>) -> OverlayId {
        self.push_overlay(Overlay::new(
            "models",
            OverlayKind::ModelPicker(ModelPickerState::new(items)),
        ))
    }

    pub fn open_prompt_completion_picker(
        &mut self,
        prefix: PromptCompletionPrefix,
        items: impl IntoIterator<Item = PickerItem>,
    ) -> OverlayId {
        self.push_overlay(Overlay::new(
            "prompt-completion",
            OverlayKind::PromptCompletion(PromptCompletionState::new(prefix, items)),
        ))
    }

    #[must_use]
    pub fn selected_prompt_completion(&self) -> Option<PickerItem> {
        let OverlayKind::PromptCompletion(completions) = &self.focused_overlay()?.kind else {
            return None;
        };
        completions.selected_item()
    }

    pub fn confirm_prompt_completion(&mut self) -> Option<PickerItem> {
        let id = self.focused_overlay;
        let (prefix, item) = {
            let OverlayKind::PromptCompletion(completions) = &self.focused_overlay()?.kind else {
                return None;
            };
            (completions.prefix().clone(), completions.confirm()?)
        };
        self.prompt
            .replace_completion_prefix(&prefix, &item.value)?;
        if let Some(id) = id {
            let _ = self.close_overlay(id);
        }
        Some(item)
    }

    #[must_use]
    pub fn selected_model(&self) -> Option<PickerItem> {
        let OverlayKind::ModelPicker(picker) = &self.focused_overlay()?.kind else {
            return None;
        };
        picker.confirm()
    }

    pub fn confirm_model_picker(&mut self) -> Option<PickerItem> {
        let id = self.focused_overlay;
        let selected = self.selected_model()?;
        if let Some(id) = id {
            let _ = self.close_overlay(id);
        }
        Some(selected)
    }

    // -- Rich Neo dialog overlays ---------------------------------------

    pub fn open_model_selector(&mut self, opts: crate::dialogs::ModelSelectorOptions) -> OverlayId {
        let state = crate::dialogs::ModelSelectorState::new(opts);
        self.push_overlay(Overlay::new("models", OverlayKind::ModelSelector(state)))
    }

    pub fn open_tabbed_model_selector(
        &mut self,
        opts: crate::dialogs::TabbedModelSelectorOptions,
    ) -> OverlayId {
        let state = crate::dialogs::TabbedModelSelectorState::new(opts);
        self.push_overlay(Overlay::new(
            "models",
            OverlayKind::TabbedModelSelector(state),
        ))
    }

    pub fn open_provider_manager(
        &mut self,
        opts: &crate::dialogs::ProviderManagerOptions,
    ) -> OverlayId {
        let state = crate::dialogs::ProviderManagerState::new(opts);
        self.push_overlay(Overlay::new(
            "providers",
            OverlayKind::ProviderManager(state),
        ))
    }

    pub fn open_choice_picker(&mut self, opts: crate::dialogs::ChoicePickerOptions) -> OverlayId {
        let state = crate::dialogs::ChoicePickerState::new(opts);
        self.push_overlay(Overlay::new("choice", OverlayKind::ChoicePicker(state)))
    }

    pub fn open_api_key_input(&mut self, opts: crate::dialogs::ApiKeyInputOptions) -> OverlayId {
        let state = crate::dialogs::ApiKeyInputState::new(opts, self.theme);
        self.push_overlay(Overlay::new("api-key", OverlayKind::ApiKeyInput(state)))
    }

    pub fn open_custom_registry_import(
        &mut self,
        opts: crate::dialogs::CustomRegistryImportOptions,
    ) -> OverlayId {
        let state = crate::dialogs::CustomRegistryImportState::new(opts, self.theme);
        self.push_overlay(Overlay::new(
            "registry",
            OverlayKind::CustomRegistryImport(state),
        ))
    }

    /// Render the focused overlay (if any) into ANSI lines at the given width.
    #[must_use]
    pub fn focused_overlay_lines(&self, width: usize) -> Vec<String> {
        self.focused_overlay()
            .map_or_else(Vec::new, |overlay| overlay.render_lines(width, &self.theme))
    }

    /// Height in terminal lines the focused overlay wants to occupy.
    #[must_use]
    pub fn focused_overlay_height(&self) -> u16 {
        self.focused_overlay().map_or(0, Overlay::height)
    }

    /// Check if the focused overlay is one of the rich dialog types that
    /// handles its own keyboard input via `handle_input`.
    #[must_use]
    pub fn focused_overlay_is_rich_dialog(&self) -> bool {
        let Some(overlay) = self.focused_overlay() else {
            return false;
        };
        matches!(
            overlay.kind,
            OverlayKind::ModelSelector(_)
                | OverlayKind::TabbedModelSelector(_)
                | OverlayKind::ProviderManager(_)
                | OverlayKind::ChoicePicker(_)
                | OverlayKind::ApiKeyInput(_)
                | OverlayKind::CustomRegistryImport(_)
                | OverlayKind::QuestionDialog(_)
        )
    }

    /// Forward an input event to the focused rich dialog overlay.
    pub fn handle_focused_dialog_input(&mut self, input: InputEvent) -> InputResult {
        let Some(id) = self.focused_overlay else {
            return InputResult::Ignored;
        };
        let input = Self::translate_key_event_for_dialog(input);
        let Some(overlay) = self.overlays.iter_mut().find(|o| o.id == id) else {
            return InputResult::Ignored;
        };
        let mut close_overlay = false;
        let result = match &mut overlay.kind {
            OverlayKind::ModelSelector(state) => state.handle_input(&input),
            OverlayKind::TabbedModelSelector(state) => state.handle_input(&input),
            OverlayKind::ProviderManager(state) => state.handle_input(&input),
            OverlayKind::ChoicePicker(state) => state.handle_input(&input),
            OverlayKind::ApiKeyInput(state) => state.handle_input(&input),
            OverlayKind::CustomRegistryImport(state) => state.handle_input(input),
            OverlayKind::QuestionDialog(state) => {
                let result = state.handle_input(&input);
                if matches!(result, InputResult::Submitted | InputResult::Cancelled) {
                    if result == InputResult::Submitted {
                        self.pending_question_result = Some(QuestionResult {
                            id: state.id.clone(),
                            answers: state.compile_answers(),
                        });
                    }
                    close_overlay = true;
                }
                result
            }
            _ => InputResult::Ignored,
        };
        if close_overlay {
            let _ = self.close_overlay(id);
        }
        result
    }

    /// Convenience result accessors for rich dialogs
    fn translate_key_event_for_dialog(input: InputEvent) -> InputEvent {
        match &input {
            InputEvent::Key(key) => match key.as_str() {
                "enter" => InputEvent::Submit,
                "escape" => InputEvent::Cancel,
                "up" => InputEvent::Action(KeybindingAction::SelectUp),
                "down" => InputEvent::Action(KeybindingAction::SelectDown),
                "pageup" => InputEvent::Action(KeybindingAction::SelectPageUp),
                "pagedown" => InputEvent::Action(KeybindingAction::SelectPageDown),
                "tab" => InputEvent::Insert('\t'),
                "backspace" => InputEvent::Backspace,
                "delete" => InputEvent::Delete,
                "left" => InputEvent::MoveLeft,
                "right" => InputEvent::MoveRight,
                "home" => InputEvent::MoveHome,
                "end" => InputEvent::MoveEnd,
                _ => input,
            },
            _ => input,
        }
    }

    #[must_use]
    pub fn model_selector_result(&self) -> Option<&crate::dialogs::ModelSelectorResult> {
        let OverlayKind::ModelSelector(state) = &self.focused_overlay()?.kind else {
            return None;
        };
        state.result()
    }

    #[must_use]
    pub fn tabbed_model_selector_result(&self) -> Option<&crate::dialogs::ModelSelectorResult> {
        let OverlayKind::TabbedModelSelector(state) = &self.focused_overlay()?.kind else {
            return None;
        };
        state.result()
    }

    #[must_use]
    pub fn provider_manager_action(&self) -> Option<crate::dialogs::ProviderManagerAction> {
        let OverlayKind::ProviderManager(state) = &self.focused_overlay()?.kind else {
            return None;
        };
        state.action()
    }

    #[must_use]
    pub fn choice_picker_result(&self) -> Option<&crate::dialogs::ChoiceResult> {
        let OverlayKind::ChoicePicker(state) = &self.focused_overlay()?.kind else {
            return None;
        };
        state.result()
    }

    #[must_use]
    pub fn api_key_input_result(&self) -> Option<&crate::dialogs::ApiKeyInputResult> {
        let OverlayKind::ApiKeyInput(state) = &self.focused_overlay()?.kind else {
            return None;
        };
        state.result()
    }

    #[must_use]
    pub fn custom_registry_import_result(
        &self,
    ) -> Option<&crate::dialogs::CustomRegistryImportResult> {
        let OverlayKind::CustomRegistryImport(state) = &self.focused_overlay()?.kind else {
            return None;
        };
        state.result()
    }

    pub fn take_question_result(&mut self) -> Option<QuestionResult> {
        self.pending_question_result.take()
    }

    #[must_use]
    pub fn approval_choice(&self) -> Option<ApprovalChoice> {
        if let Some(approval) = self.pending_approvals.front() {
            return approval.modal.selected_choice();
        }
        let OverlayKind::Approval(modal) = &self.focused_overlay()?.kind else {
            return None;
        };
        modal.modal.selected_choice()
    }

    #[must_use]
    pub fn approval_is_pending(&self) -> bool {
        !self.pending_approvals.is_empty()
    }

    #[must_use]
    pub fn approval_selection(&self) -> Option<(&str, usize, &str)> {
        self.pending_approvals.front().map(|approval| {
            (
                approval.request_id.as_str(),
                approval.modal.selected,
                approval.feedback_input.as_str(),
            )
        })
    }

    pub fn choose_approval_number(&mut self, number: usize) -> Option<ApprovalResult> {
        let approval = self.pending_approvals.front_mut()?;
        if number == 0 || number > approval.modal.options.len() {
            return None;
        }
        approval.modal.selected = number - 1;
        if approval.is_collecting_feedback() {
            return None;
        }
        self.confirm_approval()
    }

    pub fn deny_approval(&mut self) -> Option<ApprovalResult> {
        if let Some(approval) = self.pending_approvals.front_mut() {
            if let Some(index) = approval
                .modal
                .options
                .iter()
                .position(|option| option.choice == ApprovalChoice::Deny)
            {
                approval.modal.selected = index;
            }
            return self.confirm_approval();
        }

        let id = self.focused_overlay;
        let overlay = self.focused_overlay()?;
        let OverlayKind::Approval(modal) = &overlay.kind else {
            return None;
        };
        let result = ApprovalResult {
            request_id: modal.request_id.clone(),
            choice: ApprovalChoice::Deny,
            feedback: None,
        };
        if let Some(id) = id {
            let _ = self.close_overlay(id);
        }
        Some(result)
    }

    pub fn cancel_all_approvals(&mut self) -> Vec<ApprovalResult> {
        let results = self
            .pending_approvals
            .drain(..)
            .map(|modal| ApprovalResult {
                request_id: modal.request_id,
                choice: ApprovalChoice::Deny,
                feedback: None,
            })
            .collect();
        self.mode = self.overlay_mode();
        results
    }

    pub fn confirm_approval(&mut self) -> Option<ApprovalResult> {
        if let Some(modal) = self.pending_approvals.pop_front() {
            let choice = modal.modal.selected_choice()?;
            let result = ApprovalResult {
                request_id: modal.request_id,
                choice,
                feedback: (choice == ApprovalChoice::Revise)
                    .then_some(modal.feedback_input)
                    .filter(|feedback| !feedback.is_empty()),
            };
            self.mode = self.overlay_mode();
            return Some(result);
        }

        let id = self.focused_overlay;
        let overlay = self.focused_overlay()?;
        let OverlayKind::Approval(modal) = &overlay.kind else {
            return None;
        };
        let result = ApprovalResult {
            request_id: modal.request_id.clone(),
            choice: modal.modal.selected_choice()?,
            feedback: None,
        };
        if let Some(id) = id {
            let _ = self.close_overlay(id);
        }
        Some(result)
    }

    // -- Question dialog overlay ---------------------------------------------

    pub fn push_question_overlay(
        &mut self,
        id: impl Into<String>,
        questions: Vec<QuestionDisplayData>,
    ) -> OverlayId {
        let state = QuestionStateMachine::new(id, questions);
        self.push_overlay(Overlay::new(
            "questions",
            OverlayKind::QuestionDialog(state),
        ))
    }

    #[must_use]
    pub fn question_dialog_state(&self) -> Option<&QuestionStateMachine> {
        let overlay = self.focused_overlay()?;
        match &overlay.kind {
            OverlayKind::QuestionDialog(state) => Some(state),
            _ => None,
        }
    }

    #[must_use]
    pub fn question_dialog_is_focused(&self) -> bool {
        self.focused_overlay()
            .is_some_and(|o| matches!(o.kind, OverlayKind::QuestionDialog(_)))
    }

    /// Process a crossterm key event in the question dialog.
    /// Returns the action produced (None if no question dialog is focused).
    #[must_use]
    pub fn handle_question_dialog_key(
        &mut self,
        event: crossterm::event::KeyEvent,
    ) -> Option<QuestionDialogAction> {
        let id = self.focused_overlay?;
        let action = {
            let overlay = self.overlays.iter_mut().find(|o| o.id == id)?;
            let OverlayKind::QuestionDialog(state) = &mut overlay.kind else {
                return None;
            };
            state.handle_key(event)
        };
        if matches!(
            action,
            QuestionDialogAction::Submit(_) | QuestionDialogAction::Cancel
        ) {
            self.close_overlay(id);
        }
        Some(action)
    }

    /// Confirm / submit the question dialog. Returns answers if all questions
    /// were answered.
    pub fn confirm_question(&mut self) -> Option<QuestionResult> {
        let id = self.focused_overlay?;
        let result = {
            let overlay = self.focused_overlay()?;
            let OverlayKind::QuestionDialog(state) = &overlay.kind else {
                return None;
            };
            if !state.is_complete() {
                return None;
            }
            QuestionResult {
                id: state.id.clone(),
                answers: state.compile_answers(),
            }
        };
        self.close_overlay(id);
        Some(result)
    }

    /// Cancel the question dialog. Returns the question id.
    pub fn cancel_question(&mut self) -> Option<String> {
        let id = self.focused_overlay?;
        let question_id = {
            let overlay = self.focused_overlay()?;
            let OverlayKind::QuestionDialog(state) = &overlay.kind else {
                return None;
            };
            state.id.clone()
        };
        self.close_overlay(id);
        Some(question_id)
    }

    pub fn move_overlay_selection_down(&mut self) {
        if let Some(approval) = self.pending_approvals.front_mut() {
            approval.move_down();
            self.mode = ChromeMode::Approval;
            return;
        }
        self.with_focused_overlay_mut(Overlay::move_selection_down);
    }

    pub fn move_overlay_selection_up(&mut self) {
        if let Some(approval) = self.pending_approvals.front_mut() {
            approval.move_up();
            self.mode = ChromeMode::Approval;
            return;
        }
        self.with_focused_overlay_mut(Overlay::move_selection_up);
    }

    pub fn handle_pending_approval_input(&mut self, input: InputEvent) -> Option<ApprovalResult> {
        let input = Self::translate_key_event_for_dialog(input);
        match input {
            InputEvent::Insert(character) => {
                if let Some(number) = approval_number(character)
                    && let Some(result) = self.choose_approval_number(number)
                {
                    return Some(result);
                }
                if let Some(approval) = self.pending_approvals.front_mut() {
                    approval.insert_feedback(&character.to_string());
                }
                None
            }
            InputEvent::Paste(text) => {
                if let Some(approval) = self.pending_approvals.front_mut() {
                    approval.insert_feedback(&text);
                }
                None
            }
            InputEvent::Backspace | InputEvent::Delete => {
                if let Some(approval) = self.pending_approvals.front_mut() {
                    approval.backspace_feedback();
                }
                None
            }
            InputEvent::Action(KeybindingAction::SelectDown) => {
                self.move_overlay_selection_down();
                None
            }
            InputEvent::Action(KeybindingAction::SelectUp) => {
                self.move_overlay_selection_up();
                None
            }
            InputEvent::Submit
            | InputEvent::Action(KeybindingAction::SelectConfirm | KeybindingAction::InputSubmit) => {
                self.confirm_approval()
            }
            InputEvent::Cancel | InputEvent::Action(KeybindingAction::SelectCancel) => {
                self.deny_approval()
            }
            _ => None,
        }
    }

    pub fn move_overlay_selection_page_down(&mut self) {
        self.with_focused_overlay_mut(Overlay::move_selection_page_down);
    }

    pub fn move_overlay_selection_page_up(&mut self) {
        self.with_focused_overlay_mut(Overlay::move_selection_page_up);
    }

    fn with_focused_overlay_mut(&mut self, action: impl FnOnce(&mut Overlay)) {
        let Some(id) = self.focused_overlay else {
            return;
        };
        if let Some(overlay) = self.overlays.iter_mut().find(|overlay| overlay.id == id) {
            action(overlay);
        }
    }

    fn overlay_mode(&self) -> ChromeMode {
        if !self.pending_approvals.is_empty() {
            return ChromeMode::Approval;
        }
        if let Some(overlay) = self.focused_overlay() {
            if matches!(
                overlay.kind,
                OverlayKind::Approval(_) | OverlayKind::QuestionDialog(_)
            ) {
                ChromeMode::Approval
            } else {
                ChromeMode::Overlay
            }
        } else {
            ChromeMode::Editing
        }
    }

    #[must_use]
    pub fn focused_overlay_blocks_prompt(&self) -> bool {
        if !self.pending_approvals.is_empty() {
            return true;
        }
        let Some(overlay) = self.focused_overlay() else {
            return false;
        };
        matches!(
            overlay.kind,
            OverlayKind::SessionPicker(_)
                | OverlayKind::ModelPicker(_)
                | OverlayKind::ModelSelector(_)
                | OverlayKind::TabbedModelSelector(_)
                | OverlayKind::ProviderManager(_)
                | OverlayKind::ChoicePicker(_)
                | OverlayKind::ApiKeyInput(_)
                | OverlayKind::CustomRegistryImport(_)
                | OverlayKind::QuestionDialog(_)
                | OverlayKind::Approval(_)
        )
    }
}

fn approval_number(character: char) -> Option<usize> {
    match character {
        '1' => Some(1),
        '2' => Some(2),
        '3' => Some(3),
        '4' => Some(4),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamUpdate {
    AssistantStarted {
        id: String,
    },
    TextDelta {
        text: String,
    },
    ToolStarted {
        id: String,
        name: String,
        detail: String,
    },
    ToolUpdated {
        id: String,
        detail: String,
    },
    ToolFinished {
        id: String,
        detail: String,
        success: bool,
        details: Option<serde_json::Value>,
    },
    ThinkingStarted,
    ThinkingDelta {
        text: String,
    },
    ThinkingFinished,
    Error {
        text: String,
    },
    TurnFinished,
    RunFinished {
        turn: u32,
        stop_reason: neo_agent_core::StopReason,
    },
    PlanModeChanged {
        active: bool,
    },
    TodoUpdated {
        todos: Vec<TodoDisplayItem>,
    },
    QuestionRequested {
        id: String,
        questions: Vec<QuestionDisplayData>,
    },
    SkillActivated {
        name: String,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OverlayId(u64);

impl OverlayId {
    #[must_use]
    const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlay {
    pub id: OverlayId,
    pub title: String,
    pub kind: OverlayKind,
}

impl Overlay {
    #[must_use]
    pub fn new(title: impl Into<String>, kind: OverlayKind) -> Self {
        Self {
            id: OverlayId::default(),
            title: title.into(),
            kind,
        }
    }

    pub fn move_selection_down(&mut self) {
        if let Some(list) = self.kind.list_selection_mut() {
            list.select_down();
            return;
        }
        match &mut self.kind {
            OverlayKind::Approval(request) => request.move_down(),
            OverlayKind::QuestionDialog(state) => state.move_cursor_down(),
            kind => handle_dialog_selection(kind, KeybindingAction::SelectDown),
        }
    }

    pub fn move_selection_up(&mut self) {
        if let Some(list) = self.kind.list_selection_mut() {
            list.select_up();
            return;
        }
        match &mut self.kind {
            OverlayKind::Approval(request) => request.move_up(),
            OverlayKind::QuestionDialog(state) => state.move_cursor_up(),
            kind => handle_dialog_selection(kind, KeybindingAction::SelectUp),
        }
    }

    pub fn move_selection_page_down(&mut self) {
        if let Some(list) = self.kind.list_selection_mut() {
            list.select_page_down();
        } else {
            handle_dialog_selection(&mut self.kind, KeybindingAction::SelectPageDown);
        }
    }

    pub fn move_selection_page_up(&mut self) {
        if let Some(list) = self.kind.list_selection_mut() {
            list.select_page_up();
        } else {
            handle_dialog_selection(&mut self.kind, KeybindingAction::SelectPageUp);
        }
    }

    #[must_use]
    fn render_standalone_lines(&self, width: usize, theme: &TuiTheme) -> Option<Vec<String>> {
        self.kind
            .session_picker_lines(width, theme)
            .or_else(|| self.kind.rich_dialog_lines(width))
    }

    #[must_use]
    fn render_lines(&self, width: usize, theme: &TuiTheme) -> Vec<String> {
        self.kind
            .picker_lines(width, theme)
            .or_else(|| self.kind.rich_dialog_lines(width))
            .or_else(|| self.kind.message_lines())
            .unwrap_or_default()
    }

    #[must_use]
    fn height(&self) -> u16 {
        self.kind
            .compact_height()
            .or_else(|| self.kind.input_dialog_height())
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum OverlayKind {
    CommandPalette(CommandPaletteState),
    SessionPicker(SessionPickerState),
    ModelPicker(ModelPickerState),
    PromptCompletion(PromptCompletionState),
    Approval(ApprovalRequestModal),
    QuestionDialog(QuestionStateMachine),
    Message(String),
    // Neo rich dialogs
    ModelSelector(crate::dialogs::ModelSelectorState),
    TabbedModelSelector(crate::dialogs::TabbedModelSelectorState),
    ProviderManager(crate::dialogs::ProviderManagerState),
    ChoicePicker(crate::dialogs::ChoicePickerState),
    ApiKeyInput(crate::dialogs::ApiKeyInputState),
    CustomRegistryImport(crate::dialogs::CustomRegistryImportState),
}

impl OverlayKind {
    fn list_selection_mut(&mut self) -> Option<OverlayListSelection<'_>> {
        match self {
            Self::CommandPalette(state) => Some(OverlayListSelection::CommandPalette(state)),
            Self::SessionPicker(state) => Some(OverlayListSelection::SessionPicker(state)),
            _ => self.secondary_list_selection_mut(),
        }
    }

    fn secondary_list_selection_mut(&mut self) -> Option<OverlayListSelection<'_>> {
        match self {
            Self::ModelPicker(state) => Some(OverlayListSelection::ModelPicker(state)),
            Self::PromptCompletion(state) => Some(OverlayListSelection::PromptCompletion(state)),
            _ => None,
        }
    }

    #[must_use]
    fn session_picker_lines(&self, width: usize, theme: &TuiTheme) -> Option<Vec<String>> {
        let Self::SessionPicker(picker) = self else {
            return None;
        };
        Some(picker.render_lines(width, theme))
    }

    #[must_use]
    fn picker_lines(&self, width: usize, theme: &TuiTheme) -> Option<Vec<String>> {
        match self {
            Self::CommandPalette(palette) => Some(palette.render_lines(width)),
            Self::SessionPicker(_) => self.session_picker_lines(width, theme),
            Self::ModelPicker(picker) => Some(picker.render_lines(width)),
            Self::PromptCompletion(completions) => Some(completions.render_lines(width)),
            _ => None,
        }
    }

    #[must_use]
    fn rich_dialog_lines(&self, width: usize) -> Option<Vec<String>> {
        match self {
            Self::ModelSelector(state) => Some(state.render_lines(width)),
            Self::TabbedModelSelector(state) => Some(state.render_lines(width)),
            Self::ProviderManager(state) => Some(state.render_lines(width)),
            _ => self.input_dialog_lines(width),
        }
    }

    #[must_use]
    fn input_dialog_lines(&self, width: usize) -> Option<Vec<String>> {
        match self {
            Self::ChoicePicker(state) => Some(state.render_lines(width)),
            Self::ApiKeyInput(state) => Some(state.render_lines(width)),
            Self::CustomRegistryImport(state) => Some(state.render_lines(width)),
            _ => None,
        }
    }

    #[must_use]
    fn message_lines(&self) -> Option<Vec<String>> {
        let Self::Message(text) = self else {
            return None;
        };
        Some(vec![text.clone()])
    }

    #[must_use]
    fn compact_height(&self) -> Option<u16> {
        match self {
            Self::CommandPalette(_) => Some(12),
            Self::PromptCompletion(_) | Self::Approval(_) => Some(8),
            Self::Message(_) => Some(3),
            _ => None,
        }
    }

    #[must_use]
    fn input_dialog_height(&self) -> Option<u16> {
        match self {
            Self::ApiKeyInput(_) | Self::CustomRegistryImport(_) => Some(10),
            Self::SessionPicker(_)
            | Self::ModelPicker(_)
            | Self::QuestionDialog(_)
            | Self::ModelSelector(_)
            | Self::TabbedModelSelector(_)
            | Self::ProviderManager(_)
            | Self::ChoicePicker(_) => Some(16),
            _ => None,
        }
    }
}

enum OverlayListSelection<'a> {
    CommandPalette(&'a mut CommandPaletteState),
    SessionPicker(&'a mut SessionPickerState),
    ModelPicker(&'a mut ModelPickerState),
    PromptCompletion(&'a mut PromptCompletionState),
}

impl OverlayListSelection<'_> {
    fn select_up(self) {
        match self {
            Self::CommandPalette(state) => state.move_up(),
            Self::SessionPicker(state) => state.move_up(),
            Self::ModelPicker(state) => state.move_up(),
            Self::PromptCompletion(state) => state.move_up(),
        }
    }

    fn select_down(self) {
        match self {
            Self::CommandPalette(state) => state.move_down(),
            Self::SessionPicker(state) => state.move_down(),
            Self::ModelPicker(state) => state.move_down(),
            Self::PromptCompletion(state) => state.move_down(),
        }
    }

    fn select_page_up(self) {
        match self {
            Self::CommandPalette(state) => state.page_up(),
            Self::SessionPicker(state) => state.page_up(),
            Self::ModelPicker(state) => state.page_up(),
            Self::PromptCompletion(state) => state.page_up(),
        }
    }

    fn select_page_down(self) {
        match self {
            Self::CommandPalette(state) => state.page_down(),
            Self::SessionPicker(state) => state.page_down(),
            Self::ModelPicker(state) => state.page_down(),
            Self::PromptCompletion(state) => state.page_down(),
        }
    }
}

fn handle_dialog_selection(kind: &mut OverlayKind, action: KeybindingAction) {
    let input = InputEvent::Action(action);
    if handle_selector_dialog_selection(kind, &input) {
        return;
    }
    handle_input_dialog_selection(kind, input);
}

fn handle_selector_dialog_selection(kind: &mut OverlayKind, input: &InputEvent) -> bool {
    if handle_model_dialog_selection(kind, input) {
        return true;
    }
    handle_provider_choice_dialog_selection(kind, input)
}

fn handle_model_dialog_selection(kind: &mut OverlayKind, input: &InputEvent) -> bool {
    match kind {
        OverlayKind::ModelSelector(state) => handle_input_ref(state, input),
        OverlayKind::TabbedModelSelector(state) => handle_input_ref(state, input),
        _ => return false,
    }
    true
}

fn handle_provider_choice_dialog_selection(kind: &mut OverlayKind, input: &InputEvent) -> bool {
    match kind {
        OverlayKind::ProviderManager(state) => handle_input_ref(state, input),
        OverlayKind::ChoicePicker(state) => handle_input_ref(state, input),
        _ => return false,
    }
    true
}

fn handle_input_dialog_selection(kind: &mut OverlayKind, input: InputEvent) {
    match kind {
        OverlayKind::ApiKeyInput(state) => handle_input_ref(state, &input),
        OverlayKind::CustomRegistryImport(state) => handle_input_owned(state, input),
        _ => {}
    }
}

fn handle_input_ref<T: DialogInputRef>(state: &mut T, input: &InputEvent) {
    state.handle_dialog_input(input);
}

fn handle_input_owned<T: DialogInputOwned>(state: &mut T, input: InputEvent) {
    state.handle_dialog_input(input);
}

trait DialogInputRef {
    fn handle_dialog_input(&mut self, input: &InputEvent);
}

trait DialogInputOwned {
    fn handle_dialog_input(&mut self, input: InputEvent);
}

impl DialogInputRef for ModelSelectorState {
    fn handle_dialog_input(&mut self, input: &InputEvent) {
        let _ = self.handle_input(input);
    }
}

impl DialogInputRef for TabbedModelSelectorState {
    fn handle_dialog_input(&mut self, input: &InputEvent) {
        let _ = self.handle_input(input);
    }
}

impl DialogInputRef for ProviderManagerState {
    fn handle_dialog_input(&mut self, input: &InputEvent) {
        let _ = self.handle_input(input);
    }
}

impl DialogInputRef for ChoicePickerState {
    fn handle_dialog_input(&mut self, input: &InputEvent) {
        let _ = self.handle_input(input);
    }
}

impl DialogInputRef for ApiKeyInputState {
    fn handle_dialog_input(&mut self, input: &InputEvent) {
        let _ = self.handle_input(input);
    }
}

impl DialogInputOwned for CustomRegistryImportState {
    fn handle_dialog_input(&mut self, input: InputEvent) {
        let _ = self.handle_input(input);
    }
}

pub type ModelPickerState = PickerState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPickerScope {
    Workspace,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPickerItem {
    pub id: String,
    pub title: String,
    pub last_prompt: Option<String>,
    pub work_dir: PathBuf,
    pub updated_at: SystemTime,
    pub is_current: bool,
}

impl SessionPickerItem {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        last_prompt: Option<String>,
        work_dir: impl Into<PathBuf>,
        updated_at: SystemTime,
        is_current: bool,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            last_prompt,
            work_dir: work_dir.into(),
            updated_at,
            is_current,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPickerState {
    items: Vec<SessionPickerItem>,
    current_session_id: String,
    scope: SessionPickerScope,
    filter: String,
    /// Selected index into the filtered list.
    selected: usize,
    max_visible: usize,
}

impl SessionPickerState {
    #[must_use]
    pub fn new(
        items: impl IntoIterator<Item = SessionPickerItem>,
        current_session_id: impl Into<String>,
        scope: SessionPickerScope,
        max_visible: usize,
    ) -> Self {
        Self {
            items: items.into_iter().collect(),
            current_session_id: current_session_id.into(),
            scope,
            filter: String::new(),
            selected: 0,
            max_visible: max_visible.max(1),
        }
    }

    fn filtered_items(&self) -> Vec<&SessionPickerItem> {
        if self.filter.is_empty() {
            self.items.iter().collect()
        } else {
            let q = self.filter.to_lowercase();
            self.items
                .iter()
                .filter(|item| {
                    item.title.to_lowercase().contains(&q)
                        || item.id.to_lowercase().contains(&q)
                        || item
                            .last_prompt
                            .as_deref()
                            .is_some_and(|p| p.to_lowercase().contains(&q))
                })
                .collect()
        }
    }

    pub fn set_filter(&mut self, filter: &str) {
        filter.clone_into(&mut self.filter);
        self.selected = 0;
    }

    /// Clear the filter. Returns `true` if there was a filter to clear
    /// (for the Esc two-stage behaviour).
    pub fn clear_filter(&mut self) -> bool {
        let had = !self.filter.is_empty();
        self.filter.clear();
        self.selected = 0;
        had
    }

    pub fn move_up(&mut self) {
        let len = self.filtered_items().len();
        if len > 0 {
            self.selected = (self.selected + len - 1) % len;
        }
    }

    pub fn move_down(&mut self) {
        let len = self.filtered_items().len();
        if len > 0 {
            self.selected = (self.selected + 1) % len;
        }
    }

    pub fn page_up(&mut self) {
        let len = self.filtered_items().len();
        if len > 0 {
            self.selected = self.selected.saturating_sub(self.max_visible);
        }
    }

    pub fn page_down(&mut self) {
        let len = self.filtered_items().len();
        if len > 0 {
            self.selected = (self.selected + self.max_visible).min(len - 1);
        }
    }

    pub fn set_scope(&mut self, scope: SessionPickerScope) {
        self.scope = scope;
        self.selected = 0;
        self.filter.clear();
    }

    #[must_use]
    pub const fn scope(&self) -> SessionPickerScope {
        self.scope
    }

    #[must_use]
    pub fn selected_item(&self) -> Option<SessionPickerItem> {
        self.filtered_items()
            .get(self.selected)
            .map(|item| (*item).clone())
    }

    #[must_use]
    pub fn confirm(&self) -> Option<SessionPickerItem> {
        self.selected_item()
    }

    /// Render the picker as ANSI-styled lines matching the Neo card layout.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn render_lines(&self, width: usize, theme: &TuiTheme) -> Vec<String> {
        let brand = theme.brand;
        let text_muted = theme.text_muted;
        let status_ok = theme.status_ok;
        let text_color = theme.text_primary;
        let border =
            crate::ansi::paint(&"─".repeat(width), crate::ansi::Style::default().fg(brand)).clone();

        let mut lines = vec![border.clone()];

        let title = match self.scope {
            SessionPickerScope::Workspace => "Sessions",
            SessionPickerScope::All => "All sessions",
        };
        let title_suffix = if self.filter.is_empty() {
            format!(
                "  {}",
                crate::ansi::paint(
                    "(type to search)",
                    crate::ansi::Style::default().fg(text_muted)
                )
            )
        } else {
            String::new()
        };
        lines.push(format!(
            "{}{}",
            crate::ansi::paint(title, crate::ansi::Style::default().fg(brand).bold()),
            title_suffix
        ));

        // Hint line
        let scope_hint = match self.scope {
            SessionPickerScope::Workspace => "Ctrl+A all",
            SessionPickerScope::All => "Ctrl+A current cwd",
        };
        let hint_parts: Vec<&str> = if self.filter.is_empty() {
            vec![
                "\u{2191}\u{2193} navigate",
                scope_hint,
                "Enter select",
                "Esc cancel",
            ]
        } else {
            vec![
                "Backspace clear",
                "\u{2191}\u{2193} navigate",
                scope_hint,
                "Enter select",
                "Esc cancel",
            ]
        };
        lines.push(crate::ansi::paint(
            &hint_parts.join(" \u{00b7} "),
            crate::ansi::Style::default().fg(text_muted),
        ));

        lines.push(String::new());

        if !self.filter.is_empty() {
            lines.push(format!(
                "{}{}",
                crate::ansi::paint("Search: ", crate::ansi::Style::default().fg(brand)),
                crate::ansi::paint(&self.filter, crate::ansi::Style::default().fg(text_color))
            ));
        }

        let filtered = self.filtered_items();
        if filtered.is_empty() {
            let msg = if self.items.is_empty() {
                "No sessions found."
            } else {
                "No matches"
            };
            lines.push(crate::ansi::paint(
                msg,
                crate::ansi::Style::default().fg(text_muted),
            ));
            lines.push(border);
            return lines;
        }

        let visible_start = (self.selected / self.max_visible) * self.max_visible;
        let visible_end = (visible_start + self.max_visible).min(filtered.len());
        for (vi, item) in filtered
            .iter()
            .enumerate()
            .take(visible_end)
            .skip(visible_start)
        {
            let is_selected = vi == self.selected;
            for card_line in Self::render_card(
                item,
                is_selected,
                width,
                brand,
                text_muted,
                status_ok,
                text_color,
            ) {
                lines.push(card_line);
            }
            if vi < visible_end - 1 {
                lines.push(String::new());
            }
        }

        // Footer
        if filtered.len() > self.max_visible || !self.filter.is_empty() {
            lines.push(String::new());
            let total_suffix = if self.filter.is_empty() {
                format!("{} sessions", filtered.len())
            } else {
                format!("{} matches", filtered.len())
            };
            let footer = format!(
                "Showing {}-{} of {}",
                visible_start + 1,
                visible_end,
                total_suffix
            );
            lines.push(crate::ansi::paint(
                &footer,
                crate::ansi::Style::default().fg(text_muted),
            ));
        }

        lines.push(border);
        lines
    }

    #[allow(clippy::too_many_arguments)]
    fn render_card(
        item: &SessionPickerItem,
        is_selected: bool,
        width: usize,
        brand: Color,
        text_muted: Color,
        status_ok: Color,
        text_color: Color,
    ) -> Vec<String> {
        let pointer = if is_selected { "\u{276f} " } else { "  " };
        let pointer_style = if is_selected {
            crate::ansi::Style::default().fg(brand)
        } else {
            crate::ansi::Style::default().fg(text_muted)
        };

        // Relative time
        let time_str = format_relative_time(item.updated_at);

        // Current badge
        let badge = if item.is_current {
            " \u{2190} current"
        } else {
            ""
        };

        // Title with inline trailing
        let title_text = if item.title.is_empty() {
            &item.id
        } else {
            &item.title
        };
        let title_style = if is_selected {
            crate::ansi::Style::default().fg(brand).bold()
        } else {
            crate::ansi::Style::default().fg(text_color)
        };

        let mut header = crate::ansi::paint(pointer, pointer_style);
        header.push_str(&crate::ansi::paint(&single_line(title_text), title_style));
        if !time_str.is_empty() {
            header.push_str("  ");
            header.push_str(&crate::ansi::paint(
                &time_str,
                crate::ansi::Style::default().fg(text_muted),
            ));
        }
        if !badge.is_empty() {
            header.push_str("  ");
            header.push_str(&crate::ansi::paint(
                badge,
                crate::ansi::Style::default().fg(status_ok),
            ));
        }

        // Truncate header to width
        let mut card = vec![truncate_ansi_to_width(&header, width)];

        // Meta line: session id + work_dir
        let id_str = &item.id;
        let dir_str = home_alias(&item.work_dir);
        let indent = "  ";
        let meta_gap = "   ";
        let meta_line = format!(
            "{}{}{}{}",
            indent,
            crate::ansi::paint(id_str, crate::ansi::Style::default().fg(text_muted)),
            crate::ansi::paint(meta_gap, crate::ansi::Style::default().fg(text_muted)),
            crate::ansi::paint(&dir_str, crate::ansi::Style::default().fg(text_muted))
        );
        let meta_visible = crate::ansi::strip_ansi(&meta_line).chars().count();
        if meta_visible <= width {
            card.push(meta_line);
        } else {
            // Wrap: id on one line, dir on next
            card.push(format!(
                "{}{}",
                indent,
                crate::ansi::paint(id_str, crate::ansi::Style::default().fg(text_muted))
            ));
            let dir_budget = width.saturating_sub(indent.len());
            let truncated_dir = truncate_left(&dir_str, dir_budget);
            card.push(format!(
                "{}{}",
                indent,
                crate::ansi::paint(&truncated_dir, crate::ansi::Style::default().fg(text_muted))
            ));
        }

        // Last prompt preview
        if let Some(prompt) = &item.last_prompt {
            let trimmed = single_line(prompt);
            if !trimmed.is_empty() {
                let marker = "\u{203a} ";
                let budget = width.saturating_sub(indent.len() + marker.len());
                let truncated = truncate_to_chars(&trimmed, budget);
                card.push(format!(
                    "{}{}{}",
                    indent,
                    crate::ansi::paint(marker, crate::ansi::Style::default().fg(text_muted)),
                    crate::ansi::paint(&truncated, crate::ansi::Style::default().fg(text_muted))
                ));
            }
        }

        card
    }
}

fn format_relative_time(time: SystemTime) -> String {
    let now = SystemTime::now();
    let diff = now.duration_since(time).unwrap_or_default();
    let secs = diff.as_secs();
    if secs < 60 {
        "just now".to_owned()
    } else {
        let mins = secs / 60;
        if mins < 60 {
            format!("{mins}m ago")
        } else {
            let hours = mins / 60;
            if hours < 24 {
                format!("{hours}h ago")
            } else {
                let days = hours / 24;
                format!("{days}d ago")
            }
        }
    }
}

fn single_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}

fn home_alias(path: &Path) -> String {
    if let Ok(home) = std::env::var("HOME") {
        let home = PathBuf::from(&home);
        if let Ok(rel) = path.strip_prefix(&home) {
            return format!("~/{}", rel.display());
        }
    }
    path.display().to_string()
}

fn truncate_left(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_width {
        return s.to_owned();
    }
    if max_width == 1 {
        return "\u{2026}".to_owned();
    }
    let keep = max_width - 1;
    let start = chars.len() - keep;
    format!("\u{2026}{}", chars[start..].iter().collect::<String>())
}

fn truncate_to_chars(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        return s.to_owned();
    }
    if max_chars <= 1 {
        return "\u{2026}".to_owned();
    }
    format!(
        "{}\u{2026}",
        chars[..max_chars - 1].iter().collect::<String>()
    )
}

fn truncate_ansi_to_width(s: &str, width: usize) -> String {
    let visible = crate::ansi::strip_ansi(s);
    if visible.chars().count() <= width {
        return s.to_owned();
    }
    // Simple truncation: just cut at width characters of visible text
    let truncated: String = visible.chars().take(width).collect();
    truncated
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCompletionState {
    prefix: PromptCompletionPrefix,
    picker: PickerState,
}

impl PromptCompletionState {
    #[must_use]
    pub fn new(
        prefix: PromptCompletionPrefix,
        items: impl IntoIterator<Item = PickerItem>,
    ) -> Self {
        Self {
            prefix,
            picker: PickerState::new(items),
        }
    }

    #[must_use]
    pub const fn prefix(&self) -> &PromptCompletionPrefix {
        &self.prefix
    }

    pub fn move_up(&mut self) {
        self.picker.move_up();
    }

    pub fn move_down(&mut self) {
        self.picker.move_down();
    }

    pub fn page_up(&mut self) {
        self.picker.page_up();
    }

    pub fn page_down(&mut self) {
        self.picker.page_down();
    }

    #[must_use]
    pub fn selected_item(&self) -> Option<PickerItem> {
        self.picker.selected_item()
    }

    #[must_use]
    pub fn confirm(&self) -> Option<PickerItem> {
        self.picker.confirm()
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        self.picker.render_lines(width)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

impl PickerItem {
    #[must_use]
    pub fn new(
        value: impl Into<String>,
        label: impl Into<String>,
        description: Option<impl Into<String>>,
    ) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: description.map(Into::into),
        }
    }
}

impl From<PickerItem> for SelectItem {
    fn from(item: PickerItem) -> Self {
        Self::new(item.value, item.label, item.description)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerState {
    list: SelectListState,
}

impl PickerState {
    #[must_use]
    pub fn new(items: impl IntoIterator<Item = PickerItem>) -> Self {
        Self::new_with_visible(items, 8)
    }

    #[must_use]
    pub fn new_with_visible(
        items: impl IntoIterator<Item = PickerItem>,
        max_visible: usize,
    ) -> Self {
        Self {
            list: SelectListState::new(items.into_iter().map(SelectItem::from), max_visible),
        }
    }

    pub fn set_filter(&mut self, filter: &str) {
        self.list.set_filter(filter);
    }

    pub fn move_up(&mut self) {
        self.list.move_up();
    }

    pub fn move_down(&mut self) {
        self.list.move_down();
    }

    pub fn page_up(&mut self) {
        self.list.page_up();
    }

    pub fn page_down(&mut self) {
        self.list.page_down();
    }

    #[must_use]
    pub const fn list(&self) -> &SelectListState {
        &self.list
    }

    #[must_use]
    pub fn selected_item(&self) -> Option<PickerItem> {
        self.list.selected_item().map(picker_from_select_item)
    }

    #[must_use]
    pub fn selected_model(&self) -> Option<PickerItem> {
        self.selected_item()
    }

    #[must_use]
    pub fn confirm(&self) -> Option<PickerItem> {
        self.selected_item()
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        self.list.render_lines(width)
    }
}

fn picker_from_select_item(item: &SelectItem) -> PickerItem {
    PickerItem {
        value: item.value.clone(),
        label: item.label.clone(),
        description: item.description.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

impl CommandSpec {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        description: Option<impl Into<String>>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: description.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandPaletteState {
    list: SelectListState,
}

impl CommandPaletteState {
    #[must_use]
    pub fn new(commands: impl IntoIterator<Item = CommandSpec>) -> Self {
        Self {
            list: SelectListState::new(commands.into_iter().map(select_from_command), 8),
        }
    }

    pub fn set_filter(&mut self, filter: &str) {
        self.list.set_filter(filter);
    }

    pub fn move_up(&mut self) {
        self.list.move_up();
    }

    pub fn move_down(&mut self) {
        self.list.move_down();
    }

    pub fn page_up(&mut self) {
        self.list.page_up();
    }

    pub fn page_down(&mut self) {
        self.list.page_down();
    }

    #[must_use]
    pub const fn list(&self) -> &SelectListState {
        &self.list
    }

    #[must_use]
    pub fn selected_command(&self) -> Option<CommandSpec> {
        self.list.selected_item().map(command_from_select_item)
    }

    #[must_use]
    pub fn confirm(&self) -> Option<CommandSpec> {
        self.selected_command()
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        self.list.render_lines(width)
    }
}

fn select_from_command(command: CommandSpec) -> SelectItem {
    SelectItem::new(command.id, command.label, command.description)
}

fn command_from_select_item(item: &SelectItem) -> CommandSpec {
    CommandSpec {
        id: item.value.clone(),
        label: item.label.clone(),
        description: item.description.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequestModal {
    pub request_id: String,
    pub modal: ApprovalModal,
    pub feedback_input: String,
}

impl ApprovalRequestModal {
    #[must_use]
    pub fn new(
        request_id: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            feedback_input: String::new(),
            modal: ApprovalModal::new(
                title,
                body,
                [
                    ApprovalOption::new(ApprovalChoice::Approve, "Approve once"),
                    ApprovalOption::new(ApprovalChoice::AlwaysApprove, "Approve for this session"),
                    ApprovalOption::new(ApprovalChoice::Deny, "Reject"),
                    ApprovalOption::new(ApprovalChoice::Revise, "Reject with feedback"),
                ],
            ),
        }
    }

    /// Create a review approval modal with Approve / Reject / Revise options.
    #[must_use]
    pub fn new_review(
        request_id: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            feedback_input: String::new(),
            modal: ApprovalModal::new(
                title,
                body,
                [
                    ApprovalOption::new(ApprovalChoice::Approve, "Approve"),
                    ApprovalOption::new(ApprovalChoice::Deny, "Reject"),
                    ApprovalOption::new(ApprovalChoice::Revise, "Revise"),
                ],
            ),
        }
    }

    pub fn move_up(&mut self) {
        if self.modal.options.is_empty() {
            self.modal.selected = 0;
        } else if self.modal.selected == 0 {
            self.modal.selected = self.modal.options.len() - 1;
        } else {
            self.modal.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.modal.options.is_empty() {
            self.modal.selected = 0;
        } else {
            self.modal.selected = (self.modal.selected + 1) % self.modal.options.len();
        }
    }

    #[must_use]
    pub fn is_collecting_feedback(&self) -> bool {
        self.modal.selected_choice() == Some(ApprovalChoice::Revise)
    }

    pub fn insert_feedback(&mut self, text: &str) {
        if self.is_collecting_feedback() {
            self.feedback_input.push_str(text);
        }
    }

    pub fn backspace_feedback(&mut self) {
        if self.is_collecting_feedback() {
            self.feedback_input.pop();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalResult {
    pub request_id: String,
    pub choice: ApprovalChoice,
    /// Feedback text when the user picks Revise (`ExitPlanMode` plan review).
    pub feedback: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InlineImageRenderCache {
    rendered: BTreeMap<String, String>,
}

impl InlineImageRenderCache {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rendered.is_empty()
    }

    pub fn reset_for_full_redraw(&mut self) {
        self.rendered.clear();
    }

    pub fn take_pending(
        &mut self,
        renders: impl IntoIterator<Item = crate::transcript::InlineImageRender>,
    ) -> Vec<crate::transcript::InlineImageRender> {
        let mut pending = Vec::new();
        for render in renders {
            if self.rendered.get(&render.id) == Some(&render.escape_sequence) {
                continue;
            }
            self.rendered
                .insert(render.id.clone(), render.escape_sequence.clone());
            pending.push(render);
        }
        pending
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum ToolStatusKind {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl ToolStatusKind {
    #[must_use]
    pub fn label(self) -> &'static str {
        ["pending", "running", "succeeded", "failed", "cancelled"][self as usize]
    }

    #[must_use]
    pub fn marker(self) -> &'static str {
        ["-", "*", "+", "!", "x"][self as usize]
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptState {
    pub text: String,
    pub cursor: usize,
    history: Vec<String>,
    history_index: Option<usize>,
    history_draft: Option<PromptSnapshot>,
    undo_stack: Vec<PromptSnapshot>,
    kill_ring: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PromptSnapshot {
    text: String,
    cursor: usize,
}

impl PromptState {
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.chars().count();
        Self {
            text,
            cursor,
            history: Vec::new(),
            history_index: None,
            history_draft: None,
            undo_stack: Vec::new(),
            kill_ring: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_cursor(mut self, cursor: usize) -> Self {
        self.cursor = cursor.min(self.text.chars().count());
        self
    }

    pub fn remember_history(&mut self, entry: impl Into<String>) {
        let entry = entry.into();
        if entry.trim().is_empty() {
            return;
        }
        self.history.push(entry);
        self.stop_history_navigation();
    }

    pub fn clear_after_submit(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.undo_stack.clear();
        self.kill_ring.clear();
        self.stop_history_navigation();
    }

    pub fn recall_previous_history(&mut self) -> bool {
        if self.history.is_empty() {
            return false;
        }
        let index = if let Some(index) = self.history_index {
            index.saturating_sub(1)
        } else {
            self.history_draft = Some(self.snapshot());
            self.history.len() - 1
        };
        self.history_index = Some(index);
        self.replace_with_history_text(index);
        true
    }

    pub fn recall_next_history(&mut self) -> bool {
        let Some(index) = self.history_index else {
            return false;
        };
        let next = index + 1;
        if next < self.history.len() {
            self.history_index = Some(next);
            self.replace_with_history_text(next);
        } else {
            if let Some(snapshot) = self.history_draft.take() {
                self.text = snapshot.text;
                self.cursor = snapshot.cursor.min(self.char_len());
            } else {
                self.text.clear();
                self.cursor = 0;
            }
            self.history_index = None;
            self.undo_stack.clear();
        }
        true
    }

    pub fn apply_edit(&mut self, edit: PromptEdit<'_>) -> Option<String> {
        self.cursor = self.cursor.min(self.char_len());

        match edit {
            PromptEdit::Insert(text) => {
                let inserted = text.to_string();
                if inserted.is_empty() {
                    return None;
                }
                self.stop_history_navigation();
                let before = self.snapshot();
                let index = self.byte_index(self.cursor);
                self.text.insert_str(index, &inserted);
                self.cursor += inserted.chars().count();
                self.push_undo(before);
                Some(inserted)
            }
            PromptEdit::Clear => {
                if self.text.is_empty() {
                    return None;
                }
                self.stop_history_navigation();
                let before = self.snapshot();
                let cleared = std::mem::take(&mut self.text);
                self.cursor = 0;
                self.push_undo(before);
                Some(cleared)
            }
            PromptEdit::Backspace => self.apply_delete(
                self.cursor.saturating_sub(1),
                self.cursor,
                DeleteDirection::Backward,
                false,
            ),
            PromptEdit::Delete => self.apply_delete(
                self.cursor,
                self.cursor + 1,
                DeleteDirection::Forward,
                false,
            ),
            PromptEdit::MoveLeft => {
                self.cursor = self.cursor.saturating_sub(1);
                None
            }
            PromptEdit::MoveRight => {
                self.cursor = (self.cursor + 1).min(self.char_len());
                None
            }
            PromptEdit::MoveHome => {
                self.cursor = 0;
                None
            }
            PromptEdit::MoveEnd => {
                self.cursor = self.char_len();
                None
            }
            PromptEdit::MoveWordLeft => {
                self.cursor = find_word_backward(&self.text, self.cursor);
                None
            }
            PromptEdit::MoveWordRight => {
                self.cursor = find_word_forward(&self.text, self.cursor);
                None
            }
            PromptEdit::DeleteWordBackward => {
                let start = find_word_backward(&self.text, self.cursor);
                self.apply_delete(start, self.cursor, DeleteDirection::Backward, true)
            }
            PromptEdit::DeleteWordForward => {
                let end = find_word_forward(&self.text, self.cursor);
                self.apply_delete(self.cursor, end, DeleteDirection::Forward, true)
            }
            PromptEdit::DeleteToLineStart => {
                self.apply_delete(0, self.cursor, DeleteDirection::Backward, true)
            }
            PromptEdit::DeleteToLineEnd => {
                self.apply_delete(self.cursor, self.char_len(), DeleteDirection::Forward, true)
            }
            PromptEdit::Yank => {
                let yanked = self.kill_ring.last().cloned()?;
                self.stop_history_navigation();
                let before = self.snapshot();
                let index = self.byte_index(self.cursor);
                self.text.insert_str(index, &yanked);
                self.cursor += yanked.chars().count();
                self.push_undo(before);
                Some(yanked)
            }
            PromptEdit::Undo => {
                self.stop_history_navigation();
                if let Some(snapshot) = self.undo_stack.pop() {
                    self.text = snapshot.text;
                    self.cursor = snapshot.cursor.min(self.char_len());
                }
                None
            }
        }
    }

    #[must_use]
    pub fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    #[must_use]
    pub fn copy_text(&self) -> Option<String> {
        (!self.text.is_empty()).then(|| self.text.clone())
    }

    #[must_use]
    pub fn completion_prefix(&self) -> Option<PromptCompletionPrefix> {
        let chars = self.text.chars().collect::<Vec<_>>();
        let cursor = self.cursor.min(chars.len());
        let mut start = cursor;
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        if start == cursor {
            return None;
        }
        Some(PromptCompletionPrefix {
            start,
            end: cursor,
            text: chars[start..cursor].iter().collect(),
        })
    }

    pub fn replace_completion_prefix(
        &mut self,
        prefix: &PromptCompletionPrefix,
        replacement: &str,
    ) -> Option<String> {
        if replacement.is_empty() {
            return None;
        }
        let len = self.char_len();
        if prefix.start > prefix.end || prefix.end > len {
            return None;
        }
        if self.slice_chars(prefix.start, prefix.end)? != prefix.text {
            return None;
        }

        self.stop_history_navigation();
        let before = self.snapshot();
        let start_byte = self.byte_index(prefix.start);
        let end_byte = self.byte_index(prefix.end);
        self.text.replace_range(start_byte..end_byte, replacement);
        self.cursor = prefix.start + replacement.chars().count();
        self.push_undo(before);
        Some(replacement.to_owned())
    }

    fn byte_index(&self, char_index: usize) -> usize {
        if char_index == 0 {
            return 0;
        }

        self.text
            .char_indices()
            .nth(char_index)
            .map_or(self.text.len(), |(index, _)| index)
    }

    fn slice_chars(&self, start: usize, end: usize) -> Option<String> {
        if start > end || end > self.char_len() {
            return None;
        }
        let start_byte = self.byte_index(start);
        let end_byte = self.byte_index(end);
        Some(self.text[start_byte..end_byte].to_owned())
    }

    fn snapshot(&self) -> PromptSnapshot {
        PromptSnapshot {
            text: self.text.clone(),
            cursor: self.cursor,
        }
    }

    fn push_undo(&mut self, snapshot: PromptSnapshot) {
        self.undo_stack.push(snapshot);
    }

    fn replace_with_history_text(&mut self, index: usize) {
        self.text = self.history[index].clone();
        self.cursor = self.char_len();
        self.undo_stack.clear();
    }

    fn stop_history_navigation(&mut self) {
        self.history_index = None;
        self.history_draft = None;
    }

    fn apply_delete(
        &mut self,
        start: usize,
        end: usize,
        direction: DeleteDirection,
        record_kill: bool,
    ) -> Option<String> {
        self.stop_history_navigation();
        let before = self.snapshot();
        let deleted = self.delete_range(start, end, direction)?;
        self.push_undo(before);
        if record_kill {
            self.kill_ring.push(deleted.clone());
        }
        Some(deleted)
    }

    fn delete_range(
        &mut self,
        start: usize,
        end: usize,
        direction: DeleteDirection,
    ) -> Option<String> {
        let len = self.char_len();
        let start = start.min(len);
        let end = end.min(len);
        if start >= end {
            return None;
        }

        let start_byte = self.byte_index(start);
        let end_byte = self.byte_index(end);
        let deleted = self.text[start_byte..end_byte].to_string();
        self.text.replace_range(start_byte..end_byte, "");

        match direction {
            DeleteDirection::Backward => self.cursor = start,
            DeleteDirection::Forward => self.cursor = self.cursor.min(self.char_len()),
        }

        Some(deleted)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCompletionPrefix {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeleteDirection {
    Backward,
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptEdit<'a> {
    Insert(&'a str),
    Clear,
    Backspace,
    Delete,
    MoveLeft,
    MoveRight,
    MoveHome,
    MoveEnd,
    MoveWordLeft,
    MoveWordRight,
    DeleteWordBackward,
    DeleteWordForward,
    DeleteToLineStart,
    DeleteToLineEnd,
    Yank,
    Undo,
}

fn find_word_backward(text: &str, cursor: usize) -> usize {
    let chars = text.chars().collect::<Vec<_>>();
    let mut index = cursor.min(chars.len());

    while index > 0 && chars[index - 1].is_whitespace() {
        index -= 1;
    }

    if index == 0 {
        return 0;
    }

    let word_like = is_word_like(chars[index - 1]);
    while index > 0
        && is_word_like(chars[index - 1]) == word_like
        && !chars[index - 1].is_whitespace()
    {
        index -= 1;
    }

    index
}

fn find_word_forward(text: &str, cursor: usize) -> usize {
    let chars = text.chars().collect::<Vec<_>>();
    let mut index = cursor.min(chars.len());

    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }

    if index >= chars.len() {
        return index;
    }

    let word_like = is_word_like(chars[index]);
    while index < chars.len()
        && is_word_like(chars[index]) == word_like
        && !chars[index].is_whitespace()
    {
        index += 1;
    }

    index
}

fn is_word_like(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalChoice {
    Approve,
    Deny,
    AlwaysApprove,
    /// Revise — like Deny but the user provides feedback that gets sent to the model.
    /// Used for `ExitPlanMode` plan review.
    Revise,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalOption {
    pub choice: ApprovalChoice,
    pub label: String,
}

impl ApprovalOption {
    #[must_use]
    pub fn new(choice: ApprovalChoice, label: impl Into<String>) -> Self {
        Self {
            choice,
            label: label.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalModal {
    pub title: String,
    pub body: String,
    pub options: Vec<ApprovalOption>,
    pub selected: usize,
    pub theme: TuiTheme,
}

impl ApprovalModal {
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        body: impl Into<String>,
        options: impl IntoIterator<Item = ApprovalOption>,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            options: options.into_iter().collect(),
            selected: 0,
            theme: TuiTheme::default(),
        }
    }

    #[must_use]
    pub fn with_selected(mut self, selected: usize) -> Self {
        if !self.options.is_empty() {
            self.selected = selected.min(self.options.len() - 1);
        }
        self
    }

    #[must_use]
    pub const fn with_theme(mut self, theme: TuiTheme) -> Self {
        self.theme = theme;
        self
    }

    #[must_use]
    pub fn selected_choice(&self) -> Option<ApprovalChoice> {
        self.options.get(self.selected).map(|option| option.choice)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

impl SelectItem {
    #[must_use]
    pub fn new(
        value: impl Into<String>,
        label: impl Into<String>,
        description: Option<impl Into<String>>,
    ) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: description.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectListState {
    items: Vec<SelectItem>,
    filtered_indices: Vec<usize>,
    selected_index: usize,
    max_visible: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct VisibleSelectItem<'a> {
    pub item: &'a SelectItem,
    pub selected: bool,
}

impl SelectListState {
    #[must_use]
    pub fn new(items: impl IntoIterator<Item = SelectItem>, max_visible: usize) -> Self {
        let items = items.into_iter().collect::<Vec<_>>();
        let filtered_indices = (0..items.len()).collect();
        Self {
            items,
            filtered_indices,
            selected_index: 0,
            max_visible: max_visible.max(1),
        }
    }

    pub fn set_filter(&mut self, filter: &str) {
        let filter = filter.to_lowercase();
        self.filtered_indices = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| select_item_matches(item, &filter).then_some(index))
            .collect();
        self.selected_index = 0;
    }

    #[must_use]
    pub fn filtered_len(&self) -> usize {
        self.filtered_indices.len()
    }

    #[must_use]
    pub fn selected_item(&self) -> Option<&SelectItem> {
        self.filtered_indices
            .get(self.selected_index)
            .and_then(|index| self.items.get(*index))
    }

    pub fn move_up(&mut self) {
        let len = self.filtered_len();
        if len == 0 {
            self.selected_index = 0;
        } else if self.selected_index == 0 {
            self.selected_index = len - 1;
        } else {
            self.selected_index -= 1;
        }
    }

    pub fn move_down(&mut self) {
        let len = self.filtered_len();
        if len == 0 {
            self.selected_index = 0;
        } else {
            self.selected_index = (self.selected_index + 1) % len;
        }
    }

    pub fn page_up(&mut self) {
        if self.filtered_len() == 0 {
            self.selected_index = 0;
        } else {
            self.selected_index = self.selected_index.saturating_sub(self.max_visible);
        }
    }

    pub fn page_down(&mut self) {
        let len = self.filtered_len();
        if len == 0 {
            self.selected_index = 0;
        } else {
            self.selected_index = (self.selected_index + self.max_visible).min(len - 1);
        }
    }

    #[must_use]
    pub fn visible_range(&self) -> Range<usize> {
        let len = self.filtered_len();
        if len == 0 {
            return 0..0;
        }

        let visible = self.max_visible.min(len);
        let half = visible / 2;
        let max_start = len.saturating_sub(visible);
        let start = self.selected_index.saturating_sub(half).min(max_start);
        start..start + visible
    }

    #[must_use]
    pub fn visible_items(&self) -> Vec<VisibleSelectItem<'_>> {
        self.visible_range()
            .filter_map(|filtered_index| {
                self.filtered_indices
                    .get(filtered_index)
                    .and_then(|index| self.items.get(*index))
                    .map(|item| VisibleSelectItem {
                        item,
                        selected: filtered_index == self.selected_index,
                    })
            })
            .collect()
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        if self.filtered_indices.is_empty() {
            return vec![truncate_width("  No matching commands", width, "", false)];
        }

        let range = self.visible_range();
        let mut lines = Vec::new();
        for filtered_index in range.clone() {
            let Some(item) = self
                .filtered_indices
                .get(filtered_index)
                .and_then(|index| self.items.get(*index))
            else {
                continue;
            };
            lines.push(render_select_item(
                item,
                filtered_index == self.selected_index,
                width,
            ));
        }

        if range.start > 0 || range.end < self.filtered_len() {
            let info = format!("  ({}/{})", self.selected_index + 1, self.filtered_len());
            lines.push(truncate_width(&info, width, "", false));
        }

        lines
    }
}

fn render_select_item(item: &SelectItem, selected: bool, width: usize) -> String {
    let prefix = if selected { "> " } else { "  " };
    let label = if item.label.is_empty() {
        &item.value
    } else {
        &item.label
    };
    let prefix_width = visible_width(prefix);
    let description = item
        .description
        .as_deref()
        .map(|description| description.replace(['\r', '\n'], " ").trim().to_string())
        .filter(|description| !description.is_empty());

    if let Some(description) = description.filter(|_| width > 40) {
        let primary_width = 32usize.min(width.saturating_sub(prefix_width + 4)).max(1);
        let label = truncate_width(label, primary_width.saturating_sub(2).max(1), "", false);
        let spacing = " ".repeat(primary_width.saturating_sub(visible_width(&label)).max(1));
        let used = prefix_width + visible_width(&label) + spacing.len();
        let remaining = width.saturating_sub(used + 2);
        if remaining > 10 {
            let description = truncate_width(&description, remaining, "", false);
            return format!("{prefix}{label}{spacing}{description}");
        }
    }

    let max_label_width = width.saturating_sub(prefix_width + 2).max(1);
    format!(
        "{prefix}{}",
        truncate_width(label, max_label_width, "", false)
    )
}

fn select_item_matches(item: &SelectItem, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }

    item.value.to_lowercase().contains(filter)
        || item.label.to_lowercase().contains(filter)
        || item
            .description
            .as_deref()
            .is_some_and(|description| description.to_lowercase().contains(filter))
}
