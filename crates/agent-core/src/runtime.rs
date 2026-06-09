use std::{future::Future, path::PathBuf, sync::Arc};

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

use crate::{
    AgentEvent, AgentMessage, AgentToolCall, CompactionSummary, Content, PermissionDecision,
    PermissionOperation, PermissionPolicy, QueueKind, StopReason, ToolContext, ToolError,
    ToolRegistry, ToolResult,
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
pub type ApprovalHandler = Arc<dyn Fn(&ApprovalRequest) -> PermissionDecision + Send + Sync>;
pub type AsyncApprovalHandler =
    Arc<dyn Fn(ApprovalRequest) -> BoxFuture<'static, PermissionDecision> + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalRequest {
    pub turn: u32,
    pub id: String,
    pub operation: PermissionOperation,
    pub subject: String,
    pub arguments: serde_json::Value,
}

enum ToolPreparation {
    Run(ToolContext),
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
    pub max_turns: u32,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub replay_reasoning: bool,
    pub tools: Vec<ToolSpec>,
    pub steering_queue_mode: QueueMode,
    pub follow_up_queue_mode: QueueMode,
    pub tool_execution_mode: ToolExecutionMode,
    pub tool_permission_policy: PermissionPolicy,
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
}

impl AgentConfig {
    #[must_use]
    pub fn for_model(model: ModelSpec) -> Self {
        Self {
            model,
            workspace_root: None,
            system_prompt: None,
            max_turns: 8,
            temperature: None,
            max_tokens: None,
            reasoning_effort: None,
            replay_reasoning: true,
            tools: Vec::new(),
            steering_queue_mode: QueueMode::All,
            follow_up_queue_mode: QueueMode::All,
            tool_execution_mode: ToolExecutionMode::Parallel,
            tool_permission_policy: PermissionPolicy::default(),
            compaction: None,
            context_transform: None,
            before_tool_call: None,
            async_before_tool_call: None,
            after_tool_call: None,
            async_after_tool_call: None,
            approval_handler: None,
            async_approval_handler: None,
        }
    }

    #[must_use]
    pub fn with_system_prompt(mut self, system_prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(system_prompt.into());
        self
    }

    #[must_use]
    pub fn with_max_turns(mut self, max_turns: u32) -> Self {
        self.max_turns = max_turns;
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
    pub const fn with_tool_permission_policy(mut self, policy: PermissionPolicy) -> Self {
        self.tool_permission_policy = policy;
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
        handler: impl Fn(&ApprovalRequest) -> PermissionDecision + Send + Sync + 'static,
    ) -> Self {
        self.approval_handler = Some(Arc::new(handler));
        self
    }

    #[must_use]
    pub fn with_async_approval_handler<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(ApprovalRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = PermissionDecision> + Send + 'static,
    {
        self.async_approval_handler = Some(Arc::new(move |request| handler(request).boxed()));
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
    compaction_summary: Option<CompactionSummary>,
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
                _ => {}
            }
        }
        context
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
    #[error("maximum turns reached")]
    MaxTurns,
    #[error("turn cancelled")]
    Cancelled,
}

pub type AgentEventStream<'a> = stream::BoxStream<'a, Result<AgentEvent, AgentRuntimeError>>;

#[derive(Clone)]
pub struct AgentRuntime {
    config: AgentConfig,
    model: Arc<dyn ModelClient>,
    tools: Option<Arc<ToolRegistry>>,
}

impl AgentRuntime {
    #[must_use]
    pub fn new(config: AgentConfig, model: Arc<dyn ModelClient>) -> Self {
        Self {
            config,
            model,
            tools: None,
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
        }
    }

    #[must_use]
    pub fn config(&self) -> &AgentConfig {
        &self.config
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

        if context.turns >= self.config.max_turns {
            let turn = context.turns.saturating_add(1);
            return terminal_lifecycle_stream(turn, StopReason::MaxTurns);
        }

        let live_context = context.clone();
        let model = Arc::clone(&self.model);
        let tools = self.tools.clone();
        let config = self.config.clone();
        let (sender, receiver) = mpsc::unbounded_channel();
        let (final_sender, final_receiver) = oneshot::channel();

        tokio::spawn(async move {
            let mut emitter = EventEmitter::new(sender, live_context);
            emitter.emit(AgentEvent::RunStarted {
                turn: emitter.context.turns.saturating_add(1),
            });
            emitter.emit(AgentEvent::MessageAppended { message });
            if let Err(err) = run_agent_turn(model, config, tools, &mut emitter, cancel_token).await
            {
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

fn chat_request(config: &AgentConfig, context: &AgentContext) -> ChatRequest {
    let mut messages = Vec::new();
    if let Some(system_prompt) = &config.system_prompt {
        messages.push(AgentMessage::system_text(system_prompt).to_chat_message());
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

impl EventPublisher for EventSink {
    fn emit(&mut self, event: AgentEvent) {
        let _ = self.sender.send(Ok(event));
    }
}

async fn run_agent_turn(
    model: Arc<dyn ModelClient>,
    config: AgentConfig,
    tools: Option<Arc<ToolRegistry>>,
    emitter: &mut EventEmitter,
    cancel_token: CancellationToken,
) -> Result<(), AgentRuntimeError> {
    let mut final_turn: u32;
    let mut final_stop_reason = StopReason::EndTurn;
    let mut pending_messages = drain_steering_queue(&config, emitter);

    loop {
        if !pending_messages.is_empty() {
            append_queued_messages(emitter, pending_messages);
        }

        maybe_compact(&config, emitter);

        if let Some((turn, stop_reason)) = terminal_pre_model_stop(&config, emitter, &cancel_token)
        {
            final_turn = turn;
            final_stop_reason = stop_reason;
            break;
        }

        let turn = emitter.context.turns.saturating_add(1);
        let request = chat_request(&config, &emitter.context);
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
                break;
            }
            continue;
        };
        let tool_calls = model_tool_calls.clone();

        let Some(registry) = &tools else {
            break;
        };
        let tool_results =
            execute_tool_calls(&config, registry, turn, &tool_calls, emitter, &cancel_token)
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
        if terminates_tool_batch(&tool_results) {
            break;
        }
        pending_messages = drain_steering_queue(&config, emitter);
    }

    emit_run_finished(emitter, final_turn, final_stop_reason);
    Ok(())
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
    config: &AgentConfig,
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

    if emitter.context.turns >= config.max_turns {
        let turn = emitter.context.turns.saturating_add(1);
        emitter.emit(AgentEvent::TurnFinished {
            turn,
            stop_reason: StopReason::MaxTurns,
        });
        return Some((turn, StopReason::MaxTurns));
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
    let keep_recent = settings
        .keep_recent_messages
        .min(emitter.context.messages().len());
    let first_kept_message_index = emitter.context.messages().len().saturating_sub(keep_recent);
    if first_kept_message_index == 0 {
        return;
    }
    let compacted_messages = &emitter.context.messages()[..first_kept_message_index];
    let summary = CompactionSummary {
        summary: summarize_messages(compacted_messages),
        tokens_before: estimated_tokens,
        first_kept_message_index,
    };
    emitter.emit(AgentEvent::CompactionApplied { summary });
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

async fn execute_tool_calls(
    config: &AgentConfig,
    registry: &ToolRegistry,
    turn: u32,
    tool_calls: &[AgentToolCall],
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
) -> Result<Vec<(AgentToolCall, ToolResult)>, AgentRuntimeError> {
    match config.tool_execution_mode {
        ToolExecutionMode::Sequential => {
            execute_tool_calls_sequential(config, registry, turn, tool_calls, emitter, cancel_token)
                .await
        }
        ToolExecutionMode::Parallel => {
            execute_tool_calls_parallel(config, registry, turn, tool_calls, emitter, cancel_token)
                .await
        }
    }
}

async fn execute_tool_calls_sequential(
    config: &AgentConfig,
    registry: &ToolRegistry,
    turn: u32,
    tool_calls: &[AgentToolCall],
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
) -> Result<Vec<(AgentToolCall, ToolResult)>, AgentRuntimeError> {
    let tool_context = default_tool_context(config, cancel_token)?;
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
        if tool_call.name == "bash" {
            emit_shell_finished(turn, tool_call, &result, emitter);
        }
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

async fn execute_tool_calls_parallel(
    config: &AgentConfig,
    registry: &ToolRegistry,
    turn: u32,
    tool_calls: &[AgentToolCall],
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
) -> Result<Vec<(AgentToolCall, ToolResult)>, AgentRuntimeError> {
    let tool_context = default_tool_context(config, cancel_token)?;
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
            emitter.emit(AgentEvent::ToolExecutionFinished {
                turn,
                id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                result: result.clone(),
            });
            completed.push((index, tool_call, result));
            continue;
        }

        let config = config.clone();
        let tool_context = tool_context.clone();
        let cancel_token = cancel_token.clone();
        let mut sink = emitter.sink();
        running.push(async move {
            let tool_context = tokio::select! {
                preparation = prepare_tool_context(&config, &tool_context, turn, &tool_call, &mut sink) => {
                    match preparation {
                    ToolPreparation::Run(context) => context,
                    ToolPreparation::Skip(result) => {
                        return Ok::<_, AgentRuntimeError>((index, tool_call, result));
                    }
                    }
                }
                () = cancel_token.cancelled() => {
                    return Ok::<_, AgentRuntimeError>((index, tool_call, cancelled_tool_result()));
                }
            };
            let mut result = run_tool_with_cancel(
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
            AiStreamEvent::MessageEnd {
                stop_reason,
                usage: _,
            } => self.finish_current_message(turn, stop_reason.into(), emitter),
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

async fn prepare_and_run_tool(
    config: &AgentConfig,
    registry: &ToolRegistry,
    tool_context: &ToolContext,
    turn: u32,
    tool_call: &AgentToolCall,
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
) -> Result<ToolResult, AgentRuntimeError> {
    let preparation = tokio::select! {
        biased;
        preparation = prepare_tool_context(config, tool_context, turn, tool_call, emitter) => preparation,
        () = cancel_token.cancelled() => return Ok(cancelled_tool_result()),
    };
    match preparation {
        ToolPreparation::Run(context) => {
            Ok(run_tool_with_cancel(registry, tool_call, &context, cancel_token).await)
        }
        ToolPreparation::Skip(result) => Ok(result),
    }
}

async fn run_tool_with_cancel(
    registry: &ToolRegistry,
    tool_call: &AgentToolCall,
    tool_context: &ToolContext,
    cancel_token: &CancellationToken,
) -> ToolResult {
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

async fn prepare_tool_context(
    config: &AgentConfig,
    base_context: &ToolContext,
    turn: u32,
    tool_call: &AgentToolCall,
    emitter: &mut impl EventPublisher,
) -> ToolPreparation {
    let mut context = base_context.clone();
    if let Some(result) = permission_result_for_decision(
        config.tool_permission_policy.tool,
        config,
        turn,
        tool_call,
        PermissionOperation::Tool,
        tool_call.name.clone(),
        emitter,
    )
    .await
    {
        return ToolPreparation::Skip(result);
    }
    context.permissions.tool = PermissionDecision::Allow;

    let Some((operation, subject)) = permission_operation_for_tool(tool_call) else {
        return ToolPreparation::Run(context);
    };
    let decision = match operation {
        PermissionOperation::FileRead => config.tool_permission_policy.file_read,
        PermissionOperation::FileWrite => config.tool_permission_policy.file_write,
        PermissionOperation::Shell => config.tool_permission_policy.shell,
        PermissionOperation::Tool => config.tool_permission_policy.tool,
    };
    if let Some(result) = permission_result_for_decision(
        decision, config, turn, tool_call, operation, subject, emitter,
    )
    .await
    {
        return ToolPreparation::Skip(result);
    }
    match operation {
        PermissionOperation::FileRead => context.permissions.file_read = PermissionDecision::Allow,
        PermissionOperation::FileWrite => {
            context.permissions.file_write = PermissionDecision::Allow;
        }
        PermissionOperation::Shell => context.permissions.shell = PermissionDecision::Allow,
        PermissionOperation::Tool => context.permissions.tool = PermissionDecision::Allow,
    }
    if tool_call.name == "bash" {
        emit_shell_started(turn, tool_call, &context, emitter);
    }
    ToolPreparation::Run(context)
}

async fn permission_result_for_decision(
    decision: PermissionDecision,
    config: &AgentConfig,
    turn: u32,
    tool_call: &AgentToolCall,
    operation: PermissionOperation,
    subject: String,
    emitter: &mut impl EventPublisher,
) -> Option<ToolResult> {
    match decision {
        PermissionDecision::Allow => None,
        PermissionDecision::Deny => Some(permission_error(operation, &subject, "denied")),
        PermissionDecision::Ask => {
            approval_decision(config, turn, tool_call, operation, subject, emitter).await
        }
    }
}

async fn approval_decision(
    config: &AgentConfig,
    turn: u32,
    tool_call: &AgentToolCall,
    operation: PermissionOperation,
    subject: String,
    emitter: &mut impl EventPublisher,
) -> Option<ToolResult> {
    let request = ApprovalRequest {
        turn,
        id: tool_call.id.clone(),
        operation,
        subject,
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
        handler(request.clone()).await
    } else {
        PermissionDecision::Ask
    };
    match decision {
        PermissionDecision::Allow => None,
        PermissionDecision::Deny => Some(permission_error(
            request.operation,
            &request.subject,
            "approval denied",
        )),
        PermissionDecision::Ask => Some(permission_error(
            request.operation,
            &request.subject,
            "approval required",
        )),
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
    };
    ToolResult::error(format!("{prefix} for {noun}: {subject}"))
}

fn permission_operation_for_tool(
    tool_call: &AgentToolCall,
) -> Option<(PermissionOperation, String)> {
    match tool_call.name.as_str() {
        "read" | "list" | "grep" | "find" => Some((
            PermissionOperation::FileRead,
            path_subject(&tool_call.arguments).unwrap_or_else(|| tool_call.name.clone()),
        )),
        "write" | "edit" => Some((
            PermissionOperation::FileWrite,
            path_subject(&tool_call.arguments).unwrap_or_else(|| tool_call.name.clone()),
        )),
        "bash" => Some((
            PermissionOperation::Shell,
            tool_call
                .arguments
                .get("command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("bash")
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
    if tool_call.name != "bash" {
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
    if tool_call.name != "bash" {
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

fn default_tool_context(
    config: &AgentConfig,
    cancel_token: &CancellationToken,
) -> Result<ToolContext, AgentRuntimeError> {
    let workspace_root = if let Some(workspace_root) = &config.workspace_root {
        workspace_root.clone()
    } else {
        std::env::current_dir()?
    };
    ToolContext::new(workspace_root)
        .map(|context| {
            context
                .with_permission_policy(config.tool_permission_policy.clone())
                .with_cancel_token(cancel_token.clone())
        })
        .map_err(AgentRuntimeError::Tool)
}
