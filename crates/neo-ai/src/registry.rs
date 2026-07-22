use std::{collections::BTreeMap, sync::Arc};

use crate::{
    AiError, ApiKind, ApiType, ModelCapabilities, ModelClient, ModelSpec, ProviderId,
    ReasoningCapability,
    auth::env_value,
    providers::{
        anthropic::AnthropicMessagesClient, google::GoogleGenerativeAiClient,
        openai::compatible::OpenAiCompatibleClient, openai::responses::OpenAiResponsesClient,
    },
};
#[derive(Debug, Clone, Default)]
pub struct ModelRegistry {
    models: Vec<ModelSpec>,
    default: Option<(String, String)>,
}

impl ModelRegistry {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            models: Vec::new(),
            default: None,
        }
    }

    #[must_use]
    pub fn seeded() -> Self {
        let mut registry = Self::new();
        registry.register_builtin_models();
        registry
    }

    pub fn register_builtin_models(&mut self) {
        for model in builtin_models() {
            self.register(model);
        }
    }

    pub fn register(&mut self, model: ModelSpec) {
        let key = model_key(&model);
        if self.default.is_none() {
            self.default = Some(key.clone());
        }

        if let Some(existing) = self
            .models
            .iter_mut()
            .find(|existing| model_key(existing) == key)
        {
            *existing = model;
        } else {
            self.models.push(model);
        }
    }

    #[must_use]
    pub fn list(&self) -> Vec<ModelSpec> {
        self.models.clone()
    }

    #[must_use]
    pub fn get(&self, provider: &str, model: &str) -> Option<&ModelSpec> {
        self.models
            .iter()
            .find(|spec| spec.provider.0 == provider && spec.model == model)
    }

    #[must_use]
    pub fn default_model(&self) -> Option<&ModelSpec> {
        let (provider, model) = self.default.as_ref()?;
        self.get(provider, model)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSpec {
    pub id: String,
    pub display_name: String,
    pub base_url: Option<String>,
    /// Inline API key stored in config (e.g. `api_key = "sk-..."`).
    /// Takes priority over `api_key_env_vars` during credential resolution.
    pub api_key: Option<String>,
    pub api_key_env_vars: Vec<String>,
    /// Protocol type declared in config.toml `[providers.<id>].type`.
    /// The resolver uses this to select the wire client.
    pub provider_type: ApiType,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderRegistry {
    providers: BTreeMap<String, ProviderSpec>,
}

impl ProviderRegistry {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            providers: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn production() -> Self {
        let mut registry = Self::new();
        registry.register_builtin_providers();
        registry
    }

    pub fn register_builtin_providers(&mut self) {
        for provider in builtin_providers() {
            self.register(provider);
        }
    }

    pub fn register(&mut self, provider: ProviderSpec) {
        self.providers.insert(provider.id.clone(), provider);
    }

    #[must_use]
    pub fn get(&self, provider: &str) -> Option<&ProviderSpec> {
        self.providers.get(provider)
    }

    #[must_use]
    pub fn resolver(&self) -> ProviderResolver {
        ProviderResolver {
            registry: self.clone(),
            env: std::env::vars().collect(),
        }
    }

    #[must_use]
    pub fn resolver_from(&self, env: BTreeMap<String, String>) -> ProviderResolver {
        ProviderResolver {
            registry: self.clone(),
            env,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderResolver {
    registry: ProviderRegistry,
    env: BTreeMap<String, String>,
}

impl ProviderResolver {
    pub fn resolve(&self, model: &ModelSpec) -> Result<Arc<dyn ModelClient>, AiError> {
        let provider = self.registry.get(&model.provider.0).ok_or_else(|| {
            AiError::Configuration {
                message: format!(
                    "provider {} is not registered. Define it in config.toml with [providers.{}]",
                    model.provider.0, model.provider.0
                ),
            }
        })?;

        // Credential: inline api_key > env vars.
        let api_key = provider
            .api_key
            .clone()
            .or_else(|| api_key_from_provider(provider, &self.env))
            .ok_or_else(|| {
                let reason = missing_reason(provider);
                AiError::Configuration {
                    message: format!(
                        "missing credentials for provider {} ({reason})",
                        provider.id
                    ),
                }
            })?;

        let base_url = provider
            .base_url
            .as_deref()
            .ok_or_else(|| AiError::Configuration {
                message: format!("provider {} does not define a base URL", provider.id),
            })?;

        match provider.provider_type {
            ApiType::OpenAiResponse => Ok(Arc::new(OpenAiResponsesClient::new(base_url, api_key))),
            ApiType::Anthropic => Ok(Arc::new(AnthropicMessagesClient::new(base_url, api_key))),
            ApiType::OpenAi => Ok(Arc::new(OpenAiCompatibleClient::new(base_url, api_key))),
            ApiType::Google => Ok(Arc::new(GoogleGenerativeAiClient::new(base_url, api_key))),
        }
    }
}

fn api_key_from_provider(
    provider: &ProviderSpec,
    env: &BTreeMap<String, String>,
) -> Option<String> {
    provider.api_key_env_vars.iter().find_map(|key| {
        env_value(env, key)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

fn missing_reason(provider: &ProviderSpec) -> String {
    match provider.api_key_env_vars.as_slice() {
        [] => "no environment credential sources are registered".to_owned(),
        [key] => format!("missing {key}"),
        options => format!("missing one of: {}", options.join("; ")),
    }
}

fn model_key(model: &ModelSpec) -> (String, String) {
    (model.provider.0.clone(), model.model.clone())
}

fn builtin_models() -> Vec<ModelSpec> {
    vec![
        builtin_model(
            "openai",
            "gpt-5.4",
            ApiKind::OpenAiResponse,
            reasoning_tool_chat(400_000),
        ),
        builtin_model(
            "openai",
            "gpt-5-mini",
            ApiKind::OpenAiResponse,
            reasoning_tool_chat(400_000),
        ),
        builtin_model(
            "openai",
            "gpt-4.1",
            ApiKind::OpenAiResponse,
            ModelCapabilities::tool_chat().with_max_context_tokens(1_047_576),
        ),
        builtin_model(
            "openai",
            "gpt-4o-mini",
            ApiKind::OpenAi,
            ModelCapabilities::tool_chat().with_max_context_tokens(128_000),
        ),
        builtin_model(
            "anthropic",
            "claude-sonnet-4-5",
            ApiKind::AnthropicMessages,
            reasoning_tool_chat(200_000),
        ),
        builtin_model(
            "google",
            "gemini-2.5-pro",
            ApiKind::GoogleGenerativeAi,
            reasoning_tool_chat(1_000_000),
        ),
    ]
}

fn reasoning_tool_chat(max_context_tokens: u32) -> ModelCapabilities {
    let mut capabilities =
        ModelCapabilities::tool_chat().with_max_context_tokens(max_context_tokens);
    capabilities.reasoning = ReasoningCapability::Toggle {
        disable_supported: true,
    };
    capabilities
}

fn builtin_model(
    provider: &str,
    model: &str,
    api: ApiKind,
    capabilities: ModelCapabilities,
) -> ModelSpec {
    ModelSpec {
        provider: ProviderId(provider.to_owned()),
        model: model.to_owned(),
        api,
        capabilities,
    }
}

fn builtin_providers() -> Vec<ProviderSpec> {
    vec![
        provider(
            "openai",
            "OpenAI",
            ApiType::OpenAiResponse,
            Some("https://api.openai.com/v1"),
            &["OPENAI_API_KEY"],
        ),
        provider(
            "anthropic",
            "Anthropic",
            ApiType::Anthropic,
            Some("https://api.anthropic.com/v1"),
            &["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"],
        ),
        provider(
            "google",
            "Google Generative AI",
            ApiType::Google,
            Some("https://generativelanguage.googleapis.com/v1beta"),
            &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        ),
        provider(
            "openrouter",
            "OpenRouter",
            ApiType::OpenAi,
            Some("https://openrouter.ai/api/v1"),
            &["OPENROUTER_API_KEY"],
        ),
    ]
}

fn provider(
    id: &str,
    display_name: &str,
    provider_type: ApiType,
    base_url: Option<&str>,
    api_key_env_vars: &[&str],
) -> ProviderSpec {
    ProviderSpec {
        id: id.to_owned(),
        display_name: display_name.to_owned(),
        base_url: base_url.map(str::to_owned),
        api_key: None,
        api_key_env_vars: api_key_env_vars
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        provider_type,
    }
}
