mod approval;
mod command_palette;
mod context;
mod dialog_dispatch;
mod image_cache;
mod overlay;
mod pending_input;
mod pickers;
mod prompt;
mod select_list;
mod session_picker;
mod stream;

use crate::primitive::theme::{ChromeMode, DevelopmentMode, GoalModeStatus, TuiTheme};

pub use approval::{
    ApprovalChoice, ApprovalModal, ApprovalOption, ApprovalRequestModal, ApprovalResult,
};
pub use command_palette::{CommandPaletteState, CommandSpec};
pub use context::ContextWindow;
pub use image_cache::InlineImageRenderCache;
pub use overlay::{Overlay, OverlayId, OverlayKind};
pub use pending_input::PendingInputState;
pub use pickers::{
    ModelPickerState, PickerItem, PickerState, PromptCompletionPrefix, PromptCompletionState,
};
pub use prompt::{PromptEdit, PromptState};
pub use select_list::{SelectItem, SelectListState, VisibleSelectItem};
pub use session_picker::{SessionPickerItem, SessionPickerScope, SessionPickerState};
pub use stream::{StreamUpdate, ToolStatusKind};

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use neo_agent_core::{AgentEvent, PermissionMode, PermissionOperation};

use crate::dialogs::{
    QuestionDialogAction, QuestionDisplayData, QuestionDisplayOption, QuestionResult,
    QuestionStateMachine,
};
use crate::input::{InputEvent, KeybindingAction};
use crate::primitive::{Color, InputResult};
use crate::tasks_browser::TaskBrowserState;
use crate::terminal_image::{ImageRenderPolicy, TerminalImageCapabilities};
use crate::widgets::{TodoDisplayItem, TodoDisplayStatus};

/// Maximum number of visible content lines in the composer input box.
pub(crate) const MAX_PROMPT_VISIBLE_LINES: usize = 8;

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
    /// Optional `/btw` sidecar panel state.
    btw_panel_state: Option<crate::widgets::btw_panel::BtwPanelState>,
    /// Optional custom label shown in the footer as a working indicator.
    custom_working_label: Option<String>,
    /// Whether the current model has thinking enabled (shown in the footer).
    thinking_enabled: bool,
    /// Optional persistent exit-confirmation message shown in the footer.
    exit_confirmation_label: Option<String>,
    /// Formatted git branch/status badge shown after the workspace path.
    git_status_label: Option<String>,
    /// Pending steers and queued follow-ups waiting to be injected or sent.
    pending_input: PendingInputState,
    shell_mode_active: bool,
    shell_running: bool,
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
            btw_panel_state: None,
            custom_working_label: None,
            thinking_enabled: false,
            exit_confirmation_label: None,
            git_status_label: None,
            pending_input: PendingInputState::new(),
            shell_mode_active: false,
            shell_running: false,
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
        if self.shell_running {
            return Some("shell · esc to cancel".to_owned());
        }
        matches!(self.mode, ChromeMode::Streaming).then(|| "working · esc interrupt".to_owned())
    }

    #[must_use]
    pub const fn shell_mode_active(&self) -> bool {
        self.shell_mode_active
    }

    #[must_use]
    pub const fn shell_running(&self) -> bool {
        self.shell_running
    }

    pub const fn set_shell_running(&mut self, running: bool) {
        self.shell_running = running;
    }

    pub const fn enter_shell_mode(&mut self) {
        self.shell_mode_active = true;
    }

    pub const fn exit_shell_mode(&mut self) {
        self.shell_mode_active = false;
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
            PermissionMode::Ask => ("ask", self.theme().footer_permission_ask),
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
    pub fn btw_panel_state(&self) -> Option<&crate::widgets::btw_panel::BtwPanelState> {
        self.btw_panel_state.as_ref()
    }

    pub fn btw_panel_state_mut(&mut self) -> Option<&mut crate::widgets::btw_panel::BtwPanelState> {
        self.btw_panel_state.as_mut()
    }

    pub fn set_btw_panel_state(&mut self, state: Option<crate::widgets::btw_panel::BtwPanelState>) {
        self.btw_panel_state = state;
    }

    #[must_use]
    pub fn has_btw_panel(&self) -> bool {
        self.btw_panel_state.is_some()
    }

    pub fn close_btw_panel(&mut self) {
        self.btw_panel_state = None;
    }

    pub fn scroll_btw_panel_up(&mut self, rows: usize) {
        if let Some(state) = self.btw_panel_state.as_mut() {
            state.scroll_up(rows);
        }
    }

    pub fn scroll_btw_panel_down(&mut self, rows: usize) {
        if let Some(state) = self.btw_panel_state.as_mut() {
            state.scroll_down(rows);
        }
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
    pub const fn pending_input(&self) -> &PendingInputState {
        &self.pending_input
    }

    pub fn pending_input_mut(&mut self) -> &mut PendingInputState {
        &mut self.pending_input
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
                session_scope,
                prefix_rule,
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
                // Derive the dynamic option labels. Review transitions and
                // scope-less prompts omit both; prefix is offered only when the
                // runtime proposed a persistent rule.
                let mut session_label = if is_review {
                    None
                } else {
                    session_scope
                        .as_ref()
                        .filter(|scope| !scope.is_empty())
                        .map(|scope| scope.label.clone())
                };
                // Tool and shell approvals always offer a session-approval
                // option, even when no explicit session scope was derived.
                // Use the default label so the modal keeps its four-option
                // layout, matching the transcript pane.
                if session_label.is_none()
                    && matches!(
                        operation,
                        PermissionOperation::Tool | PermissionOperation::Shell
                    )
                {
                    session_label = Some("Approve for this session".to_owned());
                }
                let prefix_label = if is_review {
                    None
                } else {
                    prefix_rule
                        .as_ref()
                        .map(|rule| format!("Approve commands starting with {}", rule.label))
                };
                self.pending_approvals.push_back(
                    if operation == PermissionOperation::PlanTransition {
                        // ExitPlanMode carries `{plan_summary, options: [{label, description}]}`.
                        // Surface the model-supplied options as a real picker (mirrors
                        // kimi-code) instead of dumping the raw JSON into the body.
                        let (option_labels, options_body) = crate::primitive::theme::plan_review_options(&arguments);
                        let body = match arguments.get("plan_summary").and_then(|v| v.as_str()) {
                            Some(summary) if !summary.trim().is_empty() => {
                                if options_body.is_empty() {
                                    summary.to_owned()
                                } else {
                                    format!("{summary}\n\n{options_body}")
                                }
                            }
                            _ => options_body,
                        };
                        ApprovalRequestModal::new_plan_review(
                            id,
                            crate::primitive::theme::review_title(operation),
                            body,
                            option_labels,
                        )
                    } else if is_review {
                        ApprovalRequestModal::new_review(id, crate::primitive::theme::review_title(operation), body)
                    } else {
                        ApprovalRequestModal::new_with_options(
                            id,
                            format!("{operation:?} approval"),
                            body,
                            session_label,
                            prefix_label,
                        )
                    },
                );
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
            AgentEvent::SteeringQueued { message } => {
                self.pending_input.queue_steer(message.text());
            }
            AgentEvent::FollowUpQueued { message } => {
                self.pending_input.queue_follow_up(message.text());
            }
            AgentEvent::QueueDrained { kind, count } => {
                self.pending_input.drain(kind, count);
            }
            AgentEvent::CompactionStarted { .. }
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
            AgentEvent::PlanModeExited { .. } => {
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

    /// Search `self.overlays` for an existing overlay whose kind matches `predicate`.
    /// If found, returns its `OverlayId`; otherwise `None`.
    pub fn find_overlay_by_kind(
        &self,
        predicate: impl Fn(&OverlayKind) -> bool,
    ) -> Option<OverlayId> {
        self.overlays
            .iter()
            .find(|o| predicate(&o.kind))
            .map(|o| o.id)
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
        // Try to find existing provider manager overlay and update in place
        let existing_id =
            self.find_overlay_by_kind(|kind| matches!(kind, OverlayKind::ProviderManager(_)));
        if let Some(id) = existing_id {
            if let Some(overlay) = self.overlays.iter_mut().find(|o| o.id == id) {
                if let OverlayKind::ProviderManager(state) = &mut overlay.kind {
                    state.set_options(opts);
                }
            }
            self.focus_overlay(id);
            return id;
        }
        // No existing overlay — create new one
        let state = crate::dialogs::ProviderManagerState::new(opts);
        self.push_overlay(Overlay::new(
            "providers",
            OverlayKind::ProviderManager(state),
        ))
    }

    pub fn open_mcp_manager(&mut self, opts: &crate::dialogs::McpManagerOptions) -> OverlayId {
        // Try to find existing MCP manager overlay and update in place
        let existing_id =
            self.find_overlay_by_kind(|kind| matches!(kind, OverlayKind::McpManager(_)));
        if let Some(id) = existing_id {
            if let Some(overlay) = self.overlays.iter_mut().find(|o| o.id == id) {
                if let OverlayKind::McpManager(state) = &mut overlay.kind {
                    state.set_options(opts);
                }
            }
            self.focus_overlay(id);
            return id;
        }
        // No existing overlay — create new one
        let state = crate::dialogs::McpManagerState::new(opts);
        self.push_overlay(Overlay::new("mcp", OverlayKind::McpManager(state)))
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

    pub fn open_mcp_add_form(&mut self, opts: crate::dialogs::McpAddFormOptions) -> OverlayId {
        let state = crate::dialogs::McpAddFormState::new(opts, self.theme);
        self.push_overlay(Overlay::new("mcp-add", OverlayKind::McpAddForm(state)))
    }

    pub fn open_trust_dialog(&mut self, data: crate::dialogs::TrustDialogData) -> OverlayId {
        let state = crate::dialogs::TrustDialogState::new(data, self.theme);
        self.push_overlay(Overlay::new("trust", OverlayKind::TrustDialog(state)))
    }

    pub fn push_task_browser_overlay(&mut self, state: TaskBrowserState) -> OverlayId {
        if let Some(overlay) = self
            .overlays
            .iter_mut()
            .find(|overlay| matches!(overlay.kind, OverlayKind::TaskBrowser(_)))
        {
            overlay.kind = OverlayKind::TaskBrowser(state);
            let id = overlay.id;
            self.focus_overlay(id);
            return id;
        }
        self.push_overlay(Overlay::new("tasks", OverlayKind::TaskBrowser(state)))
    }

    #[must_use]
    pub fn take_trust_dialog_result(&mut self) -> Option<crate::dialogs::TrustDialogResult> {
        let id = self.focused_overlay?;
        let overlay = self.overlays.iter_mut().find(|overlay| overlay.id == id)?;
        let OverlayKind::TrustDialog(state) = &mut overlay.kind else {
            return None;
        };
        state.take_result()
    }

    /// Render the focused overlay (if any) into ANSI lines at the given width.
    #[must_use]
    pub fn focused_overlay_lines(&self, width: usize) -> Vec<String> {
        self.focused_overlay()
            .map_or_else(Vec::new, |overlay| overlay.render_lines(width, &self.theme))
    }

    #[must_use]
    pub fn render_focused_full_screen_overlay(
        &self,
        width: usize,
        height: usize,
    ) -> Option<Vec<String>> {
        self.focused_overlay()?
            .render_full_screen_lines(width, height, &self.theme)
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
                | OverlayKind::McpManager(_)
                | OverlayKind::McpAddForm(_)
                | OverlayKind::ChoicePicker(_)
                | OverlayKind::ApiKeyInput(_)
                | OverlayKind::TextInput(_)
                | OverlayKind::CustomRegistryImport(_)
                | OverlayKind::QuestionDialog(_)
                | OverlayKind::TrustDialog(_)
                | OverlayKind::TaskBrowser(_)
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
            OverlayKind::McpManager(state) => state.handle_input(&input),
            OverlayKind::ChoicePicker(state) => state.handle_input(&input),
            OverlayKind::ApiKeyInput(state) => state.handle_input(&input),
            OverlayKind::TextInput(state) => state.handle_input(&input),
            OverlayKind::CustomRegistryImport(state) => state.handle_input(input),
            OverlayKind::McpAddForm(state) => state.handle_input(input),
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
            OverlayKind::TrustDialog(state) => state.handle_input(&input),
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
            // The keybinding layer (see `keybinding_priority` /
            // `OVERLAY_ACTION_PRIORITY`) rewrites `enter`→`SelectConfirm` and
            // `escape`→`SelectCancel` before the dialog sees them. Normalize
            // those back to `Submit`/`Cancel` so text-input dialogs (API key,
            // custom registry import) that match on `Submit`/`Cancel` keep
            // working. Picker-style dialogs handle these actions directly too,
            // so they are unaffected.
            InputEvent::Action(KeybindingAction::SelectConfirm) => InputEvent::Submit,
            InputEvent::Action(KeybindingAction::SelectCancel) => InputEvent::Cancel,
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
    pub fn mcp_manager_action(&self) -> Option<crate::dialogs::McpManagerAction> {
        let OverlayKind::McpManager(state) = &self.focused_overlay()?.kind else {
            return None;
        };
        state.action()
    }

    pub fn take_mcp_manager_action(&mut self) -> Option<crate::dialogs::McpManagerAction> {
        let OverlayKind::McpManager(state) = &mut self.focused_overlay_mut()?.kind else {
            return None;
        };
        state.take_action()
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
    pub fn text_input_result(&self) -> Option<&crate::dialogs::TextInputResult> {
        let OverlayKind::TextInput(state) = &self.focused_overlay()?.kind else {
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

    #[must_use]
    pub fn mcp_add_form_result(&self) -> Option<&crate::dialogs::McpAddFormResult> {
        let OverlayKind::McpAddForm(state) = &self.focused_overlay()?.kind else {
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
            picked_prefix: false,
            selected_option_label: None,
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
                picked_prefix: false,
                selected_option_label: None,
            })
            .collect();
        self.mode = self.overlay_mode();
        results
    }

    pub fn confirm_approval(&mut self) -> Option<ApprovalResult> {
        if let Some(modal) = self.pending_approvals.pop_front() {
            let selected = modal.modal.selected;
            let selected_label = modal
                .modal
                .options
                .get(selected)
                .map(|opt| opt.label.clone());
            let choice = modal.modal.selected_choice()?;
            // The prefix option (Layer 2) is rendered as
            // "Approve commands starting with …" and uses AlwaysApprove. Detect
            // it by label so the controller persists a prefix rule instead of a
            // session key.
            let picked_prefix = choice == ApprovalChoice::AlwaysApprove
                && selected_label
                    .as_deref()
                    .is_some_and(|label| label.starts_with("Approve commands starting with"));
            // Plan-review approve choices occupy the leading indices, one per
            // model-supplied label. Recover the chosen approach label only when
            // the user actually picked one of those entries.
            let selected_option_label = (choice == ApprovalChoice::Approve
                && selected < modal.plan_option_labels.len())
            .then(|| modal.plan_option_labels[selected].clone());
            let result = ApprovalResult {
                request_id: modal.request_id,
                choice,
                feedback: (choice == ApprovalChoice::Revise)
                    .then_some(modal.feedback_input)
                    .filter(|feedback| !feedback.is_empty()),
                picked_prefix,
                selected_option_label,
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
            picked_prefix: false,
            selected_option_label: None,
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
                if let Some(number) = approval::approval_number(character)
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
                | OverlayKind::McpManager(_)
                | OverlayKind::McpAddForm(_)
                | OverlayKind::ChoicePicker(_)
                | OverlayKind::ApiKeyInput(_)
                | OverlayKind::TextInput(_)
                | OverlayKind::CustomRegistryImport(_)
                | OverlayKind::QuestionDialog(_)
                | OverlayKind::Approval(_)
                | OverlayKind::TrustDialog(_)
                | OverlayKind::TaskBrowser(_)
        )
    }

    #[must_use]
    pub fn task_browser_state(&self) -> Option<&TaskBrowserState> {
        let OverlayKind::TaskBrowser(state) = &self.focused_overlay()?.kind else {
            return None;
        };
        Some(state)
    }

    pub fn task_browser_state_mut(&mut self) -> Option<&mut TaskBrowserState> {
        let id = self.focused_overlay?;
        let overlay = self.overlays.iter_mut().find(|overlay| overlay.id == id)?;
        let OverlayKind::TaskBrowser(state) = &mut overlay.kind else {
            return None;
        };
        Some(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backspace_selects_marker_first_then_deletes() {
        let mut prompt = PromptState::new("hello [paste +5 lines]");
        prompt.cursor = prompt.char_len();
        // First backspace selects marker, text unchanged.
        assert!(prompt.apply_edit(PromptEdit::Backspace).is_none());
        assert!(prompt.text.contains("[paste +5 lines]"));
        assert!(prompt.selected_marker().is_some());
        // Second backspace deletes marker.
        assert!(prompt.apply_edit(PromptEdit::Backspace).is_some());
        assert!(!prompt.text.contains("[paste +5 lines]"));
        assert_eq!(prompt.text, "hello ");
    }

    #[test]
    fn delete_selects_marker_first_then_deletes() {
        let mut prompt = PromptState::new("[paste +5 lines] world");
        prompt.cursor = 0;
        assert!(prompt.apply_edit(PromptEdit::Delete).is_none());
        assert!(prompt.text.contains("[paste +5 lines]"));
        assert!(prompt.apply_edit(PromptEdit::Delete).is_some());
        assert!(!prompt.text.contains("[paste +5 lines]"));
        assert_eq!(prompt.text, " world");
    }

    #[test]
    fn cursor_movement_clears_marker_selection() {
        let mut prompt = PromptState::new("hello [paste +5 lines]");
        prompt.cursor = prompt.char_len();
        prompt.apply_edit(PromptEdit::Backspace);
        assert!(prompt.selected_marker().is_some());
        prompt.apply_edit(PromptEdit::MoveLeft);
        assert!(prompt.selected_marker().is_none());
    }

    #[test]
    fn normal_backspace_still_deletes_single_character() {
        let mut prompt = PromptState::new("hello");
        prompt.cursor = prompt.char_len();
        prompt.apply_edit(PromptEdit::Backspace);
        assert_eq!(prompt.text, "hell");
    }

    #[test]
    fn mcp_add_form_overlay_opens_and_has_no_result_initially() {
        let mut chrome = NeoChromeState::new("title", "session", "model", "/tmp");
        let opts = crate::dialogs::McpAddFormOptions {
            title: "Add MCP Server".into(),
            transport: "stdio".into(),
        };
        let id = chrome.open_mcp_add_form(opts);

        assert_eq!(chrome.focused_overlay_id(), Some(id));
        assert!(chrome.focused_overlay_is_rich_dialog());
        assert!(chrome.focused_overlay_blocks_prompt());
        assert!(chrome.mcp_add_form_result().is_none());

        let lines = chrome.focused_overlay_lines(80);
        assert!(!lines.is_empty());
        // The dialog reserves 16 rows so the larger http/sse form fits; the
        // actual rendered lines may be fewer (e.g. 11 for stdio).
        assert_eq!(chrome.focused_overlay_height(), 16);
    }
}
