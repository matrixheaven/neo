use std::{
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use futures::{FutureExt, StreamExt, future::BoxFuture, stream, stream::FuturesUnordered};
use neo_ai::{
    AiError, AiStreamEvent, ChatMessage, ChatRequest, ContentPart, ModelClient, ModelSpec,
    ReasoningEffort, RequestOptions, ToolSpec,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::image_blobs::*;
use super::queue::*;
use super::tokens::*;
use crate::goal::GoalManager;
use crate::permissions::{
    ApprovalRuleStore, FileWriteApprovalOperation, PrefixApprovalRule, SessionApprovalKey,
    SessionApprovalScope, command_might_be_dangerous, is_known_safe_command,
};
use crate::skills::SkillStore;
use crate::tools::normalize_path;
use crate::tools::{BackgroundTaskManager, execute_model_bash_for_runtime};
use crate::{
    AgentEvent, AgentMessage, AgentToolCall, CompactionPhase, CompactionReason, CompactionSource,
    CompactionSummary, Content, PermissionApprovalDecision, PermissionMode,
    PermissionOperation, PlanMode, PlanModeGuard, PlanModeInjector, ProcessSupervisor, QueueKind,
    ShellCommandOrigin, ShellCommandOutcome, StopReason, TodoEventData, ToolAccess, ToolContext,
    ToolError, ToolRegistry, ToolResult, ToolUpdateCallback, check_plan_mode_guard,
    compaction::{self, CompactionStrategy},
    is_active_plan_file_path, sanitize_tool_exchange_messages,
};

pub type ContextTransform = Arc<dyn Fn(&[AgentMessage]) -> Vec<AgentMessage> + Send + Sync>;
pub type BeforeToolCallHook = Arc<dyn Fn(&AgentToolCall) -> Option<ToolResult> + Send + Sync>;
pub type AsyncBeforeToolCallHook = Arc<
    dyn Fn(AgentToolCall, CancellationToken) -> BoxFuture<'static, Option<ToolResult>>
        + Send
        + Sync,
>;
pub type AfterToolCallHook = Arc<dyn Fn(&AgentToolCall, ToolResult) -> ToolResult + Send + Sync>;
pub type AsyncAfterToolCallHook = Arc<
    dyn Fn(AgentToolCall, ToolResult, CancellationToken) -> BoxFuture<'static, ToolResult>
        + Send
        + Sync,
>;
pub type ApprovalHandler =
    Arc<dyn Fn(&ApprovalRequest) -> PermissionApprovalDecision + Send + Sync>;
pub type AsyncApprovalHandler =
    Arc<dyn Fn(ApprovalRequest) -> BoxFuture<'static, PermissionApprovalDecision> + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalRequest {
    pub turn: u32,
    pub id: String,
    pub operation: PermissionOperation,
    pub subject: String,
    pub arguments: serde_json::Value,
    /// Reusable session scope for this request, when safely derivable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_scope: Option<SessionApprovalScope>,
    /// Proposed persistent prefix rule for this request (Layer 2), when the
    /// command reduces to a stable argv prefix. `None` when no prefix option
    /// should be offered (compound/opaque commands, non-shell tools).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_rule: Option<PrefixApprovalRule>,
}

enum PermissionPreparation {
    Run(ToolAccess),
    Ask {
        operation: PermissionOperation,
        subject: String,
        session_scope: Option<SessionApprovalScope>,
        prefix_rule: Option<PrefixApprovalRule>,
    },
    Deny(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolSchedulingClass {
    ParallelSafe,
    Exclusive,
    BlockingDialog,
}

struct PreparedToolCall {
    result: PreparedToolCallResult,
    access: ToolAccess,
}

enum PreparedToolCallResult {
    Run,
    Skip(ToolResult),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum QueueMode {
    All,
    OneAtATime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ToolExecutionMode {
    Sequential,
    Parallel,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentConfig {
    pub model: ModelSpec,
    pub workspace_root: Option<PathBuf>,
    pub system_prompt: Option<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub replay_reasoning: bool,
    pub tools: Vec<ToolSpec>,
    pub steering_queue_mode: QueueMode,
    pub follow_up_queue_mode: QueueMode,
    pub tool_execution_mode: ToolExecutionMode,
    pub permission_mode: PermissionMode,
    /// Shared live permission state. Updated by the TUI when the user runs
    /// `/ask`, `/auto`, `/yolo` (or opens `/permissions`) even mid-turn, and
    /// read by `permission_preparation_for_mode` at every tool call so a
    /// running turn immediately honors the new posture.
    ///
    /// `permission_mode` above is kept for serialization/snapshots and initial
    /// state; live evaluation must go through [`AgentConfig::current_permission_mode`].
    #[serde(skip)]
    #[schemars(skip)]
    pub live_permission_mode: Arc<RwLock<PermissionMode>>,
    pub compaction: Option<CompactionSettings>,
    #[serde(skip)]
    #[schemars(skip)]
    pub context_transform: Option<ContextTransform>,
    #[serde(skip)]
    #[schemars(skip)]
    pub before_tool_call: Option<BeforeToolCallHook>,
    #[serde(skip)]
    #[schemars(skip)]
    pub async_before_tool_call: Option<AsyncBeforeToolCallHook>,
    #[serde(skip)]
    #[schemars(skip)]
    pub after_tool_call: Option<AfterToolCallHook>,
    #[serde(skip)]
    #[schemars(skip)]
    pub async_after_tool_call: Option<AsyncAfterToolCallHook>,
    #[serde(skip)]
    #[schemars(skip)]
    pub approval_handler: Option<ApprovalHandler>,
    #[serde(skip)]
    #[schemars(skip)]
    pub async_approval_handler: Option<AsyncApprovalHandler>,
    /// Shared plan-mode state. Checked before every tool call via
    /// [`check_plan_mode_guard`]. Updated when the model calls
    /// `EnterPlanMode` / `ExitPlanMode`.
    #[serde(skip)]
    #[schemars(skip)]
    pub plan_mode: Arc<RwLock<PlanMode>>,
    /// True while the TUI is in AI-assisted goal authoring mode.
    #[serde(skip)]
    #[schemars(skip)]
    pub goal_mode_authoring: bool,
    /// Side-channel for `ExitPlanMode` Revise feedback, keyed by `tool_call.id`.
    /// Populated by the approval handler when the user picks Revise.
    #[serde(skip)]
    #[schemars(skip)]
    pub plan_review_feedback: Arc<Mutex<std::collections::HashMap<String, String>>>,
    /// Side-channel for the `ExitPlanMode` selected-option label, keyed by
    /// `tool_call.id`. Populated by the approval handler when the user picks a
    /// model-supplied option from the plan-review picker. Consumed by
    /// `attach_exit_plan_details` to prefix the tool result with
    /// "Selected approach: <label>" so the model runs only that branch.
    #[serde(skip)]
    #[schemars(skip)]
    pub plan_review_selected_label: Arc<Mutex<std::collections::HashMap<String, String>>>,
    /// Narrow reusable approval grants for this session. Keyed by
    /// [`SessionApprovalKey`] (exact canonical command + cwd, exact file
    /// write/edit path), never by tool name, so approving one command never
    /// approves a different command on the same tool.
    #[serde(skip)]
    #[schemars(skip)]
    pub session_approvals: Arc<Mutex<std::collections::HashSet<SessionApprovalKey>>>,
    /// Persistent prefix approval rules (Layer 2). Loaded from
    /// `~/.neo/approval_rules.json` on startup; a prefix match auto-approves
    /// any shell command whose argv starts with the rule prefix.
    #[serde(skip)]
    #[schemars(skip)]
    pub prefix_approval_rules: Arc<Mutex<ApprovalRuleStore>>,
    /// Home directory for persistent state (e.g. `~/.neo`, approval rules).
    pub home_dir: Option<PathBuf>,
    /// Session directory for this turn. Used to store plan files (`plans/`) and
    /// image blobs scoped to the active session.
    #[serde(skip)]
    #[schemars(skip)]
    pub session_directory: Option<PathBuf>,
    /// Shared todo list state. Used by `TodoTool` read mode and kept in sync
    /// with replayed/runtime `TodoUpdated` events.
    #[serde(skip)]
    #[schemars(skip)]
    pub todos: Arc<Mutex<Vec<TodoEventData>>>,
    /// Shared background task manager for Bash and `AskUserQuestion` background tasks.
    #[serde(skip)]
    #[schemars(skip)]
    pub background_tasks: BackgroundTaskManager,
    /// Shared manual `/compact` request. `Some(instruction)` means a manual
    /// compaction was requested with an optional custom instruction; `None`
    /// means no request is pending. Set by the TUI and taken by `maybe_compact`.
    #[serde(skip)]
    #[schemars(skip)]
    pub manual_compact_request: Arc<std::sync::Mutex<Option<String>>>,
}

impl AgentConfig {
    #[must_use]
    pub fn for_model(model: ModelSpec) -> Self {
        Self {
            model,
            workspace_root: None,
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            reasoning_effort: None,
            replay_reasoning: true,
            tools: Vec::new(),
            steering_queue_mode: QueueMode::All,
            follow_up_queue_mode: QueueMode::All,
            tool_execution_mode: ToolExecutionMode::Parallel,
            permission_mode: PermissionMode::default(),
            live_permission_mode: Arc::new(RwLock::new(PermissionMode::default())),
            compaction: None,
            context_transform: None,
            before_tool_call: None,
            async_before_tool_call: None,
            after_tool_call: None,
            async_after_tool_call: None,
            approval_handler: None,
            async_approval_handler: None,
            plan_mode: Arc::new(RwLock::new(PlanMode::default())),
            goal_mode_authoring: false,
            plan_review_feedback: Arc::new(Mutex::new(std::collections::HashMap::new())),
            plan_review_selected_label: Arc::new(Mutex::new(std::collections::HashMap::new())),
            session_approvals: Arc::new(Mutex::new(std::collections::HashSet::new())),
            prefix_approval_rules: Arc::new(Mutex::new(ApprovalRuleStore::default())),
            home_dir: None,
            session_directory: None,
            todos: Arc::new(Mutex::new(Vec::new())),
            background_tasks: BackgroundTaskManager::new(),
            manual_compact_request: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    #[must_use]
    pub fn with_system_prompt(mut self, system_prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(system_prompt.into());
        self
    }

    #[must_use]
    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = tools;
        self
    }

    #[must_use]
    pub const fn with_queue_modes(mut self, steering: QueueMode, follow_up: QueueMode) -> Self {
        self.steering_queue_mode = steering;
        self.follow_up_queue_mode = follow_up;
        self
    }

    #[must_use]
    pub const fn with_tool_execution_mode(mut self, mode: ToolExecutionMode) -> Self {
        self.tool_execution_mode = mode;
        self
    }

    #[must_use]
    pub fn with_permission_mode(mut self, mode: PermissionMode) -> Self {
        self.permission_mode = mode;
        if let Ok(mut live) = self.live_permission_mode.write() {
            *live = mode;
        }
        self
    }

    /// Replace the shared live permission state. The static `permission_mode`
    /// is seeded from the live value so they stay consistent at attachment time.
    #[must_use]
    pub fn with_live_permission_mode(
        mut self,
        live_permission_mode: Arc<RwLock<PermissionMode>>,
    ) -> Self {
        if let Ok(mode) = live_permission_mode.read().map(|guard| *guard) {
            self.permission_mode = mode;
        }
        self.live_permission_mode = live_permission_mode;
        self
    }

    pub fn with_workspace_root(
        mut self,
        workspace_root: impl Into<PathBuf>,
    ) -> Result<Self, std::io::Error> {
        self.workspace_root = Some(workspace_root.into().canonicalize()?);
        Ok(self)
    }

    #[must_use]
    pub const fn with_compaction(mut self, settings: CompactionSettings) -> Self {
        self.compaction = Some(settings);
        self
    }

    #[must_use]
    pub fn with_context_transform(
        mut self,
        transform: impl Fn(&[AgentMessage]) -> Vec<AgentMessage> + Send + Sync + 'static,
    ) -> Self {
        self.context_transform = Some(Arc::new(transform));
        self
    }

    #[must_use]
    pub fn with_before_tool_call(
        mut self,
        hook: impl Fn(&AgentToolCall) -> Option<ToolResult> + Send + Sync + 'static,
    ) -> Self {
        self.before_tool_call = Some(Arc::new(hook));
        self
    }

    #[must_use]
    pub fn with_async_before_tool_call<F, Fut>(mut self, hook: F) -> Self
    where
        F: Fn(AgentToolCall, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<ToolResult>> + Send + 'static,
    {
        self.async_before_tool_call = Some(Arc::new(move |call, cancel_token| {
            hook(call, cancel_token).boxed()
        }));
        self
    }

    #[must_use]
    pub fn with_after_tool_call(
        mut self,
        hook: impl Fn(&AgentToolCall, ToolResult) -> ToolResult + Send + Sync + 'static,
    ) -> Self {
        self.after_tool_call = Some(Arc::new(hook));
        self
    }

    #[must_use]
    pub fn with_async_after_tool_call<F, Fut>(mut self, hook: F) -> Self
    where
        F: Fn(AgentToolCall, ToolResult, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ToolResult> + Send + 'static,
    {
        self.async_after_tool_call = Some(Arc::new(move |call, result, cancel_token| {
            hook(call, result, cancel_token).boxed()
        }));
        self
    }

    #[must_use]
    pub fn with_approval_handler(
        mut self,
        handler: impl Fn(&ApprovalRequest) -> PermissionApprovalDecision + Send + Sync + 'static,
    ) -> Self {
        self.approval_handler = Some(Arc::new(handler));
        self
    }

    #[must_use]
    pub fn with_async_approval_handler<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(ApprovalRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = PermissionApprovalDecision> + Send + 'static,
    {
        self.async_approval_handler = Some(Arc::new(move |request| handler(request).boxed()));
        self
    }

    /// Set the home directory used for plan file creation.
    #[must_use]
    pub fn with_home_dir(mut self, home_dir: impl Into<PathBuf>) -> Self {
        self.home_dir = Some(home_dir.into());
        self
    }

    /// Set the session directory used for plan files and image blobs.
    #[must_use]
    pub fn with_session_directory(mut self, session_directory: impl Into<PathBuf>) -> Self {
        self.session_directory = Some(session_directory.into());
        self
    }

    /// Path to the persistent prefix-approval-rules file, when a home dir is set.
    #[must_use]
    pub fn approval_rules_path(&self) -> Option<PathBuf> {
        self.home_dir
            .as_ref()
            .map(|home| home.join("approval_rules.json"))
    }

    /// Load persistent Layer-2 prefix rules from `<home>/approval_rules.json`.
    /// Missing or malformed file is treated as an empty rule set (no error).
    pub fn load_prefix_approval_rules(&mut self) {
        let Some(path) = self.approval_rules_path() else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        match serde_json::from_str::<ApprovalRuleStore>(&text) {
            Ok(store) => {
                if let Ok(mut guard) = self.prefix_approval_rules.lock() {
                    *guard = store;
                }
            }
            Err(err) => {
                eprintln!(
                    "ignoring malformed approval rules at {}: {err}",
                    path.display()
                );
            }
        }
    }

    /// Persist the current Layer-2 prefix rules to `<home>/approval_rules.json`.
    /// Returns the path written, or `None` if no home dir is set or the write
    /// failed (a failed write is logged but does not interrupt the turn).
    #[must_use]
    pub fn save_prefix_approval_rules(&self) -> Option<PathBuf> {
        let path = self.approval_rules_path()?;
        let store = self
            .prefix_approval_rules
            .lock()
            .ok()
            .map(|guard| guard.clone())?;
        let text = serde_json::to_string_pretty(&store).ok()?;
        if let Some(parent) = path.parent()
            && std::fs::create_dir_all(parent).is_err()
        {
            eprintln!("failed to create dir for approval rules");
            return None;
        }
        if std::fs::write(&path, text).is_err() {
            eprintln!("failed to write approval rules at {}", path.display());
            return None;
        }
        Some(path)
    }

    /// Replace the shared plan-mode state. Useful when constructing from a
    /// pre-existing state (e.g. after replay).
    #[must_use]
    pub fn with_plan_mode(mut self, plan_mode: Arc<RwLock<PlanMode>>) -> Self {
        self.plan_mode = plan_mode;
        self
    }

    #[must_use]
    pub const fn with_goal_mode_authoring(mut self, active: bool) -> Self {
        self.goal_mode_authoring = active;
        self
    }

    /// Replace the shared todo list state.
    #[must_use]
    pub fn with_todos(mut self, todos: Arc<Mutex<Vec<TodoEventData>>>) -> Self {
        self.todos = todos;
        self
    }

    /// Replace the shared background task manager.
    #[must_use]
    pub fn with_background_tasks(mut self, background_tasks: BackgroundTaskManager) -> Self {
        self.background_tasks = background_tasks;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CompactionSettings {
    pub enabled: bool,
    pub max_estimated_tokens: usize,
    pub keep_recent_messages: usize,
    /// Fraction of `max_context_tokens` at which auto-compaction triggers.
    pub trigger_ratio: f64,
    /// Reserved headroom in tokens that forces compaction when
    /// `used + reserved >= max_context_tokens`.
    pub reserved_context_tokens: usize,
    /// Maximum recent messages to keep during auto-compaction.
    pub max_recent_messages: usize,
    /// Whether experimental micro compaction (old tool-result truncation) is on.
    pub micro_enabled: bool,
    /// Number of recent messages exempt from micro compaction.
    pub micro_keep_recent: usize,
}

impl CompactionSettings {
    #[must_use]
    pub const fn new(max_estimated_tokens: usize, keep_recent_messages: usize) -> Self {
        Self {
            enabled: true,
            max_estimated_tokens,
            keep_recent_messages,
            trigger_ratio: 0.85,
            reserved_context_tokens: 50_000,
            max_recent_messages: 4,
            micro_enabled: false,
            micro_keep_recent: 20,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentContext {
    // IMPORTANT: do not delete this comment unless the cancellation model is
    // intentionally redesigned and the regression test below is replaced with
    // an equivalent guard:
    // `runtime_resumed_cancelled_turn_accepts_followup_prompt`.
    //
    // Bug background, 2026-06-22:
    // `AgentContext` used to contain a persistent `cancelled: bool`. Replay of
    // any historical `TurnFinished { stop_reason: Cancelled }` set that flag,
    // and `run_turn_with_cancel` checked it before starting the next turn. That
    // made a resumed session permanently poisoned: after a user interrupted one
    // turn, every later prompt in that JSONL session immediately produced
    // `RunFinished(Cancelled)` without calling the model. The observed failure
    // was `neo resume session_0774471a-c613-40d3-b758-3ebfb3dc40d1`, where
    // turns 321 and 322 were cancelled as soon as the recalled prompt was sent.
    //
    // Cancellation is a property of the currently executing turn, carried by
    // that turn's `CancellationToken`. It is not durable session state. A
    // replayed cancelled turn must remain visible in the transcript and must
    // still advance the turn counter, but it must not affect future turns.
    pub(super) messages: Vec<AgentMessage>,
    pub(super) turns: u32,
    pub(super) steering_queue: Vec<AgentMessage>,
    pub(super) follow_up_queue: Vec<AgentMessage>,
    /// Skill context injected before the next user message in the current turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) skill_context: Option<AgentMessage>,
    pub(super) compaction_summary: Option<CompactionSummary>,
    /// Whether plan mode was active at the end of the last replayed/exected turn.
    #[serde(default)]
    pub(super) plan_mode_active: bool,
    /// The plan id from the last `PlanModeEntered` event, if any.
    #[serde(default)]
    pub(super) plan_mode_id: Option<String>,
    /// Latest todo list state, restored on resume replay.
    #[serde(default)]
    pub(super) todos: Vec<TodoEventData>,
}

impl AgentContext {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn messages(&self) -> &[AgentMessage] {
        &self.messages
    }

    #[must_use]
    pub fn estimated_context_tokens(&self) -> u32 {
        u32::try_from(estimate_messages_tokens(&self.messages)).unwrap_or(u32::MAX)
    }

    #[must_use]
    pub fn turns(&self) -> u32 {
        self.turns
    }

    pub fn append_message(&mut self, message: AgentMessage) {
        self.messages.push(message);
    }

    pub fn queue_steering_message(&mut self, message: AgentMessage) {
        self.steering_queue.push(message);
    }

    pub fn queue_follow_up_message(&mut self, message: AgentMessage) {
        self.follow_up_queue.push(message);
    }

    /// Set a skill context message to be inserted before the next user message.
    pub fn set_skill_context(&mut self, message: AgentMessage) {
        self.skill_context = Some(message);
    }

    /// Take the pending skill context message, if any.
    #[must_use]
    pub fn take_skill_context(&mut self) -> Option<AgentMessage> {
        self.skill_context.take()
    }

    pub fn apply_compaction(&mut self, summary: CompactionSummary) {
        let keep_from = summary.first_kept_message_index.min(self.messages.len());
        let mut kept = self.messages.split_off(keep_from);
        // Drop any trailing assistant-with-tool-calls whose results were
        // compacted away, so the retained tail is always provider-valid.
        kept = sanitize_tool_exchange_messages(kept);
        // Inject the LLM-generated summary as a system message so the model
        // has the compacted context when continuing the conversation.
        let summary_msg = AgentMessage::system_text(format!(
            "<compaction_summary>\nThe following is a summary of the earlier conversation, \
             compacted to preserve essential context:\n\n{}\n</compaction_summary>",
            summary.summary
        ));
        kept.insert(0, summary_msg);
        self.messages = kept;
        self.compaction_summary = Some(summary);
    }

    #[must_use]
    pub fn compaction_summary(&self) -> Option<&CompactionSummary> {
        self.compaction_summary.as_ref()
    }

    #[must_use]
    pub fn pending_steering_len(&self) -> usize {
        self.steering_queue.len()
    }

    #[must_use]
    pub fn pending_follow_up_len(&self) -> usize {
        self.follow_up_queue.len()
    }

    /// Whether plan mode is currently active (from replayed state).
    #[must_use]
    pub fn is_plan_mode_active(&self) -> bool {
        self.plan_mode_active
    }

    /// The plan id from the last replayed `PlanModeEntered` event, if any.
    #[must_use]
    pub fn plan_mode_id(&self) -> Option<&str> {
        self.plan_mode_id.as_deref()
    }

    /// Latest todo list from replayed state.
    #[must_use]
    pub fn todos(&self) -> &[TodoEventData] {
        &self.todos
    }

    #[must_use]
    pub fn from_replay<'a>(events: impl IntoIterator<Item = &'a AgentEvent>) -> Self {
        let mut context = Self::new();
        for event in events {
            context.apply_replay_event(event);
        }
        context.messages = sanitize_tool_exchange_messages(context.messages);
        context
    }

    fn apply_replay_event(&mut self, event: &AgentEvent) {
        if self.apply_replay_message_event(event) {
            return;
        }
        if self.apply_replay_queue_event(event) {
            return;
        }
        self.apply_replay_state_event(event);
    }

    fn apply_replay_message_event(&mut self, event: &AgentEvent) -> bool {
        match event {
            AgentEvent::MessageAppended { message } => self.append_message(message.clone()),
            AgentEvent::TurnFinished { turn, .. } => {
                // See the invariant on `AgentContext`: replayed cancellation is
                // historical transcript state only. Do not inspect
                // `stop_reason` here or reintroduce durable cancellation.
                self.turns = self.turns.max(*turn);
            }
            _ => return false,
        }
        true
    }

    fn apply_replay_queue_event(&mut self, event: &AgentEvent) -> bool {
        match event {
            AgentEvent::SteeringQueued { message } => {
                self.queue_steering_message(message.clone());
            }
            AgentEvent::FollowUpQueued { message } => {
                self.queue_follow_up_message(message.clone());
            }
            AgentEvent::QueueDrained { kind, count } => self.drain_replay_queue(*kind, *count),
            _ => return false,
        }
        true
    }

    fn drain_replay_queue(&mut self, kind: QueueKind, count: usize) {
        match kind {
            QueueKind::Steering => {
                let drain_count = count.min(self.steering_queue.len());
                self.steering_queue.drain(0..drain_count);
            }
            QueueKind::FollowUp => {
                let drain_count = count.min(self.follow_up_queue.len());
                self.follow_up_queue.drain(0..drain_count);
            }
        }
    }

    fn apply_replay_state_event(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::CompactionApplied { summary } => self.apply_compaction(summary.clone()),
            AgentEvent::PlanModeEntered { id, .. } => {
                self.plan_mode_active = true;
                self.plan_mode_id = Some(id.clone());
            }
            AgentEvent::PlanModeExited { .. } => {
                self.plan_mode_active = false;
            }
            AgentEvent::PlanUpdated { enabled, .. } => {
                self.plan_mode_active = *enabled;
            }
            AgentEvent::TodoUpdated { todos, .. } => self.todos.clone_from(todos),
            _ => {}
        }
    }
}

#[derive(Debug, Error)]
pub enum AgentRuntimeError {
    #[error("model stream failed: {0}")]
    Model(#[from] neo_ai::AiError),
    #[error("tool execution failed: {0}")]
    Tool(#[from] ToolError),
    #[error("runtime I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("compaction failed: {0}")]
    Compaction(#[from] compaction::CompactionError),
    #[error("turn cancelled")]
    Cancelled,
}

pub type AgentEventStream<'a> = stream::BoxStream<'a, Result<AgentEvent, AgentRuntimeError>>;

#[derive(Clone)]
pub struct AgentRuntime {
    config: AgentConfig,
    model: Arc<dyn ModelClient>,
    tools: Option<Arc<ToolRegistry>>,
    skills: Option<Arc<SkillStore>>,
    goal_manager: Option<Arc<GoalManager>>,
    steer_input: SteerInputHandle,
}

impl AgentRuntime {
    #[must_use]
    pub fn new(config: AgentConfig, model: Arc<dyn ModelClient>) -> Self {
        Self {
            config,
            model,
            tools: None,
            skills: None,
            goal_manager: None,
            steer_input: SteerInputHandle::new(),
        }
    }

    #[must_use]
    pub fn with_tools(
        config: AgentConfig,
        model: Arc<dyn ModelClient>,
        tools: ToolRegistry,
    ) -> Self {
        let mut config = config;
        config.tools = tools.specs();
        Self {
            config,
            model,
            tools: Some(Arc::new(tools)),
            skills: None,
            goal_manager: None,
            steer_input: SteerInputHandle::new(),
        }
    }

    #[must_use]
    pub fn with_tools_and_skills(
        mut config: AgentConfig,
        model: Arc<dyn ModelClient>,
        tools: ToolRegistry,
        skills: SkillStore,
    ) -> Self {
        let mut tool_specs = tools.specs();
        tool_specs.push(invoke_skill_tool_spec());
        config.tools = tool_specs;
        Self {
            config,
            model,
            tools: Some(Arc::new(tools)),
            skills: Some(Arc::new(skills)),
            goal_manager: None,
            steer_input: SteerInputHandle::new(),
        }
    }

    /// Attach a shared steer-input handle so the controller can push live
    /// input into a running turn. The runtime drains this handle at each
    /// step boundary and feeds it into the existing queue machinery.
    #[must_use]
    pub fn with_steer_input(mut self, steer_input: SteerInputHandle) -> Self {
        self.steer_input = steer_input;
        self
    }

    #[must_use]
    pub fn with_goal_manager(mut self, manager: &Arc<GoalManager>) -> Self {
        self.goal_manager = Some(Arc::clone(manager));
        self
    }

    pub fn tools_mut(&mut self) -> Option<&mut Arc<ToolRegistry>> {
        self.tools.as_mut()
    }

    #[must_use]
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Restore plan-mode state from a replayed context.
    pub fn restore_plan_mode(&self, context: &AgentContext) {
        if !context.is_plan_mode_active() {
            return;
        }
        let Some(id) = context.plan_mode_id() else {
            return;
        };
        let Some(plans_dir) = plan_mode_plans_dir(&self.config) else {
            return;
        };
        if let Ok(mut pm) = self.config.plan_mode.write() {
            pm.restore_enter(&plans_dir, id);
        }
    }

    pub fn run_turn<'a>(
        &'a self,
        context: &'a mut AgentContext,
        message: AgentMessage,
    ) -> AgentEventStream<'a> {
        self.run_turn_with_cancel(context, message, CancellationToken::new())
    }

    pub fn run_turn_with_cancel<'a>(
        &'a self,
        context: &'a mut AgentContext,
        message: AgentMessage,
        cancel_token: CancellationToken,
    ) -> AgentEventStream<'a> {
        if let Ok(mut todos) = self.config.todos.lock() {
            todos.clone_from(&context.todos);
        }

        let live_context = context.clone();
        let model = Arc::clone(&self.model);
        let tools = self.tools.clone();
        let skills = self.skills.clone();
        let goal_manager = self.goal_manager.clone();
        let config = self.config.clone();
        let steer_input = self.steer_input.clone();
        let process_supervisor = ProcessSupervisor::default();
        let (sender, receiver) = mpsc::unbounded_channel();
        let (final_sender, final_receiver) = oneshot::channel();

        tokio::spawn(async move {
            let mut emitter = EventEmitter::new(sender, live_context);
            emitter.emit(AgentEvent::RunStarted {
                turn: emitter.context.turns.saturating_add(1),
            });
            if let Some(skill_context) = emitter.context.take_skill_context() {
                emitter.emit(AgentEvent::MessageAppended {
                    message: skill_context,
                });
            }
            emitter.emit(AgentEvent::MessageAppended { message });
            if let Err(err) = run_agent_turn(
                model,
                config,
                tools,
                skills,
                goal_manager,
                steer_input,
                &mut emitter,
                cancel_token,
                process_supervisor.clone(),
            )
            .await
            {
                process_supervisor.cleanup_all().await;
                emitter.emit(AgentEvent::RunFinished {
                    turn: emitter.context.turns.saturating_add(1),
                    stop_reason: StopReason::Error,
                });
                let _ = emitter.send_error(err);
            }
            let _ = final_sender.send(emitter.context);
        });

        stream::unfold(
            SpawnedRun {
                receiver,
                final_receiver: Some(final_receiver),
                context,
            },
            |mut state| async move {
                if let Some(event) = state.receiver.recv().await {
                    if let Ok(event) = &event {
                        EventEmitter::apply_to_context(state.context, event);
                    }
                    return Some((event, state));
                }
                if let Some(final_receiver) = state.final_receiver.take()
                    && let Ok(final_context) = final_receiver.await
                {
                    *state.context = final_context;
                }
                None
            },
        )
        .boxed()
    }

    /// Run a compaction-only turn.  This does not append a user message and does
    /// not call the model afterwards; it simply executes any pending compaction
    /// (manual or automatic) and finishes.  Used by the TUI's `/compact` slash
    /// command when the session is idle.
    pub fn run_compaction_turn<'a>(
        &'a self,
        context: &'a mut AgentContext,
    ) -> AgentEventStream<'a> {
        self.run_compaction_turn_with_cancel(context, CancellationToken::new())
    }

    /// Run a compaction-only turn with an external cancellation token.
    pub fn run_compaction_turn_with_cancel<'a>(
        &'a self,
        context: &'a mut AgentContext,
        cancel_token: CancellationToken,
    ) -> AgentEventStream<'a> {
        let live_context = context.clone();
        let model = Arc::clone(&self.model);
        let config = self.config.clone();
        let process_supervisor = ProcessSupervisor::default();
        let (sender, receiver) = mpsc::unbounded_channel();
        let (final_sender, final_receiver) = oneshot::channel();

        tokio::spawn(async move {
            let mut emitter = EventEmitter::new(sender, live_context);
            let turn = emitter.context.turns.saturating_add(1);
            emitter.emit(AgentEvent::RunStarted { turn });
            maybe_compact(&model, &config, &mut emitter, &cancel_token).await;
            process_supervisor.cleanup_all().await;
            emit_run_finished(&config, &mut emitter, turn, StopReason::EndTurn).await;
            let _ = final_sender.send(emitter.context);
        });

        stream::unfold(
            SpawnedRun {
                receiver,
                final_receiver: Some(final_receiver),
                context,
            },
            |mut state| async move {
                if let Some(event) = state.receiver.recv().await {
                    if let Ok(event) = &event {
                        EventEmitter::apply_to_context(state.context, event);
                    }
                    return Some((event, state));
                }
                if let Some(final_receiver) = state.final_receiver.take()
                    && let Ok(final_context) = final_receiver.await
                {
                    *state.context = final_context;
                }
                None
            },
        )
        .boxed()
    }
}

struct SpawnedRun<'a> {
    receiver: mpsc::UnboundedReceiver<Result<AgentEvent, AgentRuntimeError>>,
    final_receiver: Option<oneshot::Receiver<AgentContext>>,
    context: &'a mut AgentContext,
}

/// Compute the session-scoped plans directory (`<session_dir>/plans`).
fn plan_mode_plans_dir(config: &AgentConfig) -> Option<PathBuf> {
    config
        .session_directory
        .as_deref()
        .map(|dir| dir.join("plans"))
}

async fn chat_request(config: &AgentConfig, context: &AgentContext) -> ChatRequest {
    let mut messages = Vec::new();
    if let Some(system_prompt) = &config.system_prompt {
        messages.push(AgentMessage::system_text(system_prompt).to_chat_message());
    }
    if let Some(workspace_context) = workspace_context_message(config) {
        messages.push(workspace_context.to_chat_message());
    }
    if config.goal_mode_authoring {
        messages.push(goal_mode_authoring_message().to_chat_message());
    }
    let context_messages = if let Some(transform) = &config.context_transform {
        transform(context.messages())
    } else {
        context.messages.clone()
    };
    // Resolve blob references to inline base64 before sending to the provider.
    let context_messages =
        resolve_image_blobs(context_messages, config.session_directory.as_deref()).await;
    // Apply micro compaction (experimental): truncate old, large tool results
    // to reclaim context tokens without a full LLM-driven compaction.
    let context_messages = if config
        .compaction
        .is_some_and(|settings| settings.micro_enabled)
    {
        let settings = config.compaction.expect("checked above");
        crate::compaction::micro::apply_micro_compaction(
            &context_messages,
            &crate::compaction::micro::MicroCompactionConfig {
                keep_recent_messages: settings.micro_keep_recent,
                ..crate::compaction::micro::MicroCompactionConfig::default()
            },
        )
    } else {
        context_messages
    };
    // Never send a provider request with an assistant message that has pending
    // tool_calls but no matching tool results.  This guards against incomplete
    // trailing tool turns and against compaction boundaries that accidentally
    // orphan such a message.
    let context_messages = sanitize_tool_exchange_messages(context_messages);
    messages.extend(context_messages.iter().map(|message| {
        if config.replay_reasoning {
            message.to_chat_message()
        } else {
            without_reasoning_content(message.to_chat_message())
        }
    }));
    let mut injector = PlanModeInjector::new(Arc::clone(&config.plan_mode));
    if let Some(injected) = injector.inject(context) {
        messages.push(injected.to_chat_message());
    }
    ChatRequest {
        model: config.model.clone(),
        messages,
        tools: config.tools.clone(),
        options: RequestOptions {
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            reasoning_effort: config.reasoning_effort,
            replay_reasoning: config.replay_reasoning,
            ..RequestOptions::default()
        },
    }
}

fn goal_mode_authoring_message() -> AgentMessage {
    AgentMessage::system_text(
        "Goal mode is active. Do not start a durable goal directly with StartGoal. \
         First draft a structured goal with objective, acceptance criteria, phase plan, risks/assumptions, and validation commands. \
         Then call ExitGoalMode with the reviewed objective, completion_criterion, and ordered phases so the user can Accept, Reject, or Revise it in a blocking dialog."
            .to_owned(),
    )
}

fn workspace_context_message(config: &AgentConfig) -> Option<AgentMessage> {
    let workspace_root = config.workspace_root.as_ref()?;
    Some(AgentMessage::system_text(format!(
        "<environment_context>\n<cwd>{}</cwd>\n</environment_context>\n\nShell tools already run in this workspace. Do not prefix shell commands with `cd <cwd> &&`; use the bash `cwd` field for a workspace subdirectory.",
        workspace_root.display()
    )))
}

fn without_reasoning_content(message: ChatMessage) -> ChatMessage {
    match message {
        ChatMessage::System { content } => ChatMessage::System {
            content: filter_reasoning(content),
        },
        ChatMessage::User { content } => ChatMessage::User {
            content: filter_reasoning(content),
        },
        ChatMessage::Assistant {
            content,
            tool_calls,
        } => ChatMessage::Assistant {
            content: filter_reasoning(content),
            tool_calls,
        },
        ChatMessage::ToolResult {
            tool_call_id,
            content,
            is_error,
        } => ChatMessage::ToolResult {
            tool_call_id,
            content: filter_reasoning(content),
            is_error,
        },
    }
}

fn filter_reasoning(content: Vec<neo_ai::ContentPart>) -> Vec<neo_ai::ContentPart> {
    content
        .into_iter()
        .filter(|part| !matches!(part, neo_ai::ContentPart::Thinking { .. }))
        .collect()
}

fn validate_model_capabilities(request: &ChatRequest) -> Result<(), AiError> {
    let capabilities = &request.model.capabilities;
    if !request.tools.is_empty() && !capabilities.tools {
        return Err(AiError::Configuration(format!(
            "model {}/{} does not support tools",
            request.model.provider.0, request.model.model
        )));
    }
    if request.options.reasoning_effort.is_some() && !capabilities.reasoning {
        return Err(AiError::Configuration(format!(
            "model {}/{} does not support reasoning",
            request.model.provider.0, request.model.model
        )));
    }
    if request_messages_contain_image(&request.messages) && !capabilities.images {
        return Err(AiError::Configuration(format!(
            "model {}/{} does not support image input",
            request.model.provider.0, request.model.model
        )));
    }
    Ok(())
}

fn request_messages_contain_image(messages: &[ChatMessage]) -> bool {
    messages.iter().any(|message| {
        let content = match message {
            ChatMessage::System { content }
            | ChatMessage::User { content }
            | ChatMessage::Assistant { content, .. }
            | ChatMessage::ToolResult { content, .. } => content,
        };
        content
            .iter()
            .any(|part| matches!(part, ContentPart::Image { .. }))
    })
}

pub(super) struct EventEmitter {
    pub(super) sender: mpsc::UnboundedSender<Result<AgentEvent, AgentRuntimeError>>,
    pub(super) context: AgentContext,
    pub(super) last_context_window_tokens: Option<u32>,
}

impl EventEmitter {
    pub(super) fn new(
        sender: mpsc::UnboundedSender<Result<AgentEvent, AgentRuntimeError>>,
        context: AgentContext,
    ) -> Self {
        Self {
            sender,
            context,
            last_context_window_tokens: None,
        }
    }

    pub(super) fn emit(&mut self, event: AgentEvent) {
        Self::apply_to_context(&mut self.context, &event);
        let _ = self.sender.send(Ok(event));
    }

    pub(super) fn sink(&self) -> EventSink {
        EventSink {
            sender: self.sender.clone(),
        }
    }

    pub(super) fn send_error(&mut self, err: AgentRuntimeError) -> Result<(), AgentRuntimeError> {
        self.sender
            .send(Err(err))
            .map_err(|_| AgentRuntimeError::Cancelled)
    }

    pub(super) fn apply_to_context(context: &mut AgentContext, event: &AgentEvent) {
        match event {
            AgentEvent::MessageAppended { message } => context.append_message(message.clone()),
            AgentEvent::TurnFinished { turn, .. } => {
                // Same invariant as replay: even live cancelled turns must not
                // poison the context used by subsequent user prompts.
                context.turns = context.turns.max(*turn);
            }
            AgentEvent::SteeringQueued { message } => {
                context.queue_steering_message(message.clone());
            }
            AgentEvent::FollowUpQueued { message } => {
                context.queue_follow_up_message(message.clone());
            }
            AgentEvent::QueueDrained { kind, count } => match kind {
                QueueKind::Steering => {
                    let drain_count = (*count).min(context.steering_queue.len());
                    context.steering_queue.drain(0..drain_count);
                }
                QueueKind::FollowUp => {
                    let drain_count = (*count).min(context.follow_up_queue.len());
                    context.follow_up_queue.drain(0..drain_count);
                }
            },
            AgentEvent::CompactionApplied { summary } => {
                context.apply_compaction(summary.clone());
            }
            AgentEvent::PlanModeEntered { id, .. } => {
                context.plan_mode_active = true;
                context.plan_mode_id = Some(id.clone());
            }
            AgentEvent::PlanModeExited { .. } => {
                context.plan_mode_active = false;
            }
            AgentEvent::PlanUpdated { enabled, .. } => {
                context.plan_mode_active = *enabled;
            }
            AgentEvent::TodoUpdated { todos, .. } => {
                context.todos.clone_from(todos);
            }
            _ => {}
        }
    }
}

pub(super) trait EventPublisher {
    fn emit(&mut self, event: AgentEvent);
}

impl EventPublisher for EventEmitter {
    fn emit(&mut self, event: AgentEvent) {
        Self::emit(self, event);
    }
}

#[derive(Clone)]
pub(super) struct EventSink {
    pub(super) sender: mpsc::UnboundedSender<Result<AgentEvent, AgentRuntimeError>>,
}

impl EventSink {
    /// Emit an event by value without needing `&mut self`.
    pub(super) fn emit_event(&self, event: AgentEvent) {
        let _ = self.sender.send(Ok(event));
    }
}

impl EventPublisher for EventSink {
    fn emit(&mut self, event: AgentEvent) {
        self.emit_event(event);
    }
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
async fn run_agent_turn(
    model: Arc<dyn ModelClient>,
    config: AgentConfig,
    tools: Option<Arc<ToolRegistry>>,
    skills: Option<Arc<SkillStore>>,
    goal_manager: Option<Arc<GoalManager>>,
    steer_input: SteerInputHandle,
    emitter: &mut EventEmitter,
    cancel_token: CancellationToken,
    process_supervisor: ProcessSupervisor,
) -> Result<(), AgentRuntimeError> {
    let mut final_turn: u32;
    let mut final_stop_reason = StopReason::EndTurn;
    drain_live_steer_input(&steer_input, emitter);
    let mut pending_messages = drain_steering_queue(&config, emitter);

    loop {
        if !pending_messages.is_empty() {
            append_queued_messages(emitter, pending_messages);
        }

        maybe_compact(&model, &config, emitter, &cancel_token).await;

        if let Some((turn, stop_reason)) = terminal_pre_model_stop(emitter, &cancel_token) {
            final_turn = turn;
            final_stop_reason = stop_reason;
            break;
        }

        let turn = emitter.context.turns.saturating_add(1);
        let request = chat_request(&config, &emitter.context).await;
        emit_context_window_update(
            emitter,
            turn,
            estimate_chat_messages_tokens(&request.messages),
        );
        validate_model_capabilities(&request)?;
        let assistant = run_model_turn(
            Arc::clone(&model),
            &config,
            request,
            turn,
            emitter,
            cancel_token.clone(),
        )
        .await?;
        final_turn = turn;
        if let Some(AgentMessage::Assistant { stop_reason, .. }) = &assistant {
            final_stop_reason = *stop_reason;
        }

        let Some(AgentMessage::Assistant {
            tool_calls: model_tool_calls,
            stop_reason: StopReason::ToolUse,
            ..
        }) = assistant.clone()
        else {
            drain_live_steer_input(&steer_input, emitter);
            if let Some(messages) =
                next_pending_after_assistant(&config, emitter, goal_manager.as_deref())
            {
                pending_messages = messages;
                continue;
            }
            break;
        };
        let tool_calls = model_tool_calls.clone();

        let Some(registry) = &tools else {
            break;
        };
        let mut tool_results = execute_tool_calls(
            &config,
            registry,
            skills.as_deref(),
            turn,
            &tool_calls,
            emitter,
            &cancel_token,
            &process_supervisor,
        )
        .await?;
        if cancel_token.is_cancelled() {
            emitter.emit(AgentEvent::TurnFinished {
                turn,
                stop_reason: StopReason::Cancelled,
            });
            final_stop_reason = StopReason::Cancelled;
            break;
        }
        // Attach plan details + the selected-option prefix BEFORE appending the
        // tool results to the context so the next model turn sees the prefix,
        // and before the side-effect events flip plan mode off.
        attach_exit_plan_details(&config, &mut tool_results);
        // For EnterPlanMode: create the plan file and inject its path into the
        // tool result so the model knows where to write. Must happen before
        // append_tool_result_messages and before the duplicate enter in
        // emit_tool_side_effect_events.
        let has_enter_plan_mode = tool_results
            .iter()
            .any(|(tc, _)| tc.name == "EnterPlanMode");
        if has_enter_plan_mode {
            enter_plan_mode_state(&config);
            attach_enter_plan_details(&config, &mut tool_results);
        }
        append_tool_result_messages(&tool_results, emitter);
        emit_effective_context_window(&config, emitter, turn).await;
        emit_tool_side_effect_events(turn, &config, &tool_results, emitter);
        drain_live_steer_input(&steer_input, emitter);
        if terminates_tool_batch(&tool_results) {
            if continues_after_terminating_batch(&tool_results) {
                pending_messages = drain_steering_queue(&config, emitter);
                continue;
            }
            break;
        }
        pending_messages = drain_steering_queue(&config, emitter);
    }

    process_supervisor.cleanup_all().await;
    emit_run_finished(&config, emitter, final_turn, final_stop_reason).await;
    Ok(())
}

fn next_pending_after_assistant(
    config: &AgentConfig,
    emitter: &mut EventEmitter,
    goal_manager: Option<&GoalManager>,
) -> Option<Vec<AgentMessage>> {
    let pending_messages = drain_next_pending_queue(config, emitter);
    if pending_messages.is_empty() {
        goal_continuation_messages(goal_manager)
    } else {
        Some(pending_messages)
    }
}

fn append_tool_result_messages(
    tool_results: &[(AgentToolCall, ToolResult)],
    emitter: &mut EventEmitter,
) {
    for (tool_call, result) in tool_results {
        let message = AgentMessage::tool_result(
            tool_call.id.clone(),
            tool_call.name.clone(),
            vec![Content::text(result.content.clone())],
            result.is_error,
        );
        emitter.emit(AgentEvent::MessageAppended { message });
    }
}

fn attach_exit_plan_details(
    config: &AgentConfig,
    tool_results: &mut [(AgentToolCall, ToolResult)],
) {
    let pm = config.plan_mode.read().unwrap();
    if !pm.is_active() {
        return;
    }
    let Some(plan_data) = pm.data().ok().flatten() else {
        return;
    };
    let mut selected_labels = config.plan_review_selected_label.lock().ok();
    for (tool_call, result) in tool_results {
        if tool_call.name == "ExitPlanMode" {
            if result.details.is_none() {
                result.details = Some(serde_json::json!({
                    "plan_content": plan_data.content,
                    "plan_path": plan_data.path.display().to_string(),
                }));
            }
            // When the user approved a specific model-supplied option from
            // the plan-review picker, prefix the tool result so the model runs
            // only the selected branch. The label is consumed once.
            if !result.is_error
                && let Some(labels) = selected_labels.as_mut()
                && let Some(label) = labels.remove(&tool_call.id)
                && !label.trim().is_empty()
            {
                result.content = format!(
                    "Selected approach: {label}\n\
                     Execute ONLY the selected approach. Do not execute any unselected alternatives.\n\n{}",
                    result.content
                );
                if let Some(details) = result.details.as_mut()
                    && let Some(obj) = details.as_object_mut()
                {
                    obj.insert(
                        "plan_selected_label".to_string(),
                        serde_json::Value::String(label),
                    );
                }
            }
        }
    }
}

fn emit_tool_side_effect_events(
    turn: u32,
    config: &AgentConfig,
    tool_results: &[(AgentToolCall, ToolResult)],
    emitter: &mut EventEmitter,
) {
    for (tool_call, result) in tool_results {
        emit_plan_tool_event(turn, config, tool_call.name.as_str(), result, emitter);
        emit_todo_event(turn, config, tool_call.name.as_str(), result, emitter);
        emit_goal_event_from_result(turn, tool_call.name.as_str(), result, emitter);
    }
}

fn emit_plan_tool_event(
    turn: u32,
    config: &AgentConfig,
    tool_name: &str,
    result: &ToolResult,
    emitter: &mut EventEmitter,
) {
    if !result.terminate {
        return;
    }
    match tool_name {
        "EnterPlanMode" => emit_plan_mode_entered(turn, config, emitter),
        "ExitPlanMode" => emit_plan_mode_exited(turn, config, emitter),
        _ => {}
    }
}

fn emit_plan_mode_entered(turn: u32, config: &AgentConfig, emitter: &mut EventEmitter) {
    // If plan mode is already active (the enter side-effect was executed
    // earlier in the turn via `enter_plan_mode_state`), skip the duplicate
    // enter and just emit the events with the existing id.
    let id = {
        let pm = config.plan_mode.read().unwrap();
        if pm.is_active() {
            pm.plan_id().unwrap_or("").to_owned()
        } else {
            drop(pm);
            enter_plan_mode_state(config)
        }
    };
    emitter.emit(AgentEvent::PlanModeEntered {
        turn,
        id: id.clone(),
    });
    emitter.emit(AgentEvent::PlanUpdated {
        turn,
        enabled: true,
    });
}

/// Execute the plan-mode enter side-effect (create plan file, set active state)
/// and return the plan id. Extracted so [`attach_enter_plan_details`] can run
/// the side-effect *before* tool results are appended to the model context.
fn enter_plan_mode_state(config: &AgentConfig) -> String {
    let mut pm = config.plan_mode.write().unwrap();
    if let Some(plans_dir) = plan_mode_plans_dir(config) {
        pm.enter(&plans_dir, true).map_or_else(
            |_| {
                pm.enter_in_memory();
                pm.plan_id().unwrap_or("").to_owned()
            },
            |data| data.id,
        )
    } else {
        pm.enter_in_memory();
        pm.plan_id().unwrap_or("").to_owned()
    }
}

/// After EnterPlanMode runs and the plan file has been created, inject the plan
/// file path into the tool result so the model knows where to write its plan.
/// Must run *after* [`enter_plan_mode_state`] and *before*
/// [`append_tool_result_messages`].
fn attach_enter_plan_details(
    config: &AgentConfig,
    tool_results: &mut [(AgentToolCall, ToolResult)],
) {
    let plan_path = config
        .plan_mode
        .read()
        .ok()
        .and_then(|pm| pm.plan_file_path().map(|p| p.display().to_string()));
    let Some(plan_path) = plan_path else {
        return;
    };
    for (tool_call, result) in tool_results {
        if tool_call.name == "EnterPlanMode" && !result.is_error {
            result.content = format!(
                "{}\n\nThe plan file is at: {plan_path}\nWrite your plan to this file.",
                result.content
            );
        }
    }
}

fn emit_plan_mode_exited(turn: u32, config: &AgentConfig, emitter: &mut EventEmitter) {
    let mut pm = config.plan_mode.write().unwrap();
    let id = pm.plan_id().unwrap_or("").to_owned();
    pm.exit();
    drop(pm);
    emitter.emit(AgentEvent::PlanModeExited { turn, id });
    emitter.emit(AgentEvent::PlanUpdated {
        turn,
        enabled: false,
    });
}

fn emit_todo_event(
    turn: u32,
    config: &AgentConfig,
    tool_name: &str,
    result: &ToolResult,
    emitter: &mut EventEmitter,
) {
    if tool_name != "TodoList" || result.is_error {
        return;
    }
    let Some(details) = &result.details else {
        return;
    };
    let Some(todos_val) = details.get("todos") else {
        return;
    };
    let Ok(todos) = serde_json::from_value::<Vec<TodoEventData>>(todos_val.clone()) else {
        return;
    };
    if let Ok(mut shared) = config.todos.lock() {
        shared.clone_from(&todos);
    }
    emitter.emit(AgentEvent::TodoUpdated { turn, todos });
}

fn goal_continuation_messages(manager: Option<&GoalManager>) -> Option<Vec<AgentMessage>> {
    let manager = manager?;
    let goal = manager.active()?;
    let objective = goal.objective;
    let artifact = goal.artifact_dir.as_ref().map_or_else(
        || "(no artifact directory)".to_owned(),
        |path| path.display().to_string(),
    );
    let phase = goal
        .current_phase
        .and_then(|index| goal.phases.get(index).cloned())
        .unwrap_or_else(|| "No current phase recorded.".to_owned());
    Some(vec![AgentMessage::system_text(format!(
        "Goal still active: {objective}. Continue making progress using the goal artifacts.\n\n\
         Artifact directory: {artifact}\n\
         Current phase: {phase}\n\n\
         Work phase by phase. On repeated failures, retry once, write a focused fix spec on the second failure, and report blocked with handoff details on the third. Run a final audit before marking complete. \
         Use `UpdateGoalStatus` when the goal is complete or blocked, or `GetGoalStatus` to check current state."
    ))])
}

fn emit_goal_event_from_result(
    turn: u32,
    tool_name: &str,
    result: &ToolResult,
    emitter: &mut EventEmitter,
) {
    if result.is_error {
        return;
    }
    let Some(details) = &result.details else {
        return;
    };
    if details.get("kind").and_then(serde_json::Value::as_str) != Some("goal") {
        return;
    }
    let Some(objective) = details
        .get("objective")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
    else {
        return;
    };
    match (
        tool_name,
        details.get("event").and_then(serde_json::Value::as_str),
        details.get("status").and_then(serde_json::Value::as_str),
    ) {
        ("StartGoal" | "ExitGoalMode", Some("started"), _) => {
            emitter.emit(AgentEvent::GoalStarted { turn, objective });
        }
        ("UpdateGoalStatus", Some("updated"), Some("paused")) => {
            emitter.emit(AgentEvent::GoalPaused { turn, objective });
        }
        ("UpdateGoalStatus", Some("updated"), Some("active")) => {
            emitter.emit(AgentEvent::GoalResumed { turn, objective });
        }
        ("UpdateGoalStatus", Some("updated"), Some("blocked")) => {
            let reason = details
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("blocked")
                .to_owned();
            emitter.emit(AgentEvent::GoalBlocked {
                turn,
                objective,
                reason,
            });
        }
        ("UpdateGoalStatus", Some("updated"), Some("complete")) => {
            emitter.emit(AgentEvent::GoalFinished {
                turn,
                objective,
                outcome: "complete".to_owned(),
            });
        }
        _ => {}
    }
}

async fn emit_run_finished(
    config: &AgentConfig,
    emitter: &mut EventEmitter,
    turn: u32,
    stop_reason: StopReason,
) {
    emit_effective_context_window(config, emitter, turn).await;
    emitter.emit(AgentEvent::RunFinished { turn, stop_reason });
}

async fn emit_effective_context_window(
    config: &AgentConfig,
    emitter: &mut EventEmitter,
    turn: u32,
) {
    let request = chat_request_for_context_estimate(config, &emitter.context).await;
    emit_context_window_update(
        emitter,
        turn,
        estimate_chat_messages_tokens(&request.messages),
    );
}

async fn chat_request_for_context_estimate(
    config: &AgentConfig,
    context: &AgentContext,
) -> ChatRequest {
    let mut config = config.clone();
    let plan_mode = config
        .plan_mode
        .read()
        .expect("plan mode lock poisoned")
        .clone();
    config.plan_mode = Arc::new(RwLock::new(plan_mode));
    chat_request(&config, context).await
}

fn emit_context_window_update(emitter: &mut EventEmitter, turn: u32, used_tokens: usize) {
    let used_tokens = u32::try_from(used_tokens).unwrap_or(u32::MAX);
    if emitter.last_context_window_tokens == Some(used_tokens) {
        return;
    }
    emitter.last_context_window_tokens = Some(used_tokens);
    emitter.emit(AgentEvent::ContextWindowUpdated { turn, used_tokens });
}

fn terminal_pre_model_stop(
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
) -> Option<(u32, StopReason)> {
    if cancel_token.is_cancelled() {
        let turn = emitter.context.turns.saturating_add(1);
        emitter.emit(AgentEvent::TurnFinished {
            turn,
            stop_reason: StopReason::Cancelled,
        });
        return Some((turn, StopReason::Cancelled));
    }

    None
}

fn append_queued_messages(emitter: &mut EventEmitter, messages: Vec<AgentMessage>) {
    for message in messages {
        emitter.emit(AgentEvent::MessageAppended { message });
    }
}

/// Run compaction if needed.  Replaces the old counter-based logic with an
/// LLM-driven structured summary (see `compaction` module).
///
/// Compaction is triggered when:
/// - `manual_compact_request` is set (from `/compact`), or
/// - the token estimate exceeds the strategy threshold.
///
/// The LLM call runs inline (blocking the turn) and hard-fails on any error.
async fn maybe_compact(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
) {
    let Some(trigger) = evaluate_compaction_need(config, emitter) else {
        return;
    };

    let compacted_count = compute_compacted_count(&trigger);
    if compacted_count == 0 {
        if trigger.force {
            let _ = emitter.send_error(AgentRuntimeError::Compaction(
                compaction::CompactionError::NoBoundary,
            ));
        }
        return;
    }

    let (messages_to_compact, target_summary_chars) = emit_compaction_started(
        emitter,
        &trigger.messages,
        compacted_count,
        trigger.force,
        trigger.used_tokens,
    )
    .await;

    let (mut progress_rx, mut summary_rx) = spawn_summary_task(
        model,
        config,
        &messages_to_compact,
        trigger.custom_instruction.as_deref(),
        cancel_token,
    );

    let Some((summary_text, progress_percent)) = run_summary_progress_loop(
        emitter,
        &mut progress_rx,
        &mut summary_rx,
        target_summary_chars,
    )
    .await
    else {
        return;
    };

    apply_compaction_result(
        emitter,
        config,
        &trigger.messages,
        compacted_count,
        summary_text,
        trigger.used_tokens,
        progress_percent,
    )
    .await;
}

/// Bundled information produced by [`evaluate_compaction_need`] when compaction
/// should proceed.
struct CompactionTrigger {
    force: bool,
    custom_instruction: Option<String>,
    messages: Vec<AgentMessage>,
    used_tokens: usize,
    max_context_tokens: usize,
    settings: CompactionSettings,
}

/// Build the [`CompactionStrategy`] derived from [`CompactionSettings`].
fn build_compaction_strategy(settings: &CompactionSettings) -> CompactionStrategy {
    CompactionStrategy {
        trigger_ratio: settings.trigger_ratio,
        // Use keep_recent_messages as the auto-compaction retention limit so
        // the configured value directly controls how many messages survive.
        max_recent_messages: settings
            .keep_recent_messages
            .min(settings.max_recent_messages),
        max_recent_size_ratio: 0.2,
        reserved_context_tokens: settings.reserved_context_tokens,
    }
}

/// Evaluate whether compaction should run based on settings, a manual request,
/// and token thresholds. Returns `None` when no compaction is needed.
fn evaluate_compaction_need(
    config: &AgentConfig,
    emitter: &EventEmitter,
) -> Option<CompactionTrigger> {
    let settings = config.compaction?;
    if !settings.enabled {
        return None;
    }

    let (force, custom_instruction) = match config.manual_compact_request.lock() {
        Ok(mut request) => {
            let instruction = request.take();
            (instruction.is_some(), instruction)
        }
        Err(poisoned) => {
            let instruction = poisoned.into_inner().take();
            (instruction.is_some(), instruction)
        }
    };

    // Clone the messages out of the context so we can borrow `emitter` mutably
    // for event emission while still referencing the pre-compaction history.
    // Drop any trailing incomplete tool turn first so it is not treated as a
    // safe suffix boundary.
    let messages = sanitize_tool_exchange_messages(emitter.context.messages().to_vec());
    let max_context_tokens = config.model.capabilities.max_context_tokens.unwrap_or(0) as usize;
    let used_tokens = compaction::estimate_messages_tokens(&messages);

    let strategy = build_compaction_strategy(&settings);

    // Trigger compaction when:
    // 1. Manually requested via `/compact`, OR
    // 2. Token estimate exceeds the configured absolute threshold, OR
    // 3. Token estimate exceeds the ratio-based threshold of max_context_tokens.
    let ratio_triggered = strategy.should_compact(used_tokens, max_context_tokens);
    let absolute_triggered = used_tokens > settings.max_estimated_tokens;
    if !force && !ratio_triggered && !absolute_triggered {
        return None;
    }

    Some(CompactionTrigger {
        force,
        custom_instruction,
        messages,
        used_tokens,
        max_context_tokens,
        settings,
    })
}

/// Compute how many leading messages to compact. Only applies the fit-to-window
/// constraint when the model actually advertises a context window.
fn compute_compacted_count(trigger: &CompactionTrigger) -> usize {
    let source = if trigger.force {
        CompactionSource::Manual
    } else {
        CompactionSource::Auto
    };
    let strategy = build_compaction_strategy(&trigger.settings);
    compaction::compute_compact_count(
        &trigger.messages,
        source,
        &strategy,
        // Only apply the fit-to-window constraint when the model actually
        // advertises a context window. The trigger threshold
        // (max_estimated_tokens) is NOT the window — it's the compaction
        // trigger point — so passing it as the fit window would shrink
        // compaction to near-zero.
        trigger.max_context_tokens,
    )
}

/// Emit `CompactionStarted` and the early progress phases, then compute the
/// messages to compact and the target summary size.
async fn emit_compaction_started(
    emitter: &mut EventEmitter,
    messages: &[AgentMessage],
    compacted_count: usize,
    force: bool,
    used_tokens: usize,
) -> (Vec<AgentMessage>, usize) {
    let reason = if force {
        CompactionReason::Manual
    } else {
        CompactionReason::Threshold
    };
    let message_count = messages.len();
    emitter.emit(AgentEvent::CompactionStarted {
        reason,
        tokens_before: used_tokens,
        message_count,
    });
    emitter.emit(AgentEvent::CompactionProgress {
        phase: CompactionPhase::Estimating,
        percent: 0,
    });

    // Brief pause so the near-instant Estimating phase is visible in the TUI.
    tokio::time::sleep(Duration::from_millis(120)).await;

    let messages_to_compact = messages[..compacted_count].to_vec();
    emitter.emit(AgentEvent::CompactionProgress {
        phase: CompactionPhase::SelectingBoundary,
        percent: 15,
    });

    // Brief pause so SelectingBoundary is visible too.
    tokio::time::sleep(Duration::from_millis(120)).await;

    emitter.emit(AgentEvent::CompactionProgress {
        phase: CompactionPhase::Summarizing,
        percent: 15,
    });

    // Estimate the target summary size from the rendered input so the progress
    // bar can advance proportionally to the streaming LLM output, similar to
    // kimi-code's swarm progress estimator.
    let rendered_input_chars = compaction::render_messages_to_text(&messages_to_compact).len();
    let target_summary_chars = (rendered_input_chars / 10).max(500);

    (messages_to_compact, target_summary_chars)
}

/// Spawn the summary LLM in its own task and return the progress and result
/// channels.
#[allow(clippy::type_complexity)]
fn spawn_summary_task(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    messages_to_compact: &[AgentMessage],
    custom_instruction: Option<&str>,
    cancel_token: &CancellationToken,
) -> (
    mpsc::UnboundedReceiver<usize>,
    oneshot::Receiver<Result<String, compaction::CompactionError>>,
) {
    let (progress_tx, progress_rx) = mpsc::unbounded_channel::<usize>();
    let (summary_tx, summary_rx) =
        oneshot::channel::<Result<String, compaction::CompactionError>>();
    let summary_model = Arc::clone(model);
    let summary_config = config.clone();
    let summary_messages = messages_to_compact.to_vec();
    let summary_instruction = custom_instruction.map(str::to_owned);
    let summary_cancel = cancel_token.child_token();
    tokio::spawn(async move {
        let result = compaction::generate_compaction_summary(
            &summary_model,
            &summary_config,
            &summary_messages,
            summary_instruction.as_deref(),
            &summary_cancel,
            |summary_chars| {
                let _ = progress_tx.send(summary_chars);
            },
        )
        .await;
        let _ = summary_tx.send(result);
    });
    (progress_rx, summary_rx)
}

/// Drive the progress bar while waiting for the summary LLM to complete.
/// Returns `None` (after sending an error) on failure, or `Some((text,
/// progress_percent))` on success.
async fn run_summary_progress_loop(
    emitter: &mut EventEmitter,
    progress_rx: &mut mpsc::UnboundedReceiver<usize>,
    summary_rx: &mut oneshot::Receiver<Result<String, compaction::CompactionError>>,
    target_summary_chars: usize,
) -> Option<(String, u8)> {
    let mut progress_percent: u8 = 15;
    let mut last_emitted_percent = progress_percent;
    let mut progress_tick = tokio::time::interval(Duration::from_millis(200));
    progress_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Skip the immediate first tick.
    let _ = progress_tick.tick().await;

    let summary_text: String;
    loop {
        tokio::select! {
            _ = progress_tick.tick() => {
                // Keep the bar inching forward even if the model stalls.
                if progress_percent < 84 {
                    progress_percent += 1;
                    if progress_percent != last_emitted_percent {
                        last_emitted_percent = progress_percent;
                        emitter.emit(AgentEvent::CompactionProgress {
                            phase: CompactionPhase::Summarizing,
                            percent: progress_percent,
                        });
                    }
                }
            }
            Some(summary_chars) = progress_rx.recv() => {
                // Map growing summary length to 15..=85%.
                let stream_percent = 15 + ((summary_chars.min(target_summary_chars) * 70)
                    .div_ceil(target_summary_chars))
                    .min(70);
                progress_percent = progress_percent.max(stream_percent as u8);
                if progress_percent != last_emitted_percent {
                    last_emitted_percent = progress_percent;
                    emitter.emit(AgentEvent::CompactionProgress {
                        phase: CompactionPhase::Summarizing,
                        percent: progress_percent,
                    });
                }
            }
            result = &mut *summary_rx => {
                match result {
                    Ok(Ok(text)) => summary_text = text,
                    Ok(Err(err)) => {
                        let _ = emitter.send_error(AgentRuntimeError::Compaction(err));
                        return None;
                    }
                    Err(_) => {
                        let _ = emitter.send_error(AgentRuntimeError::Compaction(
                            compaction::CompactionError::Llm("summary task aborted".to_owned()),
                        ));
                        return None;
                    }
                }
                break;
            }
        }
    }

    // Drain any progress update that arrived just before the summary completed
    // so the bar reaches its streamed cap before switching to Applying.
    while let Ok(summary_chars) = progress_rx.try_recv() {
        let stream_percent = 15
            + ((summary_chars.min(target_summary_chars) * 70).div_ceil(target_summary_chars))
                .min(70);
        progress_percent = progress_percent.max(stream_percent as u8);
        if progress_percent != last_emitted_percent {
            last_emitted_percent = progress_percent;
            emitter.emit(AgentEvent::CompactionProgress {
                phase: CompactionPhase::Summarizing,
                percent: progress_percent,
            });
        }
    }

    Some((summary_text, progress_percent))
}

/// Build the [`CompactionSummary`], animate the progress bar to 100%, emit
/// `CompactionApplied`, and refresh the effective context window display.
async fn apply_compaction_result(
    emitter: &mut EventEmitter,
    config: &AgentConfig,
    messages: &[AgentMessage],
    compacted_count: usize,
    summary_text: String,
    used_tokens: usize,
    mut progress_percent: u8,
) {
    let kept_messages = &messages[compacted_count..];
    let tokens_after =
        summary_text.len().div_ceil(4) + compaction::estimate_messages_tokens(kept_messages);

    let summary = CompactionSummary {
        summary: summary_text,
        tokens_before: used_tokens,
        tokens_after,
        first_kept_message_index: compacted_count,
    };

    // Smoothly animate from the streamed cap to 100% so the user does not see
    // a frozen bar followed by an abrupt jump to complete.
    let animation_steps: u8 = 15;
    let step = ((100 - progress_percent) as f32 / f32::from(animation_steps)).ceil() as u8;
    for _ in 0..animation_steps {
        if progress_percent >= 100 {
            break;
        }
        progress_percent = (progress_percent + step).min(100);
        tokio::time::sleep(Duration::from_millis(20)).await;
        emitter.emit(AgentEvent::CompactionProgress {
            phase: CompactionPhase::Applying,
            percent: progress_percent,
        });
    }

    emitter.emit(AgentEvent::CompactionApplied { summary });

    let turn = emitter.context.turns.saturating_add(1);
    emit_effective_context_window(config, emitter, turn).await;
}

fn terminates_tool_batch(tool_results: &[(AgentToolCall, ToolResult)]) -> bool {
    !tool_results.is_empty() && tool_results.iter().all(|(_, result)| result.terminate)
}

fn continues_after_terminating_batch(tool_results: &[(AgentToolCall, ToolResult)]) -> bool {
    tool_results.iter().any(|(call, result)| {
        // Mode transitions terminate their batch (so the runtime can fire the
        // mode-switch side effects keyed off `result.terminate`), but the loop
        // generally keeps going so the model can act on the result: continue
        // planning after EnterPlanMode, execute the approved plan after
        // ExitPlanMode. Only the successful branch continues; a rejected/revised
        // ExitPlanMode returns a non-terminating synthesized result and never
        // reaches this predicate.
        //
        // ExitGoalMode is intentionally excluded: it starts the durable goal,
        // and goal continuation (`goal_continuation_messages`) drives subsequent
        // turns on the next `run_agent_turn` entry by design. Continuing inline
        // here would re-feed the continuation message every turn and spin.
        !result.is_error && matches!(call.name.as_str(), "EnterPlanMode" | "ExitPlanMode")
    })
}

#[allow(clippy::too_many_arguments)]
async fn execute_tool_calls(
    config: &AgentConfig,
    registry: &ToolRegistry,
    skills: Option<&SkillStore>,
    turn: u32,
    tool_calls: &[AgentToolCall],
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
    process_supervisor: &ProcessSupervisor,
) -> Result<Vec<(AgentToolCall, ToolResult)>, AgentRuntimeError> {
    if matches!(config.tool_execution_mode, ToolExecutionMode::Sequential) {
        return execute_tool_calls_sequential(
            config,
            registry,
            skills,
            turn,
            tool_calls,
            emitter,
            cancel_token,
            process_supervisor,
        )
        .await;
    }

    if tool_calls.iter().any(|call| {
        let prep = permission_preparation_for_mode(config, call);
        scheduling_class_for_preparation(config, call, &prep) == ToolSchedulingClass::BlockingDialog
    }) {
        return execute_tool_calls_sequential(
            config,
            registry,
            skills,
            turn,
            tool_calls,
            emitter,
            cancel_token,
            process_supervisor,
        )
        .await;
    }

    if tool_calls.iter().any(|call| {
        let prep = permission_preparation_for_mode(config, call);
        scheduling_class_for_preparation(config, call, &prep) == ToolSchedulingClass::Exclusive
    }) {
        return execute_tool_calls_sequential(
            config,
            registry,
            skills,
            turn,
            tool_calls,
            emitter,
            cancel_token,
            process_supervisor,
        )
        .await;
    }

    execute_tool_calls_parallel(
        config,
        registry,
        skills,
        turn,
        tool_calls,
        emitter,
        cancel_token,
        process_supervisor,
    )
    .await
}

fn scheduling_class_for_preparation(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    preparation: &PermissionPreparation,
) -> ToolSchedulingClass {
    if matches!(preparation, PermissionPreparation::Ask { .. }) {
        return ToolSchedulingClass::BlockingDialog;
    }
    if tool_call.name == "AskUserQuestion" && !ask_user_runs_in_background(tool_call) {
        return ToolSchedulingClass::BlockingDialog;
    }
    if tool_call.name == "ExitPlanMode"
        && current_permission_mode(config) != PermissionMode::Auto
        && exit_plan_mode_has_reviewable_plan(config)
    {
        return ToolSchedulingClass::BlockingDialog;
    }
    if tool_call.name == "ExitGoalMode" && current_permission_mode(config) != PermissionMode::Auto {
        return ToolSchedulingClass::BlockingDialog;
    }
    if matches!(
        tool_call.name.as_str(),
        "Bash" | "Terminal" | "Write" | "Edit"
    ) {
        return ToolSchedulingClass::Exclusive;
    }
    ToolSchedulingClass::ParallelSafe
}

fn ask_user_runs_in_background(tool_call: &AgentToolCall) -> bool {
    tool_call
        .arguments
        .get("background")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn exit_plan_mode_has_reviewable_plan(config: &AgentConfig) -> bool {
    let Ok(pm) = config.plan_mode.read() else {
        return false;
    };
    if !pm.is_active() {
        return false;
    }
    pm.data()
        .ok()
        .flatten()
        .is_some_and(|data| !data.content.trim().is_empty())
}

#[allow(clippy::too_many_arguments)]
async fn execute_tool_calls_sequential(
    config: &AgentConfig,
    registry: &ToolRegistry,
    skills: Option<&SkillStore>,
    turn: u32,
    tool_calls: &[AgentToolCall],
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
    process_supervisor: &ProcessSupervisor,
) -> Result<Vec<(AgentToolCall, ToolResult)>, AgentRuntimeError> {
    let tool_context = default_tool_context(config, cancel_token, process_supervisor.clone())?;
    let mut results = Vec::new();
    for tool_call in tool_calls {
        emitter.emit(AgentEvent::ToolExecutionStarted {
            turn,
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            arguments: tool_call.arguments.clone(),
        });
        let mut result =
            if let Some(blocked) = before_tool_result(config, tool_call, cancel_token).await {
                blocked
            } else {
                prepare_and_run_tool(
                    config,
                    registry,
                    skills,
                    &tool_context,
                    turn,
                    tool_call,
                    emitter,
                    cancel_token,
                )
                .await?
            };
        if !cancel_token.is_cancelled() {
            result = after_tool_result(config, tool_call, result, cancel_token).await;
        }
        emit_shell_finished(turn, tool_call, &result, emitter);
        emit_terminal_events(turn, tool_call, &result, &tool_context, emitter);
        emitter.emit(AgentEvent::ToolExecutionFinished {
            turn,
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            result: result.clone(),
        });
        results.push((tool_call.clone(), result));
        if cancel_token.is_cancelled() {
            break;
        }
    }
    Ok(results)
}

#[allow(clippy::too_many_arguments)]
async fn execute_tool_calls_parallel(
    config: &AgentConfig,
    registry: &ToolRegistry,
    skills: Option<&SkillStore>,
    turn: u32,
    tool_calls: &[AgentToolCall],
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
    process_supervisor: &ProcessSupervisor,
) -> Result<Vec<(AgentToolCall, ToolResult)>, AgentRuntimeError> {
    let tool_context = default_tool_context(config, cancel_token, process_supervisor.clone())?;
    let mut completed = Vec::with_capacity(tool_calls.len());
    let mut running = FuturesUnordered::new();

    for (index, tool_call) in tool_calls.iter().cloned().enumerate() {
        if cancel_token.is_cancelled() {
            break;
        }
        emitter.emit(AgentEvent::ToolExecutionStarted {
            turn,
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            arguments: tool_call.arguments.clone(),
        });
        if let Some(mut result) = before_tool_result(config, &tool_call, cancel_token).await {
            if !cancel_token.is_cancelled() {
                result = after_tool_result(config, &tool_call, result, cancel_token).await;
            }
            emit_shell_finished(turn, &tool_call, &result, emitter);
            emit_terminal_events(turn, &tool_call, &result, &tool_context, emitter);
            emitter.emit(AgentEvent::ToolExecutionFinished {
                turn,
                id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                result: result.clone(),
            });
            completed.push((index, tool_call, result));
            continue;
        }

        let prepared = prepare_tool_call(config, &tool_call, turn, emitter, cancel_token).await;
        if let PreparedToolCallResult::Skip(result) = prepared.result {
            if !cancel_token.is_cancelled() {
                let result = after_tool_result(config, &tool_call, result, cancel_token).await;
                emit_shell_finished(turn, &tool_call, &result, emitter);
                emit_terminal_events(turn, &tool_call, &result, &tool_context, emitter);
                emitter.emit(AgentEvent::ToolExecutionFinished {
                    turn,
                    id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    result: result.clone(),
                });
                completed.push((index, tool_call, result));
            }
            continue;
        }

        let config = config.clone();
        let tool_context = tool_context.clone().with_access(prepared.access);
        let cancel_token = cancel_token.clone();
        let sink = emitter.sink();
        running.push(async move {
            let tool_context = tool_context.with_tool_update(make_tool_update_callback(
                sink.clone(),
                turn,
                tool_call.id.clone(),
                tool_call.name.clone(),
            ));
            let mut result =
                run_tool_with_cancel(skills, registry, &tool_call, &tool_context, &cancel_token)
                    .await;
            if !cancel_token.is_cancelled() {
                result = after_tool_result(&config, &tool_call, result, &cancel_token).await;
            }
            Ok::<_, AgentRuntimeError>((index, tool_call, result))
        });
    }

    while let Some(outcome) = running.next().await {
        let (index, tool_call, result) = outcome?;
        emit_shell_finished(turn, &tool_call, &result, emitter);
        emit_terminal_events(turn, &tool_call, &result, &tool_context, emitter);
        emitter.emit(AgentEvent::ToolExecutionFinished {
            turn,
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            result: result.clone(),
        });
        completed.push((index, tool_call, result));
    }

    completed.sort_by_key(|(index, _, _)| *index);
    Ok(completed
        .into_iter()
        .map(|(_, tool_call, result)| (tool_call, result))
        .collect())
}

async fn before_tool_result(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    cancel_token: &CancellationToken,
) -> Option<ToolResult> {
    if let Some(before_tool_call) = &config.before_tool_call
        && let Some(result) = before_tool_call(tool_call)
    {
        return Some(result);
    }
    let async_before_tool_call = config.async_before_tool_call.as_ref()?;
    tokio::select! {
        biased;
        result = async_before_tool_call(tool_call.clone(), cancel_token.clone()) => result,
        () = cancel_token.cancelled() => Some(cancelled_tool_result()),
    }
}

async fn after_tool_result(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    mut result: ToolResult,
    cancel_token: &CancellationToken,
) -> ToolResult {
    if let Some(after_tool_call) = &config.after_tool_call {
        result = after_tool_call(tool_call, result);
    }
    let Some(async_after_tool_call) = &config.async_after_tool_call else {
        return result;
    };
    tokio::select! {
        biased;
        result = async_after_tool_call(tool_call.clone(), result, cancel_token.clone()) => result,
        () = cancel_token.cancelled() => cancelled_tool_result(),
    }
}

async fn run_model_turn(
    model: Arc<dyn ModelClient>,
    config: &AgentConfig,
    request: ChatRequest,
    turn: u32,
    emitter: &mut EventEmitter,
    cancel_token: CancellationToken,
) -> Result<Option<AgentMessage>, AgentRuntimeError> {
    emitter.emit(AgentEvent::TurnStarted { turn });
    let mut state = ModelTurnState::new();
    let mut stream = model.stream_chat(request);

    while let Some(event) = next_model_event(&mut stream, &cancel_token).await {
        if cancel_token.is_cancelled() {
            state.finish_current_message(turn, StopReason::Cancelled, emitter);
            break;
        }
        state.apply_model_event(turn, event?, emitter);
    }

    let stop_reason = state.stop_reason;
    let message = state.into_assistant_message(stop_reason);
    emitter.emit(AgentEvent::MessageAppended {
        message: message.clone(),
    });
    emit_effective_context_window(config, emitter, turn).await;
    emitter.emit(AgentEvent::TurnFinished { turn, stop_reason });
    Ok(Some(message))
}

struct ModelTurnState {
    content: Vec<Content>,
    active_text_index: Option<usize>,
    active_thinking_index: Option<usize>,
    tool_calls: Vec<AgentToolCall>,
    tool_names: std::collections::HashMap<String, String>,
    current_message_id: Option<String>,
    stop_reason: StopReason,
}

impl ModelTurnState {
    fn new() -> Self {
        Self {
            content: Vec::new(),
            active_text_index: None,
            active_thinking_index: None,
            tool_calls: Vec::new(),
            tool_names: std::collections::HashMap::new(),
            current_message_id: None,
            stop_reason: StopReason::EndTurn,
        }
    }

    fn apply_model_event(&mut self, turn: u32, event: AiStreamEvent, emitter: &mut EventEmitter) {
        match event {
            AiStreamEvent::MessageStart { id } => self.start_message(turn, id, emitter),
            AiStreamEvent::TextDelta { text } => self.apply_text_delta(turn, text, emitter),
            AiStreamEvent::ThinkingStart { id } => self.start_thinking(turn, id, emitter),
            AiStreamEvent::ThinkingDelta { text } => {
                self.apply_thinking_delta(turn, text, emitter);
            }
            AiStreamEvent::ThinkingEnd {
                signature,
                redacted,
            } => self.finish_thinking(turn, signature, redacted, emitter),
            AiStreamEvent::ToolCallStart { id, name } => {
                self.start_tool_call(turn, id, name, emitter);
            }
            AiStreamEvent::ToolCallArgsDelta { id, json_fragment } => {
                emitter.emit(AgentEvent::ToolCallArgumentsDelta {
                    turn,
                    id,
                    json_fragment,
                });
            }
            AiStreamEvent::ToolCallEnd { id, arguments } => {
                self.finish_tool_call(turn, id, arguments, emitter);
            }
            AiStreamEvent::MessageEnd { stop_reason, usage } => {
                if let Some(usage) = usage {
                    emitter.emit(AgentEvent::TokenUsage {
                        turn,
                        usage: usage.into(),
                    });
                }
                self.finish_current_message(turn, stop_reason.into(), emitter);
            }
            AiStreamEvent::Error { message } => {
                emitter.emit(AgentEvent::Error {
                    turn,
                    message: message.clone(),
                });
                self.finish_current_message(turn, StopReason::Error, emitter);
            }
        }
    }

    fn start_message(&mut self, turn: u32, id: String, emitter: &mut EventEmitter) {
        self.current_message_id = Some(id.clone());
        emitter.emit(AgentEvent::MessageStarted { turn, id });
    }

    fn apply_text_delta(&mut self, turn: u32, text: String, emitter: &mut EventEmitter) {
        self.append_text(&text);
        emitter.emit(AgentEvent::TextDelta { turn, text });
    }

    fn start_thinking(&mut self, turn: u32, id: String, emitter: &mut EventEmitter) {
        self.content.push(Content::thinking("", None, false));
        self.active_thinking_index = Some(self.content.len() - 1);
        self.active_text_index = None;
        emitter.emit(AgentEvent::ThinkingStarted { turn, id });
    }

    fn apply_thinking_delta(&mut self, turn: u32, text: String, emitter: &mut EventEmitter) {
        let index = self.ensure_active_thinking();
        if let Some(Content::Thinking { text: thinking, .. }) = self.content.get_mut(index) {
            thinking.push_str(&text);
        }
        emitter.emit(AgentEvent::ThinkingDelta { turn, text });
    }

    fn finish_thinking(
        &mut self,
        turn: u32,
        signature: Option<String>,
        redacted: bool,
        emitter: &mut EventEmitter,
    ) {
        let index = self.ensure_active_thinking();
        if let Some(Content::Thinking {
            signature: thinking_signature,
            redacted: thinking_redacted,
            ..
        }) = self.content.get_mut(index)
        {
            *thinking_signature = signature;
            *thinking_redacted = redacted;
        }
        emitter.emit(AgentEvent::ThinkingFinished {
            turn,
            signature: match self.content.get(index) {
                Some(Content::Thinking { signature, .. }) => signature.clone(),
                _ => None,
            },
            redacted,
        });
        self.active_thinking_index = None;
    }

    fn start_tool_call(&mut self, turn: u32, id: String, name: String, emitter: &mut EventEmitter) {
        self.tool_names.insert(id.clone(), name.clone());
        emitter.emit(AgentEvent::ToolCallStarted { turn, id, name });
    }

    fn finish_tool_call(
        &mut self,
        turn: u32,
        id: String,
        arguments: serde_json::Value,
        emitter: &mut EventEmitter,
    ) {
        let tool_call = AgentToolCall {
            name: self.tool_names.remove(&id).unwrap_or_default(),
            id,
            arguments,
        };
        emitter.emit(AgentEvent::ToolCallFinished {
            turn,
            tool_call: tool_call.clone(),
        });
        self.tool_calls.push(tool_call);
    }

    fn finish_current_message(
        &mut self,
        turn: u32,
        stop_reason: StopReason,
        emitter: &mut EventEmitter,
    ) {
        self.stop_reason = stop_reason;
        if let Some(id) = self.current_message_id.take() {
            emitter.emit(AgentEvent::MessageFinished {
                turn,
                id,
                stop_reason,
            });
        }
    }

    fn into_assistant_message(self, stop_reason: StopReason) -> AgentMessage {
        AgentMessage::assistant(self.content, self.tool_calls, stop_reason)
    }

    fn append_text(&mut self, delta: &str) {
        if let Some(index) = self.active_text_index
            && let Some(Content::Text { text }) = self.content.get_mut(index)
        {
            text.push_str(delta);
            return;
        }

        self.content.push(Content::text(delta));
        self.active_text_index = Some(self.content.len() - 1);
    }

    fn ensure_active_thinking(&mut self) -> usize {
        if let Some(index) = self.active_thinking_index
            && matches!(self.content.get(index), Some(Content::Thinking { .. }))
        {
            return index;
        }

        self.content.push(Content::thinking("", None, false));
        let index = self.content.len() - 1;
        self.active_thinking_index = Some(index);
        self.active_text_index = None;
        index
    }
}

async fn next_model_event(
    stream: &mut futures::stream::BoxStream<'_, Result<AiStreamEvent, neo_ai::AiError>>,
    cancel_token: &CancellationToken,
) -> Option<Result<AiStreamEvent, neo_ai::AiError>> {
    tokio::select! {
        event = stream.next() => event,
        () = cancel_token.cancelled() => Some(Err(neo_ai::AiError::Cancelled)),
    }
}

#[allow(clippy::too_many_arguments)]
async fn prepare_and_run_tool(
    config: &AgentConfig,
    registry: &ToolRegistry,
    skills: Option<&SkillStore>,
    tool_context: &ToolContext,
    turn: u32,
    tool_call: &AgentToolCall,
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
) -> Result<ToolResult, AgentRuntimeError> {
    let prepared = prepare_tool_call(config, tool_call, turn, emitter, cancel_token).await;
    match prepared.result {
        PreparedToolCallResult::Skip(result) => Ok(result),
        PreparedToolCallResult::Run => {
            let context = tool_context
                .clone()
                .with_access(prepared.access)
                .with_tool_update(make_tool_update_callback(
                    emitter.sink(),
                    turn,
                    tool_call.id.clone(),
                    tool_call.name.clone(),
                ));
            if tool_call.name == "Bash" {
                emit_shell_started(turn, tool_call, &context, emitter);
            }
            let result =
                run_tool_with_cancel(skills, registry, tool_call, &context, cancel_token).await;
            if tool_call.name == "Skill" && !result.is_error {
                emitter.emit(AgentEvent::SkillActivated {
                    turn,
                    name: tool_call
                        .arguments
                        .get("skill")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown")
                        .to_owned(),
                });
            }
            Ok(result)
        }
    }
}

async fn prepare_tool_call(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    turn: u32,
    emitter: &mut impl EventPublisher,
    cancel_token: &CancellationToken,
) -> PreparedToolCall {
    let preparation = permission_preparation_for_mode(config, tool_call);

    match preparation {
        PermissionPreparation::Run(access) => PreparedToolCall {
            result: PreparedToolCallResult::Run,
            access,
        },
        PermissionPreparation::Deny(message) => PreparedToolCall {
            result: PreparedToolCallResult::Skip(ToolResult::error(message)),
            access: ToolAccess::none(),
        },
        PermissionPreparation::Ask {
            operation,
            subject,
            session_scope,
            prefix_rule,
        } => {
            match resolve_approval(
                config,
                turn,
                tool_call,
                operation,
                subject,
                session_scope,
                prefix_rule,
                emitter,
                cancel_token,
            )
            .await
            {
                Some(result) => PreparedToolCall {
                    result: PreparedToolCallResult::Skip(result),
                    access: ToolAccess::none(),
                },
                None => PreparedToolCall {
                    result: PreparedToolCallResult::Run,
                    access: access_for_tool(tool_call, true),
                },
            }
        }
    }
}

fn permission_preparation_for_mode(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
) -> PermissionPreparation {
    // Read live permission state once. The TUI may switch this mid-turn via
    // `/ask`, `/auto`, `/yolo`, or `/permissions`; every branch below must use
    // this `mode` instead of the static `config.permission_mode` snapshot.
    let mode = current_permission_mode(config);

    // 1. Plan-mode hard guard.
    if let Some(prep) = check_plan_guard(config, tool_call) {
        return prep;
    }

    // 2-5. Mode/tool-specific early returns (auto mode, EnterPlanMode, background AskUser).
    if let Some(prep) = check_mode_early_returns(tool_call, mode) {
        return prep;
    }

    // 6. Derive the reusable scope + prefix rule for the ask fallback.
    let (session_scope, prefix_rule) = approval_scope_for_tool_call(config, tool_call);

    // 7-8. Cached approvals (persistent prefix rules + session approvals).
    if let Some(prep) = check_cached_approvals(config, tool_call, &session_scope) {
        return prep;
    }

    // 9-10. Transition tools (ExitPlanMode, ExitGoalMode).
    if let Some(prep) = check_transition_tools(config, tool_call, mode) {
        return prep;
    }

    // 10. Plan-mode helper approvals (e.g. writing the active plan file).
    if let Some(prep) = check_plan_file_write(config, tool_call) {
        return prep;
    }

    // 11-13. Yolo mode, safe commands, and default-approved tools.
    if let Some(prep) = check_safe_or_prompt(config, tool_call, mode) {
        return prep;
    }

    // 14. Ask fallback prompt.
    let (operation, subject) = permission_operation_for_tool(tool_call)
        .unwrap_or((PermissionOperation::Tool, tool_call.name.clone()));
    PermissionPreparation::Ask {
        operation,
        subject,
        session_scope,
        prefix_rule,
    }
}

/// Section 1 — plan-mode hard guard. Returns `Some(Deny)` if the plan-mode guard
/// rejects the call, or `None` to continue the pipeline.
fn check_plan_guard(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
) -> Option<PermissionPreparation> {
    let plan_mode = config.plan_mode.read().ok()?;
    if plan_mode.is_active() {
        match check_plan_mode_guard(
            &plan_mode,
            config.workspace_root.as_deref(),
            &tool_call.name,
            &tool_call.arguments,
        ) {
            PlanModeGuard::Allow => {}
            PlanModeGuard::Deny { message } => {
                return Some(PermissionPreparation::Deny(message));
            }
        }
    }
    None
}

/// Sections 2-5 — mode/tool-specific early returns: auto-mode AskUserQuestion
/// deny, background AskUserQuestion run, auto-mode approves-all, EnterPlanMode
/// auto-approve.
fn check_mode_early_returns(
    tool_call: &AgentToolCall,
    mode: PermissionMode,
) -> Option<PermissionPreparation> {
    // 2. Auto mode hard deny for AskUserQuestion.
    if tool_call.name == "AskUserQuestion" && mode == PermissionMode::Auto {
        return Some(PermissionPreparation::Deny(
            "AskUserQuestion is disabled while auto permission mode is active".to_owned(),
        ));
    }

    // 3. Background AskUserQuestion does not need an approval dialog in any mode.
    if tool_call.name == "AskUserQuestion" && ask_user_runs_in_background(tool_call) {
        return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
    }

    // 4. Auto mode approves everything else.
    if mode == PermissionMode::Auto {
        return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
    }

    // 5. EnterPlanMode is auto-approved in all modes.
    if tool_call.name == "EnterPlanMode" {
        return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
    }

    None
}

/// Sections 7-8 — cached approvals: persistent prefix rules (layer 2) and
/// session approvals (layer 1). Returns `Some(Run)` when the call matches a
/// cached approval.
fn check_cached_approvals(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    session_scope: &Option<SessionApprovalScope>,
) -> Option<PermissionPreparation> {
    // Layer 2 — persistent prefix rules (loaded from disk).
    if let Some(argv) = shell_argv_for_prefix_check(config, tool_call)
        && config
            .prefix_approval_rules
            .lock()
            .ok()
            .is_some_and(|store| store.matches(&argv))
    {
        return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
    }

    // Layer 1 — session approvals scoped by exact canonical command + cwd
    // (or exact file path + operation).
    if let Some(scope) = session_scope
        && config
            .session_approvals
            .lock()
            .ok()
            .is_some_and(|set| scope.is_approved(&set))
    {
        return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
    }

    None
}

/// Sections 9-10 — transition tools: ExitPlanMode and ExitGoalMode. These
/// transitions must never become session-scoped wildcards.
fn check_transition_tools(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    mode: PermissionMode,
) -> Option<PermissionPreparation> {
    if tool_call.name == "ExitPlanMode" {
        if exit_plan_mode_has_reviewable_plan(config) {
            return Some(PermissionPreparation::Ask {
                operation: PermissionOperation::PlanTransition,
                subject: "Exit plan mode".to_owned(),
                session_scope: None,
                prefix_rule: None,
            });
        }
        return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
    }

    if tool_call.name == "ExitGoalMode" {
        if mode == PermissionMode::Auto {
            return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
        }
        return Some(PermissionPreparation::Ask {
            operation: PermissionOperation::GoalTransition,
            subject: "Start reviewed goal".to_owned(),
            session_scope: None,
            prefix_rule: None,
        });
    }

    None
}

/// Section 10 — plan-mode helper approvals (Write/Edit to the active plan file).
fn check_plan_file_write(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
) -> Option<PermissionPreparation> {
    if !matches!(tool_call.name.as_str(), "Write" | "Edit") {
        return None;
    }
    let plan_mode = config.plan_mode.read().ok()?;
    if let Some(path) = tool_call.arguments.get("path").and_then(|v| v.as_str())
        && plan_mode.is_active()
        && is_active_plan_file_path(&plan_mode, config.workspace_root.as_deref(), path)
    {
        return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
    }
    None
}

/// Sections 11-13 — yolo mode approves-all, dangerous-command force-prompt,
/// known-safe commands, and default-approved tools.
fn check_safe_or_prompt(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    mode: PermissionMode,
) -> Option<PermissionPreparation> {
    // 11. Yolo mode approves all remaining tools.
    if mode == PermissionMode::Yolo {
        return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
    }

    // 12. Read-only safe commands skip the prompt in ask mode. Dangerous
    //     commands bypass this and force a prompt.
    if let Some(argv) = shell_argv_for_prefix_check(config, tool_call) {
        if command_might_be_dangerous(&argv) {
            let (operation, subject) = permission_operation_for_tool(tool_call)
                .unwrap_or((PermissionOperation::Tool, tool_call.name.clone()));
            return Some(PermissionPreparation::Ask {
                operation,
                subject,
                session_scope: None,
                prefix_rule: None,
            });
        }
        if is_known_safe_command(&argv) {
            return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
        }
    }

    // 13. Default safe tools in ask mode.
    if is_default_approved_tool(tool_call) {
        return Some(PermissionPreparation::Run(access_for_tool(tool_call, true)));
    }

    None
}

/// Read the live permission mode. Falls back to the static snapshot only when
/// the live lock is poisoned (which would already abort the turn elsewhere).
#[inline]
fn current_permission_mode(config: &AgentConfig) -> PermissionMode {
    config
        .live_permission_mode
        .read()
        .map_or(config.permission_mode, |guard| *guard)
}

fn is_default_approved_tool(tool_call: &AgentToolCall) -> bool {
    matches!(
        tool_call.name.as_str(),
        "Read"
            | "List"
            | "Grep"
            | "Find"
            | "Glob"
            | "TodoList"
            | "TaskList"
            | "TaskOutput"
            | "Skill"
            | "AskUserQuestion"
    )
}

fn access_for_tool(tool_call: &AgentToolCall, grant: bool) -> ToolAccess {
    match tool_call.name.as_str() {
        "Read" | "List" | "Grep" | "Find" | "Glob" => ToolAccess {
            file_read: grant,
            ..ToolAccess::none()
        },
        "Write" | "Edit" => ToolAccess {
            file_write: grant,
            ..ToolAccess::none()
        },
        "Bash" | "Terminal" | "TaskStop" => ToolAccess {
            shell: grant,
            ..ToolAccess::none()
        },
        "AskUserQuestion" => ToolAccess {
            user_question: grant,
            ..ToolAccess::none()
        },
        _ => ToolAccess {
            tool: grant,
            ..ToolAccess::none()
        },
    }
}

#[allow(clippy::too_many_arguments)]
async fn resolve_approval(
    config: &AgentConfig,
    turn: u32,
    tool_call: &AgentToolCall,
    operation: PermissionOperation,
    subject: String,
    session_scope: Option<SessionApprovalScope>,
    prefix_rule: Option<PrefixApprovalRule>,
    emitter: &mut impl EventPublisher,
    cancel_token: &CancellationToken,
) -> Option<ToolResult> {
    let mut arguments = tool_call.arguments.clone();
    // For plan transitions, inject the plan file content so the TUI can
    // render it inside the approval dialog.
    if operation == PermissionOperation::PlanTransition
        && let Ok(plan_mode) = config.plan_mode.read()
        && let Ok(Some(plan_data)) = plan_mode.data()
        && let Some(obj) = arguments.as_object_mut()
    {
        obj.insert(
            "plan_content".to_string(),
            serde_json::Value::String(plan_data.content.clone()),
        );
        obj.insert(
            "plan_path".to_string(),
            serde_json::Value::String(plan_data.path.display().to_string()),
        );
    }
    let request = ApprovalRequest {
        turn,
        id: tool_call.id.clone(),
        operation,
        subject: subject.clone(),
        arguments,
        session_scope: session_scope.clone(),
        prefix_rule: prefix_rule.clone(),
    };
    emitter.emit(AgentEvent::ApprovalRequested {
        turn: request.turn,
        id: request.id.clone(),
        operation: request.operation,
        subject: request.subject.clone(),
        arguments: request.arguments.clone(),
        session_scope: request.session_scope.clone(),
        prefix_rule: request.prefix_rule.clone(),
    });
    let decision = if let Some(handler) = &config.approval_handler {
        handler(&request)
    } else if let Some(handler) = &config.async_approval_handler {
        tokio::select! {
            biased;
            () = cancel_token.cancelled() => return Some(cancelled_tool_result()),
            decision = handler(request.clone()) => decision,
        }
    } else {
        return Some(permission_error(operation, &subject, "approval required"));
    };
    match decision {
        PermissionApprovalDecision::AllowOnce => None,
        PermissionApprovalDecision::AllowForSession => {
            // Layer 1: record each narrow key (exact canonical command/cwd,
            // exact file path/op). With no derived scope this degrades to a
            // no-op AllowOnce — it never creates a tool-name wildcard.
            if let Some(scope) = &session_scope
                && let Ok(mut set) = config.session_approvals.lock()
            {
                scope.record(&mut set);
            }
            None
        }
        PermissionApprovalDecision::AllowForPrefix => {
            if let Some(rule) = &prefix_rule
                && !ApprovalRuleStore::is_would_approve_all(&rule.prefix)
            {
                let should_save = if let Ok(mut store) = config.prefix_approval_rules.lock() {
                    let was_new = !store.prefix_rules.iter().any(|r| r.prefix == rule.prefix);
                    store.insert(rule.clone());
                    was_new
                } else {
                    false
                };
                if should_save {
                    let _ = config.save_prefix_approval_rules();
                }
            }
            None
        }
        PermissionApprovalDecision::Reject => {
            // Review feedback is delivered via the review-feedback side-channel.
            if matches!(tool_call.name.as_str(), "ExitPlanMode" | "ExitGoalMode")
                && let Some(feedback) = config
                    .plan_review_feedback
                    .lock()
                    .ok()
                    .and_then(|mut m| m.remove(&tool_call.id))
            {
                let target = if tool_call.name == "ExitGoalMode" {
                    "Goal mode"
                } else {
                    "Plan mode"
                };
                return Some(ToolResult::ok(format!(
                    "User requested revisions. {target} remains active.\n\nFeedback: {feedback}"
                )));
            }
            Some(permission_error(operation, &subject, "approval denied"))
        }
    }
}

fn permission_error(
    operation: PermissionOperation,
    subject: &str,
    prefix: &'static str,
) -> ToolResult {
    let noun = match operation {
        PermissionOperation::FileRead => "file read",
        PermissionOperation::FileWrite => "file write",
        PermissionOperation::Shell => "shell",
        PermissionOperation::Tool => "tool",
        PermissionOperation::UserQuestion => "user question",
        PermissionOperation::PlanTransition => "plan transition",
        PermissionOperation::GoalTransition => "goal transition",
    };
    ToolResult::error(format!("{prefix} for {noun}: {subject}"))
}

fn permission_operation_for_tool(
    tool_call: &AgentToolCall,
) -> Option<(PermissionOperation, String)> {
    match tool_call.name.as_str() {
        "Read" | "List" | "Grep" | "Find" | "Glob" => Some((
            PermissionOperation::FileRead,
            path_subject(&tool_call.arguments).unwrap_or_else(|| tool_call.name.clone()),
        )),
        "Write" | "Edit" => Some((
            PermissionOperation::FileWrite,
            path_subject(&tool_call.arguments).unwrap_or_else(|| tool_call.name.clone()),
        )),
        "Bash" | "Terminal" | "TaskStop" => Some((
            PermissionOperation::Shell,
            tool_call
                .arguments
                .get("command")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    tool_call
                        .arguments
                        .get("task_id")
                        .and_then(serde_json::Value::as_str)
                })
                .or_else(|| {
                    tool_call
                        .arguments
                        .get("handle")
                        .and_then(serde_json::Value::as_str)
                })
                .unwrap_or(tool_call.name.as_str())
                .to_owned(),
        )),
        "AskUserQuestion" => Some((
            PermissionOperation::UserQuestion,
            tool_call
                .arguments
                .get("questions")
                .and_then(|q| q.as_array())
                .and_then(|arr| arr.first())
                .and_then(|q| q.get("question"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("question")
                .to_owned(),
        )),
        _ => None,
    }
}

fn path_subject(arguments: &serde_json::Value) -> Option<String> {
    arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
}

// ---------------------------------------------------------------------------
// Layer 1/2/3 — approval scope, prefix rule, and safety derivation helpers
// ---------------------------------------------------------------------------

/// Workspace root string used as part of every approval key. Stored on the key
/// so a session store reused across workspaces never leaks an approval. Empty
/// when the workspace root is unknown.
fn workspace_key_root(config: &AgentConfig) -> String {
    config
        .workspace_root
        .as_deref()
        .map_or_else(String::new, |root| root.display().to_string())
}

/// Resolve the effective Bash cwd: if the caller passed `cwd`, resolve it
/// through workspace containment, else use the workspace root. Returns `None`
/// when the path escapes the workspace or the workspace root is unknown.
fn resolve_bash_cwd(config: &AgentConfig, arguments: &serde_json::Value) -> Option<String> {
    let workspace_root = config.workspace_root.as_deref()?;
    let candidate = arguments
        .get("cwd")
        .and_then(serde_json::Value::as_str)
        .map(std::path::Path::new);
    let resolved = match candidate {
        Some(rel) if !rel.is_absolute() => workspace_root.join(rel),
        Some(abs) => abs.to_path_buf(),
        None => workspace_root.to_path_buf(),
    };
    let normalized = normalize_path(&resolved);
    if !normalized.starts_with(workspace_root) {
        return None;
    }
    Some(normalized.display().to_string())
}

/// Split a shell command string into argv tokens using POSIX-ish word rules.
/// Handles single/double quotes and backslash escapes. Returns `None` when the
/// string is empty or unparseable (e.g. unmatched quote).
fn tokenize_shell_command(command: &str) -> Option<Vec<String>> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut has_token = false;
    let mut chars = trimmed.chars().peekable();
    while let Some(ch) = chars.next() {
        if in_single {
            if ch == '\'' {
                in_single = false;
            } else {
                current.push(ch);
            }
            continue;
        }
        if in_double {
            if ch == '"' {
                in_double = false;
            } else if ch == '\\' {
                if let Some(&next) = chars.peek()
                    && matches!(next, '"' | '\\' | '$' | '`')
                {
                    current.push(next);
                    chars.next();
                    continue;
                }
                current.push(ch);
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            '\'' => {
                in_single = true;
                has_token = true;
            }
            '"' => {
                in_double = true;
                has_token = true;
            }
            '\\' => {
                if let Some(&next) = chars.peek() {
                    current.push(next);
                    chars.next();
                    has_token = true;
                }
            }
            c if c.is_whitespace() => {
                if has_token {
                    tokens.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            c => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if in_single || in_double {
        return None; // unmatched quote
    }
    if has_token {
        tokens.push(current);
    }
    if tokens.is_empty() {
        None
    } else {
        Some(tokens)
    }
}

/// True when a command string contains shell control operators that make it a
/// compound/opaque script (`&&`, `||`, `;`, `|`, `>`, `<`, backticks, `$(...)`,
/// `{...}`). Used to decide whether a stable argv prefix can be proposed.
fn is_compound_or_opaque_command(command: &str) -> bool {
    // Quick scan outside of quotes. Conservative: any of these operators marks
    // the line as compound/opaque for prefix purposes.
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = command.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '&' if !in_single && !in_double => {
                if chars.peek() == Some(&'&') {
                    return true;
                }
            }
            '|' | ';' | '>' | '<' | '`' | '{' if !in_single && !in_double => return true,
            '$' if !in_single && !in_double && chars.peek() == Some(&'(') => return true,
            _ => {}
        }
    }
    false
}

/// Tokenize a Bash command for prefix-check / safety classification. Returns
/// `None` when there is no `command` arg or it cannot be tokenized.
fn shell_argv(config: &AgentConfig, tool_call: &AgentToolCall) -> Option<Vec<String>> {
    if tool_call.name != "Bash" {
        return None;
    }
    let background = tool_call
        .arguments
        .get("run_in_background")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if background {
        return None;
    }
    let raw = tool_call
        .arguments
        .get("command")
        .and_then(serde_json::Value::as_str)?;
    let _ = config.workspace_root.as_deref()?;
    tokenize_shell_command(raw)
}

/// Alias used in `permission_preparation_for_mode` for clarity.
fn shell_argv_for_prefix_check(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
) -> Option<Vec<String>> {
    shell_argv(config, tool_call)
}

/// Derive `(session_scope, prefix_rule)` for a tool call. Returns `(None, None)`
/// for review transitions, dangerous commands, interactive tools, and anything
/// where a reusable grant is unsafe.
fn approval_scope_for_tool_call(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
) -> (Option<SessionApprovalScope>, Option<PrefixApprovalRule>) {
    // Review transitions and dangerous commands never offer scope/prefix.
    if matches!(tool_call.name.as_str(), "ExitPlanMode" | "ExitGoalMode") {
        return (None, None);
    }
    match tool_call.name.as_str() {
        "Bash" => bash_approval_scope(config, &tool_call.arguments),
        "Write" => {
            let (scope, _) = file_write_approval_scope(
                config,
                &tool_call.arguments,
                FileWriteApprovalOperation::Write,
            );
            (scope, None)
        }
        "Edit" => {
            let (scope, _) = file_write_approval_scope(
                config,
                &tool_call.arguments,
                FileWriteApprovalOperation::Edit,
            );
            (scope, None)
        }
        _ => tool_approval_scope(config, &tool_call.name),
    }
}

/// Build the session scope + optional prefix rule for a Bash call.
fn bash_approval_scope(
    config: &AgentConfig,
    arguments: &serde_json::Value,
) -> (Option<SessionApprovalScope>, Option<PrefixApprovalRule>) {
    let background = arguments
        .get("run_in_background")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if background {
        return (None, None); // background bash has no safe reusable scope
    }
    let Some(raw_command) = arguments.get("command").and_then(serde_json::Value::as_str) else {
        return (None, None);
    };
    let command = raw_command.trim();
    if command.is_empty() {
        return (None, None);
    }
    let workspace = workspace_key_root(config);
    let cwd = resolve_bash_cwd(config, arguments).unwrap_or_else(|| workspace.clone());
    // Dangerous commands get no scope (re-prompt every time).
    if let Some(argv) = tokenize_shell_command(command) {
        if command_might_be_dangerous(&argv) {
            return (None, None);
        }
        // Layer 1: exact canonical argv key (only when not compound/opaque, so
        // `git status && git push` does not get cached as if it were `git status`).
        let key = if is_compound_or_opaque_command(command) {
            SessionApprovalKey::Shell {
                workspace: workspace.clone(),
                cwd: cwd.clone(),
                command: vec!["__shell_script__".to_owned(), command.to_owned()],
            }
        } else {
            SessionApprovalKey::Shell {
                workspace: workspace.clone(),
                cwd: cwd.clone(),
                command: argv.clone(),
            }
        };
        let scope = SessionApprovalScope {
            keys: vec![key],
            label: "Approve this exact command for this session".to_owned(),
            detail: format!("Exact command in {cwd}: {command}"),
        };
        // Layer 2: propose a prefix rule only for non-compound commands (so the
        // prefix is a real argv prefix, not half of a `&&`). Use the first
        // program token; refuse empty (would approve everything).
        let prefix_rule = if !is_compound_or_opaque_command(command) && !argv.is_empty() {
            let prefix = vec![argv[0].clone()];
            if ApprovalRuleStore::is_would_approve_all(&prefix) {
                None
            } else {
                Some(PrefixApprovalRule {
                    label: argv[0].clone(),
                    prefix,
                })
            }
        } else {
            None
        };
        (Some(scope), prefix_rule)
    } else {
        // Could not tokenize (unmatched quote etc.) — opaque exact-text key.
        let key = SessionApprovalKey::Shell {
            workspace: workspace.clone(),
            cwd: cwd.clone(),
            command: vec!["__shell_script__".to_owned(), command.to_owned()],
        };
        let scope = SessionApprovalScope {
            keys: vec![key],
            label: "Approve this exact command for this session".to_owned(),
            detail: format!("Exact command in {cwd}: {command}"),
        };
        (Some(scope), None)
    }
}

/// Build the session scope for a Write/Edit call. Returns no prefix rule.
fn file_write_approval_scope(
    config: &AgentConfig,
    arguments: &serde_json::Value,
    operation: FileWriteApprovalOperation,
) -> (Option<SessionApprovalScope>, Option<PrefixApprovalRule>) {
    let Some(raw_path) = arguments.get("path").and_then(serde_json::Value::as_str) else {
        return (None, None);
    };
    if raw_path.trim().is_empty() {
        return (None, None);
    }
    let workspace = workspace_key_root(config);
    let Some(workspace_root) = config.workspace_root.as_deref() else {
        return (None, None);
    };
    let resolved = if std::path::Path::new(raw_path).is_absolute() {
        std::path::PathBuf::from(raw_path)
    } else {
        workspace_root.join(raw_path)
    };
    let normalized = normalize_path(&resolved);
    if !normalized.starts_with(workspace_root) {
        return (None, None);
    }
    let path = normalized.display().to_string();
    let key = SessionApprovalKey::FileWrite {
        workspace: workspace.clone(),
        path: path.clone(),
        operation,
    };
    let (verb, label) = match operation {
        FileWriteApprovalOperation::Write => {
            ("writes to", "Approve writes to this file for this session")
        }
        FileWriteApprovalOperation::Edit => {
            ("edits to", "Approve edits to this file for this session")
        }
    };
    let scope = SessionApprovalScope {
        keys: vec![key],
        label: label.to_owned(),
        detail: format!("File ({verb}): {path}"),
    };
    (Some(scope), None)
}

/// Build the session scope for a generic tool call (MCP/extension tools and any
/// non-builtin tool). The scope is keyed by the fully-qualified tool name so the
/// same tool is auto-approved for the rest of the session. Returns no prefix rule
/// (prefix rules are shell-specific). Built-in write/shell tools are handled by
/// their own dedicated scope derivations before this is reached.
fn tool_approval_scope(
    config: &AgentConfig,
    tool_name: &str,
) -> (Option<SessionApprovalScope>, Option<PrefixApprovalRule>) {
    if tool_name.is_empty() {
        return (None, None);
    }
    let workspace = workspace_key_root(config);
    let key = SessionApprovalKey::Tool {
        workspace,
        name: tool_name.to_owned(),
    };
    let scope = SessionApprovalScope {
        keys: vec![key],
        label: "Approve this tool for this session".to_owned(),
        detail: format!("Tool: {tool_name}"),
    };
    (Some(scope), None)
}

fn emit_shell_started(
    turn: u32,
    tool_call: &AgentToolCall,
    tool_context: &ToolContext,
    emitter: &mut impl EventPublisher,
) {
    if tool_call.name != "Bash" {
        return;
    }
    if let Some(command) = tool_call
        .arguments
        .get("command")
        .and_then(serde_json::Value::as_str)
    {
        emitter.emit(AgentEvent::ShellCommandStarted {
            turn,
            id: tool_call.id.clone(),
            command: command.to_owned(),
            cwd: tool_context.workspace_root().to_path_buf(),
            origin: ShellCommandOrigin::ModelBashTool,
        });
    }
}

fn emit_shell_finished(
    turn: u32,
    tool_call: &AgentToolCall,
    result: &ToolResult,
    emitter: &mut impl EventPublisher,
) {
    if tool_call.name != "Bash" {
        return;
    }
    let Some(details) = &result.details else {
        return;
    };
    let stdout = details
        .get("stdout")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let stderr = details
        .get("stderr")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let exit_code = details
        .get("exit_code")
        .and_then(serde_json::Value::as_i64)
        .and_then(|code| i32::try_from(code).ok());
    let truncated = details
        .get("truncated")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let outcome = shell_command_outcome_from_details(details);
    emitter.emit(AgentEvent::ShellCommandFinished {
        turn,
        id: tool_call.id.clone(),
        exit_code,
        stdout,
        stderr,
        truncated,
        origin: ShellCommandOrigin::ModelBashTool,
        outcome,
    });
}

fn shell_command_outcome_from_details(details: &serde_json::Value) -> ShellCommandOutcome {
    match details.get("outcome").and_then(serde_json::Value::as_str) {
        Some("cancelled") => ShellCommandOutcome::Cancelled,
        Some("timed_out") => ShellCommandOutcome::TimedOut,
        Some("backgrounded") => {
            let task_id = details
                .get("task_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            ShellCommandOutcome::Backgrounded { task_id }
        }
        _ if details.get("kind").and_then(serde_json::Value::as_str) == Some("bash")
            && details.get("status").and_then(serde_json::Value::as_str) == Some("running") =>
        {
            let task_id = details
                .get("task_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            ShellCommandOutcome::Backgrounded { task_id }
        }
        _ => ShellCommandOutcome::Completed,
    }
}

fn emit_terminal_events(
    turn: u32,
    tool_call: &AgentToolCall,
    result: &ToolResult,
    tool_context: &ToolContext,
    emitter: &mut impl EventPublisher,
) {
    if tool_call.name != "Terminal" {
        return;
    }
    let Some(details) = &result.details else {
        return;
    };
    let Some(handle) = details
        .get("handle")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
    else {
        return;
    };
    match tool_call
        .arguments
        .get("mode")
        .and_then(serde_json::Value::as_str)
    {
        Some("start") => {
            let command = details
                .get("command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let cols = details
                .get("cols")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u16::try_from(value).ok())
                .unwrap_or(80);
            let rows = details
                .get("rows")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u16::try_from(value).ok())
                .unwrap_or(24);
            emitter.emit(AgentEvent::TerminalSessionStarted {
                turn,
                id: tool_call.id.clone(),
                handle,
                command,
                cwd: tool_context.workspace_root().to_path_buf(),
                cols,
                rows,
            });
        }
        Some("read") => {
            let output = details
                .get("output")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if output.is_empty() {
                return;
            }
            let truncated = details
                .get("truncated")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            emitter.emit(AgentEvent::TerminalSessionOutput {
                turn,
                id: tool_call.id.clone(),
                handle,
                output,
                truncated,
            });
        }
        Some("stop") => {
            let status = details
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("stopped")
                .to_owned();
            let exit_code = details
                .get("exit_code")
                .and_then(serde_json::Value::as_i64)
                .and_then(|code| i32::try_from(code).ok());
            emitter.emit(AgentEvent::TerminalSessionFinished {
                turn,
                id: tool_call.id.clone(),
                handle,
                status,
                exit_code,
            });
        }
        _ => {}
    }
}

/// Creates a `ToolUpdateCallback` that emits `ToolExecutionUpdate` events
/// through an `EventSink`. This lets tools (e.g. bash) stream intermediate
/// output that the TUI renders live.
fn make_tool_update_callback(
    sink: EventSink,
    turn: u32,
    id: String,
    name: String,
) -> ToolUpdateCallback {
    Arc::new(move |partial: &str| {
        sink.emit_event(AgentEvent::ToolExecutionUpdate {
            turn,
            id: id.clone(),
            name: name.clone(),
            partial_result: ToolResult {
                content: partial.to_owned(),
                is_error: false,
                details: None,
                terminate: false,
            },
        });
    })
}

async fn run_tool_with_cancel(
    skills: Option<&SkillStore>,
    registry: &ToolRegistry,
    tool_call: &AgentToolCall,
    tool_context: &ToolContext,
    cancel_token: &CancellationToken,
) -> ToolResult {
    if tool_call.name == "Skill" {
        return execute_invoke_skill(skills, tool_call);
    }
    if tool_call.name == "Bash" {
        return run_model_bash_with_cancel(tool_call, tool_context, cancel_token).await;
    }
    tokio::select! {
        biased;
        result = registry.run(&tool_call.name, tool_context, tool_call.arguments.clone()) => {
            result.unwrap_or_else(|err| ToolResult::error(err.to_string()))
        }
        () = cancel_token.cancelled() => cancelled_tool_result(),
    }
}

fn cancelled_tool_result() -> ToolResult {
    ToolResult::error(ToolError::Cancelled.to_string())
}

async fn run_model_bash_with_cancel(
    tool_call: &AgentToolCall,
    tool_context: &ToolContext,
    cancel_token: &CancellationToken,
) -> ToolResult {
    tokio::select! {
        biased;
        result = execute_model_bash_for_runtime(tool_context, tool_call.arguments.clone()) => {
            result.unwrap_or_else(|err| ToolResult::error(err.to_string()))
        }
        () = cancel_token.cancelled() => cancelled_tool_result(),
    }
}

fn invoke_skill_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "Skill".to_owned(),
        description: "Invoke an available skill by name with arguments. Use this when the user's request matches a skill's description or whenToUse.".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "Name of the skill to invoke"
                },
                "arguments": {
                    "type": "object",
                    "description": "Named arguments for the skill"
                }
            },
            "required": ["skill"]
        }),
    }
}

fn execute_invoke_skill(skills: Option<&SkillStore>, tool_call: &AgentToolCall) -> ToolResult {
    let Some(skills) = skills else {
        return ToolResult::error("skill system is not enabled");
    };
    let request = match skill_tool_request(&tool_call.arguments) {
        Ok(request) => request,
        Err(message) => return ToolResult::error(message),
    };
    let Some(skill) = skills.get(&request.skill_name) else {
        return ToolResult::error(format!("skill `{}` is not available", request.skill_name));
    };
    if skill_is_manual_only(skill) {
        return ToolResult::error(format!(
            "skill `{}` is type `flow` and can only be invoked manually via /skill:{}",
            request.skill_name, request.skill_name
        ));
    }

    let invocation = request.into_invocation();

    match crate::skills::expand_skill_body(skill, &invocation) {
        Ok(body) => ToolResult::ok(body),
        Err(err) => ToolResult::error(format!(
            "failed to expand skill `{}`: {err}",
            invocation.name
        )),
    }
}

struct SkillToolRequest {
    skill_name: String,
    arguments: serde_json::Map<String, serde_json::Value>,
}

impl SkillToolRequest {
    fn into_invocation(self) -> crate::skills::SkillInvocation {
        crate::skills::SkillInvocation {
            name: self.skill_name,
            raw_arguments: serde_json::to_string(&self.arguments).unwrap_or_default(),
            positional: Vec::new(),
            named: string_skill_arguments(self.arguments),
        }
    }
}

fn skill_tool_request(arguments: &serde_json::Value) -> Result<SkillToolRequest, String> {
    let skill_name = arguments
        .get("skill")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "Skill requires a `skill` string argument".to_owned())?;
    let arguments = arguments
        .get("arguments")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();
    Ok(SkillToolRequest {
        skill_name: skill_name.to_owned(),
        arguments,
    })
}

fn string_skill_arguments(
    arguments: serde_json::Map<String, serde_json::Value>,
) -> std::collections::HashMap<String, String> {
    arguments
        .into_iter()
        .filter_map(|(key, value)| value.as_str().map(|string| (key, string.to_owned())))
        .collect()
}

fn skill_is_manual_only(skill: &crate::skills::LoadedSkill) -> bool {
    matches!(skill.manifest.skill_type, crate::skills::SkillType::Flow)
}

fn default_tool_context(
    config: &AgentConfig,
    cancel_token: &CancellationToken,
    process_supervisor: ProcessSupervisor,
) -> Result<ToolContext, AgentRuntimeError> {
    let workspace_root = if let Some(workspace_root) = &config.workspace_root {
        workspace_root.clone()
    } else {
        std::env::current_dir()?
    };
    ToolContext::new(workspace_root)
        .map(|context| {
            context
                .with_access(ToolAccess::none())
                .with_cancel_token(cancel_token.clone())
                .with_process_supervisor(process_supervisor)
                .with_background_tasks(config.background_tasks.clone())
        })
        .map(|context| {
            // The active plan file lives under the NEO_HOME sessions bucket
            // (outside the workspace). Whitelist it so Write/Edit can resolve
            // the path while plan mode is active; the plan-mode guard and the
            // permission layer still restrict writes to *only* that path.
            let plan_path = config
                .plan_mode
                .read()
                .ok()
                .and_then(|plan_mode| plan_mode.plan_file_path().map(PathBuf::from));
            match plan_path {
                Some(path) => context.with_allowed_external_write_paths([path]),
                None => context,
            }
        })
        .map_err(AgentRuntimeError::Tool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{SkillArgument, SkillManifest, SkillSource, SkillType};
    use futures::TryStreamExt;
    use serde_json::json;

    fn write_skill(root: &std::path::Path, name: &str, skill_type: &str, body: &str) {
        let skill_dir = root.join(name);
        std::fs::create_dir_all(&skill_dir).expect("skill dir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!(
                r"---
name: {name}
description: {name} skill
type: {skill_type}
arguments:
  - name: target
    required: true
---
{body}
"
            ),
        )
        .expect("skill file");
    }

    fn skill_tool_call(arguments: serde_json::Value) -> AgentToolCall {
        AgentToolCall {
            id: "tool-1".to_owned(),
            name: "Skill".to_owned(),
            arguments,
        }
    }

    fn skill_store(root: &std::path::Path) -> SkillStore {
        SkillStore::load(&[], &[root.to_path_buf()], Vec::new()).expect("skill store")
    }

    #[test]
    fn execute_invoke_skill_expands_named_string_arguments() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(temp.path(), "review", "prompt", "Review $target.");
        let store = skill_store(temp.path());

        let result = execute_invoke_skill(
            Some(&store),
            &skill_tool_call(json!({
                "skill": "review",
                "arguments": {
                    "target": "src/lib.rs",
                    "ignored": 42
                }
            })),
        );

        assert_eq!(result, ToolResult::ok("Review src/lib.rs.\n"));
    }

    #[test]
    fn execute_invoke_skill_rejects_disabled_missing_and_flow_cases() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(temp.path(), "manual-flow", "flow", "Manual only.");
        let store = skill_store(temp.path());

        let no_store = execute_invoke_skill(None, &skill_tool_call(json!({"skill": "review"})));
        assert_eq!(no_store.content, "skill system is not enabled");
        assert!(no_store.is_error);

        let missing_name =
            execute_invoke_skill(Some(&store), &skill_tool_call(json!({"arguments": {}})));
        assert_eq!(
            missing_name.content,
            "Skill requires a `skill` string argument"
        );

        let missing_skill =
            execute_invoke_skill(Some(&store), &skill_tool_call(json!({"skill": "review"})));
        assert_eq!(missing_skill.content, "skill `review` is not available");

        let flow = execute_invoke_skill(
            Some(&store),
            &skill_tool_call(json!({"skill": "manual-flow"})),
        );
        assert_eq!(
            flow.content,
            "skill `manual-flow` is type `flow` and can only be invoked manually via /skill:manual-flow"
        );
    }

    #[test]
    fn execute_invoke_skill_allows_multiple_invocations_in_same_turn() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(temp.path(), "review", "prompt", "Review $target.");
        write_skill(temp.path(), "summarize", "prompt", "Summarize $target.");
        let store = skill_store(temp.path());

        let first = execute_invoke_skill(
            Some(&store),
            &skill_tool_call(json!({
                "skill": "review",
                "arguments": {"target": "src/lib.rs"}
            })),
        );
        let second = execute_invoke_skill(
            Some(&store),
            &skill_tool_call(json!({
                "skill": "summarize",
                "arguments": {"target": "src/main.rs"}
            })),
        );

        assert_eq!(first, ToolResult::ok("Review src/lib.rs.\n"));
        assert_eq!(second, ToolResult::ok("Summarize src/main.rs.\n"));
    }

    #[test]
    fn skill_tool_request_converts_only_string_named_arguments() {
        let request = skill_tool_request(&json!({
            "skill": "review",
            "arguments": {
                "target": "src/lib.rs",
                "count": 3,
                "flag": true
            }
        }))
        .expect("request");
        let invocation = request.into_invocation();

        assert_eq!(invocation.name, "review");
        assert_eq!(
            invocation.named.get("target"),
            Some(&"src/lib.rs".to_owned())
        );
        assert!(!invocation.named.contains_key("count"));
        assert!(!invocation.named.contains_key("flag"));
        assert_eq!(
            invocation.raw_arguments,
            r#"{"count":3,"flag":true,"target":"src/lib.rs"}"#
        );
    }

    #[test]
    fn skill_is_manual_only_tracks_flow_manifest_type() {
        let prompt = crate::skills::LoadedSkill {
            name: "prompt".to_owned(),
            root: PathBuf::from("/tmp/prompt"),
            manifest: SkillManifest {
                name: "prompt".to_owned(),
                description: "Prompt".to_owned(),
                skill_type: SkillType::Prompt,
                when_to_use: None,
                disable_model_invocation: false,
                arguments: Vec::<SkillArgument>::new(),
                slash_commands: Vec::new(),
            },
            body: String::new(),
            source: SkillSource::default(),
        };
        let flow = crate::skills::LoadedSkill {
            manifest: SkillManifest {
                skill_type: SkillType::Flow,
                ..prompt.manifest.clone()
            },
            ..prompt.clone()
        };

        assert!(!skill_is_manual_only(&prompt));
        assert!(skill_is_manual_only(&flow));
    }

    fn fake_compaction_config() -> AgentConfig {
        let mut config = AgentConfig::for_model(neo_ai::ModelSpec {
            provider: neo_ai::ProviderId("fake".to_owned()),
            model: "fake".to_owned(),
            api: neo_ai::ApiKind::OpenAiChatCompletions,
            capabilities: neo_ai::ModelCapabilities::chat()
                .with_max_context_tokens(100_000)
                .with_max_output_tokens(4_096),
        });
        config = config.with_compaction(CompactionSettings {
            enabled: true,
            max_estimated_tokens: 100_000,
            keep_recent_messages: 4,
            trigger_ratio: 0.85,
            reserved_context_tokens: 50_000,
            max_recent_messages: 4,
            micro_enabled: false,
            micro_keep_recent: 20,
        });
        config
    }

    #[tokio::test]
    async fn compaction_only_turn_runs_compaction_without_model_reply() {
        let fake = neo_ai::providers::fake::FakeModelClient::new(vec![
            neo_ai::AiStreamEvent::TextDelta {
                text: "summary".to_owned(),
            },
            neo_ai::AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ]);
        let mut config = fake_compaction_config();
        config.manual_compact_request = Arc::new(std::sync::Mutex::new(Some(String::new())));
        let runtime = AgentRuntime::new(config, Arc::new(fake));
        let mut context = AgentContext::new();
        context.append_message(AgentMessage::user_text("hello"));
        context.append_message(AgentMessage::assistant(
            vec![Content::text("hi")],
            Vec::new(),
            StopReason::EndTurn,
        ));
        context.append_message(AgentMessage::user_text("world"));
        context.append_message(AgentMessage::assistant(
            vec![Content::text("yes")],
            Vec::new(),
            StopReason::EndTurn,
        ));

        let events: Vec<AgentEvent> = runtime
            .run_compaction_turn(&mut context)
            .try_collect()
            .await
            .expect("compaction turn succeeds");

        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::CompactionStarted { .. })),
            "missing CompactionStarted"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::CompactionApplied { .. })),
            "missing CompactionApplied"
        );
        assert!(
            !events.iter().any(|e| matches!(
                e,
                AgentEvent::MessageAppended {
                    message: AgentMessage::Assistant { .. },
                    ..
                }
            )),
            "compaction-only turn must not produce an assistant reply"
        );
        assert!(context.compaction_summary().is_some());
    }

    #[tokio::test]
    async fn compaction_turn_passes_custom_instruction_to_summary_llm() {
        let fake = neo_ai::providers::fake::FakeModelClient::new(vec![
            neo_ai::AiStreamEvent::TextDelta {
                text: "summary".to_owned(),
            },
            neo_ai::AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ]);
        let mut config = fake_compaction_config();
        config.manual_compact_request =
            Arc::new(std::sync::Mutex::new(Some("keep the todo list".to_owned())));
        let runtime = AgentRuntime::new(config, Arc::new(fake.clone()));
        let mut context = AgentContext::new();
        context.append_message(AgentMessage::user_text("hello"));
        context.append_message(AgentMessage::assistant(
            vec![Content::text("hi")],
            Vec::new(),
            StopReason::EndTurn,
        ));
        context.append_message(AgentMessage::user_text("world"));
        context.append_message(AgentMessage::assistant(
            vec![Content::text("yes")],
            Vec::new(),
            StopReason::EndTurn,
        ));

        let _events: Vec<AgentEvent> = runtime
            .run_compaction_turn(&mut context)
            .try_collect()
            .await
            .expect("compaction turn succeeds");

        let requests = fake.requests();
        let request = requests.first().expect("summary LLM was called");
        let last_message_text = request
            .messages
            .last()
            .and_then(|message| match message {
                neo_ai::ChatMessage::User { content } => Some(
                    content
                        .iter()
                        .filter_map(|part| match part {
                            neo_ai::ContentPart::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<String>(),
                ),
                _ => None,
            })
            .unwrap_or_default();
        assert!(
            last_message_text.contains("keep the todo list"),
            "instruction not in compaction prompt: {last_message_text}"
        );
    }
}
