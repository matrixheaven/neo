//! Micro compaction — lightweight truncation of old, large tool results.
//!
//! Unlike full compaction (which calls the LLM to generate a structured
//! summary), micro compaction is a pure data transformation: it replaces
//! the *content* of old tool result messages with a placeholder marker,
//! preserving message structure and tool-call pairing while reclaiming
//! context tokens.
//!
//! This is opt-in (experimental) and runs transparently inside
//! [`chat_request`](crate::runtime) construction — it does not modify the
//! durable `AgentContext.messages`, only the ephemeral `ChatRequest`.

use crate::{AgentMessage, Content};

/// Configuration for micro compaction.
#[derive(Debug, Clone)]
pub struct MicroCompactionConfig {
    /// Number of recent messages exempt from truncation.
    pub keep_recent_messages: usize,
    /// Minimum content token estimate before a tool result is eligible for
    /// truncation.
    pub min_content_tokens: usize,
    /// Placeholder text replacing truncated tool result content.
    pub truncated_marker: &'static str,
}

impl Default for MicroCompactionConfig {
    fn default() -> Self {
        Self {
            keep_recent_messages: 20,
            min_content_tokens: 100,
            truncated_marker: "[Old tool result content cleared]",
        }
    }
}

/// Apply micro compaction to a message slice.
///
/// Returns a new vector where tool results at indices below
/// `config.keep_recent_messages` and with content larger than
/// `config.min_content_tokens` have their content replaced by the
/// truncation marker.
///
/// Messages above the cutoff or below the size threshold are preserved
/// verbatim.
#[must_use]
pub fn apply_micro_compaction(
    messages: &[AgentMessage],
    config: &MicroCompactionConfig,
) -> Vec<AgentMessage> {
    if messages.len() <= config.keep_recent_messages {
        return messages.to_vec();
    }
    let cutoff = messages.len() - config.keep_recent_messages;
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            if index < cutoff && is_large_tool_result(message, config.min_content_tokens) {
                truncate_tool_result(message, config.truncated_marker)
            } else {
                message.clone()
            }
        })
        .collect()
}

fn is_large_tool_result(message: &AgentMessage, min_tokens: usize) -> bool {
    let AgentMessage::ToolResult { content, .. } = message else {
        return false;
    };
    let chars: usize = content
        .iter()
        .map(|part| match part {
            Content::Text { text } => text.len(),
            Content::Thinking { .. } => 0,
            Content::Image { .. } => 4800,
        })
        .sum();
    chars.div_ceil(4) >= min_tokens
}

fn truncate_tool_result(message: &AgentMessage, marker: &str) -> AgentMessage {
    let AgentMessage::ToolResult {
        tool_call_id,
        tool_name,
        is_error,
        ..
    } = message
    else {
        return message.clone();
    };
    AgentMessage::tool_result(
        tool_call_id.clone(),
        tool_name.clone(),
        vec![Content::text(marker)],
        *is_error,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn large_tool_result(id: &str, size: usize) -> AgentMessage {
        AgentMessage::tool_result(
            id,
            "bash",
            vec![Content::text(&"x".repeat(size * 4))],
            false,
        )
    }

    fn small_tool_result(id: &str) -> AgentMessage {
        AgentMessage::tool_result(id, "bash", vec![Content::text("ok")], false)
    }

    #[test]
    fn does_not_truncate_below_keep_recent() {
        let config = MicroCompactionConfig::default();
        let messages = vec![
            large_tool_result("t1", 200),
            AgentMessage::user_text("hi"),
        ];
        let result = apply_micro_compaction(&messages, &config);
        assert_eq!(result, messages);
    }

    #[test]
    fn truncates_old_large_tool_results() {
        let config = MicroCompactionConfig {
            keep_recent_messages: 1,
            ..MicroCompactionConfig::default()
        };
        let messages = vec![
            large_tool_result("t1", 200), // 200 tokens, old
            AgentMessage::user_text("recent"),
        ];
        let result = apply_micro_compaction(&messages, &config);
        // First message (index 0 < cutoff=1) should be truncated
        if let AgentMessage::ToolResult { content, .. } = &result[0] {
            assert_eq!(content.len(), 1);
            assert_eq!(content[0].as_text(), Some("[Old tool result content cleared]"));
        } else {
            panic!("expected tool result");
        }
    }

    #[test]
    fn preserves_small_tool_results() {
        let config = MicroCompactionConfig {
            keep_recent_messages: 1,
            min_content_tokens: 100,
            ..MicroCompactionConfig::default()
        };
        let messages = vec![
            small_tool_result("t1"), // small, old
            AgentMessage::user_text("recent"),
        ];
        let result = apply_micro_compaction(&messages, &config);
        assert_eq!(result, messages);
    }

    #[test]
    fn preserves_non_tool_messages() {
        let config = MicroCompactionConfig {
            keep_recent_messages: 1,
            ..MicroCompactionConfig::default()
        };
        let messages = vec![
            AgentMessage::user_text(&"x".repeat(1000)),
            AgentMessage::user_text("recent"),
        ];
        let result = apply_micro_compaction(&messages, &config);
        assert_eq!(result, messages);
    }
}
