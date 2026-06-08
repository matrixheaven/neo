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
                content: content.iter().filter_map(to_content_part).collect(),
            },
            Self::User { content } => ChatMessage::User {
                content: content.iter().filter_map(to_content_part).collect(),
            },
            Self::Assistant {
                content,
                tool_calls,
                stop_reason: _,
            } => ChatMessage::Assistant {
                content: content.iter().filter_map(to_content_part).collect(),
                tool_calls: tool_calls.iter().cloned().map(Into::into).collect(),
            },
            Self::ToolResult {
                tool_call_id,
                tool_name: _,
                content,
                is_error,
            } => ChatMessage::ToolResult {
                tool_call_id: tool_call_id.clone(),
                content: content.iter().filter_map(to_content_part).collect(),
                is_error: *is_error,
            },
        }
    }
}

fn to_content_part(content: &Content) -> Option<ContentPart> {
    match content {
        Content::Text { text } => Some(ContentPart::Text { text: text.clone() }),
        Content::Thinking { .. } => None,
        Content::Image { mime_type, data } => Some(ContentPart::Image {
            mime_type: mime_type.clone(),
            data: match data {
                ImageRef::Base64(value) => ImageData::Base64(value.clone()),
                ImageRef::Url(value) => ImageData::Url(value.clone()),
            },
        }),
    }
}
