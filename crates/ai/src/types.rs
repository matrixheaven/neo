use futures::stream::BoxStream;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AiError, RequestOptions};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ApiKind {
    OpenAiResponses,
    OpenAiChatCompletions,
    AnthropicMessages,
    GoogleGenerativeAi,
    OpenAiCompatible,
    Local,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelCapabilities {
    pub streaming: bool,
    pub tools: bool,
    pub images: bool,
    pub reasoning: bool,
    pub embeddings: bool,
    pub max_context_tokens: Option<u32>,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self::chat()
    }
}

impl ModelCapabilities {
    #[must_use]
    pub const fn chat() -> Self {
        Self {
            streaming: true,
            tools: false,
            images: false,
            reasoning: false,
            embeddings: false,
            max_context_tokens: None,
        }
    }

    #[must_use]
    pub const fn tool_chat() -> Self {
        Self {
            tools: true,
            ..Self::chat()
        }
    }

    #[must_use]
    pub const fn vision_chat() -> Self {
        Self {
            images: true,
            ..Self::chat()
        }
    }

    #[must_use]
    pub const fn reasoning_chat() -> Self {
        Self {
            reasoning: true,
            ..Self::chat()
        }
    }

    #[must_use]
    pub const fn embedding() -> Self {
        Self {
            streaming: false,
            tools: false,
            images: false,
            reasoning: false,
            embeddings: true,
            max_context_tokens: None,
        }
    }

    #[must_use]
    pub const fn with_max_context_tokens(mut self, max_context_tokens: u32) -> Self {
        self.max_context_tokens = Some(max_context_tokens);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelSpec {
    pub provider: ProviderId,
    pub model: String,
    pub api: ApiKind,
    pub capabilities: ModelCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ImageData {
    Base64(String),
    Url(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ContentPart {
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
        data: ImageData,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ChatMessage {
    System {
        content: Vec<ContentPart>,
    },
    User {
        content: Vec<ContentPart>,
    },
    Assistant {
        content: Vec<ContentPart>,
        tool_calls: Vec<ToolCall>,
    },
    ToolResult {
        tool_call_id: String,
        content: Vec<ContentPart>,
        is_error: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

impl ToolSpec {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }

    #[must_use]
    pub fn from_schema<T: JsonSchema>(
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self::new(
            name,
            description,
            crate::tool_schema::root_schema_for::<T>(),
        )
    }

    #[must_use]
    pub fn string_arg(
        name: impl Into<String>,
        description: impl Into<String>,
        arg_name: impl Into<String>,
        arg_description: impl Into<String>,
    ) -> Self {
        let arg_name = arg_name.into();
        Self::new(
            name,
            description,
            serde_json::json!({
                "type": "object",
                "properties": {
                    arg_name.clone(): {
                        "type": "string",
                        "description": arg_description.into(),
                    },
                },
                "required": [arg_name],
                "additionalProperties": false,
            }),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChatRequest {
    pub model: ModelSpec,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSpec>,
    pub options: RequestOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Cancelled,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum AiStreamEvent {
    MessageStart {
        id: String,
    },
    ThinkingStart {
        id: String,
    },
    ThinkingDelta {
        text: String,
    },
    ThinkingEnd {
        signature: Option<String>,
        redacted: bool,
    },
    TextDelta {
        text: String,
    },
    ToolCallStart {
        id: String,
        name: String,
    },
    ToolCallArgsDelta {
        id: String,
        json_fragment: String,
    },
    ToolCallEnd {
        id: String,
        arguments: serde_json::Value,
    },
    MessageEnd {
        stop_reason: StopReason,
        usage: Option<TokenUsage>,
    },
    Error {
        message: String,
    },
}

pub trait ModelClient: Send + Sync {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> BoxStream<'static, Result<AiStreamEvent, AiError>>;
}
