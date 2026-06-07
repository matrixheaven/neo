use crate::config::AppConfig;
use std::{
    future::{Future, Ready, ready},
    io::{IsTerminal as _, Stdout, stdout},
    pin::Pin,
    time::Duration,
};

use anyhow::{Context, Result};
use crossterm::{
    event, execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use neo_agent_core::{
    AgentEvent, AgentMessage,
    session::{JsonlSessionReader, SessionMetadataStore},
};
use neo_tui::{
    InputEvent, KeyId, KeybindingAction, KeybindingsManager, NeoTuiApp, PickerItem, PromptEdit,
};
use ratatui::{
    Terminal,
    backend::{CrosstermBackend, TestBackend},
    buffer::Cell,
};

type BoxedTurnFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<AgentEvent>>> + Send + 'a>>;
type BoxedSessionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<LoadedSessionTranscript>> + Send + 'a>>;
type BoxedForkFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ForkedSessionTranscript>> + Send + 'a>>;

pub fn execute(config: &AppConfig) -> String {
    let mut controller = controller_for_config(config);
    let _ = controller.submit_empty_prompt();
    controller.render_snapshot()
}

pub async fn execute_tty(config: &AppConfig) -> Result<Option<String>> {
    if !stdout().is_terminal() {
        return Ok(Some(execute(config)));
    }

    let mut terminal = RawTerminal::enter()?;
    let mut controller = controller_for_config(config);
    controller
        .run_terminal_loop(|app| terminal.draw(app), CrosstermEvents)
        .await?;
    Ok(None)
}

pub(crate) struct InteractiveController<RunTurn, LoadSession, ForkSession> {
    app: NeoTuiApp,
    keybindings: KeybindingsManager,
    run_turn: RunTurn,
    session_items: Vec<PickerItem>,
    session_list_error: Option<String>,
    model_items: Vec<PickerItem>,
    model_list_error: Option<String>,
    load_session: LoadSession,
    fork_session: ForkSession,
    active_session_id: Option<String>,
    active_model: Option<SelectedModel>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PickerCatalogs {
    session_items: Vec<PickerItem>,
    session_error: Option<String>,
    model_items: Vec<PickerItem>,
    model_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnRequest {
    pub prompt: Vec<String>,
    pub session_id: Option<String>,
    pub model: Option<SelectedModel>,
}

impl TurnRequest {
    #[must_use]
    pub(crate) fn new(
        prompt: Vec<String>,
        session_id: Option<String>,
        model: Option<SelectedModel>,
    ) -> Self {
        Self {
            prompt,
            session_id,
            model,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedModel {
    pub provider: String,
    pub model: String,
}

impl SelectedModel {
    fn from_picker_item(item: &PickerItem) -> Result<Self> {
        let Some((provider, model)) = item.value.split_once('/') else {
            anyhow::bail!("invalid model picker value {}", item.value);
        };
        Ok(Self {
            provider: provider.to_owned(),
            model: model.to_owned(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedSessionTranscript {
    label: String,
    notices: Vec<String>,
    messages: Vec<AgentMessage>,
}

impl LoadedSessionTranscript {
    #[must_use]
    pub(crate) fn new(
        label: impl Into<String>,
        notices: impl IntoIterator<Item = String>,
        messages: impl IntoIterator<Item = AgentMessage>,
    ) -> Self {
        Self {
            label: label.into(),
            notices: notices.into_iter().collect(),
            messages: messages.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForkedSessionTranscript {
    session_id: String,
    transcript: LoadedSessionTranscript,
}

impl ForkedSessionTranscript {
    #[must_use]
    pub(crate) fn new(session_id: impl Into<String>, transcript: LoadedSessionTranscript) -> Self {
        Self {
            session_id: session_id.into(),
            transcript,
        }
    }
}

impl<RunTurn, Fut>
    InteractiveController<
        RunTurn,
        fn(String) -> Ready<Result<LoadedSessionTranscript>>,
        fn(String) -> Ready<Result<ForkedSessionTranscript>>,
    >
where
    RunTurn: Fn(TurnRequest) -> Fut,
    Fut: Future<Output = Result<Vec<AgentEvent>>> + Send,
{
    #[allow(dead_code)]
    pub fn new(
        title: impl Into<String>,
        session_label: impl Into<String>,
        model_label: impl Into<String>,
        run_turn: RunTurn,
    ) -> Self {
        Self::new_with_session_forker(
            title,
            session_label,
            model_label,
            run_turn,
            PickerCatalogs::default(),
            empty_session_loader,
            empty_session_forker,
        )
    }
}

impl<RunTurn, Fut, LoadSession, LoadFut>
    InteractiveController<
        RunTurn,
        LoadSession,
        fn(String) -> Ready<Result<ForkedSessionTranscript>>,
    >
where
    RunTurn: Fn(TurnRequest) -> Fut,
    Fut: Future<Output = Result<Vec<AgentEvent>>> + Send,
    LoadSession: Fn(String) -> LoadFut,
    LoadFut: Future<Output = Result<LoadedSessionTranscript>> + Send,
{
    #[allow(dead_code)]
    pub fn new_with_sessions(
        title: impl Into<String>,
        session_label: impl Into<String>,
        model_label: impl Into<String>,
        run_turn: RunTurn,
        catalogs: PickerCatalogs,
        load_session: LoadSession,
    ) -> Self {
        Self::new_with_session_forker(
            title,
            session_label,
            model_label,
            run_turn,
            catalogs,
            load_session,
            empty_session_forker,
        )
    }
}

impl<RunTurn, Fut, LoadSession, LoadFut, ForkSession, ForkFut>
    InteractiveController<RunTurn, LoadSession, ForkSession>
where
    RunTurn: Fn(TurnRequest) -> Fut,
    Fut: Future<Output = Result<Vec<AgentEvent>>> + Send,
    LoadSession: Fn(String) -> LoadFut,
    LoadFut: Future<Output = Result<LoadedSessionTranscript>> + Send,
    ForkSession: Fn(String) -> ForkFut,
    ForkFut: Future<Output = Result<ForkedSessionTranscript>> + Send,
{
    pub fn new_with_session_forker(
        title: impl Into<String>,
        session_label: impl Into<String>,
        model_label: impl Into<String>,
        run_turn: RunTurn,
        catalogs: PickerCatalogs,
        load_session: LoadSession,
        fork_session: ForkSession,
    ) -> Self {
        Self {
            app: NeoTuiApp::new(title, session_label, model_label),
            keybindings: KeybindingsManager::default(),
            run_turn,
            session_items: catalogs.session_items,
            session_list_error: catalogs.session_error,
            model_items: catalogs.model_items,
            model_list_error: catalogs.model_error,
            load_session,
            fork_session,
            active_session_id: None,
            active_model: None,
        }
    }

    #[allow(dead_code)]
    pub fn type_text(&mut self, text: &str) {
        self.app.prompt_mut().apply_edit(PromptEdit::Insert(text));
    }

    pub fn submit_empty_prompt(&mut self) -> Option<String> {
        self.app.submit_prompt()
    }

    #[allow(dead_code)]
    pub async fn submit_prompt(&mut self) -> Result<String> {
        self.submit_current_prompt().await?;
        Ok(self.render_snapshot())
    }

    pub async fn run_terminal_loop(
        &mut self,
        mut render: impl FnMut(&NeoTuiApp) -> Result<()>,
        mut events: impl TerminalEvents,
    ) -> Result<()> {
        render(&self.app)?;
        loop {
            let event = events.next_input_event()?;
            if self.handle_input_event(event).await? {
                break;
            }
            render(&self.app)?;
        }
        Ok(())
    }

    async fn handle_input_event(&mut self, event: InputEvent) -> Result<bool> {
        match event {
            InputEvent::Insert(character) => {
                self.app
                    .prompt_mut()
                    .apply_edit(PromptEdit::Insert(&character.to_string()));
            }
            InputEvent::Key(key) => return self.handle_keybinding_key(&key).await,
            InputEvent::Action(action) => return self.handle_keybinding_action(action).await,
            InputEvent::Backspace => {
                self.app.prompt_mut().apply_edit(PromptEdit::Backspace);
            }
            InputEvent::Delete => {
                self.app.prompt_mut().apply_edit(PromptEdit::Delete);
            }
            InputEvent::MoveLeft => {
                self.app.prompt_mut().apply_edit(PromptEdit::MoveLeft);
            }
            InputEvent::MoveRight => {
                self.app.prompt_mut().apply_edit(PromptEdit::MoveRight);
            }
            InputEvent::MoveHome => {
                self.app.prompt_mut().apply_edit(PromptEdit::MoveHome);
            }
            InputEvent::MoveEnd => {
                self.app.prompt_mut().apply_edit(PromptEdit::MoveEnd);
            }
            InputEvent::NewLine => {
                self.app.prompt_mut().apply_edit(PromptEdit::Insert("\n"));
            }
            InputEvent::Submit => {
                self.submit_current_prompt().await?;
            }
            InputEvent::Resize { .. } => {}
            InputEvent::Cancel | InputEvent::Interrupt => return Ok(true),
        }

        Ok(false)
    }

    async fn handle_keybinding_key(&mut self, key: &KeyId) -> Result<bool> {
        let actions = self.keybindings.matching_actions(key);
        let priority = if self.app.focused_overlay_id().is_some() {
            OVERLAY_ACTION_PRIORITY
        } else {
            EDITING_ACTION_PRIORITY
        };

        for action in priority {
            if actions.contains(action) {
                return self.handle_keybinding_action(*action).await;
            }
        }

        Ok(false)
    }

    async fn handle_keybinding_action(&mut self, action: KeybindingAction) -> Result<bool> {
        if self.handle_prompt_keybinding_action(action) {
            return Ok(false);
        }

        match action {
            KeybindingAction::InputNewLine => {
                self.app.prompt_mut().apply_edit(PromptEdit::Insert("\n"));
            }
            KeybindingAction::InputTab => {
                self.app.prompt_mut().apply_edit(PromptEdit::Insert("\t"));
            }
            KeybindingAction::InputCopy => {
                let _ = self.app.copy_prompt_text();
            }
            KeybindingAction::SessionPickerOpen => {
                self.open_session_picker();
            }
            KeybindingAction::SessionFork => {
                if self.app.selected_session().is_some() {
                    self.fork_selected_session().await?;
                }
            }
            KeybindingAction::ModelPickerOpen => {
                self.open_model_picker();
            }
            KeybindingAction::InputSubmit => {
                self.submit_current_prompt().await?;
            }
            KeybindingAction::SelectUp => {
                self.app.move_overlay_selection_up();
            }
            KeybindingAction::SelectDown => {
                self.app.move_overlay_selection_down();
            }
            KeybindingAction::SelectPageUp => {
                self.app.move_overlay_selection_page_up();
            }
            KeybindingAction::SelectPageDown => {
                self.app.move_overlay_selection_page_down();
            }
            KeybindingAction::SelectConfirm => {
                if self.app.approval_choice().is_some() {
                    let _ = self.app.confirm_approval();
                } else if self.app.selected_session().is_some() {
                    self.load_selected_session().await?;
                } else if self.app.selected_model().is_some() {
                    self.apply_selected_model()?;
                } else if self.app.focused_overlay_id().is_none() {
                    self.submit_current_prompt().await?;
                }
            }
            KeybindingAction::SelectCancel => {
                if self.app.focused_overlay_id().is_some() {
                    let _ = self.app.close_focused_overlay();
                } else {
                    return Ok(true);
                }
            }
            KeybindingAction::EditorCursorUp => {
                self.app.scroll_transcript_up(1);
            }
            KeybindingAction::EditorCursorDown => {
                self.app.scroll_transcript_down(1);
            }
            KeybindingAction::EditorPageUp => {
                self.app.scroll_transcript_up(8);
            }
            KeybindingAction::EditorPageDown => {
                self.app.scroll_transcript_down(8);
            }
            KeybindingAction::EditorCursorLeft
            | KeybindingAction::EditorCursorRight
            | KeybindingAction::EditorCursorWordLeft
            | KeybindingAction::EditorCursorWordRight
            | KeybindingAction::EditorCursorLineStart
            | KeybindingAction::EditorCursorLineEnd
            | KeybindingAction::EditorDeleteCharBackward
            | KeybindingAction::EditorDeleteCharForward
            | KeybindingAction::EditorDeleteWordBackward
            | KeybindingAction::EditorDeleteWordForward
            | KeybindingAction::EditorDeleteToLineStart
            | KeybindingAction::EditorDeleteToLineEnd
            | KeybindingAction::EditorYank
            | KeybindingAction::EditorUndo => {
                unreachable!("prompt edit actions are handled before overlay actions")
            }
        }

        Ok(false)
    }

    fn handle_prompt_keybinding_action(&mut self, action: KeybindingAction) -> bool {
        let edit = match action {
            KeybindingAction::EditorCursorLeft => PromptEdit::MoveLeft,
            KeybindingAction::EditorCursorRight => PromptEdit::MoveRight,
            KeybindingAction::EditorCursorWordLeft => PromptEdit::MoveWordLeft,
            KeybindingAction::EditorCursorWordRight => PromptEdit::MoveWordRight,
            KeybindingAction::EditorCursorLineStart => PromptEdit::MoveHome,
            KeybindingAction::EditorCursorLineEnd => PromptEdit::MoveEnd,
            KeybindingAction::EditorDeleteCharBackward => PromptEdit::Backspace,
            KeybindingAction::EditorDeleteCharForward => PromptEdit::Delete,
            KeybindingAction::EditorDeleteWordBackward => PromptEdit::DeleteWordBackward,
            KeybindingAction::EditorDeleteWordForward => PromptEdit::DeleteWordForward,
            KeybindingAction::EditorDeleteToLineStart => PromptEdit::DeleteToLineStart,
            KeybindingAction::EditorDeleteToLineEnd => PromptEdit::DeleteToLineEnd,
            KeybindingAction::EditorYank => PromptEdit::Yank,
            KeybindingAction::EditorUndo => PromptEdit::Undo,
            _ => return false,
        };
        self.app.prompt_mut().apply_edit(edit);
        true
    }

    async fn submit_current_prompt(&mut self) -> Result<()> {
        let Some(prompt) = self.app.submit_prompt() else {
            return Ok(());
        };
        let events = (self.run_turn)(TurnRequest::new(
            vec![prompt],
            self.active_session_id.clone(),
            self.active_model.clone(),
        ))
        .await?;
        for event in events {
            self.app.apply_agent_event(event);
        }
        Ok(())
    }

    fn open_session_picker(&mut self) {
        if let Some(error) = &self.session_list_error {
            self.app.apply_stream_update(neo_tui::StreamUpdate::Notice {
                text: format!("Error loading sessions: {error}"),
            });
            return;
        }
        if self.session_items.is_empty() {
            self.app.apply_stream_update(neo_tui::StreamUpdate::Notice {
                text: "No local sessions".to_owned(),
            });
            return;
        }
        self.app.open_session_picker(self.session_items.clone());
    }

    fn open_model_picker(&mut self) {
        if let Some(error) = &self.model_list_error {
            self.app.apply_stream_update(neo_tui::StreamUpdate::Notice {
                text: format!("Error loading models: {error}"),
            });
            return;
        }
        if self.model_items.is_empty() {
            self.app.apply_stream_update(neo_tui::StreamUpdate::Notice {
                text: "No configured models".to_owned(),
            });
            return;
        }
        self.app.open_model_picker(self.model_items.clone());
    }

    async fn load_selected_session(&mut self) -> Result<()> {
        let Some(session) = self.app.confirm_session_picker() else {
            return Ok(());
        };
        let loaded = (self.load_session)(session.value.clone())
            .await
            .with_context(|| format!("failed to load session {}", session.value))?;
        self.app
            .load_session_transcript(loaded.label, loaded.notices, loaded.messages);
        self.active_session_id = Some(session.value);
        Ok(())
    }

    async fn fork_selected_session(&mut self) -> Result<()> {
        let Some(parent) = self.app.confirm_session_picker() else {
            return Ok(());
        };
        let forked = (self.fork_session)(parent.value.clone())
            .await
            .with_context(|| format!("failed to fork session {}", parent.value))?;
        self.app.load_session_transcript(
            forked.transcript.label,
            forked.transcript.notices,
            forked.transcript.messages,
        );
        self.active_session_id = Some(forked.session_id);
        Ok(())
    }

    fn apply_selected_model(&mut self) -> Result<()> {
        let Some(model) = self.app.confirm_model_picker() else {
            return Ok(());
        };
        let selected = SelectedModel::from_picker_item(&model)?;
        self.app.set_model_label(model.label);
        self.active_model = Some(selected);
        Ok(())
    }

    #[allow(dead_code)]
    #[must_use]
    pub const fn app(&self) -> &NeoTuiApp {
        &self.app
    }

    #[must_use]
    pub fn render_snapshot(&self) -> String {
        render_terminal_fallback(&self.app)
    }
}

pub trait TerminalEvents {
    fn next_input_event(&mut self) -> Result<InputEvent>;
}

struct CrosstermEvents;

impl TerminalEvents for CrosstermEvents {
    fn next_input_event(&mut self) -> Result<InputEvent> {
        let keybindings = KeybindingsManager::default();
        loop {
            if event::poll(Duration::from_millis(250))? {
                let event = event::read()?;
                if let Some(input) =
                    InputEvent::from_crossterm_event_with_keybindings(&event, &keybindings)
                {
                    return Ok(input);
                }
            }
        }
    }
}

const EDITING_ACTION_PRIORITY: &[KeybindingAction] = &[
    KeybindingAction::InputSubmit,
    KeybindingAction::InputNewLine,
    KeybindingAction::SessionPickerOpen,
    KeybindingAction::ModelPickerOpen,
    KeybindingAction::EditorCursorLeft,
    KeybindingAction::EditorCursorRight,
    KeybindingAction::EditorCursorWordLeft,
    KeybindingAction::EditorCursorWordRight,
    KeybindingAction::EditorCursorLineStart,
    KeybindingAction::EditorCursorLineEnd,
    KeybindingAction::EditorDeleteCharBackward,
    KeybindingAction::EditorDeleteCharForward,
    KeybindingAction::EditorDeleteWordBackward,
    KeybindingAction::EditorDeleteWordForward,
    KeybindingAction::EditorDeleteToLineStart,
    KeybindingAction::EditorDeleteToLineEnd,
    KeybindingAction::EditorYank,
    KeybindingAction::EditorUndo,
    KeybindingAction::InputTab,
    KeybindingAction::SelectCancel,
];

const OVERLAY_ACTION_PRIORITY: &[KeybindingAction] = &[
    KeybindingAction::SelectConfirm,
    KeybindingAction::SelectCancel,
    KeybindingAction::SessionFork,
    KeybindingAction::SelectUp,
    KeybindingAction::SelectDown,
    KeybindingAction::SelectPageUp,
    KeybindingAction::SelectPageDown,
];

struct RawTerminal {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    raw_mode: RawModeGuard,
}

impl RawTerminal {
    fn enter() -> Result<Self> {
        let raw_mode = RawModeGuard::enable()?;
        let mut output = stdout();
        execute!(output, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(output);
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;
        Ok(Self { terminal, raw_mode })
    }

    fn draw(&mut self, app: &NeoTuiApp) -> Result<()> {
        self.terminal
            .draw(|frame| frame.render_widget(app, frame.area()))?;
        Ok(())
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
        self.raw_mode.disable();
    }
}

struct RawModeGuard {
    active: bool,
}

impl RawModeGuard {
    fn enable() -> Result<Self> {
        enable_raw_mode()?;
        Ok(Self { active: true })
    }

    fn disable(&mut self) {
        if self.active {
            let _ = disable_raw_mode();
            self.active = false;
        }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        self.disable();
    }
}

pub fn controller_for_config<'a>(
    config: &'a AppConfig,
) -> InteractiveController<
    impl Fn(TurnRequest) -> BoxedTurnFuture<'a> + 'a,
    impl Fn(String) -> BoxedSessionFuture<'a> + 'a,
    impl Fn(String) -> BoxedForkFuture<'a> + 'a,
> {
    let catalogs = picker_catalogs_for_config(config);
    InteractiveController::new_with_session_forker(
        "neo",
        "new",
        format!("{}/{}", config.default_provider, config.default_model),
        move |request| {
            let future: BoxedTurnFuture<'a> = Box::pin(async move {
                let mut effective_config = config.clone();
                if let Some(model) = request.model {
                    effective_config.default_provider = model.provider;
                    effective_config.default_model = model.model;
                }
                let turn = if let Some(session_id) = request.session_id {
                    crate::modes::run::run_prompt_in_session(
                        &session_id,
                        &request.prompt,
                        &effective_config,
                    )
                    .await?
                } else {
                    crate::modes::run::run_prompt(&request.prompt, &effective_config).await?
                };
                Ok(turn.events)
            });
            future
        },
        catalogs,
        move |session_id| {
            let future: BoxedSessionFuture<'a> =
                Box::pin(async move { load_session_transcript(session_id, config).await });
            future
        },
        move |session_id| {
            let future: BoxedForkFuture<'a> =
                Box::pin(async move { fork_session_transcript(session_id, config).await });
            future
        },
    )
}

#[allow(dead_code)]
fn empty_session_loader(session_id: String) -> Ready<Result<LoadedSessionTranscript>> {
    ready(Ok(LoadedSessionTranscript::new(
        session_id,
        Vec::new(),
        Vec::new(),
    )))
}

#[allow(dead_code)]
fn empty_session_forker(session_id: String) -> Ready<Result<ForkedSessionTranscript>> {
    ready(Ok(ForkedSessionTranscript::new(
        session_id.clone(),
        LoadedSessionTranscript::new(session_id, Vec::new(), Vec::new()),
    )))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionCatalog {
    items: Vec<PickerItem>,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelCatalog {
    items: Vec<PickerItem>,
    error: Option<String>,
}

fn picker_catalogs_for_config(config: &AppConfig) -> PickerCatalogs {
    let sessions = session_catalog_for_config(config);
    let models = model_catalog_for_config(config);
    PickerCatalogs {
        session_items: sessions.items,
        session_error: sessions.error,
        model_items: models.items,
        model_error: models.error,
    }
}

fn session_catalog_for_config(config: &AppConfig) -> SessionCatalog {
    match SessionMetadataStore::new(&config.sessions_dir).list() {
        Ok(records) => SessionCatalog {
            items: crate::session_commands::tree_order_sessions(&records)
                .into_iter()
                .map(session_record_to_picker_item)
                .collect(),
            error: None,
        },
        Err(error) => SessionCatalog {
            items: Vec::new(),
            error: Some(error.to_string()),
        },
    }
}

fn model_catalog_for_config(config: &AppConfig) -> ModelCatalog {
    match crate::modes::run::model_registry_for_config(config) {
        Ok(registry) => ModelCatalog {
            items: registry.list().iter().map(model_to_picker_item).collect(),
            error: None,
        },
        Err(error) => ModelCatalog {
            items: Vec::new(),
            error: Some(error.to_string()),
        },
    }
}

fn model_to_picker_item(model: &neo_ai::ModelSpec) -> PickerItem {
    let value = format!("{}/{}", model.provider.0, model.model);
    PickerItem::new(value.clone(), value, Some(format!("{:?}", model.api)))
}

fn session_record_to_picker_item(
    tree_record: crate::session_commands::SessionTreeRecord,
) -> PickerItem {
    let record = tree_record.record;
    let label = record.name.clone().unwrap_or_else(|| record.id.clone());
    let label = format!("{}{}", "  ".repeat(tree_record.depth), label);
    let mut details = vec![record.id.clone()];
    if let Some(parent_id) = &record.parent_id {
        details.push(format!("parent={parent_id}"));
    }
    if let Some(summary) = &record.summary {
        details.push(summary.clone());
    }
    if !record.children.is_empty() {
        details.push(format!("children={}", record.children.join(",")));
    }
    PickerItem::new(record.id, label, Some(details.join(" | ")))
}

async fn load_session_transcript(
    session_id: String,
    config: &AppConfig,
) -> Result<LoadedSessionTranscript> {
    let path = crate::session_commands::session_path(&session_id, config)?;
    let context = JsonlSessionReader::replay_context(&path)
        .await
        .with_context(|| format!("failed to replay session {}", path.display()))?;
    let mut notices = Vec::new();
    if let Some(summary) = context.compaction_summary() {
        notices.push(format!("compaction: {}", summary.summary));
    }
    if let Some(summary) = SessionMetadataStore::new(&config.sessions_dir)
        .list()
        .ok()
        .and_then(|sessions| {
            sessions
                .into_iter()
                .find(|session| session.id == session_id)
                .and_then(|session| session.summary)
        })
    {
        notices.push(format!("branch summary: {summary}"));
    }
    Ok(LoadedSessionTranscript::new(
        session_id,
        notices,
        context.messages().to_vec(),
    ))
}

async fn fork_session_transcript(
    parent_id: String,
    config: &AppConfig,
) -> Result<ForkedSessionTranscript> {
    let session = SessionMetadataStore::new(&config.sessions_dir)
        .fork(&parent_id, None)
        .with_context(|| format!("failed to create local fork for session {parent_id}"))?;
    let child_id = session.id;
    let mut loaded = load_session_transcript(child_id.clone(), config).await?;
    loaded.notices.insert(0, format!("forked from {parent_id}"));
    Ok(ForkedSessionTranscript::new(child_id, loaded))
}

fn render_terminal_fallback(app: &NeoTuiApp) -> String {
    let width = 80;
    let height = 24;
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend is valid");
    terminal
        .draw(|frame| frame.render_widget(app, frame.area()))
        .expect("fallback app render succeeds");

    let lines = terminal
        .backend()
        .buffer()
        .content
        .chunks(width as usize)
        .map(|line| {
            line.iter()
                .map(Cell::symbol)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>();
    format!("{}\n", lines.join("\n").trim_end())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
    };

    use neo_agent_core::{AgentEvent, AgentMessage, Content, PermissionPolicy, StopReason};
    use neo_tui::{KeybindingAction, OverlayKind};

    use super::*;
    use crate::config::{Defaults, McpConfig, RuntimeConfig};

    #[tokio::test]
    async fn controller_submits_prompt_reduces_turn_events_and_renders_snapshot() {
        let mut controller = InteractiveController::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            |request| async move {
                assert_eq!(request.prompt, vec!["hello neo".to_owned()]);
                assert_eq!(request.session_id, None);
                assert_eq!(request.model, None);
                Ok(vec![
                    AgentEvent::MessageStarted {
                        turn: 1,
                        id: "assistant-1".to_owned(),
                    },
                    AgentEvent::TextDelta {
                        turn: 1,
                        text: "Hello".to_owned(),
                    },
                    AgentEvent::TextDelta {
                        turn: 1,
                        text: ", Neo".to_owned(),
                    },
                    AgentEvent::TurnFinished {
                        turn: 1,
                        stop_reason: StopReason::EndTurn,
                    },
                ])
            },
        );

        controller.type_text("hello neo");
        let snapshot = controller.submit_prompt().await.expect("turn succeeds");

        assert!(snapshot.contains("neo | session: test-session | model: openai/gpt-4.1 | Editing"));
        assert!(snapshot.contains("You"));
        assert!(snapshot.contains("hello neo"));
        assert!(snapshot.contains("Assistant"));
        assert!(snapshot.contains("Hello, Neo"));
        assert_eq!(controller.app().mode(), neo_tui::AppMode::Editing);
    }

    #[tokio::test]
    async fn event_loop_types_submits_renders_and_exits_without_a_real_terminal() {
        struct FakeEvents {
            events: std::vec::IntoIter<InputEvent>,
        }

        impl TerminalEvents for FakeEvents {
            fn next_input_event(&mut self) -> Result<InputEvent> {
                self.events
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("expected test event"))
            }
        }

        let mut rendered = Vec::new();
        let mut controller = InteractiveController::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            |request| async move {
                assert_eq!(request.prompt, vec!["hi".to_owned()]);
                assert_eq!(request.session_id, None);
                assert_eq!(request.model, None);
                Ok(vec![
                    AgentEvent::MessageStarted {
                        turn: 1,
                        id: "assistant-1".to_owned(),
                    },
                    AgentEvent::TextDelta {
                        turn: 1,
                        text: "hello from controller".to_owned(),
                    },
                    AgentEvent::TurnFinished {
                        turn: 1,
                        stop_reason: StopReason::EndTurn,
                    },
                ])
            },
        );

        controller
            .run_terminal_loop(
                |app| {
                    rendered.push(render_terminal_fallback(app));
                    Ok(())
                },
                FakeEvents {
                    events: vec![
                        InputEvent::Insert('h'),
                        InputEvent::Insert('i'),
                        InputEvent::Submit,
                        InputEvent::Cancel,
                    ]
                    .into_iter(),
                },
            )
            .await
            .expect("event loop succeeds");

        assert_eq!(controller.app().mode(), neo_tui::AppMode::Editing);
        assert!(rendered.iter().any(|snapshot| snapshot.contains("> hi")));
        assert!(
            rendered
                .last()
                .expect("final render")
                .contains("hello from controller")
        );
    }

    #[tokio::test]
    async fn event_loop_renders_after_terminal_resize_without_submitting_prompt() {
        struct FakeEvents {
            events: std::vec::IntoIter<InputEvent>,
        }

        impl TerminalEvents for FakeEvents {
            fn next_input_event(&mut self) -> Result<InputEvent> {
                self.events
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("expected test event"))
            }
        }

        let mut rendered = Vec::new();
        let mut controller = InteractiveController::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            |_request| async move {
                panic!("resize should not submit a turn");
                #[allow(unreachable_code)]
                Ok(Vec::<AgentEvent>::new())
            },
        );

        controller
            .run_terminal_loop(
                |app| {
                    rendered.push(render_terminal_fallback(app));
                    Ok(())
                },
                FakeEvents {
                    events: vec![
                        InputEvent::Insert('h'),
                        InputEvent::Resize {
                            columns: 100,
                            rows: 30,
                        },
                        InputEvent::Cancel,
                    ]
                    .into_iter(),
                },
            )
            .await
            .expect("event loop succeeds");

        assert_eq!(rendered.len(), 3);
        assert!(rendered[1].contains("> h"));
        assert_eq!(controller.app().mode(), neo_tui::AppMode::Editing);
    }

    #[tokio::test]
    async fn event_loop_dispatches_editor_keybinding_actions_to_prompt_edits() {
        struct FakeEvents {
            events: std::vec::IntoIter<InputEvent>,
        }

        impl TerminalEvents for FakeEvents {
            fn next_input_event(&mut self) -> Result<InputEvent> {
                self.events
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("expected test event"))
            }
        }

        let mut controller = InteractiveController::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );

        for character in "hello brave world".chars() {
            controller
                .handle_input_event(InputEvent::Insert(character))
                .await
                .expect("insert succeeds");
        }

        controller
            .run_terminal_loop(
                |_app| Ok(()),
                FakeEvents {
                    events: vec![
                        InputEvent::Action(KeybindingAction::InputCopy),
                        InputEvent::Action(KeybindingAction::EditorCursorWordLeft),
                        InputEvent::Action(KeybindingAction::EditorDeleteWordBackward),
                        InputEvent::Action(KeybindingAction::EditorDeleteToLineEnd),
                        InputEvent::Action(KeybindingAction::EditorYank),
                        InputEvent::Action(KeybindingAction::EditorUndo),
                        InputEvent::Action(KeybindingAction::EditorUndo),
                        InputEvent::Action(KeybindingAction::InputTab),
                        InputEvent::Action(KeybindingAction::SelectCancel),
                    ]
                    .into_iter(),
                },
            )
            .await
            .expect("event loop succeeds");

        assert_eq!(controller.app().copy_buffer(), Some("hello brave world"));
        assert_eq!(controller.app().prompt().text, "hello \tworld");
        assert_eq!(controller.app().prompt().cursor, 7);
    }

    #[tokio::test]
    async fn event_loop_dispatches_editor_scroll_actions_to_transcript_view() {
        let mut controller = InteractiveController::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        for index in 0..10 {
            controller
                .app
                .transcript_mut()
                .push(neo_tui::TranscriptItem::notice(format!("line {index}")));
        }

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::EditorPageUp))
            .await
            .expect("page up scrolls transcript");
        assert_eq!(controller.app().transcript_view().scrollback(), 8);

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::EditorCursorDown))
            .await
            .expect("cursor down scrolls transcript toward bottom");
        assert_eq!(controller.app().transcript_view().scrollback(), 7);

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::EditorPageDown))
            .await
            .expect("page down returns transcript to bottom");
        assert_eq!(controller.app().transcript_view().scrollback(), 0);
    }

    #[tokio::test]
    async fn event_loop_dispatches_select_keybinding_actions_to_overlay_primitives() {
        struct FakeEvents {
            events: std::vec::IntoIter<InputEvent>,
        }

        impl TerminalEvents for FakeEvents {
            fn next_input_event(&mut self) -> Result<InputEvent> {
                self.events
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("expected test event"))
            }
        }

        let mut controller = InteractiveController::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller
            .app
            .request_approval("approval-1", "Run command?", "cargo test");

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("selection moves down");
        assert_eq!(
            controller.app().approval_choice(),
            Some(neo_tui::ApprovalChoice::Deny)
        );

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectUp))
            .await
            .expect("selection moves up");
        assert_eq!(
            controller.app().approval_choice(),
            Some(neo_tui::ApprovalChoice::Approve)
        );

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("approval confirms");
        assert!(controller.app().focused_overlay().is_none());

        controller.app.push_overlay(neo_tui::Overlay::new(
            "palette",
            OverlayKind::CommandPalette(neo_tui::CommandPaletteState::new((0..10).map(|index| {
                neo_tui::CommandSpec::new(
                    format!("command-{index}"),
                    format!("Command {index}"),
                    None::<String>,
                )
            }))),
        ));
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectPageDown))
            .await
            .expect("selection pages down");
        let Some(OverlayKind::CommandPalette(palette)) = controller
            .app()
            .focused_overlay()
            .map(|overlay| &overlay.kind)
        else {
            panic!("expected command palette overlay");
        };
        assert_eq!(palette.selected_command().expect("command").id, "command-8");

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectPageUp))
            .await
            .expect("selection pages up");
        let Some(OverlayKind::CommandPalette(palette)) = controller
            .app()
            .focused_overlay()
            .map(|overlay| &overlay.kind)
        else {
            panic!("expected command palette overlay");
        };
        assert_eq!(palette.selected_command().expect("command").id, "command-0");
        let _ = controller.app.close_focused_overlay();

        controller.app.push_overlay(neo_tui::Overlay::new(
            "custom",
            OverlayKind::Message("Body".to_owned()),
        ));
        controller
            .run_terminal_loop(
                |_app| Ok(()),
                FakeEvents {
                    events: vec![
                        InputEvent::Action(KeybindingAction::SelectCancel),
                        InputEvent::Action(KeybindingAction::SelectCancel),
                    ]
                    .into_iter(),
                },
            )
            .await
            .expect("event loop exits after canceling overlay and receiving cancel again");

        assert!(controller.app().focused_overlay().is_none());
    }

    #[tokio::test]
    async fn event_loop_opens_session_picker_and_continues_selected_transcript() {
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_requests = std::sync::Arc::clone(&requests);
        let mut controller = InteractiveController::new_with_sessions(
            "neo",
            "new",
            "openai/gpt-4.1",
            move |request| {
                let captured_requests = std::sync::Arc::clone(&captured_requests);
                async move {
                    captured_requests
                        .lock()
                        .expect("record request")
                        .push(request);
                    Ok(vec![
                        AgentEvent::MessageStarted {
                            turn: 2,
                            id: "assistant-2".to_owned(),
                        },
                        AgentEvent::TextDelta {
                            turn: 2,
                            text: "continued".to_owned(),
                        },
                        AgentEvent::TurnFinished {
                            turn: 2,
                            stop_reason: StopReason::EndTurn,
                        },
                    ])
                }
            },
            PickerCatalogs {
                session_items: vec![PickerItem::new(
                    "alpha",
                    "Alpha session",
                    Some("branch summary"),
                )],
                session_error: None,
                model_items: Vec::new(),
                model_error: None,
            },
            |session_id| async move {
                assert_eq!(session_id, "alpha");
                Ok(LoadedSessionTranscript::new(
                    "alpha",
                    ["branch summary: Local branch summary".to_owned()],
                    [
                        AgentMessage::user_text("hello"),
                        AgentMessage::assistant(
                            [Content::text("hi back")],
                            Vec::new(),
                            StopReason::EndTurn,
                        ),
                    ],
                ))
            },
        );

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SessionPickerOpen))
            .await
            .expect("session picker opens");
        assert!(matches!(
            controller
                .app()
                .focused_overlay()
                .map(|overlay| &overlay.kind),
            Some(OverlayKind::SessionPicker(_))
        ));

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("session loads");

        assert_eq!(controller.app().session_label(), "alpha");
        assert!(controller.app().focused_overlay().is_none());
        assert!(matches!(
            &controller.app().transcript().items()[0],
            neo_tui::TranscriptItem::Notice { content }
                if content == "branch summary: Local branch summary"
        ));
        assert!(matches!(
            &controller.app().transcript().items()[1],
            neo_tui::TranscriptItem::User { content } if content == "hello"
        ));
        assert!(matches!(
            &controller.app().transcript().items()[2],
            neo_tui::TranscriptItem::Assistant { content } if content == "hi back"
        ));

        controller.type_text("continue");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("continued prompt submits");
        let requests = requests.lock().expect("recorded requests");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].prompt, vec!["continue".to_owned()]);
        assert_eq!(requests[0].session_id.as_deref(), Some("alpha"));
        assert_eq!(requests[0].model, None);
        assert!(controller.app().transcript().items().iter().any(|item| {
            matches!(item, neo_tui::TranscriptItem::Assistant { content } if content == "continued")
        }));
    }

    #[tokio::test]
    async fn event_loop_forks_selected_session_and_continues_child_session() {
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_requests = std::sync::Arc::clone(&requests);
        let mut controller = InteractiveController::new_with_session_forker(
            "neo",
            "new",
            "openai/gpt-4.1",
            move |request| {
                let captured_requests = std::sync::Arc::clone(&captured_requests);
                async move {
                    captured_requests
                        .lock()
                        .expect("record request")
                        .push(request);
                    Ok(vec![
                        AgentEvent::MessageStarted {
                            turn: 3,
                            id: "assistant-3".to_owned(),
                        },
                        AgentEvent::TextDelta {
                            turn: 3,
                            text: "continued on fork".to_owned(),
                        },
                        AgentEvent::TurnFinished {
                            turn: 3,
                            stop_reason: StopReason::EndTurn,
                        },
                    ])
                }
            },
            PickerCatalogs {
                session_items: vec![PickerItem::new(
                    "alpha",
                    "Alpha session",
                    Some("branch summary"),
                )],
                session_error: None,
                model_items: Vec::new(),
                model_error: None,
            },
            |_session_id| async move {
                panic!("fork action should not use the plain session loader");
                #[allow(unreachable_code)]
                Ok(LoadedSessionTranscript::new("", Vec::new(), Vec::new()))
            },
            |parent_id| async move {
                assert_eq!(parent_id, "alpha");
                Ok(ForkedSessionTranscript::new(
                    "alpha-fork-1",
                    LoadedSessionTranscript::new(
                        "alpha-fork-1",
                        ["forked from alpha".to_owned()],
                        [
                            AgentMessage::user_text("hello"),
                            AgentMessage::assistant(
                                [Content::text("hi back")],
                                Vec::new(),
                                StopReason::EndTurn,
                            ),
                        ],
                    ),
                ))
            },
        );

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SessionPickerOpen))
            .await
            .expect("session picker opens");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SessionFork))
            .await
            .expect("session fork loads child transcript");

        assert_eq!(controller.app().session_label(), "alpha-fork-1");
        assert!(controller.app().focused_overlay().is_none());
        assert!(matches!(
            &controller.app().transcript().items()[0],
            neo_tui::TranscriptItem::Notice { content } if content == "forked from alpha"
        ));
        assert!(matches!(
            &controller.app().transcript().items()[1],
            neo_tui::TranscriptItem::User { content } if content == "hello"
        ));

        controller.type_text("continue fork");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("continued prompt submits on fork");
        let requests = requests.lock().expect("recorded requests");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].prompt, vec!["continue fork".to_owned()]);
        assert_eq!(requests[0].session_id.as_deref(), Some("alpha-fork-1"));
        assert_eq!(requests[0].model, None);
    }

    #[tokio::test]
    async fn event_loop_opens_model_picker_and_submits_with_selected_model() {
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_requests = std::sync::Arc::clone(&requests);
        let mut controller = InteractiveController::new_with_sessions(
            "neo",
            "new",
            "openai/gpt-4.1",
            move |request| {
                let captured_requests = std::sync::Arc::clone(&captured_requests);
                async move {
                    captured_requests
                        .lock()
                        .expect("record request")
                        .push(request);
                    Ok(vec![
                        AgentEvent::MessageStarted {
                            turn: 1,
                            id: "assistant-1".to_owned(),
                        },
                        AgentEvent::TextDelta {
                            turn: 1,
                            text: "model switched".to_owned(),
                        },
                        AgentEvent::TurnFinished {
                            turn: 1,
                            stop_reason: StopReason::EndTurn,
                        },
                    ])
                }
            },
            PickerCatalogs {
                session_items: Vec::new(),
                session_error: None,
                model_items: vec![
                    PickerItem::new("openai/gpt-4.1", "openai/gpt-4.1", Some("Responses")),
                    PickerItem::new(
                        "anthropic/claude-sonnet-4-5",
                        "anthropic/claude-sonnet-4-5",
                        Some("Messages"),
                    ),
                ],
                model_error: None,
            },
            |session_id| async move {
                Ok(LoadedSessionTranscript::new(
                    session_id,
                    Vec::new(),
                    Vec::new(),
                ))
            },
        );

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::ModelPickerOpen))
            .await
            .expect("model picker opens");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("model selection moves");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("model selection applies");

        assert_eq!(
            controller.app().model_label(),
            "anthropic/claude-sonnet-4-5"
        );
        controller.type_text("use selected model");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("turn submits with selected model");

        let requests = requests.lock().expect("recorded requests");
        assert_eq!(requests.len(), 1);
        let selected = requests[0].model.as_ref().expect("selected model");
        assert_eq!(selected.provider, "anthropic");
        assert_eq!(selected.model, "claude-sonnet-4-5");
        assert_eq!(requests[0].session_id, None);
    }

    #[tokio::test]
    async fn session_catalog_and_loader_use_real_local_session_store() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = temp.path().join(".neo/sessions");
        fs::create_dir_all(&sessions_dir).expect("create sessions dir");
        fs::write(
            sessions_dir.join("alpha.jsonl"),
            concat!(
                "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello\"}}]}}}}\n",
                "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"hi back\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
            ),
        )
        .expect("write session jsonl");

        let store = SessionMetadataStore::new(&sessions_dir);
        store
            .rename("alpha", "Alpha Session".to_owned())
            .expect("rename session");
        store
            .summarize("alpha", "Local branch summary".to_owned())
            .expect("summarize session");
        let child = store
            .fork("alpha", Some("Parser branch".to_owned()))
            .expect("fork session");

        let config = test_config(temp.path(), sessions_dir);
        let catalog = session_catalog_for_config(&config);
        assert_eq!(catalog.error, None);
        assert_eq!(catalog.items.len(), 2);
        assert_eq!(catalog.items[0].value, "alpha");
        assert_eq!(catalog.items[0].label, "Alpha Session");
        assert!(
            catalog.items[0]
                .description
                .as_deref()
                .is_some_and(|description| {
                    description.contains("alpha") && description.contains("Local branch summary")
                })
        );
        assert_eq!(catalog.items[1].value, child.id);
        assert_eq!(catalog.items[1].label, "  Parser branch");
        assert!(
            catalog.items[1]
                .description
                .as_deref()
                .is_some_and(|description| description.contains("parent=alpha"))
        );

        let loaded = load_session_transcript("alpha".to_owned(), &config)
            .await
            .expect("load session transcript");
        assert_eq!(loaded.label, "alpha");
        assert_eq!(
            loaded.notices,
            vec!["branch summary: Local branch summary".to_owned()]
        );
        assert_eq!(loaded.messages.len(), 2);
        assert!(matches!(
            &loaded.messages[0],
            AgentMessage::User { content } if content[0].as_text() == Some("hello")
        ));
        assert!(matches!(
            &loaded.messages[1],
            AgentMessage::Assistant { content, .. } if content[0].as_text() == Some("hi back")
        ));
    }

    #[tokio::test]
    async fn fork_session_transcript_copies_jsonl_metadata_and_loads_child() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = temp.path().join(".neo/sessions");
        fs::create_dir_all(&sessions_dir).expect("create sessions dir");
        fs::write(
            sessions_dir.join("alpha.jsonl"),
            concat!(
                "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello\"}}]}}}}\n",
                "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"hi back\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
            ),
        )
        .expect("write session jsonl");

        let config = test_config(temp.path(), sessions_dir.clone());
        let forked = fork_session_transcript("alpha".to_owned(), &config)
            .await
            .expect("fork session");

        assert!(forked.session_id.starts_with("alpha-fork-"));
        assert_eq!(forked.transcript.label, forked.session_id);
        assert_eq!(
            forked.transcript.notices.first().map(String::as_str),
            Some("forked from alpha")
        );
        assert_eq!(forked.transcript.messages.len(), 2);
        assert!(
            sessions_dir
                .join(format!("{}.jsonl", forked.session_id))
                .is_file()
        );

        let sessions = SessionMetadataStore::new(&sessions_dir)
            .list()
            .expect("list sessions");
        let parent = sessions
            .iter()
            .find(|session| session.id == "alpha")
            .expect("parent listed");
        assert!(parent.children.contains(&forked.session_id));
        let child = sessions
            .iter()
            .find(|session| session.id == forked.session_id)
            .expect("child listed");
        assert_eq!(child.parent_id.as_deref(), Some("alpha"));
    }

    fn test_config(project_dir: &Path, sessions_dir: PathBuf) -> AppConfig {
        AppConfig {
            default_model: "gpt-4.1".to_owned(),
            default_provider: "openai".to_owned(),
            api_base: None,
            api_key_env: None,
            providers: BTreeMap::new(),
            model_catalogs: Vec::new(),
            sessions_dir,
            permissions: PermissionPolicy::default(),
            defaults: Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: RuntimeConfig::default(),
            mcp: McpConfig::default(),
            approve: false,
            no_approve: false,
            project_dir: project_dir.to_path_buf(),
            config_path: project_dir.join(".neo/config.toml"),
        }
    }
}
