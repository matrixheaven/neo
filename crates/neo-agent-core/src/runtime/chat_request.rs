use neo_ai::{AiError, ChatMessage, ChatRequest, ContentPart, RequestOptions};

use super::config::AgentConfig;
use super::context::AgentContext;
use super::image_blobs::resolve_image_blobs;
use crate::{AgentMessage, sanitize_tool_exchange_messages};

pub(super) async fn chat_request(config: &AgentConfig, context: &AgentContext) -> ChatRequest {
    let mut messages = Vec::new();
    if let Some(system_prompt) = &config.system_prompt {
        messages.push(AgentMessage::system_text(system_prompt.as_str()).to_chat_message());
    }
    if let Some(workspace_context) = workspace_context_message(config) {
        messages.push(workspace_context.to_chat_message());
    }
    let mut context_messages = context.messages.clone();
    if let Some(transform) = &config.context_append_transform {
        context_messages.extend(transform(context.messages()));
    }
    // Resolve blob references to inline base64 before sending to the provider.
    let context_messages =
        resolve_image_blobs(context_messages, config.session_directory.as_deref()).await;
    // Never send a provider request with an assistant message that has pending
    // tool_calls but no matching tool results.  This guards against incomplete
    // trailing tool turns and against compaction boundaries that accidentally
    // orphan such a message.
    let context_messages = sanitize_tool_exchange_messages(&context_messages);
    for message in context_messages.iter() {
        messages.push(if config.replay_reasoning {
            message.to_chat_message()
        } else {
            without_reasoning_content(message.to_chat_message())
        });
    }
    ChatRequest {
        model: config.model.clone(),
        messages,
        tools: config.tools.clone(),
        options: RequestOptions {
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            reasoning: config.reasoning.clone(),
            replay_reasoning: config.replay_reasoning,
            session_id: prompt_cache_key(config),
            response_format: config.response_format.clone(),
            ..RequestOptions::default()
        },
    }
}

pub(super) fn workspace_context_message(config: &AgentConfig) -> Option<AgentMessage> {
    let workspace_root = config.workspace_root.as_ref()?;
    Some(AgentMessage::system_text(format!(
        "Runtime Context\n\
         - cwd: {}\n\
         - Read may accept absolute paths when the user asks for them or the task requires them.\n\
         - Write, Edit, Bash, and Terminal are governed by Neo's permission layer; write and shell tools are constrained by workspace permissions.\n\
         - Shell tools already run in this workspace. Do not prefix shell commands with `cd <cwd> &&`; use the bash `cwd` field for a workspace subdirectory.\n\
         - Commands that work inside a nested project subtree must set the tool's typed `cwd` field (Bash, Terminal start) to that subtree. Command text is never inspected for paths, so nested AGENTS.md instructions load only from typed `cwd`/path arguments.\n\
         - Network access is not a separate Neo prompt guarantee; it depends on the available tools, host environment, and permission decisions.\n\
         - If an approval is denied, treat it as the user's decision and choose a different safe path instead of retrying the same request.",
        workspace_root.display()
    )))
}

fn prompt_cache_key(config: &AgentConfig) -> Option<String> {
    config
        .session_directory
        .as_ref()?
        .file_name()?
        .to_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
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
        return Err(AiError::Configuration {
            message: format!(
                "model {}/{} does not support tools",
                request.model.provider.0, request.model.model
            ),
        });
    }
    if !capabilities.reasoning.supports(&request.options.reasoning) {
        return Err(AiError::Configuration {
            message: format!(
                "model {}/{} does not support reasoning selection {:?}; capability is {:?}",
                request.model.provider.0,
                request.model.model,
                request.options.reasoning,
                capabilities.reasoning
            ),
        });
    }
    if request_messages_contain_image(&request.messages) && !capabilities.images {
        return Err(AiError::Configuration {
            message: format!(
                "model {}/{} does not support image input",
                request.model.provider.0, request.model.model
            ),
        });
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

#[cfg(test)]
#[path = "test_cases/chat_request.rs"]
mod tests;
