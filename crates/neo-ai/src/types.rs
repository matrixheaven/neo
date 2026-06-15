use futures::stream::BoxStream;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AiError, RequestOptions};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ApiKind {
    OpenAiResponses,
    OpenAiChatCompletions,
    AnthropicMessages,
    GoogleGenerativeAi,
    OpenAiCompatible,
    Local,
}

/// Provider protocol type — the user-facing type declared in `config.toml`
/// `[providers.<id>].type`. It determines which wire-protocol client is used.
/// This is the config-level counterpart of [`ApiKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ApiType {
    /// OpenAI Responses API (`/responses` endpoint).
    #[serde(rename = "openai-responses")]
    OpenAiResponses,
    /// OpenAI Chat Completions — third-party compatible endpoints.
    #[serde(rename = "openai-compatible")]
    OpenAiCompatible,
    /// OpenAI Chat Completions — native OpenAI endpoint.
    #[serde(rename = "openai-chat")]
    OpenAiChat,
    /// Anthropic Messages API.
    #[serde(rename = "anthropic")]
    Anthropic,
    /// Google Generative AI.
    #[serde(rename = "google")]
    Google,
}

impl Default for ApiType {
    fn default() -> Self {
        Self::OpenAiCompatible
    }
}

impl ApiType {
    /// Convert to the internal [`ApiKind`] used by `ModelSpec`.
    #[must_use]
    pub const fn to_api_kind(self) -> ApiKind {
        match self {
            Self::OpenAiResponses => ApiKind::OpenAiResponses,
            Self::OpenAiCompatible => ApiKind::OpenAiCompatible,
            Self::OpenAiChat => ApiKind::OpenAiChatCompletions,
            Self::Anthropic => ApiKind::AnthropicMessages,
            Self::Google => ApiKind::GoogleGenerativeAi,
        }
    }

    /// Parse from a config string (case-insensitive, kebab-case preferred).
    /// Accepts both `"openai-responses"` and `"OpenAiResponses"` style names.
    #[must_use]
    pub fn from_config_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "openai-responses" | "openairesponses" => Some(Self::OpenAiResponses),
            "openai-compatible" | "openaicompatible" | "openai-completions" => {
                Some(Self::OpenAiCompatible)
            }
            "openai-chat"
            | "openaichat"
            | "openai-chat-completions"
            | "openaichatcompletions"
            | "openai" => Some(Self::OpenAiChat),
            "anthropic" | "anthropic-messages" | "anthropicmessages" => Some(Self::Anthropic),
            "google" | "google-generative-ai" | "googlegenerativeai" | "google-genai" => {
                Some(Self::Google)
            }
            _ => None,
        }
    }

    /// Convert to a kebab-case config string.
    #[must_use]
    pub const fn as_config_str(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "openai-responses",
            Self::OpenAiCompatible => "openai-compatible",
            Self::OpenAiChat => "openai-chat",
            Self::Anthropic => "anthropic",
            Self::Google => "google",
        }
    }
}

/// Parse an [`ApiKind`] from a config-style string, used by legacy JSON catalogs.
#[must_use]
pub fn api_kind_from_str(s: &str) -> Option<ApiKind> {
    ApiType::from_config_str(s).map(ApiType::to_api_kind)
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
