use futures::stream::BoxStream;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{AiError, ReasoningCapability, RequestOptions};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ApiKind {
    OpenAiResponse,
    OpenAi,
    AnthropicMessages,
    GoogleGenerativeAi,
    Local,
}

/// Provider protocol type — the user-facing type declared in `config.toml`
/// `[providers.<id>].type`. It determines which wire-protocol client is used.
/// This is the config-level counterpart of [`ApiKind`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub enum ApiType {
    /// `OpenAI` Responses API (`/responses` endpoint).
    #[serde(rename = "openai_response")]
    OpenAiResponse,
    /// `OpenAI` Chat Completions compatible endpoint.
    #[serde(rename = "openai")]
    #[default]
    OpenAi,
    /// Anthropic Messages API.
    #[serde(rename = "anthropic")]
    Anthropic,
    /// Google Generative AI.
    #[serde(rename = "google")]
    Google,
}

impl<'de> Deserialize<'de> for ApiType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_config_str(&value)
            .ok_or_else(|| serde::de::Error::custom(provider_type_error_message(value.as_str())))
    }
}

impl ApiType {
    /// Convert to the internal [`ApiKind`] used by `ModelSpec`.
    #[must_use]
    pub const fn to_api_kind(self) -> ApiKind {
        match self {
            Self::OpenAiResponse => ApiKind::OpenAiResponse,
            Self::OpenAi => ApiKind::OpenAi,
            Self::Anthropic => ApiKind::AnthropicMessages,
            Self::Google => ApiKind::GoogleGenerativeAi,
        }
    }

    /// Parse from a config string (case-insensitive, kebab-case preferred).
    #[must_use]
    pub fn from_config_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "openai_response" => Some(Self::OpenAiResponse),
            "openai" => Some(Self::OpenAi),
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
            Self::OpenAiResponse => "openai_response",
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::Google => "google",
        }
    }
}

fn provider_type_error_message(value: &str) -> String {
    let normalized = value.trim().to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "openai-chat" | "openai-compatible" | "openai-completions" | "openai-responses"
    ) {
        format!(
            "provider type '{value}' has been removed; use 'openai' for Chat Completions/OpenAI-compatible providers or 'openai_response' for the Responses API"
        )
    } else {
        format!(
            "unknown provider type '{value}'; expected 'openai', 'openai_response', 'anthropic', or 'google'"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_type_config_strings_are_canonical_kebab_case() {
        assert_eq!(ApiType::OpenAiResponse.as_config_str(), "openai_response");
        assert_eq!(ApiType::OpenAi.as_config_str(), "openai");
        assert_eq!(ApiType::Anthropic.as_config_str(), "anthropic");
        assert_eq!(ApiType::Google.as_config_str(), "google");
    }

    #[test]
    fn api_type_config_strings_round_trip() {
        for api_type in [
            ApiType::OpenAiResponse,
            ApiType::OpenAi,
            ApiType::Anthropic,
            ApiType::Google,
        ] {
            assert_eq!(
                ApiType::from_config_str(api_type.as_config_str()),
                Some(api_type)
            );
        }
    }

    #[test]
    fn api_type_accepts_only_openai_and_openai_response() {
        assert_eq!(ApiType::from_config_str("openai"), Some(ApiType::OpenAi));
        assert_eq!(
            ApiType::from_config_str("openai_response"),
            Some(ApiType::OpenAiResponse)
        );

        for removed in [
            "openai-chat",
            "openai-compatible",
            "openai-completions",
            "openai-responses",
        ] {
            assert_eq!(ApiType::from_config_str(removed), None);
        }
    }

    #[test]
    fn removed_openai_provider_types_report_migration_hint() {
        let err = serde_json::from_str::<ApiType>(r#""openai-compatible""#).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("has been removed"), "{message}");
        assert!(message.contains("openai_response"), "{message}");
    }
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelCapabilities {
    pub streaming: bool,
    pub tools: bool,
    pub images: bool,
    #[serde(default)]
    pub reasoning: ReasoningCapability,
    pub embeddings: bool,
    pub max_context_tokens: Option<u32>,
    /// Maximum output tokens the model can emit in a single response.
    /// When `Some`, this feeds `RequestOptions.max_tokens` unless the user
    /// overrides it with `[runtime].max_tokens`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self::chat()
    }
}

impl ModelCapabilities {
    #[must_use]
    pub fn chat() -> Self {
        Self {
            streaming: true,
            tools: false,
            images: false,
            reasoning: ReasoningCapability::None,
            embeddings: false,
            max_context_tokens: None,
            max_output_tokens: None,
        }
    }

    #[must_use]
    pub fn tool_chat() -> Self {
        Self {
            tools: true,
            ..Self::chat()
        }
    }

    #[must_use]
    pub fn vision_chat() -> Self {
        Self {
            images: true,
            ..Self::chat()
        }
    }

    #[must_use]
    pub fn reasoning_chat() -> Self {
        Self {
            reasoning: ReasoningCapability::Toggle {
                disable_supported: true,
            },
            ..Self::chat()
        }
    }

    #[must_use]
    pub fn embedding() -> Self {
        Self {
            streaming: false,
            tools: false,
            images: false,
            reasoning: ReasoningCapability::None,
            embeddings: true,
            max_context_tokens: None,
            max_output_tokens: None,
        }
    }

    #[must_use]
    pub fn with_max_context_tokens(mut self, max_context_tokens: u32) -> Self {
        self.max_context_tokens = Some(max_context_tokens);
        self
    }

    #[must_use]
    pub fn with_max_output_tokens(mut self, max_output_tokens: u32) -> Self {
        self.max_output_tokens = Some(max_output_tokens);
        self
    }

    #[must_use]
    pub fn supports_reasoning(&self) -> bool {
        self.reasoning.supports_reasoning()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelSpec {
    pub provider: ProviderId,
    pub model: String,
    /// Catalog metadata for display and model selection. Wire protocol is owned by
    /// the registered provider's [`ApiType`].
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
    pub raw_arguments: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
    #[serde(default)]
    pub input_cache_read_tokens: u32,
    #[serde(default)]
    pub input_cache_write_tokens: u32,
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
        raw_arguments: String,
    },
    MessageEnd {
        stop_reason: StopReason,
        usage: Option<TokenUsage>,
    },
}

pub trait ModelClient: Send + Sync {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> BoxStream<'static, Result<AiStreamEvent, AiError>>;
}
