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
    collections::{BTreeMap, BTreeSet, VecDeque},
    env,
    fmt::Write as _,
    future::Future,
    io::{IsTerminal as _, stdout},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use crossterm::terminal::size;
use neo_agent_core::{
    AgentEvent, AgentMessage, Content, McpConnectionManager, McpOAuthService,
    McpOAuthServiceConfig, McpServerStatus, PendingQuestion, PermissionApprovalDecision,
    PermissionMode, ProcessSupervisor, QuestionResponse, ShellCommandOrigin, ShellCommandOutcome,
    mode::PlanMode,
    session::{JsonlSessionReader, SessionMetadataStore, SessionSummary},
};
use neo_tui::tasks_browser::TaskBrowserAction;
use neo_tui::{
    input::{InputEvent, KeyId, KeybindingAction, KeybindingsManager},
    primitive::InputResult,
    shell::{
        ApprovalChoice, ApprovalResult, ContextWindow, MainAgentTokenUsage, NeoChromeState,
        OverlayKind, PickerItem, PromptEdit, SessionPickerItem, SessionPickerScope, StreamUpdate,
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
use prompt_completion::{
    longest_common_completion_prefix, prompt_completions, session_completion_items,
};

#[cfg(test)]
use prompt_completion::{CompletionCatalog, CompletionSource, completion_source_candidates};

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

mod terminal_io;
use terminal_io::{NeoTerminal, RawStdinEvents, TerminalEvents};

mod shell_command;
use shell_command::{RunningShellCommand, ShellDriver, ShellDriverError};

mod input;
use input::ExitConfirmation;

mod prompt_edit;
use prompt_edit::{
    InlineSkillDirectives, InlineSkillInvocation, content_to_display_text, expand_slash_skill,
    parse_inline_skill_directives, prompt_edit_for_action,
};

mod dialog_results;

mod controller_factory;
pub use controller_factory::controller_for_config;
use controller_factory::{
    create_interactive_session_path, current_unix_timestamp, dialog_result_may_close,
    parse_timestamp, same_work_dir, session_id_from_wire_path, startup_notices,
};
#[cfg(test)]
use controller_factory::{empty_session_forker, empty_session_loader};

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
type ClipboardWriter = Arc<dyn Fn(&str) -> Result<()> + Send + Sync>;
type GitStatusProvider = Arc<dyn Fn(&Path) -> Option<String> + Send + Sync>;

const GIT_STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const TASK_BROWSER_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const SHELL_FOREGROUND_TIMEOUT: Duration = Duration::from_secs(120);
const SHELL_BACKGROUND_TIMEOUT: Duration = Duration::from_secs(600);
const SHELL_MAX_OUTPUT_BYTES: usize = 200_000;

fn mcp_manager_with_oauth_service() -> McpConnectionManager {
    let supervisor = ProcessSupervisor::default();
    let oauth_service = McpOAuthService::new(McpOAuthServiceConfig {
        neo_home: neo_home(),
    });
    McpConnectionManager::with_oauth_service(supervisor, oauth_service)
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
    controller
        .connect_mcp_at_startup(|tui| terminal.borrow_mut().draw_tui(tui))
        .await?;
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
    resolved_approvals: BTreeMap<String, (PermissionApprovalDecision, Option<String>)>,
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
    /// Kimi-style skill activation prompt waiting to be injected as context for
    /// the next turn.
    pending_skill_context: Option<String>,
    /// Stripped prompt body already rendered inside a skill activation card.
    /// Suppress the matching runtime user-message echo so the transcript does
    /// not show the same body twice.
    pending_skill_user_message_to_suppress: Option<String>,
    /// User prompt already rendered optimistically on idle submit. Suppress the
    /// matching runtime append event so the transcript does not duplicate it.
    pending_local_user_message_to_suppress: Option<String>,
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
    completion_notification: neo_tui::notify::NotificationMode,
    question_notification: neo_tui::notify::NotificationMode,
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
    events: Vec<AgentEvent>,
    estimated_context_tokens: Option<u32>,
    main_agent_token_usage: MainAgentTokenUsage,
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
            events: Vec::new(),
            estimated_context_tokens: None,
            main_agent_token_usage: MainAgentTokenUsage::default(),
        }
    }

    #[must_use]
    pub(crate) fn with_events(mut self, events: impl IntoIterator<Item = AgentEvent>) -> Self {
        self.events = events.into_iter().collect();
        self
    }

    #[must_use]
    pub(crate) const fn with_estimated_context_tokens(mut self, used_tokens: u32) -> Self {
        self.estimated_context_tokens = Some(used_tokens);
        self
    }

    #[must_use]
    pub(crate) const fn with_main_agent_token_usage(mut self, usage: MainAgentTokenUsage) -> Self {
        self.main_agent_token_usage = usage;
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
/// Best-effort dimension extraction from image data for display purposes.
impl InteractiveController {
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
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
            mcp_manager: Some(mcp_manager_with_oauth_service()),
            skill_store: None,
            pending_skill_context: None,
            pending_skill_user_message_to_suppress: None,
            pending_local_user_message_to_suppress: None,
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
            completion_notification: neo_tui::notify::NotificationMode::Bell,
            question_notification: neo_tui::notify::NotificationMode::None,
        }
    }

    /// Fire a notification for the given event based on the controller's
    /// configured notification modes. Called from `drain_active_turn`.
    fn notify_for_event(&self, event: &AgentEvent) {
        use neo_agent_core::StopReason;
        if let AgentEvent::RunFinished { stop_reason, .. } = event
            && matches!(
                stop_reason,
                StopReason::EndTurn | StopReason::ToolUse | StopReason::MaxTokens
            )
        {
            neo_tui::notify::notify_event(
                self.completion_notification,
                neo_tui::notify::EventKind::Completion,
            );
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
        let controller = Self::new(
            title,
            session_label,
            model_label,
            workspace_root,
            driver,
            catalogs,
            Arc::new(move |session_id| Box::pin(load_session(session_id))),
            Arc::new(move |session_id| Box::pin(fork_session(session_id))),
        );
        // Tests must not ring bells or spawn desktop notifications.
        let mut controller = controller;
        controller.completion_notification = neo_tui::notify::NotificationMode::None;
        controller.question_notification = neo_tui::notify::NotificationMode::None;
        controller
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

    async fn connect_mcp_at_startup(
        &mut self,
        mut render: impl FnMut(&mut neo_tui::NeoTui) -> Result<()>,
    ) -> Result<()> {
        let Some(config) = self.local_config.clone() else {
            return Ok(());
        };
        let Some(manager) = self.mcp_manager.clone() else {
            return Ok(());
        };

        let startup_statuses = mcp_ops::mcp_startup_connecting_statuses(&config);
        if !startup_statuses.is_empty() {
            for status in startup_statuses {
                self.transcript_mut().upsert_mcp_startup_status(status);
            }
            render(&mut self.tui)?;
        }

        match mcp_ops::reload_mcp_manager_from_config(&config, &manager).await {
            Ok(_) => loop {
                let snapshots = manager.snapshots().await;
                let settled = snapshots.iter().all(|snapshot| {
                    !matches!(
                        snapshot.status,
                        McpServerStatus::Pending | McpServerStatus::Reconnecting
                    )
                });
                for snapshot in snapshots.iter().filter(|snapshot| {
                    config
                        .mcp
                        .servers
                        .iter()
                        .any(|server| server.enabled && server.id == snapshot.id)
                }) {
                    self.transcript_mut().upsert_mcp_startup_status(
                        mcp_ops::mcp_startup_status_from_snapshot(snapshot),
                    );
                }
                self.tui.transcript_mut().render_tick();
                render(&mut self.tui)?;
                if settled {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            },
            Err(error) => {
                for status in mcp_ops::mcp_startup_failed_statuses(&config, &error.to_string()) {
                    self.transcript_mut().upsert_mcp_startup_status(status);
                }
                self.push_status(format!("MCP startup failed: {error}"));
                render(&mut self.tui)?;
            }
        }
        Ok(())
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
        // Cap render frequency to ~30 FPS (33ms). During streaming, multiple
        // TextDelta events arrive within one 50ms poll cycle; without a cap,
        // the loop renders on every iteration even when the previous render
        // was only milliseconds ago. The cap is a floor, not a ceiling —
        // user input (keyboard, resize) always renders immediately.
        const MIN_RENDER_INTERVAL: Duration = Duration::from_millis(33);
        let mut last_render = Instant::now();
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
                        last_render = Instant::now();
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
            // Throttle rendering during streaming to avoid O(n) re-render
            // storms when there are hundreds of transcript entries.
            let now = Instant::now();
            if self.tui.is_transcript_dirty()
                || now.duration_since(last_render) >= MIN_RENDER_INTERVAL
            {
                render(&mut self.tui)?;
                last_render = now;
            }
        }
        Ok(())
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

    /// Expand a marker at the cursor back to its original text. Returns true
    /// if a marker was expanded.
    /// Read text from the system clipboard and paste it into the prompt.
    /// Used as a fallback when Ctrl+V is pressed but no image is available.
    fn active_session_directory(&self) -> Option<PathBuf> {
        let session_id = self.active_session_id.as_ref()?;
        let config = self.local_config.as_ref()?;
        Some(crate::config::workspace_sessions_dir(config).join(session_id))
    }

    /// Sanitize pasted text: strip CR and drop control characters except newline.
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

    /// Width available for prompt content after borders and padding.
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

        if let Some(directives) = parse_inline_skill_directives(&prompt) {
            if directives
                .invocations
                .iter()
                .any(|invocation| invocation.name.is_empty())
            {
                self.push_status("Usage: /skill:<name> [args]");
                return Ok(());
            }
            let (stripped_prompt, display_body) = match self.activate_skill_directives(directives) {
                Ok(pair) => pair,
                Err(err) => {
                    self.push_status(format!("Skill error: {err}"));
                    return Ok(());
                }
            };
            if stripped_prompt.trim().is_empty() {
                self.clear_submitted_prompt();
                return Ok(());
            }
            self.pending_skill_user_message_to_suppress = Some(display_body);
            let Some(prompt) = self.submit_prompt_text(stripped_prompt) else {
                return Ok(());
            };
            self.start_turn_from_submitted_prompt(prompt, false)?;
            self.drain_active_turn().await?;
            return self.start_pending_background_question_followups().await;
        }

        // Slash commands: handle without submitting a turn or entering streaming mode.
        if self.handle_slash_command(&prompt).await {
            return Ok(());
        }

        let Some(prompt) = self.tui.chrome_mut().submit_prompt() else {
            return Ok(());
        };
        self.start_turn_from_submitted_prompt(prompt, true)?;
        self.drain_active_turn().await?;
        self.start_pending_background_question_followups().await
    }

    fn submit_prompt_text(&mut self, prompt: String) -> Option<String> {
        self.tui.chrome_mut().prompt_mut().set_text(prompt);
        self.tui.chrome_mut().submit_prompt()
    }

    fn start_turn_from_submitted_prompt(
        &mut self,
        prompt: String,
        render_local_user_message: bool,
    ) -> Result<()> {
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
        let display_text = content_to_display_text(&content);
        self.append_prompt_history(&display_text);
        if render_local_user_message {
            self.tui
                .transcript_mut()
                .push_user_message(display_text.clone());
            self.pending_local_user_message_to_suppress = Some(display_text);
        }
        self.start_turn_with_prompt(content, model_override);
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
        let display_text = content_to_display_text(&content);
        let message = AgentMessage::User { content };
        self.tui.chrome_mut().prompt_mut().clear_after_submit();
        let Some(turn) = &self.active_turn else {
            self.tui
                .chrome_mut()
                .pending_input_mut()
                .queue_follow_up(display_text);
            return;
        };
        self.tui
            .chrome_mut()
            .pending_input_mut()
            .queue_follow_up_optimistic(display_text);
        turn.steer_input
            .push(neo_agent_core::ActiveTurnInput::FollowUp(message));
    }

    /// Handle the `PromptSteer` keybinding (Ctrl+S by default).
    ///
    /// 1. Re-classify the oldest queued follow-up as a steer (FIFO pull).
    /// 2. If no follow-up is queued and the composer has text, steer the
    ///    running turn with it at the next natural break point.
    /// 3. If no turn is active, fall back to a normal submit so Ctrl+S is never
    ///    a dead key when idle.
    async fn handle_prompt_steer(&mut self) -> Result<()> {
        if self.tui.chrome().shell_mode_active() {
            return Ok(());
        }
        let text = self.tui.chrome().prompt().text.trim().to_owned();
        let Some(turn) = &self.active_turn else {
            if text.is_empty() {
                self.push_status("No active turn to steer");
                return Ok(());
            }
            // Idle: behave like a normal submit.
            return self.submit_current_prompt().await;
        };
        let steer_input = turn.steer_input.clone();
        if self
            .tui
            .chrome_mut()
            .pending_input_mut()
            .promote_oldest_follow_up_to_steer_optimistic()
            .is_some()
        {
            steer_input.push(neo_agent_core::ActiveTurnInput::PromoteFollowUpToSteer);
            return Ok(());
        }

        if !text.is_empty() {
            let content = crate::prompt::parts::expand_prompt_markers(
                &text,
                &self.paste_store,
                &self.image_attachment_store,
            );
            let display_text = content_to_display_text(&content);
            let message = AgentMessage::User { content };
            steer_input.push(neo_agent_core::ActiveTurnInput::SteerNow(message));
            self.tui
                .chrome_mut()
                .pending_input_mut()
                .queue_steer_optimistic(display_text);
            self.tui.chrome_mut().prompt_mut().clear_after_submit();
            return Ok(());
        }

        self.push_status("No queued follow-up to steer");
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
        self.render_appended_user_message_if_needed(&event);
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

    fn render_appended_user_message_if_needed(&mut self, event: &AgentEvent) {
        let AgentEvent::MessageAppended {
            message: AgentMessage::User { content },
        } = event
        else {
            return;
        };
        let text = content_to_display_text(content);
        if text.trim().is_empty() {
            return;
        }
        if self
            .pending_skill_user_message_to_suppress
            .as_deref()
            .is_some_and(|expected| expected.trim() == text.trim())
        {
            self.pending_skill_user_message_to_suppress = None;
            return;
        }
        if self
            .pending_local_user_message_to_suppress
            .as_deref()
            .is_some_and(|expected| expected.trim() == text.trim())
        {
            self.pending_local_user_message_to_suppress = None;
            return;
        }
        self.tui.transcript_mut().push_user_message(text);
    }

    fn sync_inline_approval_selection(&mut self) {
        let Some((id, selected, feedback_input, selected_suggestion)) =
            self.tui.chrome().approval_selection()
        else {
            return;
        };
        let id = id.to_owned();
        let feedback_input = feedback_input.to_owned();
        self.tui.transcript_mut().select_approval(
            &id,
            selected,
            &feedback_input,
            selected_suggestion,
        );
    }

    fn register_pending_approval(&mut self, approval: crate::modes::run::PromptApprovalRequest) {
        if let Some((decision, feedback)) = self.resolved_approvals.remove(&approval.id) {
            if let Some(tx) = approval.feedback_tx {
                let _ = tx.send(feedback);
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

async fn load_session_transcript(
    session_id: String,
    config: &AppConfig,
) -> Result<LoadedSessionTranscript> {
    let path = crate::modes::sessions::session_path(&session_id, config)?;
    let events = JsonlSessionReader::read_all(&path)
        .await
        .with_context(|| format!("failed to replay session {}", path.display()))?;
    config.multi_agent.restore_from_replay(events.iter());
    let context = neo_agent_core::AgentContext::from_replay(events.iter());
    let main_agent_token_usage = replay_main_agent_token_usage(events.iter());
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
            .with_events(events)
            .with_estimated_context_tokens(estimated_context_tokens)
            .with_main_agent_token_usage(main_agent_token_usage),
    )
}

fn replay_main_agent_token_usage<'a>(
    events: impl IntoIterator<Item = &'a AgentEvent>,
) -> MainAgentTokenUsage {
    let mut usage = MainAgentTokenUsage::default();
    for event in events {
        if let AgentEvent::TokenUsage {
            usage: event_usage, ..
        } = event
        {
            usage.add(*event_usage);
        }
    }
    usage
}

fn replay_session_into_transcript(
    transcript: &mut TranscriptPane,
    loaded: &LoadedSessionTranscript,
) {
    for notice in &loaded.notices {
        transcript.push_transcript(neo_tui::transcript::TranscriptEntry::status(notice.clone()));
    }
    let mut suppressor = DelegateReplaySuppressor::from_events(&loaded.events);
    if loaded.events.is_empty() {
        for message in &loaded.messages {
            suppressor.replay_message(transcript, message);
        }
    } else {
        for event in &loaded.events {
            match event {
                AgentEvent::MessageAppended { message } => {
                    suppressor.replay_message(transcript, message);
                }
                AgentEvent::DelegateStarted { .. }
                | AgentEvent::DelegateUpdated { .. }
                | AgentEvent::DelegateFinished { .. }
                | AgentEvent::DelegateSwarmStarted { .. }
                | AgentEvent::DelegateSwarmUpdated { .. }
                | AgentEvent::DelegateSwarmFinished { .. } => {
                    transcript.apply_agent_event(event);
                }
                _ => {}
            }
        }
    }
    suppressor.finish(transcript);
}

#[derive(Debug, Default)]
struct DelegateReplaySuppressor {
    delegate_ids: BTreeSet<String>,
    swarm_ids: BTreeSet<String>,
    pending: BTreeMap<String, PendingDelegateToolCall>,
}

#[derive(Debug, Clone)]
struct PendingDelegateToolCall {
    kind: DelegateReplayKind,
    tool_call: neo_agent_core::AgentToolCall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DelegateReplayKind {
    Delegate,
    DelegateSwarm,
}

impl DelegateReplaySuppressor {
    fn from_events(events: &[AgentEvent]) -> Self {
        let mut suppressor = Self::default();
        for event in events {
            match event {
                AgentEvent::DelegateStarted { agent, .. }
                | AgentEvent::DelegateUpdated { agent, .. }
                | AgentEvent::DelegateFinished { agent, .. } => {
                    suppressor.delegate_ids.insert(agent.id.as_str().to_owned());
                }
                AgentEvent::DelegateSwarmStarted { swarm, .. }
                | AgentEvent::DelegateSwarmUpdated { swarm, .. }
                | AgentEvent::DelegateSwarmFinished { swarm, .. } => {
                    suppressor.swarm_ids.insert(swarm.swarm_id.clone());
                }
                _ => {}
            }
        }
        suppressor
    }

    fn replay_message(&mut self, transcript: &mut TranscriptPane, message: &AgentMessage) {
        match message {
            AgentMessage::Assistant {
                content,
                tool_calls,
                stop_reason,
            } => {
                self.flush_pending(transcript);
                let tool_calls = tool_calls
                    .iter()
                    .filter(|tool_call| !self.defer_delegate_tool_call(tool_call))
                    .cloned()
                    .collect::<Vec<_>>();
                if !content.is_empty() || !tool_calls.is_empty() {
                    transcript.replay_message(&AgentMessage::Assistant {
                        content: content.clone(),
                        tool_calls,
                        stop_reason: *stop_reason,
                    });
                }
            }
            AgentMessage::ToolResult {
                tool_call_id,
                content,
                is_error,
                ..
            } => {
                if let Some(pending) = self.pending.remove(tool_call_id.as_ref()) {
                    if *is_error || !self.result_matches_restored_target(pending.kind, content) {
                        Self::replay_tool_call(transcript, pending.tool_call);
                        transcript.replay_message(message);
                    }
                } else {
                    transcript.replay_message(message);
                }
            }
            _ => {
                self.flush_pending(transcript);
                transcript.replay_message(message);
            }
        }
    }

    fn finish(&mut self, transcript: &mut TranscriptPane) {
        self.flush_pending(transcript);
    }

    fn defer_delegate_tool_call(&mut self, tool_call: &neo_agent_core::AgentToolCall) -> bool {
        let Some(kind) = DelegateReplayKind::from_tool_name(&tool_call.name) else {
            return false;
        };
        if !self.has_targets(kind) {
            return false;
        }
        self.pending.insert(
            tool_call.id.to_string(),
            PendingDelegateToolCall {
                kind,
                tool_call: tool_call.clone(),
            },
        );
        true
    }

    fn has_targets(&self, kind: DelegateReplayKind) -> bool {
        match kind {
            DelegateReplayKind::Delegate => !self.delegate_ids.is_empty(),
            DelegateReplayKind::DelegateSwarm => !self.swarm_ids.is_empty(),
        }
    }

    fn result_matches_restored_target(
        &self,
        kind: DelegateReplayKind,
        content: &[Content],
    ) -> bool {
        let text = content
            .iter()
            .filter_map(Content::as_text)
            .collect::<Vec<_>>()
            .join("");
        match kind {
            DelegateReplayKind::Delegate => self.delegate_ids.iter().any(|id| text.contains(id)),
            DelegateReplayKind::DelegateSwarm => self.swarm_ids.iter().any(|id| text.contains(id)),
        }
    }

    fn flush_pending(&mut self, transcript: &mut TranscriptPane) {
        let pending = std::mem::take(&mut self.pending);
        for pending in pending.into_values() {
            Self::replay_tool_call(transcript, pending.tool_call);
        }
    }

    fn replay_tool_call(transcript: &mut TranscriptPane, tool_call: neo_agent_core::AgentToolCall) {
        transcript.replay_message(&AgentMessage::Assistant {
            content: Vec::new(),
            tool_calls: vec![tool_call],
            stop_reason: neo_agent_core::StopReason::ToolUse,
        });
    }
}

impl DelegateReplayKind {
    fn from_tool_name(name: &str) -> Option<Self> {
        match name {
            "Delegate" => Some(Self::Delegate),
            "DelegateSwarm" => Some(Self::DelegateSwarm),
            _ => None,
        }
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
