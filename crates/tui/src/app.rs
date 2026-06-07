use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Editing,
    Streaming,
    Overlay,
    Approval,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeoTuiApp {
    title: String,
    session_label: String,
    model_label: String,
    transcript: ChatTranscript,
    transcript_view: TranscriptView,
    prompt: PromptState,
    mode: AppMode,
    overlays: Vec<Overlay>,
    next_overlay_id: OverlayId,
    focused_overlay: Option<OverlayId>,
    active_assistant_id: Option<String>,
    active_assistant_buffer: String,
    active_tools: Vec<ActiveTool>,
}

impl NeoTuiApp {
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        session_label: impl Into<String>,
        model_label: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            session_label: session_label.into(),
            model_label: model_label.into(),
            transcript: ChatTranscript::default(),
            transcript_view: TranscriptView::new(),
            prompt: PromptState::default(),
            mode: AppMode::Editing,
            overlays: Vec::new(),
            next_overlay_id: OverlayId::default(),
            focused_overlay: None,
            active_assistant_id: None,
            active_assistant_buffer: String::new(),
            active_tools: Vec::new(),
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
    pub const fn mode(&self) -> AppMode {
        self.mode
    }

    #[must_use]
    pub const fn transcript(&self) -> &ChatTranscript {
        &self.transcript
    }

    #[must_use]
    pub const fn transcript_view(&self) -> &TranscriptView {
        &self.transcript_view
    }

    #[must_use]
    pub const fn prompt(&self) -> &PromptState {
        &self.prompt
    }

    pub fn prompt_mut(&mut self) -> &mut PromptState {
        &mut self.prompt
    }

    pub fn transcript_mut(&mut self) -> &mut ChatTranscript {
        &mut self.transcript
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
                if !tool.detail.is_empty() {
                    status = status.with_detail(tool.detail.clone());
                }
                status
            })
            .collect()
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
        self.prompt = PromptState::default();
        self.mode = AppMode::Streaming;
        self.transcript_view.follow_bottom();
        Some(submitted)
    }

    pub fn apply_stream_update(&mut self, update: StreamUpdate) {
        match update {
            StreamUpdate::AssistantStarted { id } => {
                self.active_assistant_id = Some(id);
                self.active_assistant_buffer.clear();
                self.transcript.push(TranscriptItem::assistant(""));
                self.mode = AppMode::Streaming;
            }
            StreamUpdate::TextDelta { text } => {
                if self.active_assistant_id.is_none() {
                    self.active_assistant_id = Some(String::new());
                    self.transcript.push(TranscriptItem::assistant(""));
                }
                self.active_assistant_buffer.push_str(&text);
                let _ = self
                    .transcript
                    .update_last_assistant(self.active_assistant_buffer.clone());
            }
            StreamUpdate::ToolStarted { id, name, detail } => {
                self.active_tools.push(ActiveTool {
                    id,
                    name,
                    detail,
                    status: ToolStatusKind::Running,
                });
            }
            StreamUpdate::ToolUpdated { id, detail } => {
                if let Some(tool) = self.active_tools.iter_mut().find(|tool| tool.id == id) {
                    tool.detail = detail;
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
                    tool.detail = detail;
                    tool.status = status;
                    self.transcript
                        .push(TranscriptItem::tool(tool.name, tool.detail, tool.status));
                }
            }
            StreamUpdate::Notice { text } => {
                self.transcript.push(TranscriptItem::notice(text));
            }
            StreamUpdate::Error { text } => {
                self.transcript
                    .push(TranscriptItem::notice(format!("Error: {text}")));
                self.mode = self.overlay_mode();
            }
            StreamUpdate::TurnFinished => {
                self.active_assistant_id = None;
                self.active_assistant_buffer.clear();
                self.active_tools.clear();
                self.mode = self.overlay_mode();
            }
        }
        self.transcript_view.follow_bottom();
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

    pub fn move_overlay_selection_down(&mut self) {
        self.with_focused_overlay_mut(Overlay::move_selection_down);
    }

    pub fn move_overlay_selection_up(&mut self) {
        self.with_focused_overlay_mut(Overlay::move_selection_up);
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
            if matches!(overlay.kind, OverlayKind::Approval(_)) {
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveTool {
    id: String,
    name: String,
    detail: String,
    status: ToolStatusKind,
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
    Error {
        text: String,
    },
    TurnFinished,
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
            OverlayKind::Approval(request) => request.move_down(),
            OverlayKind::Message(_) => {}
        }
    }

    pub fn move_selection_up(&mut self) {
        match &mut self.kind {
            OverlayKind::CommandPalette(state) => state.move_up(),
            OverlayKind::SessionPicker(state) | OverlayKind::ModelPicker(state) => {
                state.move_up();
            }
            OverlayKind::Approval(request) => request.move_up(),
            OverlayKind::Message(_) => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayKind {
    CommandPalette(CommandPaletteState),
    SessionPicker(SessionPickerState),
    ModelPicker(ModelPickerState),
    Approval(ApprovalRequestModal),
    Message(String),
}

pub type SessionPickerState = PickerState;
pub type ModelPickerState = PickerState;

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
        Self {
            list: SelectListState::new(items.into_iter().map(SelectItem::from), 8),
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
        content: String,
    },
    Tool {
        name: String,
        detail: String,
        status: ToolStatusKind,
    },
    Notice {
        content: String,
    },
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
        text: String,
    },
    Code {
        language: Option<String>,
        text: String,
    },
    Quote {
        text: String,
    },
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
            | Self::Quote { text } => text,
        }
    }

    #[must_use]
    pub fn display_text(&self) -> String {
        match self {
            Self::Blank => String::new(),
            Self::Text { text } => text.clone(),
            Self::Heading { level, text } => {
                format!("{} {text}", "#".repeat(usize::from(*level)))
            }
            Self::ListItem { indent, text } => format!("{}- {text}", " ".repeat(indent * 2)),
            Self::Code { text, .. } => format!("  {text}"),
            Self::Quote { text } => format!("> {text}"),
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
    pub fn render_markdownish(&self, text: &str) -> Vec<TranscriptLine> {
        let mut lines = Vec::new();
        let mut code_language: Option<String> = None;

        for raw_line in text.lines() {
            let trimmed_end = raw_line.trim_end();
            let trimmed = trimmed_end.trim_start();
            if let Some(language) = fence_language(trimmed) {
                if code_language.is_some() {
                    code_language = None;
                } else {
                    code_language = Some(language);
                }
                continue;
            }

            if let Some(language) = &code_language {
                push_wrapped_line(&mut lines, trimmed_end, self.width, |text| {
                    TranscriptLine::Code {
                        language: Some(language.clone()),
                        text,
                    }
                });
                continue;
            }

            if trimmed.is_empty() {
                lines.push(TranscriptLine::Blank);
            } else if let Some((level, heading)) = parse_heading(trimmed) {
                push_wrapped_line(&mut lines, heading, self.width, |text| {
                    TranscriptLine::Heading { level, text }
                });
            } else if let Some((indent, text)) = parse_list_item(trimmed_end) {
                push_wrapped_line(&mut lines, text, self.width, |text| {
                    TranscriptLine::ListItem { indent, text }
                });
            } else if let Some(text) = trimmed.strip_prefix("> ") {
                push_wrapped_line(&mut lines, text, self.width, |text| TranscriptLine::Quote {
                    text,
                });
            } else {
                push_wrapped_line(&mut lines, trimmed_end, self.width, |text| {
                    TranscriptLine::Text { text }
                });
            }
        }

        if lines.is_empty() {
            lines.push(TranscriptLine::Blank);
        }
        lines
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

fn parse_list_item(line: &str) -> Option<(usize, &str)> {
    let leading_spaces = line
        .chars()
        .take_while(|character| *character == ' ')
        .count();
    let indent = leading_spaces / 2;
    let trimmed = line.trim_start();
    if let Some(text) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
    {
        return Some((indent, text));
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
        .map(|text| (indent, text))
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
        Self::Assistant {
            content: content.into(),
        }
    }

    #[must_use]
    pub fn tool(
        name: impl Into<String>,
        detail: impl Into<String>,
        status: ToolStatusKind,
    ) -> Self {
        Self::Tool {
            name: name.into(),
            detail: detail.into(),
            status,
        }
    }

    #[must_use]
    pub fn notice(content: impl Into<String>) -> Self {
        Self::Notice {
            content: content.into(),
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
        let Some(TranscriptItem::Assistant { content: existing }) = self
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
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptView {
    scrollback: usize,
}

impl TranscriptView {
    #[must_use]
    pub const fn new() -> Self {
        Self { scrollback: 0 }
    }

    #[must_use]
    pub const fn scrollback(&self) -> usize {
        self.scrollback
    }

    pub fn follow_bottom(&mut self) {
        self.scrollback = 0;
    }

    pub fn scroll_up(&mut self, lines: usize, transcript: &ChatTranscript, height: usize) {
        let max_scrollback = transcript.len().saturating_sub(height);
        self.scrollback = self.scrollback.saturating_add(lines).min(max_scrollback);
    }

    pub fn scroll_down(&mut self, lines: usize, transcript: &ChatTranscript, height: usize) {
        let max_scrollback = transcript.len().saturating_sub(height);
        self.scrollback = self.scrollback.saturating_sub(lines).min(max_scrollback);
    }

    #[must_use]
    pub fn visible_range(&self, transcript: &ChatTranscript, height: usize) -> Range<usize> {
        if height == 0 || transcript.is_empty() {
            return 0..0;
        }

        let len = transcript.len();
        let window = height.min(len);
        let bottom = len.saturating_sub(self.scrollback).max(window);
        bottom - window..bottom
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
}

impl PromptState {
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.chars().count();
        Self { text, cursor }
    }

    #[must_use]
    pub fn with_cursor(mut self, cursor: usize) -> Self {
        self.cursor = cursor.min(self.text.chars().count());
        self
    }

    pub fn apply_edit(&mut self, edit: PromptEdit<'_>) -> Option<String> {
        self.cursor = self.cursor.min(self.char_len());

        match edit {
            PromptEdit::Insert(text) => {
                let inserted = text.to_string();
                let index = self.byte_index(self.cursor);
                self.text.insert_str(index, &inserted);
                self.cursor += inserted.chars().count();
                Some(inserted)
            }
            PromptEdit::Backspace => self.delete_range(
                self.cursor.saturating_sub(1),
                self.cursor,
                DeleteDirection::Backward,
            ),
            PromptEdit::Delete => {
                self.delete_range(self.cursor, self.cursor + 1, DeleteDirection::Forward)
            }
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
                self.delete_range(start, self.cursor, DeleteDirection::Backward)
            }
            PromptEdit::DeleteWordForward => {
                let end = find_word_forward(&self.text, self.cursor);
                self.delete_range(self.cursor, end, DeleteDirection::Forward)
            }
            PromptEdit::DeleteToLineStart => {
                self.delete_range(0, self.cursor, DeleteDirection::Backward)
            }
            PromptEdit::DeleteToLineEnd => {
                self.delete_range(self.cursor, self.char_len(), DeleteDirection::Forward)
            }
        }
    }

    #[must_use]
    pub fn char_len(&self) -> usize {
        self.text.chars().count()
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeleteDirection {
    Backward,
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptEdit<'a> {
    Insert(&'a str),
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
