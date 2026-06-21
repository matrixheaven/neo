use std::{
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock, atomic::AtomicBool},
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

use crate::goal::GoalManager;
use crate::skills::SkillStore;
use crate::tools::BackgroundTaskManager;
use crate::{
    AgentEvent, AgentMessage, AgentToolCall, CompactionPhase, CompactionReason, CompactionSummary,
    Content, InjectionManager, PermissionApprovalDecision, PermissionMode, PermissionOperation,
    PlanMode, PlanModeGuard, ProcessSupervisor, QueueKind, StopReason, TodoEventData, ToolAccess,
    ToolContext, ToolError, ToolRegistry, ToolResult, ToolUpdateCallback, check_plan_mode_guard,
    is_active_plan_file_path,
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
}

enum PermissionPreparation {
    Run(ToolAccess),
    Ask {
        operation: PermissionOperation,
        subject: String,
    },
    Deny(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolSchedulingClass {
    ParallelSafe,
    Exclusive,
    BlockingDialog,
}

#[allow(dead_code)]
struct PreparedToolCall {
    tool_call: AgentToolCall,
    result: PreparedToolCallResult,
    scheduling: ToolSchedulingClass,
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
    pub compaction: Option<CompactionSettings>,
    /// Maximum number of autonomous turns a goal may run before blocking.
    #[serde(default)]
    pub max_goal_turns: Option<u32>,
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
    /// Side-channel for `ExitPlanMode` Revise feedback, keyed by `tool_call.id`.
    /// Populated by the approval handler when the user picks Revise.
    #[serde(skip)]
    #[schemars(skip)]
    pub plan_review_feedback: Arc<Mutex<std::collections::HashMap<String, String>>>,
    /// Tool names approved for the current session via "Approve for this session".
    #[serde(skip)]
    #[schemars(skip)]
    pub session_approvals: Arc<Mutex<std::collections::HashSet<String>>>,
    /// Home directory used for plan file creation (e.g. `~/.neo`).
    /// Falls back to `workspace_root` if unset.
    pub home_dir: Option<PathBuf>,
    /// Shared todo list state. Used by `TodoTool` read mode and kept in sync
    /// with replayed/runtime `TodoUpdated` events.
    #[serde(skip)]
    #[schemars(skip)]
    pub todos: Arc<Mutex<Vec<TodoEventData>>>,
    /// Shared background task manager for Bash and `AskUserQuestion` background tasks.
    #[serde(skip)]
    #[schemars(skip)]
    pub background_tasks: BackgroundTaskManager,
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
            compaction: None,
            max_goal_turns: None,
            context_transform: None,
            before_tool_call: None,
            async_before_tool_call: None,
            after_tool_call: None,
            async_after_tool_call: None,
            approval_handler: None,
            async_approval_handler: None,
            plan_mode: Arc::new(RwLock::new(PlanMode::default())),
            plan_review_feedback: Arc::new(Mutex::new(std::collections::HashMap::new())),
            session_approvals: Arc::new(Mutex::new(std::collections::HashSet::new())),
            home_dir: None,
            todos: Arc::new(Mutex::new(Vec::new())),
            background_tasks: BackgroundTaskManager::new(),
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
    pub const fn with_permission_mode(mut self, mode: PermissionMode) -> Self {
        self.permission_mode = mode;
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

    /// Replace the shared plan-mode state. Useful when constructing from a
    /// pre-existing state (e.g. after replay).
    #[must_use]
    pub fn with_plan_mode(mut self, plan_mode: Arc<RwLock<PlanMode>>) -> Self {
        self.plan_mode = plan_mode;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompactionSettings {
    pub enabled: bool,
    pub max_estimated_tokens: usize,
    pub keep_recent_messages: usize,
}

impl CompactionSettings {
    #[must_use]
    pub const fn new(max_estimated_tokens: usize, keep_recent_messages: usize) -> Self {
        Self {
            enabled: true,
            max_estimated_tokens,
            keep_recent_messages,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentContext {
    messages: Vec<AgentMessage>,
    turns: u32,
    cancelled: bool,
    steering_queue: Vec<AgentMessage>,
    follow_up_queue: Vec<AgentMessage>,
    /// Skill context injected before the next user message in the current turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    skill_context: Option<AgentMessage>,
    compaction_summary: Option<CompactionSummary>,
    /// Whether plan mode was active at the end of the last replayed/exected turn.
    #[serde(default)]
    plan_mode_active: bool,
    /// The plan id from the last `PlanModeEntered` event, if any.
    #[serde(default)]
    plan_mode_id: Option<String>,
    /// Latest todo list state, restored on resume replay.
    #[serde(default)]
    todos: Vec<TodoEventData>,
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
        let kept = self.messages.split_off(keep_from);
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

    pub fn cancel(&mut self) {
        self.cancelled = true;
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    #[must_use]
    pub fn from_replay<'a>(events: impl IntoIterator<Item = &'a AgentEvent>) -> Self {
        let mut context = Self::new();
        for event in events {
            match event {
                AgentEvent::MessageAppended { message } => {
                    context.append_message(message.clone());
                }
                AgentEvent::TurnFinished { turn, stop_reason } => {
                    context.turns = context.turns.max(*turn);
                    if matches!(stop_reason, StopReason::Cancelled) {
                        context.cancel();
                    }
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
                AgentEvent::PlanModeExited { .. } | AgentEvent::PlanModeCancelled { .. } => {
                    context.plan_mode_active = false;
                }
                AgentEvent::PlanUpdated { enabled, .. } => {
                    context.plan_mode_active = *enabled;
                }
                AgentEvent::TodoUpdated { todos, .. } => {
                    context.todos.clone_from(todos);
                }
                // QuestionRequested is interactive — not replayed.
                _ => {}
            }
        }
        context.messages = drop_incomplete_trailing_tool_turn(context.messages);
        context
    }
}

fn drop_incomplete_trailing_tool_turn(messages: Vec<AgentMessage>) -> Vec<AgentMessage> {
    let Some(assistant_index) = messages.iter().rposition(|message| {
        matches!(
            message,
            AgentMessage::Assistant {
                tool_calls,
                stop_reason: StopReason::ToolUse,
                ..
            } if !tool_calls.is_empty()
        )
    }) else {
        return messages;
    };

    if messages[assistant_index + 1..].iter().any(|message| {
        matches!(
            message,
            AgentMessage::User { .. } | AgentMessage::Assistant { .. }
        )
    }) {
        return messages;
    }

    let AgentMessage::Assistant { tool_calls, .. } = &messages[assistant_index] else {
        return messages;
    };
    let mut missing_tool_result_ids = tool_calls
        .iter()
        .map(|tool_call| tool_call.id.as_str())
        .collect::<Vec<_>>();
    for message in &messages[assistant_index + 1..] {
        let AgentMessage::ToolResult { tool_call_id, .. } = message else {
            continue;
        };
        if let Some(index) = missing_tool_result_ids
            .iter()
            .position(|id| *id == tool_call_id)
        {
            missing_tool_result_ids.remove(index);
        }
    }
    if missing_tool_result_ids.is_empty() {
        messages
    } else {
        messages[..assistant_index].to_vec()
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
    skill_invocation_active: Arc<AtomicBool>,
    goal_manager: Option<Arc<GoalManager>>,
}

impl AgentRuntime {
    #[must_use]
    pub fn new(config: AgentConfig, model: Arc<dyn ModelClient>) -> Self {
        Self {
            config,
            model,
            tools: None,
            skills: None,
            skill_invocation_active: Arc::new(AtomicBool::new(false)),
            goal_manager: None,
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
            skill_invocation_active: Arc::new(AtomicBool::new(false)),
            goal_manager: None,
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
            skill_invocation_active: Arc::new(AtomicBool::new(false)),
            goal_manager: None,
        }
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
        if context.is_cancelled() {
            let turn = context.turns.saturating_add(1);
            return terminal_lifecycle_stream(turn, StopReason::Cancelled);
        }
        if let Ok(mut todos) = self.config.todos.lock() {
            todos.clone_from(&context.todos);
        }

        let live_context = context.clone();
        let model = Arc::clone(&self.model);
        let tools = self.tools.clone();
        let skills = self.skills.clone();
        let skill_invocation_active = Arc::clone(&self.skill_invocation_active);
        let goal_manager = self.goal_manager.clone();
        let config = self.config.clone();
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
                skill_invocation_active,
                goal_manager,
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
}

fn terminal_lifecycle_stream<'a>(turn: u32, stop_reason: StopReason) -> AgentEventStream<'a> {
    stream::iter([
        Ok(AgentEvent::RunStarted { turn }),
        Ok(AgentEvent::TurnFinished { turn, stop_reason }),
        Ok(AgentEvent::RunFinished { turn, stop_reason }),
    ])
    .boxed()
}

struct SpawnedRun<'a> {
    receiver: mpsc::UnboundedReceiver<Result<AgentEvent, AgentRuntimeError>>,
    final_receiver: Option<oneshot::Receiver<AgentContext>>,
    context: &'a mut AgentContext,
}

/// Compute the workspace-scoped plans directory.
fn plan_mode_plans_dir(config: &AgentConfig) -> Option<PathBuf> {
    let home = config.home_dir.as_deref()?;
    if let Some(workdir) = config.workspace_root.as_deref() {
        Some(crate::session::workspace_sessions_dir(&home.join("sessions"), workdir).join("plans"))
    } else {
        Some(home.join("plans"))
    }
}

async fn chat_request(config: &AgentConfig, context: &AgentContext) -> ChatRequest {
    let mut messages = Vec::new();
    if let Some(system_prompt) = &config.system_prompt {
        messages.push(AgentMessage::system_text(system_prompt).to_chat_message());
    }
    if let Some(workspace_context) = workspace_context_message(config) {
        messages.push(workspace_context.to_chat_message());
    }
    let context_messages = if let Some(transform) = &config.context_transform {
        transform(context.messages())
    } else {
        context.messages.clone()
    };
    messages.extend(context_messages.iter().map(|message| {
        if config.replay_reasoning {
            message.to_chat_message()
        } else {
            without_reasoning_content(message.to_chat_message())
        }
    }));
    let mut injector = InjectionManager::new(Arc::clone(&config.plan_mode));
    for injected in injector.inject(context).await {
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

struct EventEmitter {
    sender: mpsc::UnboundedSender<Result<AgentEvent, AgentRuntimeError>>,
    context: AgentContext,
}

impl EventEmitter {
    fn new(
        sender: mpsc::UnboundedSender<Result<AgentEvent, AgentRuntimeError>>,
        context: AgentContext,
    ) -> Self {
        Self { sender, context }
    }

    fn emit(&mut self, event: AgentEvent) {
        Self::apply_to_context(&mut self.context, &event);
        let _ = self.sender.send(Ok(event));
    }

    fn sink(&self) -> EventSink {
        EventSink {
            sender: self.sender.clone(),
        }
    }

    fn send_error(&mut self, err: AgentRuntimeError) -> Result<(), AgentRuntimeError> {
        self.sender
            .send(Err(err))
            .map_err(|_| AgentRuntimeError::Cancelled)
    }

    fn apply_to_context(context: &mut AgentContext, event: &AgentEvent) {
        match event {
            AgentEvent::MessageAppended { message } => context.append_message(message.clone()),
            AgentEvent::TurnFinished { turn, stop_reason } => {
                context.turns = context.turns.max(*turn);
                if matches!(stop_reason, StopReason::Cancelled) {
                    context.cancel();
                }
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
            AgentEvent::PlanModeExited { .. } | AgentEvent::PlanModeCancelled { .. } => {
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

trait EventPublisher {
    fn emit(&mut self, event: AgentEvent);
}

impl EventPublisher for EventEmitter {
    fn emit(&mut self, event: AgentEvent) {
        Self::emit(self, event);
    }
}

#[derive(Clone)]
struct EventSink {
    sender: mpsc::UnboundedSender<Result<AgentEvent, AgentRuntimeError>>,
}

impl EventSink {
    /// Emit an event by value without needing `&mut self`.
    fn emit_event(&self, event: AgentEvent) {
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
    skill_invocation_active: Arc<AtomicBool>,
    goal_manager: Option<Arc<GoalManager>>,
    emitter: &mut EventEmitter,
    cancel_token: CancellationToken,
    process_supervisor: ProcessSupervisor,
) -> Result<(), AgentRuntimeError> {
    skill_invocation_active.store(false, std::sync::atomic::Ordering::SeqCst);
    let mut final_turn: u32;
    let mut final_stop_reason = StopReason::EndTurn;
    let mut pending_messages = drain_steering_queue(&config, emitter);

    loop {
        if !pending_messages.is_empty() {
            append_queued_messages(emitter, pending_messages);
        }

        maybe_compact(&config, emitter);

        if let Some((turn, stop_reason)) = terminal_pre_model_stop(emitter, &cancel_token) {
            final_turn = turn;
            final_stop_reason = stop_reason;
            break;
        }

        let turn = emitter.context.turns.saturating_add(1);
        let request = chat_request(&config, &emitter.context).await;
        validate_model_capabilities(&request)?;
        let assistant = run_model_turn(
            Arc::clone(&model),
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
            pending_messages = drain_next_pending_queue(&config, emitter);
            if pending_messages.is_empty() {
                if let Some(messages) =
                    goal_continuation_messages(goal_manager.as_deref(), &config).await
                {
                    pending_messages = messages;
                    continue;
                }
                break;
            }
            continue;
        };
        let tool_calls = model_tool_calls.clone();

        let Some(registry) = &tools else {
            break;
        };
        let mut tool_results = execute_tool_calls(
            &config,
            registry,
            skills.as_deref(),
            &skill_invocation_active,
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
        for (tool_call, result) in &tool_results {
            let message = AgentMessage::tool_result(
                tool_call.id.clone(),
                tool_call.name.clone(),
                vec![Content::text(result.content.clone())],
                result.is_error,
            );
            emitter.emit(AgentEvent::MessageAppended { message });
        }
        // Inject plan data into ExitPlanMode tool results for TUI PlanBox rendering.
        {
            let pm = config.plan_mode.read().unwrap();
            if pm.is_active()
                && let Some(plan_data) = pm.data().ok().flatten()
            {
                for (tool_call, result) in &mut tool_results {
                    if tool_call.name == "ExitPlanMode" && result.details.is_none() {
                        result.details = Some(serde_json::json!({
                            "plan_content": plan_data.content,
                            "plan_path": plan_data.path.display().to_string(),
                        }));
                    }
                }
            }
        }
        // Intercept plan-mode tools and TodoList tool to emit structured events.
        for (tool_call, result) in &tool_results {
            if result.terminate {
                match tool_call.name.as_str() {
                    "EnterPlanMode" => {
                        let mut pm = config.plan_mode.write().unwrap();
                        let id = if let Some(plans_dir) = plan_mode_plans_dir(&config) {
                            if let Ok(data) = pm.enter(&plans_dir, true) {
                                data.id
                            } else {
                                pm.enter_in_memory();
                                pm.plan_id().unwrap_or("").to_owned()
                            }
                        } else {
                            pm.enter_in_memory();
                            pm.plan_id().unwrap_or("").to_owned()
                        };
                        drop(pm);
                        emitter.emit(AgentEvent::PlanModeEntered {
                            turn,
                            id: id.clone(),
                        });
                        emitter.emit(AgentEvent::PlanUpdated {
                            turn,
                            enabled: true,
                        });
                    }
                    "ExitPlanMode" => {
                        // Exit plan mode and emit events. In non-auto permission
                        // mode, the TUI layer can intercept with an approval
                        // dialog before this code runs; if we reach here the
                        // exit has been approved (or auto-approved).
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
                    _ => {}
                }
            }
            if tool_call.name == "TodoList"
                && !result.is_error
                && let Some(details) = &result.details
                && let Some(todos_val) = details.get("todos")
                && let Ok(todos) = serde_json::from_value::<Vec<TodoEventData>>(todos_val.clone())
            {
                // Sync shared todo state.
                if let Ok(mut shared) = config.todos.lock() {
                    shared.clone_from(&todos);
                }
                emitter.emit(AgentEvent::TodoUpdated { turn, todos });
            }
        }
        if terminates_tool_batch(&tool_results) {
            break;
        }
        pending_messages = drain_steering_queue(&config, emitter);
    }

    process_supervisor.cleanup_all().await;
    emit_run_finished(emitter, final_turn, final_stop_reason);
    Ok(())
}

async fn goal_continuation_messages(
    manager: Option<&GoalManager>,
    config: &AgentConfig,
) -> Option<Vec<AgentMessage>> {
    let manager = manager?;
    let goal = manager.active()?;
    let max_turns = config.max_goal_turns.unwrap_or(30);
    if goal.turn_count >= max_turns {
        let _ = manager
            .update_status(
                crate::goal::GoalStatus::Blocked,
                Some(format!("turn budget of {max_turns} exceeded")),
            )
            .await;
        return None;
    }
    let _ = manager.increment_turn().await;
    let objective = goal.objective;
    Some(vec![AgentMessage::system_text(format!(
        "Goal still active: {objective}. Continue making progress. \
         Use `UpdateGoalStatus` when the goal is complete or blocked, \
         or `GetGoalStatus` to check current state."
    ))])
}

fn drain_next_pending_queue(config: &AgentConfig, emitter: &mut EventEmitter) -> Vec<AgentMessage> {
    let steering = drain_steering_queue(config, emitter);
    if steering.is_empty() {
        drain_follow_up_queue(config, emitter)
    } else {
        steering
    }
}

fn drain_steering_queue(config: &AgentConfig, emitter: &mut EventEmitter) -> Vec<AgentMessage> {
    let messages = take_messages(&emitter.context.steering_queue, config.steering_queue_mode);
    emit_queue_drained(emitter, QueueKind::Steering, messages.len());
    messages
}

fn drain_follow_up_queue(config: &AgentConfig, emitter: &mut EventEmitter) -> Vec<AgentMessage> {
    let messages = take_messages(
        &emitter.context.follow_up_queue,
        config.follow_up_queue_mode,
    );
    emit_queue_drained(emitter, QueueKind::FollowUp, messages.len());
    messages
}

fn emit_queue_drained(emitter: &mut EventEmitter, kind: QueueKind, count: usize) {
    if count > 0 {
        emitter.emit(AgentEvent::QueueDrained { kind, count });
    }
}

fn emit_run_finished(emitter: &mut EventEmitter, turn: u32, stop_reason: StopReason) {
    emitter.emit(AgentEvent::RunFinished { turn, stop_reason });
}

fn terminal_pre_model_stop(
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
) -> Option<(u32, StopReason)> {
    if emitter.context.is_cancelled() || cancel_token.is_cancelled() {
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

fn maybe_compact(config: &AgentConfig, emitter: &mut EventEmitter) {
    let Some(settings) = config.compaction else {
        return;
    };
    if !settings.enabled {
        return;
    }
    let estimated_tokens = estimate_messages_tokens(emitter.context.messages());
    if estimated_tokens <= settings.max_estimated_tokens {
        return;
    }
    emitter.emit(AgentEvent::CompactionStarted {
        reason: CompactionReason::Threshold,
        tokens_before: estimated_tokens,
        message_count: emitter.context.messages().len(),
    });
    emitter.emit(AgentEvent::CompactionProgress {
        phase: CompactionPhase::Estimating,
        percent: 15,
    });
    let keep_recent = settings
        .keep_recent_messages
        .min(emitter.context.messages().len());
    let first_kept_message_index = compact_keep_boundary(
        emitter.context.messages(),
        emitter.context.messages().len().saturating_sub(keep_recent),
    );
    if first_kept_message_index == 0 {
        return;
    }
    emitter.emit(AgentEvent::CompactionProgress {
        phase: CompactionPhase::SelectingBoundary,
        percent: 35,
    });
    emitter.emit(AgentEvent::CompactionProgress {
        phase: CompactionPhase::Summarizing,
        percent: 70,
    });
    let summary_text = summarize_messages(&emitter.context.messages()[..first_kept_message_index]);
    let summary = CompactionSummary {
        summary: summary_text,
        tokens_before: estimated_tokens,
        first_kept_message_index,
    };
    emitter.emit(AgentEvent::CompactionProgress {
        phase: CompactionPhase::Applying,
        percent: 90,
    });
    emitter.emit(AgentEvent::CompactionApplied { summary });
}

fn compact_keep_boundary(messages: &[AgentMessage], proposed_index: usize) -> usize {
    let mut index = proposed_index.min(messages.len());
    while index < messages.len() && matches!(messages[index], AgentMessage::ToolResult { .. }) {
        index += 1;
    }

    if let Some(AgentMessage::Assistant {
        tool_calls,
        stop_reason: StopReason::ToolUse,
        ..
    }) = index
        .checked_sub(1)
        .and_then(|previous| messages.get(previous))
    {
        let mut result_count = 0;
        for message in &messages[index..] {
            match message {
                AgentMessage::ToolResult { .. } => result_count += 1,
                AgentMessage::User { .. } | AgentMessage::Assistant { .. } => break,
                AgentMessage::System { .. } => {}
            }
        }
        if result_count < tool_calls.len() {
            while index < messages.len()
                && !matches!(
                    messages[index],
                    AgentMessage::User { .. } | AgentMessage::Assistant { .. }
                )
            {
                index += 1;
            }
        }
    }

    index
}

fn estimate_messages_tokens(messages: &[AgentMessage]) -> usize {
    messages.iter().map(estimate_message_tokens).sum()
}

fn estimate_message_tokens(message: &AgentMessage) -> usize {
    let chars = match message {
        AgentMessage::System { content }
        | AgentMessage::User { content }
        | AgentMessage::ToolResult { content, .. } => estimate_content_chars(content),
        AgentMessage::Assistant {
            content,
            tool_calls,
            ..
        } => {
            let content_chars = estimate_content_chars(content);
            let tool_chars = tool_calls
                .iter()
                .map(|call| call.name.len() + call.arguments.to_string().len())
                .sum::<usize>();
            content_chars + tool_chars
        }
    };
    chars.div_ceil(4)
}

fn estimate_content_chars(content: &[Content]) -> usize {
    content
        .iter()
        .map(|part| match part {
            Content::Text { text } => text.len(),
            Content::Thinking { .. } => 0,
            Content::Image { .. } => 4800,
        })
        .sum()
}

fn summarize_messages(messages: &[AgentMessage]) -> String {
    let user_messages = messages
        .iter()
        .filter(|message| matches!(message, AgentMessage::User { .. }))
        .count();
    let assistant_messages = messages
        .iter()
        .filter(|message| matches!(message, AgentMessage::Assistant { .. }))
        .count();
    let tool_results = messages
        .iter()
        .filter(|message| matches!(message, AgentMessage::ToolResult { .. }))
        .count();
    format!(
        "Compacted {count} messages: {user_messages} user, {assistant_messages} assistant, {tool_results} tool result.",
        count = messages.len()
    )
}

fn take_messages(queue: &[AgentMessage], mode: QueueMode) -> Vec<AgentMessage> {
    let count = match mode {
        QueueMode::All => queue.len(),
        QueueMode::OneAtATime => usize::from(!queue.is_empty()),
    };
    queue.iter().take(count).cloned().collect()
}

fn terminates_tool_batch(tool_results: &[(AgentToolCall, ToolResult)]) -> bool {
    !tool_results.is_empty() && tool_results.iter().all(|(_, result)| result.terminate)
}

#[allow(clippy::too_many_arguments)]
async fn execute_tool_calls(
    config: &AgentConfig,
    registry: &ToolRegistry,
    skills: Option<&SkillStore>,
    skill_invocation_active: &AtomicBool,
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
            skill_invocation_active,
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
            skill_invocation_active,
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
            skill_invocation_active,
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
        skill_invocation_active,
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
        && config.permission_mode != PermissionMode::Auto
        && exit_plan_mode_has_reviewable_plan(config)
    {
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
    skill_invocation_active: &AtomicBool,
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
                    skill_invocation_active,
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
    skill_invocation_active: &AtomicBool,
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
            let mut result = run_tool_with_cancel(
                skills,
                skill_invocation_active,
                registry,
                &tool_call,
                &tool_context,
                &cancel_token,
            )
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
    skill_invocation_active: &AtomicBool,
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
            let result = run_tool_with_cancel(
                skills,
                skill_invocation_active,
                registry,
                tool_call,
                &context,
                cancel_token,
            )
            .await;
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
    let scheduling = scheduling_class_for_preparation(config, tool_call, &preparation);

    match preparation {
        PermissionPreparation::Run(access) => PreparedToolCall {
            tool_call: tool_call.clone(),
            result: PreparedToolCallResult::Run,
            scheduling,
            access,
        },
        PermissionPreparation::Deny(message) => PreparedToolCall {
            tool_call: tool_call.clone(),
            result: PreparedToolCallResult::Skip(ToolResult::error(message)),
            scheduling,
            access: ToolAccess::none(),
        },
        PermissionPreparation::Ask { operation, subject } => {
            match resolve_approval(
                config,
                turn,
                tool_call,
                operation,
                subject,
                emitter,
                cancel_token,
            )
            .await
            {
                Some(result) => PreparedToolCall {
                    tool_call: tool_call.clone(),
                    result: PreparedToolCallResult::Skip(result),
                    scheduling,
                    access: ToolAccess::none(),
                },
                None => PreparedToolCall {
                    tool_call: tool_call.clone(),
                    result: PreparedToolCallResult::Run,
                    scheduling,
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
    // 1. Plan-mode hard guard.
    {
        let Ok(plan_mode) = config.plan_mode.read() else {
            return PermissionPreparation::Deny("plan mode state is unavailable".to_owned());
        };
        if plan_mode.is_active() {
            match check_plan_mode_guard(
                &plan_mode,
                config.workspace_root.as_deref(),
                &tool_call.name,
                &tool_call.arguments,
            ) {
                PlanModeGuard::Allow => {}
                PlanModeGuard::Deny { message } => {
                    return PermissionPreparation::Deny(message);
                }
            }
        }
    }

    // 2. Auto mode hard deny for AskUserQuestion.
    if tool_call.name == "AskUserQuestion" && config.permission_mode == PermissionMode::Auto {
        return PermissionPreparation::Deny(
            "AskUserQuestion is disabled while auto permission mode is active".to_owned(),
        );
    }

    // 3. Background AskUserQuestion does not need an approval dialog in any mode.
    if tool_call.name == "AskUserQuestion" && ask_user_runs_in_background(tool_call) {
        return PermissionPreparation::Run(access_for_tool(tool_call, true));
    }

    // 4. Auto mode approves everything else.
    if config.permission_mode == PermissionMode::Auto {
        return PermissionPreparation::Run(access_for_tool(tool_call, true));
    }

    // 5. EnterPlanMode is auto-approved in all modes.
    if tool_call.name == "EnterPlanMode" {
        return PermissionPreparation::Run(access_for_tool(tool_call, true));
    }

    // 6. Session approvals.
    if config
        .session_approvals
        .lock()
        .ok()
        .is_some_and(|set| set.contains(&tool_call.name))
    {
        return PermissionPreparation::Run(access_for_tool(tool_call, true));
    }

    // 7. ExitPlanMode review in manual/yolo when plan is non-empty.
    if tool_call.name == "ExitPlanMode" {
        if exit_plan_mode_has_reviewable_plan(config) {
            return PermissionPreparation::Ask {
                operation: PermissionOperation::PlanTransition,
                subject: "Exit plan mode".to_owned(),
            };
        }
        return PermissionPreparation::Run(access_for_tool(tool_call, true));
    }

    // 8. Plan-mode helper approvals (e.g. writing the active plan file).
    if matches!(tool_call.name.as_str(), "Write" | "Edit") {
        let Ok(plan_mode) = config.plan_mode.read() else {
            return PermissionPreparation::Deny("plan mode state is unavailable".to_owned());
        };
        if let Some(path) = tool_call.arguments.get("path").and_then(|v| v.as_str())
            && plan_mode.is_active()
            && is_active_plan_file_path(&plan_mode, config.workspace_root.as_deref(), path)
        {
            return PermissionPreparation::Run(access_for_tool(tool_call, true));
        }
    }

    // 9. Yolo mode approves all remaining tools.
    if config.permission_mode == PermissionMode::Yolo {
        return PermissionPreparation::Run(access_for_tool(tool_call, true));
    }

    // 10. Default safe tools in manual mode.
    if is_default_approved_tool(tool_call) {
        return PermissionPreparation::Run(access_for_tool(tool_call, true));
    }

    // 11. Manual fallback ask.
    let (operation, subject) = permission_operation_for_tool(tool_call)
        .unwrap_or((PermissionOperation::Tool, tool_call.name.clone()));
    PermissionPreparation::Ask { operation, subject }
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

async fn resolve_approval(
    config: &AgentConfig,
    turn: u32,
    tool_call: &AgentToolCall,
    operation: PermissionOperation,
    subject: String,
    emitter: &mut impl EventPublisher,
    cancel_token: &CancellationToken,
) -> Option<ToolResult> {
    let request = ApprovalRequest {
        turn,
        id: tool_call.id.clone(),
        operation,
        subject: subject.clone(),
        arguments: tool_call.arguments.clone(),
    };
    emitter.emit(AgentEvent::ApprovalRequested {
        turn: request.turn,
        id: request.id.clone(),
        operation: request.operation,
        subject: request.subject.clone(),
        arguments: request.arguments.clone(),
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
            if let Ok(mut set) = config.session_approvals.lock() {
                set.insert(tool_call.name.clone());
            }
            None
        }
        PermissionApprovalDecision::Reject => {
            // ExitPlanMode revise feedback is delivered via the plan_review_feedback side-channel.
            if tool_call.name == "ExitPlanMode"
                && let Some(feedback) = config
                    .plan_review_feedback
                    .lock()
                    .ok()
                    .and_then(|mut m| m.remove(&tool_call.id))
            {
                return Some(ToolResult::ok(format!(
                    "User requested revisions. Plan mode remains active.\n\nFeedback: {feedback}"
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
    emitter.emit(AgentEvent::ShellCommandFinished {
        turn,
        id: tool_call.id.clone(),
        exit_code,
        stdout,
        stderr,
        truncated,
    });
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
    skill_invocation_active: &AtomicBool,
    registry: &ToolRegistry,
    tool_call: &AgentToolCall,
    tool_context: &ToolContext,
    cancel_token: &CancellationToken,
) -> ToolResult {
    if tool_call.name == "Skill" {
        return execute_invoke_skill(skills, skill_invocation_active, tool_call);
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

fn execute_invoke_skill(
    skills: Option<&SkillStore>,
    skill_invocation_active: &AtomicBool,
    tool_call: &AgentToolCall,
) -> ToolResult {
    let Some(skills) = skills else {
        return ToolResult::error("skill system is not enabled");
    };
    if skill_invocation_active.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return ToolResult::error("nested skill invocation is not allowed");
    }
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
        .map_err(AgentRuntimeError::Tool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{SkillArgument, SkillManifest, SkillSource, SkillType};
    use serde_json::json;
    use std::sync::atomic::Ordering;

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
        SkillStore::load(None, &[], &[root.to_path_buf()], Vec::new()).expect("skill store")
    }

    #[test]
    fn execute_invoke_skill_expands_named_string_arguments() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(temp.path(), "review", "prompt", "Review $target.");
        let store = skill_store(temp.path());
        let active = AtomicBool::new(false);

        let result = execute_invoke_skill(
            Some(&store),
            &active,
            &skill_tool_call(json!({
                "skill": "review",
                "arguments": {
                    "target": "src/lib.rs",
                    "ignored": 42
                }
            })),
        );

        assert_eq!(result, ToolResult::ok("Review src/lib.rs.\n"));
        assert!(active.load(Ordering::SeqCst));
    }

    #[test]
    fn execute_invoke_skill_rejects_disabled_missing_nested_and_flow_cases() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(temp.path(), "manual-flow", "flow", "Manual only.");
        let store = skill_store(temp.path());

        let no_store = execute_invoke_skill(
            None,
            &AtomicBool::new(false),
            &skill_tool_call(json!({"skill": "review"})),
        );
        assert_eq!(no_store.content, "skill system is not enabled");
        assert!(no_store.is_error);

        let missing_name = execute_invoke_skill(
            Some(&store),
            &AtomicBool::new(false),
            &skill_tool_call(json!({"arguments": {}})),
        );
        assert_eq!(
            missing_name.content,
            "Skill requires a `skill` string argument"
        );

        let missing_skill = execute_invoke_skill(
            Some(&store),
            &AtomicBool::new(false),
            &skill_tool_call(json!({"skill": "review"})),
        );
        assert_eq!(missing_skill.content, "skill `review` is not available");

        let flow = execute_invoke_skill(
            Some(&store),
            &AtomicBool::new(false),
            &skill_tool_call(json!({"skill": "manual-flow"})),
        );
        assert_eq!(
            flow.content,
            "skill `manual-flow` is type `flow` and can only be invoked manually via /skill:manual-flow"
        );

        let nested = execute_invoke_skill(
            Some(&store),
            &AtomicBool::new(true),
            &skill_tool_call(json!({"skill": "manual-flow"})),
        );
        assert_eq!(nested.content, "nested skill invocation is not allowed");
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
}
