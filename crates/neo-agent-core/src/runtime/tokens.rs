//! Token estimation helpers — pure, stateless approximations based on
//! character counts divided by 4.

use neo_ai::{ChatMessage, ContentPart};

use crate::{AgentMessage, Content};

pub(super) fn estimate_messages_tokens(messages: &[AgentMessage]) -> usize {
    messages.iter().map(estimate_message_tokens).sum()
}

pub(super) fn estimate_chat_messages_tokens(messages: &[ChatMessage]) -> usize {
    messages.iter().map(estimate_chat_message_tokens).sum()
}

pub(super) fn estimate_chat_message_tokens(message: &ChatMessage) -> usize {
    let chars = match message {
        ChatMessage::System { content }
        | ChatMessage::User { content }
        | ChatMessage::ToolResult { content, .. } => estimate_chat_content_chars(content),
        ChatMessage::Assistant {
            content,
            tool_calls,
        } => {
            let content_chars = estimate_chat_content_chars(content);
            let tool_chars = tool_calls
                .iter()
                .map(|call| call.name.len() + call.arguments.to_string().len())
                .sum::<usize>();
            content_chars + tool_chars
        }
    };
    chars.div_ceil(4)
}

pub(super) fn estimate_message_tokens(message: &AgentMessage) -> usize {
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
        AgentMessage::ShellCommand {
            command,
            stdout,
            stderr,
            ..
        } => command.len() + stdout.len() + stderr.len(),
    };
    chars.div_ceil(4)
}

pub(super) fn estimate_chat_content_chars(content: &[ContentPart]) -> usize {
    content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => text.len(),
            ContentPart::Thinking { .. } => 0,
            ContentPart::Image { .. } => 4800,
        })
        .sum()
}

pub(super) fn estimate_content_chars(content: &[Content]) -> usize {
    content
        .iter()
        .map(|part| match part {
            Content::Text { text } => text.len(),
            Content::Thinking { .. } => 0,
            Content::Image { .. } => 4800,
        })
        .sum()
}
