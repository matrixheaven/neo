use crate::{
    config::{self, AppConfig, neo_home, workspace_sessions_dir},
    modes::sessions::{SessionPickerScope as SessionDataScope, session_summaries},
    prompt_templates::{
        PromptTemplateLocation, discover_prompt_template_commands, expand_prompt_template_args,
        load_project_prompt_templates,
    },
    resources,
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    env, fs,
    future::{Future, Ready, ready},
    io::{IsTerminal as _, Write as _, stdout},
    path::{Path, PathBuf},
    pin::Pin,
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use crossterm::{event, terminal::size};
use neo_agent_core::{
    AgentEvent, AgentMessage, PendingQuestion, PermissionDecision, QuestionResponse,
    format_collected_answers,
    goal::GoalManager,
    session::{JsonlSessionReader, SessionMetadataStore, SessionSummary},
    skills::SkillStore,
};
use neo_tui::{
    chrome::{
        ApprovalChoice, ApprovalResult, CommandSpec, ContextWindow, NeoChromeState, OverlayKind,
        PickerItem, PromptEdit, SessionPickerItem, SessionPickerScope, StreamUpdate,
    },
    core::InputResult,
    image::{ImageProtocolPreference, ImageRenderPolicy, TerminalImageCapabilities},
    input::{InputEvent, InputParser, KeyId, KeybindingAction, KeybindingsManager},
    terminal::TuiRenderer,
    transcript::TranscriptPane,
};

use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

type BoxedTurnFuture = Pin<Box<dyn Future<Output = Result<TurnOutcome>> + Send + 'static>>;
type BoxedSessionFuture = Pin<Box<dyn Future<Output = Result<LoadedSessionTranscript>> + Send>>;
type BoxedForkFuture = Pin<Box<dyn Future<Output = Result<ForkedSessionTranscript>> + Send>>;
type TurnDriver = Arc<dyn Fn(TurnRequest, TurnChannels) -> BoxedTurnFuture + Send + Sync>;
type SessionLoader = Arc<dyn Fn(String) -> BoxedSessionFuture + Send + Sync>;
type SessionForker = Arc<dyn Fn(String) -> BoxedForkFuture + Send + Sync>;
type ClipboardWriter = Arc<dyn Fn(&str) -> Result<()> + Send + Sync>;

fn approval_number(character: char) -> Option<usize> {
    match character {
        '1' => Some(1),
        '2' => Some(2),
        '3' => Some(3),
        '4' => Some(4),
        _ => None,
    }
}

fn approval_result_label(choice: ApprovalChoice) -> &'static str {
    match choice {
        ApprovalChoice::Approve => "Approved",
        ApprovalChoice::AlwaysApprove => "Approved for this session",
        ApprovalChoice::Deny => "Rejected",
        ApprovalChoice::Revise => "Rejected with feedback",
    }
}

fn background_question_followup_prompt(task_id: &str) -> String {
    format!(
        "Background question `{task_id}` has been answered. Use TaskOutput with task_id `{task_id}` to read the answer, then continue the current work."
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupAction {
    None,
    OpenSessionPicker,
    LoadSession(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InteractiveOptions {
    pub verbose_startup: bool,
}

pub fn execute_with_startup(
    config: &AppConfig,
    startup: &StartupAction,
    options: InteractiveOptions,
) -> String {
    let mut controller = controller_for_config(config);
    controller.apply_startup_options(config, options);
    controller.apply_startup_action(startup);
    if let StartupAction::LoadSession(session_id) = startup {
        controller.push_status(format!(
            "Session resume requires a terminal (session {session_id})"
        ));
    }
    controller.render_snapshot()
}

pub async fn execute_tty_with_startup(
    config: &AppConfig,
    startup: StartupAction,
    options: InteractiveOptions,
) -> Result<Option<String>> {
    if !stdout().is_terminal() {
        return Ok(Some(execute_with_startup(config, &startup, options)));
    }

    let mut controller = controller_for_config(config);
    controller.apply_startup_options(config, options);
    if let StartupAction::LoadSession(session_id) = &startup {
        if let Err(error) = controller.load_session_at_startup(session_id).await {
            controller.push_status(format!("Failed to resume session: {error}"));
        }
    } else {
        controller.apply_startup_action(&startup);
    }
    let events = CrosstermEvents::new(controller.keybindings.clone());
    {
        let terminal = RefCell::new(NeoTerminal::enter()?);
        controller
            .run_terminal_loop_with_suspend(
                |tui| terminal.borrow_mut().draw_tui(tui),
                || terminal.borrow_mut().suspend(),
                events,
            )
            .await?;
    }
    Ok(Some(exit_message(controller.active_session_id())))
}

fn exit_message(session_id: Option<&str>) -> String {
    let mut message = String::from("Bye\n");
    if let Some(session_id) = session_id {
        message.push_str(&format!("neo resume {session_id}\n"));
    }
    message
}

struct PendingCustomRegistry {
    source: neo_tui::dialogs::CustomRegistrySource,
    catalog: BTreeMap<String, neo_ai::catalog::CatalogEntry>,
}

enum CatalogFetchSource {
    Known,
    Custom(neo_tui::dialogs::CustomRegistrySource),
}

struct PendingCatalogFetch {
    source: CatalogFetchSource,
    #[allow(clippy::type_complexity)]
    handle: tokio::task::JoinHandle<
        Result<BTreeMap<String, neo_ai::catalog::CatalogEntry>, neo_ai::error::AiError>,
    >,
}

pub(crate) struct InteractiveController {
    tui: neo_tui::NeoTui,
    keybindings: KeybindingsManager,
    run_turn: TurnDriver,
    session_items: Vec<SessionSummary>,
    session_list_error: Option<String>,
    model_items: Vec<PickerItem>,
    load_session: SessionLoader,
    fork_session: SessionForker,
    active_session_id: Option<String>,
    local_config: Option<AppConfig>,
    active_model: Option<SelectedModel>,
    current_thinking: bool,
    active_turn: Option<RunningTurn>,
    pending_approvals: BTreeMap<String, oneshot::Sender<PermissionDecision>>,
    resolved_approvals: BTreeMap<String, PermissionDecision>,
    /// Pending `AskUser` question response channels, keyed by question id.
    pending_questions: BTreeMap<String, oneshot::Sender<QuestionResponse>>,
    pending_question_prompts: BTreeMap<String, Vec<neo_agent_core::QuestionEventData>>,
    pending_background_question_followups: VecDeque<String>,
    clipboard_writer: ClipboardWriter,
    always_approve: bool,
    completion_root: PathBuf,
    workspace_root: PathBuf,
    pending_exit_confirmation: Option<ExitConfirmation>,
    suspend_requested: bool,
    pending_custom_registry: Option<PendingCustomRegistry>,
    pending_catalog_provider_id: Option<String>,
    pending_catalog_fetch: Option<PendingCatalogFetch>,
    skill_store: Option<neo_agent_core::skills::SkillStore>,
    /// Expanded skill body waiting to be injected as context for the next turn.
    pending_skill_context: Option<String>,
    goal_manager: Option<Arc<neo_agent_core::goal::GoalManager>>,
}

pub(crate) struct TurnChannels {
    events: mpsc::UnboundedSender<Result<AgentEvent>>,
    approvals: mpsc::UnboundedSender<crate::modes::run::PromptApprovalRequest>,
    session_ids: mpsc::UnboundedSender<String>,
    cancel_token: CancellationToken,
    /// Channel sender for `AskUserTool`'s reverse-RPC questions.
    questions: mpsc::UnboundedSender<PendingQuestion>,
}

#[cfg(test)]
impl TurnChannels {
    fn send_event(&self, event: AgentEvent) {
        let _ = self.events.send(Ok(event));
    }
}

struct RunningTurn {
    events: mpsc::UnboundedReceiver<Result<AgentEvent>>,
    approvals: mpsc::UnboundedReceiver<crate::modes::run::PromptApprovalRequest>,
    session_ids: mpsc::UnboundedReceiver<String>,
    task: JoinHandle<Result<TurnOutcome>>,
    cancel_token: CancellationToken,
    /// Receiver for `AskUserTool`'s reverse-RPC questions.
    questions: mpsc::UnboundedReceiver<PendingQuestion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitGesture {
    CtrlC,
    CtrlD,
}

#[derive(Debug, Clone, Copy)]
struct ExitConfirmation {
    gesture: ExitGesture,
    timestamp: Instant,
}

impl ExitConfirmation {
    const WINDOW: Duration = Duration::from_millis(500);

    fn new(gesture: ExitGesture) -> Self {
        Self {
            gesture,
            timestamp: Instant::now(),
        }
    }

    fn matches(self, gesture: ExitGesture) -> bool {
        self.gesture == gesture && self.timestamp.elapsed() <= Self::WINDOW
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PickerCatalogs {
    session_items: Vec<SessionSummary>,
    session_error: Option<String>,
    model_items: Vec<PickerItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnRequest {
    pub prompt: Vec<String>,
    pub session_id: Option<String>,
    pub model: Option<SelectedModel>,
    pub reasoning_effort: Option<neo_ai::ReasoningEffort>,
    /// Expanded skill body to inject as context before the user prompt.
    pub skill_context: Option<String>,
}

impl TurnRequest {
    #[must_use]
    pub(crate) fn new(
        prompt: Vec<String>,
        session_id: Option<String>,
        model: Option<SelectedModel>,
        reasoning_effort: Option<neo_ai::ReasoningEffort>,
    ) -> Self {
        Self {
            prompt,
            session_id,
            model,
            reasoning_effort,
            skill_context: None,
        }
    }

    #[must_use]
    pub(crate) fn with_skill_context(mut self, skill_context: impl Into<String>) -> Self {
        self.skill_context = Some(skill_context.into());
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TurnOutcome {
    session_id: Option<String>,
}

impl TurnOutcome {
    fn session(session_id: impl Into<String>) -> Self {
        Self {
            session_id: Some(session_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedModel {
    pub provider: String,
    pub model: String,
    pub max_context_tokens: Option<u32>,
}

impl SelectedModel {
    fn from_picker_item(item: &PickerItem) -> Result<Self> {
        let Some((provider, model)) = item.value.split_once('/') else {
            anyhow::bail!("invalid model picker value {}", item.value);
        };
        Ok(Self {
            provider: provider.to_owned(),
            model: model.to_owned(),
            max_context_tokens: context_window_from_picker_item(item),
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        title: impl Into<String>,
        session_label: impl Into<String>,
        model_label: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
        run_turn: TurnDriver,
        catalogs: PickerCatalogs,
        load_session: SessionLoader,
        fork_session: SessionForker,
    ) -> Self {
        let workspace_root = workspace_root.into();
        Self {
            tui: neo_tui::NeoTui::with_welcome_banner(
                NeoChromeState::new(title, session_label, model_label, workspace_root.clone()),
                80,
                24,
                env!("CARGO_PKG_VERSION"),
            ),
            keybindings: KeybindingsManager::default(),
            run_turn,
            session_items: catalogs.session_items,
            session_list_error: catalogs.session_error,
            model_items: catalogs.model_items,
            load_session,
            fork_session,
            active_session_id: None,
            local_config: None,
            active_model: None,
            current_thinking: false,
            active_turn: None,
            pending_approvals: BTreeMap::new(),
            resolved_approvals: BTreeMap::new(),
            pending_questions: BTreeMap::new(),
            pending_question_prompts: BTreeMap::new(),
            pending_background_question_followups: VecDeque::new(),
            clipboard_writer: Arc::new(write_system_clipboard),
            always_approve: false,
            completion_root: workspace_root.clone(),
            workspace_root,
            pending_exit_confirmation: None,
            suspend_requested: false,
            pending_custom_registry: None,
            pending_catalog_provider_id: None,
            pending_catalog_fetch: None,
            skill_store: None,
            pending_skill_context: None,
            goal_manager: None,
        }
    }

    #[cfg(test)]
    fn new_with_event_driver<RunTurn, TurnFut, LoadSession, LoadFut>(
        title: impl Into<String>,
        session_label: impl Into<String>,
        model_label: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
        run_turn: RunTurn,
        catalogs: PickerCatalogs,
        load_session: LoadSession,
    ) -> Self
    where
        RunTurn: Fn(TurnRequest) -> TurnFut + Send + Sync + 'static,
        TurnFut: Future<Output = Result<Vec<AgentEvent>>> + Send + 'static,
        LoadSession: Fn(String) -> LoadFut + Send + Sync + 'static,
        LoadFut: Future<Output = Result<LoadedSessionTranscript>> + Send + 'static,
    {
        Self::new_with_event_driver_and_forker(
            title,
            session_label,
            model_label,
            workspace_root,
            run_turn,
            catalogs,
            load_session,
            empty_session_forker,
        )
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    fn new_with_event_driver_and_forker<
        RunTurn,
        TurnFut,
        LoadSession,
        LoadFut,
        ForkSession,
        ForkFut,
    >(
        title: impl Into<String>,
        session_label: impl Into<String>,
        model_label: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
        run_turn: RunTurn,
        catalogs: PickerCatalogs,
        load_session: LoadSession,
        fork_session: ForkSession,
    ) -> Self
    where
        RunTurn: Fn(TurnRequest) -> TurnFut + Send + Sync + 'static,
        TurnFut: Future<Output = Result<Vec<AgentEvent>>> + Send + 'static,
        LoadSession: Fn(String) -> LoadFut + Send + Sync + 'static,
        LoadFut: Future<Output = Result<LoadedSessionTranscript>> + Send + 'static,
        ForkSession: Fn(String) -> ForkFut + Send + Sync + 'static,
        ForkFut: Future<Output = Result<ForkedSessionTranscript>> + Send + 'static,
    {
        let run_turn = Arc::new(run_turn);
        let driver: TurnDriver = Arc::new(move |request, channels| {
            let run_turn = Arc::clone(&run_turn);
            Box::pin(async move {
                let events = run_turn(request).await?;
                for event in events {
                    let _ = channels.events.send(Ok(event));
                }
                Ok(TurnOutcome::default())
            })
        });
        Self::new(
            title,
            session_label,
            model_label,
            workspace_root,
            driver,
            catalogs,
            Arc::new(move |session_id| Box::pin(load_session(session_id))),
            Arc::new(move |session_id| Box::pin(fork_session(session_id))),
        )
    }

    #[cfg(test)]
    fn new_for_test<RunTurn, Fut>(
        title: impl Into<String>,
        session_label: impl Into<String>,
        model_label: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
        run_turn: RunTurn,
    ) -> Self
    where
        RunTurn: Fn(TurnRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Vec<AgentEvent>>> + Send + 'static,
    {
        Self::new_with_event_driver(
            title,
            session_label,
            model_label,
            workspace_root,
            run_turn,
            PickerCatalogs::default(),
            empty_session_loader,
        )
    }

    #[allow(dead_code)]
    pub fn type_text(&mut self, text: &str) {
        self.tui
            .chrome_mut()
            .prompt_mut()
            .apply_edit(PromptEdit::Insert(text));
    }

    #[cfg(test)]
    fn set_clipboard_writer(&mut self, writer: ClipboardWriter) {
        self.clipboard_writer = writer;
    }

    #[cfg(test)]
    const fn chrome(&self) -> &NeoChromeState {
        self.tui.chrome()
    }

    /// Returns the filesystem path to the active config file, if known.
    fn config_path(&self) -> Option<PathBuf> {
        self.local_config.as_ref().map(|c| c.config_path.clone())
    }

    fn transcript_mut(&mut self) -> &mut TranscriptPane {
        self.tui.transcript_mut()
    }

    /// Reloads configuration from disk and refreshes all derived state.
    fn refresh_config(&mut self) {
        let Some(path) = self.config_path() else {
            return;
        };
        // Build minimal overrides pointing at the same config path.
        let overrides = crate::config::ConfigOverrides {
            config_path: Some(path),
            yolo: false,
        };
        match crate::config::AppConfig::load(overrides) {
            Ok(config) => {
                let catalogs = picker_catalogs_for_config(&config);
                self.session_items = catalogs.session_items;
                self.session_list_error = catalogs.session_error;
                self.model_items = catalogs.model_items;
                self.tui.chrome_mut().set_theme(config.theme.theme);
                self.local_config = Some(config);
            }
            Err(error) => {
                tracing::warn!("failed to reload config: {error}");
            }
        }
    }

    pub fn apply_startup_action(&mut self, startup: &StartupAction) {
        match startup {
            StartupAction::OpenSessionPicker => self.open_session_picker(),
            StartupAction::None | StartupAction::LoadSession(_) => {
                // `LoadSession` is loaded asynchronously before the terminal loop.
            }
        }
    }

    async fn load_session_at_startup(&mut self, session_id: &str) -> Result<()> {
        let loaded = (self.load_session)(session_id.to_owned())
            .await
            .with_context(|| format!("failed to load session {session_id}"))?;
        self.tui
            .chrome_mut()
            .set_session_label(loaded.label.clone());
        self.rebuild_transcript_from_session(&loaded);
        self.active_session_id = Some(session_id.to_owned());
        Ok(())
    }

    pub fn apply_startup_options(&mut self, config: &AppConfig, options: InteractiveOptions) {
        self.tui.chrome_mut().set_theme(config.theme.theme);
        self.tui
            .chrome_mut()
            .set_permission_decision(config.permissions.shell);
        self.tui
            .chrome_mut()
            .set_image_render_policy(ImageRenderPolicy::new(
                config.tui.image_protocol,
                config.tui.fetch_remote_images,
            ));
        self.tui
            .chrome_mut()
            .set_image_capabilities(terminal_image_capabilities_for_policy(
                config.tui.image_protocol,
                |name| env::var(name),
            ));
        if !options.verbose_startup {
            return;
        }
        self.push_status(startup_notices(config).join("\n"));
    }

    fn push_status(&mut self, message: impl Into<String>) {
        self.transcript_mut().push_status(message);
    }

    #[allow(dead_code)]
    pub async fn submit_prompt(&mut self) -> Result<String> {
        self.submit_current_prompt().await?;
        self.wait_for_active_turn().await?;
        Ok(self.render_snapshot())
    }

    #[cfg(test)]
    pub async fn run_terminal_loop(
        &mut self,
        mut render: impl FnMut(&mut NeoChromeState) -> Result<()>,
        events: impl TerminalEvents,
    ) -> Result<()> {
        self.run_terminal_loop_with_suspend(|tui| render(tui.chrome_mut()), || Ok(()), events)
            .await
    }

    async fn run_terminal_loop_with_suspend(
        &mut self,
        mut render: impl FnMut(&mut neo_tui::NeoTui) -> Result<()>,
        mut suspend: impl FnMut() -> Result<()>,
        mut events: impl TerminalEvents,
    ) -> Result<()> {
        render(&mut self.tui)?;
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
                            render(&mut self.tui)?;
                        }
                        break;
                    }
                    if self.take_suspend_requested() {
                        suspend()?;
                        render(&mut self.tui)?;
                    }
                }
                None => tokio::task::yield_now().await,
            }
            self.drain_active_turn().await?;
            self.poll_pending_catalog_fetch().await;
            self.tui.chrome_mut().advance_activity_frame();
            render(&mut self.tui)?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn handle_input_event(&mut self, event: InputEvent) -> Result<bool> {
        if self.tui.chrome().approval_is_pending() {
            if let Some(result) = self.tui.chrome_mut().handle_pending_approval_input(event) {
                self.resolve_approval(&result);
            } else {
                self.sync_inline_approval_selection();
            }
            return Ok(false);
        }
        // If a rich dialog overlay is focused, forward ALL input events to it
        // first. Rich dialogs consume keys, actions, submit, cancel, etc.
        if self.tui.chrome_mut().focused_overlay_is_rich_dialog() {
            let cancelled_question = self
                .tui
                .chrome()
                .question_dialog_state()
                .map(|state| state.id.clone());
            let event = self.dialog_input_event(event);
            let result = self.tui.chrome_mut().handle_focused_dialog_input(event);
            if result == InputResult::Cancelled
                && let Some(id) = cancelled_question
            {
                self.pending_questions.remove(&id);
                self.pending_question_prompts.remove(&id);
            }
            self.process_rich_dialog_result(result).await?;
            return Ok(false);
        }
        match event {
            InputEvent::Insert(character) => {
                if let Some(number) = approval_number(character)
                    && let Some(result) = self.tui.chrome_mut().choose_approval_number(number)
                {
                    self.resolve_approval(&result);
                    return Ok(false);
                }
                self.clear_pending_exit_confirmation();
                self.tui
                    .chrome_mut()
                    .prompt_mut()
                    .apply_edit(PromptEdit::Insert(&character.to_string()));
                self.sync_inline_prompt_completion();
            }
            InputEvent::Paste(text) => {
                self.clear_pending_exit_confirmation();
                self.tui
                    .chrome_mut()
                    .prompt_mut()
                    .apply_edit(PromptEdit::Insert(&text));
                self.sync_inline_prompt_completion();
            }
            InputEvent::Key(key) => return self.handle_keybinding_key(&key).await,
            InputEvent::Action(action) => return self.handle_keybinding_action(action).await,
            InputEvent::Backspace => {
                self.clear_pending_exit_confirmation();
                self.tui
                    .chrome_mut()
                    .prompt_mut()
                    .apply_edit(PromptEdit::Backspace);
                self.sync_inline_prompt_completion();
            }
            InputEvent::Delete => {
                self.clear_pending_exit_confirmation();
                self.tui
                    .chrome_mut()
                    .prompt_mut()
                    .apply_edit(PromptEdit::Delete);
                self.sync_inline_prompt_completion();
            }
            InputEvent::MoveLeft => {
                self.clear_pending_exit_confirmation();
                self.tui
                    .chrome_mut()
                    .prompt_mut()
                    .apply_edit(PromptEdit::MoveLeft);
                self.sync_inline_prompt_completion();
            }
            InputEvent::MoveRight => {
                self.clear_pending_exit_confirmation();
                self.tui
                    .chrome_mut()
                    .prompt_mut()
                    .apply_edit(PromptEdit::MoveRight);
                self.sync_inline_prompt_completion();
            }
            InputEvent::MoveHome => {
                self.clear_pending_exit_confirmation();
                self.tui
                    .chrome_mut()
                    .prompt_mut()
                    .apply_edit(PromptEdit::MoveHome);
                self.sync_inline_prompt_completion();
            }
            InputEvent::MoveEnd => {
                self.clear_pending_exit_confirmation();
                self.tui
                    .chrome_mut()
                    .prompt_mut()
                    .apply_edit(PromptEdit::MoveEnd);
                self.sync_inline_prompt_completion();
            }
            InputEvent::NewLine => {
                self.clear_pending_exit_confirmation();
                self.tui
                    .chrome_mut()
                    .prompt_mut()
                    .apply_edit(PromptEdit::Insert("\n"));
                self.sync_inline_prompt_completion();
            }
            InputEvent::Submit => {
                self.clear_pending_exit_confirmation();
                self.submit_current_prompt().await?;
            }
            InputEvent::ScrollUp(rows) => {
                self.clear_pending_exit_confirmation();
                self.transcript_mut().scroll_transcript_up(rows);
            }
            InputEvent::ScrollDown(rows) => {
                self.clear_pending_exit_confirmation();
                self.transcript_mut().scroll_transcript_down(rows);
            }
            InputEvent::Resize { .. } => {}
            InputEvent::Cancel => {
                if self.reject_pending_approval() {
                    return Ok(false);
                }
                if self.cancel_focused_overlay() {
                    return Ok(false);
                }
                let _ = self.interrupt_active_or_stale_turn().await?;
                // When idle, ESC is a no-op (never exits the app).
                return Ok(false);
            }
            InputEvent::Interrupt => {
                if self.reject_all_pending_approvals() {
                    if self.active_turn.is_some() {
                        self.cancel_active_turn().await?;
                        self.show_notice("Interrupted");
                    }
                    return Ok(false);
                }
                if self.interrupt_active_or_stale_turn().await? {
                    return Ok(false);
                }
                return Ok(self.handle_app_clear());
            }
        }

        Ok(false)
    }

    async fn handle_keybinding_key(&mut self, key: &KeyId) -> Result<bool> {
        let actions = self.keybindings.matching_actions(key);
        for action in self.keybinding_priority() {
            if *action == KeybindingAction::TranscriptCopySelection
                && !self.tui.transcript().has_transcript_selection()
            {
                continue;
            }
            if actions.contains(action) {
                return self.handle_keybinding_action(*action).await;
            }
        }

        Ok(false)
    }

    fn dialog_input_event(&self, event: InputEvent) -> InputEvent {
        let InputEvent::Key(key) = event else {
            return event;
        };
        let actions = self.keybindings.matching_actions(&key);
        self.keybinding_priority()
            .iter()
            .copied()
            .find(|action| actions.contains(action))
            .map_or(InputEvent::Key(key), InputEvent::Action)
    }

    fn keybinding_priority(&self) -> &'static [KeybindingAction] {
        if self.tui.chrome().question_dialog_is_focused() {
            QUESTION_ACTION_PRIORITY
        } else if self
            .tui
            .chrome()
            .focused_overlay()
            .is_some_and(|overlay| matches!(overlay.kind, OverlayKind::PromptCompletion(_)))
        {
            PROMPT_COMPLETION_ACTION_PRIORITY
        } else if self.tui.chrome().approval_is_pending()
            || self.tui.chrome().focused_overlay_id().is_some()
        {
            OVERLAY_ACTION_PRIORITY
        } else {
            EDITING_ACTION_PRIORITY
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn handle_keybinding_action(&mut self, action: KeybindingAction) -> Result<bool> {
        if self.handle_prompt_keybinding_action(action) {
            return Ok(false);
        }
        if self.handle_transcript_keybinding_action(action) {
            return Ok(false);
        }

        match action {
            KeybindingAction::InputNewLine => {
                self.clear_pending_exit_confirmation();
                self.tui
                    .chrome_mut()
                    .prompt_mut()
                    .apply_edit(PromptEdit::Insert("\n"));
            }
            KeybindingAction::InputTab => self.complete_prompt_or_insert_tab(),
            KeybindingAction::InputCopy => self.copy_prompt_to_clipboard(),
            KeybindingAction::AppClear => {
                if self.interrupt_active_or_stale_turn().await? {
                    return Ok(false);
                }
                return Ok(self.handle_app_clear());
            }
            KeybindingAction::AppExit => return Ok(self.handle_app_exit()),
            KeybindingAction::AppSuspend => {
                self.suspend_requested = true;
            }
            KeybindingAction::CommandPaletteOpen => self.open_command_palette(),
            KeybindingAction::SessionPickerOpen => {
                self.open_session_picker();
            }
            KeybindingAction::SessionPickerToggleScope => {
                self.toggle_session_picker_scope();
            }
            KeybindingAction::SessionFork => {
                if self.tui.chrome_mut().selected_session().is_some() {
                    self.fork_selected_session().await?;
                }
            }
            KeybindingAction::ToolOutputToggle => {
                self.transcript_mut().toggle_tool_output_expanded();
            }
            KeybindingAction::ModelPickerOpen => {
                self.open_model_picker();
            }
            KeybindingAction::TogglePlanMode => {
                let currently_active = self.tui.chrome_mut().is_plan_mode();
                self.tui.chrome_mut().set_plan_mode(!currently_active);
                self.push_status(if currently_active {
                    "Exited plan mode"
                } else {
                    " Entered plan mode — read-only until you exit"
                });
            }
            KeybindingAction::InputSubmit => {
                self.clear_pending_exit_confirmation();
                self.submit_current_prompt().await?;
            }
            KeybindingAction::SelectUp => {
                self.tui.chrome_mut().move_overlay_selection_up();
                self.sync_inline_approval_selection();
            }
            KeybindingAction::SelectDown => {
                self.tui.chrome_mut().move_overlay_selection_down();
                self.sync_inline_approval_selection();
            }
            KeybindingAction::SelectPageUp => {
                self.tui.chrome_mut().move_overlay_selection_page_up();
            }
            KeybindingAction::SelectPageDown => {
                self.tui.chrome_mut().move_overlay_selection_page_down();
            }
            KeybindingAction::SelectConfirm => {
                if self.tui.chrome_mut().selected_command().is_some() {
                    self.run_selected_command().await?;
                } else if self.tui.chrome_mut().approval_choice().is_some() {
                    if let Some(result) = self.tui.chrome_mut().confirm_approval() {
                        self.resolve_approval(&result);
                    }
                } else if self.tui.chrome_mut().selected_session().is_some() {
                    self.load_selected_session().await?;
                } else if self.tui.chrome_mut().selected_model().is_some() {
                    self.apply_selected_model();
                } else if self.tui.chrome_mut().selected_prompt_completion().is_some() {
                    let _ = self.tui.chrome_mut().confirm_prompt_completion();
                } else if self.tui.chrome_mut().focused_overlay_id().is_none() {
                    self.submit_current_prompt().await?;
                }
            }
            KeybindingAction::SelectCancel => {
                if self.reject_pending_approval() {
                    return Ok(false);
                }
                if self.cancel_focused_overlay() {
                    return Ok(false);
                }
                let _ = self.interrupt_active_or_stale_turn().await?;
                // When idle, ESC is a no-op (never exits the app).
            }
            KeybindingAction::EditorCursorUp | KeybindingAction::EditorCursorDown => {
                unreachable!("prompt history actions are handled before overlay actions")
            }
            KeybindingAction::EditorPageUp => self.transcript_mut().scroll_transcript_up(8),
            KeybindingAction::EditorPageDown => self.transcript_mut().scroll_transcript_down(8),
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

    fn cancel_focused_overlay(&mut self) -> bool {
        // Check if the focused overlay is a question dialog and handle its
        // cancellation (drops response_tx → AskUserTool gets "cancelled").
        if self.tui.chrome_mut().question_dialog_is_focused() {
            if let Some(id) = self.tui.chrome_mut().cancel_question() {
                self.pending_questions.remove(&id);
                self.pending_question_prompts.remove(&id);
            }
            return true;
        }
        let Some(overlay) = self.tui.chrome_mut().close_focused_overlay() else {
            return false;
        };
        if let OverlayKind::Approval(modal) = overlay.kind {
            self.resolve_approval(&ApprovalResult {
                request_id: modal.request_id,
                choice: ApprovalChoice::Deny,
                feedback: None,
            });
        }
        true
    }

    fn reject_pending_approval(&mut self) -> bool {
        let Some(result) = self.tui.chrome_mut().deny_approval() else {
            return false;
        };
        self.resolve_approval(&result);
        true
    }

    fn reject_all_pending_approvals(&mut self) -> bool {
        let chrome_results = self.tui.chrome_mut().cancel_all_approvals();
        let had_pending = !chrome_results.is_empty() || !self.pending_approvals.is_empty();
        for result in chrome_results {
            self.resolve_approval(&result);
        }
        for (request_id, tx) in std::mem::take(&mut self.pending_approvals) {
            self.tui
                .transcript_mut()
                .resolve_approval(&request_id, "Rejected");
            let _ = tx.send(PermissionDecision::Deny);
        }
        self.tui
            .transcript_mut()
            .resolve_unresolved_approvals("Rejected");
        had_pending
    }

    async fn interrupt_active_or_stale_turn(&mut self) -> Result<bool> {
        if self.active_turn.is_some() {
            self.cancel_active_turn().await?;
            self.show_notice("Interrupted");
            return Ok(true);
        }
        if self.clear_stale_streaming_turn() {
            self.show_notice("Interrupted");
            return Ok(true);
        }
        Ok(false)
    }

    fn clear_stale_streaming_turn(&mut self) -> bool {
        if self.tui.chrome().mode() != neo_tui::chrome::ChromeMode::Streaming {
            return false;
        }
        self.clear_interrupted_turn_state();
        true
    }

    fn clear_interrupted_turn_state(&mut self) {
        self.clear_pending_exit_confirmation();
        for id in self.tui.chrome_mut().clear_interrupted_turn_state() {
            self.pending_questions.remove(&id);
            self.pending_question_prompts.remove(&id);
        }
        self.pending_questions.clear();
        self.pending_question_prompts.clear();
        self.pending_background_question_followups.clear();
    }

    fn complete_prompt_or_insert_tab(&mut self) {
        self.clear_pending_exit_confirmation();
        if self.tui.chrome_mut().selected_prompt_completion().is_some() {
            let _ = self.tui.chrome_mut().confirm_prompt_completion();
            return;
        }
        let Some(prefix) = self.tui.chrome_mut().prompt().completion_prefix() else {
            self.tui
                .chrome_mut()
                .prompt_mut()
                .apply_edit(PromptEdit::Insert("\t"));
            return;
        };
        let completions = match prompt_completions(
            &self.completion_root,
            &prefix.text,
            &self.model_items,
            self.skill_store.as_ref(),
        ) {
            Ok(completions) => completions,
            Err(error) => {
                self.push_status(format!("Completion error: {error}"));
                return;
            }
        };

        if completions.is_empty() {
            self.tui
                .chrome_mut()
                .prompt_mut()
                .apply_edit(PromptEdit::Insert("\t"));
            return;
        }

        if let Some(common_prefix) = longest_common_completion_prefix(&completions)
            && common_prefix.chars().count() > prefix.text.chars().count()
        {
            let _ = self
                .tui
                .chrome_mut()
                .prompt_mut()
                .replace_completion_prefix(&prefix, &common_prefix);
            return;
        }

        if completions.len() == 1 {
            let _ = self
                .tui
                .chrome_mut()
                .prompt_mut()
                .replace_completion_prefix(&prefix, &completions[0].value);
            return;
        }

        self.tui
            .chrome_mut()
            .open_prompt_completion_picker(prefix, completions);
    }

    fn sync_inline_prompt_completion(&mut self) {
        let Some(prefix) = self.tui.chrome_mut().prompt().completion_prefix() else {
            self.close_inline_prompt_completion();
            return;
        };

        if !prefix.text.starts_with('/') {
            self.close_inline_prompt_completion();
            return;
        }

        let completions = match prompt_completions(
            &self.completion_root,
            &prefix.text,
            &self.model_items,
            self.skill_store.as_ref(),
        ) {
            Ok(completions) => completions,
            Err(error) => {
                self.close_inline_prompt_completion();
                self.push_status(format!("Completion error: {error}"));
                return;
            }
        };

        if completions.is_empty() {
            self.close_inline_prompt_completion();
            return;
        }

        let focused_is_prompt_completion = self
            .tui
            .chrome_mut()
            .focused_overlay()
            .is_some_and(|overlay| matches!(overlay.kind, OverlayKind::PromptCompletion(_)));
        if focused_is_prompt_completion {
            let _ = self.tui.chrome_mut().close_focused_overlay();
        } else if self.tui.chrome_mut().focused_overlay_id().is_some() {
            return;
        }

        self.tui
            .chrome_mut()
            .open_prompt_completion_picker(prefix, completions);
    }

    fn close_inline_prompt_completion(&mut self) {
        if self
            .tui
            .chrome_mut()
            .focused_overlay()
            .is_some_and(|overlay| matches!(overlay.kind, OverlayKind::PromptCompletion(_)))
        {
            let _ = self.tui.chrome_mut().close_focused_overlay();
        }
    }

    fn handle_prompt_keybinding_action(&mut self, action: KeybindingAction) -> bool {
        match action {
            KeybindingAction::EditorCursorUp => {
                self.clear_pending_exit_confirmation();
                self.tui.chrome_mut().prompt_mut().recall_previous_history();
                self.sync_inline_prompt_completion();
                return true;
            }
            KeybindingAction::EditorCursorDown => {
                self.clear_pending_exit_confirmation();
                self.tui.chrome_mut().prompt_mut().recall_next_history();
                self.sync_inline_prompt_completion();
                return true;
            }
            _ => {}
        }

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
        self.clear_pending_exit_confirmation();
        self.tui.chrome_mut().prompt_mut().apply_edit(edit);
        self.sync_inline_prompt_completion();
        true
    }

    fn handle_transcript_keybinding_action(&mut self, action: KeybindingAction) -> bool {
        match action {
            KeybindingAction::TranscriptSelectionStart => {
                self.transcript_mut().select_visible_transcript_entry();
            }
            KeybindingAction::TranscriptSelectionClear => {
                self.transcript_mut().clear_transcript_selection();
            }
            KeybindingAction::TranscriptSelectionExtendUp => {
                self.transcript_mut().extend_transcript_selection_up(1);
            }
            KeybindingAction::TranscriptSelectionExtendDown => {
                self.transcript_mut().extend_transcript_selection_down(1);
            }
            KeybindingAction::TranscriptSelectionExtendPageUp => {
                self.transcript_mut().extend_transcript_selection_up(8);
            }
            KeybindingAction::TranscriptSelectionExtendPageDown => {
                self.transcript_mut().extend_transcript_selection_down(8);
            }
            KeybindingAction::TranscriptCopySelection => {
                self.copy_transcript_selection_to_clipboard();
            }
            _ => return false,
        }
        true
    }

    fn copy_prompt_to_clipboard(&mut self) {
        let Some(copied) = self.tui.chrome_mut().copy_prompt_text() else {
            return;
        };
        self.write_clipboard_text(&copied);
    }

    fn copy_transcript_selection_to_clipboard(&mut self) {
        let Some(copied) = self.transcript_mut().copy_selected_transcript_text() else {
            return;
        };
        self.write_clipboard_text(&copied);
    }

    fn write_clipboard_text(&mut self, copied: &str) {
        if let Err(error) = (self.clipboard_writer)(copied) {
            self.push_status(format!("Clipboard copy failed: {error}"));
        }
    }

    fn handle_app_clear(&mut self) -> bool {
        if self.tui.transcript().has_transcript_selection() {
            self.copy_transcript_selection_to_clipboard();
            self.clear_pending_exit_confirmation();
            return false;
        }
        if !self.tui.chrome_mut().prompt().text.is_empty() {
            self.tui
                .chrome_mut()
                .prompt_mut()
                .apply_edit(PromptEdit::Clear);
        }
        self.handle_exit_confirmation(ExitGesture::CtrlC)
    }

    fn handle_app_exit(&mut self) -> bool {
        if self.tui.chrome_mut().prompt().text.is_empty() {
            return self.handle_exit_confirmation(ExitGesture::CtrlD);
        }
        self.clear_pending_exit_confirmation();
        self.tui
            .chrome_mut()
            .prompt_mut()
            .apply_edit(PromptEdit::Delete);
        false
    }

    fn handle_exit_confirmation(&mut self, gesture: ExitGesture) -> bool {
        if self
            .pending_exit_confirmation
            .as_ref()
            .is_some_and(|confirmation| confirmation.matches(gesture))
        {
            self.pending_exit_confirmation = None;
            self.tui.chrome_mut().set_exit_confirmation_label(None);
            return true;
        }
        let message = match gesture {
            ExitGesture::CtrlC => "Press Ctrl-C again to exit",
            ExitGesture::CtrlD => "Press Ctrl-D again to exit",
        };
        self.tui
            .chrome_mut()
            .set_exit_confirmation_label(Some(message.to_owned()));
        self.pending_exit_confirmation = Some(ExitConfirmation::new(gesture));
        false
    }

    fn show_notice(&mut self, message: impl Into<String>) {
        self.push_status(message);
    }

    fn clear_pending_exit_confirmation(&mut self) {
        self.pending_exit_confirmation = None;
        self.tui.chrome_mut().set_exit_confirmation_label(None);
    }

    fn open_command_palette(&mut self) {
        let (commands, error) = command_specs(&self.completion_root);
        if let Some(error) = error {
            self.push_status(format!("Error loading prompt templates: {error}"));
        }
        self.tui.chrome_mut().open_command_palette(commands);
    }

    async fn run_selected_command(&mut self) -> Result<()> {
        let Some(command) = self.tui.chrome_mut().confirm_command_palette() else {
            return Ok(());
        };
        if let Some(name) = command.id.strip_prefix("prompt-template.") {
            self.tui
                .chrome_mut()
                .prompt_mut()
                .apply_edit(PromptEdit::Insert(&format!("/{name} ")));
            return Ok(());
        }

        match command.id.as_str() {
            "sessions" => self.open_session_picker(),
            "models" => self.open_model_picker(),
            "providers" => self.open_provider_picker(),
            "copy-prompt" => self.copy_prompt_to_clipboard(),
            "select-transcript" => self.transcript_mut().select_visible_transcript_entry(),
            "clear-transcript-selection" => self.transcript_mut().clear_transcript_selection(),
            "copy-transcript-selection" => self.copy_transcript_selection_to_clipboard(),
            "session.exportHtml" => self.export_active_session_to_html().await?,
            "fork" => self.fork_current_session().await?,
            "plan" => {
                // Toggle is handled in submit_current_prompt where we have
                // access to the raw prompt text for /plan on|off|clear parsing.
                // This path is for bare command invocation via picker.
                let currently_active = self.tui.chrome_mut().is_plan_mode();
                self.tui.chrome_mut().set_plan_mode(!currently_active);
                self.push_status(if currently_active {
                    "Exited plan mode"
                } else {
                    " Entered plan mode — read-only until you exit"
                });
            }
            "submit" => self.submit_current_prompt().await?,
            unknown => self.push_status(format!("Unknown command: {unknown}")),
        }
        Ok(())
    }

    async fn export_active_session_to_html(&mut self) -> Result<()> {
        let Some(session_id) = self.active_session_id.clone() else {
            self.push_status("No active session to export");
            return Ok(());
        };
        let config = self
            .local_config
            .clone()
            .context("session HTML export is unavailable")?;
        let html = crate::modes::sessions::export_html(&session_id, &config).await?;
        let output_path =
            crate::modes::sessions::session_path(&session_id, &config)?.with_extension("html");
        fs::write(&output_path, html)
            .with_context(|| format!("failed to write {}", output_path.display()))?;
        self.push_status(format!(
            "Exported session {session_id} to {}",
            output_path.display()
        ));
        Ok(())
    }

    /// Handle slash commands. Returns `true` if the prompt was consumed and should
    /// not be submitted as a chat turn.
    async fn handle_slash_command(&mut self, prompt: &str) -> bool {
        match prompt.trim() {
            "/resume" => {
                self.tui.chrome_mut().prompt_mut().clear_after_submit();
                self.open_session_picker();
                return true;
            }
            "/provider" => {
                self.tui.chrome_mut().prompt_mut().clear_after_submit();
                self.open_provider_picker();
                return true;
            }
            _ => {}
        }
        // /model [alias] — opens model picker, optionally pre-selected
        if prompt.trim() == "/model" || prompt.trim().starts_with("/model ") {
            self.tui.chrome_mut().prompt_mut().clear_after_submit();
            let alias = prompt.trim().strip_prefix("/model").unwrap_or("").trim();
            if alias.is_empty() {
                self.open_model_picker();
            } else if self.model_items.iter().any(|item| item.value == alias) {
                self.open_model_picker_with_alias(alias);
            } else {
                self.push_status(format!("Error: Unknown model alias: {alias}"));
            }
            return true;
        }
        if let Some(rest) = prompt.trim().strip_prefix("/skill:") {
            self.tui.chrome_mut().prompt_mut().clear_after_submit();
            let arg = rest.trim();
            if arg.is_empty() {
                self.push_status("Usage: /skill:<name> [args]");
            } else if let Err(err) = self.handle_skill_invocation(arg) {
                self.push_status(format!("Skill error: {err}"));
            }
            return true;
        }
        if let Some(rest) = prompt.trim().strip_prefix("/plan") {
            self.tui.chrome_mut().prompt_mut().clear_after_submit();
            let arg = rest.trim();
            match arg {
                "" | "on" => {
                    if self.tui.chrome_mut().is_plan_mode() {
                        self.push_status("Plan mode is already active");
                    } else {
                        self.tui.chrome_mut().set_plan_mode(true);
                        self.push_status(" Entered plan mode — read-only until you exit");
                    }
                }
                "off" => {
                    if self.tui.chrome_mut().is_plan_mode() {
                        self.tui.chrome_mut().set_plan_mode(false);
                        self.push_status("Exited plan mode");
                    } else {
                        self.push_status("Plan mode is not active");
                    }
                }
                "clear" => {
                    let plans_dir = self.workspace_root.join(".neo").join("plans");
                    let cleared = std::fs::read_dir(&plans_dir)
                        .ok()
                        .and_then(|mut entries| entries.next().and_then(std::result::Result::ok))
                        .and_then(|entry| std::fs::write(entry.path(), "").ok())
                        .is_some();
                    self.push_status(if cleared {
                        "Plan file cleared"
                    } else {
                        "No plan file to clear"
                    });
                }
                _ => {
                    self.push_status(format!(
                        "Unknown /plan argument: '{arg}'. Usage: /plan [on|off|clear]"
                    ));
                }
            }
            return true;
        }
        if let Some(rest) = prompt.trim().strip_prefix("/goal") {
            self.tui.chrome_mut().prompt_mut().clear_after_submit();
            return self.handle_goal_command(rest.trim()).await;
        }
        false
    }

    #[allow(clippy::too_many_lines)]
    async fn handle_goal_command(&mut self, arg: &str) -> bool {
        if self.goal_manager.is_none()
            && let Some(home) = neo_home()
        {
            match GoalManager::load(home).await {
                Ok(manager) => self.goal_manager = Some(Arc::new(manager)),
                Err(err) => {
                    self.push_status(format!("Failed to load goal manager: {err}"));
                    return true;
                }
            }
        }
        let Some(manager) = self.goal_manager.clone() else {
            self.push_status("Goal mode is not available");
            return true;
        };
        match arg {
            "" | "status" => {
                match manager.active() {
                    Some(goal) => self.push_status(format!(
                        "Goal: {} | status: {:?} | turns: {}",
                        goal.objective, goal.status, goal.turn_count
                    )),
                    None => self.push_status("No active goal."),
                }
                true
            }
            "pause" => {
                match manager.pause().await {
                    Ok(Some(goal)) => {
                        self.transcript_mut().push_transcript(
                            neo_tui::transcript::TranscriptEntry::status(format!(
                                "⏸ Goal paused: {}",
                                goal.objective
                            )),
                        );
                    }
                    Ok(None) => self.push_status("No active goal to pause"),
                    Err(err) => self.push_status(format!("Failed to pause goal: {err}")),
                }
                true
            }
            "resume" => {
                match manager.resume().await {
                    Ok(Some(goal)) => {
                        self.transcript_mut().push_transcript(
                            neo_tui::transcript::TranscriptEntry::status(format!(
                                "▶ Goal resumed: {}",
                                goal.objective
                            )),
                        );
                    }
                    Ok(None) => self.push_status("No active goal to resume"),
                    Err(err) => self.push_status(format!("Failed to resume goal: {err}")),
                }
                true
            }
            "cancel" => {
                match manager.cancel().await {
                    Ok(Some(goal)) => {
                        self.transcript_mut().push_transcript(
                            neo_tui::transcript::TranscriptEntry::status(format!(
                                "⏹ Goal cancelled: {}",
                                goal.objective
                            )),
                        );
                    }
                    Ok(None) => self.push_status("No active goal to cancel"),
                    Err(err) => self.push_status(format!("Failed to cancel goal: {err}")),
                }
                true
            }
            _ => {
                let trimmed = arg.trim();
                if let Some(objective) = trimmed.strip_prefix("replace ") {
                    let goal = neo_agent_core::goal::Goal::new(objective.trim());
                    match manager.replace(goal).await {
                        Ok(Some(_previous)) => {
                            self.push_status(format!("Replaced goal with: {objective}"));
                        }
                        Ok(None) => self.push_status(format!("Started goal: {objective}")),
                        Err(err) => {
                            self.push_status(format!("Failed to replace goal: {err}"));
                            return true;
                        }
                    }
                    false
                } else if let Some(objective) = trimmed.strip_prefix("next ") {
                    let goal = neo_agent_core::goal::Goal::new(objective.trim());
                    match manager.queue_next(goal).await {
                        Ok(()) => self.push_status(format!("Queued goal: {objective}")),
                        Err(err) => {
                            self.push_status(format!("Failed to queue goal: {err}"));
                            return true;
                        }
                    }
                    true
                } else {
                    let goal = neo_agent_core::goal::Goal::new(trimmed);
                    let objective = goal.objective.clone();
                    match manager.start(goal).await {
                        Ok(Some(_previous)) => {
                            self.push_status(format!(
                                "Started goal: {objective} (previous goal superseded)"
                            ));
                        }
                        Ok(None) => self.push_status(format!("Started goal: {objective}")),
                        Err(err) => {
                            self.push_status(format!("Failed to start goal: {err}"));
                            return true;
                        }
                    }
                    self.transcript_mut().push_transcript(
                        neo_tui::transcript::TranscriptEntry::status(format!(
                            "▶ Goal started: {objective}"
                        )),
                    );
                    false
                }
            }
        }
    }

    fn handle_skill_invocation(&mut self, arg: &str) -> Result<()> {
        let skill_store = self
            .skill_store
            .as_ref()
            .context("skill store not loaded")?;
        let (name, args_str) = match arg.find(' ') {
            Some(pos) => (&arg[..pos], &arg[pos + 1..]),
            None => (arg, ""),
        };
        let skill = skill_store
            .get(name)
            .with_context(|| format!("skill `{name}` not found"))?;
        let mut invocation = neo_agent_core::skills::parse_skill_invocation(args_str)
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        name.clone_into(&mut invocation.name);
        let expanded = neo_agent_core::skills::expand_skill_body(skill, &invocation)
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        self.transcript_mut()
            .push_transcript(neo_tui::transcript::TranscriptEntry::skill_activated(name));
        self.pending_skill_context = Some(expanded);
        let prompt = self.tui.chrome_mut().prompt_mut();
        args_str.clone_into(&mut prompt.text);
        prompt.cursor = prompt.text.chars().count();
        Ok(())
    }

    async fn submit_current_prompt(&mut self) -> Result<()> {
        if self.active_turn.is_some() {
            self.push_status("A turn is already running");
            return Ok(());
        }
        let prompt = self.tui.chrome_mut().prompt().text.trim_end().to_owned();
        if prompt.trim().is_empty() {
            return Ok(());
        }

        // Dismiss any open prompt-completion overlay before handling slash commands
        // or submitting, so it doesn't linger under a newly-opened picker.
        self.close_inline_prompt_completion();

        // Slash commands: handle without submitting a turn or entering streaming mode.
        if self.handle_slash_command(&prompt).await {
            return Ok(());
        }

        let Some(prompt) = self.tui.chrome_mut().submit_prompt() else {
            return Ok(());
        };
        let PromptSubmission {
            prompt,
            model_override,
        } = PromptSubmission::from_text(
            prompt,
            &self.model_items,
            self.local_config.as_ref(),
            &self.completion_root,
        )?;
        self.start_turn_with_prompt(prompt, model_override, true);
        self.drain_active_turn().await?;
        self.start_pending_background_question_followups().await
    }

    fn start_turn_with_prompt(
        &mut self,
        prompt: String,
        model_override: Option<SelectedModel>,
        show_user_message: bool,
    ) {
        if self.active_turn.is_some() {
            self.push_status("A turn is already running");
            return;
        }
        if show_user_message {
            self.tui.transcript_mut().push_user_message(prompt.clone());
        }
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (approval_tx, approval_rx) = mpsc::unbounded_channel();
        let (session_id_tx, session_id_rx) = mpsc::unbounded_channel();
        let (question_tx, question_rx) = mpsc::unbounded_channel::<PendingQuestion>();
        let cancel_token = CancellationToken::new();
        let channels = TurnChannels {
            events: event_tx.clone(),
            approvals: approval_tx,
            session_ids: session_id_tx,
            cancel_token: cancel_token.clone(),
            questions: question_tx,
        };
        let request = TurnRequest::new(
            vec![prompt],
            self.active_session_id.clone(),
            model_override.or_else(|| self.active_model.clone()),
            if self.current_thinking {
                Some(neo_ai::ReasoningEffort::High)
            } else {
                None
            },
        );
        let request = if let Some(skill_context) = self.pending_skill_context.take() {
            request.with_skill_context(skill_context)
        } else {
            request
        };
        let future = (self.run_turn)(request, channels);
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
            session_ids: session_id_rx,
            task,
            cancel_token,
            questions: question_rx,
        });
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
        self.pending_questions.clear();
        self.pending_question_prompts.clear();
        self.pending_background_question_followups.clear();
        let result = if let Ok(result) =
            tokio::time::timeout(Duration::from_secs(2), self.wait_for_active_turn()).await
        {
            result
        } else {
            self.abort_active_turn();
            Ok(())
        };
        self.clear_interrupted_turn_state();
        result
    }

    async fn drain_active_turn(&mut self) -> Result<()> {
        let Some(mut turn) = self.active_turn.take() else {
            return Ok(());
        };

        while let Ok(session_id) = turn.session_ids.try_recv() {
            self.set_active_session_id(session_id);
        }
        while let Ok(approval) = turn.approvals.try_recv() {
            self.register_pending_approval(approval);
        }
        while let Ok(pending) = turn.questions.try_recv() {
            self.register_pending_question(pending);
        }
        while let Ok(event) = turn.events.try_recv() {
            match event {
                Ok(event) => self.apply_turn_event(event),
                Err(error) => {
                    self.push_status(format!("Error: {error}"));
                }
            }
        }

        if turn.task.is_finished() {
            let turn_result = turn
                .task
                .await
                .map_err(|error| anyhow::anyhow!("interactive turn task failed: {error}"))?;
            while let Ok(session_id) = turn.session_ids.try_recv() {
                self.set_active_session_id(session_id);
            }
            while let Ok(approval) = turn.approvals.try_recv() {
                self.register_pending_approval(approval);
            }
            while let Ok(pending) = turn.questions.try_recv() {
                self.register_pending_question(pending);
            }
            while let Ok(event) = turn.events.try_recv() {
                match event {
                    Ok(event) => self.apply_turn_event(event),
                    Err(error) => {
                        self.push_status(format!("Error: {error}"));
                    }
                }
            }
            // Turn-driver errors are already forwarded through the event channel
            // and rendered into the transcript. Keep the interactive shell alive.
            match turn_result {
                Ok(outcome) => {
                    if let Some(session_id) = outcome.session_id {
                        self.set_active_session_id(session_id);
                    }
                }
                Err(error) => {
                    self.tui
                        .chrome_mut()
                        .apply_stream_update(StreamUpdate::Error {
                            text: error.to_string(),
                        });
                }
            }
        } else {
            self.active_turn = Some(turn);
        }
        Ok(())
    }

    fn set_active_session_id(&mut self, session_id: String) {
        self.active_session_id = Some(session_id.clone());
        self.tui.chrome_mut().set_session_label(session_id);
    }

    fn active_session_id(&self) -> Option<&str> {
        self.active_session_id.as_deref()
    }

    fn apply_turn_event(&mut self, event: AgentEvent) {
        if self.always_approve && matches!(event, AgentEvent::ApprovalRequested { .. }) {
            return;
        }
        if let AgentEvent::ToolExecutionFinished { name, result, .. } = &event
            && name == "TaskStop"
            && let Some(details) = &result.details
            && details.get("kind").and_then(serde_json::Value::as_str) == Some("question")
            && details.get("status").and_then(serde_json::Value::as_str) == Some("stopped")
            && let Some(task_id) = details.get("task_id").and_then(serde_json::Value::as_str)
        {
            let _ = self.tui.chrome_mut().close_question_overlay(task_id);
            self.pending_questions.remove(task_id);
            self.pending_question_prompts.remove(task_id);
        }
        self.tui.transcript_mut().apply_agent_event(event.clone());
        self.tui.chrome_mut().apply_agent_event(event);
    }

    fn sync_inline_approval_selection(&mut self) {
        let Some((id, selected, feedback_input)) = self.tui.chrome().approval_selection() else {
            return;
        };
        let id = id.to_owned();
        let feedback_input = feedback_input.to_owned();
        self.tui
            .transcript_mut()
            .select_approval(&id, selected, &feedback_input);
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
        self.tui
            .transcript_mut()
            .resolve_approval(&result.request_id, approval_result_label(result.choice));
        let decision = match result.choice {
            ApprovalChoice::Approve => PermissionDecision::Allow,
            ApprovalChoice::AlwaysApprove => {
                self.always_approve = true;
                PermissionDecision::Allow
            }
            ApprovalChoice::Deny | ApprovalChoice::Revise => PermissionDecision::Deny,
        };
        // Store revise feedback for the transcript to pick up when building the
        // exit_plan_mode tool result.
        if result.choice == ApprovalChoice::Revise
            && let Some(feedback) = &result.feedback
        {
            // Feedback is communicated via the approval subject side-channel:
            // we can't reach the runtime's plan_review_feedback map from here,
            // so we encode it in the resolved_approvals as a special marker.
            // The transcript will check plan_review_feedback; for now, the feedback
            // is passed through the TUI notice system.
            self.push_status(format!("Revision feedback: {feedback}"));
        }
        if let Some(tx) = self.pending_approvals.remove(&result.request_id) {
            let _ = tx.send(decision);
        } else {
            self.resolved_approvals
                .insert(result.request_id.clone(), decision);
        }
    }

    /// Register a pending `AskUser` question. Stores the oneshot response channel
    /// and synthesizes a `QuestionRequested` event for the TUI so it can display
    /// the question dialog.
    fn register_pending_question(&mut self, pending: PendingQuestion) {
        let id = pending.id.clone();
        let questions = pending.questions.clone();
        // Synthesize a QuestionRequested event for the TUI to display the dialog.
        // The TUI's apply_agent_event will push a question overlay (implemented by
        // the TUI subagent).
        self.tui
            .chrome_mut()
            .apply_agent_event(AgentEvent::QuestionRequested {
                turn: 0,
                id: id.clone(),
                questions: questions.clone(),
            });
        self.pending_questions
            .insert(id.clone(), pending.response_tx);
        self.pending_question_prompts.insert(id, questions);
    }

    /// Resolve a pending question by sending the user's answers through the
    /// stored oneshot channel.
    async fn resolve_question(&mut self, id: &str, answers: Vec<String>) -> Result<()> {
        if let Some(questions) = self.pending_question_prompts.remove(id) {
            self.transcript_mut()
                .push_transcript(neo_tui::transcript::TranscriptEntry::status(
                    format_collected_answers(&questions, &answers),
                ));
        }
        if let Some(tx) = self.pending_questions.remove(id) {
            let _ = tx.send(QuestionResponse { answers });
        }
        if id.starts_with("question-") {
            self.pending_background_question_followups
                .push_back(background_question_followup_prompt(id));
            self.start_pending_background_question_followups().await?;
        }
        Ok(())
    }

    async fn start_pending_background_question_followups(&mut self) -> Result<()> {
        while self.active_turn.is_none() {
            let Some(prompt) = self.pending_background_question_followups.pop_front() else {
                break;
            };
            self.start_turn_with_prompt(prompt, None, false);
            self.drain_active_turn().await?;
        }
        Ok(())
    }

    fn abort_active_turn(&mut self) {
        if let Some(turn) = self.active_turn.take() {
            turn.cancel_token.cancel();
            turn.task.abort();
        }
        self.pending_approvals.clear();
        self.resolved_approvals.clear();
        self.pending_questions.clear();
        self.pending_question_prompts.clear();
        self.pending_background_question_followups.clear();
        self.clear_interrupted_turn_state();
    }

    fn open_session_picker(&mut self) {
        self.open_session_picker_with_scope(SessionDataScope::Workspace);
    }

    fn open_session_picker_with_scope(&mut self, scope: SessionDataScope) {
        if let Some(error) = &self.session_list_error {
            self.push_status(format!("Error loading sessions: {error}"));
            return;
        }
        if self.session_items.is_empty() {
            self.push_status("No local sessions");
            return;
        }
        let current_session_id = self.active_session_id.clone().unwrap_or_default();
        let picker_scope = match scope {
            SessionDataScope::Workspace => SessionPickerScope::Workspace,
            SessionDataScope::All => SessionPickerScope::All,
        };
        let items: Vec<SessionPickerItem> = self
            .session_items
            .iter()
            .map(|summary| {
                let title = summary.title.clone().unwrap_or_else(|| summary.id.clone());
                SessionPickerItem::new(
                    summary.id.clone(),
                    title,
                    summary.last_prompt.clone(),
                    summary.work_dir.clone(),
                    parse_timestamp(&summary.updated_at),
                    summary.id == current_session_id,
                )
            })
            .collect();
        self.tui
            .chrome_mut()
            .open_session_picker(&current_session_id, picker_scope, items);
    }

    fn open_model_picker(&mut self) {
        let entries = self.model_entries_for_picker();
        if entries.is_empty() {
            self.push_status("No configured models");
            return;
        }
        let current_alias = self
            .active_model
            .as_ref()
            .map(|m| format!("{}/{}", m.provider, m.model))
            .unwrap_or_default();
        let theme = self.tui.chrome().theme();
        self.tui.chrome_mut().open_tabbed_model_selector(
            neo_tui::dialogs::TabbedModelSelectorOptions {
                models: entries,
                current_alias,
                selected_alias: None,
                current_thinking: self.current_thinking,
                initial_tab_id: None,
                theme,
            },
        );
    }

    /// Open the model picker with a specific alias pre-selected.
    fn open_model_picker_with_alias(&mut self, alias: &str) {
        let entries = self.model_entries_for_picker();
        if entries.is_empty() {
            self.push_status("No configured models");
            return;
        }
        let current_alias = self
            .active_model
            .as_ref()
            .map(|m| format!("{}/{}", m.provider, m.model))
            .unwrap_or_default();
        let initial_tab_id = entries
            .iter()
            .find(|e| e.alias == alias)
            .map(|e| e.provider_id.clone());
        let theme = self.tui.chrome().theme();
        self.tui.chrome_mut().open_tabbed_model_selector(
            neo_tui::dialogs::TabbedModelSelectorOptions {
                models: entries,
                current_alias,
                selected_alias: Some(alias.to_owned()),
                current_thinking: self.current_thinking,
                initial_tab_id,
                theme,
            },
        );
    }

    /// Resolve the ordered list of `ModelEntry` to show in the picker.
    /// Only providers/models explicitly configured via `[models.*]` are shown
    /// so the picker does not list providers the user has not set up.
    fn model_entries_for_picker(&self) -> Vec<neo_tui::dialogs::ModelEntry> {
        self.local_config
            .as_ref()
            .map_or_else(Vec::new, model_entries_from_config)
    }

    fn open_provider_picker(&mut self) {
        let Some(config) = &self.local_config else {
            self.push_status("No config available");
            return;
        };
        if config.providers.is_empty() {
            self.push_status("No configured providers");
            return;
        }
        // Build provider sources from config
        let active_provider_id = self.active_model.as_ref().map(|m| m.provider.clone());
        let sources: Vec<neo_tui::dialogs::ProviderSource> = config
            .providers
            .keys()
            .map(|id| {
                let label = id.clone();
                neo_tui::dialogs::ProviderSource {
                    provider_ids: vec![id.clone()],
                    label,
                    kind: neo_tui::dialogs::ProviderSourceKind::Inline,
                }
            })
            .collect();
        let theme = self.tui.chrome().theme();
        self.tui
            .chrome_mut()
            .open_provider_manager(&neo_tui::dialogs::ProviderManagerOptions {
                sources,
                active_provider_id,
                theme,
            });
    }

    async fn load_selected_session(&mut self) -> Result<()> {
        let Some(session) = self.tui.chrome_mut().confirm_session_picker() else {
            return Ok(());
        };

        if same_work_dir(&session.work_dir, &self.workspace_root) {
            let loaded = (self.load_session)(session.id.clone())
                .await
                .with_context(|| format!("failed to load session {}", session.id))?;
            self.tui
                .chrome_mut()
                .set_session_label(loaded.label.clone());
            self.rebuild_transcript_from_session(&loaded);
            self.active_session_id = Some(session.id);
            return Ok(());
        }

        let command = format!(
            "cd '{}' && neo --resume '{}'",
            session.work_dir.display(),
            session.id
        );
        self.push_status(command.clone());
        if let Err(error) = (self.clipboard_writer)(&command) {
            tracing::warn!("failed to copy resume command to clipboard: {error}");
        }
        Ok(())
    }

    fn toggle_session_picker_scope(&mut self) {
        let current_scope = {
            let Some(overlay) = self.tui.chrome_mut().focused_overlay() else {
                return;
            };
            let OverlayKind::SessionPicker(picker) = &overlay.kind else {
                return;
            };
            picker.scope()
        };
        let new_scope = match current_scope {
            SessionPickerScope::Workspace => SessionDataScope::All,
            SessionPickerScope::All => SessionDataScope::Workspace,
        };
        let Some(config) = self.local_config.as_ref() else {
            return;
        };
        match session_summaries(config, new_scope) {
            Ok(summaries) => {
                self.session_items = summaries;
                self.tui.chrome_mut().close_focused_overlay();
                self.open_session_picker_with_scope(new_scope);
            }
            Err(error) => {
                self.push_status(format!("Error loading sessions: {error}"));
            }
        }
    }

    async fn fork_selected_session(&mut self) -> Result<()> {
        let Some(parent) = self.tui.chrome_mut().confirm_session_picker() else {
            return Ok(());
        };
        let forked = (self.fork_session)(parent.id.clone())
            .await
            .with_context(|| format!("failed to fork session {}", parent.id))?;
        self.tui
            .chrome_mut()
            .set_session_label(forked.transcript.label.clone());
        self.rebuild_transcript_from_session(&forked.transcript);
        self.active_session_id = Some(forked.session_id);
        Ok(())
    }

    async fn fork_current_session(&mut self) -> Result<()> {
        let Some(parent_id) = self.active_session_id.clone() else {
            self.push_status("No active session to fork");
            return Ok(());
        };
        let forked = (self.fork_session)(parent_id.clone())
            .await
            .with_context(|| format!("failed to fork session {parent_id}"))?;
        let child_id = forked.session_id.clone();
        self.tui
            .chrome_mut()
            .set_session_label(forked.transcript.label.clone());
        self.rebuild_transcript_from_session(&forked.transcript);
        self.active_session_id = Some(forked.session_id);
        self.push_status(format!("Forked session {parent_id} to {child_id}"));
        Ok(())
    }

    fn rebuild_transcript_from_session(&mut self, loaded: &LoadedSessionTranscript) {
        let (cols, rows) = size().unwrap_or((80, 24));
        let mut transcript = TranscriptPane::new(usize::from(cols), usize::from(rows));
        transcript.set_theme(self.tui.chrome().theme());
        transcript.push_welcome_banner(
            self.tui.chrome().title(),
            self.tui.chrome().session_label(),
            self.tui.chrome().model_label(),
            &self.tui.chrome().cwd_label(),
            env!("CARGO_PKG_VERSION"),
            None,
        );
        replay_session_into_transcript(&mut transcript, loaded);
        *self.tui.transcript_mut() = transcript;
    }

    fn apply_selected_model(&mut self) {
        let Some(item) = self.tui.chrome_mut().confirm_model_picker() else {
            return;
        };
        if let Ok(selected) = SelectedModel::from_picker_item(&item) {
            self.tui.chrome_mut().set_model_label(item.label);
            self.tui
                .chrome_mut()
                .set_context_window(selected.max_context_tokens.map(ContextWindow::new));
            self.active_model = Some(selected);
        } else {
            // Not a model item (e.g. a provider from /provider) — show info.
            let detail = item
                .description
                .as_deref()
                .filter(|d| !d.is_empty())
                .map(|d| format!(" — {d}"))
                .unwrap_or_default();
            self.push_status(format!("Provider: {}{detail}", item.label));
        }
    }

    /// Dispatch a rich dialog result after an input event was forwarded.
    async fn process_rich_dialog_result(&mut self, result: InputResult) -> Result<()> {
        if !matches!(
            result,
            InputResult::Submitted | InputResult::Cancelled | InputResult::Handled
        ) {
            return Ok(());
        }
        // For ModelSelector / TabbedModelSelector, the Submit path stores the
        // result in the state. We poll the accessor to apply it.
        if self
            .tui
            .chrome_mut()
            .tabbed_model_selector_result()
            .is_some()
        {
            self.apply_tabbed_model_selection();
        } else if self.tui.chrome_mut().model_selector_result().is_some() {
            self.apply_model_selector_result();
        } else if self.tui.chrome_mut().provider_manager_action().is_some() {
            self.handle_provider_manager_action();
        } else if self.tui.chrome_mut().choice_picker_result().is_some() {
            self.handle_choice_picker_result();
        } else if self.tui.chrome_mut().api_key_input_result().is_some() {
            self.handle_api_key_input_result().await;
        } else if self
            .tui
            .chrome_mut()
            .custom_registry_import_result()
            .is_some()
        {
            self.handle_custom_registry_import_result();
        } else if let Some(result) = self.tui.chrome_mut().take_question_result() {
            self.resolve_question(&result.id, result.answers).await?;
        }
        Ok(())
    }

    /// Apply a model selection, updating the active model, context window,
    /// thinking state, and footer indicator.
    fn apply_model_selection(&mut self, selection: &neo_tui::dialogs::ModelSelection) {
        let (provider, model) = selection
            .alias
            .split_once('/')
            .unwrap_or((&selection.alias, ""));
        self.tui
            .chrome_mut()
            .set_model_label(selection.alias.clone());
        let max_ctx = self
            .model_items
            .iter()
            .find(|item| item.value == selection.alias)
            .and_then(context_window_from_picker_item);
        self.tui
            .chrome_mut()
            .set_context_window(max_ctx.map(ContextWindow::new));
        self.active_model = Some(SelectedModel {
            provider: provider.to_owned(),
            model: model.to_owned(),
            max_context_tokens: max_ctx,
        });
        self.current_thinking = selection.thinking;
        self.tui
            .chrome_mut()
            .set_thinking_enabled(selection.thinking);
        if let Some(config) = self.local_config.as_mut() {
            config.runtime.reasoning_effort = if selection.thinking {
                Some(neo_ai::ReasoningEffort::High)
            } else {
                None
            };
        }
        let notice = if selection.thinking {
            format!("Switched to {} (thinking: on)", selection.alias)
        } else {
            format!("Switched to {}", selection.alias)
        };
        self.push_status(notice);
    }

    /// Apply the selection from the rich `TabbedModelSelector` dialog.
    fn apply_tabbed_model_selection(&mut self) {
        let result = self
            .tui
            .chrome_mut()
            .tabbed_model_selector_result()
            .cloned();
        let Some(result) = result else {
            return;
        };
        self.tui.chrome_mut().close_focused_overlay();
        if let neo_tui::dialogs::ModelSelectorResult::Selected(selection) = result {
            self.apply_model_selection(&selection);
        }
    }

    /// Apply the selection from the flat `ModelSelector` dialog.
    fn apply_model_selector_result(&mut self) {
        let result = self.tui.chrome_mut().model_selector_result().cloned();
        let Some(result) = result else {
            return;
        };
        self.tui.chrome_mut().close_focused_overlay();
        if let neo_tui::dialogs::ModelSelectorResult::Selected(selection) = result {
            self.apply_model_selection(&selection);
        }
    }

    /// Handle a `ProviderManager` action (Add / `DeleteSource` / Close).
    fn handle_provider_manager_action(&mut self) {
        let action = self.tui.chrome_mut().provider_manager_action();
        let Some(action) = action else {
            return;
        };
        match action {
            neo_tui::dialogs::ProviderManagerAction::Close => {
                self.tui.chrome_mut().close_focused_overlay();
            }
            neo_tui::dialogs::ProviderManagerAction::Add => {
                self.tui.chrome_mut().close_focused_overlay();
                // Open choice picker for add method
                let theme = self.tui.chrome().theme();
                self.tui
                    .chrome_mut()
                    .open_choice_picker(neo_tui::dialogs::ChoicePickerOptions {
                        title: "Add Provider".to_owned(),
                        items: vec![
                            neo_tui::dialogs::ChoiceItem::new(
                                "known",
                                "Known third-party provider",
                            )
                            .with_description("Import from models.dev catalog"),
                            neo_tui::dialogs::ChoiceItem::new(
                                "custom",
                                "Custom registry (api.json)",
                            )
                            .with_description("Import from a custom registry URL"),
                        ],
                        initial_id: None,
                        theme,
                        page_size: 0,
                    });
            }
            neo_tui::dialogs::ProviderManagerAction::DeleteSource(ids) => {
                self.tui.chrome_mut().close_focused_overlay();
                if let Some(config_path) = self.config_path() {
                    for id in &ids {
                        if let Err(e) = crate::config_ops::remove_provider(&config_path, id) {
                            self.push_status(format!("Failed to remove provider {id}: {e}"));
                        }
                    }
                    self.push_status(format!("Removed {} provider(s)", ids.len()));
                    self.refresh_config();
                }
            }
        }
    }

    /// Handle a `ChoicePicker` result.
    fn handle_choice_picker_result(&mut self) {
        let Some(result) = self.tui.chrome_mut().choice_picker_result().cloned() else {
            return;
        };
        self.tui.chrome_mut().close_focused_overlay();
        match result {
            neo_tui::dialogs::ChoiceResult::Selected(item) => match item.id.as_str() {
                "known" => {
                    self.tui.chrome_mut().set_custom_working_label(Some(
                        "Fetching models.dev catalog...".to_owned(),
                    ));
                    let handle =
                        tokio::spawn(async move { neo_ai::catalog::fetch_catalog().await });
                    self.pending_catalog_fetch = Some(PendingCatalogFetch {
                        source: CatalogFetchSource::Known,
                        handle,
                    });
                }
                "custom" => {
                    self.tui.chrome_mut().open_custom_registry_import(
                        neo_tui::dialogs::CustomRegistryImportOptions {
                            title: "Import Custom Registry".to_owned(),
                        },
                    );
                }
                id if id.starts_with("catalog:") => {
                    // Catalog provider selected — ask for API key
                    let provider_id = id.strip_prefix("catalog:").unwrap_or(id);
                    self.pending_catalog_provider_id = Some(provider_id.to_owned());
                    self.tui.chrome_mut().open_api_key_input(
                        neo_tui::dialogs::ApiKeyInputOptions {
                            title: "API Key".to_owned(),
                            provider_name: provider_id.to_owned(),
                        },
                    );
                }
                id if id.starts_with("custom-catalog:") => {
                    let provider_id = id.strip_prefix("custom-catalog:").unwrap_or(id);
                    if let Some(pending) = self.pending_custom_registry.take() {
                        if let Some(entry) = pending.catalog.get(provider_id) {
                            match self.config_path() {
                                Some(config_path) => {
                                    match crate::config_ops::add_provider_from_catalog_entry(
                                        &config_path,
                                        provider_id,
                                        entry,
                                        Some(&pending.source.token),
                                        None,
                                    ) {
                                        Ok(message) => {
                                            self.push_status(message);
                                            self.refresh_config();
                                        }
                                        Err(error) => {
                                            self.push_status(format!(
                                                "Error: Failed to import provider: {error}"
                                            ));
                                        }
                                    }
                                }
                                None => {
                                    self.push_status("No config available");
                                }
                            }
                        } else {
                            self.push_status(format!(
                                "Error: Provider '{provider_id}' not found in registry"
                            ));
                        }
                    }
                }
                _ => {}
            },
            neo_tui::dialogs::ChoiceResult::Cancelled => {}
        }
    }

    /// Handle an API key input result.
    async fn handle_api_key_input_result(&mut self) {
        let Some(result) = self.tui.chrome_mut().api_key_input_result().cloned() else {
            return;
        };
        self.tui.chrome_mut().close_focused_overlay();
        match result {
            neo_tui::dialogs::ApiKeyInputResult::Submitted(key) => {
                if let Some(provider_id) = self.pending_catalog_provider_id.take() {
                    match self.config_path() {
                        Some(config_path) => {
                            match crate::config_ops::catalog_add_provider(
                                &config_path,
                                &provider_id,
                                Some(&key),
                                None,
                            )
                            .await
                            {
                                Ok(message) => {
                                    self.push_status(message);
                                    self.refresh_config();
                                }
                                Err(error) => {
                                    self.push_status(format!(
                                        "Error: Failed to add provider: {error}"
                                    ));
                                }
                            }
                        }
                        None => {
                            self.push_status("No config available");
                        }
                    }
                } else {
                    self.push_status("API key saved.");
                }
            }
            neo_tui::dialogs::ApiKeyInputResult::Cancelled => {
                self.pending_catalog_provider_id = None;
            }
        }
    }

    /// Handle a custom registry import result.
    fn handle_custom_registry_import_result(&mut self) {
        let Some(result) = self
            .tui
            .chrome_mut()
            .custom_registry_import_result()
            .cloned()
        else {
            return;
        };
        self.tui.chrome_mut().close_focused_overlay();
        match result {
            neo_tui::dialogs::CustomRegistryImportResult::Submitted(source) => {
                self.tui
                    .chrome_mut()
                    .set_custom_working_label(Some("Fetching custom registry...".to_owned()));
                let url = source.url.clone();
                let handle =
                    tokio::spawn(async move { neo_ai::catalog::fetch_catalog_from(&url).await });
                self.pending_catalog_fetch = Some(PendingCatalogFetch {
                    source: CatalogFetchSource::Custom(source),
                    handle,
                });
            }
            neo_tui::dialogs::CustomRegistryImportResult::Cancelled => {}
        }
    }

    /// Poll a pending catalog fetch. If it has finished, clear the working
    /// indicator and open the provider picker; if not, leave it in place.
    async fn poll_pending_catalog_fetch(&mut self) {
        let Some(pending) = self.pending_catalog_fetch.take() else {
            return;
        };
        if !pending.handle.is_finished() {
            self.pending_catalog_fetch = Some(pending);
            return;
        }
        self.tui.chrome_mut().set_custom_working_label(None);
        match pending.handle.await {
            Ok(Ok(catalog)) => {
                let items: Vec<_> = catalog
                    .iter()
                    .map(|(id, entry)| {
                        let label = entry.name.clone().unwrap_or_else(|| id.clone());
                        let description = entry.api.clone().unwrap_or_default();
                        neo_tui::dialogs::ChoiceItem::new(format!("catalog:{id}"), label)
                            .with_description(description)
                    })
                    .collect();
                if items.is_empty() {
                    self.push_status("No providers found in catalog.");
                    return;
                }
                match pending.source {
                    CatalogFetchSource::Known => {
                        let theme = self.tui.chrome().theme();
                        self.tui.chrome_mut().open_choice_picker(
                            neo_tui::dialogs::ChoicePickerOptions {
                                title: "Select a provider".to_owned(),
                                items,
                                initial_id: None,
                                theme,
                                page_size: 0,
                            },
                        );
                    }
                    CatalogFetchSource::Custom(source) => {
                        self.pending_custom_registry =
                            Some(PendingCustomRegistry { source, catalog });
                        let custom_items: Vec<_> = items
                            .into_iter()
                            .map(|mut item| {
                                item.id = item.id.replacen("catalog:", "custom-catalog:", 1);
                                item
                            })
                            .collect();
                        let theme = self.tui.chrome().theme();
                        self.tui.chrome_mut().open_choice_picker(
                            neo_tui::dialogs::ChoicePickerOptions {
                                title: "Select a provider".to_owned(),
                                items: custom_items,
                                initial_id: None,
                                theme,
                                page_size: 0,
                            },
                        );
                    }
                }
            }
            Ok(Err(error)) => {
                self.push_status(format!("Error: Failed to fetch catalog: {error}"));
            }
            Err(join_error) => {
                self.push_status(format!("Error: Failed to fetch catalog: {join_error}"));
            }
        }
    }

    #[allow(dead_code)]
    #[must_use]
    pub const fn app(&self) -> &NeoChromeState {
        self.tui.chrome()
    }

    #[cfg(test)]
    #[must_use]
    fn transcript(&self) -> &TranscriptPane {
        self.tui.transcript()
    }

    #[must_use]
    pub fn take_suspend_requested(&mut self) -> bool {
        let requested = self.suspend_requested;
        self.suspend_requested = false;
        requested
    }

    #[must_use]
    pub fn render_snapshot(&self) -> String {
        let mut transcript = self.tui.transcript().clone();
        render_transcript_snapshot(self.tui.chrome(), &mut transcript, 80, 24)
    }
}

fn terminal_image_capabilities_for_policy(
    protocol: ImageProtocolPreference,
    env_var: impl Fn(&str) -> std::result::Result<String, env::VarError>,
) -> TerminalImageCapabilities {
    if matches!(protocol, ImageProtocolPreference::None) {
        return TerminalImageCapabilities::default();
    }

    let term = env_var("TERM").unwrap_or_default().to_ascii_lowercase();
    let term_program = env_var("TERM_PROGRAM")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let has_env = |name: &str| env_var(name).is_ok();
    let conservative_multiplexer = has_env("TMUX")
        || has_env("STY")
        || has_env("SSH_CONNECTION")
        || has_env("SSH_TTY")
        || term.starts_with("screen")
        || term.contains("tmux");
    if conservative_multiplexer {
        return TerminalImageCapabilities::default();
    }

    let static_hints = TerminalImageCapabilities::default()
        .with_kitty(
            has_env("KITTY_WINDOW_ID")
                || has_env("WEZTERM_PANE")
                || term.contains("kitty")
                || term_program.contains("wezterm"),
        )
        .with_iterm2(term_program.contains("iterm"))
        .with_sixel(term.contains("sixel") || has_env("SIXEL"));

    match protocol {
        ImageProtocolPreference::Kitty => {
            TerminalImageCapabilities::default().with_kitty(static_hints.kitty())
        }
        ImageProtocolPreference::Iterm2 => {
            TerminalImageCapabilities::default().with_iterm2(static_hints.iterm2())
        }
        ImageProtocolPreference::Sixel => {
            TerminalImageCapabilities::default().with_sixel(static_hints.sixel())
        }
        ImageProtocolPreference::Auto => static_hints,
        ImageProtocolPreference::None => TerminalImageCapabilities::default(),
    }
}

fn prompt_completions(
    root: &Path,
    prefix: &str,
    model_items: &[PickerItem],
    skill_store: Option<&SkillStore>,
) -> Result<Vec<PickerItem>> {
    let catalog = CompletionCatalog {
        slash_prompts: slash_prompt_template_completion_items(root, prefix)?.unwrap_or_default(),
        prompt_packages: prompt_package_completion_items(root)?,
        extension_commands: extension_command_completion_items(root)?,
        session_commands: session_completion_items(skill_store),
        model_items: model_items.to_vec(),
    };
    Ok(completion_source_candidates(root, prefix, &catalog)?
        .into_iter()
        .map(|candidate| candidate.to_picker_item())
        .collect())
}

fn prompt_package_completion_items(root: &Path) -> Result<Vec<PickerItem>> {
    let mut items = discover_prompt_template_commands(root, None, &[])?
        .into_iter()
        .filter(|command| command.location == PromptTemplateLocation::Project)
        .filter_map(|command| {
            let relative_path = command
                .template
                .path
                .strip_prefix(root.join(".neo/prompts"))
                .ok()?;
            let provider = relative_path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .and_then(|parent| parent.components().next())
                .and_then(|component| component.as_os_str().to_str())
                .filter(|provider| !provider.is_empty())?;
            let value = format!("/{}", command.template.name);
            let description = prompt_source_description(
                (!command.template.description.is_empty())
                    .then_some(command.template.description.as_str()),
                Some(provider),
                None,
            );
            Some(PickerItem::new(value.clone(), value, Some(description)))
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| left.value.cmp(&right.value));
    items.dedup_by(|left, right| left.value == right.value);
    Ok(items)
}

fn extension_command_completion_items(root: &Path) -> Result<Vec<PickerItem>> {
    let extension_root = neo_agent_core::tools::extensions::default_extension_root(root);
    if !extension_root.exists() {
        return Ok(Vec::new());
    }
    let mut items = neo_agent_core::tools::extensions::ExtensionDiscovery::new(&extension_root)
        .discover()
        .with_context(|| {
            format!(
                "failed to discover extensions under {}",
                extension_root.display()
            )
        })?
        .into_iter()
        .map(|extension| {
            let value = format!("/{}", extension.manifest.id);
            let description = prompt_source_description(
                extension.manifest.description.as_deref(),
                Some(&extension.manifest.id),
                Some("local extension"),
            );
            PickerItem::new(value.clone(), value, Some(description))
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| left.value.cmp(&right.value));
    items.dedup_by(|left, right| left.value == right.value);
    items.truncate(100);
    Ok(items)
}

fn session_completion_items(skill_store: Option<&SkillStore>) -> Vec<PickerItem> {
    let mut items = vec![
        PickerItem::new(
            "/resume",
            "/resume",
            Some(prompt_source_description(
                Some("Resume a local session"),
                Some("local sessions"),
                Some("local"),
            )),
        ),
        PickerItem::new(
            "/model",
            "/model",
            Some(prompt_source_description(
                Some("Switch active model"),
                Some("model picker"),
                Some("local"),
            )),
        ),
        PickerItem::new(
            "/provider",
            "/provider",
            Some(prompt_source_description(
                Some("View configured providers"),
                Some("provider picker"),
                Some("local"),
            )),
        ),
        PickerItem::new(
            "/plan",
            "/plan",
            Some(prompt_source_description(
                Some("Toggle plan mode (on / off / clear)"),
                Some("plan mode"),
                Some("local"),
            )),
        ),
    ];
    if let Some(skill_store) = skill_store {
        for skill in skill_store.iter() {
            let value = format!("/skill:{}", skill.name);
            items.push(PickerItem::new(
                value.clone(),
                value,
                Some(prompt_source_description(
                    Some(&skill.manifest.description),
                    Some("skill"),
                    Some("local"),
                )),
            ));
        }
    }
    items
}

fn prompt_source_description(
    description: Option<&str>,
    provider: Option<&str>,
    trust: Option<&str>,
) -> String {
    let mut details = Vec::new();
    if let Some(description) = description.filter(|description| !description.is_empty()) {
        details.push(description.to_owned());
    }
    if let Some(provider) = provider {
        details.push(format!("provider: {provider}"));
    }
    if let Some(trust) = trust {
        details.push(format!("trust: {trust}"));
    }
    details.join(" | ")
}

fn slash_prompt_template_completion_items(
    root: &Path,
    prefix: &str,
) -> Result<Option<Vec<PickerItem>>> {
    let Some(name_prefix) = prefix.strip_prefix('/') else {
        return Ok(None);
    };
    if name_prefix.contains('/') {
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

fn filesystem_completion_candidates(root: &Path, prefix: &str) -> Result<Vec<CompletionCandidate>> {
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
        completions.push(CompletionCandidate::new(
            value.clone(),
            value,
            Some(description.to_owned()),
            CompletionSource::LocalFile,
        ));
    }

    completions.sort_by(|left, right| left.value.cmp(&right.value));
    completions.truncate(100);
    Ok(completions)
}

fn model_completion_candidates(
    prefix: &str,
    model_items: &[PickerItem],
) -> Option<Vec<CompletionCandidate>> {
    let model_prefix = prefix.strip_prefix('@')?;
    if model_items.is_empty() {
        return None;
    }

    let mut completions = model_items
        .iter()
        .filter(|item| item.value.starts_with(model_prefix))
        .map(|item| {
            let value = format!("@{}", item.value);
            CompletionCandidate::new(
                value.clone(),
                value,
                item.description.clone(),
                CompletionSource::ProviderModel,
            )
        })
        .collect::<Vec<_>>();
    completions.sort_by(|left, right| left.value.cmp(&right.value));
    completions.truncate(100);
    Some(completions)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CompletionCatalog {
    slash_prompts: Vec<PickerItem>,
    prompt_packages: Vec<PickerItem>,
    extension_commands: Vec<PickerItem>,
    session_commands: Vec<PickerItem>,
    model_items: Vec<PickerItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletionSource {
    LocalFile,
    SlashPrompt,
    PromptPackage,
    ExtensionCommand,
    SessionCommand,
    ProviderModel,
}

impl CompletionSource {
    const fn label(self) -> &'static str {
        match self {
            Self::LocalFile => "local file",
            Self::SlashPrompt => "slash prompt",
            Self::PromptPackage => "prompt package",
            Self::ExtensionCommand => "extension command",
            Self::SessionCommand => "session command",
            Self::ProviderModel => "provider model",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompletionCandidate {
    value: String,
    label: String,
    description: Option<String>,
    source: CompletionSource,
    source_label: &'static str,
}

impl CompletionCandidate {
    fn new(
        value: impl Into<String>,
        label: impl Into<String>,
        description: Option<String>,
        source: CompletionSource,
    ) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description,
            source,
            source_label: source.label(),
        }
    }

    fn from_picker(item: PickerItem, source: CompletionSource) -> Self {
        Self::new(item.value, item.label, item.description, source)
    }

    fn to_picker_item(&self) -> PickerItem {
        PickerItem::new(
            self.value.clone(),
            self.label.clone(),
            Some(completion_description(
                self.description.as_deref(),
                self.source_label,
            )),
        )
    }
}

fn completion_source_candidates(
    root: &Path,
    prefix: &str,
    catalog: &CompletionCatalog,
) -> Result<Vec<CompletionCandidate>> {
    let mut candidates = if prefix.starts_with('/') {
        slash_source_candidates(prefix, catalog)
    } else if prefix.starts_with('@') {
        model_completion_candidates(prefix, &catalog.model_items).unwrap_or_default()
    } else {
        filesystem_completion_candidates(root, prefix)?
    };
    candidates.sort_by(|left, right| {
        completion_source_rank(left.source)
            .cmp(&completion_source_rank(right.source))
            .then_with(|| left.value.cmp(&right.value))
    });
    candidates.truncate(100);
    Ok(candidates)
}

fn slash_source_candidates(prefix: &str, catalog: &CompletionCatalog) -> Vec<CompletionCandidate> {
    let sources = [
        (&catalog.slash_prompts, CompletionSource::SlashPrompt),
        (&catalog.prompt_packages, CompletionSource::PromptPackage),
        (
            &catalog.extension_commands,
            CompletionSource::ExtensionCommand,
        ),
        (&catalog.session_commands, CompletionSource::SessionCommand),
    ];
    sources
        .into_iter()
        .flat_map(|(items, source)| {
            items
                .iter()
                .filter(move |item| item.value.starts_with(prefix))
                .cloned()
                .map(move |item| CompletionCandidate::from_picker(item, source))
        })
        .collect()
}

const fn completion_source_rank(source: CompletionSource) -> u8 {
    match source {
        CompletionSource::LocalFile => 0,
        CompletionSource::SlashPrompt => 1,
        CompletionSource::PromptPackage => 2,
        CompletionSource::ExtensionCommand => 3,
        CompletionSource::SessionCommand => 4,
        CompletionSource::ProviderModel => 5,
    }
}

fn completion_description(description: Option<&str>, source_label: &str) -> String {
    match description {
        Some(description) if !description.is_empty() => {
            format!("{description} | source: {source_label}")
        }
        _ => format!("source: {source_label}"),
    }
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
    let (project_dir, selectors) = if let Some(config) = config {
        (
            config.project_dir.as_path(),
            config.prompt_templates.clone(),
        )
    } else {
        (fallback_project_dir, Vec::new())
    };
    let expanded = expand_prompt_template_args(
        args,
        project_dir,
        config::global_prompts_dir().as_deref(),
        &selectors,
        false,
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
            "providers",
            "Open providers",
            Some("View configured providers"),
        ),
        CommandSpec::new(
            "session.exportHtml",
            "Export session to HTML",
            Some("Write the active local session as sanitized HTML"),
        ),
        CommandSpec::new(
            "fork",
            "Fork session",
            Some("Create a child fork of the current session"),
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
        CommandSpec::new(
            "plan",
            "Toggle plan mode",
            Some("Read-only mode for investigation and planning"),
        ),
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

        self.pending.extend(self.parser.flush_timeout());
        if let Some(input) = self.pending.pop_front() {
            return Ok(Some(input));
        }

        Ok(None)
    }
}

const EDITING_ACTION_PRIORITY: &[KeybindingAction] = &[
    KeybindingAction::InputSubmit,
    KeybindingAction::InputNewLine,
    KeybindingAction::TranscriptCopySelection,
    KeybindingAction::AppClear,
    KeybindingAction::AppExit,
    KeybindingAction::AppSuspend,
    KeybindingAction::TranscriptSelectionStart,
    KeybindingAction::TranscriptSelectionClear,
    KeybindingAction::TranscriptSelectionExtendUp,
    KeybindingAction::TranscriptSelectionExtendDown,
    KeybindingAction::TranscriptSelectionExtendPageUp,
    KeybindingAction::TranscriptSelectionExtendPageDown,
    KeybindingAction::CommandPaletteOpen,
    KeybindingAction::SessionPickerOpen,
    KeybindingAction::ToolOutputToggle,
    KeybindingAction::ModelPickerOpen,
    KeybindingAction::EditorCursorUp,
    KeybindingAction::EditorCursorDown,
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
    KeybindingAction::InputCopy,
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

const QUESTION_ACTION_PRIORITY: &[KeybindingAction] = &[
    KeybindingAction::SelectConfirm,
    KeybindingAction::SelectCancel,
    KeybindingAction::SelectUp,
    KeybindingAction::SelectDown,
    KeybindingAction::EditorCursorUp,
    KeybindingAction::EditorCursorDown,
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
    KeybindingAction::InputSubmit,
    KeybindingAction::InputNewLine,
    KeybindingAction::InputTab,
];

const PROMPT_COMPLETION_ACTION_PRIORITY: &[KeybindingAction] = &[
    KeybindingAction::SelectConfirm,
    KeybindingAction::SelectCancel,
    KeybindingAction::SelectUp,
    KeybindingAction::SelectDown,
    KeybindingAction::SelectPageUp,
    KeybindingAction::SelectPageDown,
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
    KeybindingAction::InputCopy,
];

struct NeoTerminal {
    tui: TuiRenderer,
}

impl NeoTerminal {
    fn enter() -> Result<Self> {
        let tui = TuiRenderer::enter()?;
        Ok(Self { tui })
    }

    fn draw_tui(&mut self, tui: &mut neo_tui::NeoTui) -> Result<()> {
        let (cols, rows) = size()?;
        if cols == 0 || rows == 0 {
            return Ok(());
        }
        let (lines, cursor) = tui.render_frame(usize::from(cols), usize::from(rows));
        // Single-buffer differential render: hand the whole frame to
        // TuiRenderer::render, which diffs against the previous frame and
        // rewrites only changed lines in place.
        self.tui.render(lines, cursor)?;
        Ok(())
    }

    #[allow(dead_code)]
    fn leave(&mut self) {
        self.tui.leave();
    }

    fn reenter(&mut self) -> Result<()> {
        // Force a full redraw on the next render so the resumed session paints
        // cleanly after the terminal state was disturbed by SIGTSTP.
        self.tui.suspend_resume()?;
        Ok(())
    }
}

/// Compose the full frame (body + chrome) as ANSI strings, without writing to
/// the terminal. Used by tests that need to inspect what would be drawn.
#[cfg(test)]
fn compose_tui_frame(
    app: &NeoChromeState,
    transcript: &mut TranscriptPane,
    cols: u16,
    rows: u16,
) -> Option<Vec<String>> {
    if cols == 0 || rows == 0 {
        return None;
    }
    transcript.mark_dirty();
    let mut tui = neo_tui::NeoTui::new(app.clone(), transcript.clone());
    let (lines, _) = tui.render_frame(usize::from(cols), usize::from(rows));
    *transcript = tui.transcript().clone();
    Some(lines)
}

impl Drop for NeoTerminal {
    fn drop(&mut self) {
        self.tui.leave();
    }
}

impl NeoTerminal {
    fn suspend(&mut self) -> Result<()> {
        self.tui.suspend_prepare();
        #[cfg(unix)]
        {
            rustix::process::kill_current_process_group(rustix::process::Signal::TSTP)?;
        }
        #[cfg(not(unix))]
        {
            eprintln!("Suspend to background is not supported on this platform");
        }
        self.reenter()
    }
}

#[allow(clippy::too_many_lines)]
pub fn controller_for_config(config: &AppConfig) -> InteractiveController {
    let catalogs = picker_catalogs_for_config(config);
    let selected_model = crate::modes::run::model_registry_for_config(config)
        .and_then(|registry| crate::modes::run::select_config_model(&registry, config))
        .ok();
    let mut config = config.clone();
    if let Some(model) = &selected_model {
        config.default_provider.clone_from(&model.provider.0);
        config.default_model.clone_from(&model.model);
    }
    let run_config = config.clone();
    let run_turn: TurnDriver = Arc::new(move |request, channels| {
        let mut effective_config = run_config.clone();
        Box::pin(async move {
            if let Some(model) = request.model {
                effective_config.default_provider = model.provider;
                effective_config.default_model = model.model;
            }
            effective_config.runtime.reasoning_effort = request.reasoning_effort;
            if let Some(session_id) = request.session_id {
                let turn = crate::modes::run::run_prompt_in_session_streaming(
                    &session_id,
                    &request.prompt,
                    &effective_config,
                    channels.events,
                    channels.approvals,
                    Some(channels.session_ids),
                    channels.cancel_token,
                    Some(channels.questions),
                    request.skill_context.clone(),
                )
                .await?;
                Ok(TurnOutcome::session(turn.session_id))
            } else {
                let turn = crate::modes::run::run_prompt_streaming(
                    &request.prompt,
                    &effective_config,
                    channels.events,
                    channels.approvals,
                    Some(channels.session_ids),
                    channels.cancel_token,
                    Some(channels.questions),
                    request.skill_context.clone(),
                )
                .await?;
                Ok(TurnOutcome::session(turn.session_id))
            }
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

    let mut controller = InteractiveController::new(
        "neo",
        "new",
        format!("{}/{}", config.default_provider, config.default_model),
        config.project_dir.clone(),
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
    let default_model_value = format!("{}/{}", config.default_provider, config.default_model);
    let default_context_window = selected_model
        .as_ref()
        .and_then(|model| model.capabilities.max_context_tokens)
        .map(ContextWindow::new)
        .or_else(|| {
            controller
                .model_items
                .iter()
                .find(|item| item.value == default_model_value)
                .and_then(context_window_from_picker_item)
                .map(ContextWindow::new)
        });
    controller
        .tui
        .chrome_mut()
        .set_context_window(default_context_window);
    controller.current_thinking = config.runtime.reasoning_effort.is_some();
    controller
        .tui
        .chrome_mut()
        .set_thinking_enabled(controller.current_thinking);
    controller.local_config = Some(config.clone());
    let skill_store = resources::load_skill_store(
        &config.project_dir,
        neo_home().as_deref(),
        &config.extra_skill_dirs,
        &config.skill_path,
    )
    .ok();
    if let Some(ref store) = skill_store {
        controller
            .tui
            .transcript_mut()
            .set_skill_store(store.clone());
    }
    controller.skill_store = skill_store;
    controller
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
    items: Vec<SessionSummary>,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelPickerCatalog {
    items: Vec<PickerItem>,
    error: Option<String>,
}

fn picker_catalogs_for_config(config: &AppConfig) -> PickerCatalogs {
    let sessions = session_catalog_for_config(config);
    let models = model_picker_catalog_for_config(config);
    PickerCatalogs {
        session_items: sessions.items,
        session_error: sessions.error,
        model_items: models.items,
    }
}

fn session_catalog_for_config(config: &AppConfig) -> SessionCatalog {
    match session_summaries(config, SessionDataScope::Workspace) {
        Ok(items) => SessionCatalog { items, error: None },
        Err(error) => SessionCatalog {
            items: Vec::new(),
            error: Some(error.to_string()),
        },
    }
}

fn model_picker_catalog_for_config(config: &AppConfig) -> ModelPickerCatalog {
    match crate::modes::run::model_registry_for_config(config) {
        Ok(registry) => {
            let models = registry.list();
            let models = config::scoped_models(models.iter(), &config.model_scope);
            ModelPickerCatalog {
                items: models.iter().map(model_to_picker_item).collect(),
                error: None,
            }
        }
        Err(error) => ModelPickerCatalog {
            items: Vec::new(),
            error: Some(error.to_string()),
        },
    }
}

fn model_to_picker_item(model: &neo_ai::ModelSpec) -> PickerItem {
    let value = format!("{}/{}", model.provider.0, model.model);
    let description = match model.capabilities.max_context_tokens {
        Some(max_context_tokens) => {
            format!("{:?} · ctx {max_context_tokens}", model.api)
        }
        None => format!("{:?}", model.api),
    };
    PickerItem::new(value.clone(), value, Some(description))
}

/// Build `ModelEntry` list directly from `[models.*]` in config.
fn model_entries_from_config(config: &AppConfig) -> Vec<neo_tui::dialogs::ModelEntry> {
    if !config.models.is_empty() {
        return config
            .models
            .iter()
            .map(|(alias, model)| {
                let provider_id = model.provider.clone();
                let alias_full = if alias.contains('/') {
                    alias.clone()
                } else {
                    format!("{}/{}", provider_id, model.model)
                };
                let mut capabilities = model.capabilities.clone();
                if capabilities.iter().any(|c| c == "reasoning")
                    && !capabilities.iter().any(|c| c == "thinking")
                {
                    capabilities.push("thinking".to_owned());
                }
                neo_tui::dialogs::ModelEntry {
                    alias: alias_full,
                    provider_id,
                    display_name: model.display_name.clone().unwrap_or_else(|| alias.clone()),
                    model_id: model.model.clone(),
                    capabilities,
                    max_context_tokens: model.max_context_tokens,
                }
            })
            .collect();
    }
    Vec::new()
}

fn context_window_from_picker_item(item: &PickerItem) -> Option<u32> {
    let description = item.description.as_deref()?;
    let (_, context) = description.rsplit_once("ctx ")?;
    parse_token_count(context.trim())
}

fn parse_token_count(value: &str) -> Option<u32> {
    let value = value.trim().to_ascii_lowercase();
    let (number, multiplier) = match value.strip_suffix('m') {
        Some(number) => (number, 1_000_000u32),
        None => match value.strip_suffix('k') {
            Some(number) => (number, 1_000u32),
            None => (value.as_str(), 1u32),
        },
    };
    number
        .parse::<u32>()
        .ok()
        .and_then(|count| count.checked_mul(multiplier))
}

fn startup_notices(config: &AppConfig) -> Vec<String> {
    let model_scope = if config.model_scope.is_empty() {
        "all".to_owned()
    } else {
        config.model_scope.join(",")
    };
    let mut notices = vec![
        "Startup".to_owned(),
        format!("project: {}", config.project_dir.display()),
        format!("sessions: {}", workspace_sessions_dir(config).display()),
        format!(
            "model: {}/{}",
            config.default_provider, config.default_model
        ),
        format!("model scope: {model_scope}"),
        format!("theme: {}", config.theme.name),
        "resources: auto-discovered".to_owned(),
        format!("trust: project={}", enabled_label(config.project_trusted)),
    ];
    if !config.tui.keybindings.is_empty() {
        notices.push(format!(
            "keybindings: {} {}",
            config.tui.keybindings.len(),
            pluralize(config.tui.keybindings.len(), "override", "overrides")
        ));
        notices.push("local config: tui.keybindings available".to_owned());
    }
    notices
}

fn enabled_label(enabled: bool) -> &'static str {
    if enabled { "enabled" } else { "disabled" }
}

const fn pluralize(count: usize, singular: &'static str, plural: &'static str) -> &'static str {
    if count == 1 { singular } else { plural }
}

fn same_work_dir(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// Parse a timestamp string (epoch millis, epoch secs, or RFC3339) into `SystemTime`.
fn parse_timestamp(ts: &str) -> std::time::SystemTime {
    // Try epoch millis first
    if let Ok(millis) = ts.parse::<u64>() {
        let secs = millis / 1000;
        let nanos = u32::try_from((millis % 1000) * 1_000_000)
            .expect("millisecond remainder fits in nanoseconds");
        if let Some(t) = std::time::UNIX_EPOCH.checked_add(std::time::Duration::new(secs, nanos)) {
            return t;
        }
    }
    // Try epoch seconds
    let seconds_str = ts.split_once('.').map_or(ts, |(s, _)| s);
    if let Ok(secs) = seconds_str.parse::<u64>()
        && let Some(t) = std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(secs))
    {
        return t;
    }
    std::time::UNIX_EPOCH
}

async fn load_session_transcript(
    session_id: String,
    config: &AppConfig,
) -> Result<LoadedSessionTranscript> {
    let path = crate::modes::sessions::session_path(&session_id, config)?;
    let context = JsonlSessionReader::replay_context(&path)
        .await
        .with_context(|| format!("failed to replay session {}", path.display()))?;
    let mut notices = Vec::new();
    if let Some(summary) = context.compaction_summary() {
        notices.push(format!("compaction: {}", summary.summary));
    }
    if let Some(summary) = SessionMetadataStore::new(workspace_sessions_dir(config))
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

fn replay_session_into_transcript(
    transcript: &mut TranscriptPane,
    loaded: &LoadedSessionTranscript,
) {
    for notice in &loaded.notices {
        transcript.push_transcript(neo_tui::transcript::TranscriptEntry::status(notice.clone()));
    }
    for message in &loaded.messages {
        transcript.replay_message(message);
    }
}

async fn fork_session_transcript(
    parent_id: String,
    config: &AppConfig,
) -> Result<ForkedSessionTranscript> {
    let session = SessionMetadataStore::new(workspace_sessions_dir(config))
        .fork(&parent_id, None)
        .with_context(|| format!("failed to create local fork for session {parent_id}"))?;
    let child_id = session.id;
    let mut loaded = load_session_transcript(child_id.clone(), config).await?;
    loaded.notices.insert(0, format!("forked from {parent_id}"));
    Ok(ForkedSessionTranscript::new(child_id, loaded))
}

fn render_transcript_snapshot(
    app: &NeoChromeState,
    transcript: &mut TranscriptPane,
    width: usize,
    height: usize,
) -> String {
    transcript.resize(width, height);
    transcript.mark_dirty();
    let _ = transcript.render_frame(width, height);

    let mut lines = transcript
        .frame_ansi_lines()
        .into_iter()
        .map(|line| neo_tui::ansi::strip_ansi(&line).trim_end().to_owned())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    lines.extend(render_overlay_snapshot(app, width));
    format!("{}\n", lines.join("\n").trim_end())
}

fn render_overlay_snapshot(app: &NeoChromeState, width: usize) -> Vec<String> {
    let content_width = neo_tui::transcript::frame_content_width(width);
    let mut lines = match app.focused_overlay().map(|overlay| &overlay.kind) {
        Some(OverlayKind::SessionPicker(picker)) => {
            let theme = app.theme();
            picker.render_lines(content_width, &theme)
        }
        Some(OverlayKind::ModelPicker(picker)) => {
            render_picker_snapshot("Models", picker, content_width)
        }
        Some(OverlayKind::CommandPalette(_)) => vec!["Commands".to_owned()],
        Some(OverlayKind::PromptCompletion(_)) => vec![],
        Some(OverlayKind::Message(message)) => vec![message.clone()],
        Some(OverlayKind::Approval(_) | OverlayKind::QuestionDialog(_)) | None => Vec::new(),
        // Rich dialogs — use their own render_lines.
        Some(_) => app.focused_overlay_lines(content_width),
    };
    lines.extend(
        neo_tui::transcript::render_chrome_lines(app, width)
            .lines
            .into_iter()
            .map(|line| neo_tui::ansi::strip_ansi(&line).trim_end().to_owned()),
    );
    lines
}

fn render_picker_snapshot(
    title: &str,
    picker: &neo_tui::chrome::PickerState,
    width: usize,
) -> Vec<String> {
    let mut lines = vec![title.to_owned()];
    lines.extend(picker.render_lines(width));
    lines
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
    };

    use neo_agent_core::ToolResult;
    use neo_agent_core::{AgentEvent, AgentMessage, Content, PermissionPolicy, StopReason};
    use neo_tui::{
        chrome::{ApprovalChoice, ChromeMode, CommandPaletteState, Overlay, OverlayKind},
        input::KeybindingAction,
        transcript::TranscriptEntry,
    };

    use super::*;
    use crate::config::{Defaults, McpConfig, ModelConfig, RuntimeConfig, TuiConfig};

    const SESSION_A: &str = "session_00000000-0000-4000-8000-000000000601";
    const SESSION_B: &str = "session_00000000-0000-4000-8000-000000000602";
    const SESSION_CHILD: &str = "session_00000000-0000-4000-8000-000000000603";
    const SESSION_NEW: &str = "session_00000000-0000-4000-8000-000000000604";

    fn test_workspace_root() -> PathBuf {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    fn test_session_summary(
        id: impl Into<String>,
        title: impl Into<String>,
        work_dir: impl Into<PathBuf>,
        last_prompt: impl Into<String>,
    ) -> SessionSummary {
        SessionSummary {
            id: id.into(),
            title: Some(title.into()),
            last_prompt: Some(last_prompt.into()),
            work_dir: work_dir.into(),
            updated_at: String::new(),
            metadata: None,
        }
    }

    fn transcript_entries(controller: &InteractiveController) -> &[TranscriptEntry] {
        controller.transcript().transcript().entries()
    }

    fn transcript_has_status(controller: &InteractiveController, expected: &str) -> bool {
        transcript_entries(controller).iter().any(|entry| {
            matches!(entry, TranscriptEntry::Status { text, .. } if text.contains(expected))
        })
    }

    fn transcript_scrollback(controller: &InteractiveController) -> usize {
        controller.transcript().transcript().viewport().scrollback()
    }

    fn render_tui_snapshot(tui: &neo_tui::NeoTui) -> String {
        let mut transcript = tui.transcript().clone();
        render_transcript_snapshot(tui.chrome(), &mut transcript, 80, 24)
    }

    #[test]
    fn exit_message_prints_resume_command_when_session_exists() {
        assert_eq!(exit_message(None), "Bye\n");
        assert_eq!(
            exit_message(Some("session_550e8400-e29b-41d4-a716-446655440000")),
            "Bye\nneo resume session_550e8400-e29b-41d4-a716-446655440000\n"
        );
    }

    #[test]
    fn transcript_pane_exposes_live_rows_for_neo_tui_draw() {
        let app = NeoChromeState::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
        );
        let mut transcript = TranscriptPane::new(80, 12);
        transcript.apply_agent_event(AgentEvent::ToolExecutionStarted {
            turn: 0,
            id: "tool-1".to_owned(),
            name: "Bash".to_owned(),
            arguments: serde_json::json!({ "command": "cargo test" }),
        });

        let lines =
            compose_tui_frame(&app, &mut transcript, 80, 12).expect("non-zero terminal size");

        let plain: Vec<String> = lines
            .iter()
            .map(|line| neo_tui::ansi::strip_ansi(line))
            .collect();
        assert!(plain.iter().any(|line| line.contains("Using Bash")));
        assert_eq!(compose_tui_frame(&app, &mut transcript, 0, 12), None);
    }

    #[tokio::test]
    async fn resolving_question_records_collected_answers_in_transcript() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "session",
            "model",
            test_workspace_root(),
            |_| async { Ok(Vec::new()) },
        );
        let (response_tx, mut response_rx) = oneshot::channel();
        controller.register_pending_question(PendingQuestion {
            id: "question-1".to_owned(),
            questions: vec![neo_agent_core::QuestionEventData {
                question: "Pick a side?".to_owned(),
                header: Some("Choice".to_owned()),
                body: None,
                options: vec![
                    neo_agent_core::QuestionOptionData {
                        label: "Left".to_owned(),
                        description: None,
                    },
                    neo_agent_core::QuestionOptionData {
                        label: "Right".to_owned(),
                        description: None,
                    },
                ],
                multi_select: false,
            }],
            response_tx,
        });

        controller
            .resolve_question("question-1", vec!["Left".to_owned()])
            .await
            .expect("question resolves");

        assert_eq!(
            response_rx
                .try_recv()
                .expect("response should be sent")
                .answers,
            vec!["Left"]
        );
        assert!(transcript_has_status(&controller, "Collected your answers"));
        assert!(transcript_has_status(&controller, "Pick a side?"));
        assert!(transcript_has_status(&controller, "Left"));
    }

    #[tokio::test]
    async fn background_question_answer_starts_followup_turn() {
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_requests = std::sync::Arc::clone(&requests);
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "session",
            "model",
            test_workspace_root(),
            move |request| {
                let captured_requests = std::sync::Arc::clone(&captured_requests);
                async move {
                    captured_requests.lock().expect("requests").push(request);
                    Ok(Vec::new())
                }
            },
        );
        let (response_tx, mut response_rx) = oneshot::channel();
        controller.register_pending_question(PendingQuestion {
            id: "question-1".to_owned(),
            questions: vec![neo_agent_core::QuestionEventData {
                question: "Pick a side?".to_owned(),
                header: Some("Choice".to_owned()),
                body: None,
                options: vec![
                    neo_agent_core::QuestionOptionData {
                        label: "Left".to_owned(),
                        description: None,
                    },
                    neo_agent_core::QuestionOptionData {
                        label: "Right".to_owned(),
                        description: None,
                    },
                ],
                multi_select: false,
            }],
            response_tx,
        });

        controller
            .resolve_question("question-1", vec!["Left".to_owned()])
            .await
            .expect("question resolves");
        controller
            .wait_for_active_turn()
            .await
            .expect("followup completes");

        assert_eq!(
            response_rx
                .try_recv()
                .expect("response should be sent")
                .answers,
            vec!["Left"]
        );
        let requests = requests.lock().expect("requests");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].session_id.as_deref(), None);
        assert!(requests[0].prompt[0].contains("Background question `question-1`"));
        assert!(requests[0].prompt[0].contains("TaskOutput"));
    }

    #[test]
    fn task_stop_for_question_closes_pending_question_overlay() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "session",
            "model",
            test_workspace_root(),
            |_| async { Ok(Vec::new()) },
        );
        let (response_tx, _response_rx) = oneshot::channel();
        controller.register_pending_question(PendingQuestion {
            id: "question-1".to_owned(),
            questions: vec![neo_agent_core::QuestionEventData {
                question: "Continue?".to_owned(),
                header: None,
                body: None,
                options: vec![
                    neo_agent_core::QuestionOptionData {
                        label: "Yes".to_owned(),
                        description: None,
                    },
                    neo_agent_core::QuestionOptionData {
                        label: "No".to_owned(),
                        description: None,
                    },
                ],
                multi_select: false,
            }],
            response_tx,
        });
        assert!(controller.chrome().question_dialog_is_focused());

        controller.apply_turn_event(AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "tool-1".to_owned(),
            name: "TaskStop".to_owned(),
            result: neo_agent_core::ToolResult::ok("stopped").with_details(serde_json::json!({
                "task_id": "question-1",
                "kind": "question",
                "status": "stopped"
            })),
        });

        assert!(!controller.chrome().question_dialog_is_focused());
        assert!(!controller.pending_questions.contains_key("question-1"));
        assert!(
            !controller
                .pending_question_prompts
                .contains_key("question-1")
        );
    }

    #[test]
    fn neo_tui_draw_composes_body_then_chrome_in_one_frame() {
        let mut app = NeoChromeState::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
        );
        app.prompt_mut().text = "next".to_owned();
        app.prompt_mut().cursor = 4;
        let mut transcript = TranscriptPane::new(80, 12);
        transcript.push_banner("Welcome to neo");
        transcript.apply_agent_event(AgentEvent::ToolExecutionStarted {
            turn: 0,
            id: "tool-1".to_owned(),
            name: "Bash".to_owned(),
            arguments: serde_json::json!({ "command": "cargo test" }),
        });

        let lines = compose_tui_frame(&app, &mut transcript, 80, 12)
            .expect("transcript frame composes body + chrome");

        let joined = lines
            .iter()
            .map(|line| neo_tui::ansi::strip_ansi(line))
            .collect::<Vec<_>>()
            .join("\n");
        // Banner (finalized) appears in the body before the running tool card,
        // which appears before the prompt chrome.
        let welcome = joined.find("Welcome to neo").expect("welcome in body");
        let tool = joined.find("Using Bash").expect("running tool in body");
        let prompt = joined.find("> next").expect("prompt chrome at tail");
        assert!(welcome < tool, "banner should precede the tool card");
        assert!(tool < prompt, "tool card should precede the prompt chrome");
        // The running tool card is live (● Using), not finalized (● Used).
        assert!(!joined.contains("Used Bash"));
    }

    #[test]
    fn neo_tui_draw_replays_finished_tool_before_prompt_chrome() {
        let mut app = NeoChromeState::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
        );
        app.prompt_mut().text = "next".to_owned();
        app.prompt_mut().cursor = 4;
        let loaded = LoadedSessionTranscript::new(
            "alpha",
            Vec::new(),
            [
                AgentMessage::user_text("inspect"),
                AgentMessage::assistant(
                    [Content::text("reading")],
                    [neo_agent_core::AgentToolCall {
                        id: "tool-1".to_owned(),
                        name: "Read".to_owned(),
                        arguments: serde_json::json!({ "path": "README.md" }),
                    }],
                    StopReason::ToolUse,
                ),
                AgentMessage::tool_result(
                    "tool-1",
                    "Read",
                    [Content::text("README contents")],
                    false,
                ),
            ],
        );
        let mut transcript = TranscriptPane::new(80, 12);
        transcript.push_banner("Welcome to neo");
        replay_session_into_transcript(&mut transcript, &loaded);

        let lines = compose_tui_frame(&app, &mut transcript, 80, 12)
            .expect("transcript frame composes replay");

        // Tool header spans are individually ANSI-colored, so strip codes
        // before substring searching for the committed tool card.
        let plain: Vec<String> = lines
            .iter()
            .map(|line| neo_tui::ansi::strip_ansi(line))
            .collect();
        let joined = plain.join("\n");
        let welcome = joined.find("Welcome to neo").expect("welcome in body");
        let prompt = joined.find("> next").expect("prompt chrome live row");
        let tool = joined
            .find("Used Read (README.md)")
            .expect("tool committed");
        assert!(welcome < tool);
        assert!(tool < prompt);
        assert!(!joined.contains("Using Read"));
    }

    #[tokio::test]
    async fn controller_snapshot_uses_transcript_tool_card_rendering() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move {
                Ok(vec![
                    AgentEvent::ToolExecutionStarted {
                        turn: 1,
                        id: "tool-1".to_owned(),
                        name: "Read".to_owned(),
                        arguments: serde_json::json!({ "path": "README.md" }),
                    },
                    AgentEvent::ToolExecutionFinished {
                        turn: 1,
                        id: "tool-1".to_owned(),
                        name: "Read".to_owned(),
                        result: ToolResult::ok("line one\nline two"),
                    },
                ])
            },
        );

        controller.type_text("inspect");
        let snapshot = controller.submit_prompt().await.expect("prompt succeeds");

        assert!(
            snapshot.contains("● Used Read (README.md)"),
            "transcript snapshot should include finalized tool card, got:\n{snapshot}"
        );
        assert!(snapshot.contains("> "));
    }

    #[tokio::test]
    async fn controller_submits_prompt_reduces_turn_events_and_renders_snapshot() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
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

        assert!(snapshot.contains("Welcome to neo"));
        assert!(snapshot.contains("test-session"));
        assert!(snapshot.contains("openai/gpt-4.1"));
        // The user prompt and assistant reply appear in the rendered frame.
        assert!(snapshot.contains("hello neo"));
        assert!(snapshot.contains("Hello, Neo"));
        assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
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
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
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
            .run_terminal_loop_with_suspend(
                |tui| {
                    rendered.push(render_tui_snapshot(tui));
                    Ok(())
                },
                || Ok(()),
                FakeEvents {
                    events: vec![
                        InputEvent::Insert('h'),
                        InputEvent::Insert('i'),
                        InputEvent::Submit,
                        InputEvent::Interrupt,
                        InputEvent::Interrupt,
                        InputEvent::Interrupt,
                    ]
                    .into_iter(),
                },
            )
            .await
            .expect("event loop succeeds");

        assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
        assert!(rendered.iter().any(|snapshot| snapshot.contains("> hi")));
        assert!(
            rendered
                .last()
                .expect("final render")
                .contains("hello from controller")
        );
    }

    #[tokio::test]
    async fn event_loop_reports_turn_error_and_keeps_running() {
        use std::collections::VecDeque;

        struct ScriptedEvents {
            events: VecDeque<InputEvent>,
        }

        impl TerminalEvents for ScriptedEvents {
            fn next_input_event(&mut self) -> Result<InputEvent> {
                self.events
                    .pop_front()
                    .ok_or_else(|| anyhow::anyhow!("expected scripted input"))
            }
        }

        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { anyhow::bail!("provider stream error: http status 400") },
        );
        let mut prompt_snapshots = Vec::new();

        controller.type_text("trigger error");
        controller
            .run_terminal_loop(
                |app| {
                    prompt_snapshots.push(app.prompt().text.clone());
                    Ok(())
                },
                ScriptedEvents {
                    events: VecDeque::from([
                        InputEvent::Submit,
                        InputEvent::Insert('o'),
                        InputEvent::Insert('k'),
                        InputEvent::Interrupt,
                        InputEvent::Interrupt,
                        InputEvent::Interrupt,
                    ]),
                },
            )
            .await
            .expect("turn error should not exit the interactive loop");

        let snapshot = controller.render_snapshot();
        assert!(snapshot.contains("Error: provider stream error: http status 400"));
        assert!(prompt_snapshots.iter().any(|prompt| prompt == "ok"));
        assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
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
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |request| async move {
                assert_eq!(request.prompt, vec!["alpha\nbeta".to_owned()]);
                Ok(vec![AgentEvent::TurnFinished {
                    turn: 1,
                    stop_reason: StopReason::EndTurn,
                }])
            },
        );

        controller
            .run_terminal_loop_with_suspend(
                |tui| {
                    rendered.push(render_tui_snapshot(tui));
                    Ok(())
                },
                || Ok(()),
                FakeEvents {
                    events: vec![
                        InputEvent::Paste("alpha\nbeta".to_owned()),
                        InputEvent::Submit,
                        InputEvent::Interrupt,
                        InputEvent::Interrupt,
                        InputEvent::Interrupt,
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
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move {
                panic!("resize should not submit a turn");
                #[allow(unreachable_code)]
                Ok(Vec::<AgentEvent>::new())
            },
        );

        controller
            .run_terminal_loop_with_suspend(
                |tui| {
                    rendered.push(render_tui_snapshot(tui));
                    Ok(())
                },
                || Ok(()),
                FakeEvents {
                    events: vec![
                        InputEvent::Insert('h'),
                        InputEvent::Resize {
                            columns: 100,
                            rows: 30,
                        },
                        InputEvent::Interrupt,
                        InputEvent::Interrupt,
                        InputEvent::Interrupt,
                    ]
                    .into_iter(),
                },
            )
            .await
            .expect("event loop succeeds");

        assert_eq!(rendered.len(), 4);
        assert!(rendered[1].contains("> h"));
        assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
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

        let mut controller = InteractiveController::new_with_event_driver(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
            PickerCatalogs {
                session_items: vec![test_session_summary(
                    "alpha",
                    "Alpha",
                    test_workspace_root(),
                    "session",
                )],
                session_error: None,
                model_items: Vec::new(),
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

        let mut last_prompt_text = String::new();
        let mut last_prompt_cursor = 0usize;

        controller
            .run_terminal_loop(
                |app| {
                    let prompt = app.prompt();
                    if !prompt.text.is_empty() {
                        last_prompt_text = prompt.text.clone();
                        last_prompt_cursor = prompt.cursor;
                    }
                    Ok(())
                },
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
                        InputEvent::Interrupt,
                        InputEvent::Interrupt,
                        InputEvent::Interrupt,
                    ]
                    .into_iter(),
                },
            )
            .await
            .expect("event loop succeeds");

        assert_eq!(controller.chrome().copy_buffer(), Some("hello brave world"));
        assert_eq!(last_prompt_text, "hello \tworld");
        assert_eq!(last_prompt_cursor, 7);
    }

    #[tokio::test]
    async fn event_loop_default_ctrl_c_clears_prompt_instead_of_copying() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.set_clipboard_writer(Arc::new(|_text| Ok(())));

        controller.type_text("copy through keybinding");
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
            .await
            .expect("clear keybinding handled");

        assert_eq!(controller.chrome().copy_buffer(), None);
        assert_eq!(controller.chrome().prompt().text, "");
    }

    #[tokio::test]
    async fn event_loop_copy_action_writes_prompt_to_injected_clipboard() {
        let copied = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = std::sync::Arc::clone(&copied);
        let mut controller = InteractiveController::new_with_event_driver(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
            PickerCatalogs {
                session_items: vec![test_session_summary(
                    "alpha",
                    "Alpha",
                    test_workspace_root(),
                    "session",
                )],
                session_error: None,
                model_items: Vec::new(),
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
            .handle_input_event(InputEvent::Action(KeybindingAction::InputCopy))
            .await
            .expect("copy action succeeds");

        assert_eq!(
            copied.lock().expect("clipboard writes").as_slice(),
            ["copy to system clipboard"]
        );
        assert_eq!(
            controller.chrome().copy_buffer(),
            Some("copy to system clipboard")
        );
    }

    #[tokio::test]
    async fn event_loop_ctrl_c_prefers_selected_transcript_region() {
        let copied = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = std::sync::Arc::clone(&copied);
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
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
            .transcript_mut()
            .push_user_message("selected user prompt");
        controller
            .transcript_mut()
            .push_assistant_message("selected assistant reply");
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
        assert_eq!(controller.chrome().copy_buffer(), None);
        assert_eq!(
            controller.chrome().prompt().text,
            "prompt text stays out of clipboard"
        );
    }

    #[tokio::test]
    async fn event_loop_clipboard_failure_keeps_internal_copy_buffer() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
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

        assert_eq!(controller.chrome().copy_buffer(), Some("copy fallback"));
        assert!(transcript_entries(&controller).iter().any(|entry| {
            matches!(
                entry,
                TranscriptEntry::Status { text, .. }
                    if text.contains("Clipboard copy failed")
                        && text.contains("clipboard unavailable")
            )
        }));
    }

    #[tokio::test]
    async fn event_loop_ctrl_c_cancels_overlay_without_copying_prompt() {
        let copied = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = std::sync::Arc::clone(&copied);
        let mut controller = InteractiveController::new_with_event_driver(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
            PickerCatalogs {
                session_items: vec![test_session_summary(
                    "alpha",
                    "Alpha",
                    test_workspace_root(),
                    "session",
                )],
                session_error: None,
                model_items: Vec::new(),
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
        assert!(controller.chrome().focused_overlay().is_some());

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
            .await
            .expect("overlay cancel succeeds");

        assert!(controller.chrome().focused_overlay().is_none());
        assert_eq!(controller.chrome().copy_buffer(), None);
        assert!(copied.lock().expect("clipboard writes").is_empty());
    }

    #[tokio::test]
    async fn event_loop_ctrl_c_clears_prompt_before_confirming_exit() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );

        controller.type_text("draft prompt");
        let should_exit = controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
            .await
            .expect("ctrl-c handles prompt clear");

        assert!(!should_exit);
        assert_eq!(controller.chrome().prompt().text, "");
        assert_eq!(
            controller.chrome().exit_confirmation_label(),
            Some("Press Ctrl-C again to exit")
        );
        assert!(!transcript_has_status(
            &controller,
            "Press Ctrl-C again to exit"
        ));
    }

    #[tokio::test]
    async fn event_loop_ctrl_c_requires_second_press_to_exit_when_prompt_is_empty() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );

        let first = controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
            .await
            .expect("first ctrl-c prompts");
        let second = controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
            .await
            .expect("second ctrl-c exits");

        assert!(!first);
        assert!(second);
    }

    #[tokio::test]
    async fn event_loop_ctrl_c_key_cancels_active_turn_instead_of_confirming_exit() {
        let captured_token = Arc::new(std::sync::Mutex::new(None));
        let observed_token = Arc::clone(&captured_token);
        let run_turn: TurnDriver = Arc::new(move |_request, channels| {
            let observed_token = Arc::clone(&observed_token);
            *observed_token.lock().expect("token lock") = Some(channels.cancel_token.clone());
            Box::pin(async move {
                channels.send_event(AgentEvent::TextDelta {
                    turn: 1,
                    text: "started".to_owned(),
                });
                channels.cancel_token.cancelled().await;
                Ok(TurnOutcome::default())
            })
        });
        let mut controller = InteractiveController::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            run_turn,
            PickerCatalogs::default(),
            Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        );

        controller.type_text("cancel me");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("prompt submits");

        assert!(controller.active_turn.is_some());

        let should_exit = controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
            .await
            .expect("ctrl-c cancels active turn");

        let token = captured_token
            .lock()
            .expect("token lock")
            .clone()
            .expect("turn token captured");
        assert!(!should_exit);
        assert!(token.is_cancelled());
        assert_eq!(controller.chrome().exit_confirmation_label(), None);
        assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
        assert!(controller.active_turn.is_none());
    }

    #[tokio::test]
    async fn event_loop_ctrl_c_clears_stale_working_state_before_exit_confirmation() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );

        controller
            .tui
            .chrome_mut()
            .apply_agent_event(AgentEvent::ToolExecutionStarted {
                turn: 1,
                id: "ask".to_owned(),
                name: "AskUserQuestion".to_owned(),
                arguments: serde_json::json!({ "questions": [] }),
            });
        assert!(controller.chrome().working_label().is_some());

        let should_exit = controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
            .await
            .expect("ctrl-c clears stale working state");

        assert!(!should_exit);
        assert!(controller.chrome().working_label().is_none());
        assert_eq!(controller.chrome().exit_confirmation_label(), None);

        controller
            .handle_input_event(InputEvent::Insert('o'))
            .await
            .expect("typing after stale interrupt succeeds");
        controller
            .handle_input_event(InputEvent::Insert('k'))
            .await
            .expect("typing after stale interrupt succeeds");
        assert_eq!(controller.chrome().prompt().text, "ok");
    }

    #[tokio::test]
    async fn event_loop_ctrl_d_deletes_forward_until_prompt_is_empty_then_confirms_exit() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );

        controller.type_text("ab");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::EditorCursorLineStart))
            .await
            .expect("move cursor to start");
        let delete = controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+d").expect("valid key")))
            .await
            .expect("ctrl-d deletes while prompt has text");
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+d").expect("valid key")))
            .await
            .expect("ctrl-d deletes final char");
        let first_exit = controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+d").expect("valid key")))
            .await
            .expect("first empty ctrl-d prompts");
        let second_exit = controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+d").expect("valid key")))
            .await
            .expect("second empty ctrl-d exits");

        assert!(!delete);
        assert_eq!(controller.chrome().prompt().text, "");
        assert!(!first_exit);
        assert!(second_exit);
        assert_eq!(controller.chrome().exit_confirmation_label(), None);
        assert!(!transcript_has_status(
            &controller,
            "Press Ctrl-D again to exit"
        ));
    }

    #[tokio::test]
    async fn event_loop_ctrl_z_reports_suspend_request() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );

        let should_exit = controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+z").expect("valid key")))
            .await
            .expect("ctrl-z is handled");

        assert!(!should_exit);
        assert!(controller.take_suspend_requested());
    }

    #[tokio::test]
    async fn event_loop_tabs_through_real_filesystem_prompt_completions() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(temp.path().join("src")).expect("create src");
        fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").expect("write main");
        fs::write(temp.path().join("src/matrix.rs"), "pub fn matrix() {}\n").expect("write matrix");

        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
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
                .chrome()
                .focused_overlay()
                .map(|overlay| &overlay.kind),
            Some(OverlayKind::PromptCompletion(_))
        ));

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("completion confirms");

        assert_eq!(controller.chrome().prompt().text, "open src/main.rs");
        assert_eq!(controller.chrome().prompt().cursor, 16);
        assert!(controller.chrome().focused_overlay().is_none());
    }

    #[tokio::test]
    async fn event_loop_opens_slash_completion_after_typing_slash() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );

        controller
            .handle_input_event(InputEvent::Insert('/'))
            .await
            .expect("slash insert opens completion");

        assert_eq!(controller.chrome().prompt().text, "/");
        assert!(matches!(
            controller
                .chrome()
                .focused_overlay()
                .map(|overlay| &overlay.kind),
            Some(OverlayKind::PromptCompletion(_))
        ));
        assert!(
            controller.chrome().selected_prompt_completion().is_some(),
            "slash completion should select the first local command"
        );
    }

    #[tokio::test]
    async fn event_loop_backspace_deletes_slash_while_completion_is_open() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );

        controller
            .handle_input_event(InputEvent::Insert('/'))
            .await
            .expect("slash insert opens completion");

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("backspace").expect("valid key")))
            .await
            .expect("backspace edits prompt");

        assert_eq!(controller.chrome().prompt().text, "");
        assert_eq!(controller.chrome().prompt().cursor, 0);
        assert!(controller.chrome().focused_overlay().is_none());
    }

    #[tokio::test]
    async fn event_loop_escape_closes_slash_completion_without_exiting() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );

        controller
            .handle_input_event(InputEvent::Insert('/'))
            .await
            .expect("slash insert opens completion");
        let should_exit = controller
            .handle_input_event(InputEvent::Cancel)
            .await
            .expect("escape closes completion");

        assert!(!should_exit);
        assert_eq!(controller.chrome().prompt().text, "/");
        assert!(controller.chrome().focused_overlay().is_none());
    }

    #[tokio::test]
    async fn event_loop_escape_cancels_active_turn() {
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
                Ok(self
                    .events
                    .pop_front()
                    .unwrap_or(Some(InputEvent::Interrupt)))
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
                Ok(TurnOutcome::default())
            })
        });
        let mut controller = InteractiveController::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
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
                        // ESC should cancel the active turn
                        Some(InputEvent::Cancel),
                        // After cancellation the app is idle; two Interrupts to exit
                        Some(InputEvent::Interrupt),
                        Some(InputEvent::Interrupt),
                    ]),
                },
            )
            .await
            .expect("escape cancels turn and loop exits");

        let token = captured_token
            .lock()
            .expect("token lock")
            .clone()
            .expect("turn token captured");
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn event_loop_escape_is_noop_when_idle() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );

        controller.type_text("hello");

        // ESC when idle (no overlay, no active turn) should be a no-op
        let should_exit = controller
            .handle_input_event(InputEvent::Cancel)
            .await
            .expect("escape is no-op when idle");

        assert!(!should_exit, "ESC should not exit the app when idle");
        // Prompt text should be preserved (ESC is not clearing it)
        assert_eq!(controller.chrome().prompt().text, "hello");
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
                .chrome()
                .focused_overlay()
                .map(|overlay| &overlay.kind),
            Some(OverlayKind::CommandPalette(_))
        ));
    }

    #[test]
    fn auto_image_protocol_uses_positive_runtime_hints_on_local_terminals() {
        let env = |name: &str| match name {
            "TERM" => Ok("xterm-kitty".to_owned()),
            "TERM_PROGRAM" => Ok("WezTerm".to_owned()),
            "KITTY_WINDOW_ID" => Ok("1".to_owned()),
            "WEZTERM_PANE" => Ok("2".to_owned()),
            _ => Err(env::VarError::NotPresent),
        };

        let capabilities =
            terminal_image_capabilities_for_policy(ImageProtocolPreference::Auto, env);

        assert!(capabilities.kitty());
        assert!(!capabilities.iterm2());
        assert!(!capabilities.sixel());
    }

    #[test]
    fn auto_image_protocol_falls_back_inside_tmux_screen_or_ssh() {
        let tmux_env = |name: &str| match name {
            "TERM" => Ok("xterm-kitty".to_owned()),
            "KITTY_WINDOW_ID" | "TMUX" => Ok("1".to_owned()),
            _ => Err(env::VarError::NotPresent),
        };
        let ssh_env = |name: &str| match name {
            "TERM_PROGRAM" => Ok("iTerm.app".to_owned()),
            "SSH_CONNECTION" => Ok("127.0.0.1 1 127.0.0.1 2".to_owned()),
            _ => Err(env::VarError::NotPresent),
        };

        assert_eq!(
            terminal_image_capabilities_for_policy(ImageProtocolPreference::Auto, tmux_env),
            TerminalImageCapabilities::default()
        );
        assert_eq!(
            terminal_image_capabilities_for_policy(ImageProtocolPreference::Auto, ssh_env),
            TerminalImageCapabilities::default()
        );
    }

    #[test]
    fn explicit_image_protocol_uses_matching_static_terminal_hints() {
        let env = |name: &str| match name {
            "TERM" => Ok("xterm-kitty".to_owned()),
            "TERM_PROGRAM" => Ok("WezTerm".to_owned()),
            "KITTY_WINDOW_ID" => Ok("1".to_owned()),
            _ => Err(env::VarError::NotPresent),
        };

        let capabilities =
            terminal_image_capabilities_for_policy(ImageProtocolPreference::Kitty, env);

        assert!(capabilities.kitty());
        assert!(!capabilities.iterm2());
        assert!(!capabilities.sixel());
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

        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.completion_root = temp.path().to_path_buf();

        controller.type_text("/rev");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
            .await
            .expect("tab completes slash prompt");

        assert_eq!(controller.chrome().prompt().text, "/review");
        assert_eq!(controller.chrome().prompt().cursor, 7);
        assert!(controller.chrome().focused_overlay().is_none());
    }

    #[tokio::test]
    async fn tab_confirms_selected_prompt_completion() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );

        controller.type_text("/");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
            .await
            .expect("tab opens completion picker");

        assert!(controller.chrome().focused_overlay().is_some());
        assert!(controller.chrome().selected_prompt_completion().is_some());

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
            .await
            .expect("tab confirms selected completion");

        assert!(controller.chrome().focused_overlay().is_none());
        assert!(!controller.chrome().prompt().text.is_empty());
    }

    #[tokio::test]
    async fn slash_picker_commands_do_not_enter_streaming_mode() {
        for command in ["/model", "/provider"] {
            let mut controller = InteractiveController::new_for_test(
                "neo",
                "test-session",
                "openai/gpt-4.1",
                test_workspace_root(),
                |_request| async move { Ok(Vec::<AgentEvent>::new()) },
            );
            controller.type_text(command);
            controller
                .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
                .await
                .unwrap_or_else(|e| panic!("{command} submit failed: {e}"));
            assert_eq!(
                controller.chrome().mode(),
                ChromeMode::Editing,
                "{command} should keep editing mode"
            );
            assert!(
                controller.chrome().prompt().text.is_empty(),
                "{command} should leave the prompt empty"
            );
        }
    }

    #[test]
    fn prompt_completions_merges_real_prompt_package_extension_and_session_commands() {
        let temp = tempfile::tempdir().expect("tempdir");
        let prompts_dir = temp.path().join(".neo/prompts");
        fs::create_dir_all(prompts_dir.join("review-pack")).expect("create prompts");
        fs::write(
            prompts_dir.join("review.md"),
            "---\ndescription: Review local changes\n---\nReview $1.\n",
        )
        .expect("write local prompt");
        fs::write(
            prompts_dir.join("review-pack/refactor.md"),
            "---\ndescription: Refactor from package\n---\nRefactor $1.\n",
        )
        .expect("write packaged prompt");
        let extension_dir = temp.path().join(".neo/extensions/echo");
        fs::create_dir_all(&extension_dir).expect("create extension");
        fs::write(
            extension_dir.join("neo-extension.toml"),
            r#"
id = "echo"
name = "Echo Tools"
version = "0.1.0"
description = "Local echo extension"

[runner]
command = "echo"
"#,
        )
        .expect("write extension manifest");

        let completions =
            prompt_completions(temp.path(), "/", &[], None).expect("slash completions");
        let by_value = completions
            .iter()
            .map(|item| (item.value.as_str(), item))
            .collect::<BTreeMap<_, _>>();

        assert!(
            by_value["/review"]
                .description
                .as_deref()
                .is_some_and(|description| description.contains("source: slash prompt"))
        );
        assert!(
            by_value["/refactor"]
                .description
                .as_deref()
                .is_some_and(|description| {
                    description.contains("source: prompt package")
                        && description.contains("provider: review-pack")
                })
        );
        assert!(
            by_value["/echo"]
                .description
                .as_deref()
                .is_some_and(|description| {
                    description.contains("source: extension command")
                        && description.contains("provider: echo")
                        && description.contains("trust: local extension")
                })
        );
        assert!(
            by_value["/resume"]
                .description
                .as_deref()
                .is_some_and(|description| {
                    description.contains("source: session command")
                        && description.contains("provider: local sessions")
                        && description.contains("trust: local")
                })
        );
        assert!(!by_value.contains_key("/tree"));
        assert!(!by_value.contains_key("/sync"));
    }

    #[test]
    fn verbose_startup_mentions_local_keybinding_overrides() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
        config
            .tui
            .keybindings
            .insert("tui.input.submit".to_owned(), vec!["ctrl+j".to_owned()]);

        let mut controller = controller_for_config(&config);
        controller.apply_startup_options(
            &config,
            InteractiveOptions {
                verbose_startup: true,
            },
        );
        assert!(transcript_has_status(
            &controller,
            "keybindings: 1 override"
        ));
    }

    #[test]
    fn autocomplete_source_model_merges_local_commands_and_provider_models_with_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(temp.path().join("src")).expect("create src");
        fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").expect("write main");

        let catalog = CompletionCatalog {
            slash_prompts: vec![PickerItem::new(
                "/review",
                "/review",
                Some("Review project changes"),
            )],
            prompt_packages: vec![PickerItem::new(
                "/review-package",
                "/review-package",
                Some("Packaged review prompt"),
            )],
            extension_commands: vec![PickerItem::new(
                "/review-extension",
                "/review-extension",
                Some("Extension command"),
            )],
            session_commands: vec![PickerItem::new(
                "/review-session",
                "/review-session",
                Some("Session command"),
            )],
            model_items: vec![PickerItem::new(
                "anthropic/claude-sonnet",
                "anthropic/claude-sonnet",
                Some("Messages"),
            )],
        };

        let files = completion_source_candidates(temp.path(), "src/ma", &catalog)
            .expect("file completions");
        assert!(files.iter().any(|candidate| {
            candidate.value == "src/main.rs"
                && candidate.source == CompletionSource::LocalFile
                && candidate.source_label == "local file"
        }));

        let slash =
            completion_source_candidates(temp.path(), "/rev", &catalog).expect("slash completions");
        let slash_sources = slash
            .iter()
            .map(|candidate| candidate.source)
            .collect::<Vec<_>>();
        assert!(slash_sources.contains(&CompletionSource::SlashPrompt));
        assert!(slash_sources.contains(&CompletionSource::PromptPackage));
        assert!(slash_sources.contains(&CompletionSource::ExtensionCommand));
        assert!(slash_sources.contains(&CompletionSource::SessionCommand));
        assert!(slash.iter().any(|candidate| {
            candidate
                .to_picker_item()
                .description
                .as_deref()
                .is_some_and(|description| description.contains("source: extension command"))
        }));

        let models = completion_source_candidates(temp.path(), "@anth", &catalog)
            .expect("model completions");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].value, "@anthropic/claude-sonnet");
        assert_eq!(models[0].source, CompletionSource::ProviderModel);
        assert_eq!(models[0].source_label, "provider model");
    }

    #[test]
    fn prompt_completions_include_installed_extension_commands() {
        let temp = tempfile::tempdir().expect("tempdir");
        let extension = temp.path().join(".neo/extensions/review-extension");
        fs::create_dir_all(&extension).expect("create extension");
        fs::write(
            extension.join("neo-extension.toml"),
            r#"
id = "review-extension"
name = "Review Extension"
version = "0.1.0"
description = "Run extension review"

[runner]
command = "python3"
"#,
        )
        .expect("write manifest");

        let completions =
            prompt_completions(temp.path(), "/rev", &[], None).expect("prompt completions");

        assert!(completions.iter().any(|item| {
            item.value == "/review-extension"
                && item
                    .description
                    .as_deref()
                    .is_some_and(|description| description.contains("source: extension command"))
        }));
    }

    #[tokio::test]
    async fn event_loop_slash_resume_opens_local_session_picker() {
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_requests = std::sync::Arc::clone(&requests);
        let mut controller = InteractiveController::new_with_event_driver(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
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
                session_items: vec![test_session_summary(
                    "alpha",
                    "Alpha",
                    test_workspace_root(),
                    "root",
                )],
                session_error: None,
                model_items: Vec::new(),
            },
            |session_id| async move {
                Ok(LoadedSessionTranscript::new(
                    session_id,
                    Vec::new(),
                    Vec::new(),
                ))
            },
        );

        controller.type_text("/resume");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("slash resume command runs locally");

        assert!(matches!(
            controller
                .chrome()
                .focused_overlay()
                .map(|overlay| &overlay.kind),
            Some(OverlayKind::SessionPicker(_))
        ));
        assert!(controller.chrome().prompt().text.is_empty());
        assert!(requests.lock().expect("recorded requests").is_empty());
    }

    #[test]
    fn event_loop_slash_tree_absent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let completions =
            prompt_completions(temp.path(), "/", &[], None).expect("slash completions");
        assert!(
            !completions.iter().any(|item| item.value == "/tree"),
            "/tree should not appear in slash completion items"
        );
    }

    #[tokio::test]
    async fn event_loop_tab_completes_provider_model_prefix() {
        let mut controller = InteractiveController::new_with_event_driver(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
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

        assert_eq!(
            controller.chrome().prompt().text,
            "@anthropic/claude-sonnet"
        );
        assert_eq!(controller.chrome().prompt().cursor, 24);
        assert!(controller.chrome().focused_overlay().is_none());
    }

    #[tokio::test]
    async fn event_loop_inline_provider_model_prefix_overrides_submitted_turn() {
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_requests = std::sync::Arc::clone(&requests);
        let mut controller = InteractiveController::new_with_event_driver(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
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
        let mut controller = InteractiveController::new_with_event_driver(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
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
        let mut controller = InteractiveController::new_with_event_driver(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
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

        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.completion_root = temp.path().to_path_buf();

        controller.type_text("open R");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
            .await
            .expect("tab extends common prefix");

        assert_eq!(controller.chrome().prompt().text, "open RE");
        assert_eq!(controller.chrome().prompt().cursor, 7);
        assert!(controller.chrome().focused_overlay().is_none());
    }

    #[tokio::test]
    async fn event_loop_dispatches_editor_scroll_actions_to_transcript_view() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        for index in 0..10 {
            controller
                .transcript_mut()
                .push_status(format!("line {index}"));
        }
        controller.transcript_mut().sync_transcript_view(10, 2);

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::EditorPageUp))
            .await
            .expect("page up scrolls transcript");
        assert_eq!(transcript_scrollback(&controller), 8);

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::EditorCursorDown))
            .await
            .expect("cursor down scrolls transcript toward bottom");
        assert_eq!(transcript_scrollback(&controller), 8);

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::EditorPageDown))
            .await
            .expect("page down returns transcript to bottom");
        assert_eq!(transcript_scrollback(&controller), 0);
    }

    #[tokio::test]
    async fn event_loop_uses_up_down_keys_for_prompt_history() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );

        controller.type_text("first prompt");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("first prompt submits");
        controller
            .wait_for_active_turn()
            .await
            .expect("first turn completes");

        controller.type_text("second prompt");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("second prompt submits");
        controller
            .wait_for_active_turn()
            .await
            .expect("second turn completes");

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
            .await
            .expect("up recalls latest prompt");
        assert_eq!(controller.chrome().prompt().text, "second prompt");

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
            .await
            .expect("up recalls older prompt");
        assert_eq!(controller.chrome().prompt().text, "first prompt");

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
            .await
            .expect("down moves toward newer prompt");
        assert_eq!(controller.chrome().prompt().text, "second prompt");

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
            .await
            .expect("down restores empty draft");
        assert_eq!(controller.chrome().prompt().text, "");
    }

    #[tokio::test]
    async fn event_loop_dispatches_mouse_wheel_to_transcript_view() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.transcript_mut().sync_transcript_view(30, 6);

        controller
            .handle_input_event(InputEvent::ScrollUp(3))
            .await
            .expect("wheel up scrolls transcript toward older rows");
        assert_eq!(transcript_scrollback(&controller), 3);

        controller
            .handle_input_event(InputEvent::ScrollDown(2))
            .await
            .expect("wheel down scrolls transcript toward newest rows");
        assert_eq!(transcript_scrollback(&controller), 1);

        controller
            .handle_input_event(InputEvent::ScrollDown(3))
            .await
            .expect("wheel down follows tail at bottom");
        assert_eq!(transcript_scrollback(&controller), 0);
    }

    #[tokio::test]
    async fn event_loop_ctrl_o_toggles_tool_detail() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller
            .transcript_mut()
            .apply_agent_event(AgentEvent::ToolExecutionStarted {
                turn: 1,
                id: "tool-1".to_owned(),
                name: "Read".to_owned(),
                arguments: serde_json::json!({ "path": "README.md" }),
            });
        controller
            .transcript_mut()
            .apply_agent_event(AgentEvent::ToolExecutionFinished {
                turn: 1,
                id: "tool-1".to_owned(),
                name: "Read".to_owned(),
                result: ToolResult::ok("expanded file content"),
            });
        controller
            .transcript_mut()
            .select_visible_transcript_entry();

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+o").expect("valid key")))
            .await
            .expect("ctrl-o key toggles tool detail");

        assert!(controller.chrome().focused_overlay().is_none());
        assert!(controller.transcript().tool_output_expanded());
    }

    #[tokio::test]
    async fn event_loop_model_picker_action_opens_model_picker() {
        let mut controller = InteractiveController::new_with_event_driver(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
            PickerCatalogs {
                session_items: Vec::new(),
                session_error: None,
                model_items: vec![PickerItem::new(
                    "openai/gpt-4.1",
                    "openai/gpt-4.1",
                    Some("test model"),
                )],
            },
            empty_session_loader,
        );

        controller.local_config = Some(test_config_with_models(
            &test_workspace_root(),
            test_workspace_root().join(".neo/sessions"),
            BTreeMap::from([(
                "openai/gpt-4.1".to_owned(),
                ModelConfig {
                    provider: "openai".to_owned(),
                    model: "gpt-4.1".to_owned(),
                    display_name: Some("test model".to_owned()),
                    ..ModelConfig::default()
                },
            )]),
        ));
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::ModelPickerOpen))
            .await
            .expect("model picker action opens model picker");

        assert!(
            controller.chrome().tabbed_model_selector_result().is_some()
                || controller.chrome().focused_overlay().is_some()
        );
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

        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller
            .tui
            .chrome_mut()
            .request_approval("approval-1", "Run command?", "cargo test");

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("selection moves down");
        assert_eq!(
            controller.chrome().approval_choice(),
            Some(ApprovalChoice::AlwaysApprove)
        );

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectUp))
            .await
            .expect("selection moves up");
        assert_eq!(
            controller.chrome().approval_choice(),
            Some(ApprovalChoice::Approve)
        );

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("approval confirms");
        assert!(controller.chrome().focused_overlay().is_none());

        controller.tui.chrome_mut().push_overlay(Overlay::new(
            "palette",
            OverlayKind::CommandPalette(CommandPaletteState::new((0..10).map(|index| {
                CommandSpec::new(
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
            .chrome()
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
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind)
        else {
            panic!("expected command palette overlay");
        };
        assert_eq!(palette.selected_command().expect("command").id, "command-0");
        let _ = controller.tui.chrome_mut().close_focused_overlay();

        controller.tui.chrome_mut().push_overlay(Overlay::new(
            "custom",
            OverlayKind::Message("Body".to_owned()),
        ));
        controller
            .run_terminal_loop(
                |_app| Ok(()),
                FakeEvents {
                    events: vec![
                        InputEvent::Action(KeybindingAction::SelectCancel),
                        InputEvent::Interrupt,
                        InputEvent::Interrupt,
                    ]
                    .into_iter(),
                },
            )
            .await
            .expect("event loop exits after canceling overlay and receiving cancel again");

        assert!(controller.chrome().focused_overlay().is_none());
    }

    #[tokio::test]
    async fn event_loop_opens_command_palette_and_runs_local_model_command() {
        let mut controller = InteractiveController::new_with_event_driver(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
            PickerCatalogs {
                session_items: Vec::new(),
                session_error: None,
                model_items: vec![PickerItem::new(
                    "anthropic/claude-sonnet",
                    "anthropic/claude-sonnet",
                    Some("messages"),
                )],
            },
            |session_id| async move {
                Ok(LoadedSessionTranscript::new(
                    session_id,
                    Vec::new(),
                    Vec::new(),
                ))
            },
        );

        controller.local_config = Some(test_config_with_models(
            &test_workspace_root(),
            test_workspace_root().join(".neo/sessions"),
            BTreeMap::from([(
                "anthropic/claude-sonnet".to_owned(),
                ModelConfig {
                    provider: "anthropic".to_owned(),
                    model: "claude-sonnet".to_owned(),
                    display_name: Some("messages".to_owned()),
                    ..ModelConfig::default()
                },
            )]),
        ));
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+p").expect("valid key")))
            .await
            .expect("command palette opens");
        let Some(OverlayKind::CommandPalette(palette)) = controller
            .chrome()
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
                .chrome()
                .focused_overlay()
                .map(|overlay| &overlay.kind),
            Some(OverlayKind::TabbedModelSelector(_))
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
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
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
        );
        controller.completion_root = temp.path().to_path_buf();

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::CommandPaletteOpen))
            .await
            .expect("command palette opens");
        for _ in 0..32 {
            let selected = controller
                .chrome()
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
                .chrome()
                .selected_command()
                .expect("review command")
                .id,
            "prompt-template.review"
        );

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("prompt template command inserts invocation");

        assert_eq!(controller.chrome().prompt().text, "/review ");
        assert_eq!(controller.chrome().prompt().cursor, 8);
        assert!(controller.chrome().focused_overlay().is_none());

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
        let config = test_config(temp.path(), sessions_dir.clone());
        let bucket_dir = workspace_sessions_dir(&config);
        fs::create_dir_all(&bucket_dir).expect("create sessions bucket dir");
        fs::write(
            bucket_dir.join(format!("{SESSION_A}.jsonl")),
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
                .chrome()
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
                .chrome()
                .selected_command()
                .expect("export command")
                .id,
            "session.exportHtml"
        );
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("export command runs");

        let export_path = bucket_dir.join(format!("{SESSION_A}.html"));
        let html = fs::read_to_string(&export_path).expect("read exported html");
        assert!(html.contains(&format!("<title>neo session {SESSION_A}</title>")));
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(transcript_entries(&controller).iter().any(|entry| {
            matches!(
                entry,
                TranscriptEntry::Status { text, .. }
                    if text.contains(&format!("Exported session {SESSION_A} to"))
                        && text.contains(&export_path.display().to_string())
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
                .chrome()
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

        assert!(transcript_has_status(
            &controller,
            "No active session to export"
        ));
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
                Ok(self
                    .events
                    .pop_front()
                    .unwrap_or(Some(InputEvent::Interrupt)))
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
                    subject: "Write".to_owned(),
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
                Ok(TurnOutcome::default())
            })
        });
        let mut controller = InteractiveController::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
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
                        Some(InputEvent::Interrupt),
                        Some(InputEvent::Interrupt),
                    ]),
                },
            )
            .await
            .expect("approval loop completes");

        assert_eq!(
            *decisions.lock().expect("decisions lock"),
            vec![PermissionDecision::Allow]
        );
        assert!(controller.chrome().focused_overlay().is_none());
        assert!(controller.render_snapshot().contains("approved"));
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn event_loop_shows_and_resolves_pending_question_from_running_turn() {
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
                Ok(self
                    .events
                    .pop_front()
                    .unwrap_or(Some(InputEvent::Interrupt)))
            }
        }

        let answers = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_answers = std::sync::Arc::clone(&answers);
        let frames = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_frames = std::sync::Arc::clone(&frames);
        let run_turn: TurnDriver = Arc::new(move |_request, channels| {
            let captured_answers = std::sync::Arc::clone(&captured_answers);
            Box::pin(async move {
                let (response_tx, response_rx) = oneshot::channel();
                channels
                    .questions
                    .send(PendingQuestion {
                        id: "question-1".to_owned(),
                        questions: vec![neo_agent_core::QuestionEventData {
                            question: "1 + 1 = ?".to_owned(),
                            header: Some("Math".to_owned()),
                            body: None,
                            options: vec![
                                neo_agent_core::QuestionOptionData {
                                    label: "2".to_owned(),
                                    description: Some("Correct".to_owned()),
                                },
                                neo_agent_core::QuestionOptionData {
                                    label: "3".to_owned(),
                                    description: Some("Too high".to_owned()),
                                },
                            ],
                            multi_select: false,
                        }],
                        response_tx,
                    })
                    .expect("question sent");
                let response = response_rx.await.expect("question response");
                captured_answers
                    .lock()
                    .expect("answers lock")
                    .extend(response.answers);
                channels.send_event(AgentEvent::TextDelta {
                    turn: 1,
                    text: "answered".to_owned(),
                });
                channels.send_event(AgentEvent::TurnFinished {
                    turn: 1,
                    stop_reason: StopReason::EndTurn,
                });
                Ok(TurnOutcome::default())
            })
        });
        let mut controller = InteractiveController::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            run_turn,
            PickerCatalogs::default(),
            Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        );

        controller.type_text("ask me");
        controller
            .run_terminal_loop(
                move |app| {
                    captured_frames
                        .lock()
                        .expect("frames lock")
                        .push(render_overlay_snapshot(app, 80).join("\n"));
                    Ok(())
                },
                ScriptedEvents {
                    events: VecDeque::from([
                        Some(InputEvent::Submit),
                        None,
                        Some(InputEvent::Action(KeybindingAction::SelectConfirm)),
                        Some(InputEvent::Action(KeybindingAction::InputTab)),
                        Some(InputEvent::Action(KeybindingAction::SelectConfirm)),
                        None,
                        Some(InputEvent::Interrupt),
                        Some(InputEvent::Interrupt),
                    ]),
                },
            )
            .await
            .expect("question loop completes");

        assert_eq!(*answers.lock().expect("answers lock"), vec!["2"]);
        assert!(
            frames
                .lock()
                .expect("frames lock")
                .iter()
                .any(|frame| frame.contains("1 + 1 = ?") && frame.contains("[1] 2")),
            "pending question should be visible before it is answered"
        );
        assert!(controller.chrome().focused_overlay().is_none());
        assert!(
            controller
                .render_snapshot()
                .contains("Collected your answers")
        );
        assert!(controller.render_snapshot().contains("answered"));
    }

    #[tokio::test]
    async fn approval_number_shortcut_confirms_session_approval() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.apply_turn_event(AgentEvent::ApprovalRequested {
            turn: 1,
            id: "tool-1".to_owned(),
            operation: neo_agent_core::PermissionOperation::Tool,
            subject: "Write".to_owned(),
            arguments: serde_json::json!({"path": "approved.txt"}),
        });
        let (decision_tx, decision_rx) = oneshot::channel();
        controller
            .pending_approvals
            .insert("tool-1".to_owned(), decision_tx);

        controller
            .handle_input_event(InputEvent::Insert('2'))
            .await
            .expect("number shortcut handles approval");

        assert_eq!(
            decision_rx.await.expect("approval decision"),
            PermissionDecision::Allow
        );
        assert!(controller.always_approve);
        assert!(controller.chrome().focused_overlay().is_none());
        assert!(
            controller
                .render_snapshot()
                .contains("Approved for this session")
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn question_dialog_consumes_keyboard_before_prompt_editing() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.type_text("draft");
        let (response_tx, _response_rx) = oneshot::channel();
        controller.register_pending_question(PendingQuestion {
            id: "question-1".to_owned(),
            questions: vec![
                neo_agent_core::QuestionEventData {
                    question: "2 + 2 = ?".to_owned(),
                    header: Some("Single".to_owned()),
                    body: None,
                    options: vec![
                        neo_agent_core::QuestionOptionData {
                            label: "3".to_owned(),
                            description: None,
                        },
                        neo_agent_core::QuestionOptionData {
                            label: "4".to_owned(),
                            description: None,
                        },
                    ],
                    multi_select: false,
                },
                neo_agent_core::QuestionEventData {
                    question: "Pick primes".to_owned(),
                    header: Some("Multi".to_owned()),
                    body: None,
                    options: vec![
                        neo_agent_core::QuestionOptionData {
                            label: "2".to_owned(),
                            description: None,
                        },
                        neo_agent_core::QuestionOptionData {
                            label: "4".to_owned(),
                            description: None,
                        },
                    ],
                    multi_select: true,
                },
            ],
            response_tx,
        });

        controller
            .handle_input_event(InputEvent::Insert('2'))
            .await
            .expect("number shortcut selects a question option");
        assert_eq!(controller.chrome().prompt().text, "draft");
        {
            let state = controller
                .chrome()
                .question_dialog_state()
                .expect("question stays focused");
            assert_eq!(state.active_tab, 1);
            assert!(state.questions[0].selected[1]);
        }

        controller
            .handle_input_event(InputEvent::Insert('a'))
            .await
            .expect("letters are consumed by the question dialog");
        assert_eq!(controller.chrome().prompt().text, "draft");

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::EditorCursorRight))
            .await
            .expect("right arrow action switches to submit");
        assert_eq!(controller.chrome().prompt().text, "draft");
        assert!(
            controller
                .chrome()
                .question_dialog_state()
                .expect("question stays focused")
                .on_submit_tab()
        );

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::EditorCursorLeft))
            .await
            .expect("left arrow action switches back to the question");
        assert_eq!(controller.chrome().prompt().text, "draft");
        assert_eq!(
            controller
                .chrome()
                .question_dialog_state()
                .expect("question stays focused")
                .active_tab,
            1
        );

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
            .await
            .expect("tab switches to submit instead of editing the prompt");
        assert_eq!(controller.chrome().prompt().text, "draft");
        assert!(
            controller
                .chrome()
                .question_dialog_state()
                .expect("question stays focused")
                .on_submit_tab()
        );
    }

    #[tokio::test]
    async fn question_dialog_prioritizes_real_keybindings_before_prompt_editing() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.type_text("draft");
        let (response_tx, _response_rx) = oneshot::channel();
        controller.register_pending_question(PendingQuestion {
            id: "question-1".to_owned(),
            questions: vec![neo_agent_core::QuestionEventData {
                question: "Pick one".to_owned(),
                header: Some("Single".to_owned()),
                body: None,
                options: vec![
                    neo_agent_core::QuestionOptionData {
                        label: "First".to_owned(),
                        description: None,
                    },
                    neo_agent_core::QuestionOptionData {
                        label: "Second".to_owned(),
                        description: None,
                    },
                ],
                multi_select: false,
            }],
            response_tx,
        });

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("down selects Other");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("down selects Other");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("enter starts Other editing");
        controller
            .handle_input_event(InputEvent::Insert('x'))
            .await
            .expect("typed text goes to Other");
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("backspace").expect("valid key")))
            .await
            .expect("backspace edits Other text");
        {
            let state = controller
                .chrome()
                .question_dialog_state()
                .expect("question stays focused");
            assert_eq!(state.questions[0].other_text, "");
        }
        assert_eq!(controller.chrome().prompt().text, "draft");

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("right").expect("valid key")))
            .await
            .expect("right switches to submit");
        assert!(
            controller
                .chrome()
                .question_dialog_state()
                .expect("question stays focused")
                .on_submit_tab()
        );

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("left").expect("valid key")))
            .await
            .expect("left switches back to question");
        assert_eq!(
            controller
                .chrome()
                .question_dialog_state()
                .expect("question stays focused")
                .active_tab,
            0
        );

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("tab").expect("valid key")))
            .await
            .expect("tab switches to submit");
        assert!(
            controller
                .chrome()
                .question_dialog_state()
                .expect("question stays focused")
                .on_submit_tab()
        );
    }

    #[tokio::test]
    async fn approval_uses_selection_priority_for_real_keys() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.type_text("draft");
        controller.apply_turn_event(AgentEvent::ApprovalRequested {
            turn: 1,
            id: "tool-1".to_owned(),
            operation: neo_agent_core::PermissionOperation::Tool,
            subject: "Write".to_owned(),
            arguments: serde_json::json!({"path": "approved.txt"}),
        });
        let (decision_tx, decision_rx) = oneshot::channel();
        controller
            .pending_approvals
            .insert("tool-1".to_owned(), decision_tx);

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
            .await
            .expect("down selects approval option");
        assert_eq!(
            controller.chrome().approval_choice(),
            Some(ApprovalChoice::AlwaysApprove)
        );

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
            .await
            .expect("enter confirms approval");

        assert_eq!(
            decision_rx.await.expect("approval decision"),
            PermissionDecision::Allow
        );
        assert_eq!(controller.chrome().prompt().text, "draft");
        assert!(controller.chrome().focused_overlay().is_none());
    }

    #[tokio::test]
    async fn approval_revise_collects_feedback_without_editing_prompt() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.type_text("draft");
        controller.apply_turn_event(AgentEvent::ApprovalRequested {
            turn: 1,
            id: "tool-1".to_owned(),
            operation: neo_agent_core::PermissionOperation::Tool,
            subject: "Write".to_owned(),
            arguments: serde_json::json!({"path": "denied.txt"}),
        });
        let (decision_tx, decision_rx) = oneshot::channel();
        controller
            .pending_approvals
            .insert("tool-1".to_owned(), decision_tx);

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
            .await
            .expect("down selects approval option");
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
            .await
            .expect("down selects approval option");
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
            .await
            .expect("down selects revise option");
        assert_eq!(
            controller.chrome().approval_choice(),
            Some(ApprovalChoice::Revise)
        );

        controller
            .handle_input_event(InputEvent::Insert('n'))
            .await
            .expect("typed feedback is captured by approval dialog");
        controller
            .handle_input_event(InputEvent::Paste("o thanks".to_owned()))
            .await
            .expect("pasted feedback is captured by approval dialog");
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("backspace").expect("valid key")))
            .await
            .expect("backspace edits approval feedback");
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
            .await
            .expect("enter confirms revise");

        assert_eq!(controller.chrome().prompt().text, "draft");
        assert_eq!(
            decision_rx.await.expect("approval decision"),
            PermissionDecision::Deny
        );
        let snapshot = controller.render_snapshot();
        assert!(
            snapshot.contains("Revision feedback: no thank"),
            "feedback should be surfaced after resolve: {snapshot}"
        );
    }

    #[tokio::test]
    async fn approval_cancel_rejects_pending_approval() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.apply_turn_event(AgentEvent::ApprovalRequested {
            turn: 1,
            id: "tool-1".to_owned(),
            operation: neo_agent_core::PermissionOperation::Tool,
            subject: "Write".to_owned(),
            arguments: serde_json::json!({"path": "denied.txt"}),
        });
        let (decision_tx, decision_rx) = oneshot::channel();
        controller
            .pending_approvals
            .insert("tool-1".to_owned(), decision_tx);

        controller
            .handle_input_event(InputEvent::Cancel)
            .await
            .expect("cancel rejects approval");

        assert_eq!(
            decision_rx.await.expect("approval decision"),
            PermissionDecision::Deny
        );
        assert!(controller.render_snapshot().contains("Rejected"));
    }

    #[tokio::test]
    async fn approval_requests_are_handled_one_at_a_time() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.apply_turn_event(AgentEvent::ApprovalRequested {
            turn: 1,
            id: "tool-1".to_owned(),
            operation: neo_agent_core::PermissionOperation::Shell,
            subject: "printf one".to_owned(),
            arguments: serde_json::json!({"command": "printf one"}),
        });
        controller.apply_turn_event(AgentEvent::ApprovalRequested {
            turn: 1,
            id: "tool-2".to_owned(),
            operation: neo_agent_core::PermissionOperation::Shell,
            subject: "printf two".to_owned(),
            arguments: serde_json::json!({"command": "printf two"}),
        });
        let (first_tx, first_rx) = oneshot::channel();
        let (second_tx, _second_rx) = oneshot::channel();
        controller
            .pending_approvals
            .insert("tool-1".to_owned(), first_tx);
        controller
            .pending_approvals
            .insert("tool-2".to_owned(), second_tx);

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("first approval confirms");

        assert_eq!(
            first_rx.await.expect("first decision"),
            PermissionDecision::Allow
        );
        assert_eq!(
            controller
                .chrome()
                .approval_selection()
                .map(|(id, _, _)| id),
            Some("tool-2")
        );
        let snapshot = controller.render_snapshot();
        assert!(snapshot.contains("printf two"));
    }

    #[tokio::test]
    async fn approval_transcript_only_shows_active_request() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.apply_turn_event(AgentEvent::ApprovalRequested {
            turn: 1,
            id: "tool-1".to_owned(),
            operation: neo_agent_core::PermissionOperation::Shell,
            subject: "printf one".to_owned(),
            arguments: serde_json::json!({"command": "printf one"}),
        });
        controller.apply_turn_event(AgentEvent::ApprovalRequested {
            turn: 1,
            id: "tool-2".to_owned(),
            operation: neo_agent_core::PermissionOperation::Shell,
            subject: "printf two".to_owned(),
            arguments: serde_json::json!({"command": "printf two"}),
        });

        let snapshot = controller.render_snapshot();
        assert!(snapshot.contains("printf one"));
        assert!(!snapshot.contains("printf two"));
        assert!(snapshot.contains("queued: 1 approval waiting"));
    }

    #[tokio::test]
    async fn approval_cancel_advances_next_visible_request() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.apply_turn_event(AgentEvent::ApprovalRequested {
            turn: 1,
            id: "tool-1".to_owned(),
            operation: neo_agent_core::PermissionOperation::Shell,
            subject: "printf one".to_owned(),
            arguments: serde_json::json!({"command": "printf one"}),
        });
        controller.apply_turn_event(AgentEvent::ApprovalRequested {
            turn: 1,
            id: "tool-2".to_owned(),
            operation: neo_agent_core::PermissionOperation::Shell,
            subject: "printf two".to_owned(),
            arguments: serde_json::json!({"command": "printf two"}),
        });
        let (first_tx, first_rx) = oneshot::channel();
        let (second_tx, _second_rx) = oneshot::channel();
        controller
            .pending_approvals
            .insert("tool-1".to_owned(), first_tx);
        controller
            .pending_approvals
            .insert("tool-2".to_owned(), second_tx);

        controller
            .handle_input_event(InputEvent::Cancel)
            .await
            .expect("cancel rejects current approval");

        assert_eq!(
            first_rx.await.expect("first decision"),
            PermissionDecision::Deny
        );
        let snapshot = controller.render_snapshot();
        assert!(snapshot.contains("Rejected"));
        assert!(snapshot.contains("printf two"));
        assert!(!snapshot.contains("queued:"));
    }

    #[tokio::test]
    async fn approval_interrupt_rejects_all_pending_approvals() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.apply_turn_event(AgentEvent::ApprovalRequested {
            turn: 1,
            id: "tool-1".to_owned(),
            operation: neo_agent_core::PermissionOperation::Shell,
            subject: "printf one".to_owned(),
            arguments: serde_json::json!({"command": "printf one"}),
        });
        controller.apply_turn_event(AgentEvent::ApprovalRequested {
            turn: 1,
            id: "tool-2".to_owned(),
            operation: neo_agent_core::PermissionOperation::Shell,
            subject: "printf two".to_owned(),
            arguments: serde_json::json!({"command": "printf two"}),
        });
        let (first_tx, first_rx) = oneshot::channel();
        let (second_tx, second_rx) = oneshot::channel();
        controller
            .pending_approvals
            .insert("tool-1".to_owned(), first_tx);
        controller
            .pending_approvals
            .insert("tool-2".to_owned(), second_tx);

        controller
            .handle_input_event(InputEvent::Interrupt)
            .await
            .expect("interrupt rejects pending approvals");

        assert_eq!(
            first_rx.await.expect("first decision"),
            PermissionDecision::Deny
        );
        assert_eq!(
            second_rx.await.expect("second decision"),
            PermissionDecision::Deny
        );
        assert!(controller.pending_approvals.is_empty());
        assert!(!controller.chrome().approval_is_pending());
    }

    #[tokio::test]
    async fn approval_interrupt_preserves_rejection_for_late_channel_registration() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.apply_turn_event(AgentEvent::ApprovalRequested {
            turn: 1,
            id: "tool-1".to_owned(),
            operation: neo_agent_core::PermissionOperation::Shell,
            subject: "printf one".to_owned(),
            arguments: serde_json::json!({"command": "printf one"}),
        });

        controller
            .handle_input_event(InputEvent::Interrupt)
            .await
            .expect("interrupt rejects visible approval");
        let (decision_tx, decision_rx) = oneshot::channel();
        controller.register_pending_approval(crate::modes::run::PromptApprovalRequest {
            id: "tool-1".to_owned(),
            decision_tx,
        });

        assert_eq!(
            decision_rx.await.expect("late approval decision"),
            PermissionDecision::Deny
        );
        assert!(controller.pending_approvals.is_empty());
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
                Ok(self
                    .events
                    .pop_front()
                    .unwrap_or(Some(InputEvent::Interrupt)))
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
                Ok(TurnOutcome::default())
            })
        });
        let mut controller = InteractiveController::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
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
                Ok(self
                    .events
                    .pop_front()
                    .unwrap_or(Some(InputEvent::Interrupt)))
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
                Ok(TurnOutcome::default())
            })
        });
        let mut controller = InteractiveController::new(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
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
        assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
        assert!(controller.active_turn.is_none());
    }

    #[test]
    fn rebuild_transcript_from_session_replays_tool_calls_and_results() {
        let mut transcript = TranscriptPane::new(80, 12);
        let loaded = LoadedSessionTranscript::new(
            "alpha",
            ["branch summary: inspected project".to_owned()],
            [
                AgentMessage::user_text("inspect"),
                AgentMessage::assistant(
                    [Content::text("reading")],
                    [neo_agent_core::AgentToolCall {
                        id: "tool-1".to_owned(),
                        name: "Read".to_owned(),
                        arguments: serde_json::json!({ "path": "README.md" }),
                    }],
                    StopReason::ToolUse,
                ),
                AgentMessage::tool_result(
                    "tool-1",
                    "Read",
                    [Content::text("README contents")],
                    false,
                ),
            ],
        );

        replay_session_into_transcript(&mut transcript, &loaded);
        let rendered = transcript
            .render_frame(80, 12)
            .expect("render frame")
            .into_iter()
            .map(|line| neo_tui::ansi::strip_ansi(&line))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("branch summary: inspected project"));
        assert!(rendered.contains("inspect"));
        assert!(rendered.contains("reading"));
        assert!(rendered.contains("Used Read (README.md)"));
        assert!(rendered.contains("README contents"));
        assert!(!rendered.contains("Using Read"));
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn event_loop_opens_session_picker_and_continues_selected_transcript() {
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_requests = std::sync::Arc::clone(&requests);
        let mut controller = InteractiveController::new_with_event_driver(
            "neo",
            "new",
            "openai/gpt-4.1",
            test_workspace_root(),
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
                session_items: vec![test_session_summary(
                    SESSION_A,
                    "Alpha session",
                    test_workspace_root(),
                    "branch summary",
                )],
                session_error: None,
                model_items: Vec::new(),
            },
            |session_id| async move {
                assert_eq!(session_id, SESSION_A);
                Ok(LoadedSessionTranscript::new(
                    SESSION_A,
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
                .chrome()
                .focused_overlay()
                .map(|overlay| &overlay.kind),
            Some(OverlayKind::SessionPicker(_))
        ));

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("session loads");

        assert_eq!(controller.chrome().session_label(), SESSION_A);
        assert!(controller.chrome().focused_overlay().is_none());
        assert!(transcript_has_status(
            &controller,
            "branch summary: Local branch summary"
        ));
        assert!(transcript_entries(&controller).iter().any(|entry| {
            matches!(entry, TranscriptEntry::UserMessage(content) if content == "hello")
        }));
        assert!(transcript_entries(&controller).iter().any(|entry| {
            matches!(entry, TranscriptEntry::AssistantMessage { content } if content == "hi back")
        }));

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
        assert_eq!(requests[0].session_id.as_deref(), Some(SESSION_A));
        assert_eq!(requests[0].model, None);
        assert!(transcript_entries(&controller).iter().any(|entry| {
            matches!(entry, TranscriptEntry::AssistantMessage { content } if content == "continued")
        }));
    }

    #[tokio::test]
    async fn event_loop_keeps_new_session_active_for_followup_prompt() {
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_requests = std::sync::Arc::clone(&requests);
        let run_turn: TurnDriver = Arc::new(move |request, channels| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            Box::pin(async move {
                captured_requests
                    .lock()
                    .expect("record request")
                    .push(request);
                channels.send_event(AgentEvent::MessageStarted {
                    turn: 1,
                    id: "assistant-1".to_owned(),
                });
                channels.send_event(AgentEvent::TextDelta {
                    turn: 1,
                    text: "ok".to_owned(),
                });
                channels.send_event(AgentEvent::MessageFinished {
                    turn: 1,
                    id: "assistant-1".to_owned(),
                    stop_reason: StopReason::EndTurn,
                });
                channels.send_event(AgentEvent::TurnFinished {
                    turn: 1,
                    stop_reason: StopReason::EndTurn,
                });
                Ok(TurnOutcome::session(SESSION_NEW))
            })
        });
        let mut controller = InteractiveController::new(
            "neo",
            "new",
            "openai/gpt-4.1",
            test_workspace_root(),
            run_turn,
            PickerCatalogs::default(),
            Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        );

        controller.type_text("read project");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("first prompt submits");
        controller
            .wait_for_active_turn()
            .await
            .expect("first turn completes");

        controller.type_text("continue");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("followup prompt submits");
        controller
            .wait_for_active_turn()
            .await
            .expect("followup turn completes");

        let requests = requests.lock().expect("recorded requests");
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].prompt, vec!["read project".to_owned()]);
        assert_eq!(requests[0].session_id, None);
        assert_eq!(requests[1].prompt, vec!["continue".to_owned()]);
        assert_eq!(requests[1].session_id.as_deref(), Some(SESSION_NEW));
        assert_eq!(controller.chrome().session_label(), SESSION_NEW);
    }

    #[tokio::test]
    async fn event_loop_keeps_started_session_active_after_failed_turn() {
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_requests = std::sync::Arc::clone(&requests);
        let run_turn: TurnDriver = Arc::new(move |request, channels| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            Box::pin(async move {
                let request_index = {
                    let mut requests = captured_requests.lock().expect("record request");
                    requests.push(request);
                    requests.len()
                };
                if request_index == 1 {
                    channels
                        .session_ids
                        .send(SESSION_NEW.to_owned())
                        .expect("session id sent");
                    channels.send_event(AgentEvent::TextDelta {
                        turn: 1,
                        text: "started".to_owned(),
                    });
                    anyhow::bail!("provider stream error after tool execution");
                }
                channels.send_event(AgentEvent::MessageStarted {
                    turn: 2,
                    id: "assistant-2".to_owned(),
                });
                channels.send_event(AgentEvent::TextDelta {
                    turn: 2,
                    text: "continued".to_owned(),
                });
                channels.send_event(AgentEvent::MessageFinished {
                    turn: 2,
                    id: "assistant-2".to_owned(),
                    stop_reason: StopReason::EndTurn,
                });
                channels.send_event(AgentEvent::TurnFinished {
                    turn: 2,
                    stop_reason: StopReason::EndTurn,
                });
                Ok(TurnOutcome::session(SESSION_NEW))
            })
        });
        let mut controller = InteractiveController::new(
            "neo",
            "new",
            "openai/gpt-4.1",
            test_workspace_root(),
            run_turn,
            PickerCatalogs::default(),
            Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        );

        controller.type_text("read project");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("first prompt submits");
        controller
            .wait_for_active_turn()
            .await
            .expect("failed first turn is drained");

        assert_eq!(controller.chrome().session_label(), SESSION_NEW);
        assert!(
            controller
                .render_snapshot()
                .contains("provider stream error")
        );

        controller.type_text("continue");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("followup prompt submits");
        controller
            .wait_for_active_turn()
            .await
            .expect("followup turn completes");

        let requests = requests.lock().expect("recorded requests");
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].prompt, vec!["read project".to_owned()]);
        assert_eq!(requests[0].session_id, None);
        assert_eq!(requests[1].prompt, vec!["continue".to_owned()]);
        assert_eq!(requests[1].session_id.as_deref(), Some(SESSION_NEW));
    }

    #[tokio::test]
    async fn event_loop_forks_selected_session_and_continues_child_session() {
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_requests = std::sync::Arc::clone(&requests);
        let mut controller = InteractiveController::new_with_event_driver_and_forker(
            "neo",
            "new",
            "openai/gpt-4.1",
            test_workspace_root(),
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
                session_items: vec![test_session_summary(
                    SESSION_A,
                    "Alpha session",
                    test_workspace_root(),
                    "branch summary",
                )],
                session_error: None,
                model_items: Vec::new(),
            },
            |_session_id| async move {
                panic!("fork action should not use the plain session loader");
                #[allow(unreachable_code)]
                Ok(LoadedSessionTranscript::new("", Vec::new(), Vec::new()))
            },
            |parent_id| async move {
                assert_eq!(parent_id, SESSION_A);
                Ok(ForkedSessionTranscript::new(
                    SESSION_CHILD,
                    LoadedSessionTranscript::new(
                        SESSION_CHILD,
                        [format!("forked from {SESSION_A}")],
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

        assert_eq!(controller.chrome().session_label(), SESSION_CHILD);
        assert!(controller.chrome().focused_overlay().is_none());
        assert!(transcript_has_status(
            &controller,
            &format!("forked from {SESSION_A}")
        ));
        assert!(transcript_entries(&controller).iter().any(|entry| {
            matches!(entry, TranscriptEntry::UserMessage(content) if content == "hello")
        }));

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
        assert_eq!(requests[0].session_id.as_deref(), Some(SESSION_CHILD));
        assert_eq!(requests[0].model, None);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn event_loop_opens_model_picker_and_submits_with_selected_model() {
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_requests = std::sync::Arc::clone(&requests);
        let mut controller = InteractiveController::new_with_event_driver(
            "neo",
            "new",
            "anthropic/claude-sonnet-4-5",
            test_workspace_root(),
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
                        Some("Messages · ctx 200000"),
                    ),
                ],
            },
            |session_id| async move {
                Ok(LoadedSessionTranscript::new(
                    session_id,
                    Vec::new(),
                    Vec::new(),
                ))
            },
        );

        controller.local_config = Some(test_config_with_models(
            &test_workspace_root(),
            test_workspace_root().join(".neo/sessions"),
            BTreeMap::from([
                (
                    "openai/gpt-4.1".to_owned(),
                    ModelConfig {
                        provider: "openai".to_owned(),
                        model: "gpt-4.1".to_owned(),
                        display_name: Some("Responses".to_owned()),
                        ..ModelConfig::default()
                    },
                ),
                (
                    "anthropic/claude-sonnet-4-5".to_owned(),
                    ModelConfig {
                        provider: "anthropic".to_owned(),
                        model: "claude-sonnet-4-5".to_owned(),
                        display_name: Some("Messages · ctx 200000".to_owned()),
                        max_context_tokens: Some(200_000),
                        ..ModelConfig::default()
                    },
                ),
            ]),
        ));
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::ModelPickerOpen))
            .await
            .expect("model picker opens");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("model selection applies");

        assert_eq!(
            controller.chrome().model_label(),
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
        assert_eq!(selected.max_context_tokens, Some(200_000));
        assert_eq!(requests[0].session_id, None);
    }

    #[test]
    fn model_picker_catalog_for_config_applies_cli_models_scope() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
        config.model_scope = vec!["sonnet".to_owned()];

        let catalog = model_picker_catalog_for_config(&config);

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

    #[test]
    fn controller_for_config_exposes_default_model_context_window() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = test_config(temp.path(), temp.path().join(".neo/sessions"));

        let controller = controller_for_config(&config);

        assert_eq!(
            controller.chrome().context_window(),
            Some(ContextWindow::new(1_047_576))
        );
    }

    #[test]
    fn controller_for_config_loads_builtin_skills() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = test_config(temp.path(), temp.path().join(".neo/sessions"));

        let controller = controller_for_config(&config);

        let skill_store = controller
            .skill_store
            .as_ref()
            .expect("skill store should load");
        assert!(
            skill_store.get("define-goal").is_some(),
            "builtin define-goal skill should be loaded"
        );
        assert!(
            skill_store.get("sub-skill").is_some(),
            "builtin sub-skill skill should be loaded"
        );
        assert!(
            skill_store.get("self-evo").is_some(),
            "builtin self-evo skill should be loaded"
        );
    }

    #[test]
    fn model_picker_items_include_parseable_context_window() {
        let item = model_to_picker_item(&neo_ai::ModelSpec {
            provider: neo_ai::ProviderId("test".to_owned()),
            model: "huge".to_owned(),
            api: neo_ai::ApiKind::OpenAiResponses,
            capabilities: neo_ai::ModelCapabilities::tool_chat().with_max_context_tokens(128_000),
        });

        assert!(
            item.description
                .as_deref()
                .is_some_and(|text| text.contains("ctx 128000"))
        );
        assert_eq!(context_window_from_picker_item(&item), Some(128_000));
    }

    #[tokio::test]
    async fn session_catalog_and_loader_use_real_local_session_store() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = temp.path().join(".neo/sessions");
        // Compute the workspace-scoped bucket directory that the code will use.
        let bucket_dir = workspace_sessions_dir(&test_config(temp.path(), sessions_dir.clone()));
        fs::create_dir_all(&bucket_dir).expect("create sessions bucket dir");
        fs::write(
            bucket_dir.join(format!("{SESSION_A}.jsonl")),
            concat!(
                "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello\"}}]}}}}\n",
                "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"hi back\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
            ),
        )
        .expect("write session jsonl");

        let store = SessionMetadataStore::new(&bucket_dir);
        store
            .rename(SESSION_A, "Alpha Session".to_owned())
            .expect("rename session");
        store
            .summarize(SESSION_A, "Local branch summary".to_owned())
            .expect("summarize session");
        let child = store
            .fork(SESSION_A, Some("Parser branch".to_owned()))
            .expect("fork session");
        store
            .record_activity(
                SESSION_A,
                Some(temp.path().display().to_string()),
                Some("hello".to_owned()),
                "100".to_owned(),
            )
            .expect("record session activity");
        store
            .record_activity(
                &child.id,
                Some(temp.path().display().to_string()),
                Some("child prompt".to_owned()),
                "200".to_owned(),
            )
            .expect("record child activity");

        let config = test_config(temp.path(), sessions_dir);
        let catalog = session_catalog_for_config(&config);
        assert_eq!(catalog.error, None);
        assert_eq!(catalog.items.len(), 2);
        assert_eq!(catalog.items[0].id, child.id);
        assert_eq!(catalog.items[0].title.as_deref(), Some("Parser branch"));
        assert!(
            catalog.items[0]
                .last_prompt
                .as_deref()
                .is_some_and(|prompt| prompt.contains("child prompt"))
        );
        assert_eq!(catalog.items[1].id, SESSION_A);
        assert_eq!(catalog.items[1].title.as_deref(), Some("Alpha Session"));
        assert!(
            catalog.items[1]
                .last_prompt
                .as_deref()
                .is_some_and(|prompt| prompt.contains("hello"))
        );

        let loaded = load_session_transcript(SESSION_A.to_owned(), &config)
            .await
            .expect("load session transcript");
        assert_eq!(loaded.label, SESSION_A);
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
        let config = test_config(temp.path(), sessions_dir.clone());
        let bucket_dir = workspace_sessions_dir(&config);
        fs::create_dir_all(&bucket_dir).expect("create sessions bucket dir");
        fs::write(
            bucket_dir.join(format!("{SESSION_A}.jsonl")),
            concat!(
                "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello\"}}]}}}}\n",
                "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"hi back\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
            ),
        )
        .expect("write session jsonl");

        let forked = fork_session_transcript(SESSION_A.to_owned(), &config)
            .await
            .expect("fork session");

        assert!(forked.session_id.starts_with("session_"));
        assert_eq!(forked.transcript.label, forked.session_id);
        assert_eq!(
            forked.transcript.notices.first().map(String::as_str),
            Some(format!("forked from {SESSION_A}").as_str())
        );
        assert_eq!(forked.transcript.messages.len(), 2);
        assert!(
            bucket_dir
                .join(format!("{}.jsonl", forked.session_id))
                .is_file()
        );

        let sessions = SessionMetadataStore::new(&bucket_dir)
            .list()
            .expect("list sessions");
        let parent = sessions
            .iter()
            .find(|session| session.id == SESSION_A)
            .expect("parent listed");
        assert!(parent.children.contains(&forked.session_id));
        let child = sessions
            .iter()
            .find(|session| session.id == forked.session_id)
            .expect("child listed");
        assert_eq!(child.parent_id.as_deref(), Some(SESSION_A));
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn session_picker_ctrl_a_toggles_scope() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = temp.path().join(".neo/sessions");
        fs::create_dir_all(&sessions_dir).expect("create sessions dir");
        let neo_home = sessions_dir.parent().expect("neo home");

        let project_a = temp.path().join("project_a");
        fs::create_dir_all(&project_a).expect("create project_a");
        let config_a = test_config(&project_a, sessions_dir.clone());
        let bucket_a = workspace_sessions_dir(&config_a);
        fs::create_dir_all(&bucket_a).expect("create bucket_a");
        fs::write(
            bucket_a.join(format!("{SESSION_A}.jsonl")),
            r#"{"MessageAppended":{"message":{"User":{"content":[{"Text":{"text":"hello"}}]}}}}"#,
        )
        .expect("write alpha jsonl");
        let store_a = SessionMetadataStore::new(&bucket_a);
        store_a
            .record_activity(
                SESSION_A,
                Some(project_a.display().to_string()),
                Some("alpha prompt".to_owned()),
                "200".to_owned(),
            )
            .expect("record alpha");

        let project_b = temp.path().join("project_b");
        fs::create_dir_all(&project_b).expect("create project_b");
        let config_b = test_config(&project_b, sessions_dir.clone());
        let bucket_b = workspace_sessions_dir(&config_b);
        fs::create_dir_all(&bucket_b).expect("create bucket_b");
        fs::write(
            bucket_b.join(format!("{SESSION_B}.jsonl")),
            r#"{"MessageAppended":{"message":{"User":{"content":[{"Text":{"text":"hello"}}]}}}}"#,
        )
        .expect("write beta jsonl");
        let store_b = SessionMetadataStore::new(&bucket_b);
        store_b
            .record_activity(
                SESSION_B,
                Some(project_b.display().to_string()),
                Some("beta prompt".to_owned()),
                "100".to_owned(),
            )
            .expect("record beta");

        let index = neo_agent_core::session::SessionIndex::new(neo_home);
        index
            .append(&neo_agent_core::session::SessionIndexEntry {
                session_id: SESSION_A.to_owned(),
                session_dir: bucket_a.clone(),
                workdir: project_a.clone(),
            })
            .expect("index alpha");
        index
            .append(&neo_agent_core::session::SessionIndexEntry {
                session_id: SESSION_B.to_owned(),
                session_dir: bucket_b.clone(),
                workdir: project_b.clone(),
            })
            .expect("index beta");

        let mut controller = controller_for_config(&config_a);

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SessionPickerOpen))
            .await
            .expect("session picker opens");
        let overlay = controller.chrome().focused_overlay().expect("picker open");
        assert!(
            matches!(
                &overlay.kind,
                OverlayKind::SessionPicker(p) if p.scope() == SessionPickerScope::Workspace
            ),
            "workspace scope on open"
        );
        let snapshot = controller.render_snapshot();
        assert!(
            snapshot.to_lowercase().contains("alpha"),
            "workspace scope should show alpha: {snapshot}"
        );
        assert!(
            !snapshot.to_lowercase().contains("beta"),
            "workspace scope should not show beta: {snapshot}"
        );

        controller
            .handle_input_event(InputEvent::Action(
                KeybindingAction::SessionPickerToggleScope,
            ))
            .await
            .expect("scope toggles");
        let overlay = controller
            .chrome()
            .focused_overlay()
            .expect("picker still open");
        assert!(
            matches!(
                &overlay.kind,
                OverlayKind::SessionPicker(p) if p.scope() == SessionPickerScope::All
            ),
            "all scope after toggle"
        );
        let snapshot = controller.render_snapshot();
        assert!(
            snapshot.to_lowercase().contains("alpha"),
            "all scope should show alpha: {snapshot}"
        );
        assert!(
            snapshot.to_lowercase().contains("beta"),
            "all scope should show beta: {snapshot}"
        );
    }

    #[tokio::test]
    async fn session_picker_cross_cwd_shows_resume_command() {
        let other_dir = tempfile::tempdir().expect("tempdir");
        let mut controller = InteractiveController::new_with_event_driver(
            "neo",
            "new",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
            PickerCatalogs {
                session_items: vec![SessionSummary {
                    id: SESSION_A.to_owned(),
                    title: Some("Alpha session".to_owned()),
                    last_prompt: Some("hello".to_owned()),
                    work_dir: other_dir.path().to_path_buf(),
                    updated_at: String::new(),
                    metadata: None,
                }],
                session_error: None,
                model_items: Vec::new(),
            },
            |_session_id| async move {
                panic!("load_session should not be called for a cross-cwd session");
                #[allow(unreachable_code)]
                Ok(LoadedSessionTranscript::new("", Vec::new(), Vec::new()))
            },
        );
        controller.set_clipboard_writer(Arc::new(|_text| Ok(())));

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SessionPickerOpen))
            .await
            .expect("session picker opens");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("select cross-cwd session");

        let expected = format!(
            "cd '{}' && neo --resume '{SESSION_A}'",
            other_dir.path().display(),
        );
        assert!(controller.chrome().focused_overlay().is_none());
        assert!(transcript_has_status(&controller, &expected));
    }

    #[test]
    fn composed_frame_lines_do_not_exceed_content_width() {
        let app = NeoChromeState::new("neo", "s", "openai/gpt-4.1", "/tmp");
        let mut transcript = TranscriptPane::new(80, 12);
        transcript.push_welcome_banner("neo", "s", "m", "~Workspace/neo", "0.1.0", None);
        let lines = compose_tui_frame(&app, &mut transcript, 80, 12).expect("frame composes");
        let expected = 80usize;
        for (i, line) in lines.iter().enumerate() {
            let w = neo_tui::ansi::visible_width(line);
            assert!(
                w < expected,
                "line {i} reaches terminal autowrap column {expected}: {w}: {line:?}"
            );
        }
    }

    fn test_config(project_dir: &Path, sessions_dir: PathBuf) -> AppConfig {
        AppConfig {
            default_model: "gpt-4.1".to_owned(),
            default_provider: "openai".to_owned(),
            api_key_env: None,
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
            model_scope: Vec::new(),
            sessions_dir,
            permissions: PermissionPolicy::default(),
            defaults: Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: RuntimeConfig::default(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_dir: project_dir.to_path_buf(),
            config_path: project_dir.join(".neo/config.toml"),
        }
    }

    fn test_config_with_models(
        project_dir: &Path,
        sessions_dir: PathBuf,
        models: BTreeMap<String, ModelConfig>,
    ) -> AppConfig {
        let mut config = test_config(project_dir, sessions_dir);
        config.models = models;
        config
    }
}
