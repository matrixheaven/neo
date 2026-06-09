use std::{collections::BTreeMap, fs, path::Path, sync::Arc};

use crate::{
    AiError, ApiKind, ModelCapabilities, ModelClient, ModelSpec, ProviderId,
    providers::{
        anthropic::AnthropicMessagesClient, google::GoogleGenerativeAiClient,
        openai_compatible::OpenAiCompatibleClient, openai_responses::OpenAiResponsesClient,
    },
};
use serde::Deserialize;
use serde_json::Value;

const PI_DEFAULT_CONTEXT_WINDOW: u32 = 128_000;

#[derive(Debug, Clone, Default)]
pub struct ModelRegistry {
    models: Vec<ModelSpec>,
    display_metadata: BTreeMap<(String, String), ModelDisplayMetadata>,
    default: Option<(String, String)>,
}

impl ModelRegistry {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            models: Vec::new(),
            display_metadata: BTreeMap::new(),
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

    pub fn load_catalog_path(&mut self, path: impl AsRef<Path>) -> Result<(), AiError> {
        let path = path.as_ref();
        let source = fs::read_to_string(path).map_err(|err| {
            AiError::Configuration(format!(
                "failed to read model catalog {}: {err}",
                path.display()
            ))
        })?;
        self.load_catalog_str(&source, &path.display().to_string())
    }

    pub fn load_catalog_str(&mut self, source: &str, label: &str) -> Result<(), AiError> {
        let value: Value = serde_json::from_str(source).map_err(|err| {
            AiError::Configuration(format!("failed to parse model catalog {label}: {err}"))
        })?;
        if value.get("providers").is_some() {
            let catalog: PiModelsConfig = serde_json::from_value(value).map_err(|err| {
                AiError::Configuration(format!("failed to parse pi models.json {label}: {err}"))
            })?;
            return self.load_pi_models_config(catalog, label);
        }

        let catalog: ModelCatalog = serde_json::from_value(value).map_err(|err| {
            AiError::Configuration(format!("failed to parse model catalog {label}: {err}"))
        })?;
        if catalog.models.is_empty() {
            return Err(AiError::Configuration(format!(
                "model catalog {label} must define at least one model"
            )));
        }
        for model in &catalog.models {
            validate_catalog_model(label, model)?;
        }
        if let Some(default) = &catalog.default {
            validate_catalog_default(label, default)?;
        }

        let mut candidate = self.clone();
        for model in catalog.models {
            candidate.register(model);
        }
        if let Some(default) = catalog.default {
            if candidate.get(&default.provider, &default.model).is_none() {
                return Err(AiError::Configuration(format!(
                    "model catalog {label} default {}/{} is not registered",
                    default.provider, default.model
                )));
            }
            candidate.default = Some((default.provider, default.model));
        }
        *self = candidate;
        Ok(())
    }

    fn load_pi_models_config(
        &mut self,
        catalog: PiModelsConfig,
        label: &str,
    ) -> Result<(), AiError> {
        if catalog.providers.is_empty() {
            return Err(AiError::Configuration(format!(
                "pi models.json {label} must define at least one provider"
            )));
        }

        let mut models = Vec::new();
        let mut display_metadata = BTreeMap::new();
        for (provider, config) in catalog.providers {
            let provider = provider.trim().to_owned();
            if provider.is_empty() {
                return Err(AiError::Configuration(format!(
                    "pi models.json {label} provider must not be empty"
                )));
            }
            validate_pi_provider_metadata(label, &provider, &config.metadata)?;
            let provider_name = string_metadata(&config.metadata, "name")?;
            for model in config.models {
                let spec = pi_model_spec(label, &provider, config.api.as_ref(), &model)?;
                let model_name = string_metadata(&model.metadata, "name")?;
                display_metadata.insert(
                    model_key(&spec),
                    ModelDisplayMetadata {
                        provider_name: provider_name.clone(),
                        model_name,
                    },
                );
                models.push(spec);
            }
        }
        if models.is_empty() {
            return Err(AiError::Configuration(format!(
                "pi models.json {label} must define at least one custom model"
            )));
        }

        let mut candidate = self.clone();
        for model in models {
            candidate.register(model);
        }
        for (key, metadata) in display_metadata {
            candidate.display_metadata.insert(key, metadata);
        }
        *self = candidate;
        Ok(())
    }

    pub fn register(&mut self, model: ModelSpec) {
        let key = model_key(&model);
        self.display_metadata.remove(&key);
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

    #[must_use]
    pub fn display_metadata(&self, provider: &str, model: &str) -> Option<&ModelDisplayMetadata> {
        self.display_metadata
            .get(&(provider.to_owned(), model.to_owned()))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelDisplayMetadata {
    pub provider_name: Option<String>,
    pub model_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelCatalog {
    models: Vec<ModelSpec>,
    #[serde(default)]
    default: Option<ModelCatalogDefault>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelCatalogDefault {
    provider: String,
    model: String,
}

#[derive(Debug, Clone, Deserialize)]
struct PiModelsConfig {
    providers: BTreeMap<String, PiProviderConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct PiProviderConfig {
    #[serde(default)]
    api: Option<String>,
    #[serde(default)]
    models: Vec<PiModelDefinition>,
    #[serde(flatten)]
    metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct PiModelDefinition {
    id: String,
    #[serde(default)]
    api: Option<String>,
    #[serde(default)]
    reasoning: Option<bool>,
    #[serde(default)]
    input: Option<Vec<String>>,
    #[serde(default, rename = "contextWindow")]
    context_window: Option<u32>,
    #[serde(flatten)]
    metadata: BTreeMap<String, Value>,
}

fn pi_model_spec(
    label: &str,
    provider: &str,
    provider_api: Option<&String>,
    model: &PiModelDefinition,
) -> Result<ModelSpec, AiError> {
    let model_id = model.id.trim().to_owned();
    if model_id.is_empty() {
        return Err(AiError::Configuration(format!(
            "pi models.json {label} provider {provider} model id must not be empty"
        )));
    }
    let api = model.api.as_ref().or(provider_api).ok_or_else(|| {
        AiError::Configuration(format!(
            "pi models.json {label} provider {provider}, model {model_id}: missing api"
        ))
    })?;
    let api = pi_api_kind(label, provider, &model_id, api)?;
    let capabilities = pi_model_capabilities(label, provider, &model_id, model)?;

    Ok(ModelSpec {
        provider: ProviderId(provider.to_owned()),
        model: model_id,
        api,
        capabilities,
    })
}

fn validate_pi_provider_metadata(
    label: &str,
    provider: &str,
    metadata: &BTreeMap<String, Value>,
) -> Result<(), AiError> {
    if let Some(field) = metadata
        .keys()
        .find(|field| !is_allowed_pi_provider_metadata(field))
    {
        return Err(AiError::Configuration(format!(
            "pi models.json {label} provider {provider}: unsupported pi models.json provider metadata {field}; configure Neo provider credentials, base URLs, headers, and compatibility explicitly in Neo config instead"
        )));
    }
    Ok(())
}

fn is_allowed_pi_provider_metadata(field: &str) -> bool {
    matches!(field, "name")
}

fn pi_api_kind(label: &str, provider: &str, model_id: &str, api: &str) -> Result<ApiKind, AiError> {
    match api {
        "openai-responses" => Ok(ApiKind::OpenAiResponses),
        "openai-completions" | "openai-compatible" => Ok(ApiKind::OpenAiCompatible),
        "anthropic-messages" => Ok(ApiKind::AnthropicMessages),
        "google-generative-ai" => Ok(ApiKind::GoogleGenerativeAi),
        "local" => Ok(ApiKind::Local),
        other => Err(AiError::Configuration(format!(
            "pi models.json {label} provider {provider}, model {model_id}: unsupported pi models.json api {other}"
        ))),
    }
}

fn pi_model_capabilities(
    label: &str,
    provider: &str,
    model_id: &str,
    model: &PiModelDefinition,
) -> Result<ModelCapabilities, AiError> {
    validate_pi_model_metadata(label, provider, model_id, &model.metadata)?;

    if model.context_window == Some(0) {
        return Err(AiError::Configuration(format!(
            "pi models.json {label} provider {provider}, model {model_id}: contextWindow must be greater than 0"
        )));
    }

    let mut images = false;
    if let Some(input) = &model.input {
        for item in input {
            match item.as_str() {
                "text" => {}
                "image" => images = true,
                other => {
                    return Err(AiError::Configuration(format!(
                        "pi models.json {label} provider {provider}, model {model_id}: unsupported input type {other}"
                    )));
                }
            }
        }
    }

    Ok(ModelCapabilities {
        streaming: true,
        tools: true,
        images,
        reasoning: model.reasoning.unwrap_or(false),
        embeddings: false,
        max_context_tokens: Some(model.context_window.unwrap_or(PI_DEFAULT_CONTEXT_WINDOW)),
    })
}

fn validate_pi_model_metadata(
    label: &str,
    provider: &str,
    model_id: &str,
    metadata: &BTreeMap<String, Value>,
) -> Result<(), AiError> {
    if let Some(field) = metadata
        .keys()
        .find(|field| !is_allowed_pi_model_metadata(field))
    {
        return Err(AiError::Configuration(format!(
            "pi models.json {label} provider {provider}, model {model_id}: unsupported pi models.json model metadata {field}; configure Neo request options or provider-specific runtime support explicitly instead"
        )));
    }
    Ok(())
}

fn is_allowed_pi_model_metadata(field: &str) -> bool {
    matches!(field, "name")
}

fn string_metadata(
    metadata: &BTreeMap<String, Value>,
    field: &str,
) -> Result<Option<String>, AiError> {
    metadata.get(field).map_or(Ok(None), |value| {
        value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .map(Some)
            .ok_or_else(|| {
                AiError::Configuration(format!(
                    "pi models.json display metadata {field} must be a non-empty string"
                ))
            })
    })
}

fn validate_catalog_model(label: &str, model: &ModelSpec) -> Result<(), AiError> {
    if model.provider.0.trim().is_empty() {
        return Err(AiError::Configuration(format!(
            "model catalog {label} provider must not be empty"
        )));
    }
    if model.model.trim().is_empty() {
        return Err(AiError::Configuration(format!(
            "model catalog {label} model must not be empty"
        )));
    }
    if model.capabilities.max_context_tokens == Some(0) {
        return Err(AiError::Configuration(format!(
            "model catalog {label} max_context_tokens must be greater than 0"
        )));
    }
    Ok(())
}

fn validate_catalog_default(label: &str, default: &ModelCatalogDefault) -> Result<(), AiError> {
    if default.provider.trim().is_empty() {
        return Err(AiError::Configuration(format!(
            "model catalog {label} default provider must not be empty"
        )));
    }
    if default.model.trim().is_empty() {
        return Err(AiError::Configuration(format!(
            "model catalog {label} default model must not be empty"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSpec {
    pub id: String,
    pub display_name: String,
    pub api: ApiKind,
    pub supported_apis: Vec<ApiKind>,
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

        if !provider.supports_api(&model.api) {
            return Err(AiError::Configuration(format!(
                "provider {} does not support model API {:?}",
                provider.id, model.api
            )));
        }

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

impl ProviderSpec {
    #[must_use]
    pub fn supports_api(&self, api: &ApiKind) -> bool {
        self.supported_apis.contains(api)
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
            &[ApiKind::OpenAiResponses, ApiKind::OpenAiChatCompletions],
            Some("https://api.openai.com/v1"),
            &["OPENAI_API_KEY"],
            &[],
        ),
        provider(
            "anthropic",
            "Anthropic",
            ApiKind::AnthropicMessages,
            &[ApiKind::AnthropicMessages],
            Some("https://api.anthropic.com/v1"),
            &["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"],
            &[],
        ),
        provider(
            "google",
            "Google Generative AI",
            ApiKind::GoogleGenerativeAi,
            &[ApiKind::GoogleGenerativeAi],
            Some("https://generativelanguage.googleapis.com/v1beta"),
            &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
            &[],
        ),
        provider(
            "openrouter",
            "OpenRouter",
            ApiKind::OpenAiCompatible,
            &[ApiKind::OpenAiCompatible, ApiKind::OpenAiChatCompletions],
            Some("https://openrouter.ai/api/v1"),
            &["OPENROUTER_API_KEY"],
            &[],
        ),
        provider(
            "amazon-bedrock",
            "Amazon Bedrock",
            ApiKind::AnthropicMessages,
            &[ApiKind::AnthropicMessages],
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
    supported_apis: &[ApiKind],
    base_url: Option<&str>,
    api_key_env_vars: &[&str],
    ambient_auth_env_vars: &[&[&str]],
) -> ProviderSpec {
    ProviderSpec {
        id: id.to_owned(),
        display_name: display_name.to_owned(),
        api,
        supported_apis: supported_apis.to_vec(),
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
