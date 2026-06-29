use std::sync::{Arc, RwLock};

use neo_ai::{AiError, ChatMessage, ChatRequest, ContentPart, RequestOptions};

use super::config::AgentConfig;
use super::context::AgentContext;
use super::image_blobs::resolve_image_blobs;
use crate::{AgentMessage, PlanModeInjector, sanitize_tool_exchange_messages};

pub(super) async fn chat_request(config: &AgentConfig, context: &AgentContext) -> ChatRequest {
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

pub(super) fn validate_model_capabilities(request: &ChatRequest) -> Result<(), AiError> {
    let capabilities = &request.model.capabilities;
    if !request.tools.is_empty() && !capabilities.tools {
        return Err(AiError::Configuration { message: format!(
            "model {}/{} does not support tools",
            request.model.provider.0, request.model.model
        ) });
    }
    if request.options.reasoning_effort.is_some() && !capabilities.reasoning {
        return Err(AiError::Configuration { message: format!(
            "model {}/{} does not support reasoning",
            request.model.provider.0, request.model.model
        ) });
    }
    if request_messages_contain_image(&request.messages) && !capabilities.images {
        return Err(AiError::Configuration { message: format!(
            "model {}/{} does not support image input",
            request.model.provider.0, request.model.model
        ) });
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

pub(super) async fn chat_request_for_context_estimate(
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
