use std::sync::Arc;

use futures::{StreamExt, stream, stream::FuturesUnordered};
use neo_ai::{AiStreamEvent, ChatRequest, ModelClient, ModelSpec, RequestOptions, ToolSpec};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    AgentEvent, AgentMessage, AgentToolCall, Content, StopReason, ToolContext, ToolError,
    ToolRegistry, ToolResult,
};

pub type ContextTransform = Arc<dyn Fn(&[AgentMessage]) -> Vec<AgentMessage> + Send + Sync>;
pub type BeforeToolCallHook = Arc<dyn Fn(&AgentToolCall) -> Option<ToolResult> + Send + Sync>;
pub type AfterToolCallHook = Arc<dyn Fn(&AgentToolCall, ToolResult) -> ToolResult + Send + Sync>;

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
    pub system_prompt: Option<String>,
    pub max_turns: u32,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub tools: Vec<ToolSpec>,
    pub steering_queue_mode: QueueMode,
    pub follow_up_queue_mode: QueueMode,
    pub tool_execution_mode: ToolExecutionMode,
    #[serde(skip)]
    #[schemars(skip)]
    pub context_transform: Option<ContextTransform>,
    #[serde(skip)]
    #[schemars(skip)]
    pub before_tool_call: Option<BeforeToolCallHook>,
    #[serde(skip)]
    #[schemars(skip)]
    pub after_tool_call: Option<AfterToolCallHook>,
}

impl AgentConfig {
    #[must_use]
    pub fn for_model(model: ModelSpec) -> Self {
        Self {
            model,
            system_prompt: None,
            max_turns: 8,
            temperature: None,
            max_tokens: None,
            tools: Vec::new(),
            steering_queue_mode: QueueMode::All,
            follow_up_queue_mode: QueueMode::All,
            tool_execution_mode: ToolExecutionMode::Parallel,
            context_transform: None,
            before_tool_call: None,
            after_tool_call: None,
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
    pub fn with_after_tool_call(
        mut self,
        hook: impl Fn(&AgentToolCall, ToolResult) -> ToolResult + Send + Sync + 'static,
    ) -> Self {
        self.after_tool_call = Some(Arc::new(hook));
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentContext {
    messages: Vec<AgentMessage>,
    turns: u32,
    cancelled: bool,
    steering_queue: Vec<AgentMessage>,
    follow_up_queue: Vec<AgentMessage>,
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

pub type AgentEventStream = stream::BoxStream<'static, Result<AgentEvent, AgentRuntimeError>>;

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

    pub fn run_turn(&self, context: &mut AgentContext, message: AgentMessage) -> AgentEventStream {
        if context.is_cancelled() {
            return stream::iter([Ok(AgentEvent::TurnFinished {
                turn: context.turns.saturating_add(1),
                stop_reason: StopReason::Cancelled,
            })])
            .boxed();
        }

        if context.turns >= self.config.max_turns {
            return stream::iter([Ok(AgentEvent::TurnFinished {
                turn: context.turns.saturating_add(1),
                stop_reason: StopReason::MaxTurns,
            })])
            .boxed();
        }

        context.append_message(message);
        let model = Arc::clone(&self.model);
        let tools = self.tools.clone();
        let config = self.config.clone();
        let messages = run_agent_turn(model, config, tools, context.clone());
        let events = futures::executor::block_on(async {
            let outcome = messages.await?;
            Ok::<_, AgentRuntimeError>(outcome)
        });

        match events {
            Ok(outcome) => {
                *context = outcome.context;
                let events = outcome.events;
                stream::iter(events.into_iter().map(Ok)).boxed()
            }
            Err(err) => stream::iter([Err(err)]).boxed(),
        }
    }
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
    messages.extend(context_messages.iter().map(AgentMessage::to_chat_message));
    ChatRequest {
        model: config.model.clone(),
        messages,
        tools: config.tools.clone(),
        options: RequestOptions {
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            ..RequestOptions::default()
        },
    }
}

struct RunOutcome {
    events: Vec<AgentEvent>,
    context: AgentContext,
}

async fn run_agent_turn(
    model: Arc<dyn ModelClient>,
    config: AgentConfig,
    tools: Option<Arc<ToolRegistry>>,
    mut context: AgentContext,
) -> Result<RunOutcome, AgentRuntimeError> {
    let mut all_events = Vec::new();
    let mut pending_messages =
        drain_messages(&mut context.steering_queue, config.steering_queue_mode);

    loop {
        if !pending_messages.is_empty() {
            append_queued_messages(&mut context, pending_messages, &mut all_events);
        }

        if context.is_cancelled() {
            break;
        }

        if context.turns >= config.max_turns {
            all_events.push(AgentEvent::TurnFinished {
                turn: context.turns.saturating_add(1),
                stop_reason: StopReason::MaxTurns,
            });
            break;
        }

        context.turns = context.turns.saturating_add(1);
        let turn = context.turns;
        let request = chat_request(&config, &context);
        let events = run_model_turn(Arc::clone(&model), request, turn).await?;
        let assistant = events.iter().find_map(|event| {
            if let AgentEvent::MessageAppended { message } = event {
                Some(message.clone())
            } else {
                None
            }
        });
        all_events.extend(events);

        let Some(AgentMessage::Assistant {
            tool_calls: model_tool_calls,
            stop_reason: StopReason::ToolUse,
            ..
        }) = assistant.clone()
        else {
            if let Some(assistant) = assistant {
                context.append_message(assistant);
            }
            pending_messages =
                drain_messages(&mut context.steering_queue, config.steering_queue_mode);
            if pending_messages.is_empty() {
                pending_messages =
                    drain_messages(&mut context.follow_up_queue, config.follow_up_queue_mode);
            }
            if pending_messages.is_empty() {
                break;
            }
            continue;
        };
        let tool_calls = model_tool_calls.clone();

        if let Some(assistant) = assistant {
            context.append_message(assistant);
        }

        let Some(registry) = &tools else {
            break;
        };
        let tool_results =
            execute_tool_calls(&config, registry, turn, &tool_calls, &mut all_events).await?;
        for (tool_call, result) in &tool_results {
            let message = AgentMessage::tool_result(
                tool_call.id.clone(),
                tool_call.name.clone(),
                vec![Content::text(result.content.clone())],
                result.is_error,
            );
            context.append_message(message.clone());
            all_events.push(AgentEvent::MessageAppended { message });
        }
        if terminates_tool_batch(&tool_results) {
            break;
        }
        pending_messages = drain_messages(&mut context.steering_queue, config.steering_queue_mode);
    }

    Ok(RunOutcome {
        events: all_events,
        context,
    })
}

fn append_queued_messages(
    context: &mut AgentContext,
    messages: Vec<AgentMessage>,
    events: &mut Vec<AgentEvent>,
) {
    for message in messages {
        context.append_message(message.clone());
        events.push(AgentEvent::MessageAppended { message });
    }
}

fn drain_messages(queue: &mut Vec<AgentMessage>, mode: QueueMode) -> Vec<AgentMessage> {
    let count = match mode {
        QueueMode::All => queue.len(),
        QueueMode::OneAtATime => usize::from(!queue.is_empty()),
    };
    queue.drain(0..count).collect()
}

fn terminates_tool_batch(tool_results: &[(AgentToolCall, ToolResult)]) -> bool {
    !tool_results.is_empty() && tool_results.iter().all(|(_, result)| result.terminate)
}

async fn execute_tool_calls(
    config: &AgentConfig,
    registry: &ToolRegistry,
    turn: u32,
    tool_calls: &[AgentToolCall],
    events: &mut Vec<AgentEvent>,
) -> Result<Vec<(AgentToolCall, ToolResult)>, AgentRuntimeError> {
    match config.tool_execution_mode {
        ToolExecutionMode::Sequential => {
            execute_tool_calls_sequential(config, registry, turn, tool_calls, events).await
        }
        ToolExecutionMode::Parallel => {
            execute_tool_calls_parallel(config, registry, turn, tool_calls, events).await
        }
    }
}

async fn execute_tool_calls_sequential(
    config: &AgentConfig,
    registry: &ToolRegistry,
    turn: u32,
    tool_calls: &[AgentToolCall],
    events: &mut Vec<AgentEvent>,
) -> Result<Vec<(AgentToolCall, ToolResult)>, AgentRuntimeError> {
    let tool_context = default_tool_context()?;
    let mut results = Vec::new();
    for tool_call in tool_calls {
        events.push(AgentEvent::ToolExecutionStarted {
            turn,
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            arguments: tool_call.arguments.clone(),
        });
        let mut result = if let Some(before_tool_call) = &config.before_tool_call {
            if let Some(blocked) = before_tool_call(tool_call) {
                blocked
            } else {
                registry
                    .run(&tool_call.name, &tool_context, tool_call.arguments.clone())
                    .await?
            }
        } else {
            registry
                .run(&tool_call.name, &tool_context, tool_call.arguments.clone())
                .await?
        };
        if let Some(after_tool_call) = &config.after_tool_call {
            result = after_tool_call(tool_call, result);
        }
        events.push(AgentEvent::ToolExecutionFinished {
            turn,
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            result: result.clone(),
        });
        results.push((tool_call.clone(), result));
    }
    Ok(results)
}

async fn execute_tool_calls_parallel(
    config: &AgentConfig,
    registry: &ToolRegistry,
    turn: u32,
    tool_calls: &[AgentToolCall],
    events: &mut Vec<AgentEvent>,
) -> Result<Vec<(AgentToolCall, ToolResult)>, AgentRuntimeError> {
    let tool_context = default_tool_context()?;
    let mut completed = Vec::with_capacity(tool_calls.len());
    let mut running = FuturesUnordered::new();

    for (index, tool_call) in tool_calls.iter().cloned().enumerate() {
        events.push(AgentEvent::ToolExecutionStarted {
            turn,
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            arguments: tool_call.arguments.clone(),
        });

        if let Some(before_tool_call) = &config.before_tool_call
            && let Some(mut result) = before_tool_call(&tool_call)
        {
            if let Some(after_tool_call) = &config.after_tool_call {
                result = after_tool_call(&tool_call, result);
            }
            events.push(AgentEvent::ToolExecutionFinished {
                turn,
                id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                result: result.clone(),
            });
            completed.push((index, tool_call, result));
            continue;
        }

        let after_tool_call = config.after_tool_call.clone();
        let tool_context = tool_context.clone();
        running.push(async move {
            let mut result = registry
                .run(&tool_call.name, &tool_context, tool_call.arguments.clone())
                .await?;
            if let Some(after_tool_call) = &after_tool_call {
                result = after_tool_call(&tool_call, result);
            }
            Ok::<_, AgentRuntimeError>((index, tool_call, result))
        });
    }

    while let Some(outcome) = running.next().await {
        let (index, tool_call, result) = outcome?;
        events.push(AgentEvent::ToolExecutionFinished {
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

async fn run_model_turn(
    model: Arc<dyn ModelClient>,
    request: ChatRequest,
    turn: u32,
) -> Result<Vec<AgentEvent>, AgentRuntimeError> {
    let mut events = vec![AgentEvent::TurnStarted { turn }];
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut stop_reason = StopReason::EndTurn;
    let mut stream = model.stream_chat(request);
    let mut tool_names = std::collections::HashMap::new();

    while let Some(event) = stream.next().await {
        match event? {
            AiStreamEvent::MessageStart { id } => {
                events.push(AgentEvent::MessageStarted { turn, id });
            }
            AiStreamEvent::TextDelta { text: delta } => {
                text.push_str(&delta);
                events.push(AgentEvent::TextDelta { turn, text: delta });
            }
            AiStreamEvent::ToolCallStart { id, name } => {
                tool_names.insert(id.clone(), name.clone());
                events.push(AgentEvent::ToolCallStarted { turn, id, name });
            }
            AiStreamEvent::ToolCallArgsDelta { id, json_fragment } => {
                events.push(AgentEvent::ToolCallArgumentsDelta {
                    turn,
                    id,
                    json_fragment,
                });
            }
            AiStreamEvent::ToolCallEnd { id, arguments } => {
                let tool_call = AgentToolCall {
                    name: tool_names.remove(&id).unwrap_or_default(),
                    id,
                    arguments,
                };
                events.push(AgentEvent::ToolCallFinished {
                    turn,
                    tool_call: tool_call.clone(),
                });
                tool_calls.push(tool_call);
            }
            AiStreamEvent::MessageEnd {
                stop_reason: reason,
                usage: _,
            } => {
                stop_reason = reason.into();
            }
            AiStreamEvent::Error { message } => {
                events.push(AgentEvent::Error {
                    turn,
                    message: message.clone(),
                });
                stop_reason = StopReason::Error;
            }
        }
    }

    let content = if text.is_empty() {
        Vec::new()
    } else {
        vec![Content::text(text)]
    };
    let message = AgentMessage::assistant(content, tool_calls, stop_reason);
    events.push(AgentEvent::MessageAppended { message });
    events.push(AgentEvent::TurnFinished { turn, stop_reason });
    Ok(events)
}

fn default_tool_context() -> Result<ToolContext, AgentRuntimeError> {
    ToolContext::new(std::env::current_dir()?).map_err(AgentRuntimeError::Tool)
}
