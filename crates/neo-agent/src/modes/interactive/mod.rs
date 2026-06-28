use crate::{
    config::{self, AppConfig, neo_home, workspace_sessions_dir},
    mcp_ops,
    modes::sessions::SessionPickerScope as SessionDataScope,
    modes::task_browser,
    prompt::templates::expand_prompt_template_args,
    resources,
    trust::{self, ProjectTrustState},
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    env,
    fmt::Write as _,
    future::Future,
    io::{IsTerminal as _, stdout},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

#[cfg(test)]
use std::future::{Ready, ready};

use anyhow::{Context, Result};
use crossterm::terminal::size;
use neo_agent_core::{
    AgentEvent, AgentMessage, Content, McpConnectionManager, PendingQuestion,
    PermissionApprovalDecision, PermissionMode, ProcessSupervisor, QuestionResponse,
    ShellCommandOrigin, ShellCommandOutcome,
    mode::PlanMode,
    oauth::OAuthStore,
    session::{JsonlSessionReader, SessionMetadataStore, SessionSummary},
};
use neo_tui::tasks_browser::TaskBrowserAction;
use neo_tui::{
    input::{InputEvent, InputParser, KeyId, KeybindingAction, KeybindingsManager},
    primitive::InputResult,
    screen_output::TuiRenderer,
    shell::{
        ApprovalChoice, ApprovalResult, ContextWindow,
        NeoChromeState, OverlayKind, PickerItem, PromptEdit, SessionPickerItem,
        SessionPickerScope, StreamUpdate,
    },
    transcript::{TranscriptPane, frame_content_width},
};

#[cfg(test)]
use neo_tui::shell::{DevelopmentMode, GoalModeStatus};

#[cfg(test)]
use neo_tui::terminal_image::{ImageProtocolPreference, TerminalImageCapabilities};

use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

mod keybinding_priority;
use keybinding_priority::{
    EDITING_ACTION_PRIORITY, OVERLAY_ACTION_PRIORITY, PROMPT_COMPLETION_ACTION_PRIORITY,
    QUESTION_ACTION_PRIORITY,
};

mod log_events;

mod prompt_history;

mod git_status;
use git_status::{event_should_refresh_git_status, git_status_label};

mod clipboard;
use clipboard::write_system_clipboard;

mod snapshot;
use snapshot::render_transcript_snapshot;

mod prompt_completion;
use prompt_completion::{longest_common_completion_prefix, prompt_completions};

#[cfg(test)]
use prompt_completion::{
    CompletionCatalog, CompletionCandidate, CompletionSource, completion_source_candidates,
    session_completion_items,
};

mod mode_state;

mod approval;
use approval::PendingApprovalResponse;

mod slash_commands;

mod command_palette;

mod btw_sidecar;

mod sessions;

mod mcp_manager;
use mcp_manager::PendingMcpProbe;

mod catalog_fetch;
use catalog_fetch::{PendingCatalogFetch, PendingCustomRegistry};

mod questions;

mod startup;
pub use startup::{InteractiveOptions, StartupAction};

mod image_capabilities;
use image_capabilities::terminal_image_capabilities_for_policy;

mod turn;

mod model_picker;
use model_picker::{context_window_from_picker_item, picker_catalogs_for_config};

#[cfg(test)]
use model_picker::{
    model_picker_catalog_for_config, model_picker_items_from_config, model_to_picker_item,
    session_catalog_for_config,
};

type BoxedTurnFuture = Pin<Box<dyn Future<Output = Result<TurnOutcome>> + Send + 'static>>;
type BoxedSessionFuture = Pin<Box<dyn Future<Output = Result<LoadedSessionTranscript>> + Send>>;
type BoxedForkFuture = Pin<Box<dyn Future<Output = Result<ForkedSessionTranscript>> + Send>>;
type TurnDriver = Arc<dyn Fn(TurnRequest, TurnChannels) -> BoxedTurnFuture + Send + Sync>;
type SessionLoader = Arc<dyn Fn(String) -> BoxedSessionFuture + Send + Sync>;
type SessionForker = Arc<dyn Fn(String) -> BoxedForkFuture + Send + Sync>;
type BoxedShellFuture = Pin<
    Box<
        dyn Future<Output = Result<neo_agent_core::tools::ShellExecutionResult, ShellDriverError>>
            + Send
            + 'static,
    >,
>;
type ShellDriver = Arc<dyn Fn(ShellRunRequest) -> BoxedShellFuture + Send + Sync>;
type ClipboardWriter = Arc<dyn Fn(&str) -> Result<()> + Send + Sync>;
type GitStatusProvider = Arc<dyn Fn(&Path) -> Option<String> + Send + Sync>;

const GIT_STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const TASK_BROWSER_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const SHELL_FOREGROUND_TIMEOUT: Duration = Duration::from_secs(120);
const SHELL_BACKGROUND_TIMEOUT: Duration = Duration::from_secs(600);
const SHELL_MAX_OUTPUT_BYTES: usize = 200_000;

fn mcp_manager_with_oauth_store() -> McpConnectionManager {
    let supervisor = ProcessSupervisor::default();
    if let Some(home) = neo_home() {
        let path = home.join("oauth.json");
        match OAuthStore::load(&path) {
            Ok(store) => {
                return McpConnectionManager::with_oauth_store(
                    supervisor,
                    Arc::new(tokio::sync::RwLock::new(store)),
                    Some(path),
                );
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    "failed to load OAuth store; continuing with empty store"
                );
            }
        }
    }
    McpConnectionManager::new(supervisor)
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


fn slash_arg<'a>(prompt: &'a str, command: &str) -> Option<&'a str> {
    let rest = prompt.strip_prefix(command)?;
    if rest.is_empty() {
        return Some("");
    }
    rest.chars()
        .next()
        .filter(|character| character.is_whitespace())
        .map(|_| rest.trim())
}

fn slash_permission_mode(prompt: &str) -> Option<PermissionMode> {
    match prompt {
        "/ask" => Some(PermissionMode::Ask),
        "/auto" => Some(PermissionMode::Auto),
        "/yolo" => Some(PermissionMode::Yolo),
        _ => None,
    }
}

/// Direct permission-mode slashes (`/ask`, `/auto`, `/yolo`) are always
/// dispatchable, even while a turn is running, because they only update live
/// permission state and never submit a turn or open a focused overlay.
fn is_live_permission_slash(prompt: &str) -> bool {
    slash_permission_mode(prompt.trim()).is_some()
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

fn trust_dialog_data_for_startup(config: &AppConfig) -> Option<neo_tui::dialogs::TrustDialogData> {
    match &config.project_trust {
        ProjectTrustState::Unknown { inputs } => {
            Some(trust::trust_dialog_data_from_inputs(inputs.clone()))
        }
        _ => None,
    }
}

pub async fn execute_tty_with_startup(
    config: &AppConfig,
    startup: StartupAction,
    options: InteractiveOptions,
    log_receiver: Option<tokio::sync::mpsc::UnboundedReceiver<crate::log_capture::CapturedEvent>>,
) -> Result<Option<String>> {
    if !stdout().is_terminal() {
        return Ok(Some(execute_with_startup(config, &startup, options)));
    }

    let mut controller = controller_for_config(config);
    controller.apply_startup_options(config, options);
    if let Some(rx) = log_receiver {
        controller.set_log_event_receiver(rx);
    }

    let terminal = RefCell::new(NeoTerminal::enter()?);

    if let Some(data) = trust_dialog_data_for_startup(config) {
        controller
            .resolve_trust_dialog_at_startup(
                data,
                RawStdinEvents::new(controller.keybindings.clone()),
                |tui| terminal.borrow_mut().draw_tui(tui),
            )
            .await?;
    }
    if let StartupAction::LoadSession(session_id) = &startup {
        if let Err(error) = controller.load_session_at_startup(session_id).await {
            controller.push_status(format!("Failed to resume session: {error}"));
        }
    } else {
        controller.apply_startup_action(&startup);
    }
    let events = RawStdinEvents::new(controller.keybindings.clone());
    controller
        .run_terminal_loop_with_suspend(
            |tui| terminal.borrow_mut().draw_tui(tui),
            || terminal.borrow_mut().suspend(),
            events,
        )
        .await?;
    Ok(Some(exit_message(controller.active_session_id())))
}

fn exit_message(session_id: Option<&str>) -> String {
    let mut message = String::from("Bye\n");
    if let Some(session_id) = session_id {
        let _ = writeln!(message, "neo resume {session_id}");
    }
    message
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
    session_messages: Vec<AgentMessage>,
    current_thinking: bool,
    active_turn: Option<RunningTurn>,
    shell_driver: ShellDriver,
    active_shell_command: Option<RunningShellCommand>,
    next_shell_command_id: u64,
    btw_runner: Option<crate::modes::btw::BtwRunner>,
    btw_receiver: Option<tokio::sync::mpsc::UnboundedReceiver<crate::modes::btw::BtwEvent>>,
    #[cfg(test)]
    btw_client: Option<Arc<dyn neo_ai::ModelClient>>,
    pending_approvals: BTreeMap<String, PendingApprovalResponse>,
    resolved_approvals: BTreeMap<String, PermissionApprovalDecision>,
    /// Pending `AskUser` question response channels, keyed by question id.
    pending_questions: BTreeMap<String, oneshot::Sender<QuestionResponse>>,
    pending_question_prompts: BTreeMap<String, Vec<neo_agent_core::QuestionEventData>>,
    pending_background_question_followups: VecDeque<String>,
    clipboard_writer: ClipboardWriter,
    completion_root: PathBuf,
    workspace_root: PathBuf,
    git_status_provider: GitStatusProvider,
    last_git_status_refresh: Option<Instant>,
    git_status_refresh_interval: Duration,
    last_task_browser_refresh: Option<Instant>,
    pending_exit_confirmation: Option<ExitConfirmation>,
    suspend_requested: bool,
    pending_custom_registry: Option<PendingCustomRegistry>,
    pending_catalog_provider_id: Option<String>,
    pending_catalog_fetch: Option<PendingCatalogFetch>,
    pending_mcp_probe: Option<PendingMcpProbe>,
    /// Transport selected in the MCP add transport picker, kept while the
    /// single-page add form is open so submission can build the right input.
    pending_mcp_add_transport: Option<&'static str>,
    mcp_manager: Option<McpConnectionManager>,
    skill_store: Option<neo_agent_core::skills::SkillStore>,
    /// Expanded skill body waiting to be injected as context for the next turn.
    pending_skill_context: Option<String>,
    goal_manager: Option<Arc<neo_agent_core::goal::GoalManager>>,
    plan_mode: Arc<RwLock<PlanMode>>,
    /// Current permission mode for the session.
    permission_mode: PermissionMode,
    /// Shared live permission state. The turn driver clones an `Arc` to this
    /// into `TurnRequest`/`AppConfig`, and `set_permission_mode` writes here so
    /// a running turn picks up the new mode at its next tool call.
    live_permission_mode: Arc<RwLock<PermissionMode>>,
    /// Revise/feedback text keyed by approval request id, waiting to be sent to
    /// the runtime with the next turn.
    pending_plan_review_feedback: BTreeMap<String, String>,
    /// Workspace-scoped prompt history store. `None` for test controllers that
    /// do not exercise persistence. Real controllers seed `PromptState` from
    /// this on startup and append accepted prompts to it after each submit.
    prompt_history: Option<crate::prompt::history::PromptHistoryStore>,
    /// Optional trust store override for tests. Production controllers created
    /// via `controller_for_config` initialize this from `~/.neo/trust.json`.
    trust_store: Option<crate::trust::ProjectTrustStore>,
    /// Shared manual-compaction request. Set by `/compact`, passed to each turn's
    /// `AgentConfig` so the runtime can read it at the top of every loop
    /// iteration.
    manual_compact_request: Arc<std::sync::Mutex<Option<String>>>,
    /// Stored text pastes referenced by composer `[paste ...]` placeholders.
    paste_store: std::collections::HashMap<usize, String>,
    next_paste_id: usize,
    /// Stored image attachments referenced by composer `[image #N ...]` placeholders.
    image_attachment_store: neo_tui::paste::ImageAttachmentStore,
    /// Cached model capabilities for the active workspace/model scope.
    model_capabilities: std::collections::HashMap<String, neo_ai::ModelCapabilities>,
    /// Optional receiver for captured tracing WARN/ERROR events, surfaced as
    /// transcript status lines. `None` in tests that don't exercise this path.
    log_event_rx: Option<tokio::sync::mpsc::UnboundedReceiver<crate::log_capture::CapturedEvent>>,
}

pub(crate) struct TurnChannels {
    events: mpsc::UnboundedSender<Result<AgentEvent>>,
    approvals: mpsc::UnboundedSender<crate::modes::run::PromptApprovalRequest>,
    session_ids: mpsc::UnboundedSender<String>,
    cancel_token: CancellationToken,
    /// Channel sender for `AskUserTool`'s reverse-RPC questions.
    questions: mpsc::UnboundedSender<PendingQuestion>,
    /// Shared handle for pushing live steer/follow-up input into the running
    /// turn. The controller writes; the runtime drains at step boundaries.
    steer_input: neo_agent_core::SteerInputHandle,
}

struct ShellRunRequest {
    id: String,
    command: String,
    cwd: PathBuf,
    foreground_timeout: Duration,
    background_timeout: Duration,
    max_output_bytes: usize,
    cancel_token: CancellationToken,
    background_tasks: neo_agent_core::tools::BackgroundTaskManager,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
}

#[derive(Debug)]
enum ShellDriverError {
    Tool(neo_agent_core::ToolError),
    Other(anyhow::Error),
}

impl std::fmt::Display for ShellDriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tool(err) => write!(f, "{err}"),
            Self::Other(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ShellDriverError {}

impl From<neo_agent_core::ToolError> for ShellDriverError {
    fn from(value: neo_agent_core::ToolError) -> Self {
        Self::Tool(value)
    }
}

impl From<anyhow::Error> for ShellDriverError {
    fn from(value: anyhow::Error) -> Self {
        Self::Other(value)
    }
}

struct RunningShellCommand {
    id: String,
    command: String,
    task: JoinHandle<Result<neo_agent_core::tools::ShellExecutionResult, ShellDriverError>>,
    cancel_token: CancellationToken,
    background_tasks: neo_agent_core::tools::BackgroundTaskManager,
    foreground_task_id: Option<String>,
    events: mpsc::UnboundedReceiver<AgentEvent>,
}

#[cfg(test)]
impl TurnChannels {
    fn send_event(&self, event: AgentEvent) {
        let _ = self.events.send(Ok(event));
    }
}

pub(super) struct RunningTurn {
    pub(super) events: mpsc::UnboundedReceiver<Result<AgentEvent>>,
    pub(super) approvals: mpsc::UnboundedReceiver<crate::modes::run::PromptApprovalRequest>,
    pub(super) session_ids: mpsc::UnboundedReceiver<String>,
    pub(super) task: JoinHandle<Result<TurnOutcome>>,
    pub(super) cancel_token: CancellationToken,
    /// Receiver for `AskUserTool`'s reverse-RPC questions.
    pub(super) questions: mpsc::UnboundedReceiver<PendingQuestion>,
    /// Shared handle kept so the controller can push steer/follow-up input
    /// while the turn runs.
    pub(super) steer_input: neo_agent_core::SteerInputHandle,
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

#[derive(Clone)]
pub(crate) struct TurnRequest {
    pub prompt: Vec<Content>,
    pub session_id: Option<String>,
    pub model: Option<SelectedModel>,
    pub reasoning_effort: Option<neo_ai::ReasoningEffort>,
    /// Expanded skill body to inject as context before the user prompt.
    pub skill_context: Option<String>,
    /// Permission mode to use for this turn.
    pub permission_mode: PermissionMode,
    /// Shared live permission state for this turn. Updated by `/ask` `/auto`
    /// `/yolo` while the turn runs; the runtime reads it at each tool call.
    pub live_permission_mode: Arc<RwLock<PermissionMode>>,
    /// Shared runtime plan-mode state for this turn.
    pub plan_mode: Arc<RwLock<PlanMode>>,
    /// Whether this turn should use AI-assisted goal authoring.
    pub goal_mode_authoring: bool,
    /// Revise/feedback text keyed by approval request id, waiting to be sent to
    /// the runtime. The production driver is responsible for forwarding this to
    /// the runtime's plan-review side-channel when possible.
    pub plan_review_feedback: std::collections::BTreeMap<String, String>,
    pub mcp_manager: Option<McpConnectionManager>,
    /// Live config snapshot at dispatch time. Lets the turn driver pick up
    /// providers/models added at runtime (e.g. via `/provider`) instead of the
    /// stale snapshot captured when the controller was built. `None` for test
    /// drivers that don't depend on config.
    pub base_config: Option<crate::config::AppConfig>,
    /// Shared manual-compaction request. `Some(instruction)` means a manual
    /// compaction was requested with an optional custom instruction; `None`
    /// means no request is pending. Set by `/compact`, taken by the runtime.
    pub manual_compact_request: Arc<std::sync::Mutex<Option<String>>>,
    /// When true, the turn should only run compaction and then finish without
    /// sending anything to the model. Used by the idle `/compact` path.
    pub compaction_only: bool,
}

impl TurnRequest {
    #[must_use]
    pub(crate) fn new(
        prompt: Vec<Content>,
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
            permission_mode: PermissionMode::default(),
            live_permission_mode: Arc::new(RwLock::new(PermissionMode::default())),
            plan_mode: Arc::new(RwLock::new(PlanMode::default())),
            goal_mode_authoring: false,
            plan_review_feedback: BTreeMap::new(),
            mcp_manager: None,
            base_config: None,
            manual_compact_request: Arc::new(std::sync::Mutex::new(None)),
            compaction_only: false,
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
    pub alias: String,
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
            alias: item.value.clone(),
            provider: provider.to_owned(),
            model: model.to_owned(),
            max_context_tokens: context_window_from_picker_item(item),
        })
    }

    fn from_alias(
        alias: &str,
        config: Option<&AppConfig>,
        model_items: &[PickerItem],
    ) -> Result<Self> {
        if let Some(model_cfg) = config.and_then(|config| config.models.get(alias)) {
            return Ok(Self {
                alias: alias.to_owned(),
                provider: model_cfg.provider.clone(),
                model: model_cfg.model.clone(),
                max_context_tokens: model_cfg.max_context_tokens,
            });
        }
        if let Some(item) = model_items.iter().find(|item| item.value == alias) {
            return Self::from_picker_item(item);
        }
        let Some((provider, model)) = alias.split_once('/') else {
            anyhow::bail!("invalid model alias {alias}");
        };
        Ok(Self {
            alias: alias.to_owned(),
            provider: provider.to_owned(),
            model: model.to_owned(),
            max_context_tokens: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedSessionTranscript {
    label: String,
    notices: Vec<String>,
    messages: Vec<AgentMessage>,
    estimated_context_tokens: Option<u32>,
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
            estimated_context_tokens: None,
        }
    }

    #[must_use]
    pub(crate) const fn with_estimated_context_tokens(mut self, used_tokens: u32) -> Self {
        self.estimated_context_tokens = Some(used_tokens);
        self
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

/// Produce a short display text for a mixed content vector. Used for prompt
/// history and transcript summaries.
pub(super) fn content_to_display_text(content: &[Content]) -> String {
    let mut image_idx = 0;
    let mut out = String::new();
    for part in content {
        match part {
            Content::Text { text } => out.push_str(text),
            Content::Image { mime_type, data } => {
                image_idx += 1;
                let (w, h) = image_dimensions_from_data(mime_type, data);
                out.push_str(&format!("[image #{image_idx} ({w}x{h})]"));
            }
            Content::Thinking { .. } => {}
        }
    }
    out
}

/// Best-effort dimension extraction from image data for display purposes.
fn image_dimensions_from_data(mime_type: &str, data: &neo_agent_core::ImageRef) -> (u32, u32) {
    let bytes = match data {
        neo_agent_core::ImageRef::Base64(b64) => {
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).ok()
        }
        _ => None,
    };
    bytes
        .as_deref()
        .and_then(|b| crate::image_blob::detect_image_dimensions(b, mime_type))
        .unwrap_or((0, 0))
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
        let git_status_provider: GitStatusProvider = Arc::new(git_status_label);
        let mut chrome =
            NeoChromeState::new(title, session_label, model_label, workspace_root.clone());
        chrome.set_git_status_label(git_status_provider(&workspace_root));
        let shell_driver: ShellDriver = Arc::new(|request| {
            Box::pin(async move {
                let id = request.id.clone();
                let event_tx = request.event_tx.clone();
                let stream_update: neo_agent_core::tools::ToolUpdateCallback =
                    Arc::new(move |partial: &str| {
                        let _ = event_tx.send(AgentEvent::ToolExecutionUpdate {
                            turn: 0,
                            id: id.clone(),
                            name: "Bash".to_owned(),
                            partial_result: neo_agent_core::ToolResult {
                                content: partial.to_owned(),
                                is_error: false,
                                details: None,
                                terminate: false,
                            },
                        });
                    });
                neo_agent_core::tools::execute_shell_command(
                    neo_agent_core::tools::ShellExecutionRequest {
                        id: request.id,
                        command: request.command,
                        cwd: request.cwd,
                        origin: ShellCommandOrigin::UserShellMode,
                        foreground_timeout: request.foreground_timeout,
                        background_timeout: request.background_timeout,
                        max_output_bytes: request.max_output_bytes,
                        cancel_token: request.cancel_token,
                        stream_update: Some(stream_update),
                        background_tasks: Some(request.background_tasks),
                    },
                )
                .await
                .map_err(ShellDriverError::from)
            })
        });
        Self {
            tui: neo_tui::NeoTui::with_welcome_banner(chrome, 80, 24, env!("CARGO_PKG_VERSION")),
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
            session_messages: Vec::new(),
            current_thinking: false,
            active_turn: None,
            shell_driver,
            active_shell_command: None,
            next_shell_command_id: 1,
            btw_runner: None,
            btw_receiver: None,
            #[cfg(test)]
            btw_client: None,
            pending_approvals: BTreeMap::new(),
            resolved_approvals: BTreeMap::new(),
            pending_questions: BTreeMap::new(),
            pending_question_prompts: BTreeMap::new(),
            pending_background_question_followups: VecDeque::new(),
            clipboard_writer: Arc::new(write_system_clipboard),
            completion_root: workspace_root.clone(),
            workspace_root,
            git_status_provider,
            last_git_status_refresh: Some(Instant::now()),
            git_status_refresh_interval: GIT_STATUS_REFRESH_INTERVAL,
            last_task_browser_refresh: None,
            pending_exit_confirmation: None,
            suspend_requested: false,
            pending_custom_registry: None,
            pending_catalog_provider_id: None,
            pending_catalog_fetch: None,
            pending_mcp_probe: None,
            pending_mcp_add_transport: None,
            mcp_manager: Some(mcp_manager_with_oauth_store()),
            skill_store: None,
            pending_skill_context: None,
            goal_manager: None,
            plan_mode: Arc::new(RwLock::new(PlanMode::default())),
            permission_mode: PermissionMode::default(),
            live_permission_mode: Arc::new(RwLock::new(PermissionMode::default())),
            pending_plan_review_feedback: BTreeMap::new(),
            prompt_history: None,
            trust_store: None,
            manual_compact_request: Arc::new(std::sync::Mutex::new(None)),
            paste_store: std::collections::HashMap::new(),
            next_paste_id: 1,
            image_attachment_store: neo_tui::paste::ImageAttachmentStore::new(),
            model_capabilities: std::collections::HashMap::new(),
            log_event_rx: None,
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

    #[cfg(test)]
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
    fn set_git_status_provider(&mut self, provider: GitStatusProvider) {
        self.git_status_provider = provider;
    }

    #[cfg(test)]
    fn set_shell_driver(&mut self, driver: ShellDriver) {
        self.shell_driver = driver;
    }

    #[cfg(test)]
    fn set_last_git_status_refresh(&mut self, refreshed_at: Option<Instant>) {
        self.last_git_status_refresh = refreshed_at;
    }

    #[cfg(test)]
    fn set_trust_store(&mut self, store: crate::trust::ProjectTrustStore) {
        self.trust_store = Some(store);
    }

    #[cfg(test)]
    fn set_btw_client(&mut self, client: Arc<dyn neo_ai::ModelClient>) {
        self.btw_client = Some(client);
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
            auto: false,
            ..crate::config::ConfigOverrides::default()
        };
        match crate::config::AppConfig::load(overrides) {
            Ok(config) => {
                let catalogs = picker_catalogs_for_config(&config);
                self.session_items = catalogs.session_items;
                self.session_list_error = catalogs.session_error;
                self.model_items = catalogs.model_items;
                self.tui.chrome_mut().set_theme(config.theme.theme);
                self.local_config = Some(config);
                self.spawn_sync_mcp_manager();
            }
            Err(error) => {
                tracing::warn!("failed to reload config: {error}");
            }
        }
    }

    /// Push the current application config onto the MCP connection manager.
    ///
    /// Must be called after `local_config` is updated. Runs in a spawned task
    /// so it can be used from synchronous refresh paths. If there is no async
    /// runtime available (e.g. in unit tests that build a controller without
    /// one), the sync is skipped silently.
    fn spawn_sync_mcp_manager(&self) {
        let Some(config) = self.local_config.clone() else {
            return;
        };
        let Some(manager) = self.mcp_manager.clone() else {
            return;
        };
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        tokio::spawn(async move {
            if let Err(error) = mcp_ops::reload_mcp_manager_from_config(&config, &manager).await {
                tracing::warn!("failed to sync MCP manager: {error}");
            }
        });
    }

    fn push_status(&mut self, message: impl Into<String>) {
        self.transcript_mut().push_status(message);
    }

    #[cfg(test)]
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
                            self.refresh_git_status_now();
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
            self.drain_active_shell_command().await?;
            self.drain_btw_sidecar();
            self.drain_log_events();
            if self.active_turn.is_some() {
                self.refresh_git_status_if_due();
            }
            self.maybe_refresh_task_browser().await;
            self.poll_pending_catalog_fetch().await;
            self.poll_pending_mcp_probe().await;
            self.tui.chrome_mut().advance_activity_frame();
            render(&mut self.tui)?;
        }
        Ok(())
    }

    async fn handle_input_event(&mut self, event: InputEvent) -> Result<bool> {
        if self.handle_pending_approval_event(&event).await? {
            return Ok(false);
        }
        if self.handle_task_browser_event(event.clone()).await? {
            return Ok(false);
        }
        if self.handle_rich_dialog_event(event.clone()).await? {
            return Ok(false);
        }
        if self.handle_prompt_edit_event(&event) {
            return Ok(false);
        }
        match event {
            InputEvent::Key(key) => return self.handle_keybinding_key(&key).await,
            InputEvent::Action(action) => return self.handle_keybinding_action(action).await,
            InputEvent::Submit => {
                self.clear_pending_exit_confirmation();
                self.submit_current_prompt().await?;
            }
            InputEvent::ScrollUp(rows) => self.scroll_transcript_up(rows),
            InputEvent::ScrollDown(rows) => self.scroll_transcript_down(rows),
            InputEvent::Cancel => return self.handle_cancel_input().await,
            InputEvent::Interrupt => return self.handle_interrupt_input().await,
            InputEvent::Resize { .. }
            | InputEvent::Insert(_)
            | InputEvent::Paste(_)
            | InputEvent::Backspace
            | InputEvent::Delete
            | InputEvent::MoveLeft
            | InputEvent::MoveRight
            | InputEvent::MoveHome
            | InputEvent::MoveEnd
            | InputEvent::NewLine => {}
        }

        Ok(false)
    }

    fn handle_prompt_edit_event(&mut self, event: &InputEvent) -> bool {
        match event {
            InputEvent::Insert(character) => {
                self.handle_insert_prompt_event(*character);
            }
            InputEvent::Paste(text) => {
                self.handle_paste_text(text);
            }
            InputEvent::Backspace => {
                if self.tui.chrome().shell_mode_active()
                    && self.tui.chrome().prompt().text.is_empty()
                {
                    self.tui.chrome_mut().exit_shell_mode();
                    return true;
                }
                self.apply_prompt_edit(PromptEdit::Backspace);
            }
            InputEvent::Delete => self.apply_prompt_edit(PromptEdit::Delete),
            InputEvent::MoveLeft => self.apply_prompt_edit(PromptEdit::MoveLeft),
            InputEvent::MoveRight => self.apply_prompt_edit(PromptEdit::MoveRight),
            InputEvent::MoveHome => self.apply_prompt_edit(PromptEdit::MoveHome),
            InputEvent::MoveEnd => self.apply_prompt_edit(PromptEdit::MoveEnd),
            InputEvent::NewLine => self.apply_prompt_edit(PromptEdit::Insert("\n")),
            _ => return false,
        }
        true
    }

    fn handle_insert_prompt_event(&mut self, character: char) {
        if self.try_choose_approval_number(character) {
            return;
        }
        if character == '!'
            && !self.tui.chrome().shell_mode_active()
            && self.tui.chrome().prompt().text.is_empty()
        {
            self.tui.chrome_mut().enter_shell_mode();
            self.sync_inline_prompt_completion();
            return;
        }
        self.apply_prompt_edit(PromptEdit::Insert(&character.to_string()));
    }

    fn try_choose_approval_number(&mut self, character: char) -> bool {
        let Some(number) = approval_number(character) else {
            return false;
        };
        let Some(result) = self.tui.chrome_mut().choose_approval_number(number) else {
            return false;
        };
        self.resolve_approval(&result);
        true
    }

    fn scroll_transcript_up(&mut self, rows: usize) {
        self.clear_pending_exit_confirmation();
        self.transcript_mut().scroll_transcript_up(rows);
    }

    fn scroll_transcript_down(&mut self, rows: usize) {
        self.clear_pending_exit_confirmation();
        self.transcript_mut().scroll_transcript_down(rows);
    }

    async fn handle_pending_approval_event(&mut self, event: &InputEvent) -> Result<bool> {
        if !self.tui.chrome().approval_is_pending() {
            return Ok(false);
        }
        // Interrupt rejects every visible approval (and any pending runtime
        // channels) instead of being swallowed by the dialog handler.
        if matches!(event, InputEvent::Interrupt) {
            self.reject_all_pending_approvals();
            if self.active_turn.is_some() {
                self.cancel_active_turn().await?;
                self.show_notice("Interrupted");
            }
            return Ok(true);
        }
        if let Some(result) = self
            .tui
            .chrome_mut()
            .handle_pending_approval_input(event.clone())
        {
            self.resolve_approval(&result);
        } else {
            self.sync_inline_approval_selection();
        }
        Ok(true)
    }

    async fn handle_rich_dialog_event(&mut self, event: InputEvent) -> Result<bool> {
        if !self.tui.chrome_mut().focused_overlay_is_rich_dialog() {
            return Ok(false);
        }
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
        Ok(true)
    }

    async fn handle_task_browser_event(&mut self, event: InputEvent) -> Result<bool> {
        if self.tui.chrome().task_browser_state().is_none() {
            return Ok(false);
        }
        let Some(action) = self.task_browser_action_for_event(event) else {
            return Ok(true);
        };
        self.apply_task_browser_action(action).await?;
        Ok(true)
    }

    fn task_browser_action_for_event(&self, event: InputEvent) -> Option<TaskBrowserAction> {
        match event {
            InputEvent::Action(KeybindingAction::SelectUp) => Some(TaskBrowserAction::SelectUp),
            InputEvent::Action(KeybindingAction::SelectDown) => Some(TaskBrowserAction::SelectDown),
            InputEvent::Action(KeybindingAction::SelectPageUp) => {
                Some(TaskBrowserAction::SelectPageUp)
            }
            InputEvent::Action(KeybindingAction::SelectPageDown) => {
                Some(TaskBrowserAction::SelectPageDown)
            }
            InputEvent::Action(KeybindingAction::SelectConfirm) | InputEvent::Submit => self
                .tui
                .chrome()
                .task_browser_state()
                .map_or(Some(TaskBrowserAction::ToggleOutputFocus), |state| {
                    if state.stop_confirmation_task_id().is_some() {
                        Some(TaskBrowserAction::ConfirmStop)
                    } else {
                        Some(TaskBrowserAction::ToggleOutputFocus)
                    }
                }),
            InputEvent::Action(KeybindingAction::SelectCancel) | InputEvent::Cancel => {
                Some(TaskBrowserAction::Cancel)
            }
            InputEvent::Action(KeybindingAction::InputTab) | InputEvent::Insert('\t') => {
                Some(TaskBrowserAction::ToggleFilter)
            }
            InputEvent::MoveHome => Some(TaskBrowserAction::SelectFirst),
            InputEvent::MoveEnd => Some(TaskBrowserAction::SelectLast),
            InputEvent::Insert('q' | 'Q') => Some(TaskBrowserAction::Close),
            InputEvent::Insert('r' | 'R') => Some(TaskBrowserAction::Refresh),
            InputEvent::Insert('s' | 'S') => Some(TaskBrowserAction::RequestStop),
            InputEvent::Insert('o' | 'O') => Some(TaskBrowserAction::ToggleOutputFocus),
            InputEvent::Key(key) => {
                let actions = self.keybindings.matching_actions(&key);
                OVERLAY_ACTION_PRIORITY
                    .iter()
                    .chain(std::iter::once(&KeybindingAction::InputTab))
                    .copied()
                    .find(|action| actions.contains(action))
                    .and_then(|action| {
                        self.task_browser_action_for_event(InputEvent::Action(action))
                    })
            }
            _ => None,
        }
    }

    async fn apply_task_browser_action(&mut self, action: TaskBrowserAction) -> Result<()> {
        match action {
            TaskBrowserAction::Refresh => {
                self.refresh_task_browser().await;
            }
            TaskBrowserAction::Close => {
                self.tui.chrome_mut().close_focused_overlay();
            }
            TaskBrowserAction::ConfirmStop => {
                let task_id = self
                    .tui
                    .chrome_mut()
                    .task_browser_state_mut()
                    .and_then(|state| state.handle_action(TaskBrowserAction::ConfirmStop));
                if let Some(task_id) = task_id {
                    self.stop_task_from_browser(task_id).await;
                }
            }
            TaskBrowserAction::Cancel => {
                let result = self
                    .tui
                    .chrome_mut()
                    .task_browser_state_mut()
                    .and_then(|state| state.handle_action(TaskBrowserAction::Cancel));
                if result.as_deref() == Some("__close__") {
                    self.tui.chrome_mut().close_focused_overlay();
                }
            }
            other => {
                if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                    let _ = state.handle_action(other);
                }
            }
        }
        Ok(())
    }

    async fn refresh_task_browser(&mut self) {
        let Some(config) = self.local_config.as_ref() else {
            if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                state.set_footer_message("No config available");
            }
            return;
        };
        let tasks = config.background_tasks.list(false, 50).await;
        let snapshot = task_browser::snapshots_to_browser_snapshot(&tasks);
        if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
            state.apply_snapshot(&snapshot);
            state.clear_footer_message();
        }
        self.last_task_browser_refresh = Some(Instant::now());
    }

    async fn maybe_refresh_task_browser(&mut self) {
        if self.tui.chrome().task_browser_state().is_none() {
            self.last_task_browser_refresh = None;
            return;
        }
        let should_refresh = self
            .last_task_browser_refresh
            .is_none_or(|last_refresh| last_refresh.elapsed() >= TASK_BROWSER_REFRESH_INTERVAL);
        if should_refresh {
            self.refresh_task_browser().await;
        }
    }

    async fn stop_task_from_browser(&mut self, task_id: String) {
        let Some(config) = self.local_config.as_ref() else {
            if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                state.set_footer_message("No config available");
            }
            return;
        };
        let result = config
            .background_tasks
            .stop(
                &task_id,
                "Stopped from Task Browser",
                SHELL_MAX_OUTPUT_BYTES,
            )
            .await;
        match result {
            Ok(_) => self.refresh_task_browser().await,
            Err(error) => {
                if let Some(state) = self.tui.chrome_mut().task_browser_state_mut() {
                    state.set_footer_message(error.to_string());
                }
            }
        }
    }

    fn handle_paste_text(&mut self, text: &str) {
        let cleaned = Self::clean_pasted_text(text);
        if !self.tui.chrome().shell_mode_active()
            && self.tui.chrome().prompt().text.is_empty()
            && let Some(command) = cleaned.strip_prefix('!')
        {
            self.tui.chrome_mut().enter_shell_mode();
            if !command.is_empty() {
                self.apply_prompt_edit(PromptEdit::Insert(command));
            }
            return;
        }
        // When the terminal intercepts Ctrl+V (e.g. Ghostty on macOS) it sends
        // a bracketed-paste event. If the clipboard contains an image (not
        // text), the paste content may be empty or contain non-text artifacts.
        // Try to read an image from the clipboard in that case.
        if cleaned.is_empty() && self.model_supports_images() {
            if let Ok(image) = crate::clipboard::read_clipboard_image() {
                let (width, height) =
                    crate::image_blob::detect_image_dimensions(&image.bytes, &image.mime_type)
                        .unwrap_or((0, 0));
                let sha256 = crate::image_blob::sha256_hex(&image.bytes);
                let id = self.image_attachment_store.add(
                    sha256,
                    image.mime_type,
                    width,
                    height,
                    Some(image.bytes),
                );
                let placeholder = format!("[image #{} ({}x{})]", id, width, height);
                self.apply_prompt_edit(PromptEdit::Insert(&placeholder));
                return;
            }
        }

        let line_count = cleaned.split('\n').count();
        if line_count > 10 || cleaned.len() > 1000 {
            let id = self.next_paste_id;
            self.next_paste_id += 1;
            self.paste_store.insert(id, cleaned);
            let marker = if line_count > 10 {
                format!("[paste +{} lines]", line_count)
            } else {
                format!("[paste {id} chars]")
            };
            self.apply_prompt_edit(PromptEdit::Insert(&marker));
        } else {
            self.apply_prompt_edit(PromptEdit::Insert(&cleaned));
        }
    }

    /// Expand a marker at the cursor back to its original text. Returns true
    /// if a marker was expanded.
    fn expand_marker_at_cursor(&mut self) -> bool {
        let prompt = self.tui.chrome().prompt();
        let text = prompt.text.clone();
        let cursor_byte = prompt.byte_index(prompt.cursor);
        for cap in neo_tui::paste::marker_regex().captures_iter(&text) {
            let m = cap.get(0).expect("regex match has group 0");
            if m.start() <= cursor_byte && m.end() >= cursor_byte {
                let id = cap
                    .get(2)
                    .or_else(|| cap.get(3))
                    .or_else(|| cap.get(5))
                    .and_then(|m| m.as_str().parse::<usize>().ok());
                if let Some((id, original)) = id.and_then(|id| {
                    self.paste_store
                        .get(&id)
                        .cloned()
                        .map(|original| (id, original))
                }) {
                    let before = &text[..m.start()];
                    let after = &text[m.end()..];
                    let new_text = format!("{before}{original}{after}");
                    self.tui.chrome_mut().prompt_mut().set_text(new_text);
                    self.paste_store.remove(&id);
                    return true;
                }
            }
        }
        false
    }

    async fn handle_paste_image(&mut self) -> Result<()> {
        if !self.model_supports_images() {
            // Model doesn't support images — fall through to text paste.
            return self.fallback_text_paste();
        }

        if self.expand_marker_at_cursor() {
            return Ok(());
        }

        let image = match crate::clipboard::read_clipboard_image() {
            Ok(img) => img,
            Err(crate::clipboard::ClipboardError::NoImage) => {
                // No image in clipboard — fall through to text paste (like
                // kimi-code: Ctrl+V pastes text when no image is available).
                return self.fallback_text_paste();
            }
            Err(err) => {
                self.push_status(format!("读取剪贴板图片失败: {err}"));
                return Ok(());
            }
        };

        let (width, height) =
            crate::image_blob::detect_image_dimensions(&image.bytes, &image.mime_type)
                .unwrap_or((0, 0));

        // Use the SHA-256 as a dedup key and the blob path for persistence,
        // but store raw bytes in the attachment for inline base64 encoding.
        // This avoids requiring a session directory at paste time.
        let sha256 = crate::image_blob::sha256_hex(&image.bytes);

        let id = self.image_attachment_store.add(
            sha256,
            image.mime_type,
            width,
            height,
            Some(image.bytes),
        );
        let placeholder = format!("[image #{} ({}x{})]", id, width, height);
        self.apply_prompt_edit(PromptEdit::Insert(&placeholder));
        Ok(())
    }

    /// Read text from the system clipboard and paste it into the prompt.
    /// Used as a fallback when Ctrl+V is pressed but no image is available.
    fn fallback_text_paste(&mut self) -> Result<()> {
        let text = crate::clipboard::read_text_clipboard();
        if let Some(text) = text
            && !text.is_empty()
        {
            self.handle_paste_text(&text);
        }
        Ok(())
    }

    fn model_supports_images(&self) -> bool {
        self.active_model
            .as_ref()
            .and_then(|m| self.model_capabilities.get(&m.alias))
            .map(|c| c.images)
            .unwrap_or(false)
    }

    fn active_session_directory(&self) -> Option<PathBuf> {
        let session_id = self.active_session_id.as_ref()?;
        let config = self.local_config.as_ref()?;
        Some(crate::config::workspace_sessions_dir(config).join(session_id))
    }

    fn apply_prompt_edit(&mut self, edit: PromptEdit<'_>) {
        self.clear_pending_exit_confirmation();
        let body_width = Self::prompt_body_width();
        self.tui
            .chrome_mut()
            .prompt_mut()
            .apply_edit_with_width(edit, body_width);
        self.sync_inline_prompt_completion();
    }

    /// Sanitize pasted text: strip CR and drop control characters except newline.
    fn clean_pasted_text(text: &str) -> String {
        text.replace('\r', "")
            .chars()
            .filter(|c| *c == '\n' || !c.is_control())
            .collect()
    }

    async fn handle_cancel_input(&mut self) -> Result<bool> {
        if self.tui.chrome().shell_running() {
            self.cancel_shell_command().await?;
            return Ok(false);
        }
        if self.tui.chrome().shell_mode_active() && self.tui.chrome().prompt().text.is_empty() {
            self.tui.chrome_mut().exit_shell_mode();
            return Ok(false);
        }
        if self.reject_pending_approval() {
            return Ok(false);
        }
        if self.cancel_focused_overlay() {
            return Ok(false);
        }
        if self.tui.chrome().has_btw_panel() {
            if !self.tui.chrome().prompt().text.is_empty() {
                self.tui
                    .chrome_mut()
                    .prompt_mut()
                    .apply_edit(PromptEdit::Clear);
                return Ok(false);
            }
            self.cancel_btw_sidecar();
            self.tui.chrome_mut().close_btw_panel();
            return Ok(false);
        }
        let _ = self.interrupt_active_or_stale_turn().await?;
        Ok(false)
    }

    async fn handle_interrupt_input(&mut self) -> Result<bool> {
        if self.tui.chrome().shell_running() {
            self.cancel_shell_command().await?;
            return Ok(false);
        }
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
        Ok(self.handle_app_clear())
    }

    async fn handle_keybinding_key(&mut self, key: &KeyId) -> Result<bool> {
        if self.tui.chrome().shell_running() && key.as_str() == "ctrl+b" {
            self.detach_shell_command().await?;
            return Ok(false);
        }
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

    async fn handle_keybinding_action(&mut self, action: KeybindingAction) -> Result<bool> {
        if action == KeybindingAction::PasteImage {
            self.handle_paste_image().await?;
            return Ok(false);
        }
        if self.handle_prompt_keybinding_action(action) {
            return Ok(false);
        }
        if self.handle_transcript_keybinding_action(action) {
            return Ok(false);
        }

        if let Some(exit) = self.handle_basic_keybinding_action(action).await? {
            return Ok(exit);
        }
        if self.handle_overlay_keybinding_action(action).await? {
            return Ok(false);
        }
        unreachable!("prompt edit actions are handled before overlay actions")
    }

    async fn handle_basic_keybinding_action(
        &mut self,
        action: KeybindingAction,
    ) -> Result<Option<bool>> {
        match action {
            KeybindingAction::InputNewLine => self.apply_prompt_edit(PromptEdit::Insert("\n")),
            KeybindingAction::InputTab => self.complete_prompt_or_insert_tab(),
            KeybindingAction::InputCopy => self.copy_prompt_to_clipboard(),
            KeybindingAction::PromptSteer => {
                return self.handle_prompt_steer().await.map(|()| Some(false));
            }
            KeybindingAction::EditLastQueuedMessage => {
                if let Some(text) = self
                    .tui
                    .chrome_mut()
                    .pending_input_mut()
                    .pop_most_recent_shell_command_for_edit()
                {
                    self.tui.chrome_mut().enter_shell_mode();
                    self.tui.chrome_mut().prompt_mut().set_text(text);
                } else if let Some(text) = self
                    .tui
                    .chrome_mut()
                    .pending_input_mut()
                    .pop_most_recent_follow_up_for_edit()
                {
                    self.tui.chrome_mut().exit_shell_mode();
                    self.tui.chrome_mut().prompt_mut().set_text(text);
                }
            }
            KeybindingAction::AppClear => return self.handle_app_clear_action().await.map(Some),
            KeybindingAction::AppExit => return Ok(Some(self.handle_app_exit())),
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
                self.set_plan_mode_from_user(!currently_active);
            }
            KeybindingAction::CycleDevelopmentMode => self.cycle_development_mode(),
            _ => return Ok(None),
        }
        Ok(Some(false))
    }

    async fn handle_overlay_keybinding_action(&mut self, action: KeybindingAction) -> Result<bool> {
        match action {
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
            KeybindingAction::SelectConfirm => self.handle_select_confirm_action().await?,
            KeybindingAction::SelectCancel => {
                let _ = self.handle_cancel_input().await?;
            }
            KeybindingAction::EditorCursorUp | KeybindingAction::EditorCursorDown => {
                unreachable!("prompt history actions are handled before overlay actions")
            }
            KeybindingAction::EditorPageUp => self.transcript_mut().scroll_transcript_up(8),
            KeybindingAction::EditorPageDown => self.transcript_mut().scroll_transcript_down(8),
            _ => return Ok(false),
        }
        Ok(true)
    }

    async fn handle_app_clear_action(&mut self) -> Result<bool> {
        if self.interrupt_active_or_stale_turn().await? {
            return Ok(false);
        }
        Ok(self.handle_app_clear())
    }

    async fn handle_select_confirm_action(&mut self) -> Result<()> {
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
        Ok(())
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
                picked_prefix: false,
                selected_option_label: None,
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
        if self.tui.chrome().mode() != neo_tui::shell::ChromeMode::Streaming {
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
            self.project_trusted(),
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
            self.project_trusted(),
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
        let prompt = self.tui.chrome().prompt();
        let prompt_empty = prompt.text.is_empty();
        let in_history_navigation = prompt.in_history_navigation();

        if prompt_empty {
            if self.tui.chrome().has_btw_panel() {
                match action {
                    KeybindingAction::EditorCursorUp => {
                        self.clear_pending_exit_confirmation();
                        self.tui.chrome_mut().scroll_btw_panel_up(1);
                        return true;
                    }
                    KeybindingAction::EditorCursorDown => {
                        self.clear_pending_exit_confirmation();
                        self.tui.chrome_mut().scroll_btw_panel_down(1);
                        return true;
                    }
                    _ => {}
                }
            }
            if self.handle_prompt_history_action(action) {
                return true;
            }
        } else if !in_history_navigation
            && matches!(
                action,
                KeybindingAction::EditorCursorUp | KeybindingAction::EditorCursorDown
            )
        {
            // Normal editing mode: ↑/↓ move the cursor through wrapped lines.
            self.clear_pending_exit_confirmation();
            let body_width = Self::prompt_body_width();
            let edit = if action == KeybindingAction::EditorCursorUp {
                PromptEdit::MoveUp(body_width)
            } else {
                PromptEdit::MoveDown(body_width)
            };
            self.tui
                .chrome_mut()
                .prompt_mut()
                .apply_edit_with_width(edit, body_width);
            self.sync_inline_prompt_completion();
            return true;
        } else if in_history_navigation
            && matches!(
                action,
                KeybindingAction::EditorCursorUp | KeybindingAction::EditorCursorDown
            )
        {
            // History navigation mode: keep recalling entries even though the
            // composer now shows a non-empty historical prompt.
            if self.handle_prompt_history_action(action) {
                return true;
            }
        }
        let Some(edit) = prompt_edit_for_action(action) else {
            return false;
        };
        self.clear_pending_exit_confirmation();
        let body_width = Self::prompt_body_width();
        self.tui
            .chrome_mut()
            .prompt_mut()
            .apply_edit_with_width(edit, body_width);
        self.sync_inline_prompt_completion();
        true
    }

    /// Width available for prompt content after borders and padding.
    fn prompt_body_width() -> usize {
        let (cols, _) = size().unwrap_or((80, 24));
        let content_width = frame_content_width(usize::from(cols));
        content_width.saturating_sub(2).saturating_sub(4).max(1)
    }

    fn handle_prompt_history_action(&mut self, action: KeybindingAction) -> bool {
        match action {
            KeybindingAction::EditorCursorUp => {
                if !self.tui.chrome_mut().prompt_mut().recall_previous_history() {
                    // No history to recall; pull the most recent queued
                    // shell command or follow-up back into the composer for
                    // editing instead.
                    if let Some(text) = self
                        .tui
                        .chrome_mut()
                        .pending_input_mut()
                        .pop_most_recent_shell_command_for_edit()
                    {
                        self.tui.chrome_mut().enter_shell_mode();
                        self.tui.chrome_mut().prompt_mut().set_text(text);
                    } else if let Some(text) = self
                        .tui
                        .chrome_mut()
                        .pending_input_mut()
                        .pop_most_recent_follow_up_for_edit()
                    {
                        self.tui.chrome_mut().exit_shell_mode();
                        self.tui.chrome_mut().prompt_mut().set_text(text);
                    }
                }
            }
            KeybindingAction::EditorCursorDown => {
                self.tui.chrome_mut().prompt_mut().recall_next_history();
            }
            _ => return false,
        }
        self.clear_pending_exit_confirmation();
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

    fn project_trusted(&self) -> bool {
        self.local_config
            .as_ref()
            .is_none_or(|config| config.project_trusted)
    }

    async fn submit_current_prompt(&mut self) -> Result<()> {
        // If the `/btw` sidecar panel is open, the composer is connected to the
        // sidecar. Route Enter to the sidecar instead of the main turn path.
        if self.tui.chrome().has_btw_panel() {
            return self.submit_btw_prompt().await;
        }

        let prompt = self.tui.chrome_mut().prompt().text.trim_end().to_owned();
        if !self.tui.chrome().shell_mode_active()
            && let Some(command) = prompt.strip_prefix('!')
        {
            self.tui.chrome_mut().enter_shell_mode();
            if command.trim().is_empty() {
                self.tui.chrome_mut().prompt_mut().clear_after_submit();
                return Ok(());
            }
            return self.submit_shell_command(command.to_owned()).await;
        }
        if self.tui.chrome().shell_mode_active() {
            if prompt.trim() == "/tasks" {
                self.clear_submitted_prompt();
                let _ = self.handle_slash_command("/tasks").await;
                return Ok(());
            }
            return self.submit_shell_command(prompt).await;
        }
        if prompt.trim().is_empty() {
            return Ok(());
        }

        // Dismiss any open prompt-completion overlay before handling slash commands
        // or submitting, so it doesn't linger under a newly-opened picker.
        self.close_inline_prompt_completion();

        // `/btw` is a sidecar command: it opens a panel and may start a side
        // question, but it must never submit or queue a main turn, even while
        // one is already running.
        if let Some(arg) = slash_arg(&prompt, "/btw") {
            self.clear_submitted_prompt();
            self.open_btw_panel(if arg.is_empty() {
                None
            } else {
                Some(arg.to_owned())
            })
            .await;
            return Ok(());
        }

        // `/new` / `/clear` are session-lifecycle commands. They stay
        // dispatchable while a turn is running (and report their own blocked
        // status), so dispatch them before the turn-active guard below.
        if matches!(prompt.as_str(), "/new" | "/clear") {
            self.handle_simple_slash_command(&prompt).await;
            return Ok(());
        }

        // Permission-mode slashes (`/ask`, `/auto`, `/yolo`) are always
        // dispatchable. They only update live runtime state and never submit a
        // turn, so they must run even while a turn is active. This is what lets
        // the user switch posture mid-turn without interrupting it.
        if is_live_permission_slash(&prompt) {
            self.handle_permission_slash_command(&prompt);
            return Ok(());
        }

        // `/permissions` opens a focused overlay. During an active turn it could
        // race with an approval/question dialog from that turn, so degrade to a
        // hint and keep the turn uninterrupted. `/ask`/`/auto`/`/yolo` above
        // remain the live switch path.
        if matches!(prompt.as_str(), "/permissions" | "/permission") {
            if self.active_turn.is_some() {
                self.clear_submitted_prompt();
                self.push_status("Use /ask, /auto, or /yolo while a turn is running");
                return Ok(());
            }
            self.handle_permission_slash_command(&prompt);
            return Ok(());
        }

        // While a turn or shell command is running, Enter queues the message as
        // a follow-up instead of starting a concurrent workflow.
        if self.active_turn.is_some() || self.active_shell_command.is_some() {
            self.enqueue_follow_up_from_prompt(&prompt);
            return Ok(());
        }

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
        let content = crate::prompt::parts::expand_prompt_markers(
            &prompt,
            &self.paste_store,
            &self.image_attachment_store,
        );
        // Persist the resolved user prompt (after @model/prompt-template
        // expansion) to the workspace history. Slash commands already returned
        // above, so they never reach this point. Append failures are non-fatal.
        self.append_prompt_history(&content_to_display_text(&content));
        self.start_turn_with_prompt(content, model_override, true);
        self.drain_active_turn().await?;
        self.start_pending_background_question_followups().await
    }

    async fn submit_shell_command(&mut self, prompt: String) -> Result<()> {
        let command = prompt.trim().to_owned();
        self.tui.chrome_mut().prompt_mut().clear_after_submit();
        if command.is_empty() {
            return Ok(());
        }
        if self.active_turn.is_some() || self.active_shell_command.is_some() {
            self.tui
                .chrome_mut()
                .pending_input_mut()
                .queue_shell_command(command);
            return Ok(());
        }
        self.start_shell_command(command).await
    }

    async fn start_shell_command(&mut self, command: String) -> Result<()> {
        let id = self.next_shell_id();
        let cancel_token = CancellationToken::new();
        let background_tasks = self
            .local_config
            .as_ref()
            .map(|config| config.background_tasks.clone())
            .unwrap_or_else(neo_agent_core::tools::BackgroundTaskManager::new);
        let (event_tx, events) = mpsc::unbounded_channel();
        let request = ShellRunRequest {
            id: id.clone(),
            command: command.clone(),
            cwd: self.workspace_root.clone(),
            foreground_timeout: SHELL_FOREGROUND_TIMEOUT,
            background_timeout: SHELL_BACKGROUND_TIMEOUT,
            max_output_bytes: SHELL_MAX_OUTPUT_BYTES,
            cancel_token: cancel_token.clone(),
            background_tasks: background_tasks.clone(),
            event_tx,
        };
        let task = tokio::spawn((self.shell_driver)(request));
        self.tui.chrome_mut().set_shell_running(true);
        self.apply_turn_event(AgentEvent::ShellCommandStarted {
            turn: 0,
            id: id.clone(),
            command: command.clone(),
            cwd: self.workspace_root.clone(),
            origin: ShellCommandOrigin::UserShellMode,
        });
        self.active_shell_command = Some(RunningShellCommand {
            id,
            command,
            task,
            cancel_token,
            background_tasks,
            foreground_task_id: None,
            events,
        });
        Ok(())
    }

    fn next_shell_id(&mut self) -> String {
        let id = format!("shell-{}", self.next_shell_command_id);
        self.next_shell_command_id = self.next_shell_command_id.saturating_add(1);
        id
    }

    async fn drain_active_shell_command(&mut self) -> Result<()> {
        let (events, is_finished) = {
            let Some(shell) = self.active_shell_command.as_mut() else {
                return Ok(());
            };
            let mut events = Vec::new();
            while let Ok(event) = shell.events.try_recv() {
                events.push(event);
            }
            if shell.foreground_task_id.is_none()
                && let Some(task_id) =
                    current_shell_foreground_task_id(&shell.background_tasks).await
            {
                shell.foreground_task_id = Some(task_id);
            }
            (events, shell.task.is_finished())
        };
        for event in events {
            self.apply_turn_event(event);
        }
        if !is_finished {
            return Ok(());
        }
        let shell = self
            .active_shell_command
            .take()
            .expect("active shell was checked");
        let result = shell
            .task
            .await
            .map_err(|error| anyhow::anyhow!("interactive shell task failed: {error}"))?;
        let result = result.unwrap_or_else(|error| neo_agent_core::tools::ShellExecutionResult {
            stdout: String::new(),
            stderr: error.to_string(),
            exit_code: None,
            stdout_truncated: false,
            stderr_truncated: false,
            truncated: false,
            outcome: ShellCommandOutcome::Cancelled,
            foreground_task_id: None,
        });
        self.finish_shell_command(shell.id, shell.command, result)
            .await?;
        self.start_next_queued_after_shell().await
    }

    #[cfg(test)]
    async fn wait_for_active_shell_command(&mut self) -> Result<()> {
        let initial_id = self
            .active_shell_command
            .as_ref()
            .map(|shell| shell.id.clone());
        loop {
            self.drain_active_shell_command().await?;
            let current_id = self
                .active_shell_command
                .as_ref()
                .map(|shell| shell.id.clone());
            if current_id.is_none() || current_id != initial_id {
                tokio::task::yield_now().await;
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    async fn finish_shell_command(
        &mut self,
        id: String,
        command: String,
        result: neo_agent_core::tools::ShellExecutionResult,
    ) -> Result<()> {
        self.apply_turn_event(AgentEvent::ShellCommandFinished {
            turn: 0,
            id,
            exit_code: result.exit_code,
            stdout: result.stdout.clone(),
            stderr: result.stderr.clone(),
            truncated: result.truncated,
            origin: ShellCommandOrigin::UserShellMode,
            outcome: result.outcome.clone(),
        });
        let message = AgentMessage::shell_command(
            command,
            result.stdout,
            result.stderr,
            result.exit_code,
            result.outcome,
            result.truncated,
        );
        let event = AgentEvent::MessageAppended { message };
        self.persist_shell_event(&event).await?;
        self.apply_turn_event(event);
        Ok(())
    }

    async fn persist_shell_event(&mut self, event: &AgentEvent) -> Result<()> {
        let Some(config) = self.local_config.clone() else {
            return Ok(());
        };
        let session_path = self.ensure_shell_session_path(&config).await?;
        let mut writer = neo_agent_core::session::JsonlSessionWriter::open_append(&session_path)
            .await
            .with_context(|| format!("failed to append session {}", session_path.display()))?;
        writer.append_event(event).await?;
        writer.flush().await?;
        Ok(())
    }

    async fn ensure_shell_session_path(&mut self, config: &AppConfig) -> Result<PathBuf> {
        if let Some(session_id) = self.active_session_id.as_deref() {
            return crate::modes::sessions::session_path(session_id, config);
        }
        let session_path = create_interactive_session_path(config).await?;
        let session_id = session_id_from_transcript_path(&session_path)?;
        self.set_active_session_id(session_id.clone());
        let mut writer = neo_agent_core::session::JsonlSessionWriter::create(&session_path)
            .await
            .with_context(|| format!("failed to create session {}", session_path.display()))?;
        writer.flush().await?;
        let _ = neo_agent_core::session::SessionMetadataStore::new(workspace_sessions_dir(config))
            .record_activity(
                &session_id,
                Some(config.project_dir.display().to_string()),
                Some("shell command".to_owned()),
                current_unix_timestamp(),
            );
        Ok(session_path)
    }

    async fn start_next_queued_after_shell(&mut self) -> Result<()> {
        if let Some(command) = self
            .tui
            .chrome_mut()
            .pending_input_mut()
            .drain_next_shell_command()
        {
            return self.start_shell_command(command).await;
        }
        self.tui.chrome_mut().set_shell_running(false);
        self.tui
            .chrome_mut()
            .apply_stream_update(StreamUpdate::TurnFinished);
        if self.active_turn.is_none()
            && let Some(prompt) = self
                .tui
                .chrome_mut()
                .pending_input_mut()
                .drain_next_follow_up()
        {
            let PromptSubmission {
                prompt,
                model_override,
            } = PromptSubmission::from_text(
                prompt,
                &self.model_items,
                self.local_config.as_ref(),
                &self.completion_root,
            )?;
            let content = crate::prompt::parts::expand_prompt_markers(
                &prompt,
                &self.paste_store,
                &self.image_attachment_store,
            );
            self.append_prompt_history(&content_to_display_text(&content));
            self.start_turn_with_prompt(content, model_override, true);
        }
        Ok(())
    }

    async fn cancel_shell_command(&mut self) -> Result<()> {
        let Some(shell) = self.active_shell_command.as_ref() else {
            return Ok(());
        };
        shell.cancel_token.cancel();
        self.wait_for_shell_cancel_or_abort().await
    }

    async fn detach_shell_command(&mut self) -> Result<()> {
        let Some(shell) = self.active_shell_command.as_ref() else {
            return Ok(());
        };
        let task_id = if let Some(task_id) = shell.foreground_task_id.clone() {
            Some(task_id)
        } else {
            current_shell_foreground_task_id(&shell.background_tasks).await
        };
        let Some(task_id) = task_id else {
            self.push_status("Shell command is not ready to background yet");
            return Ok(());
        };
        shell.background_tasks.detach(&task_id).await?;
        self.wait_for_shell_detach_or_abort(task_id).await
    }

    async fn wait_for_shell_cancel_or_abort(&mut self) -> Result<()> {
        for _ in 0..200 {
            self.drain_active_shell_command().await?;
            if self.active_shell_command.is_none() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if let Some(shell) = self.active_shell_command.take() {
            let foreground_task_id = match shell.foreground_task_id {
                Some(task_id) => Some(task_id),
                None => current_shell_foreground_task_id(&shell.background_tasks).await,
            };
            let tasks = shell.background_tasks.list(true, 50).await;
            for task in tasks
                .into_iter()
                .filter(|task| foreground_task_id.as_deref() == Some(task.task_id.as_str()))
            {
                let _ = shell
                    .background_tasks
                    .stop(
                        &task.task_id,
                        "Cancelled foreground shell command",
                        SHELL_MAX_OUTPUT_BYTES,
                    )
                    .await;
            }
            shell.task.abort();
            self.tui.chrome_mut().set_shell_running(false);
            self.tui
                .chrome_mut()
                .apply_stream_update(StreamUpdate::TurnFinished);
            let result = neo_agent_core::tools::ShellExecutionResult {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                stdout_truncated: false,
                stderr_truncated: false,
                truncated: false,
                outcome: ShellCommandOutcome::Cancelled,
                foreground_task_id,
            };
            self.finish_shell_command(shell.id, shell.command, result)
                .await?;
        }
        Ok(())
    }

    async fn wait_for_shell_detach_or_abort(&mut self, task_id: String) -> Result<()> {
        for _ in 0..200 {
            self.drain_active_shell_command().await?;
            if self.active_shell_command.is_none() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if let Some(shell) = self.active_shell_command.take() {
            let snapshot = shell.background_tasks.snapshot(&task_id).await.ok();
            let output = snapshot.and_then(|snapshot| snapshot.output).unwrap_or(
                neo_agent_core::tools::CommandOutput {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                    stdout_truncated: false,
                    stderr_truncated: false,
                },
            );
            shell.task.abort();
            self.tui.chrome_mut().set_shell_running(false);
            self.tui
                .chrome_mut()
                .apply_stream_update(StreamUpdate::TurnFinished);
            let result = neo_agent_core::tools::ShellExecutionResult {
                stdout: output.stdout,
                stderr: output.stderr,
                exit_code: output.exit_code,
                stdout_truncated: false,
                stderr_truncated: false,
                truncated: false,
                outcome: ShellCommandOutcome::Backgrounded { task_id },
                foreground_task_id: shell.foreground_task_id,
            };
            self.finish_shell_command(shell.id, shell.command, result)
                .await?;
        }
        Ok(())
    }

    /// Queue the current composer text as a follow-up message into the running
    /// turn. Called when Enter is pressed while a turn is active. The runtime
    /// drains follow-ups FIFO after the current workflow completes.
    fn enqueue_follow_up_from_prompt(&mut self, prompt: &str) {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return;
        }
        // Slash commands are not meaningful as queued follow-ups; surface a
        // hint instead of silently queueing them.
        if prompt.starts_with('/') {
            self.push_status("Slash commands can't be queued — wait for the turn to finish");
            return;
        }
        let content = crate::prompt::parts::expand_prompt_markers(
            prompt,
            &self.paste_store,
            &self.image_attachment_store,
        );
        let message = AgentMessage::User { content };
        self.tui.chrome_mut().prompt_mut().clear_after_submit();
        let Some(turn) = &self.active_turn else {
            self.tui
                .chrome_mut()
                .pending_input_mut()
                .queue_follow_up(prompt);
            return;
        };
        turn.steer_input
            .push(neo_agent_core::ActiveTurnInput::FollowUp(message));
    }

    /// Handle the `PromptSteer` keybinding (Ctrl+S by default).
    ///
    /// 1. If the composer has text, steer the running turn with it (inject at
    ///    the next natural break point).
    /// 2. If the composer is empty and a turn is active, re-classify the oldest
    ///    queued follow-up as a steer (FIFO pull).
    /// 3. If no turn is active, fall back to a normal submit so Ctrl+S is never
    ///    a dead key when idle.
    async fn handle_prompt_steer(&mut self) -> Result<()> {
        if self.tui.chrome().shell_mode_active() {
            return Ok(());
        }
        let text = self.tui.chrome().prompt().text.trim().to_owned();
        if !text.is_empty() {
            if self.active_turn.is_some() {
                let content = crate::prompt::parts::expand_prompt_markers(
                    &text,
                    &self.paste_store,
                    &self.image_attachment_store,
                );
                let message = AgentMessage::User { content };
                if let Some(turn) = &self.active_turn {
                    turn.steer_input
                        .push(neo_agent_core::ActiveTurnInput::SteerNow(message));
                }
                self.tui.chrome_mut().prompt_mut().clear_after_submit();
                return Ok(());
            }
            // Idle: behave like a normal submit.
            return self.submit_current_prompt().await;
        }
        let Some(turn) = &self.active_turn else {
            self.push_status("No active turn to steer");
            return Ok(());
        };
        if self
            .tui
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .is_empty()
        {
            self.push_status("No queued follow-up to steer");
        } else {
            turn.steer_input
                .push(neo_agent_core::ActiveTurnInput::PromoteFollowUpToSteer);
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

    /// Replace the workspace prompt history store (test hook).
    #[cfg(test)]
    fn set_prompt_history_store(&mut self, store: crate::prompt::history::PromptHistoryStore) {
        self.prompt_history = Some(store);
    }

    fn refresh_git_status_now(&mut self) {
        let label = (self.git_status_provider)(&self.workspace_root);
        self.tui.chrome_mut().set_git_status_label(label);
        self.last_git_status_refresh = Some(Instant::now());
    }

    fn refresh_git_status_if_due(&mut self) {
        let should_refresh = self
            .last_git_status_refresh
            .is_none_or(|refreshed_at| refreshed_at.elapsed() >= self.git_status_refresh_interval);
        if should_refresh {
            self.refresh_git_status_now();
        }
    }

    fn apply_turn_event(&mut self, event: AgentEvent) {
        let should_refresh_git_status = event_should_refresh_git_status(&event);
        if let AgentEvent::MessageAppended { message } = &event {
            if self.session_messages.last() != Some(message) {
                self.session_messages.push(message.clone());
            }
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
        self.tui.transcript_mut().apply_agent_event(&event);
        self.tui.chrome_mut().apply_agent_event(event);
        if should_refresh_git_status {
            self.refresh_git_status_now();
        }
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
        if let Some(decision) = self.resolved_approvals.remove(&approval.id) {
            if let Some(tx) = approval.feedback_tx {
                let _ = tx.send(None);
            }
            if let Some(tx) = approval.selected_label_tx {
                let _ = tx.send(None);
            }
            let _ = approval.decision_tx.send(decision);
        } else {
            self.pending_approvals.insert(
                approval.id,
                PendingApprovalResponse {
                    decision_tx: approval.decision_tx,
                    feedback_tx: approval.feedback_tx,
                    selected_label_tx: approval.selected_label_tx,
                    session_option_label: approval.session_option_label,
                    prefix_option_label: approval.prefix_option_label,
                },
            );
        }
    }


    /// Register a pending `AskUser` question. Stores the oneshot response channel
    /// and synthesizes a `QuestionRequested` event for the TUI so it can display
    /// the question dialog.
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

    /// Handle `/compact` — request a manual LLM-driven context compaction.
    ///
    /// If a turn is running, the compaction fires at the next loop iteration
    /// (the runtime checks `manual_compact_request` at the top of every
    /// step). If the session is idle, we start a compaction-only turn that
    /// runs the compaction and finishes without sending anything to the model.
    fn request_manual_compaction(&mut self, instruction: Option<String>) {
        if let Ok(mut request) = self.manual_compact_request.lock() {
            *request = Some(instruction.unwrap_or_default());
        }
        if self.active_turn.is_some() {
            self.push_status("Compaction requested — will run after the current step");
        } else {
            // Idle: run a turn that only compacts and then finishes. It does not
            // append a user message and does not call the model afterwards.
            self.push_status("Compacting context…");
            let (event_tx, event_rx) = mpsc::unbounded_channel();
            let (approval_tx, approval_rx) = mpsc::unbounded_channel();
            let (session_id_tx, session_id_rx) = mpsc::unbounded_channel();
            let (question_tx, question_rx) = mpsc::unbounded_channel::<PendingQuestion>();
            let cancel_token = CancellationToken::new();
            let steer_input = neo_agent_core::SteerInputHandle::new();
            let channels = TurnChannels {
                events: event_tx.clone(),
                approvals: approval_tx,
                session_ids: session_id_tx,
                cancel_token: cancel_token.clone(),
                questions: question_tx,
                steer_input: steer_input.clone(),
            };
            let mut request = TurnRequest::new(
                Vec::new(),
                self.active_session_id.clone(),
                self.active_model.clone(),
                None,
            );
            request.permission_mode = self.permission_mode;
            request.live_permission_mode = Arc::clone(&self.live_permission_mode);
            request.plan_mode = Arc::clone(&self.plan_mode);
            request.base_config.clone_from(&self.local_config);
            request.manual_compact_request = Arc::clone(&self.manual_compact_request);
            request.compaction_only = true;
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
                steer_input,
            });
        }
    }

    /// Dispatch a rich dialog result after an input event was forwarded.
    async fn process_rich_dialog_result(&mut self, result: InputResult) -> Result<()> {
        if !dialog_result_may_close(result) {
            return Ok(());
        }
        if self.process_model_dialog_result() {
            return Ok(());
        }
        if self.process_provider_dialog_result().await {
            return Ok(());
        }
        self.process_question_dialog_result().await
    }

    fn process_model_dialog_result(&mut self) -> bool {
        if self
            .tui
            .chrome_mut()
            .tabbed_model_selector_result()
            .is_some()
        {
            self.apply_tabbed_model_selection();
        } else if self.tui.chrome_mut().model_selector_result().is_some() {
            self.apply_model_selector_result();
        } else {
            return false;
        }
        true
    }

    async fn process_provider_dialog_result(&mut self) -> bool {
        if self.tui.chrome_mut().provider_manager_action().is_some() {
            self.handle_provider_manager_action();
        } else if self.tui.chrome_mut().mcp_manager_action().is_some() {
            self.handle_mcp_manager_action().await;
        } else if self.tui.chrome_mut().choice_picker_result().is_some() {
            self.handle_choice_picker_result();
        } else if self.tui.chrome_mut().text_input_result().is_some() {
            self.handle_text_input_result();
        } else if self.tui.chrome_mut().api_key_input_result().is_some() {
            self.handle_api_key_input_result();
        } else if self
            .tui
            .chrome_mut()
            .custom_registry_import_result()
            .is_some()
        {
            self.handle_custom_registry_import_result();
        } else if self.tui.chrome_mut().mcp_add_form_result().is_some() {
            self.handle_mcp_add_form_result().await;
        } else {
            return false;
        }
        true
    }

    async fn process_question_dialog_result(&mut self) -> Result<()> {
        if let Some(result) = self.tui.chrome_mut().take_question_result() {
            self.resolve_question(&result.id, result.answers).await?;
        }
        Ok(())
    }

    /// Apply a model selection, updating the active model, context window,
    /// thinking state, and footer indicator.
    fn apply_model_selection(&mut self, selection: &neo_tui::dialogs::ModelSelection) {
        self.tui
            .chrome_mut()
            .set_model_label(selection.alias.clone());
        let selected_model = SelectedModel::from_alias(
            &selection.alias,
            self.local_config.as_ref(),
            &self.model_items,
        )
        .map_or_else(
            |error| {
                tracing::warn!("failed to parse selected model: {error}");
                None
            },
            Some,
        );
        let max_ctx = selected_model
            .as_ref()
            .and_then(|model| model.max_context_tokens);
        self.tui
            .chrome_mut()
            .set_context_window(max_ctx.map(ContextWindow::new));
        self.active_model = selected_model;
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
            // Keep the in-memory config in sync so subsequent turns and the next
            // startup resolve the same model.
            config.default_model.clone_from(&selection.alias);
            if let Some(model) = &self.active_model {
                config.default_provider.clone_from(&model.provider);
            }
        }
        // Persist the selection to disk so the next session opens on the same
        // model the user chose, instead of reverting to a stale default.
        if let Some(config_path) = self.config_path()
            && let Err(error) = crate::config::mutations::set_default_model(&config_path, &selection.alias)
        {
            tracing::warn!("failed to persist default model: {error}");
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
                self.open_add_provider_picker();
            }
            neo_tui::dialogs::ProviderManagerAction::DeleteSource(ids) => {
                self.tui.chrome_mut().close_focused_overlay();
                self.delete_provider_sources(&ids);
            }
        }
    }

    fn open_add_provider_picker(&mut self) {
        let theme = self.tui.chrome().theme();
        self.tui
            .chrome_mut()
            .open_choice_picker(neo_tui::dialogs::ChoicePickerOptions {
                title: "Add Provider".to_owned(),
                items: vec![
                    neo_tui::dialogs::ChoiceItem::new("known", "Known third-party provider")
                        .with_description("Import from models.dev catalog"),
                    neo_tui::dialogs::ChoiceItem::new("custom", "Custom registry (api.json)")
                        .with_description("Import from a custom registry URL"),
                ],
                initial_id: None,
                theme,
                page_size: 0,
                current_id: None,
            });
    }

    fn delete_provider_sources(&mut self, ids: &[String]) {
        let Some(config_path) = self.config_path() else {
            return;
        };
        for id in ids {
            if let Err(error) = crate::config::mutations::remove_provider(&config_path, id) {
                self.push_status(format!("Failed to remove provider {id}: {error}"));
            }
        }
        self.push_status(format!("Removed {} provider(s)", ids.len()));
        self.refresh_config();
    }

    /// Handle a `ChoicePicker` result.
    fn handle_choice_picker_result(&mut self) {
        let Some(result) = self.tui.chrome_mut().choice_picker_result().cloned() else {
            return;
        };
        self.tui.chrome_mut().close_focused_overlay();
        if let neo_tui::dialogs::ChoiceResult::Selected(item) = result {
            self.handle_selected_choice_item(&item.id);
        }
    }

    fn handle_selected_choice_item(&mut self, id: &str) {
        if self.handle_permission_choice_item(id) {
            return;
        }
        if self.handle_catalog_choice_item(id) {
            return;
        }
        self.handle_builtin_choice_item(id);
    }

    fn handle_builtin_choice_item(&mut self, id: &str) -> bool {
        if self.handle_mcp_choice_item(id) {
            return true;
        }
        match id {
            "known" => self.fetch_known_catalog(),
            "custom" => self.open_custom_registry_import(),
            _ => return false,
        }
        true
    }

    fn handle_permission_choice_item(&mut self, id: &str) -> bool {
        match id {
            "permission:ask" => self.set_permission_mode(PermissionMode::Ask),
            "permission:auto" => self.set_permission_mode(PermissionMode::Auto),
            "permission:yolo" => self.set_permission_mode(PermissionMode::Yolo),
            _ => return false,
        }
        true
    }

    fn handle_catalog_choice_item(&mut self, id: &str) -> bool {
        if let Some(provider_id) = id.strip_prefix("catalog:") {
            self.open_catalog_api_key_input(provider_id);
            return true;
        }
        if let Some(provider_id) = id.strip_prefix("custom-catalog:") {
            self.import_custom_catalog_provider(provider_id);
            return true;
        }
        false
    }

    /// Handle an API key input result.
    fn handle_text_input_result(&mut self) {
        let Some(result) = self.tui.chrome_mut().text_input_result().cloned() else {
            return;
        };
        self.tui.chrome_mut().close_focused_overlay();
        match result {
            neo_tui::dialogs::TextInputResult::Submitted(_value) => {}
            neo_tui::dialogs::TextInputResult::Cancelled => {}
        }
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
        if !model_items.iter().any(|item| item.value == model_value) {
            return Ok(Self {
                prompt: expand_interactive_prompt(&prompt, config, fallback_project_dir)?,
                model_override: None,
            });
        }
        let prompt_after_model = rest.trim_start();
        if prompt_after_model.is_empty() {
            return Ok(Self {
                prompt,
                model_override: None,
            });
        }

        Ok(Self {
            prompt: expand_interactive_prompt(prompt_after_model, config, fallback_project_dir)?,
            model_override: Some(SelectedModel::from_alias(model_value, config, model_items)?),
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
        config.is_none_or(|c| c.project_trusted),
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

pub trait TerminalEvents {
    fn next_input_event(&mut self) -> Result<InputEvent>;

    fn poll_input_event(&mut self, _timeout: Duration) -> Result<Option<InputEvent>> {
        self.next_input_event().map(Some)
    }
}

struct RawStdinEvents {
    parser: InputParser,
    pending: VecDeque<InputEvent>,
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    disconnected: bool,
}

impl RawStdinEvents {
    fn new(keybindings: KeybindingsManager) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        // Spawn a background thread that blocks on raw stdin reads and forwards
        // byte chunks through the channel. The thread exits on EOF or read error
        // (e.g. terminal closed). The JoinHandle is intentionally dropped — the
        // thread is daemon-like and will be reaped at process exit. When the
        // `RawStdinEvents` is dropped, `rx` is dropped; the next `tx.send()` in
        // the thread fails and the thread exits.
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin();
            let mut buf = [0u8; 4096];
            loop {
                match std::io::Read::read(&mut stdin, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            parser: InputParser::with_keybindings(keybindings),
            pending: VecDeque::new(),
            rx,
            disconnected: false,
        }
    }
}

impl Default for RawStdinEvents {
    fn default() -> Self {
        Self::new(KeybindingsManager::default())
    }
}

impl TerminalEvents for RawStdinEvents {
    fn next_input_event(&mut self) -> Result<InputEvent> {
        loop {
            if let Some(input) = self.poll_input_event(Duration::from_millis(250))? {
                return Ok(input);
            }
            if self.disconnected {
                anyhow::bail!("stdin reader closed");
            }
        }
    }

    fn poll_input_event(&mut self, timeout: Duration) -> Result<Option<InputEvent>> {
        if let Some(input) = self.pending.pop_front() {
            return Ok(Some(input));
        }

        if self.disconnected {
            return Ok(None);
        }

        let deadline = Instant::now() + timeout;
        let mut got_data = false;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self.rx.recv_timeout(remaining) {
                Ok(bytes) => {
                    self.pending.extend(self.parser.feed_bytes(&bytes));
                    // Drain any more immediately available bytes
                    while let Ok(more_bytes) = self.rx.try_recv() {
                        self.pending.extend(self.parser.feed_bytes(&more_bytes));
                    }
                    got_data = true;
                    break;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    self.disconnected = true;
                    break;
                }
            }
        }

        // Only flush incomplete sequences when no data arrived within the timeout
        // window. Flushing immediately after receiving data could break a partial
        // escape sequence that hasn't fully arrived yet.
        if !got_data {
            self.pending.extend(self.parser.flush_timeout());
        }

        Ok(self.pending.pop_front())
    }
}

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
    let registry = crate::modes::run::model_registry_for_config(config).ok();
    let selected_model = registry
        .as_ref()
        .and_then(|r| crate::modes::run::select_config_model(r, config).ok());
    let model_capabilities: std::collections::HashMap<String, neo_ai::ModelCapabilities> = registry
        .map(|r| {
            r.list()
                .into_iter()
                .map(|spec| {
                    let alias = format!("{}/{}", spec.provider.0, spec.model);
                    (alias, spec.capabilities)
                })
                .collect()
        })
        .unwrap_or_default();
    let mut config = config.clone();
    if let Some(model) = &selected_model {
        config.default_provider.clone_from(&model.provider.0);
        config.default_model.clone_from(&model.model);
    }
    let run_config = config.clone();
    let run_turn: TurnDriver = Arc::new(move |request, channels| {
        // Prefer the live config snapshot from the dispatching controller so
        // providers/models added at runtime (e.g. via `/provider`) resolve;
        // fall back to the startup snapshot for safety.
        let mut effective_config = request.base_config.unwrap_or_else(|| run_config.clone());
        Box::pin(async move {
            if let Some(model) = request.model {
                effective_config.default_provider = model.provider;
                effective_config.default_model = model.alias;
            }
            effective_config.runtime.reasoning_effort = request.reasoning_effort;
            effective_config.permission_mode = request.permission_mode;
            effective_config.live_permission_mode = Arc::clone(&request.live_permission_mode);
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
                    Some(request.plan_review_feedback.clone()),
                    Some(Arc::clone(&request.plan_mode)),
                    request.goal_mode_authoring,
                    channels.steer_input,
                    request.mcp_manager.clone(),
                    Arc::clone(&request.manual_compact_request),
                    request.compaction_only,
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
                    Some(request.plan_review_feedback.clone()),
                    Some(Arc::clone(&request.plan_mode)),
                    request.goal_mode_authoring,
                    channels.steer_input,
                    request.mcp_manager.clone(),
                    Arc::clone(&request.manual_compact_request),
                    request.compaction_only,
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
        config.default_model_label(),
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
    let default_model_value = config.default_model.clone();
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
    controller.spawn_sync_mcp_manager();
    let skill_store = resources::load_skill_store(
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
    controller.model_capabilities = model_capabilities;
    // Initialise the active model from the default so that features like image
    // paste work before the first turn (which would otherwise set it lazily).
    let model_label = config.default_model_label();
    if controller.active_model.is_none()
        && let Ok(model) =
            SelectedModel::from_alias(&model_label, Some(&config), &controller.model_items)
    {
        controller.active_model = Some(model);
    }
    // Seed the composer's in-memory history from the workspace bucket so Up/Down
    // can recall prompts submitted in earlier TUI sessions for this workspace.
    controller.prompt_history = Some(crate::prompt::history::PromptHistoryStore::for_config(
        &config,
    ));
    controller.load_prompt_history();
    controller.trust_store = crate::trust::ProjectTrustStore::from_home().ok();
    controller
}

#[cfg(test)]
fn empty_session_loader(session_id: String) -> Ready<Result<LoadedSessionTranscript>> {
    ready(Ok(LoadedSessionTranscript::new(
        session_id,
        Vec::new(),
        Vec::new(),
    )))
}

#[cfg(test)]
fn empty_session_forker(session_id: String) -> Ready<Result<ForkedSessionTranscript>> {
    ready(Ok(ForkedSessionTranscript::new(
        session_id.clone(),
        LoadedSessionTranscript::new(session_id, Vec::new(), Vec::new()),
    )))
}

fn split_skill_invocation(input: &str) -> (&str, &str) {
    match input.find(' ') {
        Some(pos) => (&input[..pos], &input[pos + 1..]),
        None => (input, ""),
    }
}

fn expand_slash_skill(
    name: &str,
    args_str: &str,
    skill: &neo_agent_core::skills::LoadedSkill,
) -> Result<(String, String)> {
    let mut invocation = neo_agent_core::skills::parse_skill_invocation(args_str)
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    name.clone_into(&mut invocation.name);
    let expanded = neo_agent_core::skills::expand_skill_body(skill, &invocation)
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    Ok((expanded, invocation.raw_arguments))
}

fn skill_invocation_args(raw_arguments: &str) -> Option<String> {
    if raw_arguments.trim().is_empty() {
        None
    } else {
        Some(raw_arguments.to_owned())
    }
}

const fn prompt_edit_for_action(action: KeybindingAction) -> Option<PromptEdit<'static>> {
    if let Some(edit) = prompt_cursor_edit_for_action(action) {
        return Some(edit);
    }
    prompt_delete_edit_for_action(action)
}

const fn prompt_cursor_edit_for_action(action: KeybindingAction) -> Option<PromptEdit<'static>> {
    match action {
        KeybindingAction::EditorCursorLeft => Some(PromptEdit::MoveLeft),
        KeybindingAction::EditorCursorRight => Some(PromptEdit::MoveRight),
        KeybindingAction::EditorCursorWordLeft => Some(PromptEdit::MoveWordLeft),
        KeybindingAction::EditorCursorWordRight => Some(PromptEdit::MoveWordRight),
        KeybindingAction::EditorCursorLineStart => Some(PromptEdit::MoveHome),
        KeybindingAction::EditorCursorLineEnd => Some(PromptEdit::MoveEnd),
        // Up/Down are handled directly in handle_prompt_keybinding_action, where
        // the composer body width is known, so we do not map them here.
        _ => None,
    }
}

const fn prompt_delete_edit_for_action(action: KeybindingAction) -> Option<PromptEdit<'static>> {
    if let Some(edit) = prompt_delete_range_edit_for_action(action) {
        return Some(edit);
    }
    prompt_undo_yank_edit_for_action(action)
}

const fn prompt_delete_range_edit_for_action(
    action: KeybindingAction,
) -> Option<PromptEdit<'static>> {
    if let Some(edit) = prompt_delete_char_edit_for_action(action) {
        return Some(edit);
    }
    if let Some(edit) = prompt_delete_word_edit_for_action(action) {
        return Some(edit);
    }
    prompt_delete_line_edit_for_action(action)
}

const fn prompt_delete_char_edit_for_action(
    action: KeybindingAction,
) -> Option<PromptEdit<'static>> {
    match action {
        KeybindingAction::EditorDeleteCharBackward => Some(PromptEdit::Backspace),
        KeybindingAction::EditorDeleteCharForward => Some(PromptEdit::Delete),
        _ => None,
    }
}

const fn prompt_delete_word_edit_for_action(
    action: KeybindingAction,
) -> Option<PromptEdit<'static>> {
    match action {
        KeybindingAction::EditorDeleteWordBackward => Some(PromptEdit::DeleteWordBackward),
        KeybindingAction::EditorDeleteWordForward => Some(PromptEdit::DeleteWordForward),
        _ => None,
    }
}

const fn prompt_delete_line_edit_for_action(
    action: KeybindingAction,
) -> Option<PromptEdit<'static>> {
    match action {
        KeybindingAction::EditorDeleteToLineStart => Some(PromptEdit::DeleteToLineStart),
        KeybindingAction::EditorDeleteToLineEnd => Some(PromptEdit::DeleteToLineEnd),
        _ => None,
    }
}

const fn prompt_undo_yank_edit_for_action(action: KeybindingAction) -> Option<PromptEdit<'static>> {
    match action {
        KeybindingAction::EditorYank => Some(PromptEdit::Yank),
        KeybindingAction::EditorUndo => Some(PromptEdit::Undo),
        _ => None,
    }
}

const fn dialog_result_may_close(result: InputResult) -> bool {
    matches!(
        result,
        InputResult::Submitted | InputResult::Cancelled | InputResult::Handled
    )
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

async fn create_interactive_session_path(config: &AppConfig) -> Result<PathBuf> {
    let bucket_dir = workspace_sessions_dir(config);
    tokio::fs::create_dir_all(&bucket_dir)
        .await
        .with_context(|| {
            format!(
                "failed to create sessions directory {}",
                bucket_dir.display()
            )
        })?;

    loop {
        let session_id = format!("session_{}", uuid::Uuid::new_v4());
        let session_dir = bucket_dir.join(&session_id);
        if tokio::fs::metadata(&session_dir).await.is_err() {
            tokio::fs::create_dir_all(&session_dir)
                .await
                .with_context(|| {
                    format!(
                        "failed to create session directory {}",
                        session_dir.display()
                    )
                })?;
            return Ok(session_dir.join("transcript.jsonl"));
        }
    }
}

fn session_id_from_transcript_path(path: &Path) -> Result<String> {
    let session_dir = path
        .parent()
        .with_context(|| format!("invalid session path {}", path.display()))?;
    let id = session_dir
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .with_context(|| format!("invalid session directory name {}", session_dir.display()))?;
    Ok(id.to_owned())
}

fn current_unix_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_owned())
}

async fn current_shell_foreground_task_id(
    manager: &neo_agent_core::tools::BackgroundTaskManager,
) -> Option<String> {
    manager.foreground_bash_task_id().await
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
    let estimated_context_tokens = context.estimated_context_tokens();
    Ok(
        LoadedSessionTranscript::new(session_id, notices, context.messages().to_vec())
            .with_estimated_context_tokens(estimated_context_tokens),
    )
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

#[cfg(test)]
mod tests;
