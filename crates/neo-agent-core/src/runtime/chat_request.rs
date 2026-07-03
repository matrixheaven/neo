use std::sync::Arc;

use neo_ai::{AiError, ChatMessage, ChatRequest, ContentPart, RequestOptions};

use super::config::AgentConfig;
use super::context::AgentContext;
use super::image_blobs::resolve_image_blobs;
use crate::{AgentMessage, PlanModeInjector, sanitize_tool_exchange_messages};

pub(super) async fn chat_request(config: &AgentConfig, context: &AgentContext) -> ChatRequest {
    let mut messages = Vec::new();
    if let Some(system_prompt) = &config.system_prompt {
        messages.push(AgentMessage::system_text(system_prompt.as_str()).to_chat_message());
    }
    if let Some(workspace_context) = workspace_context_message(config) {
        messages.push(workspace_context.to_chat_message());
    }
    if let Some(review_context) = review_mode_message(context) {
        messages.push(review_context.to_chat_message());
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
    let context_messages = sanitize_tool_exchange_messages(&context_messages);
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
            session_id: prompt_cache_key(config),
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
    let permission_mode = config
        .live_permission_mode
        .read()
        .map_or(config.permission_mode, |guard| *guard);
    Some(AgentMessage::system_text(format!(
        "Runtime Context\n\
         - cwd: {}\n\
         - permission mode: {}\n\
         - tool execution mode: {}\n\
         - Read may accept absolute paths when the user asks for them or the task requires them.\n\
         - Write, Edit, Bash, and Terminal are governed by Neo's permission layer; write and shell tools are constrained by workspace permissions.\n\
         - Shell tools already run in this workspace. Do not prefix shell commands with `cd <cwd> &&`; use the bash `cwd` field for a workspace subdirectory.\n\
         - Network access is not a separate Neo prompt guarantee; it depends on the available tools, host environment, and permission decisions.\n\
         - If an approval is denied, treat it as the user's decision and choose a different safe path instead of retrying the same request.",
        workspace_root.display(),
        permission_mode.label(),
        tool_execution_mode_label(config.tool_execution_mode)
    )))
}

fn tool_execution_mode_label(mode: crate::ToolExecutionMode) -> &'static str {
    match mode {
        crate::ToolExecutionMode::Sequential => "sequential",
        crate::ToolExecutionMode::Parallel => "parallel",
    }
}

fn review_mode_message(context: &AgentContext) -> Option<AgentMessage> {
    let latest_user_text = context
        .messages()
        .iter()
        .rev()
        .find_map(|message| match message {
            AgentMessage::User { .. } => Some(message.text()),
            _ => None,
        })?;
    if !looks_like_review_request(&latest_user_text) {
        return None;
    }
    Some(AgentMessage::system_text(
        "Review Mode\n\
         The latest user request asks for a review. Findings come first, ordered by severity. \
         Focus on bugs, behavioral regressions, security risks, missing tests, and concrete maintainability issues. \
         Ground findings in file and line references when possible. Keep summaries and change descriptions secondary."
            .to_owned(),
    ))
}

fn looks_like_review_request(text: &str) -> bool {
    let normalized = text.to_lowercase();
    normalized.contains("review")
        || normalized.contains("code review")
        || normalized.contains("审查")
        || normalized.contains("评审")
        || normalized.contains("代码审查")
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
    if request.options.reasoning_effort.is_some() && !capabilities.reasoning {
        return Err(AiError::Configuration {
            message: format!(
                "model {}/{} does not support reasoning",
                request.model.provider.0, request.model.model
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
mod tests {
    use neo_ai::{ApiKind, ChatMessage, ContentPart, ModelCapabilities, ModelSpec, ProviderId};

    use super::*;
    use crate::tools::ToolRegistry;

    fn tool_model() -> ModelSpec {
        ModelSpec {
            provider: ProviderId("test".to_owned()),
            model: "tool-model".to_owned(),
            api: ApiKind::Local,
            capabilities: ModelCapabilities::tool_chat(),
        }
    }

    fn system_texts(request: &ChatRequest) -> String {
        request
            .messages
            .iter()
            .filter_map(|message| match message {
                ChatMessage::System { content } => Some(content),
                _ => None,
            })
            .flat_map(|content| content.iter())
            .filter_map(|part| match part {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn chat_request_sends_tools_without_duplicate_system_schema_catalog() {
        let tools = ToolRegistry::with_builtin_tools().specs();
        let config = AgentConfig::for_model(tool_model())
            .with_system_prompt("Base system")
            .with_tools(tools.clone());
        let context = AgentContext::new();

        let request = chat_request(&config, &context).await;
        let system_text = system_texts(&request);

        assert!(
            !system_text.contains("<available_tools_schema>"),
            "{system_text}"
        );
        assert_eq!(request.tools, tools);
    }

    #[tokio::test]
    async fn chat_request_omits_tool_schema_catalog_when_no_tools_are_available() {
        let config = AgentConfig::for_model(tool_model()).with_system_prompt("Base system");
        let context = AgentContext::new();

        let request = chat_request(&config, &context).await;
        let system_text = system_texts(&request);

        assert!(
            !system_text.contains("<available_tools_schema>"),
            "{system_text}"
        );
        assert!(request.tools.is_empty());
    }

    #[tokio::test]
    async fn chat_request_uses_session_directory_name_as_prompt_cache_key() {
        let config = AgentConfig::for_model(tool_model())
            .with_session_directory("/tmp/neo/session_00000000-0000-4000-8000-000000000123");
        let context = AgentContext::new();

        let request = chat_request(&config, &context).await;

        assert_eq!(
            request.options.session_id.as_deref(),
            Some("session_00000000-0000-4000-8000-000000000123")
        );
    }

    #[tokio::test]
    async fn chat_request_injects_runtime_context_with_permission_and_boundaries() {
        let temp = tempfile::tempdir().expect("temp workspace");
        let config = AgentConfig::for_model(tool_model())
            .with_system_prompt("Base system")
            .with_workspace_root(temp.path())
            .expect("workspace root")
            .with_permission_mode(crate::PermissionMode::Yolo);
        let context = AgentContext::new();

        let request = chat_request(&config, &context).await;
        let system_text = system_texts(&request);

        assert!(system_text.contains("Runtime Context"), "{system_text}");
        assert!(
            system_text.contains("permission mode: yolo"),
            "{system_text}"
        );
        assert!(
            system_text.contains("tool execution mode: parallel"),
            "{system_text}"
        );
        assert!(
            system_text.contains("write and shell tools are constrained by workspace permissions"),
            "{system_text}"
        );
        assert!(
            system_text.contains("Read may accept absolute paths"),
            "{system_text}"
        );
    }

    #[tokio::test]
    async fn chat_request_adds_review_mode_reminder_for_review_requests() {
        let config = AgentConfig::for_model(tool_model()).with_system_prompt("Base system");
        let mut context = AgentContext::new();
        context.append_message(AgentMessage::user_text("Please review this change"));

        let request = chat_request(&config, &context).await;
        let system_text = system_texts(&request);

        assert!(system_text.contains("Review Mode"), "{system_text}");
        assert!(
            system_text.contains("Findings come first, ordered by severity"),
            "{system_text}"
        );
    }
}
