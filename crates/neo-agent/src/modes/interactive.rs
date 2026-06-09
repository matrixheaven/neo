use crate::{
    config::{self, AppConfig},
    prompt_templates::{expand_prompt_template_args, load_project_prompt_templates},
};
use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    future::{Future, Ready, ready},
    io::{IsTerminal as _, Stdout, Write as _, stdout},
    path::{Path, PathBuf},
    pin::Pin,
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use neo_agent_core::{
    AgentEvent, AgentMessage, PermissionDecision,
    session::{JsonlSessionReader, SessionMetadataStore},
};
use neo_tui::{
    ApprovalChoice, ApprovalResult, CommandSpec, InputEvent, InputParser, KeyId, KeybindingAction,
    KeybindingsManager, NeoTuiApp, PickerItem, PromptEdit,
};
use ratatui::{
    Terminal,
    backend::{CrosstermBackend, TestBackend},
    buffer::Cell,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

type BoxedTurnFuture = Pin<Box<dyn Future<Output = Result<()>> + Send + 'static>>;
type BoxedSessionFuture = Pin<Box<dyn Future<Output = Result<LoadedSessionTranscript>> + Send>>;
type BoxedForkFuture = Pin<Box<dyn Future<Output = Result<ForkedSessionTranscript>> + Send>>;
type TurnDriver = Arc<dyn Fn(TurnRequest, TurnChannels) -> BoxedTurnFuture + Send + Sync>;
type SessionLoader = Arc<dyn Fn(String) -> BoxedSessionFuture + Send + Sync>;
type SessionForker = Arc<dyn Fn(String) -> BoxedForkFuture + Send + Sync>;
type ClipboardWriter = Arc<dyn Fn(&str) -> Result<()> + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupAction {
    None,
    OpenSessionPicker,
}

pub fn execute_with_startup(config: &AppConfig, startup: StartupAction) -> String {
    let mut controller = controller_for_config(config);
    controller.apply_startup_action(startup);
    controller.render_snapshot()
}

pub async fn execute_tty_with_startup(
    config: &AppConfig,
    startup: StartupAction,
) -> Result<Option<String>> {
    if !stdout().is_terminal() {
        return Ok(Some(execute_with_startup(config, startup)));
    }

    let mut terminal = RawTerminal::enter()?;
    let mut controller = controller_for_config(config);
    controller.apply_startup_action(startup);
    let events = CrosstermEvents::new(controller.keybindings.clone());
    controller
        .run_terminal_loop(|app| terminal.draw(app), events)
        .await?;
    Ok(None)
}

pub(crate) struct InteractiveController {
    app: NeoTuiApp,
    keybindings: KeybindingsManager,
    run_turn: TurnDriver,
    session_items: Vec<PickerItem>,
    session_list_error: Option<String>,
    model_items: Vec<PickerItem>,
    model_list_error: Option<String>,
    load_session: SessionLoader,
    fork_session: SessionForker,
    active_session_id: Option<String>,
    local_config: Option<AppConfig>,
    active_model: Option<SelectedModel>,
    active_turn: Option<RunningTurn>,
    pending_approvals: BTreeMap<String, oneshot::Sender<PermissionDecision>>,
    resolved_approvals: BTreeMap<String, PermissionDecision>,
    clipboard_writer: ClipboardWriter,
    always_approve: bool,
    completion_root: PathBuf,
}

pub(crate) struct TurnChannels {
    events: mpsc::UnboundedSender<Result<AgentEvent>>,
    approvals: mpsc::UnboundedSender<crate::modes::run::PromptApprovalRequest>,
    cancel_token: CancellationToken,
}

impl TurnChannels {
    fn send_event(&self, event: AgentEvent) {
        let _ = self.events.send(Ok(event));
    }
}

struct RunningTurn {
    events: mpsc::UnboundedReceiver<Result<AgentEvent>>,
    approvals: mpsc::UnboundedReceiver<crate::modes::run::PromptApprovalRequest>,
    task: JoinHandle<Result<()>>,
    cancel_token: CancellationToken,
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

impl InteractiveController {
    #[allow(dead_code)]
    pub fn new<RunTurn, Fut>(
        title: impl Into<String>,
        session_label: impl Into<String>,
        model_label: impl Into<String>,
        run_turn: RunTurn,
    ) -> Self
    where
        RunTurn: Fn(TurnRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Vec<AgentEvent>>> + Send + 'static,
    {
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

    #[allow(dead_code)]
    pub fn new_with_sessions<RunTurn, Fut, LoadSession, LoadFut>(
        title: impl Into<String>,
        session_label: impl Into<String>,
        model_label: impl Into<String>,
        run_turn: RunTurn,
        catalogs: PickerCatalogs,
        load_session: LoadSession,
    ) -> Self
    where
        RunTurn: Fn(TurnRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Vec<AgentEvent>>> + Send + 'static,
        LoadSession: Fn(String) -> LoadFut + Send + Sync + 'static,
        LoadFut: Future<Output = Result<LoadedSessionTranscript>> + Send + 'static,
    {
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

    pub fn new_with_session_forker<RunTurn, Fut, LoadSession, LoadFut, ForkSession, ForkFut>(
        title: impl Into<String>,
        session_label: impl Into<String>,
        model_label: impl Into<String>,
        run_turn: RunTurn,
        catalogs: PickerCatalogs,
        load_session: LoadSession,
        fork_session: ForkSession,
    ) -> Self
    where
        RunTurn: Fn(TurnRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Vec<AgentEvent>>> + Send + 'static,
        LoadSession: Fn(String) -> LoadFut + Send + Sync + 'static,
        LoadFut: Future<Output = Result<LoadedSessionTranscript>> + Send + 'static,
        ForkSession: Fn(String) -> ForkFut + Send + Sync + 'static,
        ForkFut: Future<Output = Result<ForkedSessionTranscript>> + Send + 'static,
    {
        let run_turn = legacy_turn_driver(run_turn);
        let load_session: SessionLoader =
            Arc::new(move |session_id| Box::pin(load_session(session_id)));
        let fork_session: SessionForker =
            Arc::new(move |session_id| Box::pin(fork_session(session_id)));
        Self::new_with_turn_driver(
            title,
            session_label,
            model_label,
            run_turn,
            catalogs,
            load_session,
            fork_session,
        )
    }

    pub fn new_with_turn_driver(
        title: impl Into<String>,
        session_label: impl Into<String>,
        model_label: impl Into<String>,
        run_turn: TurnDriver,
        catalogs: PickerCatalogs,
        load_session: SessionLoader,
        fork_session: SessionForker,
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
            local_config: None,
            active_model: None,
            active_turn: None,
            pending_approvals: BTreeMap::new(),
            resolved_approvals: BTreeMap::new(),
            clipboard_writer: Arc::new(write_system_clipboard),
            always_approve: false,
            completion_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }

    #[allow(dead_code)]
    pub fn type_text(&mut self, text: &str) {
        self.app.prompt_mut().apply_edit(PromptEdit::Insert(text));
    }

    #[cfg(test)]
    fn set_clipboard_writer(&mut self, writer: ClipboardWriter) {
        self.clipboard_writer = writer;
    }

    pub fn apply_startup_action(&mut self, startup: StartupAction) {
        match startup {
            StartupAction::None => {}
            StartupAction::OpenSessionPicker => self.open_session_picker(),
        }
    }

    #[allow(dead_code)]
    pub async fn submit_prompt(&mut self) -> Result<String> {
        self.submit_current_prompt().await?;
        self.wait_for_active_turn().await?;
        Ok(self.render_snapshot())
    }

    pub async fn run_terminal_loop(
        &mut self,
        mut render: impl FnMut(&NeoTuiApp) -> Result<()>,
        mut events: impl TerminalEvents,
    ) -> Result<()> {
        render(&self.app)?;
        loop {
            match events.poll_input_event(Duration::from_millis(50))? {
                Some(event) => {
                    let is_interrupt = matches!(event, InputEvent::Interrupt);
                    if self.handle_input_event(event).await? {
                        let had_active_turn = self.active_turn.is_some();
                        if is_interrupt {
                            self.cancel_active_turn().await?;
                        } else {
                            self.wait_for_active_turn().await?;
                        }
                        if had_active_turn {
                            render(&self.app)?;
                        }
                        break;
                    }
                }
                None => tokio::task::yield_now().await,
            }
            self.drain_active_turn().await?;
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
            InputEvent::Paste(text) => {
                self.app.prompt_mut().apply_edit(PromptEdit::Insert(&text));
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
            if *action == KeybindingAction::TranscriptCopySelection
                && self.app.transcript_selection().is_none()
            {
                continue;
            }
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
        if self.handle_transcript_keybinding_action(action) {
            return Ok(false);
        }

        match action {
            KeybindingAction::InputNewLine => {
                self.app.prompt_mut().apply_edit(PromptEdit::Insert("\n"));
            }
            KeybindingAction::InputTab => self.complete_prompt_or_insert_tab(),
            KeybindingAction::InputCopy => self.copy_prompt_to_clipboard(),
            KeybindingAction::CommandPaletteOpen => self.open_command_palette(),
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
            KeybindingAction::SelectUp => self.app.move_overlay_selection_up(),
            KeybindingAction::SelectDown => self.app.move_overlay_selection_down(),
            KeybindingAction::SelectPageUp => self.app.move_overlay_selection_page_up(),
            KeybindingAction::SelectPageDown => self.app.move_overlay_selection_page_down(),
            KeybindingAction::SelectConfirm => {
                if self.app.selected_command().is_some() {
                    self.run_selected_command().await?;
                } else if self.app.approval_choice().is_some() {
                    if let Some(result) = self.app.confirm_approval() {
                        self.resolve_approval(&result);
                    }
                } else if self.app.selected_session().is_some() {
                    self.load_selected_session().await?;
                } else if self.app.selected_model().is_some() {
                    self.apply_selected_model()?;
                } else if self.app.selected_prompt_completion().is_some() {
                    let _ = self.app.confirm_prompt_completion();
                } else if self.app.focused_overlay_id().is_none() {
                    self.submit_current_prompt().await?;
                }
            }
            KeybindingAction::SelectCancel => {
                if self.app.focused_overlay_id().is_some() {
                    if let Some(overlay) = self.app.close_focused_overlay()
                        && let neo_tui::OverlayKind::Approval(modal) = overlay.kind
                    {
                        self.resolve_approval(&ApprovalResult {
                            request_id: modal.request_id,
                            choice: ApprovalChoice::Deny,
                        });
                    }
                } else {
                    return Ok(true);
                }
            }
            KeybindingAction::EditorCursorUp => self.app.scroll_transcript_up(1),
            KeybindingAction::EditorCursorDown => self.app.scroll_transcript_down(1),
            KeybindingAction::EditorPageUp => self.app.scroll_transcript_up(8),
            KeybindingAction::EditorPageDown => self.app.scroll_transcript_down(8),
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
            | KeybindingAction::EditorUndo
            | KeybindingAction::TranscriptSelectionStart
            | KeybindingAction::TranscriptSelectionClear
            | KeybindingAction::TranscriptSelectionExtendUp
            | KeybindingAction::TranscriptSelectionExtendDown
            | KeybindingAction::TranscriptSelectionExtendPageUp
            | KeybindingAction::TranscriptSelectionExtendPageDown
            | KeybindingAction::TranscriptCopySelection => {
                unreachable!("prompt edit actions are handled before overlay actions")
            }
        }

        Ok(false)
    }

    fn complete_prompt_or_insert_tab(&mut self) {
        let Some(prefix) = self.app.prompt().completion_prefix() else {
            self.app.prompt_mut().apply_edit(PromptEdit::Insert("\t"));
            return;
        };
        let completions =
            match prompt_completions(&self.completion_root, &prefix.text, &self.model_items) {
                Ok(completions) => completions,
                Err(error) => {
                    self.app.apply_stream_update(neo_tui::StreamUpdate::Notice {
                        text: format!("Completion error: {error}"),
                    });
                    return;
                }
            };

        if completions.is_empty() {
            self.app.prompt_mut().apply_edit(PromptEdit::Insert("\t"));
            return;
        }

        if let Some(common_prefix) = longest_common_completion_prefix(&completions)
            && common_prefix.chars().count() > prefix.text.chars().count()
        {
            let _ = self
                .app
                .prompt_mut()
                .replace_completion_prefix(&prefix, &common_prefix);
            return;
        }

        if completions.len() == 1 {
            let _ = self
                .app
                .prompt_mut()
                .replace_completion_prefix(&prefix, &completions[0].value);
            return;
        }

        self.app.open_prompt_completion_picker(prefix, completions);
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

    fn handle_transcript_keybinding_action(&mut self, action: KeybindingAction) -> bool {
        match action {
            KeybindingAction::TranscriptSelectionStart => self.app.select_visible_transcript_item(),
            KeybindingAction::TranscriptSelectionClear => self.app.clear_transcript_selection(),
            KeybindingAction::TranscriptSelectionExtendUp => {
                self.app.extend_transcript_selection_up(1);
            }
            KeybindingAction::TranscriptSelectionExtendDown => {
                self.app.extend_transcript_selection_down(1);
            }
            KeybindingAction::TranscriptSelectionExtendPageUp => {
                self.app.extend_transcript_selection_up(8);
            }
            KeybindingAction::TranscriptSelectionExtendPageDown => {
                self.app.extend_transcript_selection_down(8);
            }
            KeybindingAction::TranscriptCopySelection => {
                self.copy_transcript_selection_to_clipboard();
            }
            _ => return false,
        }
        true
    }

    fn copy_prompt_to_clipboard(&mut self) {
        let Some(copied) = self.app.copy_prompt_text() else {
            return;
        };
        self.write_clipboard_text(&copied);
    }

    fn copy_transcript_selection_to_clipboard(&mut self) {
        let Some(copied) = self.app.copy_selected_transcript_text() else {
            return;
        };
        self.write_clipboard_text(&copied);
    }

    fn write_clipboard_text(&mut self, copied: &str) {
        if let Err(error) = (self.clipboard_writer)(copied) {
            self.app.apply_stream_update(neo_tui::StreamUpdate::Notice {
                text: format!("Clipboard copy failed: {error}"),
            });
        }
    }

    fn open_command_palette(&mut self) {
        let (commands, error) = command_specs(&self.completion_root);
        if let Some(error) = error {
            self.app.apply_stream_update(neo_tui::StreamUpdate::Notice {
                text: format!("Error loading prompt templates: {error}"),
            });
        }
        self.app.open_command_palette(commands);
    }

    async fn run_selected_command(&mut self) -> Result<()> {
        let Some(command) = self.app.confirm_command_palette() else {
            return Ok(());
        };
        if let Some(name) = command.id.strip_prefix("prompt-template.") {
            self.app
                .prompt_mut()
                .apply_edit(PromptEdit::Insert(&format!("/{name} ")));
            return Ok(());
        }

        match command.id.as_str() {
            "sessions" => self.open_session_picker(),
            "models" => self.open_model_picker(),
            "copy-prompt" => self.copy_prompt_to_clipboard(),
            "select-transcript" => self.app.select_visible_transcript_item(),
            "clear-transcript-selection" => self.app.clear_transcript_selection(),
            "copy-transcript-selection" => self.copy_transcript_selection_to_clipboard(),
            "session.exportHtml" => self.export_active_session_to_html().await?,
            "submit" => self.submit_current_prompt().await?,
            unknown => self.app.apply_stream_update(neo_tui::StreamUpdate::Notice {
                text: format!("Unknown command: {unknown}"),
            }),
        }
        Ok(())
    }

    async fn export_active_session_to_html(&mut self) -> Result<()> {
        let Some(session_id) = self.active_session_id.clone() else {
            self.app.apply_stream_update(neo_tui::StreamUpdate::Notice {
                text: "No active session to export".to_owned(),
            });
            return Ok(());
        };
        let config = self
            .local_config
            .clone()
            .context("session HTML export is unavailable")?;
        let html = crate::session_commands::export_html(&session_id, &config).await?;
        let output_path =
            crate::session_commands::session_path(&session_id, &config)?.with_extension("html");
        fs::write(&output_path, html)
            .with_context(|| format!("failed to write {}", output_path.display()))?;
        self.app.apply_stream_update(neo_tui::StreamUpdate::Notice {
            text: format!("Exported session {session_id} to {}", output_path.display()),
        });
        Ok(())
    }

    async fn submit_current_prompt(&mut self) -> Result<()> {
        if self.active_turn.is_some() {
            self.app.apply_stream_update(neo_tui::StreamUpdate::Notice {
                text: "A turn is already running".to_owned(),
            });
            return Ok(());
        }
        let Some(prompt) = self.app.submit_prompt() else {
            return Ok(());
        };
        if prompt.trim() == "/tree" {
            self.open_session_picker();
            return Ok(());
        }
        let PromptSubmission {
            prompt,
            model_override,
        } = PromptSubmission::from_text(
            prompt,
            &self.model_items,
            self.local_config.as_ref(),
            &self.completion_root,
        )?;
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (approval_tx, approval_rx) = mpsc::unbounded_channel();
        let cancel_token = CancellationToken::new();
        let channels = TurnChannels {
            events: event_tx.clone(),
            approvals: approval_tx,
            cancel_token: cancel_token.clone(),
        };
        let future = (self.run_turn)(
            TurnRequest::new(vec![prompt], self.active_session_id.clone(), {
                model_override.or_else(|| self.active_model.clone())
            }),
            channels,
        );
        let task = tokio::spawn(async move {
            let result = future.await;
            if let Err(error) = &result {
                let _ = event_tx.send(Err(anyhow::anyhow!(error.to_string())));
            }
            result
        });
        self.active_turn = Some(RunningTurn {
            events: event_rx,
            approvals: approval_rx,
            task,
            cancel_token,
        });
        self.drain_active_turn().await
    }

    async fn wait_for_active_turn(&mut self) -> Result<()> {
        while self.active_turn.is_some() {
            self.drain_active_turn().await?;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(())
    }

    async fn cancel_active_turn(&mut self) -> Result<()> {
        if let Some(turn) = &self.active_turn {
            turn.cancel_token.cancel();
        }
        self.pending_approvals.clear();
        self.resolved_approvals.clear();
        if let Ok(result) =
            tokio::time::timeout(Duration::from_secs(2), self.wait_for_active_turn()).await
        {
            result
        } else {
            self.abort_active_turn();
            Ok(())
        }
    }

    async fn drain_active_turn(&mut self) -> Result<()> {
        let Some(mut turn) = self.active_turn.take() else {
            return Ok(());
        };

        while let Ok(approval) = turn.approvals.try_recv() {
            self.register_pending_approval(approval);
        }
        while let Ok(event) = turn.events.try_recv() {
            match event {
                Ok(event) => self.apply_turn_event(event),
                Err(error) => {
                    self.app.apply_stream_update(neo_tui::StreamUpdate::Error {
                        text: error.to_string(),
                    });
                }
            }
        }

        if turn.task.is_finished() {
            let result = turn
                .task
                .await
                .map_err(|error| anyhow::anyhow!("interactive turn task failed: {error}"))?;
            while let Ok(approval) = turn.approvals.try_recv() {
                self.register_pending_approval(approval);
            }
            while let Ok(event) = turn.events.try_recv() {
                match event {
                    Ok(event) => self.apply_turn_event(event),
                    Err(error) => {
                        self.app.apply_stream_update(neo_tui::StreamUpdate::Error {
                            text: error.to_string(),
                        });
                    }
                }
            }
            result?;
        } else {
            self.active_turn = Some(turn);
        }
        Ok(())
    }

    fn apply_turn_event(&mut self, event: AgentEvent) {
        if self.always_approve && matches!(event, AgentEvent::ApprovalRequested { .. }) {
            return;
        }
        self.app.apply_agent_event(event);
    }

    fn register_pending_approval(&mut self, approval: crate::modes::run::PromptApprovalRequest) {
        if self.always_approve {
            self.resolved_approvals.remove(&approval.id);
            let _ = approval.decision_tx.send(PermissionDecision::Allow);
            return;
        }
        if let Some(decision) = self.resolved_approvals.remove(&approval.id) {
            let _ = approval.decision_tx.send(decision);
        } else {
            self.pending_approvals
                .insert(approval.id, approval.decision_tx);
        }
    }

    fn resolve_approval(&mut self, result: &ApprovalResult) {
        let decision = match result.choice {
            ApprovalChoice::Approve => PermissionDecision::Allow,
            ApprovalChoice::AlwaysApprove => {
                self.always_approve = true;
                PermissionDecision::Allow
            }
            ApprovalChoice::Deny => PermissionDecision::Deny,
        };
        if let Some(tx) = self.pending_approvals.remove(&result.request_id) {
            let _ = tx.send(decision);
        } else {
            self.resolved_approvals
                .insert(result.request_id.clone(), decision);
        }
    }

    fn abort_active_turn(&mut self) {
        if let Some(turn) = self.active_turn.take() {
            turn.cancel_token.cancel();
            turn.task.abort();
        }
        self.pending_approvals.clear();
        self.resolved_approvals.clear();
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

fn prompt_completions(
    root: &Path,
    prefix: &str,
    model_items: &[PickerItem],
) -> Result<Vec<PickerItem>> {
    if let Some(completions) = slash_prompt_template_completions(root, prefix)? {
        return Ok(completions);
    }
    if let Some(completions) = model_prompt_completions(prefix, model_items)
        && !completions.is_empty()
    {
        return Ok(completions);
    }
    filesystem_prompt_completions(root, prefix)
}

fn slash_prompt_template_completions(root: &Path, prefix: &str) -> Result<Option<Vec<PickerItem>>> {
    let Some(name_prefix) = prefix.strip_prefix('/') else {
        return Ok(None);
    };
    if name_prefix.contains('/') || name_prefix.is_empty() {
        return Ok(None);
    }

    let mut completions = load_project_prompt_templates(root)?
        .into_iter()
        .filter(|template| template.name.starts_with(name_prefix))
        .map(|template| {
            let value = format!("/{}", template.name);
            let description = (!template.description.is_empty()).then_some(template.description);
            PickerItem::new(value.clone(), value, description)
        })
        .collect::<Vec<_>>();
    completions.sort_by(|left, right| left.value.cmp(&right.value));
    completions.truncate(100);
    Ok(Some(completions))
}

fn filesystem_prompt_completions(root: &Path, prefix: &str) -> Result<Vec<PickerItem>> {
    let Some(request) = FilesystemCompletionRequest::from_prefix(root, prefix) else {
        return Ok(Vec::new());
    };

    let entries = match fs::read_dir(&request.search_dir) {
        Ok(entries) => entries,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(Vec::new());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read {}", request.search_dir.display()));
        }
    };

    let mut completions = Vec::new();
    for entry in entries {
        let entry =
            entry.with_context(|| format!("failed to inspect {}", request.search_dir.display()))?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if !request.name_prefix.starts_with('.') && name.starts_with('.') {
            continue;
        }
        if !name.starts_with(&request.name_prefix) {
            continue;
        }

        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", entry.path().display()))?;
        let suffix = if file_type.is_dir() { "/" } else { "" };
        let value = format!(
            "{}{}{}{}",
            request.mention_prefix, request.display_dir, name, suffix
        );
        let description = if file_type.is_dir() {
            "directory"
        } else {
            "file"
        };
        completions.push(PickerItem::new(value.clone(), value, Some(description)));
    }

    completions.sort_by(|left, right| left.value.cmp(&right.value));
    completions.truncate(100);
    Ok(completions)
}

fn model_prompt_completions(prefix: &str, model_items: &[PickerItem]) -> Option<Vec<PickerItem>> {
    let model_prefix = prefix.strip_prefix('@')?;
    if model_items.is_empty() {
        return None;
    }

    let mut completions = model_items
        .iter()
        .filter(|item| item.value.starts_with(model_prefix))
        .map(|item| {
            let value = format!("@{}", item.value);
            PickerItem::new(value.clone(), value, item.description.clone())
        })
        .collect::<Vec<_>>();
    completions.sort_by(|left, right| left.value.cmp(&right.value));
    completions.truncate(100);
    Some(completions)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PromptSubmission {
    prompt: String,
    model_override: Option<SelectedModel>,
}

impl PromptSubmission {
    fn from_text(
        prompt: String,
        model_items: &[PickerItem],
        config: Option<&AppConfig>,
        fallback_project_dir: &Path,
    ) -> Result<Self> {
        let Some((candidate, rest)) = split_first_prompt_token(&prompt) else {
            return Ok(Self {
                prompt,
                model_override: None,
            });
        };
        let Some(model_value) = candidate.strip_prefix('@') else {
            return Ok(Self {
                prompt: expand_interactive_prompt(&prompt, config, fallback_project_dir)?,
                model_override: None,
            });
        };
        let Some(model_item) = model_items.iter().find(|item| item.value == model_value) else {
            return Ok(Self {
                prompt: expand_interactive_prompt(&prompt, config, fallback_project_dir)?,
                model_override: None,
            });
        };
        let prompt_after_model = rest.trim_start();
        if prompt_after_model.is_empty() {
            return Ok(Self {
                prompt,
                model_override: None,
            });
        }

        Ok(Self {
            prompt: expand_interactive_prompt(prompt_after_model, config, fallback_project_dir)?,
            model_override: Some(SelectedModel::from_picker_item(model_item)?),
        })
    }
}

fn expand_interactive_prompt(
    prompt: &str,
    config: Option<&AppConfig>,
    fallback_project_dir: &Path,
) -> Result<String> {
    let Some(args) = slash_prompt_args(prompt) else {
        return Ok(prompt.to_owned());
    };
    let (project_dir, selectors, disabled) = if let Some(config) = config {
        let mut selectors = config.configured_prompt_templates.clone();
        for selector in &config.prompt_templates {
            if !selectors.contains(selector) {
                selectors.push(selector.clone());
            }
        }
        (
            config.project_dir.as_path(),
            selectors,
            config.no_prompt_templates,
        )
    } else {
        (fallback_project_dir, Vec::new(), false)
    };
    let expanded = expand_prompt_template_args(
        args,
        project_dir,
        config::global_prompts_dir().as_deref(),
        &selectors,
        disabled,
    )?;
    Ok(expanded.join(" "))
}

fn slash_prompt_args(prompt: &str) -> Option<Vec<String>> {
    let prompt = prompt.trim_start();
    let (candidate, _) = split_first_prompt_token(prompt)?;
    if !candidate.starts_with('/') || candidate.len() == 1 {
        return None;
    }
    Some(prompt.split_whitespace().map(str::to_owned).collect())
}

fn split_first_prompt_token(prompt: &str) -> Option<(&str, &str)> {
    let prompt = prompt.trim_start();
    if prompt.is_empty() {
        return None;
    }
    match prompt.find(char::is_whitespace) {
        Some(index) => Some((&prompt[..index], &prompt[index..])),
        None => Some((prompt, "")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FilesystemCompletionRequest {
    mention_prefix: &'static str,
    display_dir: String,
    name_prefix: String,
    search_dir: PathBuf,
}

impl FilesystemCompletionRequest {
    fn from_prefix(root: &Path, prefix: &str) -> Option<Self> {
        if prefix.is_empty() {
            return None;
        }

        let (mention_prefix, path_prefix) = if let Some(path_prefix) = prefix.strip_prefix('@') {
            ("@", path_prefix)
        } else {
            ("", prefix)
        };
        let (display_dir, name_prefix) = split_completion_path(path_prefix);
        let search_dir = if Path::new(&display_dir).is_absolute() {
            PathBuf::from(&display_dir)
        } else {
            root.join(&display_dir)
        };

        Some(Self {
            mention_prefix,
            display_dir,
            name_prefix,
            search_dir,
        })
    }
}

fn split_completion_path(prefix: &str) -> (String, String) {
    if prefix.ends_with('/') {
        return (prefix.to_owned(), String::new());
    }
    match prefix.rsplit_once('/') {
        Some((directory, name)) => (format!("{directory}/"), name.to_owned()),
        None => (String::new(), prefix.to_owned()),
    }
}

fn longest_common_completion_prefix(completions: &[PickerItem]) -> Option<String> {
    let first = completions.first()?.value.clone();
    let mut prefix = first.chars().collect::<Vec<_>>();
    for completion in completions.iter().skip(1) {
        let candidate = completion.value.chars().collect::<Vec<_>>();
        let len = prefix
            .iter()
            .zip(candidate.iter())
            .take_while(|(left, right)| left == right)
            .count();
        prefix.truncate(len);
        if prefix.is_empty() {
            break;
        }
    }
    Some(prefix.into_iter().collect())
}

fn write_system_clipboard(text: &str) -> Result<()> {
    let commands: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else if cfg!(target_os = "windows") {
        &[("clip.exe", &[])]
    } else {
        &[("wl-copy", &[]), ("xclip", &["-selection", "clipboard"])]
    };
    let mut errors = Vec::new();
    for (program, args) in commands {
        match write_clipboard_command(program, args, text) {
            Ok(()) => return Ok(()),
            Err(error) => errors.push(format!("{program}: {error}")),
        }
    }
    anyhow::bail!(
        "no system clipboard writer succeeded ({})",
        errors.join("; ")
    )
}

fn write_clipboard_command(program: &str, args: &[&str], text: &str) -> Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start {program}"))?;
    child
        .stdin
        .as_mut()
        .context("clipboard command stdin was unavailable")?
        .write_all(text.as_bytes())
        .with_context(|| format!("failed to write to {program}"))?;
    let output = child
        .wait_with_output()
        .with_context(|| format!("failed to wait for {program}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    anyhow::bail!(
        "exited with {}{}",
        output.status,
        if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        }
    )
}

fn command_specs(project_dir: &Path) -> (Vec<CommandSpec>, Option<String>) {
    let mut commands = vec![
        CommandSpec::new("sessions", "Open sessions", Some("Browse local sessions")),
        CommandSpec::new("models", "Open models", Some("Switch active model")),
        CommandSpec::new(
            "session.exportHtml",
            "Export session to HTML",
            Some("Write the active local session as sanitized HTML"),
        ),
        CommandSpec::new(
            "copy-prompt",
            "Copy prompt",
            Some("Copy current prompt text"),
        ),
        CommandSpec::new(
            "select-transcript",
            "Select transcript item",
            Some("Start transcript item selection"),
        ),
        CommandSpec::new(
            "copy-transcript-selection",
            "Copy transcript selection",
            Some("Copy selected transcript items"),
        ),
        CommandSpec::new(
            "clear-transcript-selection",
            "Clear transcript selection",
            Some("Remove transcript selection"),
        ),
        CommandSpec::new("submit", "Submit prompt", Some("Submit the current prompt")),
    ];
    let mut templates = match load_project_prompt_templates(project_dir) {
        Ok(templates) => templates,
        Err(error) => return (commands, Some(error.to_string())),
    };
    templates.sort_by(|left, right| left.name.cmp(&right.name));
    commands.extend(templates.into_iter().map(|template| {
        let label = format!("/{}", template.name);
        let description = (!template.description.is_empty()).then_some(template.description);
        CommandSpec::new(
            format!("prompt-template.{}", template.name),
            label,
            description,
        )
    }));
    (commands, None)
}

pub trait TerminalEvents {
    fn next_input_event(&mut self) -> Result<InputEvent>;

    fn poll_input_event(&mut self, _timeout: Duration) -> Result<Option<InputEvent>> {
        self.next_input_event().map(Some)
    }
}

struct CrosstermEvents {
    parser: InputParser,
    pending: VecDeque<InputEvent>,
}

impl CrosstermEvents {
    fn new(keybindings: KeybindingsManager) -> Self {
        Self {
            parser: InputParser::with_keybindings(keybindings),
            pending: VecDeque::new(),
        }
    }
}

impl Default for CrosstermEvents {
    fn default() -> Self {
        Self::new(KeybindingsManager::default())
    }
}

impl TerminalEvents for CrosstermEvents {
    fn next_input_event(&mut self) -> Result<InputEvent> {
        loop {
            if let Some(input) = self.poll_input_event(Duration::from_millis(250))? {
                return Ok(input);
            }
        }
    }

    fn poll_input_event(&mut self, timeout: Duration) -> Result<Option<InputEvent>> {
        if let Some(input) = self.pending.pop_front() {
            return Ok(Some(input));
        }

        if event::poll(timeout)? {
            let event = event::read()?;
            self.pending
                .extend(self.parser.feed_crossterm_event(&event));
            if let Some(input) = self.pending.pop_front() {
                return Ok(Some(input));
            }
        }
        Ok(None)
    }
}

const EDITING_ACTION_PRIORITY: &[KeybindingAction] = &[
    KeybindingAction::InputSubmit,
    KeybindingAction::InputNewLine,
    KeybindingAction::TranscriptCopySelection,
    KeybindingAction::InputCopy,
    KeybindingAction::TranscriptSelectionStart,
    KeybindingAction::TranscriptSelectionClear,
    KeybindingAction::TranscriptSelectionExtendUp,
    KeybindingAction::TranscriptSelectionExtendDown,
    KeybindingAction::TranscriptSelectionExtendPageUp,
    KeybindingAction::TranscriptSelectionExtendPageDown,
    KeybindingAction::CommandPaletteOpen,
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
        execute!(output, EnterAlternateScreen, EnableBracketedPaste)?;
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
        let _ = execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
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

pub fn controller_for_config(config: &AppConfig) -> InteractiveController {
    let catalogs = picker_catalogs_for_config(config);
    let config = config.clone();
    let run_config = config.clone();
    let run_turn: TurnDriver = Arc::new(move |request, channels| {
        let mut effective_config = run_config.clone();
        Box::pin(async move {
            if let Some(model) = request.model {
                effective_config.default_provider = model.provider;
                effective_config.default_model = model.model;
            }
            if let Some(session_id) = request.session_id {
                crate::modes::run::run_prompt_in_session_streaming(
                    &session_id,
                    &request.prompt,
                    &effective_config,
                    channels.events,
                    channels.approvals,
                    channels.cancel_token,
                )
                .await?;
            } else {
                crate::modes::run::run_prompt_streaming(
                    &request.prompt,
                    &effective_config,
                    channels.events,
                    channels.approvals,
                    channels.cancel_token,
                )
                .await?;
            }
            Ok(())
        })
    });
    let load_config = config.clone();
    let load_session: SessionLoader = Arc::new(move |session_id| {
        let config = load_config.clone();
        Box::pin(async move { load_session_transcript(session_id, &config).await })
    });
    let fork_config = config.clone();
    let fork_session: SessionForker = Arc::new(move |session_id| {
        let config = fork_config.clone();
        Box::pin(async move { fork_session_transcript(session_id, &config).await })
    });

    let mut controller = InteractiveController::new_with_turn_driver(
        "neo",
        "new",
        format!("{}/{}", config.default_provider, config.default_model),
        run_turn,
        catalogs,
        load_session,
        fork_session,
    );
    let mut keybindings = KeybindingsManager::default();
    keybindings.set_user_bindings(
        config
            .tui
            .keybinding_overrides()
            .expect("AppConfig TUI keybindings should be validated before controller creation"),
    );
    controller.keybindings = keybindings;
    controller.completion_root.clone_from(&config.project_dir);
    controller.local_config = Some(config);
    controller
}

fn legacy_turn_driver<RunTurn, Fut>(run_turn: RunTurn) -> TurnDriver
where
    RunTurn: Fn(TurnRequest) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Vec<AgentEvent>>> + Send + 'static,
{
    let run_turn = Arc::new(run_turn);
    Arc::new(move |request, channels| {
        let run_turn = Arc::clone(&run_turn);
        Box::pin(async move {
            let events = run_turn(request).await?;
            for event in events {
                channels.send_event(event);
            }
            Ok(())
        })
    })
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
        Ok(registry) => {
            let models = registry.list();
            let models = config::scoped_models(models.iter(), &config.model_scope);
            ModelCatalog {
                items: models.iter().map(model_to_picker_item).collect(),
                error: None,
            }
        }
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
    use crate::config::{Defaults, McpConfig, RuntimeConfig, ToolFilterConfig, TuiConfig};

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
    async fn event_loop_inserts_paste_newlines_without_submitting_until_enter() {
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
                assert_eq!(request.prompt, vec!["alpha\nbeta".to_owned()]);
                Ok(vec![AgentEvent::TurnFinished {
                    turn: 1,
                    stop_reason: StopReason::EndTurn,
                }])
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
                        InputEvent::Paste("alpha\nbeta".to_owned()),
                        InputEvent::Submit,
                        InputEvent::Cancel,
                    ]
                    .into_iter(),
                },
            )
            .await
            .expect("event loop succeeds");

        assert!(rendered.iter().any(|snapshot| snapshot.contains("alpha")));
        assert!(rendered.iter().any(|snapshot| snapshot.contains("beta")));
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

        let mut controller = InteractiveController::new_with_sessions(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
            PickerCatalogs {
                session_items: vec![PickerItem::new("alpha", "Alpha", Some("session"))],
                session_error: None,
                model_items: Vec::new(),
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
        controller.set_clipboard_writer(Arc::new(|_text| Ok(())));

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
    async fn event_loop_copies_prompt_text_from_default_ctrl_c_keybinding() {
        let mut controller = InteractiveController::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.set_clipboard_writer(Arc::new(|_text| Ok(())));

        controller.type_text("copy through keybinding");
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
            .await
            .expect("copy keybinding handled");

        assert_eq!(
            controller.app().copy_buffer(),
            Some("copy through keybinding")
        );
        assert_eq!(controller.app().prompt().text, "copy through keybinding");
    }

    #[tokio::test]
    async fn event_loop_copy_action_writes_prompt_to_injected_clipboard() {
        let copied = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = std::sync::Arc::clone(&copied);
        let mut controller = InteractiveController::new_with_sessions(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
            PickerCatalogs {
                session_items: vec![PickerItem::new("alpha", "Alpha", Some("session"))],
                session_error: None,
                model_items: Vec::new(),
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
        controller.set_clipboard_writer(Arc::new(move |text| {
            recorded
                .lock()
                .expect("record clipboard text")
                .push(text.to_owned());
            Ok(())
        }));

        controller.type_text("copy to system clipboard");
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
            .await
            .expect("copy action succeeds");

        assert_eq!(
            copied.lock().expect("clipboard writes").as_slice(),
            ["copy to system clipboard"]
        );
        assert_eq!(
            controller.app().copy_buffer(),
            Some("copy to system clipboard")
        );
    }

    #[tokio::test]
    async fn event_loop_ctrl_c_prefers_selected_transcript_region() {
        let copied = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = std::sync::Arc::clone(&copied);
        let mut controller = InteractiveController::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.set_clipboard_writer(Arc::new(move |text| {
            recorded
                .lock()
                .expect("record clipboard text")
                .push(text.to_owned());
            Ok(())
        }));
        controller
            .app
            .transcript_mut()
            .push(neo_tui::TranscriptItem::user("selected user prompt"));
        controller
            .app
            .transcript_mut()
            .push(neo_tui::TranscriptItem::assistant(
                "selected assistant reply",
            ));
        controller.type_text("prompt text stays out of clipboard");

        controller
            .handle_input_event(InputEvent::Action(
                KeybindingAction::TranscriptSelectionStart,
            ))
            .await
            .expect("selection starts");
        controller
            .handle_input_event(InputEvent::Action(
                KeybindingAction::TranscriptSelectionExtendUp,
            ))
            .await
            .expect("selection extends");
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
            .await
            .expect("copy action succeeds");

        assert_eq!(
            copied.lock().expect("clipboard writes").as_slice(),
            ["You\nselected user prompt\n\nAssistant\nselected assistant reply"]
        );
        assert_eq!(
            controller.app().copy_buffer(),
            Some("You\nselected user prompt\n\nAssistant\nselected assistant reply")
        );
        assert_eq!(
            controller.app().prompt().text,
            "prompt text stays out of clipboard"
        );
    }

    #[tokio::test]
    async fn event_loop_clipboard_failure_keeps_internal_copy_buffer() {
        let mut controller = InteractiveController::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.set_clipboard_writer(Arc::new(|_text| {
            Err(anyhow::anyhow!("clipboard unavailable"))
        }));

        controller.type_text("copy fallback");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputCopy))
            .await
            .expect("clipboard failure is non-fatal");

        assert_eq!(controller.app().copy_buffer(), Some("copy fallback"));
        assert!(matches!(
            controller.app().transcript().items().last(),
            Some(neo_tui::TranscriptItem::Notice { content })
                if content.contains("Clipboard copy failed")
                    && content.contains("clipboard unavailable")
        ));
    }

    #[tokio::test]
    async fn event_loop_ctrl_c_cancels_overlay_without_copying_prompt() {
        let copied = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = std::sync::Arc::clone(&copied);
        let mut controller = InteractiveController::new_with_sessions(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
            PickerCatalogs {
                session_items: vec![PickerItem::new("alpha", "Alpha", Some("session"))],
                session_error: None,
                model_items: Vec::new(),
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
        controller.set_clipboard_writer(Arc::new(move |text| {
            recorded
                .lock()
                .expect("record clipboard text")
                .push(text.to_owned());
            Ok(())
        }));

        controller.type_text("do not copy while overlay is focused");
        controller.open_session_picker();
        assert!(controller.app().focused_overlay().is_some());

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
            .await
            .expect("overlay cancel succeeds");

        assert!(controller.app().focused_overlay().is_none());
        assert_eq!(controller.app().copy_buffer(), None);
        assert!(copied.lock().expect("clipboard writes").is_empty());
    }

    #[tokio::test]
    async fn event_loop_tabs_through_real_filesystem_prompt_completions() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(temp.path().join("src")).expect("create src");
        fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").expect("write main");
        fs::write(temp.path().join("src/matrix.rs"), "pub fn matrix() {}\n").expect("write matrix");

        let mut controller = InteractiveController::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.completion_root = temp.path().to_path_buf();

        controller.type_text("open src/ma");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
            .await
            .expect("tab opens completion picker");

        assert!(matches!(
            controller
                .app()
                .focused_overlay()
                .map(|overlay| &overlay.kind),
            Some(OverlayKind::PromptCompletion(_))
        ));

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("completion confirms");

        assert_eq!(controller.app().prompt().text, "open src/main.rs");
        assert_eq!(controller.app().prompt().cursor, 16);
        assert!(controller.app().focused_overlay().is_none());
    }

    #[tokio::test]
    async fn controller_for_config_applies_tui_keybinding_overrides() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = temp.path().join(".neo/sessions");
        fs::create_dir_all(&sessions_dir).expect("create sessions");
        let mut config = test_config(temp.path(), sessions_dir);
        config
            .tui
            .keybindings
            .insert("tui.command.open".to_owned(), vec!["ctrl+g".to_owned()]);
        let mut controller = controller_for_config(&config);

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+g").expect("valid key")))
            .await
            .expect("configured keybinding runs");

        assert!(matches!(
            controller
                .app()
                .focused_overlay()
                .map(|overlay| &overlay.kind),
            Some(OverlayKind::CommandPalette(_))
        ));
    }

    #[tokio::test]
    async fn event_loop_tabs_through_local_slash_prompt_template_completions() {
        let temp = tempfile::tempdir().expect("tempdir");
        let prompts_dir = temp.path().join(".neo/prompts");
        fs::create_dir_all(&prompts_dir).expect("create prompts");
        fs::write(
            prompts_dir.join("review.md"),
            "---\ndescription: Review the current change\n---\nReview this change.\n",
        )
        .expect("write review prompt");

        let mut controller = InteractiveController::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.completion_root = temp.path().to_path_buf();

        controller.type_text("/rev");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
            .await
            .expect("tab completes slash prompt");

        assert_eq!(controller.app().prompt().text, "/review");
        assert_eq!(controller.app().prompt().cursor, 7);
        assert!(controller.app().focused_overlay().is_none());
    }

    #[tokio::test]
    async fn event_loop_slash_tree_opens_local_session_picker() {
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_requests = std::sync::Arc::clone(&requests);
        let mut controller = InteractiveController::new_with_sessions(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            move |request| {
                let captured_requests = std::sync::Arc::clone(&captured_requests);
                async move {
                    captured_requests
                        .lock()
                        .expect("record request")
                        .push(request);
                    Ok(Vec::<AgentEvent>::new())
                }
            },
            PickerCatalogs {
                session_items: vec![PickerItem::new("alpha", "Alpha", Some("root"))],
                session_error: None,
                model_items: Vec::new(),
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

        controller.type_text("/tree");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("slash tree command runs locally");

        assert!(matches!(
            controller
                .app()
                .focused_overlay()
                .map(|overlay| &overlay.kind),
            Some(OverlayKind::SessionPicker(_))
        ));
        assert!(controller.app().prompt().text.is_empty());
        assert!(requests.lock().expect("recorded requests").is_empty());
    }

    #[tokio::test]
    async fn event_loop_tab_completes_provider_model_prefix() {
        let mut controller = InteractiveController::new_with_sessions(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
            PickerCatalogs {
                session_items: Vec::new(),
                session_error: None,
                model_items: vec![
                    PickerItem::new(
                        "anthropic/claude-sonnet",
                        "anthropic/claude-sonnet",
                        Some("Messages"),
                    ),
                    PickerItem::new("openai/gpt-4.1", "openai/gpt-4.1", Some("Responses")),
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

        controller.type_text("@anth");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
            .await
            .expect("tab completes provider/model prefix");

        assert_eq!(controller.app().prompt().text, "@anthropic/claude-sonnet");
        assert_eq!(controller.app().prompt().cursor, 24);
        assert!(controller.app().focused_overlay().is_none());
    }

    #[tokio::test]
    async fn event_loop_inline_provider_model_prefix_overrides_submitted_turn() {
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_requests = std::sync::Arc::clone(&requests);
        let mut controller = InteractiveController::new_with_sessions(
            "neo",
            "test-session",
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
                            text: "inline model selected".to_owned(),
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
                    PickerItem::new(
                        "anthropic/claude-sonnet",
                        "anthropic/claude-sonnet",
                        Some("Messages"),
                    ),
                    PickerItem::new("openai/gpt-4.1", "openai/gpt-4.1", Some("Responses")),
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

        controller.type_text("@anth");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
            .await
            .expect("tab completes provider/model prefix");
        controller.type_text(" explain this file");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("turn submits with inline model");
        controller
            .wait_for_active_turn()
            .await
            .expect("inline model turn completes");

        let requests = requests.lock().expect("recorded requests");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].prompt, vec!["explain this file".to_owned()]);
        let selected = requests[0].model.as_ref().expect("inline model");
        assert_eq!(selected.provider, "anthropic");
        assert_eq!(selected.model, "claude-sonnet");
    }

    #[tokio::test]
    async fn event_loop_keeps_unknown_at_prefix_as_prompt_text() {
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_requests = std::sync::Arc::clone(&requests);
        let mut controller = InteractiveController::new_with_sessions(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            move |request| {
                let captured_requests = std::sync::Arc::clone(&captured_requests);
                async move {
                    captured_requests
                        .lock()
                        .expect("record request")
                        .push(request);
                    Ok(Vec::<AgentEvent>::new())
                }
            },
            PickerCatalogs {
                session_items: Vec::new(),
                session_error: None,
                model_items: vec![PickerItem::new(
                    "anthropic/claude-sonnet",
                    "anthropic/claude-sonnet",
                    Some("Messages"),
                )],
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

        controller.type_text("@src/main.rs explain this file");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("turn submits with file mention");
        controller
            .wait_for_active_turn()
            .await
            .expect("file mention turn completes");

        let requests = requests.lock().expect("recorded requests");
        assert_eq!(
            requests[0].prompt,
            vec!["@src/main.rs explain this file".to_owned()]
        );
        assert_eq!(requests[0].model, None);
    }

    #[tokio::test]
    async fn event_loop_inline_model_token_without_prompt_does_not_override_model() {
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_requests = std::sync::Arc::clone(&requests);
        let mut controller = InteractiveController::new_with_sessions(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            move |request| {
                let captured_requests = std::sync::Arc::clone(&captured_requests);
                async move {
                    captured_requests
                        .lock()
                        .expect("record request")
                        .push(request);
                    Ok(Vec::<AgentEvent>::new())
                }
            },
            PickerCatalogs {
                session_items: Vec::new(),
                session_error: None,
                model_items: vec![PickerItem::new(
                    "anthropic/claude-sonnet",
                    "anthropic/claude-sonnet",
                    Some("Messages"),
                )],
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

        controller.type_text("@anthropic/claude-sonnet");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("turn submits literal model token");
        controller
            .wait_for_active_turn()
            .await
            .expect("literal model token turn completes");

        let requests = requests.lock().expect("recorded requests");
        assert_eq!(
            requests[0].prompt,
            vec!["@anthropic/claude-sonnet".to_owned()]
        );
        assert_eq!(requests[0].model, None);
    }

    #[tokio::test]
    async fn event_loop_tab_extends_common_filesystem_completion_prefix() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("README.md"), "readme\n").expect("write readme");
        fs::write(temp.path().join("RELEASE.md"), "release\n").expect("write release");

        let mut controller = InteractiveController::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.completion_root = temp.path().to_path_buf();

        controller.type_text("open R");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
            .await
            .expect("tab extends common prefix");

        assert_eq!(controller.app().prompt().text, "open RE");
        assert_eq!(controller.app().prompt().cursor, 7);
        assert!(controller.app().focused_overlay().is_none());
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
    async fn event_loop_opens_command_palette_and_runs_local_model_command() {
        let mut controller = InteractiveController::new_with_sessions(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
            PickerCatalogs {
                session_items: Vec::new(),
                session_error: None,
                model_items: vec![PickerItem::new(
                    "anthropic/claude-sonnet",
                    "anthropic/claude-sonnet",
                    Some("messages"),
                )],
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
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+p").expect("valid key")))
            .await
            .expect("command palette opens");
        let Some(OverlayKind::CommandPalette(palette)) = controller
            .app()
            .focused_overlay()
            .map(|overlay| &overlay.kind)
        else {
            panic!("expected command palette overlay");
        };
        assert_eq!(palette.selected_command().expect("command").id, "sessions");

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("moves to model command");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("command runs");

        assert!(matches!(
            controller
                .app()
                .focused_overlay()
                .map(|overlay| &overlay.kind),
            Some(OverlayKind::ModelPicker(_))
        ));
    }

    #[tokio::test]
    async fn command_palette_inserts_project_prompt_template_command() {
        let temp = tempfile::tempdir().expect("tempdir");
        let prompts_dir = temp.path().join(".neo/prompts");
        fs::create_dir_all(&prompts_dir).expect("create prompts");
        fs::write(
            prompts_dir.join("review.md"),
            "---\ndescription: Review a target\nargument-hint: <path>\n---\nReview $1.\n",
        )
        .expect("write review prompt");

        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_requests = std::sync::Arc::clone(&requests);
        let mut controller =
            InteractiveController::new("neo", "test-session", "openai/gpt-4.1", move |request| {
                let captured_requests = std::sync::Arc::clone(&captured_requests);
                async move {
                    captured_requests
                        .lock()
                        .expect("record request")
                        .push(request);
                    Ok(Vec::<AgentEvent>::new())
                }
            });
        controller.completion_root = temp.path().to_path_buf();

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::CommandPaletteOpen))
            .await
            .expect("command palette opens");
        for _ in 0..32 {
            let selected = controller
                .app()
                .selected_command()
                .expect("selected command");
            if selected.id == "prompt-template.review" {
                break;
            }
            controller
                .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
                .await
                .expect("move to review command");
        }
        assert_eq!(
            controller
                .app()
                .selected_command()
                .expect("review command")
                .id,
            "prompt-template.review"
        );

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("prompt template command inserts invocation");

        assert_eq!(controller.app().prompt().text, "/review ");
        assert_eq!(controller.app().prompt().cursor, 8);
        assert!(controller.app().focused_overlay().is_none());

        controller.type_text("src/lib.rs");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("prompt template command submits");
        controller
            .wait_for_active_turn()
            .await
            .expect("prompt template turn completes");

        let requests = requests.lock().expect("recorded requests");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].prompt, vec!["Review src/lib.rs.".to_owned()]);
    }

    #[tokio::test]
    async fn command_palette_exports_active_session_to_html() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = temp.path().join(".neo/sessions");
        fs::create_dir_all(&sessions_dir).expect("create sessions dir");
        fs::write(
            sessions_dir.join("alpha.jsonl"),
            concat!(
                "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello <script>alert(1)</script>\"}}]}}}}\n",
                "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"use **bold** safely\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
            ),
        )
        .expect("write session jsonl");

        let config = test_config(temp.path(), sessions_dir.clone());
        let mut controller = controller_for_config(&config);
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SessionPickerOpen))
            .await
            .expect("session picker opens");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("session loads");

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::CommandPaletteOpen))
            .await
            .expect("command palette opens");
        for _ in 0..32 {
            let selected = controller
                .app()
                .selected_command()
                .expect("selected command");
            if selected.id == "session.exportHtml" {
                break;
            }
            controller
                .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
                .await
                .expect("move to export command");
        }
        assert_eq!(
            controller
                .app()
                .selected_command()
                .expect("export command")
                .id,
            "session.exportHtml"
        );
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("export command runs");

        let export_path = sessions_dir.join("alpha.html");
        let html = fs::read_to_string(&export_path).expect("read exported html");
        assert!(html.contains("<title>neo session alpha</title>"));
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(controller.app().transcript().items().iter().any(|item| {
            matches!(
                item,
                neo_tui::TranscriptItem::Notice { content }
                    if content.contains("Exported session alpha to")
                        && content.contains(&export_path.display().to_string())
            )
        }));
    }

    #[tokio::test]
    async fn command_palette_export_html_without_active_session_shows_local_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = temp.path().join(".neo/sessions");
        fs::create_dir_all(&sessions_dir).expect("create sessions dir");
        let config = test_config(temp.path(), sessions_dir);
        let mut controller = controller_for_config(&config);

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::CommandPaletteOpen))
            .await
            .expect("command palette opens");
        for _ in 0..32 {
            let selected = controller
                .app()
                .selected_command()
                .expect("selected command");
            if selected.id == "session.exportHtml" {
                break;
            }
            controller
                .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
                .await
                .expect("move to export command");
        }

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("export command handles missing session locally");

        assert!(controller.app().transcript().items().iter().any(|item| {
            matches!(
                item,
                neo_tui::TranscriptItem::Notice { content }
                    if content.contains("No active session to export")
            )
        }));
    }

    #[tokio::test]
    async fn event_loop_confirms_approval_choice_to_running_turn() {
        use std::collections::VecDeque;

        struct ScriptedEvents {
            events: VecDeque<Option<InputEvent>>,
        }

        impl TerminalEvents for ScriptedEvents {
            fn next_input_event(&mut self) -> Result<InputEvent> {
                self.poll_input_event(Duration::from_millis(0))?
                    .ok_or_else(|| anyhow::anyhow!("expected scripted input"))
            }

            fn poll_input_event(&mut self, _timeout: Duration) -> Result<Option<InputEvent>> {
                Ok(self.events.pop_front().unwrap_or(Some(InputEvent::Cancel)))
            }
        }

        let decisions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_decisions = std::sync::Arc::clone(&decisions);
        let run_turn: TurnDriver = Arc::new(move |_request, channels| {
            let captured_decisions = std::sync::Arc::clone(&captured_decisions);
            Box::pin(async move {
                channels.send_event(AgentEvent::ApprovalRequested {
                    turn: 1,
                    id: "tool-1".to_owned(),
                    operation: neo_agent_core::PermissionOperation::Tool,
                    subject: "write".to_owned(),
                    arguments: serde_json::json!({"path": "approved.txt"}),
                });
                let (decision_tx, decision_rx) = oneshot::channel();
                channels
                    .approvals
                    .send(crate::modes::run::PromptApprovalRequest {
                        id: "tool-1".to_owned(),
                        decision_tx,
                    })
                    .expect("approval waiter sent");
                let decision = decision_rx.await.expect("approval decision");
                captured_decisions
                    .lock()
                    .expect("decisions lock")
                    .push(decision);
                channels.send_event(AgentEvent::TextDelta {
                    turn: 1,
                    text: "approved".to_owned(),
                });
                channels.send_event(AgentEvent::TurnFinished {
                    turn: 1,
                    stop_reason: StopReason::EndTurn,
                });
                Ok(())
            })
        });
        let mut controller = InteractiveController::new_with_turn_driver(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            run_turn,
            PickerCatalogs::default(),
            Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        );

        controller.type_text("write file");
        controller
            .run_terminal_loop(
                |_app| Ok(()),
                ScriptedEvents {
                    events: VecDeque::from([
                        Some(InputEvent::Submit),
                        None,
                        Some(InputEvent::Action(KeybindingAction::SelectConfirm)),
                        None,
                        Some(InputEvent::Cancel),
                    ]),
                },
            )
            .await
            .expect("approval loop completes");

        assert_eq!(
            *decisions.lock().expect("decisions lock"),
            vec![PermissionDecision::Allow]
        );
        assert!(controller.app().focused_overlay().is_none());
        assert!(controller.render_snapshot().contains("approved"));
    }

    #[tokio::test]
    async fn event_loop_interrupt_cancels_active_turn_token() {
        use std::{collections::VecDeque, sync::Arc as StdArc};

        struct ScriptedEvents {
            events: VecDeque<Option<InputEvent>>,
        }

        impl TerminalEvents for ScriptedEvents {
            fn next_input_event(&mut self) -> Result<InputEvent> {
                self.poll_input_event(Duration::from_millis(0))?
                    .ok_or_else(|| anyhow::anyhow!("expected scripted input"))
            }

            fn poll_input_event(&mut self, _timeout: Duration) -> Result<Option<InputEvent>> {
                Ok(self.events.pop_front().unwrap_or(Some(InputEvent::Cancel)))
            }
        }

        let captured_token = StdArc::new(std::sync::Mutex::new(None));
        let observed_token = StdArc::clone(&captured_token);
        let run_turn: TurnDriver = Arc::new(move |_request, channels| {
            let observed_token = StdArc::clone(&observed_token);
            Box::pin(async move {
                *observed_token.lock().expect("token lock") = Some(channels.cancel_token.clone());
                channels.send_event(AgentEvent::TextDelta {
                    turn: 1,
                    text: "started".to_owned(),
                });
                channels.cancel_token.cancelled().await;
                Ok(())
            })
        });
        let mut controller = InteractiveController::new_with_turn_driver(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            run_turn,
            PickerCatalogs::default(),
            Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        );

        controller.type_text("cancel me");
        controller
            .run_terminal_loop(
                |_app| Ok(()),
                ScriptedEvents {
                    events: VecDeque::from([
                        Some(InputEvent::Submit),
                        None,
                        Some(InputEvent::Interrupt),
                    ]),
                },
            )
            .await
            .expect("interrupt exits terminal loop");

        let token = captured_token
            .lock()
            .expect("token lock")
            .clone()
            .expect("turn token captured");
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn event_loop_interrupt_drains_cancelled_barriers_before_exit() {
        use std::{collections::VecDeque, sync::Arc as StdArc};

        struct ScriptedEvents {
            events: VecDeque<Option<InputEvent>>,
        }

        impl TerminalEvents for ScriptedEvents {
            fn next_input_event(&mut self) -> Result<InputEvent> {
                self.poll_input_event(Duration::from_millis(0))?
                    .ok_or_else(|| anyhow::anyhow!("expected scripted input"))
            }

            fn poll_input_event(&mut self, _timeout: Duration) -> Result<Option<InputEvent>> {
                Ok(self.events.pop_front().unwrap_or(Some(InputEvent::Cancel)))
            }
        }

        let captured_token = StdArc::new(std::sync::Mutex::new(None));
        let observed_token = StdArc::clone(&captured_token);
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let finished_tx = StdArc::new(std::sync::Mutex::new(Some(finished_tx)));
        let run_turn: TurnDriver = Arc::new(move |_request, channels| {
            let observed_token = StdArc::clone(&observed_token);
            let finished_tx = StdArc::clone(&finished_tx);
            Box::pin(async move {
                *observed_token.lock().expect("token lock") = Some(channels.cancel_token.clone());
                channels.send_event(AgentEvent::MessageStarted {
                    turn: 1,
                    id: "assistant-1".to_owned(),
                });
                channels.send_event(AgentEvent::TextDelta {
                    turn: 1,
                    text: "started".to_owned(),
                });
                channels.cancel_token.cancelled().await;
                channels.send_event(AgentEvent::MessageFinished {
                    turn: 1,
                    id: "assistant-1".to_owned(),
                    stop_reason: StopReason::Cancelled,
                });
                channels.send_event(AgentEvent::TurnFinished {
                    turn: 1,
                    stop_reason: StopReason::Cancelled,
                });
                channels.send_event(AgentEvent::RunFinished {
                    turn: 1,
                    stop_reason: StopReason::Cancelled,
                });
                if let Some(finished_tx) = finished_tx.lock().expect("finished lock").take() {
                    let _ = finished_tx.send(());
                }
                Ok(())
            })
        });
        let mut controller = InteractiveController::new_with_turn_driver(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            run_turn,
            PickerCatalogs::default(),
            Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        );

        controller.type_text("cancel me");
        controller
            .run_terminal_loop(
                |_app| Ok(()),
                ScriptedEvents {
                    events: VecDeque::from([
                        Some(InputEvent::Submit),
                        None,
                        Some(InputEvent::Interrupt),
                    ]),
                },
            )
            .await
            .expect("interrupt exits terminal loop after draining cancellation");

        tokio::time::timeout(Duration::from_secs(1), finished_rx)
            .await
            .expect("turn driver should finish after cancellation")
            .expect("finished sender should not be dropped before sending");
        let token = captured_token
            .lock()
            .expect("token lock")
            .clone()
            .expect("turn token captured");
        assert!(token.is_cancelled());
        assert_eq!(controller.app().mode(), neo_tui::AppMode::Editing);
        assert_eq!(controller.app().active_assistant_id(), None);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
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
        controller
            .wait_for_active_turn()
            .await
            .expect("continued turn completes");
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
        controller
            .wait_for_active_turn()
            .await
            .expect("continued fork turn completes");
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
        controller
            .wait_for_active_turn()
            .await
            .expect("selected model turn completes");

        let requests = requests.lock().expect("recorded requests");
        assert_eq!(requests.len(), 1);
        let selected = requests[0].model.as_ref().expect("selected model");
        assert_eq!(selected.provider, "anthropic");
        assert_eq!(selected.model, "claude-sonnet-4-5");
        assert_eq!(requests[0].session_id, None);
    }

    #[test]
    fn model_catalog_for_config_applies_cli_models_scope() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
        config.model_scope = vec!["sonnet".to_owned()];

        let catalog = model_catalog_for_config(&config);

        assert_eq!(catalog.error, None);
        assert!(!catalog.items.is_empty());
        assert!(
            catalog
                .items
                .iter()
                .all(|item| item.value.contains("sonnet"))
        );
        assert!(
            catalog
                .items
                .iter()
                .all(|item| !item.value.contains("openai/gpt-4.1"))
        );
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
            api_key: None,
            api_key_env: None,
            providers: BTreeMap::new(),
            model_catalogs: Vec::new(),
            model_scope: Vec::new(),
            model_selection: config::ModelSelection::Default,
            sessions_dir,
            permissions: PermissionPolicy::default(),
            defaults: Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: RuntimeConfig::default(),
            tui: TuiConfig::default(),
            mcp: McpConfig::default(),
            approve: false,
            no_approve: false,
            prompt_templates: Vec::new(),
            skill_paths: Vec::new(),
            configured_prompt_templates: Vec::new(),
            no_prompt_templates: false,
            no_context_files: false,
            system_prompt: None,
            append_system_prompt: Vec::new(),
            tool_filters: ToolFilterConfig::default(),
            project_trusted: true,
            project_dir: project_dir.to_path_buf(),
            config_path: project_dir.join(".neo/config.toml"),
        }
    }
}
