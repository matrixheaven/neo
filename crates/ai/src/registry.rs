use crate::{ApiKind, ModelCapabilities, ModelSpec, ProviderId};

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

fn model_key(model: &ModelSpec) -> (String, String) {
    (model.provider.0.clone(), model.model.clone())
}

fn builtin_models() -> Vec<ModelSpec> {
    vec![
        builtin_model(
            "openai",
            "gpt-4.1",
            ApiKind::OpenAiChatCompletions,
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
