//! Token estimation helpers — pure, stateless approximations.

use neo_ai::ToolSpec;

use crate::{AgentMessage, Content};

pub(crate) fn estimate_messages_tokens(messages: &[AgentMessage]) -> usize {
    messages.iter().map(estimate_message_tokens).sum()
}

pub(crate) fn estimate_message_tokens(message: &AgentMessage) -> usize {
    let role_tokens = estimate_text_tokens(agent_message_role(message));
    let payload_tokens = match message {
        AgentMessage::System { content } | AgentMessage::User { content, .. } => {
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

pub(crate) fn estimate_tool_specs_tokens(tools: &[ToolSpec]) -> usize {
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
    // Fast byte-based approximation that avoids per-character iteration.
    //
    // The previous implementation iterated every `char`, counting ASCII vs
    // non-ASCII separately (ASCII / 4, non-ASCII × 1).  On multi-MB context
    // this char-walk dominated CPU.
    //
    // Approximation: count non-ASCII continuation bytes (0x80–0xFF) in a
    // single byte pass.  ASCII bytes ≈ bytes / 4 tokens, non-ASCII bytes
    // ≈ bytes / 3 (covers CJK at ~3 bytes/char, 1 token/char).  This is
    // close to the original weighted result within ±10%.
    let total_bytes = text.len();
    if total_bytes == 0 {
        return 0;
    }
    let non_ascii_bytes = text.bytes().filter(|b| !b.is_ascii()).count();
    let ascii_bytes = total_bytes - non_ascii_bytes;
    ascii_bytes.div_ceil(4) + non_ascii_bytes.div_ceil(3)
}

const fn estimate_image_tokens() -> usize {
    1_200
}

const fn agent_message_role(message: &AgentMessage) -> &'static str {
    match message {
        AgentMessage::System { .. } => "system",
        AgentMessage::User { .. } | AgentMessage::ShellCommand { .. } => "user",
        AgentMessage::Assistant { .. } => "assistant",
        AgentMessage::ToolResult { .. } => "tool",
    }
}
