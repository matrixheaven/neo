//! Token estimation helpers — pure, stateless approximations.

use neo_ai::{ChatMessage, ChatRequest, ContentPart, ToolSpec};

use crate::{AgentMessage, Content};

pub(crate) fn estimate_messages_tokens(messages: &[AgentMessage]) -> usize {
    messages.iter().map(estimate_message_tokens).sum()
}

pub(crate) fn estimate_chat_request_tokens(request: &ChatRequest) -> usize {
    estimate_chat_messages_tokens(&request.messages) + estimate_tool_specs_tokens(&request.tools)
}

pub(crate) fn estimate_chat_messages_tokens(messages: &[ChatMessage]) -> usize {
    messages.iter().map(estimate_chat_message_tokens).sum()
}

pub(crate) fn estimate_chat_message_tokens(message: &ChatMessage) -> usize {
    let role_tokens = estimate_text_tokens(chat_message_role(message));
    let payload_tokens = match message {
        ChatMessage::System { content } | ChatMessage::User { content } => {
            estimate_chat_content_tokens(content)
        }
        ChatMessage::ToolResult {
            tool_call_id,
            content,
            is_error,
        } => {
            estimate_text_tokens(tool_call_id)
                + estimate_chat_content_tokens(content)
                + usize::from(*is_error)
        }
        ChatMessage::Assistant {
            content,
            tool_calls,
        } => {
            let content_tokens = estimate_chat_content_tokens(content);
            let tool_tokens = tool_calls
                .iter()
                .map(|call| {
                    estimate_text_tokens(&call.id)
                        + estimate_text_tokens(&call.name)
                        + estimate_text_tokens(&call.raw_arguments)
                })
                .sum::<usize>();
            content_tokens + tool_tokens
        }
    };
    role_tokens + payload_tokens
}

pub(crate) fn estimate_message_tokens(message: &AgentMessage) -> usize {
    let role_tokens = estimate_text_tokens(agent_message_role(message));
    let payload_tokens = match message {
        AgentMessage::System { content } | AgentMessage::User { content } => {
            estimate_content_tokens(content)
        }
        AgentMessage::ToolResult {
            tool_call_id,
            tool_name,
            content,
            is_error,
        } => {
            estimate_text_tokens(tool_call_id)
                + estimate_text_tokens(tool_name)
                + estimate_content_tokens(content)
                + usize::from(*is_error)
        }
        AgentMessage::Assistant {
            content,
            tool_calls,
            ..
        } => {
            let content_tokens = estimate_content_tokens(content);
            let tool_tokens = tool_calls
                .iter()
                .map(|call| {
                    estimate_text_tokens(&call.id)
                        + estimate_text_tokens(&call.name)
                        + estimate_text_tokens(&call.raw_arguments)
                })
                .sum::<usize>();
            content_tokens + tool_tokens
        }
        AgentMessage::ShellCommand {
            command,
            stdout,
            stderr,
            ..
        } => {
            estimate_text_tokens(command)
                + estimate_text_tokens(stdout)
                + estimate_text_tokens(stderr)
        }
    };
    role_tokens + payload_tokens
}

pub(crate) fn estimate_chat_content_tokens(content: &[ContentPart]) -> usize {
    content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } | ContentPart::Thinking { text, .. } => {
                estimate_text_tokens(text)
            }
            ContentPart::Image { .. } => estimate_image_tokens(),
        })
        .sum()
}

pub(crate) fn estimate_content_tokens(content: &[Content]) -> usize {
    content
        .iter()
        .map(|part| match part {
            Content::Text { text } => estimate_text_tokens(text),
            Content::Thinking { .. } => 0,
            Content::Image { .. } => estimate_image_tokens(),
        })
        .sum()
}

fn estimate_tool_specs_tokens(tools: &[ToolSpec]) -> usize {
    tools
        .iter()
        .map(|tool| {
            estimate_text_tokens(&tool.name)
                + estimate_text_tokens(&tool.description)
                + estimate_text_tokens(&tool.input_schema.to_string())
        })
        .sum()
}

fn estimate_text_tokens(text: &str) -> usize {
    let mut ascii_chars = 0usize;
    let mut non_ascii_chars = 0usize;
    for ch in text.chars() {
        if ch.is_ascii() {
            ascii_chars += 1;
        } else {
            non_ascii_chars += 1;
        }
    }
    ascii_chars.div_ceil(4) + non_ascii_chars
}

const fn estimate_image_tokens() -> usize {
    1_200
}

const fn chat_message_role(message: &ChatMessage) -> &'static str {
    match message {
        ChatMessage::System { .. } => "system",
        ChatMessage::User { .. } => "user",
        ChatMessage::Assistant { .. } => "assistant",
        ChatMessage::ToolResult { .. } => "tool",
    }
}

const fn agent_message_role(message: &AgentMessage) -> &'static str {
    match message {
        AgentMessage::System { .. } => "system",
        AgentMessage::User { .. } | AgentMessage::ShellCommand { .. } => "user",
        AgentMessage::Assistant { .. } => "assistant",
        AgentMessage::ToolResult { .. } => "tool",
    }
}
