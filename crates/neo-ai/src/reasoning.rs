use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ApiKind, ChatMessage, ContentPart, ModelSpec, ProviderId, ReasoningEffort};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ReasoningPolicy {
    #[serde(rename = "off", alias = "Off")]
    Off,
    #[serde(rename = "auto", alias = "Auto")]
    Auto,
    #[serde(rename = "minimal", alias = "Minimal")]
    Minimal,
    #[serde(rename = "low", alias = "Low")]
    Low,
    #[serde(rename = "medium", alias = "Medium")]
    Medium,
    #[serde(rename = "high", alias = "High")]
    High,
    #[serde(rename = "xhigh", alias = "XHigh")]
    XHigh,
}

impl ReasoningPolicy {
    #[must_use]
    pub const fn resolve_for_model(self, model: &ModelSpec) -> Option<ReasoningEffort> {
        match self {
            Self::Auto if model.capabilities.reasoning => Some(ReasoningEffort::Medium),
            Self::Off | Self::Auto => None,
            Self::Minimal => Some(ReasoningEffort::Minimal),
            Self::Low => Some(ReasoningEffort::Low),
            Self::Medium => Some(ReasoningEffort::Medium),
            Self::High => Some(ReasoningEffort::High),
            Self::XHigh => Some(ReasoningEffort::XHigh),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReasoningContinuation {
    pub provider: ProviderId,
    pub api: ApiKind,
}

impl ReasoningContinuation {
    #[must_use]
    pub fn matches_model(&self, model: &ModelSpec) -> bool {
        self.provider == model.provider && self.api == model.api
    }
}

#[must_use]
pub fn sanitize_reasoning_continuation(
    messages: Vec<ChatMessage>,
    origin: Option<&ReasoningContinuation>,
    target: &ModelSpec,
) -> Vec<ChatMessage> {
    if origin.is_some_and(|origin| origin.matches_model(target)) {
        return messages;
    }
    messages.into_iter().map(strip_opaque_reasoning).collect()
}

fn strip_opaque_reasoning(message: ChatMessage) -> ChatMessage {
    match message {
        ChatMessage::System { content } => ChatMessage::System {
            content: filter_opaque_reasoning(content),
        },
        ChatMessage::User { content } => ChatMessage::User {
            content: filter_opaque_reasoning(content),
        },
        ChatMessage::Assistant {
            content,
            tool_calls,
        } => ChatMessage::Assistant {
            content: filter_opaque_reasoning(content),
            tool_calls,
        },
        ChatMessage::ToolResult {
            tool_call_id,
            content,
            is_error,
        } => ChatMessage::ToolResult {
            tool_call_id,
            content: filter_opaque_reasoning(content),
            is_error,
        },
    }
}

fn filter_opaque_reasoning(content: Vec<ContentPart>) -> Vec<ContentPart> {
    content
        .into_iter()
        .filter(|part| match part {
            ContentPart::Thinking {
                signature,
                redacted,
                ..
            } => signature.is_none() && !redacted,
            ContentPart::Text { .. } | ContentPart::Image { .. } => true,
        })
        .collect()
}
