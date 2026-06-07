use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{
    AiError, ApiKind, ModelCapabilities, ModelClient, ModelSpec, ProviderId,
    providers::{
        anthropic::AnthropicMessagesClient, google::GoogleGenerativeAiClient,
        openai_compatible::OpenAiCompatibleClient, openai_responses::OpenAiResponsesClient,
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
    pub api: ApiKind,
    pub base_url: Option<String>,
    pub api_key_env_vars: Vec<String>,
    pub ambient_auth_env_vars: Vec<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCredentialStatus {
    pub provider: String,
    pub configured: bool,
    pub env_keys: Vec<String>,
    pub authenticated_label: Option<String>,
    pub missing_reason: Option<String>,
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
    pub fn list(&self) -> Vec<ProviderSpec> {
        self.providers.values().cloned().collect()
    }

    #[must_use]
    pub fn credential_status(&self, provider: &str) -> Option<ProviderCredentialStatus> {
        self.credential_status_from(provider, &std::env::vars().collect())
    }

    #[must_use]
    pub fn credential_status_from(
        &self,
        provider: &str,
        env: &BTreeMap<String, String>,
    ) -> Option<ProviderCredentialStatus> {
        let spec = self.get(provider)?;
        let env_keys = configured_env_keys(spec, env);
        let authenticated = spec.ambient_auth_env_vars.iter().any(|group| {
            group
                .iter()
                .all(|key| env.get(key).is_some_and(|value| !value.is_empty()))
        });
        let configured = !env_keys.is_empty() || authenticated;
        let missing_reason = (!configured).then(|| missing_reason(spec));

        Some(ProviderCredentialStatus {
            provider: provider.to_owned(),
            configured,
            env_keys,
            authenticated_label: authenticated.then(|| "<authenticated>".to_owned()),
            missing_reason,
        })
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
            AiError::Configuration(format!("provider {} is not registered", model.provider.0))
        })?;

        let api_key = api_key_from_provider(provider, &self.env).ok_or_else(|| {
            let reason = missing_reason(provider);
            AiError::Configuration(format!(
                "missing credentials for provider {} ({reason})",
                provider.id
            ))
        })?;

        let base_url = provider.base_url.as_deref().ok_or_else(|| {
            AiError::Configuration(format!(
                "provider {} does not define a base URL",
                provider.id
            ))
        })?;

        match model.api {
            ApiKind::OpenAiResponses => Ok(Arc::new(OpenAiResponsesClient::new(base_url, api_key))),
            ApiKind::AnthropicMessages => {
                Ok(Arc::new(AnthropicMessagesClient::new(base_url, api_key)))
            }
            ApiKind::OpenAiCompatible | ApiKind::OpenAiChatCompletions => {
                Ok(Arc::new(OpenAiCompatibleClient::new(base_url, api_key)))
            }
            ApiKind::GoogleGenerativeAi => {
                Ok(Arc::new(GoogleGenerativeAiClient::new(base_url, api_key)))
            }
            ApiKind::Local => Err(AiError::Configuration(format!(
                "provider {} model API {:?} is not supported by production resolver",
                provider.id, model.api
            ))),
        }
    }
}

fn api_key_from_provider(
    provider: &ProviderSpec,
    env: &BTreeMap<String, String>,
) -> Option<String> {
    provider
        .api_key_env_vars
        .iter()
        .find_map(|key| env.get(key).filter(|value| !value.is_empty()).cloned())
}

fn configured_env_keys(provider: &ProviderSpec, env: &BTreeMap<String, String>) -> Vec<String> {
    provider
        .api_key_env_vars
        .iter()
        .filter(|key| env.get(*key).is_some_and(|value| !value.is_empty()))
        .cloned()
        .collect()
}

fn missing_reason(provider: &ProviderSpec) -> String {
    let mut options: Vec<String> = provider.api_key_env_vars.clone();
    options.extend(
        provider
            .ambient_auth_env_vars
            .iter()
            .map(|group| group.join(" + ")),
    );

    match options.as_slice() {
        [] => "no environment credential sources are registered".to_owned(),
        [key] => format!("missing {key}"),
        _ => format!("missing one of: {}", options.join("; ")),
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
            ApiKind::OpenAiResponses,
            ModelCapabilities::tool_chat().with_max_context_tokens(400_000),
        ),
        builtin_model(
            "openai",
            "gpt-5-mini",
            ApiKind::OpenAiResponses,
            ModelCapabilities::tool_chat().with_max_context_tokens(400_000),
        ),
        builtin_model(
            "openai",
            "gpt-4.1",
            ApiKind::OpenAiResponses,
            ModelCapabilities::tool_chat().with_max_context_tokens(1_047_576),
        ),
        builtin_model(
            "openai",
            "gpt-4o-mini",
            ApiKind::OpenAiChatCompletions,
            ModelCapabilities::tool_chat().with_max_context_tokens(128_000),
        ),
        builtin_model(
            "anthropic",
            "claude-sonnet-4-5",
            ApiKind::AnthropicMessages,
            ModelCapabilities::tool_chat().with_max_context_tokens(200_000),
        ),
        builtin_model(
            "google",
            "gemini-2.5-pro",
            ApiKind::GoogleGenerativeAi,
            ModelCapabilities::tool_chat().with_max_context_tokens(1_000_000),
        ),
    ]
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
            ApiKind::OpenAiResponses,
            Some("https://api.openai.com/v1"),
            &["OPENAI_API_KEY"],
            &[],
        ),
        provider(
            "anthropic",
            "Anthropic",
            ApiKind::AnthropicMessages,
            Some("https://api.anthropic.com/v1"),
            &["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"],
            &[],
        ),
        provider(
            "google",
            "Google Generative AI",
            ApiKind::GoogleGenerativeAi,
            Some("https://generativelanguage.googleapis.com/v1beta"),
            &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
            &[],
        ),
        provider(
            "openrouter",
            "OpenRouter",
            ApiKind::OpenAiCompatible,
            Some("https://openrouter.ai/api/v1"),
            &["OPENROUTER_API_KEY"],
            &[],
        ),
        provider(
            "amazon-bedrock",
            "Amazon Bedrock",
            ApiKind::AnthropicMessages,
            None,
            &[],
            &[
                &["AWS_PROFILE"],
                &["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY"],
                &["AWS_BEARER_TOKEN_BEDROCK"],
                &["AWS_CONTAINER_CREDENTIALS_RELATIVE_URI"],
                &["AWS_CONTAINER_CREDENTIALS_FULL_URI"],
                &["AWS_WEB_IDENTITY_TOKEN_FILE"],
            ],
        ),
    ]
}

fn provider(
    id: &str,
    display_name: &str,
    api: ApiKind,
    base_url: Option<&str>,
    api_key_env_vars: &[&str],
    ambient_auth_env_vars: &[&[&str]],
) -> ProviderSpec {
    ProviderSpec {
        id: id.to_owned(),
        display_name: display_name.to_owned(),
        api,
        base_url: base_url.map(str::to_owned),
        api_key_env_vars: api_key_env_vars
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        ambient_auth_env_vars: ambient_auth_env_vars
            .iter()
            .map(|group| group.iter().map(|value| (*value).to_owned()).collect())
            .collect(),
    }
}
