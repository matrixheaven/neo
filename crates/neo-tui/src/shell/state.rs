use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use neo_agent_core::PermissionMode;

use crate::primitive::Color;
use crate::primitive::theme::{ChromeMode, DevelopmentMode, TuiTheme};
use crate::terminal_image::{ImageRenderPolicy, TerminalImageCapabilities};
use crate::widgets::TodoDisplayItem;

use super::approval::ApprovalRequestModal;
use super::context::ContextWindow;
use super::overlay::{Overlay, OverlayId};
use super::pending_input::PendingInputState;
use super::prompt::PromptState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeoChromeState {
    pub(super) title: String,
    pub(super) session_label: String,
    pub(super) model_label: String,
    pub(super) workspace_root: PathBuf,
    pub(super) context_window: Option<ContextWindow>,
    pub(super) activity_frame: usize,
    pub(super) prompt: PromptState,
    pub(super) copy_buffer: Option<String>,
    pub(super) mode: ChromeMode,
    pub(super) overlays: Vec<Overlay>,
    pub(super) next_overlay_id: OverlayId,
    pub(super) focused_overlay: Option<OverlayId>,
    pub(super) pending_approvals: VecDeque<ApprovalRequestModal>,
    pub(super) pending_question_result: Option<crate::dialogs::QuestionResult>,
    pub(super) image_render_policy: ImageRenderPolicy,
    pub(super) image_capabilities: TerminalImageCapabilities,
    pub(super) theme: TuiTheme,
    pub(super) permission_mode: PermissionMode,
    /// Current development mode indicator (for footer display).
    pub(super) plan_mode_active: bool,
    pub(super) development_mode: DevelopmentMode,
    /// Current todo list for the `TodoPanel`.
    pub(super) todo_items: Vec<TodoDisplayItem>,
    /// Whether the todo panel renders all items instead of the collapsed subset.
    pub(super) todo_panel_expanded: bool,
    /// Optional `/btw` sidecar panel state.
    pub(super) btw_panel_state: Option<crate::widgets::btw_panel::BtwPanelState>,
    /// Optional custom label shown in the footer as a working indicator.
    pub(super) custom_working_label: Option<String>,
    /// Whether the current model has thinking enabled (shown in the footer).
    pub(super) thinking_enabled: bool,
    /// Optional persistent exit-confirmation message shown in the footer.
    pub(super) exit_confirmation_label: Option<String>,
    /// Formatted git branch/status badge shown after the workspace path.
    pub(super) git_status_label: Option<String>,
    /// Pending steers and queued follow-ups waiting to be injected or sent.
    pub(super) pending_input: PendingInputState,
    pub(super) shell_mode_active: bool,
    pub(super) shell_running: bool,
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
            todo_panel_expanded: false,
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

    #[must_use]
    pub const fn todo_panel_expanded(&self) -> bool {
        self.todo_panel_expanded
    }

    #[must_use]
    pub fn todo_panel_has_overflow(&self) -> bool {
        self.todo_items.len() > crate::widgets::todo_panel::MAX_VISIBLE_TODOS
    }

    pub const fn set_todo_panel_expanded(&mut self, expanded: bool) {
        self.todo_panel_expanded = expanded;
    }

    pub const fn toggle_todo_panel_expanded(&mut self) {
        self.todo_panel_expanded = !self.todo_panel_expanded;
    }

    /// Clear the todo panel (e.g. when all items are done).
    pub fn clear_todos(&mut self) {
        self.todo_items.clear();
        self.todo_panel_expanded = false;
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
}
