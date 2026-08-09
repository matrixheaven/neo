use std::{borrow::Cow, collections::BTreeMap, fmt, str::FromStr, time::Duration};

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum CacheRetention {
    None,
    #[default]
    Short,
    Long,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ReasoningEffort(String);

impl ReasoningEffort {
    pub const MINIMAL: &'static str = "minimal";
    pub const LOW: &'static str = "low";
    pub const MEDIUM: &'static str = "medium";
    pub const HIGH: &'static str = "high";
    pub const XHIGH: &'static str = "xhigh";
    pub const MAX: &'static str = "max";

    #[must_use]
    pub fn minimal() -> Self {
        Self(Self::MINIMAL.to_owned())
    }

    #[must_use]
    pub fn low() -> Self {
        Self(Self::LOW.to_owned())
    }

    #[must_use]
    pub fn medium() -> Self {
        Self(Self::MEDIUM.to_owned())
    }

    #[must_use]
    pub fn high() -> Self {
        Self(Self::HIGH.to_owned())
    }

    #[must_use]
    pub fn xhigh() -> Self {
        Self(Self::XHIGH.to_owned())
    }

    #[must_use]
    pub fn max() -> Self {
        Self(Self::MAX.to_owned())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidReasoningEffort;

impl fmt::Display for InvalidReasoningEffort {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("reasoning effort cannot be empty or whitespace-only")
    }
}

impl std::error::Error for InvalidReasoningEffort {}

impl TryFrom<String> for ReasoningEffort {
    type Error = InvalidReasoningEffort;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.trim().is_empty() {
            return Err(InvalidReasoningEffort);
        }
        Ok(Self(value))
    }
}

impl TryFrom<&str> for ReasoningEffort {
    type Error = InvalidReasoningEffort;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_from(value.to_owned())
    }
}

impl FromStr for ReasoningEffort {
    type Err = InvalidReasoningEffort;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value)
    }
}

impl fmt::Display for ReasoningEffort {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ReasoningEffort {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::try_from(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for ReasoningEffort {
    fn schema_name() -> Cow<'static, str> {
        "ReasoningEffort".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": r"\S"
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ReasoningSelection {
    #[default]
    Off,
    On,
    Effort {
        effort: ReasoningEffort,
    },
    BudgetTokens {
        budget_tokens: u32,
    },
}

impl ReasoningSelection {
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        !matches!(self, Self::Off)
    }

    #[must_use]
    pub const fn effort(&self) -> Option<&ReasoningEffort> {
        match self {
            Self::Effort { effort } => Some(effort),
            Self::Off | Self::On | Self::BudgetTokens { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReasoningBudget {
    pub min: Option<u32>,
    pub max: Option<u32>,
}

impl ReasoningBudget {
    #[must_use]
    pub const fn contains(&self, budget_tokens: u32) -> bool {
        if let Some(min) = self.min
            && budget_tokens < min
        {
            return false;
        }
        if let Some(max) = self.max
            && budget_tokens > max
        {
            return false;
        }
        true
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReasoningCapability {
    #[default]
    None,
    Toggle {
        disable_supported: bool,
    },
    Effort {
        values: Vec<ReasoningEffort>,
        disable_supported: bool,
    },
    BudgetTokens {
        min: Option<u32>,
        max: Option<u32>,
        disable_supported: bool,
    },
    Combined {
        toggle: bool,
        effort: Vec<ReasoningEffort>,
        budget: Option<ReasoningBudget>,
        disable_supported: bool,
    },
}

impl ReasoningCapability {
    #[must_use]
    pub fn supports_reasoning(&self) -> bool {
        match self {
            Self::None => false,
            Self::Toggle { .. } | Self::BudgetTokens { .. } => true,
            Self::Effort { values, .. } => !values.is_empty(),
            Self::Combined {
                toggle,
                effort,
                budget,
                ..
            } => *toggle || !effort.is_empty() || budget.is_some(),
        }
    }

    #[must_use]
    pub fn supports(&self, selection: &ReasoningSelection) -> bool {
        match selection {
            ReasoningSelection::Off => matches!(self, Self::None) || self.disable_supported(),
            ReasoningSelection::On => match self {
                Self::Toggle { .. } => true,
                Self::Combined { toggle, .. } => *toggle,
                Self::None | Self::Effort { .. } | Self::BudgetTokens { .. } => false,
            },
            ReasoningSelection::Effort { effort } => match self {
                Self::Effort { values, .. } | Self::Combined { effort: values, .. } => {
                    values.contains(effort)
                }
                Self::None | Self::Toggle { .. } | Self::BudgetTokens { .. } => false,
            },
            ReasoningSelection::BudgetTokens { budget_tokens } => match self {
                Self::BudgetTokens { min, max, .. } => ReasoningBudget {
                    min: *min,
                    max: *max,
                }
                .contains(*budget_tokens),
                Self::Combined { budget, .. } => budget
                    .as_ref()
                    .is_some_and(|budget| budget.contains(*budget_tokens)),
                Self::None | Self::Toggle { .. } | Self::Effort { .. } => false,
            },
        }
    }

    #[must_use]
    pub const fn disable_supported(&self) -> bool {
        match self {
            Self::None => true,
            Self::Toggle { disable_supported }
            | Self::Effort {
                disable_supported, ..
            }
            | Self::BudgetTokens {
                disable_supported, ..
            }
            | Self::Combined {
                disable_supported, ..
            } => *disable_supported,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RequestMetadata {
    values: BTreeMap<String, String>,
}

impl RequestMetadata {
    #[must_use]
    pub fn from_pairs<const N: usize>(pairs: [(&str, &str); N]) -> Self {
        Self {
            values: pairs
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect(),
        }
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    #[must_use]
    pub fn as_map(&self) -> &BTreeMap<String, String> {
        &self.values
    }
}

/// Provider-neutral structured-output hint (JSON Schema + name + strictness).
///
/// Wire clients that can express this contract map it into their native request
/// body. Clients that cannot simply omit the hint. Host schema validation is
/// always authoritative — presence of this field never means the response was
/// accepted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ResponseFormat {
    /// Stable schema identifier required by some provider wire formats.
    pub name: String,
    /// JSON Schema document describing the expected response value.
    pub schema: Value,
    /// When true, request strict provider-side adherence where the wire format
    /// supports it. Host validation still runs either way.
    pub strict: bool,
}

impl ResponseFormat {
    /// OpenAI Chat Completions `response_format` object.
    #[must_use]
    pub fn to_openai_chat_response_format(&self) -> Value {
        serde_json::json!({
            "type": "json_schema",
            "json_schema": {
                "name": self.name,
                "strict": self.strict,
                "schema": self.schema,
            }
        })
    }

    /// OpenAI Responses API `text.format` object.
    #[must_use]
    pub fn to_openai_responses_text_format(&self) -> Value {
        serde_json::json!({
            "type": "json_schema",
            "name": self.name,
            "strict": self.strict,
            "schema": self.schema,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RequestOptions {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub headers: BTreeMap<String, String>,
    #[schemars(skip)]
    pub timeout: Option<Duration>,
    pub reasoning: ReasoningSelection,
    pub replay_reasoning: bool,
    /// Explicitly disable reasoning even when the provider would otherwise
    /// default to emitting a reasoning/thinking block. Providers that support
    /// an explicit disable setting serialize it for background requests (for
    /// example, session titles) that must stay fast and deterministic.
    #[serde(default)]
    pub disable_reasoning: bool,
    pub cache: CacheRetention,
    /// Session-correlation identifier (for example `x-client-request-id`).
    /// This is NOT a cache lane key: its semantics never change.
    pub session_id: Option<String>,
    /// Dedicated prompt-cache lane key computed from session + provider +
    /// model + static projection shape. Providers that support a prompt-cache
    /// key field map this field onto it (for example `OpenAI` `prompt_cache_key`);
    /// `session_id` keeps its own correlation semantics and is never reused
    /// as the cache lane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    pub metadata: RequestMetadata,
    /// Optional provider-native structured-output hint. Host validation remains
    /// authoritative regardless of whether a provider honors this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
}

impl Default for RequestOptions {
    fn default() -> Self {
        Self {
            temperature: None,
            max_tokens: None,
            headers: BTreeMap::new(),
            timeout: None,
            reasoning: ReasoningSelection::Off,
            replay_reasoning: true,
            disable_reasoning: false,
            cache: CacheRetention::Short,
            session_id: None,
            prompt_cache_key: None,
            metadata: RequestMetadata::default(),
            response_format: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::fake::FakeModelClient;
    use crate::types::{ApiKind, ModelCapabilities};
    use crate::{
        AiStreamEvent, ChatMessage, ChatRequest, ContentPart, MessagePhase, ModelClient, ModelSpec,
        ProviderId, StopReason,
    };
    use futures::StreamExt;
    use serde_json::json;

    #[test]
    fn response_format_schema_is_provider_neutral() {
        let format = ResponseFormat {
            name: "child_output".to_owned(),
            schema: json!({
                "type": "object",
                "properties": { "ok": { "type": "boolean" } },
                "required": ["ok"],
                "additionalProperties": false
            }),
            strict: true,
        };

        // Neutral shape: schema + name + strictness only. No provider tag, no
        // URL/model-name inference field.
        let encoded = serde_json::to_value(&format).expect("serialize");
        assert_eq!(encoded["name"], "child_output");
        assert_eq!(encoded["strict"], true);
        assert_eq!(encoded["schema"]["type"], "object");
        assert!(encoded.get("provider").is_none());
        assert!(encoded.get("api").is_none());
        assert!(encoded.get("base_url").is_none());
        assert!(encoded.get("model").is_none());

        let options = RequestOptions {
            response_format: Some(format.clone()),
            ..RequestOptions::default()
        };
        assert_eq!(options.response_format.as_ref(), Some(&format));
        assert!(RequestOptions::default().response_format.is_none());

        // OpenAI wire fragments are pure projections of the neutral value.
        assert_eq!(
            format.to_openai_chat_response_format(),
            json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "child_output",
                    "strict": true,
                    "schema": format.schema,
                }
            })
        );
        assert_eq!(
            format.to_openai_responses_text_format(),
            json!({
                "type": "json_schema",
                "name": "child_output",
                "strict": true,
                "schema": format.schema,
            })
        );
    }

    #[tokio::test]
    async fn provider_native_structured_output_is_optional_and_host_validated() {
        // Optional: default omits the hint.
        assert!(RequestOptions::default().response_format.is_none());

        let format = ResponseFormat {
            name: "host_authoritative".to_owned(),
            schema: json!({
                "type": "object",
                "properties": { "answer": { "type": "integer" } },
                "required": ["answer"],
                "additionalProperties": false
            }),
            strict: true,
        };

        // Provider-native hint is only request metadata. neo-ai does not validate
        // model output against the schema — FakeModelClient returns ordinary text
        // that violates the schema and stream collection still succeeds.
        let client = FakeModelClient::new(vec![
            AiStreamEvent::MessageStart {
                id: "m1".to_owned(),
                phase: MessagePhase::Unknown,
            },
            AiStreamEvent::TextDelta {
                text: "not-json-and-not-schema".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
                phase: MessagePhase::Unknown,
            },
        ]);

        let request = ChatRequest {
            model: ModelSpec {
                provider: ProviderId("any".to_owned()),
                model: "does-not-imply-support".to_owned(),
                api: ApiKind::Local,
                capabilities: ModelCapabilities::chat(),
            },
            messages: vec![ChatMessage::User {
                content: vec![ContentPart::Text {
                    text: "return structured".to_owned(),
                }],
            }],
            tools: vec![],
            options: RequestOptions {
                response_format: Some(format),
                ..RequestOptions::default()
            },
        };

        let events = client
            .stream_chat(request)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("neo-ai must not host-validate structured output");

        assert!(
            events.iter().any(|event| matches!(
                event,
                AiStreamEvent::TextDelta { text } if text == "not-json-and-not-schema"
            )),
            "provider-native hint is optional wire metadata; host validates later"
        );
        let recorded = client.requests();
        assert_eq!(recorded.len(), 1);
        assert!(recorded[0].options.response_format.is_some());
        // Support is never inferred from model/provider name strings on the request.
        assert_eq!(recorded[0].model.model, "does-not-imply-support");
        assert_eq!(recorded[0].model.provider.0, "any");
    }
}
