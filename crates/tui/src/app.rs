use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    ops::Range,
    path::{Path, PathBuf},
};

use neo_agent_core::{AgentEvent, AgentMessage, CompactionPhase, Content, ImageRef};
use ratatui::{layout::Rect, style::Color};

use crate::{
    ImageRenderPolicy, ImageSource, InlineImage, TerminalImageCapabilities, TodoDisplayItem,
    TodoDisplayStatus, TranscriptWidget, app_layout,
    widgets::{QuestionDialogAction, QuestionResult, QuestionStateMachine},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TuiTheme {
    pub background: Color,
    pub surface: Color,
    pub surface_border: Color,
    pub accent: Color,
    pub success: Color,
    pub danger: Color,
    pub warning: Color,
    pub muted: Color,
    pub header: Color,
    pub prompt: Color,
    pub composer_bg: Color,
    pub user: Color,
    pub user_bg: Color,
    pub assistant: Color,
    pub thinking: Color,
    pub notice: Color,
    pub diff_added: Color,
    pub diff_removed: Color,
    pub diff_hunk: Color,
    pub diff_context: Color,
    pub selection_bg: Color,
    pub pending: Color,
    /// No longer used for the running tool header; the new tool card uses
    /// [`Self::accent`] for running tools. Kept for backward compatibility
    /// with user themes.
    pub running: Color,
    pub succeeded: Color,
    pub failed: Color,
    pub cancelled: Color,
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
    pub footer_hint: Color,
}

impl Default for TuiTheme {
    fn default() -> Self {
        Self {
            background: Color::Reset,
            surface: Color::Rgb(31, 35, 43),
            surface_border: Color::Rgb(75, 88, 104),
            accent: Color::Rgb(88, 166, 255),
            success: Color::Rgb(65, 184, 131),
            danger: Color::Rgb(248, 81, 73),
            warning: Color::Rgb(210, 153, 34),
            muted: Color::Rgb(139, 148, 158),
            header: Color::White,
            prompt: Color::White,
            composer_bg: Color::Reset,
            user: Color::Cyan,
            user_bg: Color::Reset,
            assistant: Color::Green,
            thinking: Color::Rgb(139, 148, 158),
            notice: Color::Rgb(139, 148, 158),
            diff_added: Color::Rgb(65, 184, 131),
            diff_removed: Color::Rgb(248, 81, 73),
            diff_hunk: Color::Rgb(210, 153, 34),
            diff_context: Color::Rgb(139, 148, 158),
            selection_bg: Color::DarkGray,
            pending: Color::Rgb(139, 148, 158),
            running: Color::Rgb(210, 153, 34),
            succeeded: Color::Rgb(65, 184, 131),
            failed: Color::Rgb(248, 81, 73),
            cancelled: Color::DarkGray,
            approval_bg: Color::Reset,
            approval_border: Color::Rgb(75, 88, 104),
            approval_title: Color::Rgb(210, 153, 34),
            selected_fg: Color::Black,
            selected_bg: Color::Rgb(88, 166, 255),
            overlay_border: Color::Rgb(88, 166, 255),
            footer_permission_allow: Color::Rgb(65, 184, 131),
            footer_permission_ask: Color::Rgb(88, 166, 255),
            footer_permission_deny: Color::Rgb(248, 81, 73),
            footer_working: Color::Rgb(88, 166, 255),
            footer_context_ok: Color::Rgb(139, 148, 158),
            footer_context_warn: Color::Rgb(210, 153, 34),
            footer_context_critical: Color::Rgb(248, 81, 73),
            footer_hint: Color::Rgb(139, 148, 158),
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
    pub const fn with_accent(mut self, color: Color) -> Self {
        self.accent = color;
        self.overlay_border = color;
        self
    }

    #[must_use]
    pub const fn with_success(mut self, color: Color) -> Self {
        self.success = color;
        self.succeeded = color;
        self
    }

    #[must_use]
    pub const fn with_danger(mut self, color: Color) -> Self {
        self.danger = color;
        self.failed = color;
        self
    }

    #[must_use]
    pub const fn with_warning(mut self, color: Color) -> Self {
        self.warning = color;
        self.running = color;
        self.approval_title = color;
        self
    }

    #[must_use]
    pub const fn with_muted(mut self, color: Color) -> Self {
        self.muted = color;
        self.notice = color;
        self.thinking = color;
        self
    }

    #[must_use]
    pub const fn with_header(mut self, color: Color) -> Self {
        self.header = color;
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
    pub const fn with_user(mut self, color: Color) -> Self {
        self.user = color;
        self
    }

    #[must_use]
    pub const fn with_assistant(mut self, color: Color) -> Self {
        self.assistant = color;
        self
    }

    #[must_use]
    pub const fn with_thinking(mut self, color: Color) -> Self {
        self.thinking = color;
        self
    }

    #[must_use]
    pub const fn with_notice(mut self, color: Color) -> Self {
        self.notice = color;
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

    #[must_use]
    pub const fn with_footer_hint(mut self, color: Color) -> Self {
        self.footer_hint = color;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Editing,
    Streaming,
    Overlay,
    Approval,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeoTuiApp {
    title: String,
    session_label: String,
    model_label: String,
    workspace_root: PathBuf,
    context_window: Option<ContextWindow>,
    activity_frame: usize,
    transcript: ChatTranscript,
    transcript_view: TranscriptView,
    transcript_selection: Option<TranscriptSelection>,
    expanded_transcript_items: BTreeSet<usize>,
    prompt: PromptState,
    copy_buffer: Option<String>,
    mode: AppMode,
    overlays: Vec<Overlay>,
    next_overlay_id: OverlayId,
    focused_overlay: Option<OverlayId>,
    active_assistant_id: Option<String>,
    active_user_prompt: Option<String>,
    active_assistant_buffer: String,
    active_thinking_buffer: String,
    active_tools: Vec<ActiveTool>,
    completed_tool_result_ids: Vec<String>,
    next_image_id: u64,
    image_render_policy: ImageRenderPolicy,
    image_capabilities: TerminalImageCapabilities,
    theme: TuiTheme,
    /// Current agent mode indicator (for footer display)
    plan_mode_active: bool,
    /// Current todo list for the TodoPanel.
    todo_items: Vec<TodoDisplayItem>,
}

impl NeoTuiApp {
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
            transcript: ChatTranscript::default(),
            transcript_view: TranscriptView::new(),
            transcript_selection: None,
            expanded_transcript_items: BTreeSet::new(),
            prompt: PromptState::default(),
            copy_buffer: None,
            mode: AppMode::Editing,
            overlays: Vec::new(),
            next_overlay_id: OverlayId::default(),
            focused_overlay: None,
            active_assistant_id: None,
            active_user_prompt: None,
            active_assistant_buffer: String::new(),
            active_thinking_buffer: String::new(),
            active_tools: Vec::new(),
            completed_tool_result_ids: Vec::new(),
            next_image_id: 0,
            image_render_policy: ImageRenderPolicy::default(),
            image_capabilities: TerminalImageCapabilities::default(),
            theme: TuiTheme::default(),
            plan_mode_active: false,
            todo_items: Vec::new(),
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
        if !self.active_tools.is_empty() {
            return Some("working · esc interrupt".to_owned());
        }
        if !self.active_thinking_buffer.is_empty() {
            return Some("thinking · esc interrupt".to_owned());
        }
        matches!(self.mode, AppMode::Streaming).then(|| "working · esc interrupt".to_owned())
    }

    /// TODO: placeholder until `NeoTuiApp` stores actual permission state.
    #[must_use]
    pub fn permission_badge(&self) -> (&'static str, Color) {
        ("ask", self.theme().footer_permission_ask)
    }

    #[must_use]
    pub fn cwd_label(&self) -> String {
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            if let Ok(rest) = self.workspace_root.strip_prefix(&home) {
                return format!("~{}", rest.display());
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
    pub const fn mode(&self) -> AppMode {
        self.mode
    }

    pub fn set_plan_mode(&mut self, active: bool) {
        self.plan_mode_active = active;
    }

    #[must_use]
    pub const fn is_plan_mode(&self) -> bool {
        self.plan_mode_active
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
    pub fn inline_image_renders(&self) -> Vec<InlineImageRender> {
        self.transcript
            .items()
            .iter()
            .filter_map(|item| {
                inline_image_render(item, self.image_render_policy, self.image_capabilities)
            })
            .collect()
    }

    #[must_use]
    pub fn inline_image_sequences(&self) -> Vec<String> {
        self.inline_image_renders()
            .into_iter()
            .map(|render| render.escape_sequence)
            .collect()
    }

    #[must_use]
    pub const fn transcript(&self) -> &ChatTranscript {
        &self.transcript
    }

    #[must_use]
    pub const fn transcript_view(&self) -> &TranscriptView {
        &self.transcript_view
    }

    pub fn transcript_view_mut(&mut self) -> &mut TranscriptView {
        &mut self.transcript_view
    }

    #[must_use]
    pub const fn transcript_selection(&self) -> Option<&TranscriptSelection> {
        self.transcript_selection.as_ref()
    }

    #[must_use]
    pub const fn expanded_transcript_items(&self) -> &BTreeSet<usize> {
        &self.expanded_transcript_items
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

    pub fn copy_selected_transcript_text(&mut self) -> Option<String> {
        let copied = self
            .transcript_selection
            .as_ref()
            .and_then(|selection| self.transcript.copy_selection(selection))?;
        self.copy_buffer = Some(copied.clone());
        Some(copied)
    }

    pub fn toggle_selected_transcript_detail(&mut self) -> bool {
        let Some(index) = self.selected_transcript_detail_index() else {
            return false;
        };
        if !self.expanded_transcript_items.remove(&index) {
            self.expanded_transcript_items.insert(index);
        }
        true
    }

    fn selected_transcript_detail_index(&self) -> Option<usize> {
        let range = self
            .transcript_selection
            .as_ref()?
            .range(&self.transcript)?;
        let index = range.end.checked_sub(1)?;
        match self.transcript.items().get(index) {
            Some(TranscriptItem::Tool { detail, .. }) if !detail.is_empty() => Some(index),
            _ => None,
        }
    }

    pub fn transcript_mut(&mut self) -> &mut ChatTranscript {
        &mut self.transcript
    }

    pub fn scroll_transcript_up(&mut self, lines: usize) {
        self.transcript_view.scroll_up(lines);
    }

    pub fn scroll_transcript_down(&mut self, lines: usize) {
        self.transcript_view.scroll_down(lines);
    }

    pub fn sync_transcript_view_for_area(&mut self, area: Rect) {
        let body = app_layout(self, area).body;
        let content_rows = TranscriptWidget::new(&self.transcript)
            .with_selection(self.transcript_selection.as_ref())
            .with_expanded_items(&self.expanded_transcript_items)
            .with_theme(self.theme)
            .row_count(body.width);
        self.transcript_view
            .sync(content_rows, usize::from(body.height));
    }

    pub fn select_visible_transcript_item(&mut self) {
        let range = self.transcript_view.visible_range(&self.transcript, 1);
        let Some(index) = range.end.checked_sub(1) else {
            self.transcript_selection = None;
            return;
        };
        if index < self.transcript.len() {
            self.transcript_selection = Some(TranscriptSelection::new(index));
        } else {
            self.transcript_selection = None;
        }
    }

    pub fn extend_transcript_selection_up(&mut self, lines: usize) {
        if self.transcript_selection.is_none() {
            self.select_visible_transcript_item();
        }
        if let Some(selection) = &mut self.transcript_selection {
            selection.extend_up(&self.transcript, lines);
        }
    }

    pub fn extend_transcript_selection_down(&mut self, lines: usize) {
        if self.transcript_selection.is_none() {
            self.select_visible_transcript_item();
        }
        if let Some(selection) = &mut self.transcript_selection {
            selection.extend_down(&self.transcript, lines);
        }
    }

    pub fn clear_transcript_selection(&mut self) {
        self.transcript_selection = None;
    }

    pub fn set_session_label(&mut self, session_label: impl Into<String>) {
        self.session_label = session_label.into();
    }

    pub fn set_model_label(&mut self, model_label: impl Into<String>) {
        self.model_label = model_label.into();
    }

    pub fn load_session_transcript(
        &mut self,
        session_label: impl Into<String>,
        notices: impl IntoIterator<Item = String>,
        messages: impl IntoIterator<Item = AgentMessage>,
    ) {
        self.set_session_label(session_label);
        self.transcript = ChatTranscript::default();
        self.transcript_view = TranscriptView::new();
        self.transcript_selection = None;
        self.expanded_transcript_items.clear();
        self.prompt = PromptState::default();
        self.active_assistant_id = None;
        self.active_user_prompt = None;
        self.active_assistant_buffer.clear();
        self.active_thinking_buffer.clear();
        self.active_tools.clear();
        self.completed_tool_result_ids.clear();
        self.next_image_id = 0;

        for notice in notices {
            self.transcript.push(TranscriptItem::notice(notice));
        }
        for message in messages {
            self.apply_message(message);
        }

        self.transcript_view.follow_bottom();
        self.mode = self.overlay_mode();
    }

    #[must_use]
    pub fn active_assistant_id(&self) -> Option<&str> {
        self.active_assistant_id.as_deref()
    }

    #[must_use]
    pub fn tool_statuses(&self) -> Vec<ToolStatus> {
        self.active_tools
            .iter()
            .map(|tool| {
                let mut status = ToolStatus::new(tool.name.clone(), tool.status);
                if let Some(detail) = tool.status_detail() {
                    status = status.with_detail(detail);
                }
                status
            })
            .collect()
    }

    fn follow_tail_after_transcript_change(&mut self) {
        if self.transcript_view.is_following_tail() {
            self.transcript_view.follow_bottom();
        }
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

    pub fn submit_prompt(&mut self) -> Option<String> {
        let submitted = self.prompt.text.trim_end().to_owned();
        if submitted.trim().is_empty() {
            return None;
        }

        self.transcript
            .push(TranscriptItem::user(submitted.clone()));
        self.active_user_prompt = Some(submitted.clone());
        self.transcript_selection = None;
        self.prompt.remember_history(submitted.clone());
        self.prompt.clear_after_submit();
        self.mode = AppMode::Streaming;
        self.transcript_view.follow_bottom();
        Some(submitted)
    }

    #[allow(clippy::too_many_lines)]
    pub fn apply_stream_update(&mut self, update: StreamUpdate) {
        match update {
            StreamUpdate::AssistantStarted { id } => {
                self.active_assistant_id = Some(id);
                self.active_assistant_buffer.clear();
                self.active_thinking_buffer.clear();
                self.transcript.push(TranscriptItem::assistant(""));
                self.transcript_selection = None;
                self.mode = AppMode::Streaming;
            }
            StreamUpdate::TextDelta { text } => {
                if self.active_assistant_id.is_none() {
                    self.active_assistant_id = Some(String::new());
                    self.transcript.push(TranscriptItem::assistant(""));
                }
                self.active_assistant_buffer.push_str(&text);
                if !self
                    .transcript
                    .update_last_assistant(self.active_assistant_buffer.clone())
                {
                    self.transcript.push(TranscriptItem::assistant(
                        self.active_assistant_buffer.clone(),
                    ));
                }
            }
            StreamUpdate::ToolStarted { id, name, detail } => {
                self.transcript_selection = None;
                let presentation = tool_presentation_kind(&name);
                if let Some(tool) = self.active_tools.iter_mut().find(|tool| tool.id == id) {
                    tool.name = name;
                    tool.arguments = Some(detail);
                    tool.result = None;
                    tool.metadata = ToolRunMetadata::default();
                    tool.presentation = presentation;
                    tool.status = ToolStatusKind::Running;
                    self.transcript.update_tool_run(
                        tool.transcript_index,
                        tool.clone().into_transcript_item(),
                    );
                } else {
                    let transcript_index = self.transcript.len();
                    let tool = ActiveTool {
                        id,
                        name,
                        arguments: Some(detail),
                        result: None,
                        metadata: ToolRunMetadata::default(),
                        presentation,
                        status: ToolStatusKind::Running,
                        transcript_index,
                    };
                    self.transcript.push(tool.clone().into_transcript_item());
                    self.active_tools.push(ActiveTool { ..tool });
                }
            }
            StreamUpdate::ToolUpdated { id, detail } => {
                if let Some(tool) = self.active_tools.iter_mut().find(|tool| tool.id == id) {
                    if tool.status == ToolStatusKind::Running
                        && tool.presentation == ToolPresentationKind::Shell
                    {
                        if let Some(TranscriptItem::Tool { tool_run, .. }) =
                            self.transcript.items.get_mut(tool.transcript_index)
                        {
                            tool_run
                                .live_output
                                .extend(detail.lines().map(ToOwned::to_owned));
                            if tool_run.live_output.len() > 3 {
                                let excess = tool_run.live_output.len() - 3;
                                tool_run.live_output.rotate_left(excess);
                                tool_run.live_output.truncate(3);
                            }
                        }
                    } else {
                        tool.result = Some(detail);
                        self.transcript.update_tool_run(
                            tool.transcript_index,
                            tool.clone().into_transcript_item(),
                        );
                    }
                }
            }
            StreamUpdate::ToolFinished {
                id,
                detail,
                success,
            } => {
                let status = if success {
                    ToolStatusKind::Succeeded
                } else {
                    ToolStatusKind::Failed
                };
                if let Some(index) = self.active_tools.iter().position(|tool| tool.id == id) {
                    let mut tool = self.active_tools.remove(index);
                    if let Some(TranscriptItem::Tool { tool_run, .. }) =
                        self.transcript.items.get_mut(tool.transcript_index)
                    {
                        tool_run.live_output.clear();
                    }
                    tool.result = Some(detail);
                    tool.status = status;
                    self.transcript
                        .update_tool_run(tool.transcript_index, tool.into_transcript_item());
                }
            }
            StreamUpdate::Notice { text } => {
                self.transcript.push(TranscriptItem::notice(text));
                self.transcript_selection = None;
            }
            StreamUpdate::ThinkingStarted => {
                if self.active_assistant_id.is_none() {
                    self.active_assistant_id = Some(String::new());
                    self.transcript.push(TranscriptItem::assistant(""));
                }
                self.active_thinking_buffer.clear();
                self.mode = AppMode::Streaming;
            }
            StreamUpdate::ThinkingDelta { text } => {
                if self.active_assistant_id.is_none() {
                    self.active_assistant_id = Some(String::new());
                    self.transcript.push(TranscriptItem::assistant(""));
                }
                self.active_thinking_buffer.push_str(&text);
                if !self
                    .transcript
                    .update_last_assistant_thinking(Some(self.active_thinking_buffer.clone()))
                {
                    self.transcript
                        .push(TranscriptItem::assistant_with_thinking(
                            self.active_thinking_buffer.clone(),
                            "",
                        ));
                }
            }
            StreamUpdate::ThinkingFinished => {
                if !self.active_thinking_buffer.is_empty() {
                    let _ = self
                        .transcript
                        .update_last_assistant_thinking(Some(self.active_thinking_buffer.clone()));
                }
                self.active_thinking_buffer.clear();
            }
            StreamUpdate::Error { text } => {
                self.transcript
                    .push(TranscriptItem::notice(format!("Error: {text}")));
                self.transcript_selection = None;
                self.active_user_prompt = None;
                self.mode = self.overlay_mode();
            }
            StreamUpdate::TurnFinished => {
                self.active_assistant_id = None;
                self.active_user_prompt = None;
                self.active_assistant_buffer.clear();
                self.active_thinking_buffer.clear();
                // NOTE: Do NOT clear active_tools here.
                // The runtime emits TurnFinished after the *model* turn, but
                // tool execution (ToolExecutionStarted/Finished) happens
                // *afterwards*. Clearing here would orphan every tool that the
                // model just requested, causing ToolExecutionStarted to push a
                // duplicate TranscriptItem::Tool. Tools are cleaned up either
                // individually by ToolExecutionFinished or collectively by
                // RunFinished.
                self.mode = self.overlay_mode();
            }
            StreamUpdate::RunFinished { turn, stop_reason } => {
                self.active_tools.clear();
                if let Some(text) = run_finished_notice(turn, stop_reason) {
                    self.transcript.push(TranscriptItem::notice(text));
                    self.transcript_selection = None;
                }
            }
            StreamUpdate::PlanModeChanged { active } => {
                self.plan_mode_active = active;
            }
            StreamUpdate::TodoUpdated { todos } => {
                // Auto-clear when all done.
                if !todos.is_empty() && todos.iter().all(|t| t.status == TodoDisplayStatus::Done) {
                    self.todo_items.clear();
                } else {
                    self.todo_items = todos;
                }
            }
            StreamUpdate::QuestionRequested { id, questions } => {
                self.push_question_overlay(id, questions);
            }
        }
        self.follow_tail_after_transcript_change();
    }

    pub fn apply_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::MessageStarted { .. }
            | AgentEvent::TextDelta { .. }
            | AgentEvent::ThinkingStarted { .. }
            | AgentEvent::ThinkingDelta { .. }
            | AgentEvent::ThinkingFinished { .. }
            | AgentEvent::ToolCallStarted { .. }
            | AgentEvent::ToolCallArgumentsDelta { .. }
            | AgentEvent::ToolCallFinished { .. } => self.apply_model_stream_event(event),
            AgentEvent::ToolExecutionStarted { .. }
            | AgentEvent::ToolExecutionUpdate { .. }
            | AgentEvent::ToolExecutionFinished { .. } => self.apply_tool_execution_event(event),
            AgentEvent::ApprovalRequested {
                id,
                operation,
                subject,
                arguments,
                ..
            } => {
                let body = if arguments.is_null() {
                    subject
                } else {
                    format!("{subject}\n{arguments}")
                };
                self.request_approval(id, format!("{operation:?} approval"), body);
            }
            AgentEvent::ShellCommandStarted { .. } | AgentEvent::ShellCommandFinished { .. } => {
                self.apply_shell_event(event);
            }
            AgentEvent::TokenUsage { usage, .. } => {
                if let Some(context_window) = &mut self.context_window {
                    *context_window =
                        context_window.with_used_tokens(usage.input_tokens + usage.output_tokens);
                }
            }
            AgentEvent::SteeringQueued { .. }
            | AgentEvent::FollowUpQueued { .. }
            | AgentEvent::QueueDrained { .. } => self.apply_runtime_notice_event(event),
            AgentEvent::CompactionStarted {
                tokens_before,
                message_count,
                ..
            } => {
                self.start_compaction(tokens_before, message_count);
            }
            AgentEvent::CompactionProgress { phase, percent } => {
                self.update_compaction_progress(phase, percent);
            }
            AgentEvent::CompactionApplied { summary } => {
                self.finish_compaction(summary.first_kept_message_index, summary.tokens_before);
            }
            AgentEvent::MessageAppended { message } => {
                self.apply_message(message);
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
            AgentEvent::RunStarted { .. }
            | AgentEvent::TurnStarted { .. }
            | AgentEvent::MessageFinished { .. }
            | AgentEvent::TerminalSessionStarted { .. }
            | AgentEvent::TerminalSessionOutput { .. }
            | AgentEvent::TerminalSessionFinished { .. } => {}
            AgentEvent::PlanModeEntered { .. } => {
                self.plan_mode_active = true;
            }
            AgentEvent::PlanModeExited { .. } => {
                self.plan_mode_active = false;
            }
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
                // Auto-clear when all done (kimi-code behavior).
                if !display.is_empty()
                    && display.iter().all(|t| t.status == TodoDisplayStatus::Done)
                {
                    self.todo_items.clear();
                } else {
                    self.todo_items = display;
                }
            }
            AgentEvent::QuestionRequested { id, questions, .. } => {
                let display: Vec<crate::QuestionDisplayData> = questions
                    .iter()
                    .map(|q| crate::QuestionDisplayData {
                        question: q.question.clone(),
                        header: q.header.clone(),
                        body: q.body.clone(),
                        options: q
                            .options
                            .iter()
                            .map(|o| crate::QuestionDisplayOption {
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

    fn apply_model_stream_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::MessageStarted { id, .. } => {
                self.apply_stream_update(StreamUpdate::AssistantStarted { id });
            }
            AgentEvent::TextDelta { text, .. } => {
                self.apply_stream_update(StreamUpdate::TextDelta { text });
            }
            AgentEvent::ThinkingStarted { .. } => {
                self.apply_stream_update(StreamUpdate::ThinkingStarted);
            }
            AgentEvent::ThinkingDelta { text, .. } => {
                self.apply_stream_update(StreamUpdate::ThinkingDelta { text });
            }
            AgentEvent::ThinkingFinished { .. } => {
                self.apply_stream_update(StreamUpdate::ThinkingFinished);
            }
            AgentEvent::ToolCallStarted { id, name, .. } => {
                self.apply_stream_update(StreamUpdate::ToolStarted {
                    id,
                    name,
                    detail: String::new(),
                });
            }
            AgentEvent::ToolCallArgumentsDelta {
                id, json_fragment, ..
            } => {
                if let Some(tool) = self.active_tools.iter_mut().find(|tool| tool.id == id) {
                    tool.arguments
                        .get_or_insert_default()
                        .push_str(&json_fragment);
                    self.transcript.update_tool_run(
                        tool.transcript_index,
                        tool.clone().into_transcript_item(),
                    );
                }
            }
            AgentEvent::ToolCallFinished { tool_call, .. } => {
                if let Some(tool) = self
                    .active_tools
                    .iter_mut()
                    .find(|tool| tool.id == tool_call.id)
                {
                    tool.arguments = Some(tool_call.arguments.to_string());
                    self.transcript.update_tool_run(
                        tool.transcript_index,
                        tool.clone().into_transcript_item(),
                    );
                }
            }
            _ => {}
        }
    }

    fn apply_tool_execution_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::ToolExecutionStarted {
                id,
                name,
                arguments,
                ..
            } => {
                self.apply_stream_update(StreamUpdate::ToolStarted {
                    id,
                    name,
                    detail: arguments.to_string(),
                });
            }
            AgentEvent::ToolExecutionUpdate {
                id, partial_result, ..
            } => {
                self.apply_stream_update(StreamUpdate::ToolUpdated {
                    id,
                    detail: tool_result_detail(&partial_result),
                });
            }
            AgentEvent::ToolExecutionFinished {
                id, name, result, ..
            } => self.finish_tool_execution(id, name, &result),
            _ => {}
        }
    }

    fn finish_tool_execution(
        &mut self,
        id: String,
        name: String,
        result: &neo_agent_core::ToolResult,
    ) {
        let success = !result.is_error;
        let detail = tool_result_detail(result);
        if self.active_tools.iter().any(|tool| tool.id == id) {
            self.apply_stream_update(StreamUpdate::ToolFinished {
                id: id.clone(),
                detail,
                success,
            });
            self.completed_tool_result_ids.push(id);
        } else if take_completed_tool_result(&mut self.completed_tool_result_ids, &id) {
            // Tool was already finished by shell events (ShellCommandFinished
            // runs before ToolExecutionFinished in the runtime). The transcript
            // is already up to date — skip.
        } else {
            self.transcript.push(TranscriptItem::tool_run(
                name,
                None,
                Some(detail),
                if success {
                    ToolStatusKind::Succeeded
                } else {
                    ToolStatusKind::Failed
                },
                ToolRunMetadata::default(),
                ToolPresentationKind::Text,
            ));
            self.transcript_selection = None;
            self.follow_tail_after_transcript_change();
        }
    }

    fn apply_shell_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::ShellCommandStarted {
                id, command, cwd, ..
            } => {
                let arguments = format!("{command} ({})", cwd.display());
                self.apply_stream_update(StreamUpdate::ToolStarted {
                    id,
                    name: "shell.run".to_owned(),
                    detail: arguments,
                });
            }
            AgentEvent::ShellCommandFinished {
                id,
                exit_code,
                stdout,
                stderr,
                truncated,
                ..
            } => {
                let detail = shell_finished_detail(exit_code, &stdout, &stderr, truncated);
                let metadata = ToolRunMetadata {
                    exit_code,
                    stdout: if stdout.is_empty() {
                        None
                    } else {
                        Some(stdout)
                    },
                    stderr: if stderr.is_empty() {
                        None
                    } else {
                        Some(stderr)
                    },
                    elapsed: None,
                    truncated,
                };
                self.finish_shell_execution(&id, detail, exit_code == Some(0), metadata);
            }
            _ => {}
        }
    }

    fn finish_shell_execution(
        &mut self,
        id: &str,
        detail: String,
        success: bool,
        metadata: ToolRunMetadata,
    ) {
        let status = if success {
            ToolStatusKind::Succeeded
        } else {
            ToolStatusKind::Failed
        };
        if let Some(index) = self.active_tools.iter().position(|tool| tool.id == id) {
            let mut tool = self.active_tools.remove(index);
            if let Some(TranscriptItem::Tool { tool_run, .. }) =
                self.transcript.items.get_mut(tool.transcript_index)
            {
                tool_run.live_output.clear();
            }
            tool.result = Some(detail);
            tool.metadata = metadata;
            tool.presentation = ToolPresentationKind::Shell;
            tool.status = status;
            self.transcript
                .update_tool_run(tool.transcript_index, tool.into_transcript_item());
            // Mark as completed so the subsequent ToolExecutionFinished event
            // (which the runtime emits with the same id) does not push a
            // duplicate transcript item.
            self.completed_tool_result_ids.push(id.to_owned());
        } else {
            self.transcript.push(TranscriptItem::tool_run(
                "shell.run",
                None,
                Some(detail),
                status,
                metadata,
                ToolPresentationKind::Shell,
            ));
            self.transcript_selection = None;
            self.follow_tail_after_transcript_change();
        }
    }

    fn apply_runtime_notice_event(&mut self, event: AgentEvent) {
        let text = match event {
            AgentEvent::SteeringQueued { message } => {
                format!("Steering queued: {}", message_text(&message))
            }
            AgentEvent::FollowUpQueued { message } => {
                format!("Follow-up queued: {}", message_text(&message))
            }
            AgentEvent::QueueDrained { kind, count } => {
                format!("{kind:?} queue drained ({count})")
            }
            _ => return,
        };
        self.apply_stream_update(StreamUpdate::Notice { text });
    }

    fn start_compaction(&mut self, tokens_before: usize, message_count: usize) {
        let item = TranscriptItem::Compaction {
            phase: Some(CompactionPhase::Estimating),
            percent: 0,
            compacted_message_count: message_count,
            tokens_before,
        };
        if let Some(existing) = self.last_compaction_mut() {
            *existing = item;
        } else {
            self.transcript.push(item);
        }
        self.transcript_selection = None;
        self.follow_tail_after_transcript_change();
    }

    fn update_compaction_progress(&mut self, phase: CompactionPhase, percent: u8) {
        let percent = percent.min(99);
        if let Some(TranscriptItem::Compaction {
            phase: existing_phase,
            percent: existing_percent,
            ..
        }) = self.last_compaction_mut()
        {
            *existing_phase = Some(phase);
            *existing_percent = percent;
        } else {
            self.transcript.push(TranscriptItem::Compaction {
                phase: Some(phase),
                percent,
                compacted_message_count: 0,
                tokens_before: 0,
            });
        }
        self.transcript_selection = None;
        self.follow_tail_after_transcript_change();
    }

    fn finish_compaction(&mut self, compacted_message_count: usize, tokens_before: usize) {
        if let Some(TranscriptItem::Compaction {
            phase,
            percent,
            compacted_message_count: existing_count,
            tokens_before: existing_tokens,
        }) = self.last_compaction_mut()
        {
            *phase = Some(CompactionPhase::Applying);
            *percent = 100;
            *existing_count = compacted_message_count;
            *existing_tokens = tokens_before;
        } else {
            self.transcript.push(TranscriptItem::compaction(
                compacted_message_count,
                tokens_before,
            ));
        }
        self.transcript_selection = None;
        self.follow_tail_after_transcript_change();
    }

    fn last_compaction_mut(&mut self) -> Option<&mut TranscriptItem> {
        self.transcript
            .items
            .iter_mut()
            .rev()
            .find(|item| matches!(item, TranscriptItem::Compaction { .. }))
    }

    fn apply_message(&mut self, message: AgentMessage) {
        match message {
            AgentMessage::User { content } => {
                let text = content_display_text(&content);
                if text.is_empty() {
                    return;
                }
                if self.active_user_prompt.as_deref() == Some(text.as_str()) {
                    return;
                }
                self.transcript.push(TranscriptItem::user(text));
            }
            AgentMessage::Assistant { content, .. } => {
                let (thinking, text, images) = self.assistant_transcript_parts(&content);
                if thinking.is_none() && text.is_empty() {
                    if images.is_empty() {
                        return;
                    }
                    for image in images {
                        self.transcript.push(image);
                    }
                    self.transcript_selection = None;
                    self.follow_tail_after_transcript_change();
                    return;
                }
                if self.active_assistant_id.is_some() {
                    if !self
                        .transcript
                        .update_last_assistant_message(thinking.clone(), text.clone())
                    {
                        self.transcript
                            .push(TranscriptItem::assistant_parts(thinking, text));
                    }
                } else {
                    self.transcript
                        .push(TranscriptItem::assistant_parts(thinking, text));
                }
                for image in images {
                    self.transcript.push(image);
                }
            }
            AgentMessage::ToolResult {
                tool_call_id,
                tool_name,
                is_error,
                content,
                ..
            } => {
                let text = content_display_text(&content);
                if text.is_empty() {
                    return;
                }
                if take_completed_tool_result(&mut self.completed_tool_result_ids, &tool_call_id) {
                    return;
                }
                self.transcript.push(TranscriptItem::tool(
                    tool_name,
                    text,
                    if is_error {
                        ToolStatusKind::Failed
                    } else {
                        ToolStatusKind::Succeeded
                    },
                ));
            }
            AgentMessage::System { content } => {
                let text = content_display_text(&content);
                if text.is_empty() {
                    return;
                }
                self.transcript.push(TranscriptItem::notice(text));
            }
        }
        self.transcript_selection = None;
        self.follow_tail_after_transcript_change();
    }

    fn assistant_transcript_parts(
        &mut self,
        content: &[Content],
    ) -> (Option<String>, String, Vec<TranscriptItem>) {
        let mut thinking_blocks = Vec::new();
        let mut text = String::new();
        let mut images = Vec::new();
        for part in content {
            match part {
                Content::Thinking {
                    text,
                    redacted,
                    signature: _,
                } => {
                    if !text.is_empty() {
                        thinking_blocks.push(text.clone());
                    } else if *redacted {
                        thinking_blocks.push("[Reasoning redacted]".to_owned());
                    }
                }
                Content::Text { text: part_text } => {
                    text.push_str(part_text);
                }
                Content::Image { mime_type, data } => {
                    images.push(self.transcript_image_item(mime_type, data));
                }
            }
        }
        let thinking = (!thinking_blocks.is_empty()).then(|| thinking_blocks.join("\n\n"));
        (thinking, text, images)
    }

    fn transcript_image_item(&mut self, mime_type: &str, data: &ImageRef) -> TranscriptItem {
        self.next_image_id = self.next_image_id.saturating_add(1);
        let id = format!("image-{}", self.next_image_id);
        match data {
            ImageRef::Base64(encoded) => {
                let bytes = decode_base64(encoded).unwrap_or_else(|| encoded.as_bytes().to_vec());
                let inline = InlineImage::bytes(
                    id.clone(),
                    mime_type.to_owned(),
                    bytes,
                    None::<String>,
                    ImageSource::Base64,
                );
                let size_bytes = inline.size_bytes();
                TranscriptItem::image(
                    id,
                    mime_type.to_owned(),
                    size_bytes,
                    None::<String>,
                    ImageSource::Base64,
                    inline.metadata_summary(),
                    inline.into_payload_bytes(),
                )
            }
            ImageRef::Url(url) => {
                let safe_url = sanitized_image_url(url);
                let inline = InlineImage::remote_url(
                    id.clone(),
                    mime_type.to_owned(),
                    safe_url,
                    None::<String>,
                );
                TranscriptItem::image(
                    id,
                    mime_type.to_owned(),
                    None,
                    None::<String>,
                    ImageSource::RemoteUrl,
                    inline.metadata_summary(),
                    None,
                )
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

    pub fn request_approval(
        &mut self,
        request_id: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> OverlayId {
        self.push_overlay(Overlay::new(
            "approval",
            OverlayKind::Approval(ApprovalRequestModal::new(request_id, title, body)),
        ))
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
        items: impl IntoIterator<Item = PickerItem>,
    ) -> OverlayId {
        self.push_overlay(Overlay::new(
            "sessions",
            OverlayKind::SessionPicker(SessionPickerState::new_with_visible(items, 4)),
        ))
    }

    #[must_use]
    pub fn selected_session(&self) -> Option<PickerItem> {
        let OverlayKind::SessionPicker(picker) = &self.focused_overlay()?.kind else {
            return None;
        };
        picker.confirm()
    }

    pub fn confirm_session_picker(&mut self) -> Option<PickerItem> {
        let id = self.focused_overlay;
        let selected = self.selected_session()?;
        if let Some(id) = id {
            let _ = self.close_overlay(id);
        }
        Some(selected)
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

    #[must_use]
    pub fn approval_choice(&self) -> Option<ApprovalChoice> {
        let OverlayKind::Approval(modal) = &self.focused_overlay()?.kind else {
            return None;
        };
        modal.modal.selected_choice()
    }

    pub fn confirm_approval(&mut self) -> Option<ApprovalResult> {
        let id = self.focused_overlay;
        let overlay = self.focused_overlay()?;
        let OverlayKind::Approval(modal) = &overlay.kind else {
            return None;
        };
        let result = ApprovalResult {
            request_id: modal.request_id.clone(),
            choice: modal.modal.selected_choice()?,
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
        questions: Vec<crate::QuestionDisplayData>,
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
        self.with_focused_overlay_mut(Overlay::move_selection_down);
    }

    pub fn move_overlay_selection_up(&mut self) {
        self.with_focused_overlay_mut(Overlay::move_selection_up);
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

    fn overlay_mode(&self) -> AppMode {
        if let Some(overlay) = self.focused_overlay() {
            if matches!(
                overlay.kind,
                OverlayKind::Approval(_) | OverlayKind::QuestionDialog(_)
            ) {
                AppMode::Approval
            } else {
                AppMode::Overlay
            }
        } else if self.active_assistant_id.is_some() || !self.active_tools.is_empty() {
            AppMode::Streaming
        } else {
            AppMode::Editing
        }
    }
}

fn message_text(message: &AgentMessage) -> String {
    let content = match message {
        AgentMessage::System { content }
        | AgentMessage::User { content }
        | AgentMessage::Assistant { content, .. }
        | AgentMessage::ToolResult { content, .. } => content,
    };

    content
        .iter()
        .filter_map(content_visible_text)
        .collect::<String>()
}

fn content_display_text(content: &[Content]) -> String {
    content.iter().filter_map(content_visible_text).collect()
}

fn content_visible_text(content: &Content) -> Option<String> {
    match content {
        Content::Text { text } => Some(text.clone()),
        Content::Thinking { .. } => None,
        Content::Image { mime_type, data } => Some(image_summary(mime_type, data)),
    }
}

fn image_summary(mime_type: &str, data: &ImageRef) -> String {
    match data {
        ImageRef::Url(url) => format!("[image: {mime_type} url={}]", sanitized_image_url(url)),
        ImageRef::Base64(data) => format!("[image: {mime_type} data={} bytes]", data.len()),
    }
}

fn sanitized_image_url(url: &str) -> String {
    let end = url.find(['?', '#']).unwrap_or(url.len());
    url[..end].to_owned()
}

fn decode_base64(encoded: &str) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(encoded.len() / 4 * 3);
    let mut buffer = 0_u32;
    let mut bits = 0_u8;

    for byte in encoded.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        if byte == b'=' {
            break;
        }
        let value = base64_value(byte)?;
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        while bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xff) as u8);
        }
    }

    Some(output)
}

const fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn tool_result_detail(result: &neo_agent_core::ToolResult) -> String {
    result.content.clone()
}

fn shell_finished_detail(
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
    truncated: bool,
) -> String {
    let exit_label = exit_code.map_or_else(|| "signal".to_owned(), |code| code.to_string());
    let mut detail = format!("exit {exit_label}");
    if !stdout.is_empty() {
        let _ = write!(detail, ", stdout: {stdout}");
    }
    if !stderr.is_empty() {
        let _ = write!(detail, ", stderr: {stderr}");
    }
    if truncated {
        detail.push_str(", truncated");
    }
    detail
}

fn take_completed_tool_result(completed_tool_result_ids: &mut Vec<String>, id: &str) -> bool {
    if let Some(index) = completed_tool_result_ids
        .iter()
        .position(|completed_id| completed_id == id)
    {
        completed_tool_result_ids.remove(index);
        true
    } else {
        false
    }
}

fn tool_presentation_kind(name: &str) -> ToolPresentationKind {
    if name.eq_ignore_ascii_case("bash")
        || name.eq_ignore_ascii_case("shell")
        || name.eq_ignore_ascii_case("terminal")
    {
        ToolPresentationKind::Shell
    } else {
        ToolPresentationKind::Text
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveTool {
    id: String,
    name: String,
    arguments: Option<String>,
    result: Option<String>,
    metadata: ToolRunMetadata,
    presentation: ToolPresentationKind,
    status: ToolStatusKind,
    transcript_index: usize,
}

impl ActiveTool {
    fn status_detail(&self) -> Option<String> {
        self.result
            .as_ref()
            .filter(|result| !result.is_empty())
            .or_else(|| {
                self.arguments
                    .as_ref()
                    .filter(|arguments| !arguments.is_empty())
            })
            .cloned()
    }

    fn into_transcript_item(self) -> TranscriptItem {
        TranscriptItem::tool_run(
            self.name,
            self.arguments,
            self.result,
            self.status,
            self.metadata,
            self.presentation,
        )
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
    },
    Notice {
        text: String,
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
        questions: Vec<crate::QuestionDisplayData>,
    },
}

fn run_finished_notice(turn: u32, stop_reason: neo_agent_core::StopReason) -> Option<String> {
    match stop_reason {
        neo_agent_core::StopReason::MaxTokens => Some(format!(
            "Run stopped after turn {turn}: model token limit reached."
        )),
        neo_agent_core::StopReason::Error => {
            Some(format!("Run stopped after turn {turn}: runtime error."))
        }
        neo_agent_core::StopReason::Cancelled => {
            Some(format!("Run stopped after turn {turn}: cancelled."))
        }
        neo_agent_core::StopReason::EndTurn | neo_agent_core::StopReason::ToolUse => None,
    }
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
        match &mut self.kind {
            OverlayKind::CommandPalette(state) => state.move_down(),
            OverlayKind::SessionPicker(state) | OverlayKind::ModelPicker(state) => {
                state.move_down();
            }
            OverlayKind::PromptCompletion(state) => state.move_down(),
            OverlayKind::Approval(request) => request.move_down(),
            OverlayKind::QuestionDialog(state) => state.move_cursor_down(),
            OverlayKind::Message(_) => {}
        }
    }

    pub fn move_selection_up(&mut self) {
        match &mut self.kind {
            OverlayKind::CommandPalette(state) => state.move_up(),
            OverlayKind::SessionPicker(state) | OverlayKind::ModelPicker(state) => {
                state.move_up();
            }
            OverlayKind::PromptCompletion(state) => state.move_up(),
            OverlayKind::Approval(request) => request.move_up(),
            OverlayKind::QuestionDialog(state) => state.move_cursor_up(),
            OverlayKind::Message(_) => {}
        }
    }

    pub fn move_selection_page_down(&mut self) {
        match &mut self.kind {
            OverlayKind::CommandPalette(state) => state.page_down(),
            OverlayKind::SessionPicker(state) | OverlayKind::ModelPicker(state) => {
                state.page_down();
            }
            OverlayKind::PromptCompletion(state) => state.page_down(),
            OverlayKind::Approval(_) | OverlayKind::QuestionDialog(_) | OverlayKind::Message(_) => {
            }
        }
    }

    pub fn move_selection_page_up(&mut self) {
        match &mut self.kind {
            OverlayKind::CommandPalette(state) => state.page_up(),
            OverlayKind::SessionPicker(state) | OverlayKind::ModelPicker(state) => {
                state.page_up();
            }
            OverlayKind::PromptCompletion(state) => state.page_up(),
            OverlayKind::Approval(_) | OverlayKind::QuestionDialog(_) | OverlayKind::Message(_) => {
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayKind {
    CommandPalette(CommandPaletteState),
    SessionPicker(SessionPickerState),
    ModelPicker(ModelPickerState),
    PromptCompletion(PromptCompletionState),
    Approval(ApprovalRequestModal),
    QuestionDialog(QuestionStateMachine),
    Message(String),
}

pub type SessionPickerState = PickerState;
pub type ModelPickerState = PickerState;

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
            modal: ApprovalModal::new(
                title,
                body,
                [
                    ApprovalOption::new(ApprovalChoice::Approve, "Approve once"),
                    ApprovalOption::new(ApprovalChoice::Deny, "Deny"),
                    ApprovalOption::new(ApprovalChoice::AlwaysApprove, "Always approve"),
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalResult {
    pub request_id: String,
    pub choice: ApprovalChoice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptItem {
    User {
        content: String,
    },
    Assistant {
        thinking: Option<String>,
        content: String,
    },
    Tool {
        name: String,
        detail: String,
        status: ToolStatusKind,
        tool_run: ToolRunTranscript,
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
        phase: Option<CompactionPhase>,
        percent: u8,
        compacted_message_count: usize,
        tokens_before: usize,
    },
    Notice {
        content: String,
    },
    Banner {
        title: String,
        session_label: String,
        model_label: String,
        workspace_root: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRunTranscript {
    pub name: String,
    pub arguments: Option<String>,
    pub result: Option<String>,
    pub live_output: Vec<String>,
    pub status: ToolStatusKind,
    pub metadata: ToolRunMetadata,
    pub presentation: ToolPresentationKind,
}

impl ToolRunTranscript {
    #[must_use]
    pub fn display_detail(&self) -> String {
        if !self.live_output.is_empty() {
            return self.live_output.join("\n");
        }
        self.result
            .as_ref()
            .filter(|result| !result.is_empty())
            .or_else(|| {
                self.arguments
                    .as_ref()
                    .filter(|arguments| !arguments.is_empty())
            })
            .cloned()
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolRunMetadata {
    pub exit_code: Option<i32>,
    pub elapsed: Option<String>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolPresentationKind {
    #[default]
    Text,
    Shell,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineImageRender {
    pub id: String,
    pub escape_sequence: String,
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
        renders: impl IntoIterator<Item = InlineImageRender>,
    ) -> Vec<InlineImageRender> {
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

fn inline_image_render(
    item: &TranscriptItem,
    image_render_policy: ImageRenderPolicy,
    image_capabilities: TerminalImageCapabilities,
) -> Option<InlineImageRender> {
    let TranscriptItem::Image {
        id,
        mime_type,
        alt,
        source,
        payload,
        ..
    } = item
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptLine {
    Blank,
    Text {
        text: String,
    },
    Heading {
        level: u8,
        text: String,
    },
    ListItem {
        indent: usize,
        marker: ListMarker,
        text: String,
    },
    Code {
        language: Option<String>,
        text: String,
    },
    Quote {
        text: String,
    },
    DiffFileHeader {
        marker: char,
        path: String,
    },
    DiffHunk {
        text: String,
    },
    DiffContext {
        text: String,
    },
    DiffAdded {
        text: String,
    },
    DiffRemoved {
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListMarker {
    Bullet,
    Ordered(String),
    TaskOpen,
    TaskDone,
}

impl ListMarker {
    #[must_use]
    pub fn display(&self) -> &str {
        match self {
            Self::Bullet => "•",
            Self::Ordered(marker) => marker.as_str(),
            Self::TaskOpen => "○",
            Self::TaskDone => "✓",
        }
    }
}

impl TranscriptLine {
    #[must_use]
    pub fn text(&self) -> &str {
        match self {
            Self::Blank => "",
            Self::Text { text }
            | Self::Heading { text, .. }
            | Self::ListItem { text, .. }
            | Self::Code { text, .. }
            | Self::Quote { text }
            | Self::DiffHunk { text }
            | Self::DiffContext { text }
            | Self::DiffAdded { text }
            | Self::DiffRemoved { text } => text,
            Self::DiffFileHeader { path, .. } => path,
        }
    }

    #[must_use]
    pub fn display_text(&self) -> String {
        match self {
            Self::Blank => String::new(),
            Self::Text { text } | Self::DiffHunk { text } | Self::Heading { text, .. } => {
                text.clone()
            }
            Self::ListItem {
                indent,
                marker,
                text,
            } => format!("{}{} {text}", " ".repeat(indent * 2), marker.display()),
            Self::Code { text, .. } => format!("  {text}"),
            Self::Quote { text } => format!("│ {text}"),
            Self::DiffFileHeader { marker, path } => format!("{marker}{marker}{marker} {path}"),
            Self::DiffContext { text } => format!(" {text}"),
            Self::DiffAdded { text } => format!("+{text}"),
            Self::DiffRemoved { text } => format!("-{text}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptRenderer {
    width: usize,
}

impl TranscriptRenderer {
    #[must_use]
    pub const fn new(width: usize) -> Self {
        Self { width }
    }

    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    #[must_use]
    #[allow(clippy::needless_continue)]
    pub fn render_markdownish(&self, text: &str) -> Vec<TranscriptLine> {
        let mut lines = Vec::new();
        let mut code_language: Option<String> = None;
        let mut in_diff = false;
        let mut table_rows: Vec<Vec<String>> = Vec::new();

        for raw_line in text.lines() {
            let trimmed_end = raw_line.trim_end();
            let trimmed = trimmed_end.trim_start();
            if let Some(language) = fence_language(trimmed) {
                flush_table(&mut lines, &mut table_rows, self.width);
                if code_language.is_some() {
                    code_language = None;
                } else {
                    code_language = Some(language);
                }
                continue;
            }

            if let Some(language) = &code_language {
                if language == "diff" {
                    if let Some(line) = parse_diff_line(trimmed_end, true) {
                        push_diff_line(&mut lines, line, self.width);
                    }
                    continue;
                }
                push_wrapped_line(&mut lines, trimmed_end, self.width, |text| {
                    TranscriptLine::Code {
                        language: Some(language.clone()),
                        text,
                    }
                });
                continue;
            }

            if let Some(line) = parse_diff_line(trimmed_end, in_diff) {
                flush_table(&mut lines, &mut table_rows, self.width);
                push_diff_line(&mut lines, line, self.width);
                in_diff = true;
                continue;
            }
            if in_diff && !trimmed.is_empty() {
                in_diff = false;
            }

            if trimmed.is_empty() {
                in_diff = false;
                flush_table(&mut lines, &mut table_rows, self.width);
                lines.push(TranscriptLine::Blank);
            } else if let Some((level, heading)) = parse_heading(trimmed) {
                flush_table(&mut lines, &mut table_rows, self.width);
                push_wrapped_line(
                    &mut lines,
                    &strip_inline_markdown(heading),
                    self.width,
                    |text| TranscriptLine::Heading { level, text },
                );
            } else if let Some((indent, marker, text)) = parse_list_item(trimmed_end) {
                flush_table(&mut lines, &mut table_rows, self.width);
                push_wrapped_line(
                    &mut lines,
                    &strip_inline_markdown(&text),
                    self.width,
                    |text| TranscriptLine::ListItem {
                        indent,
                        marker: marker.clone(),
                        text,
                    },
                );
            } else if is_markdown_table_separator(trimmed) {
                continue;
            } else if let Some(cells) = parse_table_row(trimmed) {
                table_rows.push(cells);
            } else if let Some(text) = trimmed.strip_prefix("> ") {
                flush_table(&mut lines, &mut table_rows, self.width);
                push_wrapped_line(
                    &mut lines,
                    &strip_inline_markdown(text),
                    self.width,
                    |text| TranscriptLine::Quote { text },
                );
            } else {
                flush_table(&mut lines, &mut table_rows, self.width);
                push_wrapped_line(
                    &mut lines,
                    &strip_inline_markdown(trimmed_end),
                    self.width,
                    |text| TranscriptLine::Text { text },
                );
            }
        }
        flush_table(&mut lines, &mut table_rows, self.width);

        if lines.is_empty() {
            lines.push(TranscriptLine::Blank);
        }
        lines
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffLine<'a> {
    FileHeader { marker: char, path: &'a str },
    Hunk(&'a str),
    Context(&'a str),
    Added(&'a str),
    Removed(&'a str),
}

fn parse_diff_line(line: &str, in_diff: bool) -> Option<DiffLine<'_>> {
    if let Some(path) = line.strip_prefix("--- ") {
        return Some(DiffLine::FileHeader { marker: '-', path });
    }
    if let Some(path) = line.strip_prefix("+++ ") {
        return Some(DiffLine::FileHeader { marker: '+', path });
    }
    if !in_diff {
        return None;
    }
    if line.starts_with("@@") {
        return Some(DiffLine::Hunk(line));
    }
    if let Some(text) = line.strip_prefix('+') {
        return Some(DiffLine::Added(text));
    }
    if let Some(text) = line.strip_prefix('-') {
        return Some(DiffLine::Removed(text));
    }
    if let Some(text) = line.strip_prefix(' ') {
        return Some(DiffLine::Context(text));
    }
    None
}

fn push_diff_line(lines: &mut Vec<TranscriptLine>, line: DiffLine<'_>, width: usize) {
    match line {
        DiffLine::FileHeader { marker, path } => {
            let content_width = width.saturating_sub(4).max(1);
            push_wrapped_line(lines, path, content_width, |path| {
                TranscriptLine::DiffFileHeader { marker, path }
            });
        }
        DiffLine::Hunk(text) => {
            push_wrapped_line(lines, text, width.max(1), |text| TranscriptLine::DiffHunk {
                text,
            });
        }
        DiffLine::Context(text) => {
            let content_width = width.saturating_sub(1).max(1);
            push_wrapped_line(lines, text, content_width, |text| {
                TranscriptLine::DiffContext { text }
            });
        }
        DiffLine::Added(text) => {
            let content_width = width.saturating_sub(1).max(1);
            push_wrapped_line(lines, text, content_width, |text| {
                TranscriptLine::DiffAdded { text }
            });
        }
        DiffLine::Removed(text) => {
            let content_width = width.saturating_sub(1).max(1);
            push_wrapped_line(lines, text, content_width, |text| {
                TranscriptLine::DiffRemoved { text }
            });
        }
    }
}

fn push_wrapped_line(
    lines: &mut Vec<TranscriptLine>,
    text: &str,
    width: usize,
    make_line: impl Fn(String) -> TranscriptLine,
) {
    for line in crate::wrap_width(text, width.max(1)) {
        lines.push(make_line(line));
    }
}

fn fence_language(line: &str) -> Option<String> {
    line.strip_prefix("```")
        .map(str::trim)
        .map(ToOwned::to_owned)
}

fn parse_heading(line: &str) -> Option<(u8, &str)> {
    let level = line
        .chars()
        .take_while(|character| *character == '#')
        .count();
    if level == 0 || level > 6 {
        return None;
    }
    let text = line.get(level..)?.strip_prefix(' ')?;
    Some((u8::try_from(level).expect("heading level is <= 6"), text))
}

fn parse_list_item(line: &str) -> Option<(usize, ListMarker, String)> {
    let leading_spaces = line
        .chars()
        .take_while(|character| *character == ' ')
        .count();
    let indent = leading_spaces / 2;
    let trimmed = line.trim_start();
    if let Some(text) = trimmed.strip_prefix("- [x] ") {
        return Some((indent, ListMarker::TaskDone, text.to_owned()));
    }
    if let Some(text) = trimmed.strip_prefix("- [X] ") {
        return Some((indent, ListMarker::TaskDone, text.to_owned()));
    }
    if let Some(text) = trimmed.strip_prefix("- [ ] ") {
        return Some((indent, ListMarker::TaskOpen, text.to_owned()));
    }
    if let Some(text) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
    {
        return Some((indent, ListMarker::Bullet, text.to_owned()));
    }

    let marker_end = trimmed.find(['.', ')'])?;
    if marker_end == 0
        || !trimmed[..marker_end]
            .chars()
            .all(|character| character.is_ascii_digit())
    {
        return None;
    }
    trimmed
        .get(marker_end + 1..)?
        .strip_prefix(' ')
        .map(|text| {
            (
                indent,
                ListMarker::Ordered(trimmed[..=marker_end].to_owned()),
                text.to_owned(),
            )
        })
}

fn parse_table_row(line: &str) -> Option<Vec<String>> {
    if !line.starts_with('|') || !line.ends_with('|') {
        return None;
    }
    let cells = line
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if cells.len() < 2 {
        return None;
    }
    Some(cells)
}

fn flush_table(lines: &mut Vec<TranscriptLine>, rows: &mut Vec<Vec<String>>, width: usize) {
    if rows.is_empty() {
        return;
    }
    let table = std::mem::take(rows);
    if table.len() < 2 {
        for row in table {
            let text = row.join(" | ");
            push_wrapped_line(lines, &strip_inline_markdown(&text), width, |text| {
                TranscriptLine::Text { text }
            });
        }
        return;
    }

    let widths = table_column_widths(&table);
    let full_width =
        widths.iter().sum::<usize>() + widths.len().saturating_sub(1).saturating_mul(" | ".len());
    if full_width <= width {
        for row in table {
            let text = format_table_row(&row, &widths);
            push_wrapped_line(lines, &strip_inline_markdown(&text), width, |text| {
                TranscriptLine::Text { text }
            });
        }
        return;
    }

    let headers = &table[0];
    for (row_index, row) in table.iter().enumerate().skip(1) {
        if row_index > 1 {
            lines.push(TranscriptLine::Blank);
        }
        for (index, cell) in row.iter().enumerate() {
            let header = headers
                .get(index)
                .filter(|header| !header.is_empty())
                .map_or_else(|| format!("Column {}", index + 1), Clone::clone);
            let text = format!("{header}: {cell}");
            push_wrapped_line(lines, &strip_inline_markdown(&text), width, |text| {
                TranscriptLine::Text { text }
            });
        }
    }
}

fn table_column_widths(rows: &[Vec<String>]) -> Vec<usize> {
    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0; column_count];
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.chars().count());
        }
    }
    widths
}

fn format_table_row(cells: &[String], widths: &[usize]) -> String {
    let last_index = cells.len().saturating_sub(1);
    cells
        .iter()
        .enumerate()
        .map(|(index, cell)| {
            let width = widths.get(index).copied().unwrap_or(cell.chars().count());
            if index == last_index {
                cell.clone()
            } else {
                format!("{cell:<width$}")
            }
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn is_markdown_table_separator(line: &str) -> bool {
    if !line.starts_with('|') || !line.ends_with('|') {
        return false;
    }
    line.trim_matches('|').split('|').all(|cell| {
        let cell = cell.trim();
        cell.len() >= 3 && cell.chars().all(|character| matches!(character, '-' | ':'))
    })
}

fn strip_inline_markdown(text: &str) -> String {
    let mut output = String::new();
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if matches!(character, '*' | '_') {
            if chars.peek() == Some(&character) {
                let _ = chars.next();
            }
            continue;
        }
        output.push(character);
    }
    output
}

impl TranscriptItem {
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self::User {
            content: content.into(),
        }
    }

    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::assistant_parts(None, content)
    }

    #[must_use]
    pub fn assistant_with_thinking(
        thinking: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self::assistant_parts(Some(thinking.into()), content)
    }

    #[must_use]
    pub fn assistant_parts(thinking: Option<String>, content: impl Into<String>) -> Self {
        let thinking = thinking.filter(|thinking| !thinking.is_empty());
        Self::Assistant {
            thinking,
            content: content.into(),
        }
    }

    #[must_use]
    pub fn tool(
        name: impl Into<String>,
        detail: impl Into<String>,
        status: ToolStatusKind,
    ) -> Self {
        let name = name.into();
        let detail = detail.into();
        let tool_run = ToolRunTranscript {
            name: name.clone(),
            arguments: None,
            result: if detail.is_empty() {
                None
            } else {
                Some(detail.clone())
            },
            live_output: Vec::new(),
            status,
            metadata: ToolRunMetadata::default(),
            presentation: ToolPresentationKind::Text,
        };
        Self::Tool {
            name,
            detail,
            status,
            tool_run,
        }
    }

    #[must_use]
    pub fn tool_run(
        name: impl Into<String>,
        arguments: Option<String>,
        result: Option<String>,
        status: ToolStatusKind,
        metadata: ToolRunMetadata,
        presentation: ToolPresentationKind,
    ) -> Self {
        let name = name.into();
        let tool_run = ToolRunTranscript {
            name: name.clone(),
            arguments,
            result,
            live_output: Vec::new(),
            status,
            metadata,
            presentation,
        };
        let detail = tool_run.display_detail();
        Self::Tool {
            name,
            detail,
            status,
            tool_run,
        }
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
            phase: Some(CompactionPhase::Applying),
            percent: 100,
            compacted_message_count,
            tokens_before,
        }
    }

    #[must_use]
    pub fn notice(content: impl Into<String>) -> Self {
        Self::Notice {
            content: content.into(),
        }
    }

    #[must_use]
    pub fn banner(
        title: impl Into<String>,
        session_label: impl Into<String>,
        model_label: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
    ) -> Self {
        Self::Banner {
            title: title.into(),
            session_label: session_label.into(),
            model_label: model_label.into(),
            workspace_root: workspace_root.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatTranscript {
    items: Vec<TranscriptItem>,
}

impl ChatTranscript {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn from_items(items: impl IntoIterator<Item = TranscriptItem>) -> Self {
        Self {
            items: items.into_iter().collect(),
        }
    }

    pub fn push(&mut self, item: TranscriptItem) {
        self.items.push(item);
    }

    pub fn update_last_assistant(&mut self, content: impl Into<String>) -> bool {
        self.update_last_assistant_content(content)
    }

    pub fn update_last_assistant_content(&mut self, content: impl Into<String>) -> bool {
        let Some(TranscriptItem::Assistant {
            content: existing, ..
        }) = self
            .items
            .iter_mut()
            .rev()
            .find(|item| matches!(item, TranscriptItem::Assistant { .. }))
        else {
            return false;
        };

        *existing = content.into();
        true
    }

    pub fn update_last_assistant_thinking(&mut self, thinking: Option<String>) -> bool {
        let Some(TranscriptItem::Assistant {
            thinking: existing, ..
        }) = self
            .items
            .iter_mut()
            .rev()
            .find(|item| matches!(item, TranscriptItem::Assistant { .. }))
        else {
            return false;
        };

        *existing = thinking.filter(|thinking| !thinking.is_empty());
        true
    }

    pub fn update_last_assistant_message(
        &mut self,
        thinking: Option<String>,
        content: impl Into<String>,
    ) -> bool {
        let Some(TranscriptItem::Assistant {
            thinking: existing_thinking,
            content: existing_content,
        }) = self
            .items
            .iter_mut()
            .rev()
            .find(|item| matches!(item, TranscriptItem::Assistant { .. }))
        else {
            return false;
        };

        *existing_thinking = thinking.filter(|thinking| !thinking.is_empty());
        *existing_content = content.into();
        true
    }

    pub fn update_tool_run(&mut self, index: usize, item: TranscriptItem) -> bool {
        let Some(existing @ TranscriptItem::Tool { .. }) = self.items.get_mut(index) else {
            return false;
        };
        *existing = item;
        true
    }

    #[must_use]
    pub fn items(&self) -> &[TranscriptItem] {
        &self.items
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[must_use]
    pub fn copy_selection(&self, selection: &TranscriptSelection) -> Option<String> {
        let range = selection.range(self)?;
        let mut copied = String::new();
        for (offset, item) in self.items[range].iter().enumerate() {
            if offset > 0 {
                copied.push_str("\n\n");
            }
            let (label, content) = transcript_copy_parts(item);
            copied.push_str(label);
            copied.push('\n');
            copied.push_str(&content);
        }
        Some(copied)
    }
}

fn transcript_copy_parts(item: &TranscriptItem) -> (&'static str, String) {
    match item {
        TranscriptItem::User { content } => ("You", content.clone()),
        TranscriptItem::Assistant { thinking, content } => (
            "Assistant",
            assistant_copy_text(thinking.as_deref(), content),
        ),
        TranscriptItem::Tool {
            name,
            detail,
            status,
            ..
        } => ("Tool", format!("{} {} ({})", status.marker(), name, detail)),
        TranscriptItem::Image { metadata, .. } => ("Image", metadata.clone()),
        TranscriptItem::Compaction {
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
        TranscriptItem::Notice { content } => ("Notice", content.clone()),
        TranscriptItem::Banner {
            title,
            session_label,
            model_label,
            workspace_root,
        } => (
            "Banner",
            format!(
                "{title}\nSession: {session_label}\nModel: {model_label}\nWorkspace: {}",
                workspace_root.display()
            ),
        ),
    }
}

fn assistant_copy_text(thinking: Option<&str>, content: &str) -> String {
    match thinking {
        Some(thinking) if !content.is_empty() => format!("{thinking}\n\n{content}"),
        Some(thinking) => thinking.to_owned(),
        None => content.to_owned(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptSelection {
    start: usize,
    end: usize,
}

impl TranscriptSelection {
    #[must_use]
    pub const fn new(index: usize) -> Self {
        Self {
            start: index,
            end: index,
        }
    }

    pub fn extend_up(&mut self, transcript: &ChatTranscript, count: usize) {
        let max_index = transcript.len().saturating_sub(1);
        self.start = self.start.saturating_sub(count).min(max_index);
        self.end = self.end.min(max_index);
    }

    pub fn extend_down(&mut self, transcript: &ChatTranscript, count: usize) {
        let max_index = transcript.len().saturating_sub(1);
        self.start = self.start.min(max_index);
        self.end = self.end.saturating_add(count).min(max_index);
    }

    #[must_use]
    pub fn range(&self, transcript: &ChatTranscript) -> Option<Range<usize>> {
        if transcript.is_empty() {
            return None;
        }
        let max_index = transcript.len() - 1;
        let start = self.start.min(max_index).min(self.end.min(max_index));
        let end = self.start.min(max_index).max(self.end.min(max_index)) + 1;
        Some(start..end)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptView {
    scroll_offset_rows: usize,
    content_rows: usize,
    viewport_rows: usize,
    follow_tail: bool,
}

impl TranscriptView {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            scroll_offset_rows: 0,
            content_rows: 0,
            viewport_rows: 0,
            follow_tail: true,
        }
    }

    #[must_use]
    pub const fn scrollback(&self) -> usize {
        self.scroll_offset_rows
    }

    #[must_use]
    pub const fn is_following_tail(&self) -> bool {
        self.follow_tail
    }

    pub fn follow_bottom(&mut self) {
        self.scroll_offset_rows = 0;
        self.follow_tail = true;
    }

    pub fn scroll_up(&mut self, rows: usize) {
        self.follow_tail = false;
        self.scroll_offset_rows = self.scroll_offset_rows.saturating_add(rows);
        if self.has_synced_dimensions() {
            self.scroll_offset_rows = self.scroll_offset_rows.min(self.max_scroll_offset());
        }
    }

    pub fn scroll_down(&mut self, rows: usize) {
        self.scroll_offset_rows = self.scroll_offset_rows.saturating_sub(rows);
        if self.scroll_offset_rows == 0 {
            self.follow_tail = true;
        }
    }

    pub fn page_up(&mut self) {
        self.scroll_up(self.page_rows());
    }

    pub fn page_down(&mut self) {
        self.scroll_down(self.page_rows());
    }

    pub fn sync(&mut self, content_rows: usize, viewport_rows: usize) {
        self.content_rows = content_rows;
        self.viewport_rows = viewport_rows;
        if self.follow_tail {
            self.scroll_offset_rows = 0;
        } else {
            self.scroll_offset_rows = self.scroll_offset_rows.min(self.max_scroll_offset());
        }
    }

    #[must_use]
    pub fn visible_range(&self, transcript: &ChatTranscript, height: usize) -> Range<usize> {
        if height == 0 || transcript.is_empty() {
            return 0..0;
        }

        let len = transcript.len();
        let window = height.min(len);
        let scrollback = self.scroll_offset_rows.min(len.saturating_sub(window));
        let bottom = len.saturating_sub(scrollback).max(window);
        bottom - window..bottom
    }

    #[must_use]
    pub fn visible_row_range(&self, row_count: usize, height: usize) -> Range<usize> {
        if height == 0 || row_count == 0 {
            return 0..0;
        }

        let window = height.min(row_count);
        let scrollback = self
            .scroll_offset_rows
            .min(row_count.saturating_sub(window));
        let bottom = row_count.saturating_sub(scrollback).max(window);
        bottom - window..bottom
    }

    #[must_use]
    const fn max_scroll_offset(&self) -> usize {
        self.content_rows.saturating_sub(self.viewport_rows)
    }

    #[must_use]
    const fn has_synced_dimensions(&self) -> bool {
        self.content_rows > 0 && self.viewport_rows > 0
    }

    #[must_use]
    fn page_rows(&self) -> usize {
        self.viewport_rows.saturating_sub(1).max(1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatusKind {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl ToolStatusKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Pending => "-",
            Self::Running => "*",
            Self::Succeeded => "+",
            Self::Failed => "!",
            Self::Cancelled => "x",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolStatus {
    pub name: String,
    pub kind: ToolStatusKind,
    pub detail: Option<String>,
}

impl ToolStatus {
    #[must_use]
    pub fn new(name: impl Into<String>, kind: ToolStatusKind) -> Self {
        Self {
            name: name.into(),
            kind,
            detail: None,
        }
    }

    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
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
        use crate::truncate_width;

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
    use crate::{truncate_width, visible_width};

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
