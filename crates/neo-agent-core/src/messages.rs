use std::collections::HashSet;

use neo_ai::{ChatMessage, ContentPart, ImageData, ToolCall};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::StopReason;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Content {
    Text {
        text: String,
    },
    Thinking {
        text: String,
        signature: Option<String>,
        redacted: bool,
    },
    Image {
        mime_type: String,
        data: ImageRef,
    },
}

impl Content {
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    #[must_use]
    pub fn thinking(text: impl Into<String>, signature: Option<String>, redacted: bool) -> Self {
        Self::Thinking {
            text: text.into(),
            signature,
            redacted,
        }
    }

    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            Self::Thinking { .. } | Self::Image { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ImageRef {
    Base64(String),
    Url(String),
    /// SHA-256 reference to a blob file stored in the session directory.
    Blob(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

impl From<ToolCall> for AgentToolCall {
    fn from(value: ToolCall) -> Self {
        Self {
            id: value.id,
            name: value.name,
            arguments: value.arguments,
        }
    }
}

impl From<AgentToolCall> for ToolCall {
    fn from(value: AgentToolCall) -> Self {
        Self {
            id: value.id,
            name: value.name,
            arguments: value.arguments,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum AgentMessage {
    System {
        content: Vec<Content>,
    },
    User {
        content: Vec<Content>,
    },
    Assistant {
        content: Vec<Content>,
        tool_calls: Vec<AgentToolCall>,
        stop_reason: StopReason,
    },
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        content: Vec<Content>,
        is_error: bool,
    },
}

impl AgentMessage {
    #[must_use]
    pub fn system_text(text: impl Into<String>) -> Self {
        Self::System {
            content: vec![Content::text(text)],
        }
    }

    #[must_use]
    pub fn user_text(text: impl Into<String>) -> Self {
        Self::User {
            content: vec![Content::text(text)],
        }
    }

    #[must_use]
    pub fn assistant(
        content: impl Into<Vec<Content>>,
        tool_calls: impl Into<Vec<AgentToolCall>>,
        stop_reason: StopReason,
    ) -> Self {
        Self::Assistant {
            content: content.into(),
            tool_calls: tool_calls.into(),
            stop_reason,
        }
    }

    #[must_use]
    pub fn tool_result(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        content: impl Into<Vec<Content>>,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            content: content.into(),
            is_error,
        }
    }

    #[must_use]
    pub fn to_chat_message(&self) -> ChatMessage {
        match self {
            Self::System { content } => ChatMessage::System {
                content: content.iter().map(to_content_part).collect(),
            },
            Self::User { content } => ChatMessage::User {
                content: content.iter().map(to_content_part).collect(),
            },
            Self::Assistant {
                content,
                tool_calls,
                stop_reason: _,
            } => ChatMessage::Assistant {
                content: content.iter().map(to_content_part).collect(),
                tool_calls: tool_calls.iter().cloned().map(Into::into).collect(),
            },
            Self::ToolResult {
                tool_call_id,
                tool_name: _,
                content,
                is_error,
            } => ChatMessage::ToolResult {
                tool_call_id: tool_call_id.clone(),
                content: content.iter().map(to_content_part).collect(),
                is_error: *is_error,
            },
        }
    }
}

/// Validate and repair tool-call/tool-result exchanges.
///
/// Every `Assistant` message with non-empty `tool_calls` must be immediately
/// followed by `ToolResult` messages whose `tool_call_id`s cover every tool
/// call. Any incomplete exchange is dropped, as are orphaned `ToolResult`
/// messages that do not belong to the immediately preceding assistant turn.
/// This prevents provider 400 errors such as "an assistant message with
/// `tool_calls` must be followed by tool messages responding to each
/// `tool_call_id`".
#[must_use]
pub fn sanitize_tool_exchange_messages(messages: Vec<AgentMessage>) -> Vec<AgentMessage> {
    let mut out = Vec::with_capacity(messages.len());
    let mut i = 0;
    while i < messages.len() {
        match &messages[i] {
            AgentMessage::Assistant { tool_calls, .. } if !tool_calls.is_empty() => {
                let ids: HashSet<&str> = tool_calls
                    .iter()
                    .map(|tool_call| tool_call.id.as_str())
                    .collect();
                let mut j = i + 1;
                let mut seen = HashSet::new();
                while j < messages.len() {
                    if let AgentMessage::ToolResult { tool_call_id, .. } = &messages[j] {
                        if ids.contains(tool_call_id.as_str()) {
                            seen.insert(tool_call_id.as_str());
                            j += 1;
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                if seen.len() == ids.len() {
                    out.extend_from_slice(&messages[i..j]);
                }
                i = j;
            }
            AgentMessage::ToolResult { .. } => {
                // Orphaned tool result without a valid preceding assistant
                // exchange; drop it.
                i += 1;
            }
            _ => {
                out.push(messages[i].clone());
                i += 1;
            }
        }
    }
    out
}

fn to_content_part(content: &Content) -> ContentPart {
    match content {
        Content::Text { text } => ContentPart::Text { text: text.clone() },
        Content::Thinking {
            text,
            signature,
            redacted,
        } => ContentPart::Thinking {
            text: text.clone(),
            signature: signature.clone(),
            redacted: *redacted,
        },
        Content::Image { mime_type, data } => ContentPart::Image {
            mime_type: mime_type.clone(),
            data: match data {
                ImageRef::Base64(value) => ImageData::Base64(value.clone()),
                ImageRef::Url(value) => ImageData::Url(value.clone()),
                // Blob references must be resolved to base64 before converting
                // to provider-facing ContentPart. This arm is a fallback.
                ImageRef::Blob(_) => ImageData::Base64(String::new()),
            },
        },
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_complete_tool_exchange() {
        let messages = vec![
            AgentMessage::user_text("hi"),
            AgentMessage::assistant(
                vec![],
                vec![AgentToolCall {
                    id: "tc1".to_owned(),
                    name: "Bash".to_owned(),
                    arguments: serde_json::Value::Null,
                }],
                StopReason::ToolUse,
            ),
            AgentMessage::tool_result("tc1", "Bash", vec![Content::text("ok")], false),
            AgentMessage::user_text("thanks"),
        ];
        let out = sanitize_tool_exchange_messages(messages.clone());
        assert_eq!(out.len(), 4);
        assert!(matches!(&out[1], AgentMessage::Assistant { .. }));
    }

    #[test]
    fn sanitize_drops_orphan_assistant_with_tool_calls() {
        let messages = vec![
            AgentMessage::user_text("hi"),
            AgentMessage::assistant(
                vec![],
                vec![AgentToolCall {
                    id: "tc1".to_owned(),
                    name: "Bash".to_owned(),
                    arguments: serde_json::Value::Null,
                }],
                StopReason::ToolUse,
            ),
            AgentMessage::user_text("never mind"),
        ];
        let out = sanitize_tool_exchange_messages(messages);
        assert_eq!(out.len(), 2);
        assert!(matches!(&out[0], AgentMessage::User { .. }));
        assert!(matches!(&out[1], AgentMessage::User { .. }));
    }

    #[test]
    fn sanitize_drops_incomplete_exchange_even_with_partial_result() {
        let messages = vec![
            AgentMessage::assistant(
                vec![],
                vec![
                    AgentToolCall {
                        id: "tc1".to_owned(),
                        name: "Bash".to_owned(),
                        arguments: serde_json::Value::Null,
                    },
                    AgentToolCall {
                        id: "tc2".to_owned(),
                        name: "Bash".to_owned(),
                        arguments: serde_json::Value::Null,
                    },
                ],
                StopReason::ToolUse,
            ),
            AgentMessage::tool_result("tc1", "Bash", vec![Content::text("ok")], false),
            AgentMessage::user_text("stop"),
        ];
        let out = sanitize_tool_exchange_messages(messages);
        assert_eq!(out.len(), 1);
        assert!(matches!(&out[0], AgentMessage::User { .. }));
    }

    #[test]
    fn sanitize_drops_orphan_tool_result() {
        let messages = vec![
            AgentMessage::user_text("hi"),
            AgentMessage::tool_result("tc1", "Bash", vec![Content::text("ok")], false),
            AgentMessage::user_text("bye"),
        ];
        let out = sanitize_tool_exchange_messages(messages);
        assert_eq!(out.len(), 2);
        assert!(matches!(&out[0], AgentMessage::User { .. }));
        assert!(matches!(&out[1], AgentMessage::User { .. }));
    }

    #[test]
    fn sanitize_drops_unknown_tool_result_id_in_exchange() {
        let messages = vec![
            AgentMessage::assistant(
                vec![],
                vec![AgentToolCall {
                    id: "tc1".to_owned(),
                    name: "Bash".to_owned(),
                    arguments: serde_json::Value::Null,
                }],
                StopReason::ToolUse,
            ),
            AgentMessage::tool_result("tc1", "Bash", vec![Content::text("ok")], false),
            AgentMessage::tool_result("tc2", "Bash", vec![Content::text("orphan")], false),
            AgentMessage::user_text("next"),
        ];
        let out = sanitize_tool_exchange_messages(messages);
        assert_eq!(out.len(), 3);
        assert!(matches!(&out[0], AgentMessage::Assistant { .. }));
        assert!(matches!(&out[1], AgentMessage::ToolResult { .. }));
        assert!(matches!(&out[2], AgentMessage::User { .. }));
    }
}
