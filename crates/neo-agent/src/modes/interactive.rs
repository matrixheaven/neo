use crate::{
    config::{self, AppConfig, neo_home, workspace_sessions_dir},
    mcp_ops::{self, authenticate_mcp_server_oauth},
    modes::sessions::{SessionPickerScope as SessionDataScope, session_summaries},
    prompt_templates::{
        PromptTemplateLocation, discover_prompt_template_commands, expand_prompt_template_args,
        load_project_prompt_templates,
    },
    resources,
    trust::{self, ProjectTrustState},
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    env,
    fmt::Write as _,
    fs,
    future::{Future, Ready, ready},
    io::{IsTerminal as _, Write as _, stdout},
    path::{Path, PathBuf},
    pin::Pin,
    process::{Command, Stdio},
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use crossterm::terminal::size;
use neo_agent_core::{
    AgentEvent, AgentMessage, Content, McpConnectionManager, PendingQuestion,
    PermissionApprovalDecision, PermissionMode, ProcessSupervisor, QuestionResponse,
    format_collected_answers,
    goal::GoalManager,
    mode::PlanMode,
    oauth::OAuthStore,
    session::{JsonlSessionReader, SessionMetadataStore, SessionSummary},
    skills::SkillStore,
};
use neo_tui::{
    chrome::{
        ApprovalChoice, ApprovalResult, CommandSpec, ContextWindow, DevelopmentMode,
        GoalModeStatus, NeoChromeState, OverlayKind, PickerItem, PromptEdit, SessionPickerItem,
        SessionPickerScope, StreamUpdate,
    },
    core::InputResult,
    dialogs::{McpManagerOptions, McpServerRow, McpToolStatus},
    image::{ImageProtocolPreference, ImageRenderPolicy, TerminalImageCapabilities},
    input::{InputEvent, InputParser, KeyId, KeybindingAction, KeybindingsManager},
    terminal::TuiRenderer,
    transcript::{TranscriptPane, pane::frame_content_width},
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
type GitStatusProvider = Arc<dyn Fn(&Path) -> Option<String> + Send + Sync>;

const GIT_STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

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

fn approval_result_label(choice: ApprovalChoice) -> &'static str {
    match choice {
        ApprovalChoice::Approve => "Approved",
        ApprovalChoice::AlwaysApprove => "Approved for this session",
        ApprovalChoice::Deny => "Rejected",
        ApprovalChoice::Revise => "Rejected with feedback",
    }
}

/// Build the resolved-transcript label for an `AlwaysApprove` choice from the
/// saved scope/prefix label. The prefix option says "Approve commands starting
/// with X" → resolved shows "Approved commands starting with X". The session
/// option says "Approve this exact command for this session" → resolved shows
/// "Approved this exact command for this session".
fn session_approval_resolved_label(
    choice: ApprovalChoice,
    session_option_label: Option<&str>,
    prefix_option_label: Option<&str>,
    picked_prefix: bool,
) -> String {
    match choice {
        ApprovalChoice::AlwaysApprove => {
            if picked_prefix && let Some(label) = prefix_option_label {
                return label.replacen("Approve", "Approved", 1);
            }
            match session_option_label {
                Some(label) if label.starts_with("Approve ") => {
                    format!("Approved{}", &label["Approve".len()..])
                }
                Some(label) => format!("Approved: {label}"),
                None => approval_result_label(choice).to_owned(),
            }
        }
        other => approval_result_label(other).to_owned(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitStatusBadge {
    branch: String,
    dirty: bool,
    ahead: u32,
    behind: u32,
    added: u32,
    deleted: u32,
}

impl GitStatusBadge {
    fn format(&self) -> String {
        let mut parts = Vec::new();
        if self.added > 0 || self.deleted > 0 {
            parts.push(format!("+{} -{}", self.added, self.deleted));
        } else if self.dirty {
            parts.push("±".to_owned());
        }
        let mut sync = String::new();
        if self.ahead > 0 {
            let _ = write!(sync, "↑{}", self.ahead);
        }
        if self.behind > 0 {
            let _ = write!(sync, "↓{}", self.behind);
        }
        if !sync.is_empty() {
            parts.push(sync);
        }
        if parts.is_empty() {
            self.branch.clone()
        } else {
            format!("{} [{}]", self.branch, parts.join(" "))
        }
    }
}

fn git_status_label(workspace_root: &Path) -> Option<String> {
    git_status_label_with_program("git", workspace_root)
}

fn event_should_refresh_git_status(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::ToolExecutionFinished { .. }
            | AgentEvent::ShellCommandFinished { .. }
            | AgentEvent::TerminalSessionFinished { .. }
            | AgentEvent::TurnFinished { .. }
            | AgentEvent::RunFinished { .. }
    )
}

fn git_status_label_with_program(program: &str, workspace_root: &Path) -> Option<String> {
    let status_output = Command::new(program)
        .arg("-C")
        .arg(workspace_root)
        .args(["status", "--porcelain=v1", "--branch"])
        .output()
        .ok()?;
    if !status_output.status.success() {
        return None;
    }
    let status = String::from_utf8_lossy(&status_output.stdout);
    let mut badge = parse_git_status_porcelain(&status)?;
    if badge.dirty {
        let numstat_output = Command::new(program)
            .arg("-C")
            .arg(workspace_root)
            .args(["diff", "--numstat", "HEAD", "--"])
            .output()
            .ok();
        if let Some(output) = numstat_output
            && output.status.success()
        {
            let numstat = String::from_utf8_lossy(&output.stdout);
            let (added, deleted) = parse_git_numstat(&numstat);
            badge.added = added;
            badge.deleted = deleted;
        }
    }
    Some(badge.format())
}

fn parse_git_status_porcelain(stdout: &str) -> Option<GitStatusBadge> {
    let mut branch = None;
    let mut ahead = 0;
    let mut behind = 0;
    let mut dirty = false;

    for line in stdout.lines() {
        if let Some(header) = line.strip_prefix("## ") {
            let parsed = parse_git_branch_header(header);
            branch = Some(parsed.0);
            ahead = parsed.1;
            behind = parsed.2;
        } else if !line.trim().is_empty() {
            dirty = true;
        }
    }

    branch
        .filter(|name| !name.is_empty())
        .map(|branch| GitStatusBadge {
            branch,
            dirty,
            ahead,
            behind,
            added: 0,
            deleted: 0,
        })
}

fn parse_git_branch_header(header: &str) -> (String, u32, u32) {
    let (branch_part, sync_part) = header
        .split_once(" [")
        .map_or((header, ""), |(branch, sync)| (branch, sync));
    let branch = branch_part
        .strip_prefix("No commits yet on ")
        .unwrap_or(branch_part)
        .split_once("...")
        .map_or(branch_part, |(branch, _)| branch)
        .trim()
        .to_owned();
    let ahead = parse_git_sync_count(sync_part, "ahead ");
    let behind = parse_git_sync_count(sync_part, "behind ");
    (branch, ahead, behind)
}

fn parse_git_sync_count(sync_part: &str, label: &str) -> u32 {
    sync_part
        .split(label)
        .nth(1)
        .and_then(|rest| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok()
        })
        .unwrap_or(0)
}

fn parse_git_numstat(stdout: &str) -> (u32, u32) {
    stdout.lines().fold((0, 0), |(added, deleted), line| {
        let mut parts = line.split('\t');
        let line_added = parse_git_numstat_count(parts.next());
        let line_deleted = parse_git_numstat_count(parts.next());
        (added + line_added, deleted + line_deleted)
    })
}

fn parse_git_numstat_count(value: Option<&str>) -> u32 {
    value
        .filter(|value| *value != "-")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0)
}

fn permission_mode_items() -> Vec<neo_tui::dialogs::ChoiceItem> {
    vec![
        neo_tui::dialogs::ChoiceItem::new(
            "permission:ask",
            "Ask",
        )
        .with_description("Ask before commands, edits, and other risky actions. Read/search tools run directly; session approval rules are respected."),
        neo_tui::dialogs::ChoiceItem::new(
            "permission:auto",
            "Auto",
        )
        .with_description("Run fully non-interactively. Tool actions are approved automatically, and agent questions are skipped so it can decide on its own."),
        neo_tui::dialogs::ChoiceItem::new(
            "permission:yolo",
            "YOLO",
        )
        .with_description("Automatically approve tool actions and plan transitions. The agent can still ask you explicit questions when your input is needed."),
    ]
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
) -> Result<Option<String>> {
    if !stdout().is_terminal() {
        return Ok(Some(execute_with_startup(config, &startup, options)));
    }

    let mut controller = controller_for_config(config);
    controller.apply_startup_options(config, options);

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

type CatalogEntries = BTreeMap<String, neo_ai::catalog::CatalogEntry>;

struct PendingCustomRegistry {
    source: neo_tui::dialogs::CustomRegistrySource,
    catalog: CatalogEntries,
}

enum CatalogFetchSource {
    Known,
    Custom(neo_tui::dialogs::CustomRegistrySource),
}

/// When set, the fetched catalog should be used to write a provider into config
/// (the API-key submit path) instead of opening a provider picker.
#[derive(Clone)]
struct PendingCatalogAdd {
    provider_id: String,
    api_key: Option<String>,
    config_path: PathBuf,
}

struct PendingCatalogFetch {
    source: CatalogFetchSource,
    handle: tokio::task::JoinHandle<Result<CatalogEntries, neo_ai::error::AiError>>,
    pending_add: Option<PendingCatalogAdd>,
}

struct PendingMcpProbe {
    server_id: String,
    handle: tokio::task::JoinHandle<anyhow::Result<neo_agent_core::McpServerSnapshot>>,
}

struct PendingApprovalResponse {
    decision_tx: oneshot::Sender<PermissionApprovalDecision>,
    feedback_tx: Option<oneshot::Sender<Option<String>>>,
    /// Returns the model-supplied plan-review option label the user picked.
    selected_label_tx: Option<oneshot::Sender<Option<String>>>,
    /// Display label for the session-approval option, used for the resolved
    /// transcript line.
    session_option_label: Option<String>,
    /// Display label for the prefix-approval option.
    prefix_option_label: Option<String>,
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
    prompt_history: Option<crate::prompt_history::PromptHistoryStore>,
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

struct RunningTurn {
    events: mpsc::UnboundedReceiver<Result<AgentEvent>>,
    approvals: mpsc::UnboundedReceiver<crate::modes::run::PromptApprovalRequest>,
    session_ids: mpsc::UnboundedReceiver<String>,
    task: JoinHandle<Result<TurnOutcome>>,
    cancel_token: CancellationToken,
    /// Receiver for `AskUserTool`'s reverse-RPC questions.
    questions: mpsc::UnboundedReceiver<PendingQuestion>,
    /// Shared handle kept so the controller can push steer/follow-up input
    /// while the turn runs.
    steer_input: neo_agent_core::SteerInputHandle,
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
fn content_to_display_text(content: &[Content]) -> String {
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
    fn set_git_status_provider(&mut self, provider: GitStatusProvider) {
        self.git_status_provider = provider;
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

    /// Run the workspace trust dialog until the user makes a choice, then persist
    /// and apply the decision. Cancel/close without a choice is treated as
    /// untrusted.
    async fn resolve_trust_dialog_at_startup(
        &mut self,
        data: neo_tui::dialogs::TrustDialogData,
        mut events: impl TerminalEvents,
        mut render: impl FnMut(&mut neo_tui::NeoTui) -> Result<()>,
    ) -> Result<()> {
        self.tui.chrome_mut().open_trust_dialog(data);
        render(&mut self.tui)?;
        loop {
            let result = self.tui.chrome_mut().take_trust_dialog_result();
            if let Some(result) = result {
                self.tui.chrome_mut().close_focused_overlay();
                self.apply_trust_dialog_result(result)?;
                return Ok(());
            }
            match events.poll_input_event(Duration::from_millis(50))? {
                Some(event) => {
                    let exit = self.handle_input_event(event).await?;
                    if exit {
                        // Treat an early loop exit (e.g. double Ctrl-C) as
                        // untrusted so the workspace is never silently trusted.
                        let target = self.local_config.as_ref().map_or_else(
                            || self.workspace_root.clone(),
                            |config| config.project_dir.clone(),
                        );
                        self.apply_trust_dialog_result(
                            neo_tui::dialogs::TrustDialogResult::Untrusted { target },
                        )?;
                        return Ok(());
                    }
                }
                None => tokio::task::yield_now().await,
            }
            self.tui.chrome_mut().advance_activity_frame();
            render(&mut self.tui)?;
        }
    }

    fn apply_trust_dialog_result(
        &mut self,
        result: neo_tui::dialogs::TrustDialogResult,
    ) -> Result<()> {
        let (trusted, target) = match result {
            neo_tui::dialogs::TrustDialogResult::Trust { target } => (true, target),
            neo_tui::dialogs::TrustDialogResult::Untrusted { target } => (false, target),
        };

        if let Some(store) = self.trust_store.as_ref() {
            store.set(&target, Some(trusted))?;
        }

        let status_message = if trusted {
            format!("Workspace trusted: {}", target.display())
        } else {
            "Workspace untrusted: project context disabled".to_owned()
        };

        if let Some(config) = self.local_config.as_mut() {
            config.project_trusted = trusted;
            config.project_trust = if trusted {
                trust::ProjectTrustState::Trusted {
                    target: target.clone(),
                }
            } else {
                trust::ProjectTrustState::Untrusted {
                    target: target.clone(),
                }
            };
        }

        self.push_status(status_message);
        Ok(())
    }

    pub fn apply_startup_options(&mut self, config: &AppConfig, options: InteractiveOptions) {
        self.tui.chrome_mut().set_theme(config.theme.theme);
        self.permission_mode = config.permission_mode;
        if let Ok(mut live) = self.live_permission_mode.write() {
            *live = config.permission_mode;
        }
        self.tui
            .chrome_mut()
            .set_permission_mode(config.permission_mode);
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

    fn set_permission_mode(&mut self, mode: PermissionMode) {
        self.permission_mode = mode;
        if let Ok(mut live) = self.live_permission_mode.write() {
            *live = mode;
        }
        self.tui.chrome_mut().set_permission_mode(mode);
        self.push_status(format!("Permission Mode: {}", mode.label()));
    }

    fn open_permission_picker(&mut self) {
        let current_id = format!("permission:{}", self.permission_mode.label());
        let items = permission_mode_items();
        let theme = self.tui.chrome().theme();
        self.tui
            .chrome_mut()
            .open_choice_picker(neo_tui::dialogs::ChoicePickerOptions {
                title: "Select permission mode".to_owned(),
                items,
                initial_id: Some(current_id.clone()),
                theme,
                page_size: 3,
                current_id: Some(current_id),
            });
    }

    fn set_plan_mode_from_user(&mut self, active: bool) {
        self.sync_runtime_plan_mode(active);
        self.tui.chrome_mut().set_plan_mode(active);
        self.push_status(if active {
            "Plan Mode On"
        } else {
            "Plan Mode Off"
        });
    }

    fn sync_runtime_plan_mode(&mut self, active: bool) {
        let Ok(mut plan_mode) = self.plan_mode.write() else {
            self.push_status("Plan mode state unavailable");
            return;
        };
        if active {
            if plan_mode.is_active() {
                return;
            }
            if let Some(plans_dir) = self.plan_mode_plans_dir() {
                if plan_mode.enter(&plans_dir, true).is_err() {
                    plan_mode.enter_in_memory();
                }
            } else {
                plan_mode.enter_in_memory();
            }
        } else if plan_mode.is_active() {
            plan_mode.exit();
        }
    }

    fn plan_mode_plans_dir(&self) -> Option<PathBuf> {
        Some(self.active_session_directory()?.join("plans"))
    }

    fn cycle_development_mode(&mut self) {
        match self.tui.chrome().development_mode() {
            DevelopmentMode::Normal => self.set_plan_mode_from_user(true),
            DevelopmentMode::Plan => {
                self.set_plan_mode_from_user(false);
                self.tui
                    .chrome_mut()
                    .set_development_mode(DevelopmentMode::Goal(GoalModeStatus::Pending));
                self.push_status("Goal Mode On");
            }
            DevelopmentMode::Goal(_) => {
                self.tui
                    .chrome_mut()
                    .set_development_mode(DevelopmentMode::Normal);
                self.push_status("Goal Mode Off");
            }
        }
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
            self.drain_btw_sidecar();
            if self.active_turn.is_some() {
                self.refresh_git_status_if_due();
            }
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
            InputEvent::Backspace => self.apply_prompt_edit(PromptEdit::Backspace),
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

    fn handle_paste_text(&mut self, text: &str) {
        let cleaned = Self::clean_pasted_text(text);
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
                    .pop_most_recent_follow_up_for_edit()
                {
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

    fn reject_all_pending_approvals(&mut self) -> bool {
        let chrome_results = self.tui.chrome_mut().cancel_all_approvals();
        let had_pending = !chrome_results.is_empty() || !self.pending_approvals.is_empty();
        for result in chrome_results {
            self.resolve_approval(&result);
        }
        for (request_id, pending) in std::mem::take(&mut self.pending_approvals) {
            self.tui
                .transcript_mut()
                .resolve_approval(&request_id, "Rejected");
            if let Some(tx) = pending.feedback_tx {
                let _ = tx.send(None);
            }
            if let Some(tx) = pending.selected_label_tx {
                let _ = tx.send(None);
            }
            let _ = pending.decision_tx.send(PermissionApprovalDecision::Reject);
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
                    // follow-up back into the composer for editing instead.
                    if let Some(text) = self
                        .tui
                        .chrome_mut()
                        .pending_input_mut()
                        .pop_most_recent_follow_up_for_edit()
                    {
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

    fn open_command_palette(&mut self) {
        let (commands, error) = command_specs(&self.completion_root, self.project_trusted());
        if let Some(error) = error {
            self.push_status(format!("Error loading prompt templates: {error}"));
        }
        self.tui.chrome_mut().open_command_palette(commands);
    }

    async fn run_selected_command(&mut self) -> Result<()> {
        let Some(command) = self.tui.chrome_mut().confirm_command_palette() else {
            return Ok(());
        };
        if self.run_prompt_template_command(&command.id) {
            return Ok(());
        }
        if self.run_sync_command(&command.id).await {
            return Ok(());
        }
        self.run_async_command(&command.id).await
    }

    fn run_prompt_template_command(&mut self, command_id: &str) -> bool {
        let Some(name) = command_id.strip_prefix("prompt-template.") else {
            return false;
        };
        self.tui
            .chrome_mut()
            .prompt_mut()
            .apply_edit(PromptEdit::Insert(&format!("/{name} ")));
        true
    }

    async fn run_sync_command(&mut self, command_id: &str) -> bool {
        if self.run_picker_command(command_id).await {
            return true;
        }
        self.run_transcript_command(command_id)
            || self.run_permission_command(command_id)
            || self.run_plan_command(command_id)
    }

    async fn run_picker_command(&mut self, command_id: &str) -> bool {
        if self.run_open_picker_command(command_id).await {
            return true;
        }
        self.run_copy_prompt_command(command_id)
    }

    async fn run_open_picker_command(&mut self, command_id: &str) -> bool {
        match command_id {
            "sessions" => self.open_session_picker(),
            "models" => self.open_model_picker(),
            "providers" => self.open_provider_picker(),
            "mcp" => self.open_mcp_manager().await,
            _ => return false,
        }
        true
    }

    fn run_copy_prompt_command(&mut self, command_id: &str) -> bool {
        if command_id != "copy-prompt" {
            return false;
        }
        self.copy_prompt_to_clipboard();
        true
    }

    fn run_transcript_command(&mut self, command_id: &str) -> bool {
        match command_id {
            "select-transcript" => self.transcript_mut().select_visible_transcript_entry(),
            "clear-transcript-selection" => self.transcript_mut().clear_transcript_selection(),
            "copy-transcript-selection" => self.copy_transcript_selection_to_clipboard(),
            _ => return false,
        }
        true
    }

    fn run_permission_command(&mut self, command_id: &str) -> bool {
        match command_id {
            "permissions" => self.open_permission_picker(),
            "permission.ask" => self.set_permission_mode(PermissionMode::Ask),
            "permission.auto" => self.set_permission_mode(PermissionMode::Auto),
            "permission.yolo" => self.set_permission_mode(PermissionMode::Yolo),
            _ => return false,
        }
        true
    }

    fn run_plan_command(&mut self, command_id: &str) -> bool {
        if command_id != "plan" {
            return false;
        }
        let currently_active = self.tui.chrome_mut().is_plan_mode();
        self.set_plan_mode_from_user(!currently_active);
        true
    }

    async fn run_async_command(&mut self, command_id: &str) -> Result<()> {
        if self.run_session_async_command(command_id).await? {
            return Ok(());
        }
        if command_id == "submit" {
            self.submit_current_prompt().await?;
            return Ok(());
        }
        if command_id == "btw" {
            self.open_btw_panel(None).await;
            return Ok(());
        }
        self.push_status(format!("Unknown command: {command_id}"));
        Ok(())
    }

    async fn run_session_async_command(&mut self, command_id: &str) -> Result<bool> {
        match command_id {
            "session.exportHtml" => self.export_active_session_to_html().await?,
            "session.new" => self.start_new_session_from_slash(),
            "fork" => self.fork_current_session().await?,
            _ => return Ok(false),
        }
        Ok(true)
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
        let prompt = prompt.trim();
        if let Some(arg) = slash_arg(prompt, "/btw") {
            self.clear_submitted_prompt();
            self.open_btw_panel(if arg.is_empty() {
                None
            } else {
                Some(arg.to_owned())
            })
            .await;
            return true;
        }
        if self.handle_simple_slash_command(prompt).await {
            return true;
        }
        if self.handle_model_or_skill_slash_command(prompt) {
            return true;
        }
        if self.handle_permission_slash_command(prompt) {
            return true;
        }
        if self.handle_plan_slash_prefix(prompt) {
            return true;
        }
        self.handle_goal_slash_prefix(prompt).await
    }

    async fn handle_simple_slash_command(&mut self, prompt: &str) -> bool {
        match prompt {
            "/new" | "/clear" => {
                let blocked = self.active_turn.is_some();
                self.start_new_session_from_slash();
                if blocked {
                    // Preserve the command text so the user can retry after
                    // interrupting the running turn.
                    return true;
                }
            }
            "/resume" => self.open_session_picker(),
            "/provider" => self.open_provider_picker(),
            "/mcp" => self.open_mcp_manager().await,
            "/compact" => {
                let instruction = slash_arg(prompt, "/compact").map(|arg| {
                    if arg.is_empty() {
                        None
                    } else {
                        Some(arg.to_owned())
                    }
                });
                self.request_manual_compaction(instruction.flatten());
            }
            _ => return false,
        }
        self.clear_submitted_prompt();
        true
    }

    fn handle_model_or_skill_slash_command(&mut self, prompt: &str) -> bool {
        if let Some(alias) = slash_arg(prompt, "/model") {
            self.handle_model_slash_command(alias);
            return true;
        }
        if let Some(arg) = prompt.strip_prefix("/skill:").map(str::trim) {
            self.handle_skill_slash_command(arg);
            return true;
        }
        false
    }

    fn handle_permission_slash_command(&mut self, prompt: &str) -> bool {
        if let Some(mode) = slash_permission_mode(prompt) {
            self.clear_submitted_prompt();
            self.set_permission_mode(mode);
            return true;
        }
        if matches!(prompt, "/permissions" | "/permission") {
            self.clear_submitted_prompt();
            self.open_permission_picker();
            return true;
        }
        false
    }

    fn handle_plan_slash_prefix(&mut self, prompt: &str) -> bool {
        let Some(arg) = slash_arg(prompt, "/plan") else {
            return false;
        };
        self.handle_plan_slash_command(arg);
        true
    }

    async fn handle_goal_slash_prefix(&mut self, prompt: &str) -> bool {
        let Some(arg) = slash_arg(prompt, "/goal") else {
            return false;
        };
        self.clear_submitted_prompt();
        self.handle_goal_command(arg).await
    }

    fn clear_submitted_prompt(&mut self) {
        self.tui.chrome_mut().prompt_mut().clear_after_submit();
    }

    fn handle_model_slash_command(&mut self, alias: &str) {
        self.clear_submitted_prompt();
        if alias.is_empty() {
            self.open_model_picker();
        } else if self.model_items.iter().any(|item| item.value == alias) {
            self.open_model_picker_with_alias(alias);
        } else {
            self.push_status(format!("Error: Unknown model alias: {alias}"));
        }
    }

    fn handle_skill_slash_command(&mut self, arg: &str) {
        self.clear_submitted_prompt();
        if arg.is_empty() {
            self.push_status("Usage: /skill:<name> [args]");
        } else if let Err(err) = self.handle_skill_invocation(arg) {
            self.push_status(format!("Skill error: {err}"));
        }
    }

    fn handle_plan_slash_command(&mut self, arg: &str) {
        self.clear_submitted_prompt();
        if self.handle_plan_toggle_argument(arg) {
            return;
        }
        self.handle_plan_file_argument(arg);
    }

    fn handle_plan_toggle_argument(&mut self, arg: &str) -> bool {
        match arg {
            "" => self.toggle_plan_mode_from_user(),
            "on" => self.set_plan_mode_from_user(true),
            "off" => self.set_plan_mode_from_user(false),
            _ => return false,
        }
        true
    }

    fn handle_plan_file_argument(&mut self, arg: &str) {
        if arg == "clear" {
            self.clear_plan_file();
        } else {
            self.push_unknown_plan_argument(arg);
        }
    }

    fn toggle_plan_mode_from_user(&mut self) {
        let next = !self.tui.chrome_mut().is_plan_mode();
        self.set_plan_mode_from_user(next);
    }

    fn push_unknown_plan_argument(&mut self, arg: &str) {
        self.push_status(format!(
            "Unknown /plan argument: '{arg}'. Usage: /plan [on|off|clear]"
        ));
    }

    fn clear_plan_file(&mut self) {
        let cleared = self
            .plan_mode
            .write()
            .ok()
            .and_then(|mut plan_mode| plan_mode.clear().ok())
            .is_some();
        self.push_status(if cleared {
            "Plan file cleared"
        } else {
            "No plan file to clear"
        });
    }

    async fn handle_goal_command(&mut self, arg: &str) -> bool {
        let Some(manager) = self.goal_manager().await else {
            return true;
        };
        if self.handle_goal_lifecycle_command(&manager, arg).await {
            return true;
        }
        self.handle_goal_objective_command(&manager, arg.trim())
            .await
    }

    async fn handle_goal_lifecycle_command(&mut self, manager: &GoalManager, arg: &str) -> bool {
        if self.handle_goal_status_command(manager, arg) {
            return true;
        }
        self.handle_goal_state_command(manager, arg).await
    }

    fn handle_goal_status_command(&mut self, manager: &GoalManager, arg: &str) -> bool {
        if matches!(arg, "" | "status") {
            self.show_goal_status(manager);
            return true;
        }
        false
    }

    async fn handle_goal_state_command(&mut self, manager: &GoalManager, arg: &str) -> bool {
        match arg {
            "pause" => {
                self.pause_goal(manager).await;
                true
            }
            "resume" => {
                self.resume_goal(manager).await;
                true
            }
            "cancel" => {
                self.cancel_goal(manager).await;
                true
            }
            _ => false,
        }
    }

    async fn goal_manager(&mut self) -> Option<Arc<GoalManager>> {
        if self.goal_manager.is_none()
            && let Some(session_dir) = self.active_session_directory()
        {
            match GoalManager::load(session_dir).await {
                Ok(manager) => self.goal_manager = Some(Arc::new(manager)),
                Err(err) => {
                    self.push_status(format!("Failed to load goal manager: {err}"));
                    return None;
                }
            }
        }
        let Some(manager) = self.goal_manager.clone() else {
            self.push_status("Goal mode is not available");
            return None;
        };
        Some(manager)
    }

    fn show_goal_status(&mut self, manager: &GoalManager) -> bool {
        match manager.active() {
            Some(goal) => self.push_status(format!(
                "Goal: {} | status: {:?}",
                goal.objective, goal.status
            )),
            None => self.push_status("No active goal."),
        }
        true
    }

    async fn pause_goal(&mut self, manager: &GoalManager) {
        match manager.pause().await {
            Ok(Some(goal)) => self.push_goal_status("⏸ Goal paused", &goal.objective),
            Ok(None) => self.push_status("No active goal to pause"),
            Err(err) => self.push_status(format!("Failed to pause goal: {err}")),
        }
    }

    async fn resume_goal(&mut self, manager: &GoalManager) {
        match manager.resume().await {
            Ok(Some(goal)) => self.push_goal_status("▶ Goal resumed", &goal.objective),
            Ok(None) => self.push_status("No active goal to resume"),
            Err(err) => self.push_status(format!("Failed to resume goal: {err}")),
        }
    }

    async fn cancel_goal(&mut self, manager: &GoalManager) {
        match manager.cancel().await {
            Ok(Some(goal)) => self.push_goal_status("⏹ Goal cancelled", &goal.objective),
            Ok(None) => self.push_status("No active goal to cancel"),
            Err(err) => self.push_status(format!("Failed to cancel goal: {err}")),
        }
    }

    async fn handle_goal_objective_command(
        &mut self,
        manager: &GoalManager,
        command: &str,
    ) -> bool {
        if let Some(objective) = command.strip_prefix("replace ") {
            return self.replace_goal(manager, objective.trim()).await;
        }
        if let Some(objective) = command.strip_prefix("next ") {
            return self.queue_next_goal(manager, objective.trim()).await;
        }
        self.start_goal(manager, command).await
    }

    async fn replace_goal(&mut self, manager: &GoalManager, objective: &str) -> bool {
        let goal = neo_agent_core::goal::Goal::new(objective);
        match manager.replace(goal).await {
            Ok(Some(_previous)) => self.push_status(format!("Replaced goal with: {objective}")),
            Ok(None) => self.push_status(format!("Started goal: {objective}")),
            Err(err) => {
                self.push_status(format!("Failed to replace goal: {err}"));
                return true;
            }
        }
        false
    }

    async fn queue_next_goal(&mut self, manager: &GoalManager, objective: &str) -> bool {
        let goal = neo_agent_core::goal::Goal::new(objective);
        match manager.queue_next(goal).await {
            Ok(()) => self.push_status(format!("Queued goal: {objective}")),
            Err(err) => {
                self.push_status(format!("Failed to queue goal: {err}"));
                return true;
            }
        }
        true
    }

    async fn start_goal(&mut self, manager: &GoalManager, objective: &str) -> bool {
        let goal = neo_agent_core::goal::Goal::new(objective);
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
        self.push_goal_status("▶ Goal started", &objective);
        false
    }

    fn push_goal_status(&mut self, prefix: &str, objective: &str) {
        self.transcript_mut()
            .push_transcript(neo_tui::transcript::TranscriptEntry::status(format!(
                "{prefix}: {objective}"
            )));
    }

    fn handle_skill_invocation(&mut self, arg: &str) -> Result<()> {
        let skill_store = self
            .skill_store
            .as_ref()
            .context("skill store not loaded")?;
        let (name, args_str) = split_skill_invocation(arg);
        let skill = skill_store
            .get(name)
            .with_context(|| format!("skill `{name}` not found"))?;
        let description = skill.manifest.description.clone();
        let (expanded, raw_arguments) = expand_slash_skill(name, args_str, skill)?;
        self.push_skill_invocation_entry(name, description, &raw_arguments);
        self.pending_skill_context = Some(expanded);
        self.replace_prompt_text(args_str);
        Ok(())
    }

    fn push_skill_invocation_entry(
        &mut self,
        name: &str,
        description: String,
        raw_arguments: &str,
    ) {
        let args = skill_invocation_args(raw_arguments);
        self.transcript_mut().push_transcript(
            neo_tui::transcript::TranscriptEntry::skill_activated(name, Some(description), args),
        );
    }

    fn replace_prompt_text(&mut self, text: &str) {
        let prompt = self.tui.chrome_mut().prompt_mut();
        text.clone_into(&mut prompt.text);
        prompt.cursor = prompt.text.chars().count();
    }

    async fn submit_current_prompt(&mut self) -> Result<()> {
        // If the `/btw` sidecar panel is open, the composer is connected to the
        // sidecar. Route Enter to the sidecar instead of the main turn path.
        if self.tui.chrome().has_btw_panel() {
            return self.submit_btw_prompt().await;
        }

        let prompt = self.tui.chrome_mut().prompt().text.trim_end().to_owned();
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

        // While a turn is running, Enter queues the message as a follow-up
        // instead of rejecting it. The runtime drains follow-ups FIFO after
        // the current workflow completes.
        if self.active_turn.is_some() {
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
        let content = crate::prompt_parts::expand_prompt_markers(
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
        let content = crate::prompt_parts::expand_prompt_markers(
            prompt,
            &self.paste_store,
            &self.image_attachment_store,
        );
        let message = AgentMessage::User { content };
        self.tui.chrome_mut().prompt_mut().clear_after_submit();
        let Some(turn) = &self.active_turn else {
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
        let text = self.tui.chrome().prompt().text.trim().to_owned();
        if !text.is_empty() {
            if self.active_turn.is_some() {
                let content = crate::prompt_parts::expand_prompt_markers(
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

    fn start_turn_with_prompt(
        &mut self,
        prompt: Vec<Content>,
        model_override: Option<SelectedModel>,
        show_user_message: bool,
    ) {
        if self.active_turn.is_some() {
            self.push_status("A turn is already running");
            return;
        }
        if show_user_message {
            self.tui
                .transcript_mut()
                .push_user_message(content_to_display_text(&prompt));
        }
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
            prompt,
            self.active_session_id.clone(),
            model_override.or_else(|| self.active_model.clone()),
            if self.current_thinking {
                Some(neo_ai::ReasoningEffort::High)
            } else {
                None
            },
        );
        request.permission_mode = self.permission_mode;
        request.live_permission_mode = Arc::clone(&self.live_permission_mode);
        request.plan_mode = Arc::clone(&self.plan_mode);
        request.goal_mode_authoring = matches!(
            self.tui.chrome().development_mode(),
            DevelopmentMode::Goal(GoalModeStatus::Pending)
        );
        request.plan_review_feedback = std::mem::take(&mut self.pending_plan_review_feedback);
        request.mcp_manager.clone_from(&self.mcp_manager);
        request.base_config.clone_from(&self.local_config);
        request.manual_compact_request = Arc::clone(&self.manual_compact_request);
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
            steer_input,
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
            self.refresh_git_status_now();
        } else {
            self.active_turn = Some(turn);
        }
        Ok(())
    }

    /// Cancel any running `/btw` sidecar and clear its receiver.
    fn cancel_btw_sidecar(&mut self) {
        if let Some(runner) = self.btw_runner.take() {
            runner.cancel();
        }
        self.btw_receiver = None;
    }

    /// Open or focus the `/btw` sidecar panel.
    ///
    /// If `initial_prompt` is `Some`, a sidecar turn is started immediately using
    /// the sidecar runner's in-memory conversation. The main turn is never
    /// touched.
    async fn open_btw_panel(&mut self, initial_prompt: Option<String>) {
        if self.tui.chrome().has_btw_panel() {
            if initial_prompt.is_none() {
                self.update_btw_panel_error("BTW sidecar is already open.");
                return;
            }
            if self.tui.chrome().btw_panel_state().is_some_and(|state| {
                state.sidecar.phase == neo_tui::widgets::btw_panel::BtwPhase::Running
            }) {
                self.update_btw_panel_error(
                    "Wait for /btw to finish before sending another question.",
                );
                return;
            }
        } else {
            let sidecar_id = uuid::Uuid::new_v4().to_string();
            let state = neo_tui::widgets::btw_panel::BtwPanelState::new(
                neo_tui::widgets::btw_panel::BtwSidecar::new(sidecar_id)
                    .with_parent_session_id(self.active_session_id.clone().unwrap_or_default()),
            );
            self.tui.chrome_mut().set_btw_panel_state(Some(state));
        }

        if self.btw_runner.is_none() {
            let Some(runner) = self.create_btw_runner().await else {
                return;
            };
            self.btw_runner = Some(runner);
        }

        if let Some(prompt) = initial_prompt {
            let Some(runner) = self.btw_runner.as_ref() else {
                return;
            };
            match runner.run(prompt).await {
                Ok(receiver) => {
                    self.btw_receiver = Some(receiver);
                }
                Err(error) => {
                    self.push_status(format!("BTW failed to start: {error}"));
                    self.update_btw_panel_error(&error.to_string());
                }
            }
        }
    }

    async fn create_btw_runner(&mut self) -> Option<crate::modes::btw::BtwRunner> {
        let Some(config) = self.local_config.clone() else {
            self.push_status("BTW requires a loaded config");
            return None;
        };

        let inherited_messages = self.load_btw_inherited_messages(&config).await;
        let Some(model) = self.resolve_btw_model(&config) else {
            return None;
        };
        let Some(client) = self.resolve_btw_client(&config, &model) else {
            return None;
        };

        Some(crate::modes::btw::BtwRunner::new(
            model,
            client,
            config,
            inherited_messages,
        ))
    }

    async fn load_btw_inherited_messages(
        &self,
        config: &crate::config::AppConfig,
    ) -> Vec<AgentMessage> {
        if !self.session_messages.is_empty() {
            return self.session_messages.clone();
        }
        let Some(session_id) = self.active_session_id.as_ref() else {
            return Vec::new();
        };
        match crate::modes::sessions::session_path(session_id, config) {
            Ok(path) => {
                match neo_agent_core::session::JsonlSessionReader::replay_context(&path).await {
                    Ok(context) => context.messages().to_vec(),
                    Err(error) => {
                        tracing::warn!(?error, "failed to replay session for /btw context");
                        Vec::new()
                    }
                }
            }
            Err(error) => {
                tracing::warn!(?error, "failed to resolve session path for /btw context");
                Vec::new()
            }
        }
    }

    #[allow(clippy::unnecessary_wraps)]
    fn resolve_btw_model(
        &mut self,
        config: &crate::config::AppConfig,
    ) -> Option<neo_ai::ModelSpec> {
        #[cfg(test)]
        {
            let _ = self;
            Some(
                crate::modes::run::resolve_model(config).unwrap_or_else(|_| neo_ai::ModelSpec {
                    provider: neo_ai::ProviderId("test-provider".to_owned()),
                    model: "test-model".to_owned(),
                    api: neo_ai::ApiKind::Local,
                    capabilities: neo_ai::ModelCapabilities::tool_chat(),
                }),
            )
        }
        #[cfg(not(test))]
        match crate::modes::run::resolve_model(config) {
            Ok(model) => Some(model),
            Err(error) => {
                self.push_status(format!("BTW model unavailable: {error}"));
                self.update_btw_panel_error(&error.to_string());
                None
            }
        }
    }

    fn resolve_btw_client(
        &mut self,
        config: &crate::config::AppConfig,
        model: &neo_ai::ModelSpec,
    ) -> Option<Arc<dyn neo_ai::ModelClient>> {
        #[cfg(test)]
        if let Some(client) = self.btw_client.clone() {
            return Some(client);
        }
        match crate::modes::run::resolve_model_client(config, model) {
            Ok(client) => Some(client),
            Err(error) => {
                self.push_status(format!("BTW model client unavailable: {error}"));
                self.update_btw_panel_error(&error.to_string());
                None
            }
        }
    }

    fn update_btw_panel_error(&mut self, message: &str) {
        if let Some(state) = self.tui.chrome_mut().btw_panel_state_mut() {
            state.status_message = Some(message.to_owned());
        }
    }

    /// Drain any pending `/btw` sidecar events into the panel state.
    fn drain_btw_sidecar(&mut self) {
        let Some(receiver) = &mut self.btw_receiver else {
            return;
        };
        while let Ok(event) = receiver.try_recv() {
            if let Some(state) = self.tui.chrome_mut().btw_panel_state_mut() {
                crate::modes::btw::update_btw_panel_state(state, event);
            }
        }
    }

    /// Send the current composer text to the `/btw` sidecar instead of the main
    /// turn. The main turn path is bypassed entirely.
    async fn submit_btw_prompt(&mut self) -> Result<()> {
        // If a sidecar turn is already running, preserve the user's typed text
        // and show a busy notice instead of starting a second concurrent turn.
        if self.tui.chrome().btw_panel_state().is_some_and(|state| {
            state.sidecar.phase == neo_tui::widgets::btw_panel::BtwPhase::Running
        }) {
            if let Some(state) = self.tui.chrome_mut().btw_panel_state_mut() {
                state.status_message =
                    Some("Wait for /btw to finish before sending another question.".to_owned());
            }
            return Ok(());
        }

        let Some(prompt) = self.tui.chrome_mut().submit_prompt() else {
            return Ok(());
        };
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Ok(());
        }

        if self.btw_runner.is_none() {
            let Some(runner) = self.create_btw_runner().await else {
                return Ok(());
            };
            self.btw_runner = Some(runner);
        }

        let Some(runner) = self.btw_runner.as_ref() else {
            return Ok(());
        };
        match runner.run(prompt.to_owned()).await {
            Ok(receiver) => {
                self.btw_receiver = Some(receiver);
                self.drain_btw_sidecar();
            }
            Err(error) => {
                self.push_status(format!("BTW failed to start: {error}"));
                self.update_btw_panel_error(&error.to_string());
            }
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

    /// Load workspace prompt history into the composer's in-memory history.
    /// Failures are non-fatal: prompt history is a convenience, not a runtime
    /// dependency, so we silently keep an empty history on load errors.
    fn load_prompt_history(&mut self) {
        let Some(store) = self.prompt_history.clone() else {
            return;
        };
        match store.load_recent() {
            Ok(entries) => {
                self.tui.chrome_mut().prompt_mut().set_history(entries);
            }
            Err(error) => {
                tracing::warn!(?error, "prompt history unavailable");
            }
        }
    }

    /// Persist an accepted prompt to the workspace history store. Never fails
    /// the calling submit path: append errors become a soft status warning.
    fn append_prompt_history(&mut self, prompt: &str) {
        let Some(store) = self.prompt_history.clone() else {
            return;
        };
        let session_id = self.active_session_id.as_deref();
        if let Err(error) = store.append(session_id, prompt) {
            tracing::warn!(?error, "failed to append prompt history");
            self.push_status(format!("Prompt history unavailable: {error}"));
        }
    }

    /// Replace the workspace prompt history store (test hook).
    #[cfg(test)]
    fn set_prompt_history_store(&mut self, store: crate::prompt_history::PromptHistoryStore) {
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

    fn resolve_approval(&mut self, result: &ApprovalResult) {
        // Peek the pending labels before dispatch consumes the entry, so the
        // resolved transcript line reflects the exact saved scope (or prefix
        // rule). `picked_prefix` comes from chrome's ApprovalResult, which
        // detects the prefix option by its label.
        let (session_label, prefix_label) =
            self.pending_approvals
                .get(&result.request_id)
                .map_or((None, None), |pending| {
                    (
                        pending.session_option_label.clone(),
                        pending.prefix_option_label.clone(),
                    )
                });
        let label = session_approval_resolved_label(
            result.choice,
            session_label.as_deref(),
            prefix_label.as_deref(),
            result.picked_prefix,
        );
        self.tui
            .transcript_mut()
            .resolve_approval(&result.request_id, label);
        let decision = Self::approval_decision(result);
        let feedback = Self::approval_feedback(result);
        self.push_revision_feedback_status(feedback.as_deref());
        self.dispatch_approval_response(result, decision, feedback);
    }

    fn approval_decision(result: &ApprovalResult) -> PermissionApprovalDecision {
        match result.choice {
            ApprovalChoice::Approve => PermissionApprovalDecision::AllowOnce,
            ApprovalChoice::AlwaysApprove if result.picked_prefix => {
                PermissionApprovalDecision::AllowForPrefix
            }
            ApprovalChoice::AlwaysApprove => PermissionApprovalDecision::AllowForSession,
            ApprovalChoice::Deny | ApprovalChoice::Revise => PermissionApprovalDecision::Reject,
        }
    }

    fn approval_feedback(result: &ApprovalResult) -> Option<String> {
        (result.choice == ApprovalChoice::Revise)
            .then(|| result.feedback.clone())
            .flatten()
    }

    fn push_revision_feedback_status(&mut self, feedback: Option<&str>) {
        if let Some(feedback) = feedback {
            self.push_status(format!("Revision feedback: {feedback}"));
        }
    }

    fn dispatch_approval_response(
        &mut self,
        result: &ApprovalResult,
        decision: PermissionApprovalDecision,
        feedback: Option<String>,
    ) {
        if let Some(pending) = self.pending_approvals.remove(&result.request_id) {
            if let Some(tx) = pending.feedback_tx {
                let _ = tx.send(feedback);
            }
            if let Some(tx) = pending.selected_label_tx {
                let _ = tx.send(result.selected_option_label.clone());
            }
            let _ = pending.decision_tx.send(decision);
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
            self.start_turn_with_prompt(vec![Content::text(prompt)], None, false);
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

    async fn open_mcp_manager(&mut self) {
        let Some(config) = self.local_config.clone() else {
            self.push_status("No config available");
            return;
        };
        self.sync_mcp_manager_from_config().await;
        let summaries = if let Some(manager) = &self.mcp_manager {
            let snapshots = manager.snapshots().await;
            mcp_ops::summarize_mcp_servers_from_snapshots(&config, &snapshots)
        } else {
            mcp_ops::summarize_mcp_servers_without_discovery(&config)
        };
        let rows = Self::mcp_rows_from_summaries(summaries);
        let theme = self.tui.chrome().theme();
        self.tui.chrome_mut().open_mcp_manager(&McpManagerOptions {
            servers: rows,
            theme,
        });
    }

    async fn sync_mcp_manager_from_config(&mut self) {
        let Some(config) = self.local_config.clone() else {
            return;
        };
        let Some(manager) = self.mcp_manager.clone() else {
            return;
        };
        if let Err(error) = mcp_ops::reload_mcp_manager_from_config(&config, &manager).await {
            self.push_status(format!("MCP manager sync failed: {error}"));
        }
    }

    fn mcp_rows_from_summaries(summaries: Vec<mcp_ops::McpServerSummary>) -> Vec<McpServerRow> {
        summaries
            .into_iter()
            .map(|summary| McpServerRow {
                id: summary.id,
                transport_label: summary.transport_label,
                enabled: summary.enabled,
                endpoint_summary: summary.endpoint_summary,
                cwd_summary: summary.cwd.map(|p| p.to_string_lossy().into_owned()),
                env_keys: summary.env_keys,
                header_keys: summary.header_keys,
                tool_status: match summary.tools {
                    mcp_ops::McpToolDiscovery::SkippedDisabled => McpToolStatus::SkippedDisabled,
                    mcp_ops::McpToolDiscovery::NotRequested => McpToolStatus::NotDiscovered,
                    mcp_ops::McpToolDiscovery::Success(names) => McpToolStatus::Discovered(names),
                    mcp_ops::McpToolDiscovery::Failed(reason) => McpToolStatus::Failed(reason),
                },
            })
            .collect()
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
        if let Some(used_tokens) = loaded.estimated_context_tokens
            && let Some(window) = self.tui.chrome().context_window()
        {
            self.tui
                .chrome_mut()
                .set_context_window(Some(window.with_used_tokens(used_tokens)));
        }

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
        self.session_messages.clone_from(&loaded.messages);
        *self.tui.transcript_mut() = transcript;
    }

    /// Rebuild the transcript pane from scratch with only the welcome banner,
    /// matching the startup layout for an unsaved `new` session. Used by
    /// `/new` / `/clear` to wipe visible transcript state without deleting the
    /// previous JSONL session.
    fn rebuild_empty_welcome_transcript(&mut self) {
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
        *self.tui.transcript_mut() = transcript;
    }

    /// Reset the in-memory TUI/runtime state so the next prompt starts a fresh
    /// workspace-scoped session. Preserves user-facing choices (model, thinking,
    /// permission mode, plan/goal development mode, workspace root) and only
    /// clears transient turn/overlay/transcript state.
    fn reset_for_new_session(&mut self) {
        self.active_turn = None;
        self.pending_approvals.clear();
        self.resolved_approvals.clear();
        self.pending_questions.clear();
        self.pending_question_prompts.clear();
        self.pending_background_question_followups.clear();
        self.pending_skill_context = None;
        self.pending_plan_review_feedback.clear();
        self.clear_pending_exit_confirmation();
        self.close_inline_prompt_completion();
        self.tui.chrome_mut().clear_interrupted_turn_state();
        self.tui.chrome_mut().clear_todos();
        self.tui.chrome_mut().prompt_mut().clear_after_submit();
        self.goal_manager = None;
        self.active_session_id = None;
        self.session_messages.clear();
        self.tui.chrome_mut().set_session_label("new");
        self.rebuild_empty_welcome_transcript();
    }

    /// Begin a fresh session transition from `/new` / `/clear`. Blocked (with a
    /// status message and no state change) when a turn is still running so we
    /// never drop an in-flight session's tool/approval state on the floor.
    fn start_new_session_from_slash(&mut self) {
        if self.active_turn.is_some() {
            self.push_status(
                "Cannot start a new session while a turn is running. Press Esc to interrupt first.",
            );
            return;
        }
        self.close_inline_prompt_completion();
        self.reset_for_new_session();
        self.push_status("Started fresh session");
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
            && let Err(error) = crate::config_ops::set_default_model(&config_path, &selection.alias)
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

    async fn handle_mcp_manager_action(&mut self) {
        let action = self.tui.chrome_mut().take_mcp_manager_action();
        let Some(action) = action else {
            return;
        };
        match action {
            neo_tui::dialogs::McpManagerAction::Close => {
                self.tui.chrome_mut().close_focused_overlay();
            }
            neo_tui::dialogs::McpManagerAction::Add => {
                self.tui.chrome_mut().close_focused_overlay();
                self.open_add_mcp_transport_picker();
            }
            neo_tui::dialogs::McpManagerAction::Test(id) => {
                self.start_mcp_probe(&id, true);
            }
            neo_tui::dialogs::McpManagerAction::Refresh(id) => {
                self.start_mcp_probe(&id, false);
            }
            neo_tui::dialogs::McpManagerAction::ToggleEnabled(id) => {
                self.toggle_mcp_server_enabled(&id).await;
            }
            neo_tui::dialogs::McpManagerAction::Delete(id) => {
                self.delete_mcp_server(&id).await;
            }
            neo_tui::dialogs::McpManagerAction::Auth(id) => {
                self.start_mcp_oauth_flow(id).await;
            }
        }
    }

    async fn start_mcp_oauth_flow(&mut self, server_id: String) {
        let Some(config) = self.local_config.clone() else {
            self.push_status("No config available");
            return;
        };
        let Some(server) = config.mcp.servers.iter().find(|s| s.id == server_id) else {
            self.push_status("MCP server not found");
            return;
        };
        if server.transport != "http" && server.transport != "sse" {
            self.push_status("OAuth is limited to HTTP/SSE servers");
            return;
        }

        let Some(neo_home) = neo_home() else {
            self.push_status("Failed to resolve neo home directory");
            return;
        };

        self.push_status("Waiting for browser authorization...");
        match authenticate_mcp_server_oauth(&server_id, server, &neo_home).await {
            Ok(_) => {
                self.push_status("OAuth token saved");
                // Automatically sync the manager with the new credentials and
                // probe the server so tool discovery happens without the user
                // having to manually press Enter (Test).
                self.sync_mcp_manager_from_config().await;
                self.start_mcp_probe(&server_id, true);
            }
            Err(err) => {
                self.push_status(format!("OAuth flow failed: {err}"));
            }
        }
    }

    fn open_add_mcp_transport_picker(&mut self) {
        let theme = self.tui.chrome().theme();
        self.tui
            .chrome_mut()
            .open_choice_picker(neo_tui::dialogs::ChoicePickerOptions {
                title: "Add MCP Server".to_owned(),
                items: vec![
                    neo_tui::dialogs::ChoiceItem::new("mcp:add:stdio", "Local stdio (studio)")
                        .with_description("Run a command on this machine"),
                    neo_tui::dialogs::ChoiceItem::new("mcp:add:http", "Remote HTTP")
                        .with_description("JSON-RPC HTTP endpoint"),
                    neo_tui::dialogs::ChoiceItem::new("mcp:add:sse", "Remote SSE")
                        .with_description("JSON-RPC endpoint over SSE"),
                ],
                initial_id: None,
                theme,
                page_size: 0,
                current_id: None,
            });
    }

    fn handle_mcp_choice_item(&mut self, id: &str) -> bool {
        let transport = match id {
            "mcp:add:stdio" => "stdio",
            "mcp:add:http" => "http",
            "mcp:add:sse" => "sse",
            _ => return false,
        };
        self.pending_mcp_add_transport = Some(transport);
        let title = match transport {
            "stdio" => "Add Local stdio MCP Server",
            "http" => "Add Remote HTTP MCP Server",
            "sse" => "Add Remote SSE MCP Server",
            _ => "Add MCP Server",
        };
        self.tui
            .chrome_mut()
            .open_mcp_add_form(neo_tui::dialogs::McpAddFormOptions {
                title: title.to_owned(),
                transport: transport.to_owned(),
            });
        true
    }

    async fn handle_mcp_add_form_result(&mut self) {
        let Some(result) = self.tui.chrome_mut().mcp_add_form_result().cloned() else {
            return;
        };
        self.tui.chrome_mut().close_focused_overlay();
        let transport = self.pending_mcp_add_transport.take().unwrap_or("stdio");
        match result {
            neo_tui::dialogs::McpAddFormResult::Submitted(data) => {
                self.save_mcp_form_server(data, transport).await;
            }
            neo_tui::dialogs::McpAddFormResult::Cancelled => {
                // The add-form overlay was just closed; reopen the MCP manager
                // so the user returns to the server list (updates in-place if
                // an overlay is already focused).
                self.open_mcp_manager().await;
            }
        }
    }

    async fn save_mcp_form_server(
        &mut self,
        data: neo_tui::dialogs::McpAddFormData,
        transport: &'static str,
    ) {
        let cli_type = match transport {
            "stdio" => "studio",
            "http" => "remote-http",
            "sse" => "remote-sse",
            _ => transport,
        };
        let mut headers = data.headers;
        if let Some(token) = data.bearer_token {
            headers.push(format!("Authorization=Bearer {token}"));
        }
        let input = mcp_ops::AddMcpServerInput {
            id: data.name,
            cli_type: cli_type.to_owned(),
            command: data.command,
            url: data.url,
            env: data.env,
            headers,
            cwd: None,
            enabled_tools: vec![],
            disabled_tools: vec![],
            startup_timeout_ms: None,
            tool_timeout_ms: None,
            enabled: true,
        };
        let config = match mcp_ops::build_mcp_server_config(input) {
            Ok(config) => config,
            Err(err) => {
                self.push_status(format!("Invalid MCP server: {err}"));
                return;
            }
        };
        let Some(config_path) = self.config_path() else {
            return;
        };
        if let Err(err) = config::upsert_mcp_server(&config, &config_path) {
            self.push_status(format!("Failed to save MCP server: {err}"));
            return;
        }
        self.push_status(format!("MCP server {} saved", config.id));
        self.refresh_config();
        self.sync_mcp_manager_from_config().await;
        // Reopen the MCP manager overlay to show the newly saved server. With
        // the chrome fix this updates the existing overlay in-place rather
        // than pushing a duplicate layer.
        self.open_mcp_manager().await;
    }

    fn start_mcp_probe(&mut self, id: &str, reconnect: bool) {
        let Some(manager) = self.mcp_manager.clone() else {
            self.push_status("MCP manager unavailable");
            return;
        };
        self.tui
            .chrome_mut()
            .set_custom_working_label(Some(format!("Testing MCP server {id}...")));
        let id = id.to_owned();
        let probe_id = id.clone();
        let handle = tokio::spawn(async move {
            if reconnect {
                manager.reconnect_now(&probe_id).await
            } else {
                manager.refresh_tools(&probe_id).await
            }
        });
        self.pending_mcp_probe = Some(PendingMcpProbe {
            server_id: id,
            handle,
        });
    }

    async fn poll_pending_mcp_probe(&mut self) {
        let Some(pending) = self.pending_mcp_probe.take() else {
            return;
        };
        if !pending.handle.is_finished() {
            self.pending_mcp_probe = Some(pending);
            return;
        }
        self.tui.chrome_mut().set_custom_working_label(None);
        match pending.handle.await {
            Ok(Ok(snapshot)) => {
                self.push_status(format!(
                    "MCP {} connected ({} tools)",
                    pending.server_id, snapshot.tool_count
                ));
            }
            Ok(Err(err)) => {
                self.push_status(format!("MCP {} connect failed: {err}", pending.server_id));
            }
            Err(join_err) => {
                self.push_status(format!(
                    "MCP {} probe panicked: {join_err}",
                    pending.server_id
                ));
            }
        }
        // Refresh the MCP manager overlay to reflect the probe results.
        // Updates the existing overlay in-place rather than stacking a new one.
        self.open_mcp_manager().await;
    }

    async fn toggle_mcp_server_enabled(&mut self, id: &str) {
        let Some(config) = self.local_config.clone() else {
            return;
        };
        let Some(server) = config.mcp.servers.iter().find(|s| s.id == id) else {
            return;
        };
        let new_enabled = !server.enabled;
        let Some(config_path) = self.config_path() else {
            return;
        };
        if let Err(err) = config::set_mcp_server_enabled(id, new_enabled, &config_path) {
            self.push_status(format!("Failed to update MCP server: {err}"));
            return;
        }
        self.push_status(format!(
            "MCP server {id} {}",
            if new_enabled { "enabled" } else { "disabled" }
        ));
        self.refresh_config();
        self.sync_mcp_manager_from_config().await;
        // Refresh the MCP manager overlay to reflect the new enabled/disabled
        // state. Updates the existing overlay in-place rather than stacking.
        self.open_mcp_manager().await;
    }

    async fn delete_mcp_server(&mut self, id: &str) {
        let Some(config_path) = self.config_path() else {
            return;
        };
        if let Err(err) = config::remove_mcp_server(id, &config_path) {
            self.push_status(format!("Failed to remove MCP server: {err}"));
            return;
        }
        self.push_status(format!("MCP server {id} removed"));
        self.refresh_config();
        self.sync_mcp_manager_from_config().await;
        // Refresh the MCP manager overlay so the deleted server disappears.
        // Updates the existing overlay in-place rather than stacking.
        self.open_mcp_manager().await;
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
            if let Err(error) = crate::config_ops::remove_provider(&config_path, id) {
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

    fn fetch_known_catalog(&mut self) {
        self.tui
            .chrome_mut()
            .set_custom_working_label(Some("Fetching models.dev catalog...".to_owned()));
        let _handle = tokio::spawn(async move { neo_ai::catalog::fetch_catalog().await });
        let handle = tokio::spawn(async move { neo_ai::catalog::fetch_catalog().await });
        self.pending_catalog_fetch = Some(PendingCatalogFetch {
            source: CatalogFetchSource::Known,
            handle,
            pending_add: None,
        });
    }

    fn open_custom_registry_import(&mut self) {
        self.tui.chrome_mut().open_custom_registry_import(
            neo_tui::dialogs::CustomRegistryImportOptions {
                title: "Import Custom Registry".to_owned(),
            },
        );
    }

    fn open_catalog_api_key_input(&mut self, provider_id: &str) {
        self.pending_catalog_provider_id = Some(provider_id.to_owned());
        self.tui
            .chrome_mut()
            .open_api_key_input(neo_tui::dialogs::ApiKeyInputOptions {
                title: "API Key".to_owned(),
                provider_name: provider_id.to_owned(),
            });
    }

    fn import_custom_catalog_provider(&mut self, provider_id: &str) {
        let Some(pending) = self.pending_custom_registry.take() else {
            return;
        };
        let Some(entry) = pending.catalog.get(provider_id) else {
            self.push_status(format!(
                "Error: Provider '{provider_id}' not found in registry"
            ));
            return;
        };
        let Some(config_path) = self.config_path() else {
            self.push_status("No config available");
            return;
        };
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
                self.push_status(format!("Error: Failed to import provider: {error}"));
            }
        }
    }

    /// Handle an API key input result.
    fn handle_api_key_input_result(&mut self) {
        let Some(result) = self.tui.chrome_mut().api_key_input_result().cloned() else {
            return;
        };
        self.tui.chrome_mut().close_focused_overlay();
        match result {
            neo_tui::dialogs::ApiKeyInputResult::Submitted(key) => {
                self.handle_api_key_submitted(&key);
            }
            neo_tui::dialogs::ApiKeyInputResult::Cancelled => {
                self.pending_catalog_provider_id = None;
            }
        }
    }

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

    fn handle_api_key_submitted(&mut self, key: &str) {
        let Some(provider_id) = self.pending_catalog_provider_id.take() else {
            self.push_status("API key saved.");
            return;
        };
        let Some(config_path) = self.config_path() else {
            self.push_status("No config available");
            return;
        };
        // Fetch the catalog off the main loop so the footer spinner can animate
        // instead of freezing the UI for the duration of the network request.
        self.tui
            .chrome_mut()
            .set_custom_working_label(Some(format!("Importing provider {provider_id}...")));
        let _handle = tokio::spawn(async move { neo_ai::catalog::fetch_catalog().await });
        let handle = tokio::spawn(async move { neo_ai::catalog::fetch_catalog().await });
        self.pending_catalog_fetch = Some(PendingCatalogFetch {
            source: CatalogFetchSource::Known,
            handle,
            pending_add: Some(PendingCatalogAdd {
                provider_id,
                api_key: Some(key.to_owned()),
                config_path,
            }),
        });
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
                    pending_add: None,
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
                // API-key submit path: write the provider into config and report.
                if let Some(add) = pending.pending_add {
                    match catalog.get(&add.provider_id) {
                        Some(entry) => {
                            match crate::config_ops::add_provider_from_catalog_entry(
                                &add.config_path,
                                &add.provider_id,
                                entry,
                                add.api_key.as_deref(),
                                None,
                            ) {
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
                            self.push_status(format!(
                                "Error: provider '{}' not found in models.dev catalog",
                                add.provider_id
                            ));
                        }
                    }
                    return;
                }
                let items = catalog_choice_items(&catalog);
                if items.is_empty() {
                    self.push_status("No providers found in catalog.");
                    return;
                }
                self.open_catalog_fetch_result(pending.source, catalog, items);
            }
            Ok(Err(error)) => {
                self.push_status(format!("Error: Failed to fetch catalog: {error}"));
            }
            Err(join_error) => {
                self.push_status(format!("Error: Failed to fetch catalog: {join_error}"));
            }
        }
    }

    fn open_catalog_fetch_result(
        &mut self,
        source: CatalogFetchSource,
        catalog: CatalogEntries,
        items: Vec<neo_tui::dialogs::ChoiceItem>,
    ) {
        match source {
            CatalogFetchSource::Known => self.open_provider_choice_picker(items),
            CatalogFetchSource::Custom(source) => {
                self.pending_custom_registry = Some(PendingCustomRegistry { source, catalog });
                self.open_provider_choice_picker(custom_catalog_choice_items(items));
            }
        }
    }

    fn open_provider_choice_picker(&mut self, items: Vec<neo_tui::dialogs::ChoiceItem>) {
        let theme = self.tui.chrome().theme();
        self.tui
            .chrome_mut()
            .open_choice_picker(neo_tui::dialogs::ChoicePickerOptions {
                title: "Select a provider".to_owned(),
                items,
                initial_id: None,
                theme,
                page_size: 0,
                current_id: None,
            });
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
    project_trusted: bool,
) -> Result<Vec<PickerItem>> {
    let catalog = CompletionCatalog {
        slash_prompts: slash_prompt_template_completion_items(root, prefix, project_trusted)
            .unwrap_or_default(),
        prompt_packages: prompt_package_completion_items(root, project_trusted)?,
        extension_commands: extension_command_completion_items(root, project_trusted)?,
        session_commands: session_completion_items(skill_store),
        model_items: model_items.to_vec(),
    };
    Ok(completion_source_candidates(root, prefix, &catalog)?
        .into_iter()
        .map(|candidate| candidate.to_picker_item())
        .collect())
}

fn prompt_package_completion_items(root: &Path, project_trusted: bool) -> Result<Vec<PickerItem>> {
    let mut items = discover_prompt_template_commands(root, None, &[], project_trusted)?
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

fn extension_command_completion_items(
    root: &Path,
    project_trusted: bool,
) -> Result<Vec<PickerItem>> {
    let mut items = Vec::new();
    if project_trusted {
        let project_extension_root = root.join(".neo/extensions");
        if project_extension_root.exists() {
            items.extend(
                discover_extension_commands(&project_extension_root).with_context(|| {
                    format!(
                        "failed to discover project extensions under {}",
                        project_extension_root.display()
                    )
                })?,
            );
        }
    }
    if let Some(neo_home) = crate::config::neo_home() {
        let user_extension_root =
            neo_agent_core::tools::extensions::default_extension_root(&neo_home);
        if user_extension_root.exists() {
            items.extend(
                discover_extension_commands(&user_extension_root).with_context(|| {
                    format!(
                        "failed to discover user extensions under {}",
                        user_extension_root.display()
                    )
                })?,
            );
        }
    }
    items.sort_by(|left, right| left.value.cmp(&right.value));
    items.dedup_by(|left, right| left.value == right.value);
    items.truncate(100);
    Ok(items)
}

fn discover_extension_commands(extension_root: &Path) -> Result<Vec<PickerItem>> {
    Ok(
        neo_agent_core::tools::extensions::ExtensionDiscovery::new(extension_root)
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
            .collect::<Vec<_>>(),
    )
}

static STATIC_SLASH_COMMANDS: &[(&str, &str, Option<&str>, Option<&str>)] = &[
    (
        "/resume",
        "Resume a local session",
        Some("local sessions"),
        Some("local"),
    ),
    (
        "/new",
        "Start a fresh local session",
        Some("session"),
        Some("local"),
    ),
    ("/clear", "Alias for /new", Some("session"), Some("local")),
    (
        "/model",
        "Switch active model",
        Some("model picker"),
        Some("local"),
    ),
    (
        "/provider",
        "View configured providers",
        Some("provider picker"),
        Some("local"),
    ),
    (
        "/mcp",
        "View and manage MCP servers",
        Some("MCP manager"),
        Some("local"),
    ),
    (
        "/plan",
        "Toggle plan mode (on / off / clear)",
        Some("plan mode"),
        Some("local"),
    ),
    (
        "/compact",
        "Request manual context compaction",
        Some("session"),
        Some("local"),
    ),
    (
        "/permissions",
        "select permission mode",
        Some("permission mode"),
        Some("local"),
    ),
    (
        "/ask",
        "ask permission mode",
        Some("permission mode"),
        Some("local"),
    ),
    (
        "/auto",
        "auto permission mode",
        Some("permission mode"),
        Some("local"),
    ),
    (
        "/yolo",
        "yolo permission mode",
        Some("permission mode"),
        Some("local"),
    ),
    (
        "/btw",
        "Open a temporary side-question panel",
        Some("sidecar dialog"),
        Some("local"),
    ),
];

fn session_completion_items(skill_store: Option<&SkillStore>) -> Vec<PickerItem> {
    let mut items: Vec<PickerItem> = STATIC_SLASH_COMMANDS
        .iter()
        .map(|(value, description, provider, trust)| {
            PickerItem::new(
                (*value).to_owned(),
                (*value).to_owned(),
                Some(prompt_source_description(
                    Some(description),
                    *provider,
                    *trust,
                )),
            )
        })
        .collect();
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
    project_trusted: bool,
) -> Option<Vec<PickerItem>> {
    let name_prefix = prefix.strip_prefix('/')?;
    if name_prefix.contains('/') {
        return None;
    }

    let project_prompts_dir = root.join(".neo/prompts");
    let mut completions = load_project_prompt_templates(root, project_trusted)
        .into_iter()
        .filter(|template| {
            template
                .path
                .strip_prefix(&project_prompts_dir)
                .is_ok_and(|relative| {
                    relative
                        .parent()
                        .is_none_or(|parent| parent.as_os_str().is_empty())
                })
        })
        .filter(|template| template.name.starts_with(name_prefix))
        .map(|template| {
            let value = format!("/{}", template.name);
            let description = (!template.description.is_empty()).then_some(template.description);
            PickerItem::new(value.clone(), value, description)
        })
        .collect::<Vec<_>>();
    completions.sort_by(|left, right| left.value.cmp(&right.value));
    completions.truncate(100);
    Some(completions)
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
#[repr(usize)]
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

fn completion_source_rank(source: CompletionSource) -> u8 {
    [0, 1, 2, 3, 4, 5][source as usize]
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
    let mut errors = Vec::new();
    for (program, args) in clipboard_commands() {
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

fn clipboard_commands() -> &'static [(&'static str, &'static [&'static str])] {
    if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else if cfg!(target_os = "windows") {
        &[("clip.exe", &[])]
    } else {
        &[("wl-copy", &[]), ("xclip", &["-selection", "clipboard"])]
    }
}

fn write_clipboard_command(program: &str, args: &[&str], text: &str) -> Result<()> {
    let mut child = spawn_clipboard_command(program, args)?;
    write_clipboard_stdin(&mut child, program, text)?;
    let output = wait_clipboard_command(child, program)?;
    if output.status.success() {
        return Ok(());
    }
    Err(clipboard_exit_error(&output))
}

fn spawn_clipboard_command(program: &str, args: &[&str]) -> Result<std::process::Child> {
    Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start {program}"))
}

fn write_clipboard_stdin(child: &mut std::process::Child, program: &str, text: &str) -> Result<()> {
    child
        .stdin
        .as_mut()
        .context("clipboard command stdin was unavailable")?
        .write_all(text.as_bytes())
        .with_context(|| format!("failed to write to {program}"))
}

fn wait_clipboard_command(
    child: std::process::Child,
    program: &str,
) -> Result<std::process::Output> {
    child
        .wait_with_output()
        .with_context(|| format!("failed to wait for {program}"))
}

fn clipboard_exit_error(output: &std::process::Output) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let suffix = if stderr.is_empty() {
        String::new()
    } else {
        format!(": {stderr}")
    };
    anyhow::anyhow!("exited with {}{}", output.status, suffix)
}

fn command_specs(project_dir: &Path, project_trusted: bool) -> (Vec<CommandSpec>, Option<String>) {
    let mut commands = vec![
        CommandSpec::new("sessions", "Open sessions", Some("Browse local sessions")),
        CommandSpec::new("models", "Open models", Some("Switch active model")),
        CommandSpec::new(
            "providers",
            "Open providers",
            Some("View configured providers"),
        ),
        CommandSpec::new("mcp", "Open MCP servers", Some("Manage MCP servers")),
        CommandSpec::new(
            "session.new",
            "New session",
            Some("Start a fresh local session"),
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
            "permissions",
            "Open permissions",
            Some("Select permission mode"),
        ),
        CommandSpec::new(
            "permission.ask",
            "Ask permission mode",
            Some("Ask before risky actions"),
        ),
        CommandSpec::new(
            "permission.auto",
            "Auto permission mode",
            Some("Run non-interactively"),
        ),
        CommandSpec::new(
            "permission.yolo",
            "YOLO permission mode",
            Some("Skip confirmations"),
        ),
        CommandSpec::new(
            "plan",
            "Toggle plan mode",
            Some("Read-only mode for investigation and planning"),
        ),
        CommandSpec::new(
            "btw",
            "/btw sidecar",
            Some("Open a temporary side-question panel"),
        ),
    ];
    let mut templates = load_project_prompt_templates(project_dir, project_trusted);
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

const EDITING_ACTION_PRIORITY: &[KeybindingAction] = &[
    KeybindingAction::PasteImage,
    KeybindingAction::InputSubmit,
    KeybindingAction::InputNewLine,
    KeybindingAction::PromptSteer,
    KeybindingAction::CycleDevelopmentMode,
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
    controller.prompt_history = Some(crate::prompt_history::PromptHistoryStore::for_config(
        &config,
    ));
    controller.load_prompt_history();
    controller.trust_store = crate::trust::ProjectTrustStore::from_home().ok();
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
    if !config.models.is_empty() {
        return ModelPickerCatalog {
            items: model_picker_items_from_config(config),
            error: None,
        };
    }
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

fn model_picker_items_from_config(config: &AppConfig) -> Vec<PickerItem> {
    config
        .models
        .iter()
        .map(|(alias, model)| {
            let description = model.max_context_tokens.map_or_else(
                || model.provider.clone(),
                |max_context_tokens| format!("{} · ctx {max_context_tokens}", model.provider),
            );
            PickerItem::new(alias.clone(), alias.clone(), Some(description))
        })
        .collect()
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
                let mut capabilities = model.capabilities.clone();
                if capabilities.iter().any(|c| c == "reasoning")
                    && !capabilities.iter().any(|c| c == "thinking")
                {
                    capabilities.push("thinking".to_owned());
                }
                neo_tui::dialogs::ModelEntry {
                    alias: alias.clone(),
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

fn catalog_choice_items(catalog: &CatalogEntries) -> Vec<neo_tui::dialogs::ChoiceItem> {
    catalog
        .iter()
        .map(|(id, entry)| {
            let label = entry.name.clone().unwrap_or_else(|| id.clone());
            let description = entry.api.clone().unwrap_or_default();
            neo_tui::dialogs::ChoiceItem::new(format!("catalog:{id}"), label)
                .with_description(description)
        })
        .collect()
}

fn custom_catalog_choice_items(
    items: Vec<neo_tui::dialogs::ChoiceItem>,
) -> Vec<neo_tui::dialogs::ChoiceItem> {
    items
        .into_iter()
        .map(|mut item| {
            item.id = item.id.replacen("catalog:", "custom-catalog:", 1);
            item
        })
        .collect()
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
    let mut lines = render_overlay_content_snapshot(app, content_width);
    lines.extend(render_chrome_snapshot_lines(app, width));
    lines
}

fn render_overlay_content_snapshot(app: &NeoChromeState, content_width: usize) -> Vec<String> {
    match app.focused_overlay().map(|overlay| &overlay.kind) {
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
    }
}

fn render_chrome_snapshot_lines(
    app: &NeoChromeState,
    width: usize,
) -> impl Iterator<Item = String> {
    neo_tui::transcript::render_chrome_lines(app, width, 24)
        .lines
        .into_iter()
        .map(|line| neo_tui::ansi::strip_ansi(&line).trim_end().to_owned())
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
    use neo_agent_core::{AgentEvent, AgentMessage, Content, PermissionMode, StopReason};
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

    fn pending_approval_response(
        decision_tx: oneshot::Sender<PermissionApprovalDecision>,
    ) -> PendingApprovalResponse {
        PendingApprovalResponse {
            decision_tx,
            feedback_tx: None,
            selected_label_tx: None,
            session_option_label: None,
            prefix_option_label: None,
        }
    }

    #[test]
    fn git_status_badge_formats_branch_diff_and_sync() {
        let mut badge = parse_git_status_porcelain(
            "## main...origin/main [ahead 2, behind 1]\n M src/app.rs\n",
        )
        .expect("git badge");
        let (added, deleted) = parse_git_numstat("12\t3\tsrc/app.rs\n-\t-\tassets/image.png\n");
        badge.added = added;
        badge.deleted = deleted;

        assert_eq!(badge.format(), "main [+12 -3 ↑2↓1]");
    }

    #[test]
    fn git_status_badge_formats_dirty_without_line_counts() {
        let badge = parse_git_status_porcelain("## feature\n?? new-file.rs\n").expect("git badge");

        assert_eq!(badge.format(), "feature [±]");
    }

    #[test]
    fn git_status_badge_is_absent_when_git_program_is_missing() {
        let missing = git_status_label_with_program(
            "definitely-not-a-real-git-binary-for-neo-tests",
            &test_workspace_root(),
        );

        assert_eq!(missing, None);
    }

    #[test]
    fn refresh_git_status_now_updates_after_write_tool_finished() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.set_git_status_provider(Arc::new(|_| Some("main [+2 -1]".to_owned())));
        controller
            .tui
            .chrome_mut()
            .set_git_status_label(Some("main [+1 -1]".to_owned()));

        controller.apply_turn_event(AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "tool-1".to_owned(),
            name: "Write".to_owned(),
            result: ToolResult::ok("wrote file"),
        });

        assert_eq!(controller.chrome().git_status_label(), Some("main [+2 -1]"));
    }

    #[test]
    fn refresh_git_status_now_updates_after_edit_tool_finished() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.set_git_status_provider(Arc::new(|_| Some("main [+3 -2]".to_owned())));
        controller
            .tui
            .chrome_mut()
            .set_git_status_label(Some("main [+1 -1]".to_owned()));

        controller.apply_turn_event(AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "tool-1".to_owned(),
            name: "Edit".to_owned(),
            result: ToolResult::ok("edited file"),
        });

        assert_eq!(controller.chrome().git_status_label(), Some("main [+3 -2]"));
    }

    #[test]
    fn refresh_git_status_now_updates_after_shell_and_terminal_finished() {
        let statuses = Arc::new(std::sync::Mutex::new(VecDeque::from([
            Some("main [↑1]".to_owned()),
            Some("main".to_owned()),
        ])));
        let provider_statuses = Arc::clone(&statuses);
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.set_git_status_provider(Arc::new(move |_| {
            provider_statuses
                .lock()
                .expect("status queue lock")
                .pop_front()
                .flatten()
        }));
        controller
            .tui
            .chrome_mut()
            .set_git_status_label(Some("main [+1 -1]".to_owned()));

        controller.apply_turn_event(AgentEvent::ShellCommandFinished {
            turn: 1,
            id: "shell-1".to_owned(),
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            truncated: false,
        });
        assert_eq!(controller.chrome().git_status_label(), Some("main [↑1]"));

        controller.apply_turn_event(AgentEvent::TerminalSessionFinished {
            turn: 1,
            id: "terminal-1".to_owned(),
            handle: "terminal".to_owned(),
            status: "exited".to_owned(),
            exit_code: Some(0),
        });
        assert_eq!(controller.chrome().git_status_label(), Some("main"));
    }

    #[test]
    fn refresh_git_status_if_due_uses_30s_interval() {
        let refresh_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider_refresh_count = Arc::clone(&refresh_count);
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.set_git_status_provider(Arc::new(move |_| {
            let count =
                provider_refresh_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            Some(format!("main [refresh-{count}]"))
        }));
        controller
            .tui
            .chrome_mut()
            .set_git_status_label(Some("main".to_owned()));

        controller.set_last_git_status_refresh(Some(
            Instant::now()
                .checked_sub(Duration::from_secs(29))
                .expect("instant before now"),
        ));
        controller.refresh_git_status_if_due();
        assert_eq!(refresh_count.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(controller.chrome().git_status_label(), Some("main"));

        controller.set_last_git_status_refresh(Some(
            Instant::now()
                .checked_sub(Duration::from_secs(30))
                .expect("instant before now"),
        ));
        controller.refresh_git_status_if_due();
        assert_eq!(refresh_count.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            controller.chrome().git_status_label(),
            Some("main [refresh-1]")
        );
    }

    #[test]
    fn refresh_git_status_now_clears_badge_when_git_unavailable() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.set_git_status_provider(Arc::new(|_| None));
        controller
            .tui
            .chrome_mut()
            .set_git_status_label(Some("main [+1 -1]".to_owned()));

        controller.refresh_git_status_now();

        assert_eq!(controller.chrome().git_status_label(), None);
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
        assert!(
            requests[0].prompt[0]
                .as_text()
                .unwrap()
                .contains("Background question `question-1`")
        );
        assert!(
            requests[0].prompt[0]
                .as_text()
                .unwrap()
                .contains("TaskOutput")
        );
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
                assert_eq!(request.prompt, vec![Content::text("hello neo")]);
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
                assert_eq!(request.prompt, vec![Content::text("hi")]);
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
                assert_eq!(request.prompt, vec![Content::text("alpha\nbeta")]);
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
    async fn slash_completion_includes_btw_command() {
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

        let rendered = controller.chrome().focused_overlay_lines(80).join("\n");
        assert!(
            rendered.contains("/btw"),
            "slash completion should include /btw; got:\n{rendered}"
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
            prompt_completions(temp.path(), "/", &[], None, true).expect("slash completions");
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
            prompt_completions(temp.path(), "/rev", &[], None, true).expect("prompt completions");

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
            prompt_completions(temp.path(), "/", &[], None, true).expect("slash completions");
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
        assert_eq!(requests[0].prompt, vec![Content::text("explain this file")]);
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
            vec![Content::text("@src/main.rs explain this file")]
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
            vec![Content::text("@anthropic/claude-sonnet")]
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
            Some(ApprovalChoice::Deny)
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
        assert_eq!(
            requests[0].prompt,
            vec![Content::text("Review src/lib.rs.")]
        );
    }

    #[tokio::test]
    async fn command_palette_exports_active_session_to_html() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = temp.path().join(".neo/sessions");
        let config = test_config(temp.path(), sessions_dir.clone());
        let bucket_dir = workspace_sessions_dir(&config);
        fs::create_dir_all(&bucket_dir).expect("create sessions bucket dir");
        fs::create_dir_all(bucket_dir.join(SESSION_A)).expect("create session dir");
        fs::write(
            bucket_dir.join(SESSION_A).join("transcript.jsonl"),
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

        let export_path = bucket_dir.join(SESSION_A).join("transcript.html");
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
                    session_scope: None,
                    prefix_rule: None,
                });
                let (decision_tx, decision_rx) = oneshot::channel();
                channels
                    .approvals
                    .send(crate::modes::run::PromptApprovalRequest {
                        id: "tool-1".to_owned(),
                        operation: neo_agent_core::PermissionOperation::Tool,
                        decision_tx,
                        feedback_tx: None,
                        selected_label_tx: None,
                        session_option_label: None,
                        prefix_option_label: None,
                        prefix_rule: None,
                        session_scope: None,
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
            vec![PermissionApprovalDecision::AllowOnce]
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
            session_scope: Some(neo_agent_core::SessionApprovalScope {
                keys: vec![neo_agent_core::SessionApprovalKey::FileWrite {
                    workspace: test_workspace_root().display().to_string(),
                    path: test_workspace_root()
                        .join("approved.txt")
                        .display()
                        .to_string(),
                    operation: neo_agent_core::FileWriteApprovalOperation::Write,
                }],
                label: "Approve writes to this file for this session".to_owned(),
                detail: "approved.txt".to_owned(),
            }),
            prefix_rule: None,
        });
        let (decision_tx, decision_rx) = oneshot::channel();
        controller.pending_approvals.insert(
            "tool-1".to_owned(),
            PendingApprovalResponse {
                decision_tx,
                feedback_tx: None,
                selected_label_tx: None,
                session_option_label: Some(
                    "Approve writes to this file for this session".to_owned(),
                ),
                prefix_option_label: None,
            },
        );

        controller
            .handle_input_event(InputEvent::Insert('2'))
            .await
            .expect("number shortcut handles approval");

        assert_eq!(
            decision_rx.await.expect("approval decision"),
            PermissionApprovalDecision::AllowForSession
        );
        assert!(controller.chrome().focused_overlay().is_none());
        assert!(
            controller
                .render_snapshot()
                .contains("Approved writes to this file for this session")
        );
    }

    #[tokio::test]
    async fn prefix_approval_choice_dispatches_prefix_decision() {
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
            subject: "cargo test".to_owned(),
            arguments: serde_json::json!({"command": "cargo test"}),
            session_scope: Some(neo_agent_core::SessionApprovalScope {
                keys: vec![neo_agent_core::SessionApprovalKey::Shell {
                    workspace: test_workspace_root().display().to_string(),
                    cwd: test_workspace_root().display().to_string(),
                    command: vec!["cargo".to_owned(), "test".to_owned()],
                }],
                label: "Approve this exact command for this session".to_owned(),
                detail: test_workspace_root().display().to_string(),
            }),
            prefix_rule: Some(neo_agent_core::PrefixApprovalRule {
                prefix: vec!["cargo".to_owned(), "test".to_owned()],
                label: "cargo test".to_owned(),
            }),
        });
        let (decision_tx, decision_rx) = oneshot::channel();
        controller.pending_approvals.insert(
            "tool-1".to_owned(),
            PendingApprovalResponse {
                decision_tx,
                feedback_tx: None,
                selected_label_tx: None,
                session_option_label: Some(
                    "Approve this exact command for this session".to_owned(),
                ),
                prefix_option_label: Some("Approve commands starting with cargo test".to_owned()),
            },
        );

        controller
            .handle_input_event(InputEvent::Insert('3'))
            .await
            .expect("number shortcut handles prefix approval");

        assert_eq!(
            decision_rx.await.expect("approval decision"),
            PermissionApprovalDecision::AllowForPrefix
        );
        assert!(
            controller
                .render_snapshot()
                .contains("Approved commands starting with cargo test")
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
            session_scope: Some(neo_agent_core::SessionApprovalScope {
                keys: vec![neo_agent_core::SessionApprovalKey::FileWrite {
                    workspace: test_workspace_root().display().to_string(),
                    path: test_workspace_root()
                        .join("approved.txt")
                        .display()
                        .to_string(),
                    operation: neo_agent_core::FileWriteApprovalOperation::Write,
                }],
                label: "Approve writes to this file for this session".to_owned(),
                detail: "approved.txt".to_owned(),
            }),
            prefix_rule: None,
        });
        let (decision_tx, decision_rx) = oneshot::channel();
        controller
            .pending_approvals
            .insert("tool-1".to_owned(), pending_approval_response(decision_tx));

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
            PermissionApprovalDecision::AllowForSession
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
            session_scope: None,
            prefix_rule: None,
        });
        let (decision_tx, decision_rx) = oneshot::channel();
        controller
            .pending_approvals
            .insert("tool-1".to_owned(), pending_approval_response(decision_tx));

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
            .await
            .expect("down selects deny option");
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
            PermissionApprovalDecision::Reject
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
            session_scope: None,
            prefix_rule: None,
        });
        let (decision_tx, decision_rx) = oneshot::channel();
        controller
            .pending_approvals
            .insert("tool-1".to_owned(), pending_approval_response(decision_tx));

        controller
            .handle_input_event(InputEvent::Cancel)
            .await
            .expect("cancel rejects approval");

        assert_eq!(
            decision_rx.await.expect("approval decision"),
            PermissionApprovalDecision::Reject
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
            session_scope: Some(neo_agent_core::SessionApprovalScope {
                keys: vec![neo_agent_core::SessionApprovalKey::Shell {
                    workspace: test_workspace_root().display().to_string(),
                    cwd: test_workspace_root().display().to_string(),
                    command: vec!["printf".to_owned(), "one".to_owned()],
                }],
                label: "Approve this exact command for this session".to_owned(),
                detail: test_workspace_root().display().to_string(),
            }),
            prefix_rule: None,
        });
        controller.apply_turn_event(AgentEvent::ApprovalRequested {
            turn: 1,
            id: "tool-2".to_owned(),
            operation: neo_agent_core::PermissionOperation::Shell,
            subject: "printf two".to_owned(),
            arguments: serde_json::json!({"command": "printf two"}),
            session_scope: None,
            prefix_rule: None,
        });
        let (first_tx, first_rx) = oneshot::channel();
        let (second_tx, _second_rx) = oneshot::channel();
        controller
            .pending_approvals
            .insert("tool-1".to_owned(), pending_approval_response(first_tx));
        controller
            .pending_approvals
            .insert("tool-2".to_owned(), pending_approval_response(second_tx));

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("first approval confirms");

        assert_eq!(
            first_rx.await.expect("first decision"),
            PermissionApprovalDecision::AllowOnce
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
            session_scope: Some(neo_agent_core::SessionApprovalScope {
                keys: vec![neo_agent_core::SessionApprovalKey::Shell {
                    workspace: test_workspace_root().display().to_string(),
                    cwd: test_workspace_root().display().to_string(),
                    command: vec!["printf".to_owned(), "one".to_owned()],
                }],
                label: "Approve this exact command for this session".to_owned(),
                detail: test_workspace_root().display().to_string(),
            }),
            prefix_rule: None,
        });
        controller.apply_turn_event(AgentEvent::ApprovalRequested {
            turn: 1,
            id: "tool-2".to_owned(),
            operation: neo_agent_core::PermissionOperation::Shell,
            subject: "printf two".to_owned(),
            arguments: serde_json::json!({"command": "printf two"}),
            session_scope: None,
            prefix_rule: None,
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
            session_scope: Some(neo_agent_core::SessionApprovalScope {
                keys: vec![neo_agent_core::SessionApprovalKey::Shell {
                    workspace: test_workspace_root().display().to_string(),
                    cwd: test_workspace_root().display().to_string(),
                    command: vec!["printf".to_owned(), "one".to_owned()],
                }],
                label: "Approve this exact command for this session".to_owned(),
                detail: test_workspace_root().display().to_string(),
            }),
            prefix_rule: None,
        });
        controller.apply_turn_event(AgentEvent::ApprovalRequested {
            turn: 1,
            id: "tool-2".to_owned(),
            operation: neo_agent_core::PermissionOperation::Shell,
            subject: "printf two".to_owned(),
            arguments: serde_json::json!({"command": "printf two"}),
            session_scope: None,
            prefix_rule: None,
        });
        let (first_tx, first_rx) = oneshot::channel();
        let (second_tx, _second_rx) = oneshot::channel();
        controller
            .pending_approvals
            .insert("tool-1".to_owned(), pending_approval_response(first_tx));
        controller
            .pending_approvals
            .insert("tool-2".to_owned(), pending_approval_response(second_tx));

        controller
            .handle_input_event(InputEvent::Cancel)
            .await
            .expect("cancel rejects current approval");

        assert_eq!(
            first_rx.await.expect("first decision"),
            PermissionApprovalDecision::Reject
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
            session_scope: None,
            prefix_rule: None,
        });
        controller.apply_turn_event(AgentEvent::ApprovalRequested {
            turn: 1,
            id: "tool-2".to_owned(),
            operation: neo_agent_core::PermissionOperation::Shell,
            subject: "printf two".to_owned(),
            arguments: serde_json::json!({"command": "printf two"}),
            session_scope: None,
            prefix_rule: None,
        });
        let (first_tx, first_rx) = oneshot::channel();
        let (second_tx, second_rx) = oneshot::channel();
        controller
            .pending_approvals
            .insert("tool-1".to_owned(), pending_approval_response(first_tx));
        controller
            .pending_approvals
            .insert("tool-2".to_owned(), pending_approval_response(second_tx));

        controller
            .handle_input_event(InputEvent::Interrupt)
            .await
            .expect("interrupt rejects pending approvals");

        assert_eq!(
            first_rx.await.expect("first decision"),
            PermissionApprovalDecision::Reject
        );
        assert_eq!(
            second_rx.await.expect("second decision"),
            PermissionApprovalDecision::Reject
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
            session_scope: None,
            prefix_rule: None,
        });

        controller
            .handle_input_event(InputEvent::Interrupt)
            .await
            .expect("interrupt rejects visible approval");
        let (decision_tx, decision_rx) = oneshot::channel();
        controller.register_pending_approval(crate::modes::run::PromptApprovalRequest {
            id: "tool-1".to_owned(),
            operation: neo_agent_core::PermissionOperation::Tool,
            decision_tx,
            feedback_tx: None,
            selected_label_tx: None,
            session_option_label: None,
            prefix_option_label: None,
            prefix_rule: None,
            session_scope: None,
        });

        assert_eq!(
            decision_rx.await.expect("late approval decision"),
            PermissionApprovalDecision::Reject
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

    #[test]
    fn rebuild_transcript_from_session_initializes_context_window_usage() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "new",
            "deepseek/deepseek-v4-pro",
            test_workspace_root(),
            |_| async { Ok(Vec::new()) },
        );
        controller
            .tui
            .chrome_mut()
            .set_context_window(Some(ContextWindow::new(1_000_000)));

        let loaded =
            LoadedSessionTranscript::new("alpha", Vec::new(), [AgentMessage::user_text("hello")])
                .with_estimated_context_tokens(393);

        controller.rebuild_transcript_from_session(&loaded);

        assert_eq!(
            controller.chrome().context_window(),
            Some(ContextWindow::new(1_000_000).with_used_tokens(393))
        );
    }

    #[tokio::test]
    async fn load_session_transcript_estimates_context_usage_for_replayed_session() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = temp.path().join(".neo/sessions");
        let config = test_config(temp.path(), sessions_dir);
        let bucket_dir = workspace_sessions_dir(&config);
        fs::create_dir_all(&bucket_dir).expect("create sessions bucket dir");
        fs::create_dir_all(bucket_dir.join(SESSION_A)).expect("create session dir");
        let session_path = bucket_dir.join(SESSION_A).join("transcript.jsonl");
        let mut writer = neo_agent_core::session::JsonlSessionWriter::create(&session_path)
            .await
            .expect("create session");
        writer
            .append(&AgentEvent::MessageAppended {
                message: AgentMessage::user_text("remember this"),
            })
            .await
            .expect("append user message");
        writer.flush().await.expect("flush session");

        let loaded = load_session_transcript(SESSION_A.to_owned(), &config)
            .await
            .expect("load transcript");

        assert_eq!(loaded.estimated_context_tokens, Some(4));
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
        assert_eq!(requests[0].prompt, vec![Content::text("continue")]);
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
        assert_eq!(requests[0].prompt, vec![Content::text("read project")]);
        assert_eq!(requests[0].session_id, None);
        assert_eq!(requests[1].prompt, vec![Content::text("continue")]);
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
        assert_eq!(requests[0].prompt, vec![Content::text("read project")]);
        assert_eq!(requests[0].session_id, None);
        assert_eq!(requests[1].prompt, vec![Content::text("continue")]);
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
        assert_eq!(requests[0].prompt, vec![Content::text("continue fork")]);
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
            skill_store.get("define-goal").is_none(),
            "builtin define-goal skill should not be loaded"
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
        fs::create_dir_all(bucket_dir.join(SESSION_A)).expect("create session dir");
        fs::write(
            bucket_dir.join(SESSION_A).join("transcript.jsonl"),
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
        fs::create_dir_all(bucket_dir.join(SESSION_A)).expect("create session dir");
        fs::write(
            bucket_dir.join(SESSION_A).join("transcript.jsonl"),
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
                .join(&forked.session_id)
                .join("transcript.jsonl")
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
        fs::create_dir_all(bucket_a.join(SESSION_A)).expect("create session_a dir");
        fs::write(
            bucket_a.join(SESSION_A).join("transcript.jsonl"),
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
        fs::create_dir_all(bucket_b.join(SESSION_B)).expect("create session_b dir");
        fs::write(
            bucket_b.join(SESSION_B).join("transcript.jsonl"),
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

    #[tokio::test]
    async fn slash_ask_sets_ask_permission_mode() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.type_text("/ask");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("slash command handled");
        assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
        assert!(transcript_has_status(&controller, "Permission Mode: ask"));
        assert!(controller.render_snapshot().contains("[ask]"));
    }

    #[tokio::test]
    async fn slash_auto_sets_auto_permission_mode() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.type_text("/auto");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("slash command handled");
        assert_eq!(controller.chrome().permission_mode(), PermissionMode::Auto);
        assert!(transcript_has_status(&controller, "Permission Mode: auto"));
        assert!(controller.render_snapshot().contains("[auto]"));
    }

    #[tokio::test]
    async fn slash_yolo_sets_yolo_permission_mode() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.type_text("/yolo");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("slash command handled");
        assert_eq!(controller.chrome().permission_mode(), PermissionMode::Yolo);
        assert!(transcript_has_status(&controller, "Permission Mode: yolo"));
        assert!(controller.render_snapshot().contains("[yolo]"));
    }

    #[tokio::test]
    async fn permissions_picker_selects_auto_mode() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.type_text("/permissions");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("opens permission picker");
        assert!(controller.chrome().focused_overlay().is_some());

        // Move from Ask (index 0) to Auto (index 1) and confirm.
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move selection");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("confirm selection");

        assert_eq!(controller.chrome().permission_mode(), PermissionMode::Auto);
        assert!(transcript_has_status(&controller, "Permission Mode: auto"));
        assert!(controller.chrome().focused_overlay().is_none());
    }

    #[test]
    fn slash_completions_include_permission_commands() {
        let completions = prompt_completions(&test_workspace_root(), "/", &[], None, true)
            .expect("completions resolve");
        let values: Vec<_> = completions.iter().map(|item| item.value.as_str()).collect();
        assert!(
            values.contains(&"/permissions"),
            "missing /permissions: {values:?}"
        );
        assert!(values.contains(&"/ask"), "missing /ask: {values:?}");
        assert!(values.contains(&"/auto"), "missing /auto: {values:?}");
        assert!(values.contains(&"/yolo"), "missing /yolo: {values:?}");
    }

    #[test]
    fn slash_completions_include_compact_command() {
        let completions = prompt_completions(&test_workspace_root(), "/", &[], None, true)
            .expect("completions resolve");
        let values: Vec<_> = completions.iter().map(|item| item.value.as_str()).collect();
        assert!(values.contains(&"/compact"), "missing /compact: {values:?}");
    }

    #[tokio::test]
    async fn slash_plan_toggles_plan_mode_and_footer() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.type_text("/plan");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("toggles plan mode on");
        assert!(controller.chrome().is_plan_mode());
        assert!(transcript_has_status(&controller, "Plan Mode On"));
        assert!(controller.render_snapshot().contains("[plan]"));
        assert!(!controller.render_snapshot().contains("[PLAN MODE]"));

        controller.type_text("/plan");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("toggles plan mode off");
        assert!(!controller.chrome().is_plan_mode());
        assert!(transcript_has_status(&controller, "Plan Mode Off"));
    }

    #[tokio::test]
    async fn shift_tab_cycles_development_mode_without_changing_permission_mode() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
        assert_eq!(
            controller.chrome().development_mode(),
            DevelopmentMode::Normal
        );

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::CycleDevelopmentMode))
            .await
            .expect("cycle to plan");
        assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
        assert_eq!(
            controller.chrome().development_mode(),
            DevelopmentMode::Plan
        );
        assert!(transcript_has_status(&controller, "Plan Mode On"));

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::CycleDevelopmentMode))
            .await
            .expect("cycle to goal");
        assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
        assert_eq!(
            controller.chrome().development_mode(),
            DevelopmentMode::Goal(GoalModeStatus::Pending)
        );
        assert!(transcript_has_status(&controller, "Goal Mode On"));

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::CycleDevelopmentMode))
            .await
            .expect("cycle to normal");
        assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
        assert_eq!(
            controller.chrome().development_mode(),
            DevelopmentMode::Normal
        );
        assert!(transcript_has_status(&controller, "Goal Mode Off"));
    }

    #[tokio::test]
    async fn shift_tab_key_uses_development_cycle() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("shift+tab").expect("valid key")))
            .await
            .expect("shift tab cycles");
        assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
        assert_eq!(
            controller.chrome().development_mode(),
            DevelopmentMode::Plan
        );
    }

    #[tokio::test]
    async fn slash_plan_turn_request_uses_runtime_plan_mode() {
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
                    let active = request
                        .plan_mode
                        .read()
                        .expect("plan mode lock")
                        .is_active();
                    captured_requests.lock().expect("lock").push(active);
                    Ok(Vec::<AgentEvent>::new())
                }
            },
        );

        controller.type_text("/plan on");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("plan on");
        controller.type_text("plan this");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("submit turn");
        controller
            .wait_for_active_turn()
            .await
            .expect("turn completes");

        assert_eq!(*requests.lock().expect("lock"), vec![true]);
    }

    #[tokio::test]
    async fn goal_development_mode_sets_turn_authoring_flag() {
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
                        .expect("lock")
                        .push(request.goal_mode_authoring);
                    Ok(Vec::<AgentEvent>::new())
                }
            },
        );

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::CycleDevelopmentMode))
            .await
            .expect("cycle to plan");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::CycleDevelopmentMode))
            .await
            .expect("cycle to goal");
        controller.type_text("draft a goal");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("submit goal-mode turn");
        controller
            .wait_for_active_turn()
            .await
            .expect("turn completes");

        assert_eq!(*requests.lock().expect("lock"), vec![true]);
    }

    #[tokio::test]
    async fn revise_exit_plan_mode_feedback_is_forwarded_with_current_approval() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.apply_turn_event(AgentEvent::ApprovalRequested {
            turn: 1,
            id: "exit-plan-1".to_owned(),
            operation: neo_agent_core::PermissionOperation::PlanTransition,
            subject: "Exit plan mode".to_owned(),
            arguments: serde_json::json!({}),
            session_scope: None,
            prefix_rule: None,
        });
        let (decision_tx, decision_rx) = oneshot::channel();
        let (feedback_tx, feedback_rx) = oneshot::channel();
        controller.register_pending_approval(crate::modes::run::PromptApprovalRequest {
            id: "exit-plan-1".to_owned(),
            operation: neo_agent_core::PermissionOperation::PlanTransition,
            decision_tx,
            feedback_tx: Some(feedback_tx),
            selected_label_tx: None,
            session_option_label: None,
            prefix_option_label: None,
            prefix_rule: None,
            session_scope: None,
        });

        // Select "Revise" (index 2) and enter feedback, then confirm.
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move to revise");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move to revise");
        controller
            .handle_input_event(InputEvent::Insert('r'))
            .await
            .expect("type feedback");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("confirm revise");

        assert!(transcript_has_status(&controller, "Revision feedback: r"));
        assert_eq!(
            decision_rx.await.expect("decision"),
            PermissionApprovalDecision::Reject
        );
        assert_eq!(feedback_rx.await.expect("feedback"), Some("r".to_owned()));
    }

    #[tokio::test]
    async fn approve_for_session_does_not_globally_skip_later_ask_prompt() {
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
            session_scope: Some(neo_agent_core::SessionApprovalScope {
                keys: vec![neo_agent_core::SessionApprovalKey::Shell {
                    workspace: test_workspace_root().display().to_string(),
                    cwd: test_workspace_root().display().to_string(),
                    command: vec!["printf".to_owned(), "one".to_owned()],
                }],
                label: "Approve this exact command for this session".to_owned(),
                detail: test_workspace_root().display().to_string(),
            }),
            prefix_rule: None,
        });
        let (first_tx, first_rx) = oneshot::channel();
        controller.pending_approvals.insert(
            "tool-1".to_owned(),
            PendingApprovalResponse {
                decision_tx: first_tx,
                feedback_tx: None,
                selected_label_tx: None,
                session_option_label: Some(
                    "Approve this exact command for this session".to_owned(),
                ),
                prefix_option_label: None,
            },
        );

        // Select "Approve for this session" (index 1) and confirm.
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move to always-approve");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("confirm always-approve");

        assert_eq!(
            first_rx.await.expect("first decision"),
            PermissionApprovalDecision::AllowForSession
        );

        // Tool-session approval is scoped by the runtime. The TUI must not
        // turn one approval into a global bypass for later ask prompts.
        controller.apply_turn_event(AgentEvent::ApprovalRequested {
            turn: 1,
            id: "tool-2".to_owned(),
            operation: neo_agent_core::PermissionOperation::Tool,
            subject: "Write".to_owned(),
            arguments: serde_json::json!({"path": "later.txt"}),
            session_scope: None,
            prefix_rule: None,
        });
        let (second_tx, mut second_rx) = oneshot::channel();
        controller.register_pending_approval(crate::modes::run::PromptApprovalRequest {
            id: "tool-2".to_owned(),
            operation: neo_agent_core::PermissionOperation::Tool,
            decision_tx: second_tx,
            feedback_tx: None,
            selected_label_tx: None,
            session_option_label: None,
            prefix_option_label: None,
            prefix_rule: None,
            session_scope: None,
        });
        assert!(
            second_rx.try_recv().is_err(),
            "later approval requests should remain pending in the TUI"
        );
        assert!(controller.pending_approvals.contains_key("tool-2"));
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
            permission_mode: PermissionMode::default(),
            live_permission_mode: Arc::new(RwLock::new(PermissionMode::default())),
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
            project_trust: crate::trust::ProjectTrustState::NotRequired,
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

    /// Regression: the turn driver must receive the controller's *live*
    /// `local_config` (via `TurnRequest.base_config`), not the stale snapshot
    /// captured at construction. Without this, a provider added at runtime via
    /// `/provider` is written to disk but the next turn fails with
    /// "unknown model" because the stale registry is used.
    #[tokio::test]
    async fn turn_request_carries_live_local_config() {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
        let captured_config = std::sync::Arc::clone(&captured);
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            move |request| {
                let captured_config = std::sync::Arc::clone(&captured_config);
                async move {
                    *captured_config.lock().expect("capture config") = request.base_config;
                    Ok(vec![
                        AgentEvent::MessageStarted {
                            turn: 1,
                            id: "m".to_owned(),
                        },
                        AgentEvent::TurnFinished {
                            turn: 1,
                            stop_reason: neo_agent_core::StopReason::EndTurn,
                        },
                    ])
                }
            },
        );

        // Simulate a runtime config change (e.g. provider added via `/provider`)
        // by setting local_config AFTER the controller was built.
        let live_config = test_config_with_models(
            &test_workspace_root(),
            test_workspace_root().join(".neo/sessions"),
            BTreeMap::from([(
                "minimax-cn-coding-plan/MiniMax-M3".to_owned(),
                ModelConfig {
                    provider: "minimax-cn-coding-plan".to_owned(),
                    model: "MiniMax-M3".to_owned(),
                    ..ModelConfig::default()
                },
            )]),
        );
        controller.local_config = Some(live_config);

        controller.type_text("hello");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("submit");
        controller
            .wait_for_active_turn()
            .await
            .expect("turn completes");

        let captured = captured.lock().expect("captured").take();
        let config = captured.expect("base_config was forwarded to the driver");
        assert_eq!(config.default_provider, "openai");
        assert!(
            config
                .models
                .contains_key("minimax-cn-coding-plan/MiniMax-M3")
        );
    }

    fn controller_with_session_for_new_tests() -> (
        InteractiveController,
        std::sync::Arc<std::sync::Mutex<Vec<TurnRequest>>>,
    ) {
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_requests = std::sync::Arc::clone(&requests);
        let mut controller = InteractiveController::new_for_test(
            "neo",
            SESSION_A,
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
                            text: "hi back".to_owned(),
                        },
                        AgentEvent::MessageFinished {
                            turn: 1,
                            id: "assistant-1".to_owned(),
                            stop_reason: StopReason::EndTurn,
                        },
                        AgentEvent::TurnFinished {
                            turn: 1,
                            stop_reason: StopReason::EndTurn,
                        },
                    ])
                }
            },
        );
        // Seed an active session id, transcript content, prompt text, and todos
        // so the reset tests can prove all of them are cleared.
        controller.active_session_id = Some(SESSION_A.to_owned());
        controller
            .tui
            .chrome_mut()
            .set_session_label(SESSION_A.to_owned());
        controller
            .transcript_mut()
            .push_user_message("continue the permission refactor");
        controller
            .transcript_mut()
            .push_assistant_message("I found the old policy conversion path...");
        controller
            .tui
            .chrome_mut()
            .set_todo_items(vec![neo_tui::widgets::TodoDisplayItem::new(
                "Step 1",
                neo_tui::widgets::TodoDisplayStatus::Pending,
            )]);
        (controller, requests)
    }

    #[tokio::test]
    async fn slash_new_resets_to_unsaved_fresh_session_without_streaming() {
        let (mut controller, _requests) = controller_with_session_for_new_tests();

        controller.type_text("/new");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("/new submits");

        assert_eq!(controller.active_session_id(), None);
        assert_eq!(controller.chrome().session_label(), "new");
        assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
        let snapshot = controller.render_snapshot();
        assert!(
            snapshot.contains("Welcome to neo!"),
            "snapshot shows welcome banner"
        );
        assert!(
            snapshot.contains("Started fresh session"),
            "snapshot shows fresh session status"
        );
        assert!(
            !snapshot.contains("permission refactor"),
            "old transcript content is gone"
        );
        assert!(
            !snapshot.contains("policy conversion"),
            "old assistant content is gone"
        );
        assert!(controller.chrome().prompt().text.is_empty());
        assert!(controller.chrome().todo_items().is_empty());
    }

    #[tokio::test]
    async fn slash_clear_alias_resets_to_unsaved_fresh_session() {
        let (mut controller, _requests) = controller_with_session_for_new_tests();

        controller.type_text("/clear");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("/clear submits");

        assert_eq!(controller.active_session_id(), None);
        assert_eq!(controller.chrome().session_label(), "new");
        assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
        let snapshot = controller.render_snapshot();
        assert!(snapshot.contains("Started fresh session"));
        assert!(!snapshot.contains("permission refactor"));
    }

    #[tokio::test]
    async fn slash_new_does_not_enter_streaming_mode() {
        let (mut controller, requests) = controller_with_session_for_new_tests();

        controller.type_text("/new");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("/new submits");

        assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
        assert!(requests.lock().expect("recorded requests").is_empty());
    }

    #[tokio::test]
    async fn slash_new_preserves_model_permission_thinking_and_plan_mode() {
        let (mut controller, _requests) = controller_with_session_for_new_tests();
        // Configure preserved state.
        controller.set_permission_mode(PermissionMode::Yolo);
        controller.current_thinking = true;
        controller.tui.chrome_mut().set_thinking_enabled(true);
        controller.set_plan_mode_from_user(true);

        controller.type_text("/new");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("/new submits");

        assert_eq!(controller.chrome().permission_mode(), PermissionMode::Yolo);
        assert!(controller.chrome().thinking_enabled());
        assert_eq!(controller.chrome().model_label(), "openai/gpt-4.1");
        assert!(
            controller.chrome().is_plan_mode(),
            "user-enabled plan mode is preserved across /new"
        );
    }

    #[tokio::test]
    async fn slash_new_clears_transcript_todos_prompt_and_pending_overlays() {
        let (mut controller, _requests) = controller_with_session_for_new_tests();

        controller.type_text("/new");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("/new submits");

        let snapshot = controller.render_snapshot();
        assert!(snapshot.contains("Welcome to neo!"));
        assert!(
            !snapshot.contains("permission refactor"),
            "old transcript content is cleared"
        );
        assert!(controller.chrome().prompt().text.is_empty());
        assert!(controller.chrome().todo_items().is_empty());
        assert!(controller.active_session_id().is_none());
    }

    #[tokio::test]
    async fn slash_new_preserves_loaded_prompt_history() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = crate::prompt_history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));
        store.append(Some(SESSION_A), "remembered prompt").unwrap();
        let mut controller = controller_with_history_store(store);
        controller.active_session_id = Some(SESSION_A.to_owned());
        controller
            .tui
            .chrome_mut()
            .set_session_label(SESSION_A.to_owned());

        controller.type_text("/new");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("/new submits");

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
            .await
            .expect("up recalls history after /new");
        assert_eq!(controller.chrome().prompt().text, "remembered prompt");
    }

    #[tokio::test]
    async fn slash_new_is_blocked_while_turn_is_running_and_preserves_prompt() {
        // Use a driver that blocks forever until cancelled, so the turn stays
        // active while we submit /new.
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_requests = std::sync::Arc::clone(&requests);
        let run_turn: TurnDriver = Arc::new(move |request, _channels| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            Box::pin(async move {
                captured_requests
                    .lock()
                    .expect("record request")
                    .push(request);
                // Never complete: holds the turn open.
                std::future::pending::<Result<TurnOutcome>>().await
            })
        });
        let mut controller = InteractiveController::new(
            "neo",
            SESSION_A,
            "openai/gpt-4.1",
            test_workspace_root(),
            run_turn,
            PickerCatalogs::default(),
            Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        );
        controller.active_session_id = Some(SESSION_A.to_owned());

        controller.type_text("long running");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("first prompt submits");
        // Let the turn task spawn and register itself.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(controller.active_turn.is_some(), "turn is running");

        controller.type_text("/new");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("/new submit handles blocking");

        assert_eq!(
            controller.active_session_id(),
            Some(SESSION_A),
            "active session id is unchanged when blocked"
        );
        assert!(
            transcript_has_status(
                &controller,
                "Cannot start a new session while a turn is running"
            ),
            "blocked status is shown"
        );
        assert_eq!(
            controller.chrome().prompt().text,
            "/new",
            "blocked /new preserves the command text for retry"
        );

        // Clean up the dangling turn.
        controller.cancel_active_turn().await.expect("cancel turn");
    }

    async fn running_turn_controller() -> InteractiveController {
        let run_turn: TurnDriver = Arc::new(move |_request, _channels| {
            Box::pin(async move {
                // Never complete: holds the turn open for live-slash tests.
                std::future::pending::<Result<TurnOutcome>>().await
            })
        });
        let mut controller = InteractiveController::new(
            "neo",
            SESSION_A,
            "openai/gpt-4.1",
            test_workspace_root(),
            run_turn,
            PickerCatalogs::default(),
            Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        );
        controller.active_session_id = Some(SESSION_A.to_owned());
        controller.type_text("long running");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("first prompt submits");
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(controller.active_turn.is_some(), "turn is running");
        controller
    }

    #[tokio::test]
    async fn slash_auto_updates_permission_mode_while_turn_is_running() {
        let mut controller = running_turn_controller().await;

        controller.type_text("/auto");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("slash handled");

        assert!(controller.active_turn.is_some(), "turn should keep running");
        assert_eq!(controller.chrome().permission_mode(), PermissionMode::Auto);
        assert!(transcript_has_status(&controller, "Permission Mode: auto"));
        assert!(
            !transcript_has_status(&controller, "A turn is already running"),
            "live slash must not be blocked by the active-turn guard"
        );

        controller.cancel_active_turn().await.expect("cancel turn");
    }

    #[tokio::test]
    async fn slash_ask_updates_permission_mode_while_turn_is_running() {
        let mut controller = running_turn_controller().await;
        // Flip to Auto first so /ask is a real change.
        controller.type_text("/auto");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("slash handled");

        controller.type_text("/ask");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("slash handled");

        assert!(controller.active_turn.is_some(), "turn should keep running");
        assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
        assert!(transcript_has_status(&controller, "Permission Mode: ask"));

        controller.cancel_active_turn().await.expect("cancel turn");
    }

    #[tokio::test]
    async fn slash_yolo_updates_permission_mode_while_turn_is_running() {
        let mut controller = running_turn_controller().await;

        controller.type_text("/yolo");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("slash handled");

        assert!(controller.active_turn.is_some(), "turn should keep running");
        assert_eq!(controller.chrome().permission_mode(), PermissionMode::Yolo);
        assert!(transcript_has_status(&controller, "Permission Mode: yolo"));

        controller.cancel_active_turn().await.expect("cancel turn");
    }

    #[tokio::test]
    async fn slash_permissions_degrades_to_hint_while_turn_is_running() {
        let mut controller = running_turn_controller().await;

        controller.type_text("/permissions");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("slash handled");

        assert!(controller.active_turn.is_some(), "turn should keep running");
        // The picker must NOT open during an active turn to avoid racing with
        // approval/question overlays from the running turn.
        assert!(
            controller.chrome().focused_overlay().is_none(),
            "picker overlay must not open during an active turn"
        );
        assert!(transcript_has_status(
            &controller,
            "Use /ask, /auto, or /yolo while a turn is running"
        ));

        controller.cancel_active_turn().await.expect("cancel turn");
    }

    #[tokio::test]
    async fn slash_new_preserves_old_session_for_resume_picker_and_next_prompt_creates_new_session()
    {
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_requests = std::sync::Arc::clone(&requests);
        let run_turn: TurnDriver = Arc::new(move |request, channels| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            Box::pin(async move {
                let is_first = {
                    let mut requests = captured_requests.lock().expect("record request");
                    let is_first = requests.is_empty();
                    requests.push(request);
                    is_first
                };
                if is_first {
                    // First prompt after /new should carry session_id = None and
                    // report a brand-new session id.
                    channels
                        .session_ids
                        .send(SESSION_NEW.to_owned())
                        .expect("session id sent");
                }
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
                Ok(TurnOutcome::default())
            })
        });
        let mut controller = InteractiveController::new(
            "neo",
            SESSION_A,
            "openai/gpt-4.1",
            test_workspace_root(),
            run_turn,
            PickerCatalogs {
                session_items: vec![test_session_summary(
                    SESSION_A,
                    "Alpha",
                    test_workspace_root(),
                    "permission refactor",
                )],
                session_error: None,
                model_items: Vec::new(),
            },
            Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        );
        controller.active_session_id = Some(SESSION_A.to_owned());

        controller.type_text("/new");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("/new submits");

        assert_eq!(controller.active_session_id(), None);
        assert_eq!(controller.chrome().session_label(), "new");
        // The old session is still advertised in the picker catalog.
        assert!(
            controller
                .session_items
                .iter()
                .any(|item| item.id == SESSION_A),
            "old session remains in the picker catalog"
        );

        // The next real prompt should carry session_id = None so the runtime
        // creates a brand-new JSONL session.
        controller.type_text("hello new session");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("next prompt submits");
        controller
            .wait_for_active_turn()
            .await
            .expect("next turn completes");

        let requests = requests.lock().expect("recorded requests");
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].prompt,
            vec![Content::text("hello new session")],
            "next prompt text is forwarded"
        );
        assert_eq!(
            requests[0].session_id, None,
            "next prompt carries no session id so a new session is created"
        );
        assert_eq!(
            controller.chrome().session_label(),
            SESSION_NEW,
            "new session id becomes active"
        );
        assert_eq!(controller.active_session_id(), Some(SESSION_NEW));
    }

    #[test]
    fn slash_completions_include_new_and_clear() {
        let items = session_completion_items(None);
        let values: Vec<&str> = items.iter().map(|item| item.value.as_str()).collect();
        assert!(values.contains(&"/new"), "completions include /new");
        assert!(values.contains(&"/clear"), "completions include /clear");
    }

    #[test]
    fn configured_model_picker_preserves_unqualified_alias() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = test_config_with_models(
            temp.path(),
            temp.path().join(".neo/sessions"),
            BTreeMap::from([(
                "fast".to_owned(),
                ModelConfig {
                    provider: "openai".to_owned(),
                    model: "gpt-4.1".to_owned(),
                    max_context_tokens: Some(1_000_000),
                    ..ModelConfig::default()
                },
            )]),
        );

        let items = model_picker_items_from_config(&config);
        assert_eq!(items[0].value, "fast");
        let selected =
            SelectedModel::from_alias("fast", Some(&config), &items).expect("alias resolves");
        assert_eq!(selected.alias, "fast");
        assert_eq!(selected.provider, "openai");
        assert_eq!(selected.model, "gpt-4.1");
        assert_eq!(selected.max_context_tokens, Some(1_000_000));
    }

    #[tokio::test]
    async fn command_palette_new_session_resets_to_fresh_session() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            SESSION_A,
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.active_session_id = Some(SESSION_A.to_owned());
        controller
            .tui
            .chrome_mut()
            .set_session_label(SESSION_A.to_owned());
        controller
            .transcript_mut()
            .push_user_message("old session content");

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::CommandPaletteOpen))
            .await
            .expect("command palette opens");
        for _ in 0..64 {
            let selected = controller
                .chrome()
                .selected_command()
                .expect("selected command");
            if selected.id == "session.new" {
                break;
            }
            controller
                .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
                .await
                .expect("move to next command");
        }
        assert_eq!(
            controller
                .chrome()
                .selected_command()
                .expect("new session command")
                .id,
            "session.new"
        );

        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
            .await
            .expect("new session command runs");

        assert_eq!(controller.active_session_id(), None);
        assert_eq!(controller.chrome().session_label(), "new");
        let snapshot = controller.render_snapshot();
        assert!(snapshot.contains("Started fresh session"));
        assert!(!snapshot.contains("old session content"));
    }

    // --- NEO-23: cross-session prompt history -----------------------------

    /// Build a test controller with a temp-backed prompt history store so tests
    /// exercise the real load/append path without touching the user's home.
    fn controller_with_history_store(
        store: crate::prompt_history::PromptHistoryStore,
    ) -> InteractiveController {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.set_prompt_history_store(store);
        controller.load_prompt_history();
        controller
    }

    #[tokio::test]
    async fn controller_loads_workspace_prompt_history_on_startup() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = crate::prompt_history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));
        store
            .append(Some("prior-session"), "earlier prompt")
            .expect("seed earlier");
        store
            .append(Some("prior-session"), "latest prompt")
            .expect("seed latest");

        let mut controller = controller_with_history_store(store);

        // Empty composer: first Up recalls the most recent persisted prompt.
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
            .await
            .expect("up recalls latest persisted prompt");
        assert_eq!(controller.chrome().prompt().text, "latest prompt");

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
            .await
            .expect("up recalls older persisted prompt");
        assert_eq!(controller.chrome().prompt().text, "earlier prompt");
    }

    #[tokio::test]
    async fn submitted_prompt_is_persisted_to_workspace_history() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("prompt-history.jsonl");
        let store = crate::prompt_history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));

        let mut controller = controller_with_history_store(store);

        controller.type_text("real prompt from this session");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("prompt submits");
        controller
            .wait_for_active_turn()
            .await
            .expect("turn completes");

        let persisted = std::fs::read_to_string(&path).expect("history file exists");
        assert!(
            persisted.contains("real prompt from this session"),
            "prompt should be persisted: {persisted}"
        );

        // A fresh controller on the same workspace bucket recalls it.
        let store2 = crate::prompt_history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));
        let controller2 = controller_with_history_store(store2);
        assert_eq!(
            controller2
                .chrome()
                .prompt()
                .history_snapshot()
                .last()
                .map(String::as_str),
            Some("real prompt from this session")
        );
        drop(dir);
    }

    #[tokio::test]
    async fn slash_commands_are_not_persisted_to_prompt_history() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("prompt-history.jsonl");
        let store = crate::prompt_history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));

        let mut controller = controller_with_history_store(store);

        // `/model` opens the model picker overlay and never becomes a user
        // turn, so it must not be written to prompt history.
        controller.type_text("/model");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("slash command handled");

        let persisted = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            !persisted.contains("/model"),
            "slash commands must not be persisted: {persisted}"
        );
        drop(dir);
    }

    #[tokio::test]
    async fn slash_mcp_opens_mcp_manager_overlay() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async { Ok(vec![]) },
        );
        let project_dir = test_workspace_root();
        controller.local_config =
            Some(test_config(&project_dir, project_dir.join(".neo/sessions")));
        controller.type_text("/mcp");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("slash command handled");
        let overlay = controller
            .chrome()
            .focused_overlay()
            .expect("/mcp should open an overlay");
        assert!(
            matches!(overlay.kind, OverlayKind::McpManager(_)),
            "/mcp should open the MCP manager overlay, got {:?}",
            overlay.kind
        );
    }

    #[tokio::test]
    async fn slash_mcp_renders_mcp_manager_overlay() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async { Ok(vec![]) },
        );
        let project_dir = test_workspace_root();
        controller.local_config =
            Some(test_config(&project_dir, project_dir.join(".neo/sessions")));
        controller.type_text("/mcp");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("slash command handled");
        let mut transcript = controller.tui.transcript().clone();
        let lines = compose_tui_frame(controller.chrome(), &mut transcript, 80, 24)
            .expect("frame composes");
        let joined = lines.join("\n");
        assert!(
            joined.contains("MCP Servers"),
            "rendered frame should contain MCP manager title: {joined}"
        );
    }

    #[tokio::test]
    async fn mcp_manager_auth_action_shows_status_on_oauth_failure() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async { Ok(vec![]) },
        );
        let temp = tempfile::tempdir().expect("temp dir");
        let project_dir = temp.path().to_path_buf();
        let mut config = test_config(&project_dir, project_dir.join(".neo/sessions"));
        config.mcp.servers.push(crate::config::McpServerConfig {
            id: "example".to_owned(),
            enabled: true,
            transport: "http".to_owned(),
            command: None,
            url: Some("https://example.com/mcp".to_owned()),
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            headers: std::collections::BTreeMap::new(),
            cwd: None,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
            startup_timeout_ms: None,
            tool_timeout_ms: None,
        });
        controller.local_config = Some(config);
        controller.type_text("/mcp");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("open /mcp");
        controller
            .handle_input_event(InputEvent::Insert('O'))
            .await
            .expect("auth key");
        assert!(transcript_has_status(&controller, "OAuth flow failed"));
    }

    #[tokio::test]
    async fn mcp_manager_auth_action_ignored_for_stdio_server() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async { Ok(vec![]) },
        );
        let temp = tempfile::tempdir().expect("temp dir");
        let project_dir = temp.path().to_path_buf();
        let mut config = test_config(&project_dir, project_dir.join(".neo/sessions"));
        config.mcp.servers.push(crate::config::McpServerConfig {
            id: "fs".to_owned(),
            enabled: true,
            transport: "stdio".to_owned(),
            command: Some("mcp-server".to_owned()),
            url: None,
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            headers: std::collections::BTreeMap::new(),
            cwd: None,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
            startup_timeout_ms: None,
            tool_timeout_ms: None,
        });
        controller.local_config = Some(config);
        controller.type_text("/mcp");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("open /mcp");
        controller
            .handle_input_event(InputEvent::Insert('O'))
            .await
            .expect("auth key");
        assert!(!transcript_has_status(
            &controller,
            "No OAuth provider configured"
        ));
    }

    #[tokio::test]
    async fn mcp_add_transport_opens_form() {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async { Ok(vec![]) },
        );
        let project_dir = test_workspace_root();
        controller.local_config =
            Some(test_config(&project_dir, project_dir.join(".neo/sessions")));

        // Open the MCP manager.
        controller.type_text("/mcp");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("slash command handled");
        assert!(
            matches!(
                controller.chrome().focused_overlay().map(|o| &o.kind),
                Some(OverlayKind::McpManager(_))
            ),
            "MCP manager should be focused"
        );

        // Press 'A' to add a server.
        controller
            .handle_input_event(InputEvent::Insert('A'))
            .await
            .expect("add key handled");
        assert!(
            matches!(
                controller.chrome().focused_overlay().map(|o| &o.kind),
                Some(OverlayKind::ChoicePicker(_))
            ),
            "transport choice picker should be focused"
        );

        // Press Enter to select the first transport (real TUI sends Key("enter")).
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
            .await
            .expect("select handled");
        let overlay = controller
            .chrome()
            .focused_overlay()
            .expect("selecting a transport should open the next overlay");
        assert!(
            matches!(overlay.kind, OverlayKind::McpAddForm(_)),
            "expected MCP add form overlay after selecting transport, got {:?}",
            overlay.kind
        );

        // The form must actually be rendered in a single composed frame,
        // and the title should reflect the selected transport so the user
        // knows which transport-specific params are being collected.
        let mut transcript = controller.tui.transcript().clone();
        let lines = compose_tui_frame(controller.chrome(), &mut transcript, 80, 24)
            .expect("frame composes");
        let joined = lines.join("\n");
        assert!(
            joined.contains("Add Local stdio MCP Server"),
            "rendered frame should contain contextual form title: {joined}"
        );
        assert!(
            joined.contains("▸ Name:") && joined.contains("Command:"),
            "rendered frame should show Name and Command fields for stdio: {joined}"
        );
    }

    #[tokio::test]
    async fn mcp_add_form_stdio_submits_to_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");

        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            &project_dir,
            |_request| async { Ok(vec![]) },
        );
        controller.local_config =
            Some(test_config(&project_dir, project_dir.join(".neo/sessions")));

        // Open manager, start add, select stdio.
        controller.type_text("/mcp");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("open manager");
        controller
            .handle_input_event(InputEvent::Insert('A'))
            .await
            .expect("start add");
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
            .await
            .expect("select stdio");
        assert!(
            matches!(
                controller.chrome().focused_overlay().map(|o| &o.kind),
                Some(OverlayKind::McpAddForm(_))
            ),
            "form should be focused"
        );

        // Fill Name, Command, and Env.
        controller
            .handle_input_event(InputEvent::Paste("fs".to_owned()))
            .await
            .expect("type name");
        controller
            .handle_input_event(InputEvent::Insert('\t'))
            .await
            .expect("switch to command");
        controller
            .handle_input_event(InputEvent::Paste(
                "npx -y @server/filesystem /repo".to_owned(),
            ))
            .await
            .expect("type command");
        controller
            .handle_input_event(InputEvent::Insert('\t'))
            .await
            .expect("switch to env");
        controller
            .handle_input_event(InputEvent::Paste("KEY=value".to_owned()))
            .await
            .expect("type env");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("submit form");

        // The MCP manager overlay should be reopened after a successful add.
        assert!(
            matches!(
                controller.chrome().focused_overlay().map(|o| &o.kind),
                Some(OverlayKind::McpManager(_))
            ),
            "MCP manager should be reopened after submit"
        );

        let config = crate::config::read_file_config(&project_dir.join(".neo/config.toml"))
            .expect("read saved config");
        let servers = config.mcp.expect("mcp section").servers;
        assert_eq!(servers.len(), 1, "expected one saved MCP server");
        assert_eq!(servers[0].id, "fs");
        assert_eq!(servers[0].transport, "stdio");
        assert_eq!(
            servers[0].command,
            Some("npx".to_owned()),
            "command is parsed into program"
        );
        assert_eq!(
            servers[0].args,
            vec![
                "-y".to_owned(),
                "@server/filesystem".to_owned(),
                "/repo".to_owned()
            ]
        );
        assert_eq!(
            servers[0].env.get("KEY"),
            Some(&"value".to_owned()),
            "env key is parsed"
        );
        assert!(servers[0].enabled);
    }

    #[tokio::test]
    async fn mcp_add_form_http_submits_to_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");

        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            &project_dir,
            |_request| async { Ok(vec![]) },
        );
        controller.local_config =
            Some(test_config(&project_dir, project_dir.join(".neo/sessions")));

        // Open manager, start add, select HTTP (second item -> one Down + Enter).
        controller.type_text("/mcp");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("open manager");
        controller
            .handle_input_event(InputEvent::Insert('A'))
            .await
            .expect("start add");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move to HTTP");
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
            .await
            .expect("select http");

        // Fill Name, URL, Bearer Token, and Headers.
        controller
            .handle_input_event(InputEvent::Paste("linear".to_owned()))
            .await
            .expect("type name");
        controller
            .handle_input_event(InputEvent::Insert('\t'))
            .await
            .expect("switch to url");
        controller
            .handle_input_event(InputEvent::Paste("https://example.invalid/mcp".to_owned()))
            .await
            .expect("type url");
        controller
            .handle_input_event(InputEvent::Insert('\t'))
            .await
            .expect("switch to token");
        controller
            .handle_input_event(InputEvent::Paste("secret".to_owned()))
            .await
            .expect("type token");
        controller
            .handle_input_event(InputEvent::Insert('\t'))
            .await
            .expect("switch to headers");
        controller
            .handle_input_event(InputEvent::Paste("X-Custom=foo".to_owned()))
            .await
            .expect("type headers");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("submit form");

        let config = crate::config::read_file_config(&project_dir.join(".neo/config.toml"))
            .expect("read saved config");
        let servers = config.mcp.expect("mcp section").servers;
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].id, "linear");
        assert_eq!(servers[0].transport, "http");
        assert_eq!(
            servers[0].url,
            Some("https://example.invalid/mcp".to_owned())
        );
        assert_eq!(
            servers[0].headers.get("Authorization"),
            Some(&"Bearer secret".to_owned()),
            "bearer token is prepended as Authorization header"
        );
        assert_eq!(servers[0].headers.get("X-Custom"), Some(&"foo".to_owned()));
    }

    #[tokio::test]
    async fn mcp_add_form_sse_submits_to_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");

        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            &project_dir,
            |_request| async { Ok(vec![]) },
        );
        controller.local_config =
            Some(test_config(&project_dir, project_dir.join(".neo/sessions")));

        // Open manager, start add, select SSE (third item -> two Down + Enter).
        controller.type_text("/mcp");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("open manager");
        controller
            .handle_input_event(InputEvent::Insert('A'))
            .await
            .expect("start add");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move to HTTP");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move to SSE");
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
            .await
            .expect("select sse");

        // Fill Name and URL only; leave optional fields empty.
        controller
            .handle_input_event(InputEvent::Paste("events".to_owned()))
            .await
            .expect("type name");
        controller
            .handle_input_event(InputEvent::Insert('\t'))
            .await
            .expect("switch to url");
        controller
            .handle_input_event(InputEvent::Paste("https://events.invalid/sse".to_owned()))
            .await
            .expect("type url");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("submit form");

        let config = crate::config::read_file_config(&project_dir.join(".neo/config.toml"))
            .expect("read saved config");
        let servers = config.mcp.expect("mcp section").servers;
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].id, "events");
        assert_eq!(servers[0].transport, "sse");
        assert_eq!(
            servers[0].url,
            Some("https://events.invalid/sse".to_owned())
        );
        assert!(servers[0].headers.is_empty());
    }

    #[tokio::test]
    async fn mcp_add_form_cancel_returns_to_manager() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project dir");

        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            &project_dir,
            |_request| async { Ok(vec![]) },
        );
        controller.local_config =
            Some(test_config(&project_dir, project_dir.join(".neo/sessions")));

        controller.type_text("/mcp");
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("open manager");
        controller
            .handle_input_event(InputEvent::Insert('A'))
            .await
            .expect("start add");
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
            .await
            .expect("select stdio");
        assert!(
            matches!(
                controller.chrome().focused_overlay().map(|o| &o.kind),
                Some(OverlayKind::McpAddForm(_))
            ),
            "form should be focused"
        );

        controller
            .handle_input_event(InputEvent::Cancel)
            .await
            .expect("cancel form");

        assert!(
            matches!(
                controller.chrome().focused_overlay().map(|o| &o.kind),
                Some(OverlayKind::McpManager(_))
            ),
            "MCP manager should be reopened after cancel"
        );

        let config = crate::config::read_file_config(&project_dir.join(".neo/config.toml"))
            .expect("read config");
        assert!(
            config.mcp.is_none() || config.mcp.unwrap().servers.is_empty(),
            "no server should be saved on cancel"
        );
    }

    #[tokio::test]
    async fn prompt_history_is_shared_across_sessions_in_same_workspace() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store_a = crate::prompt_history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));

        // Session A submits a prompt.
        let mut controller_a = controller_with_history_store(store_a);
        controller_a.type_text("first from session a");
        controller_a
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("session a submits");
        controller_a
            .wait_for_active_turn()
            .await
            .expect("session a turn completes");

        // Session B starts fresh in the same workspace bucket and recalls A's
        // prompt via Up from an empty composer.
        let store_b = crate::prompt_history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));
        let mut controller_b = controller_with_history_store(store_b);
        controller_b
            .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
            .await
            .expect("up recalls cross-session prompt");
        assert_eq!(controller_b.chrome().prompt().text, "first from session a");
        drop(dir);
    }

    #[tokio::test]
    async fn prompt_history_is_isolated_by_workspace_bucket() {
        let dir_one = tempfile::tempdir().expect("temp dir one");
        let dir_two = tempfile::tempdir().expect("temp dir two");

        let store_one =
            crate::prompt_history::PromptHistoryStore::for_dir(PathBuf::from(dir_one.path()));
        let mut controller_one = controller_with_history_store(store_one);
        controller_one.type_text("workspace one");
        controller_one
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("workspace one submits");
        controller_one
            .wait_for_active_turn()
            .await
            .expect("workspace one turn completes");

        // A different workspace bucket must not recall workspace one's prompt.
        let store_two =
            crate::prompt_history::PromptHistoryStore::for_dir(PathBuf::from(dir_two.path()));
        let controller_two = controller_with_history_store(store_two);
        assert!(
            controller_two
                .chrome()
                .prompt()
                .history_snapshot()
                .is_empty(),
            "history must be isolated per workspace bucket"
        );
        drop(dir_one);
        drop(dir_two);
    }

    #[tokio::test]
    async fn approval_up_down_does_not_recall_prompt_history() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = crate::prompt_history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));
        store.append(None, "old prompt").expect("seed history");
        let mut controller = controller_with_history_store(store);
        // Composer is empty so any leaked Up would otherwise recall "old prompt".

        controller.apply_turn_event(AgentEvent::ApprovalRequested {
            turn: 1,
            id: "tool-1".to_owned(),
            operation: neo_agent_core::PermissionOperation::Tool,
            subject: "Write".to_owned(),
            arguments: serde_json::json!({"path": "approved.txt"}),
            session_scope: None,
            prefix_rule: None,
        });
        let (decision_tx, _decision_rx) = oneshot::channel();
        controller
            .pending_approvals
            .insert("tool-1".to_owned(), pending_approval_response(decision_tx));

        // Up/Down while approval is focused must move the dialog, not history.
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
            .await
            .expect("up moves approval selection");
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
            .await
            .expect("down moves approval selection");

        assert_eq!(
            controller.chrome().prompt().text,
            "",
            "approval Up/Down must not leak into PromptState"
        );
        drop(dir);
    }

    #[tokio::test]
    async fn question_up_down_does_not_recall_prompt_history() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = crate::prompt_history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));
        store.append(None, "old prompt").expect("seed history");
        let mut controller = controller_with_history_store(store);

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
            .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
            .await
            .expect("up moves question selection");
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
            .await
            .expect("down moves question selection");

        assert_eq!(
            controller.chrome().prompt().text,
            "",
            "question Up/Down must not leak into PromptState"
        );
        drop(dir);
    }

    #[tokio::test]
    async fn active_turn_enter_enqueues_follow_up_instead_of_rejecting() {
        let captured_steer = Arc::new(std::sync::Mutex::new(
            neo_agent_core::SteerInputHandle::new(),
        ));
        let observed_steer = Arc::clone(&captured_steer);
        let run_turn: TurnDriver = Arc::new(move |_request, channels| {
            let observed_steer = Arc::clone(&observed_steer);
            *observed_steer.lock().expect("steer lock") = channels.steer_input.clone();
            Box::pin(async move {
                channels.send_event(AgentEvent::TextDelta {
                    turn: 1,
                    text: "working".to_owned(),
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

        controller.type_text("first prompt");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("first prompt starts turn");
        assert!(controller.active_turn.is_some(), "turn should be active");

        // While the turn is running, typing + Enter must enqueue (not reject).
        controller.type_text("queued follow up");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("enter while busy enqueues");

        let steer_handle = captured_steer.lock().expect("steer lock").clone();
        assert_eq!(
            steer_handle.pending(),
            1,
            "follow-up should be pushed into the steer input handle"
        );
        // Composer should be cleared after queuing.
        assert_eq!(controller.chrome().prompt().text, "");
        assert!(
            controller.active_turn.is_some(),
            "turn must still be running after enqueue"
        );
    }

    #[tokio::test]
    async fn active_turn_enter_waits_for_runtime_event_before_pending_preview() {
        let captured_steer = Arc::new(std::sync::Mutex::new(
            neo_agent_core::SteerInputHandle::new(),
        ));
        let observed_steer = Arc::clone(&captured_steer);
        let run_turn: TurnDriver = Arc::new(move |_request, channels| {
            let observed_steer = Arc::clone(&observed_steer);
            *observed_steer.lock().expect("steer lock") = channels.steer_input.clone();
            Box::pin(async move {
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

        controller.type_text("first prompt");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("first prompt starts turn");

        controller.type_text("queued follow up");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("enter while busy enqueues");

        assert!(
            controller
                .chrome()
                .pending_input()
                .queued_follow_ups()
                .is_empty(),
            "local submit path must not duplicate the runtime FollowUpQueued event"
        );

        controller.apply_turn_event(AgentEvent::FollowUpQueued {
            message: AgentMessage::user_text("queued follow up"),
        });
        assert_eq!(
            controller
                .chrome()
                .pending_input()
                .queued_follow_ups()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["queued follow up"]
        );
        controller.apply_turn_event(AgentEvent::QueueDrained {
            kind: neo_agent_core::QueueKind::FollowUp,
            count: 1,
        });
        assert!(
            controller
                .chrome()
                .pending_input()
                .queued_follow_ups()
                .is_empty(),
            "one runtime drain should clear one queued preview item"
        );
    }

    #[tokio::test]
    async fn active_turn_ctrl_s_steers_running_turn() {
        let captured_steer = Arc::new(std::sync::Mutex::new(
            neo_agent_core::SteerInputHandle::new(),
        ));
        let observed_steer = Arc::clone(&captured_steer);
        let run_turn: TurnDriver = Arc::new(move |_request, channels| {
            let observed_steer = Arc::clone(&observed_steer);
            *observed_steer.lock().expect("steer lock") = channels.steer_input.clone();
            Box::pin(async move {
                channels.send_event(AgentEvent::TextDelta {
                    turn: 1,
                    text: "working".to_owned(),
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

        controller.type_text("first prompt");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("first prompt starts turn");
        assert!(controller.active_turn.is_some());

        // Ctrl+S while busy should steer the running turn.
        controller.type_text("steer this");
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
            .await
            .expect("ctrl+s steers");

        let steer_handle = captured_steer.lock().expect("steer lock").clone();
        assert_eq!(steer_handle.pending(), 1, "steer should be pushed");
        // Composer cleared after steering.
        assert_eq!(controller.chrome().prompt().text, "");
    }

    #[tokio::test]
    async fn active_turn_ctrl_s_waits_for_runtime_event_before_pending_preview() {
        let captured_steer = Arc::new(std::sync::Mutex::new(
            neo_agent_core::SteerInputHandle::new(),
        ));
        let observed_steer = Arc::clone(&captured_steer);
        let run_turn: TurnDriver = Arc::new(move |_request, channels| {
            let observed_steer = Arc::clone(&observed_steer);
            *observed_steer.lock().expect("steer lock") = channels.steer_input.clone();
            Box::pin(async move {
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

        controller.type_text("first prompt");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("first prompt starts turn");

        controller.type_text("steer this");
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
            .await
            .expect("ctrl+s steers");

        assert!(
            controller
                .chrome()
                .pending_input()
                .pending_steers()
                .is_empty(),
            "local Ctrl+S path must not duplicate the runtime SteeringQueued event"
        );
        controller.apply_turn_event(AgentEvent::SteeringQueued {
            message: AgentMessage::user_text("steer this"),
        });
        assert_eq!(
            controller
                .chrome()
                .pending_input()
                .pending_steers()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["steer this"]
        );
    }

    #[tokio::test]
    async fn empty_ctrl_s_promotes_oldest_follow_up_fifo_without_local_duplication() {
        let captured_steer = Arc::new(std::sync::Mutex::new(
            neo_agent_core::SteerInputHandle::new(),
        ));
        let observed_steer = Arc::clone(&captured_steer);
        let run_turn: TurnDriver = Arc::new(move |_request, channels| {
            let observed_steer = Arc::clone(&observed_steer);
            *observed_steer.lock().expect("steer lock") = channels.steer_input.clone();
            Box::pin(async move {
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

        controller.type_text("first prompt");
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("first prompt starts turn");
        controller.apply_turn_event(AgentEvent::FollowUpQueued {
            message: AgentMessage::user_text("queued one"),
        });
        controller.apply_turn_event(AgentEvent::FollowUpQueued {
            message: AgentMessage::user_text("queued two"),
        });

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
            .await
            .expect("empty ctrl+s promotes oldest queued follow-up");

        let steer_handle = captured_steer.lock().expect("steer lock").clone();
        assert_eq!(
            steer_handle.pending(),
            1,
            "promoted follow-up is sent as steer"
        );
        assert_eq!(
            controller
                .chrome()
                .pending_input()
                .queued_follow_ups()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["queued one", "queued two"],
            "local promotion intent must wait for runtime drain before changing follow-up preview"
        );
        assert!(
            controller
                .chrome()
                .pending_input()
                .pending_steers()
                .is_empty(),
            "local promotion must wait for the runtime SteeringQueued event before showing steer preview"
        );

        controller.apply_turn_event(AgentEvent::QueueDrained {
            kind: neo_agent_core::QueueKind::FollowUp,
            count: 1,
        });
        assert_eq!(
            controller
                .chrome()
                .pending_input()
                .queued_follow_ups()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["queued two"],
            "runtime follow-up drain removes the promoted FIFO item"
        );
        controller.apply_turn_event(AgentEvent::SteeringQueued {
            message: AgentMessage::user_text("queued one"),
        });
        assert_eq!(
            controller
                .chrome()
                .pending_input()
                .pending_steers()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["queued one"]
        );
        controller.apply_turn_event(AgentEvent::QueueDrained {
            kind: neo_agent_core::QueueKind::Steering,
            count: 1,
        });
        assert!(
            controller
                .chrome()
                .pending_input()
                .pending_steers()
                .is_empty(),
            "one runtime steer drain should clear the promoted preview"
        );
    }

    #[tokio::test]
    async fn empty_ctrl_s_with_no_queue_reports_noop_status() {
        let mut controller = running_turn_controller().await;

        controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
            .await
            .expect("empty ctrl+s with no queue is handled");

        assert!(
            transcript_has_status(&controller, "No queued follow-up to steer"),
            "empty Ctrl+S with no queue should be visible feedback"
        );

        controller.cancel_active_turn().await.expect("cancel turn");
    }

    #[tokio::test]
    async fn idle_ctrl_s_falls_back_to_normal_submit() {
        let prompt_seen = Arc::new(std::sync::Mutex::new(None));
        let observed_prompt = Arc::clone(&prompt_seen);
        let run_turn: TurnDriver = Arc::new(move |request, channels| {
            let observed_prompt = Arc::clone(&observed_prompt);
            Box::pin(async move {
                *observed_prompt.lock().expect("prompt lock") = Some(request.prompt.clone());
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

        controller.type_text("submit via ctrl+s");
        controller
            .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
            .await
            .expect("ctrl+s submits when idle");
        controller
            .wait_for_active_turn()
            .await
            .expect("idle ctrl+s turn completes");

        let seen = prompt_seen.lock().expect("prompt lock").clone();
        assert_eq!(
            seen,
            Some(vec![Content::text("submit via ctrl+s")]),
            "idle Ctrl+S should behave like a normal submit"
        );
    }

    #[tokio::test]
    async fn startup_trust_dialog_opens_when_unknown_and_trusts_workspace() {
        use std::collections::VecDeque;

        struct ScriptedEvents(VecDeque<InputEvent>);
        impl TerminalEvents for ScriptedEvents {
            fn next_input_event(&mut self) -> Result<InputEvent> {
                self.0
                    .pop_front()
                    .context("expected scripted trust dialog input")
            }
        }

        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project");
        fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");

        let trust_path = temp.path().join("trust.json");
        let store = crate::trust::ProjectTrustStore::new(trust_path.clone());

        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            &project_dir,
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        let mut config = test_config(&project_dir, project_dir.join(".neo/sessions"));
        let inputs =
            crate::trust::collect_project_trust_inputs(&project_dir).expect("collect inputs");
        config.project_trust = crate::trust::ProjectTrustState::Unknown { inputs };
        config.project_trusted = false;
        controller.local_config = Some(config);
        controller.set_trust_store(store);

        let data = crate::trust::trust_dialog_data_from_inputs(
            crate::trust::collect_project_trust_inputs(&project_dir).expect("collect inputs"),
        );
        controller
            .resolve_trust_dialog_at_startup(
                data,
                ScriptedEvents(VecDeque::from([
                    // Default is ContinueUntrusted; move up once to TrustCurrent.
                    InputEvent::Action(KeybindingAction::SelectUp),
                    InputEvent::Action(KeybindingAction::SelectConfirm),
                ])),
                |_| Ok(()),
            )
            .await
            .expect("resolve trust dialog");

        assert!(controller.local_config.as_ref().unwrap().project_trusted);
        assert!(matches!(
            controller.local_config.as_ref().unwrap().project_trust,
            crate::trust::ProjectTrustState::Trusted { .. }
        ));
        assert!(controller.render_snapshot().contains("Workspace trusted"));
        assert_eq!(
            crate::trust::ProjectTrustStore::new(trust_path)
                .get(&project_dir)
                .expect("read trust"),
            Some(true)
        );
    }

    #[tokio::test]
    async fn startup_trust_dialog_opens_when_unknown_and_continues_untrusted() {
        use std::collections::VecDeque;

        struct ScriptedEvents(VecDeque<InputEvent>);
        impl TerminalEvents for ScriptedEvents {
            fn next_input_event(&mut self) -> Result<InputEvent> {
                self.0
                    .pop_front()
                    .context("expected scripted trust dialog input")
            }
        }

        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project");
        fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");

        let trust_path = temp.path().join("trust.json");
        let store = crate::trust::ProjectTrustStore::new(trust_path.clone());

        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            &project_dir,
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        let mut config = test_config(&project_dir, project_dir.join(".neo/sessions"));
        let inputs =
            crate::trust::collect_project_trust_inputs(&project_dir).expect("collect inputs");
        config.project_trust = crate::trust::ProjectTrustState::Unknown { inputs };
        config.project_trusted = false;
        controller.local_config = Some(config);
        controller.set_trust_store(store);

        let data = crate::trust::trust_dialog_data_from_inputs(
            crate::trust::collect_project_trust_inputs(&project_dir).expect("collect inputs"),
        );
        controller
            .resolve_trust_dialog_at_startup(
                data,
                ScriptedEvents(VecDeque::from([InputEvent::Action(
                    KeybindingAction::SelectConfirm,
                )])),
                |_| Ok(()),
            )
            .await
            .expect("resolve trust dialog");

        assert!(!controller.local_config.as_ref().unwrap().project_trusted);
        assert!(matches!(
            controller.local_config.as_ref().unwrap().project_trust,
            crate::trust::ProjectTrustState::Untrusted { .. }
        ));
        assert!(controller.render_snapshot().contains("Workspace untrusted"));
        assert_eq!(
            crate::trust::ProjectTrustStore::new(trust_path)
                .get(&project_dir)
                .expect("read trust"),
            Some(false)
        );
    }

    #[test]
    fn startup_trust_dialog_data_is_some_for_unknown_and_none_otherwise() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project");

        let mut config = test_config(&project_dir, project_dir.join(".neo/sessions"));
        config.project_trust = crate::trust::ProjectTrustState::NotRequired;
        assert!(trust_dialog_data_for_startup(&config).is_none());

        fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");
        let inputs =
            crate::trust::collect_project_trust_inputs(&project_dir).expect("collect inputs");
        config.project_trust = crate::trust::ProjectTrustState::Unknown { inputs };
        let data = trust_dialog_data_for_startup(&config);
        assert!(data.is_some());
        assert_eq!(
            data.unwrap().current_dir,
            project_dir.canonicalize().expect("canonicalize")
        );

        config.project_trust = crate::trust::ProjectTrustState::Trusted {
            target: project_dir.clone(),
        };
        assert!(trust_dialog_data_for_startup(&config).is_none());

        config.project_trust = crate::trust::ProjectTrustState::Untrusted {
            target: project_dir.clone(),
        };
        assert!(trust_dialog_data_for_startup(&config).is_none());
    }

    #[tokio::test]
    async fn startup_trust_dialog_cancels_to_untrusted() {
        use std::collections::VecDeque;

        struct ScriptedEvents(VecDeque<InputEvent>);
        impl TerminalEvents for ScriptedEvents {
            fn next_input_event(&mut self) -> Result<InputEvent> {
                self.0
                    .pop_front()
                    .context("expected scripted trust dialog input")
            }
        }

        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project");
        fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");

        let trust_path = temp.path().join("trust.json");
        let store = crate::trust::ProjectTrustStore::new(trust_path.clone());

        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            &project_dir,
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        let mut config = test_config(&project_dir, project_dir.join(".neo/sessions"));
        let inputs =
            crate::trust::collect_project_trust_inputs(&project_dir).expect("collect inputs");
        config.project_trust = crate::trust::ProjectTrustState::Unknown { inputs };
        config.project_trusted = false;
        controller.local_config = Some(config);
        controller.set_trust_store(store);

        let data = crate::trust::trust_dialog_data_from_inputs(
            crate::trust::collect_project_trust_inputs(&project_dir).expect("collect inputs"),
        );
        controller
            .resolve_trust_dialog_at_startup(
                data,
                ScriptedEvents(VecDeque::from([InputEvent::Action(
                    KeybindingAction::SelectCancel,
                )])),
                |_| Ok(()),
            )
            .await
            .expect("resolve trust dialog");

        assert!(!controller.local_config.as_ref().unwrap().project_trusted);
        assert!(matches!(
            controller.local_config.as_ref().unwrap().project_trust,
            crate::trust::ProjectTrustState::Untrusted { .. }
        ));
        assert!(controller.render_snapshot().contains("Workspace untrusted"));
        assert_eq!(
            crate::trust::ProjectTrustStore::new(trust_path)
                .get(&project_dir)
                .expect("read trust"),
            Some(false)
        );
    }

    fn btw_test_config(project_dir: &std::path::Path) -> crate::config::AppConfig {
        test_config(project_dir, project_dir.join(".neo/sessions"))
    }

    fn btw_fake_client(answer: &str) -> Arc<dyn neo_ai::ModelClient> {
        use neo_ai::{AiStreamEvent, StopReason};
        Arc::new(neo_ai::providers::fake::FakeModelClient::new(vec![
            AiStreamEvent::MessageStart {
                id: "msg-1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: answer.to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]))
    }

    fn chat_message_text(message: &neo_ai::ChatMessage) -> String {
        let content = match message {
            neo_ai::ChatMessage::System { content }
            | neo_ai::ChatMessage::User { content }
            | neo_ai::ChatMessage::Assistant { content, .. }
            | neo_ai::ChatMessage::ToolResult { content, .. } => content,
        };
        content
            .iter()
            .filter_map(|part| match part {
                neo_ai::ContentPart::Text { text } => Some(text.as_str()),
                neo_ai::ContentPart::Thinking { .. } | neo_ai::ContentPart::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    #[tokio::test]
    async fn slash_btw_opens_empty_sidecar_panel_without_starting_main_turn() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project");
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            &project_dir,
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.local_config = Some(btw_test_config(&project_dir));
        controller.set_btw_client(btw_fake_client(""));

        controller.handle_slash_command("/btw").await;

        assert!(
            controller.chrome().has_btw_panel(),
            "/btw opens the sidecar panel"
        );
        assert!(
            controller.btw_runner.is_some(),
            "/btw creates a sidecar runner"
        );
        assert!(
            controller.active_turn.is_none(),
            "/btw must not start a main turn"
        );
    }

    #[tokio::test]
    async fn slash_btw_question_starts_in_memory_sidecar_only() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project");
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            &project_dir,
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.local_config = Some(btw_test_config(&project_dir));
        controller.set_btw_client(btw_fake_client("42"));

        controller.handle_slash_command("/btw what is 2+2?").await;

        assert!(controller.chrome().has_btw_panel());
        assert!(controller.btw_receiver.is_some());
        assert!(controller.active_turn.is_none());

        // Drain events so the panel state reflects the sidecar answer.
        for _ in 0..10 {
            controller.drain_btw_sidecar();
            tokio::task::yield_now().await;
        }
        let state = controller.chrome().btw_panel_state().expect("panel state");
        assert_eq!(state.sidecar.turns.len(), 1);
        assert_eq!(state.sidecar.turns[0].prompt, "what is 2+2?");
        assert_eq!(state.sidecar.turns[0].answer, "42");
    }

    #[tokio::test]
    async fn slash_btw_inherits_main_context_with_single_sidecar_projection() {
        use neo_ai::{AiStreamEvent, StopReason};

        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project");
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            &project_dir,
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.local_config = Some(btw_test_config(&project_dir));
        controller.active_session_id = Some("session-current".to_owned());
        controller.apply_turn_event(AgentEvent::MessageAppended {
            message: AgentMessage::user_text("main context in memory"),
        });
        let fake = neo_ai::providers::fake::FakeModelClient::new(vec![
            AiStreamEvent::MessageStart {
                id: "msg-1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "side".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);
        controller.set_btw_client(Arc::new(fake.clone()));

        controller
            .handle_slash_command("/btw inspect context")
            .await;
        for _ in 0..20 {
            controller.drain_btw_sidecar();
            tokio::task::yield_now().await;
        }

        let requests = fake.requests();
        assert_eq!(requests.len(), 1);
        let contents: Vec<String> = requests[0].messages.iter().map(chat_message_text).collect();
        assert!(
            contents
                .iter()
                .any(|content| content == "main context in memory"),
            "sidecar should inherit current in-memory main transcript: {contents:?}"
        );
        assert_eq!(
            contents
                .iter()
                .filter(|content| content.contains("side-channel conversation"))
                .count(),
            1,
            "sidecar reminder should be projected exactly once: {contents:?}"
        );
    }

    #[tokio::test]
    async fn bare_slash_btw_while_sidecar_running_keeps_existing_panel() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project");
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            &project_dir,
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.local_config = Some(btw_test_config(&project_dir));
        controller.set_btw_client(btw_fake_client(""));

        controller.handle_slash_command("/btw").await;
        {
            let state = controller
                .tui
                .chrome_mut()
                .btw_panel_state_mut()
                .expect("panel state");
            state.sidecar.phase = neo_tui::widgets::btw_panel::BtwPhase::Running;
        }
        let original_id = controller
            .chrome()
            .btw_panel_state()
            .expect("panel state")
            .sidecar
            .id
            .0
            .clone();

        controller.handle_slash_command("/btw").await;

        let state = controller.chrome().btw_panel_state().expect("panel state");
        assert_eq!(state.sidecar.id.0, original_id);
        assert_eq!(
            state.sidecar.phase,
            neo_tui::widgets::btw_panel::BtwPhase::Running
        );
        assert!(state.status_message.as_deref().is_some_and(|message| {
            message.contains("already open") || message.contains("Wait for /btw")
        }));
    }

    #[tokio::test]
    async fn composer_routes_to_sidecar_when_panel_open() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project");
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            &project_dir,
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.local_config = Some(btw_test_config(&project_dir));
        controller.set_btw_client(btw_fake_client("answer"));

        controller.handle_slash_command("/btw").await;
        controller.type_text("explain this");
        controller
            .submit_current_prompt()
            .await
            .expect("submit routes to sidecar");

        assert!(controller.active_turn.is_none(), "must not start main turn");
        for _ in 0..10 {
            controller.drain_btw_sidecar();
            tokio::task::yield_now().await;
        }
        let state = controller.chrome().btw_panel_state().expect("panel state");
        assert_eq!(state.sidecar.turns.len(), 1);
        assert_eq!(state.sidecar.turns[0].prompt, "explain this");
    }

    #[tokio::test]
    async fn empty_composer_esc_closes_panel() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project");
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            &project_dir,
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.local_config = Some(btw_test_config(&project_dir));
        controller.set_btw_client(btw_fake_client(""));

        controller.handle_slash_command("/btw").await;
        assert!(controller.chrome().has_btw_panel());

        controller
            .handle_input_event(InputEvent::Cancel)
            .await
            .expect("esc handled");

        assert!(
            !controller.chrome().has_btw_panel(),
            "Esc closes empty panel"
        );
    }

    #[tokio::test]
    async fn sidecar_events_do_not_append_to_main_transcript() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project");
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            &project_dir,
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.local_config = Some(btw_test_config(&project_dir));
        controller.set_btw_client(btw_fake_client("side answer"));

        let entries_before = controller.tui.transcript().transcript().entries().len();
        controller.handle_slash_command("/btw side question").await;
        for _ in 0..20 {
            controller.drain_btw_sidecar();
            tokio::task::yield_now().await;
        }
        let entries_after = controller.tui.transcript().transcript().entries().len();

        assert_eq!(
            entries_before, entries_after,
            "sidecar must not append to main transcript"
        );
        let state = controller.chrome().btw_panel_state().expect("panel state");
        assert_eq!(state.sidecar.turns[0].answer, "side answer");
    }

    #[tokio::test]
    async fn slash_btw_while_main_turn_running_does_not_steer_or_queue_main_turn() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project");
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            &project_dir,
            |_request| async move { std::future::pending::<Result<Vec<AgentEvent>>>().await },
        );
        controller.local_config = Some(btw_test_config(&project_dir));
        controller.set_btw_client(btw_fake_client("side answer"));

        controller.type_text("main question");
        controller
            .submit_current_prompt()
            .await
            .expect("main turn starts");
        assert!(
            controller.active_turn.is_some(),
            "main turn should be active"
        );

        controller.handle_slash_command("/btw side question").await;
        for _ in 0..20 {
            controller.drain_btw_sidecar();
            tokio::task::yield_now().await;
        }

        assert!(
            controller.active_turn.is_some(),
            "/btw must not cancel or queue the main turn"
        );
        let state = controller.chrome().btw_panel_state().expect("panel state");
        assert_eq!(state.sidecar.turns.len(), 1);
        assert_eq!(state.sidecar.turns[0].answer, "side answer");
    }

    #[tokio::test]
    async fn escape_closes_btw_without_touching_main_turn() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project");
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            &project_dir,
            |_request| async move { std::future::pending::<Result<Vec<AgentEvent>>>().await },
        );
        controller.local_config = Some(btw_test_config(&project_dir));
        controller.set_btw_client(btw_fake_client(""));

        controller.type_text("main question");
        controller
            .submit_current_prompt()
            .await
            .expect("main turn starts");
        assert!(controller.active_turn.is_some());

        controller.handle_slash_command("/btw").await;
        assert!(controller.chrome().has_btw_panel());

        controller
            .handle_input_event(InputEvent::Cancel)
            .await
            .expect("esc handled");

        assert!(!controller.chrome().has_btw_panel(), "Esc closes BTW panel");
        assert!(
            controller.active_turn.is_some(),
            "Esc must not cancel the main turn"
        );
    }

    #[tokio::test]
    async fn btw_running_preserves_composer_text_and_shows_busy_notice() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project");
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            &project_dir,
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.local_config = Some(btw_test_config(&project_dir));
        controller.set_btw_client(btw_fake_client(""));

        // Open an empty sidecar panel and mark it Running as if a turn were in
        // progress. This avoids coupling the test to a hanging model client.
        controller.handle_slash_command("/btw").await;
        if let Some(state) = controller.tui.chrome_mut().btw_panel_state_mut() {
            state.sidecar.phase = neo_tui::widgets::btw_panel::BtwPhase::Running;
        }

        controller.type_text("second question");
        controller
            .submit_current_prompt()
            .await
            .expect("busy check handled");

        assert_eq!(
            controller.chrome().prompt().text,
            "second question",
            "composer text must be preserved while sidecar is running"
        );
        let state = controller.chrome().btw_panel_state().expect("panel state");
        assert_eq!(state.sidecar.turns.len(), 0, "no sidecar turn started");
        assert!(
            state
                .status_message
                .as_deref()
                .expect("busy notice")
                .contains("Wait for /btw to finish"),
            "busy notice should be shown"
        );
    }

    #[tokio::test]
    async fn btw_conversation_is_not_written_to_main_session_jsonl() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project");
        let sessions_dir = project_dir.join(".neo/sessions");
        fs::create_dir_all(&sessions_dir).expect("create sessions dir");

        let session_id = "session_00000000-0000-4000-8000-000000000901";
        let session_path = sessions_dir.join(session_id).join("transcript.jsonl");
        fs::create_dir_all(session_path.parent().expect("session dir"))
            .expect("create session dir");
        let mut writer = neo_agent_core::session::JsonlSessionWriter::create(&session_path)
            .await
            .expect("create session");
        writer
            .append_event(&AgentEvent::MessageAppended {
                message: AgentMessage::user_text("existing main message"),
            })
            .await
            .expect("append event");
        writer.flush().await.expect("flush");
        drop(writer);

        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            &project_dir,
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.local_config = Some(btw_test_config(&project_dir));
        controller.set_btw_client(btw_fake_client("side answer"));
        controller.active_session_id = Some(session_id.to_owned());

        controller.handle_slash_command("/btw side question").await;
        for _ in 0..20 {
            controller.drain_btw_sidecar();
            tokio::task::yield_now().await;
        }

        let state = controller.chrome().btw_panel_state().expect("panel state");
        assert_eq!(state.sidecar.turns[0].answer, "side answer");

        let content = fs::read_to_string(&session_path).expect("read session");
        assert!(
            content.contains("existing main message"),
            "original main event should still be present"
        );
        assert!(
            !content.contains("side question"),
            "side question must not be written to main JSONL"
        );
        assert!(
            !content.contains("side answer"),
            "side answer must not be written to main JSONL"
        );
    }

    #[tokio::test]
    async fn shift_enter_inserts_newline_while_btw_panel_open() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project");
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            &project_dir,
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.local_config = Some(btw_test_config(&project_dir));
        controller.set_btw_client(btw_fake_client(""));

        controller.handle_slash_command("/btw").await;
        assert!(controller.chrome().has_btw_panel());

        controller.type_text("line1");
        controller
            .handle_input_event(InputEvent::NewLine)
            .await
            .expect("newline handled");
        controller.type_text("line2");

        assert_eq!(controller.chrome().prompt().text, "line1\nline2");
    }
}
