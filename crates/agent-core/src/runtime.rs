use std::sync::Arc;

use futures::{StreamExt, stream};
use neo_ai::{AiStreamEvent, ChatRequest, ModelClient, ModelSpec, ToolSpec};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    AgentEvent, AgentMessage, AgentToolCall, Content, StopReason, ToolContext, ToolError,
    ToolRegistry,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentConfig {
    pub model: ModelSpec,
    pub system_prompt: Option<String>,
    pub max_turns: u32,
    pub temperature: Option<u32>,
    pub max_tokens: Option<u32>,
    pub tools: Vec<ToolSpec>,
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
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentContext {
    messages: Vec<AgentMessage>,
    turns: u32,
    cancelled: bool,
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

    pub fn cancel(&mut self) {
        self.cancelled = true;
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled
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

        context.turns = context.turns.saturating_add(1);
        let turn = context.turns;
        context.append_message(message);
        let model = Arc::clone(&self.model);
        let tools = self.tools.clone();
        let config = self.config.clone();
        let messages = run_agent_turn(model, config, tools, context.clone(), turn);
        let context_messages = &mut context.messages;
        let mut generated = Vec::new();
        let events = futures::executor::block_on(async {
            let events = messages.await?;
            for event in &events {
                if let AgentEvent::MessageAppended { message } = event {
                    generated.push(message.clone());
                }
            }
            Ok::<_, AgentRuntimeError>(events)
        });

        match events {
            Ok(events) => {
                context_messages.extend(generated);
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
    messages.extend(context.messages.iter().map(AgentMessage::to_chat_message));
    ChatRequest {
        model: config.model.clone(),
        messages,
        tools: config.tools.clone(),
        temperature: config.temperature,
        max_tokens: config.max_tokens,
    }
}

async fn run_agent_turn(
    model: Arc<dyn ModelClient>,
    config: AgentConfig,
    tools: Option<Arc<ToolRegistry>>,
    mut context: AgentContext,
    turn: u32,
) -> Result<Vec<AgentEvent>, AgentRuntimeError> {
    let mut all_events = Vec::new();

    loop {
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
            break;
        };
        let tool_calls = model_tool_calls.clone();

        if let Some(assistant) = assistant {
            context.append_message(assistant);
        }

        let Some(registry) = &tools else {
            break;
        };
        let tool_context = default_tool_context()?;
        for tool_call in tool_calls {
            let result = registry
                .run(&tool_call.name, &tool_context, tool_call.arguments.clone())
                .await?;
            let message = AgentMessage::tool_result(
                tool_call.id.clone(),
                tool_call.name.clone(),
                vec![Content::text(result.content)],
                result.is_error,
            );
            context.append_message(message.clone());
            all_events.push(AgentEvent::MessageAppended { message });
        }
    }

    Ok(all_events)
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
