use std::sync::Arc;

use std::collections::BTreeMap;

use anyhow::Context;
use neo_ai::{ModelClient, ModelRegistry, ModelSpec, ProviderId, ProviderRegistry, ProviderSpec};

use crate::config::{self, AppConfig, ModelConfig};

pub(crate) fn resolve_model(config: &AppConfig) -> anyhow::Result<ModelSpec> {
    let registry = model_registry_for_config(config)?;
    select_config_model(&registry, config)
}

pub(crate) fn model_registry_for_config(config: &AppConfig) -> anyhow::Result<ModelRegistry> {
    let mut registry = ModelRegistry::seeded();

    for (alias, model_cfg) in &config.models {
        let spec = model_config_to_spec(alias, model_cfg, &config.providers)?;
        registry.register(spec);
    }

    Ok(registry)
}

pub(crate) fn select_config_model(
    registry: &ModelRegistry,
    config: &AppConfig,
) -> anyhow::Result<ModelSpec> {
    let models = registry.list();
    let candidates = config::scoped_models(models.iter(), &config.model_scope);
    if !config.model_scope.is_empty() && candidates.is_empty() {
        anyhow::bail!(
            "no models match model_scope {}; run `neo models list` for supported catalog entries",
            config.model_scope.join(",")
        );
    }
    let default = find_default_model(&models, config);
    if config.model_scope.is_empty() {
        return default.cloned().with_context(|| {
            format!(
                "unknown model {}; run `neo models list` for supported catalog entries",
                config.default_model_label()
            )
        });
    }

    candidates
        .iter()
        .find(|model| model_spec_matches_default(model, config))
        .or_else(|| candidates.first())
        .cloned()
        .with_context(|| {
            format!(
                "unknown model {}; run `neo models list` for supported catalog entries",
                config.default_model_label()
            )
        })
}

fn find_default_model<'a>(models: &'a [ModelSpec], config: &AppConfig) -> Option<&'a ModelSpec> {
    if let Some(model_cfg) = config.models.get(&config.default_model) {
        return models.iter().find(|model| {
            model.provider.0 == model_cfg.provider && model.model == model_cfg.model
        });
    }
    models
        .iter()
        .find(|model| model_spec_matches_default(model, config))
}

fn model_spec_matches_default(model: &ModelSpec, config: &AppConfig) -> bool {
    let qualified = format!("{}/{}", model.provider.0, model.model);
    qualified == config.default_model
        || (model.provider.0 == config.default_provider && model.model == config.default_model)
}

pub(crate) fn model_config_matches_default(
    alias: &str,
    model_cfg: &ModelConfig,
    config: &AppConfig,
) -> bool {
    alias == config.default_model
        || (model_cfg.provider == config.default_provider
            && model_cfg.model == config.default_model)
}

/// Convert a `[models.<alias>]` config entry into a `ModelSpec`.
fn model_config_to_spec(
    alias: &str,
    cfg: &crate::config::ModelConfig,
    providers: &BTreeMap<String, crate::config::ProviderConfig>,
) -> anyhow::Result<ModelSpec> {
    let provider_cfg = providers.get(&cfg.provider).ok_or_else(|| {
        anyhow::anyhow!(
            "model '{}' references unknown provider '{}'; define it in config.toml with [providers.{}]",
            alias,
            cfg.provider,
            cfg.provider
        )
    })?;

    let api = provider_cfg
        .provider_type
        .with_context(|| format!("provider '{}' must declare `type`", cfg.provider))?
        .to_api_kind();

    // Parse capabilities from string list
    let capabilities = parse_model_capabilities(
        &cfg.capabilities,
        cfg.reasoning.clone(),
        cfg.max_context_tokens,
        cfg.max_output_tokens,
    );

    Ok(ModelSpec {
        provider: ProviderId(cfg.provider.clone()),
        model: cfg.model.clone(),
        api,
        capabilities,
    })
}

/// Parse a capability string list into `ModelCapabilities`.
fn parse_model_capabilities(
    caps: &[String],
    reasoning: neo_ai::ReasoningCapability,
    max_context_tokens: Option<u32>,
    max_output_tokens: Option<u32>,
) -> neo_ai::ModelCapabilities {
    let mut mc = neo_ai::ModelCapabilities::tool_chat();
    mc.streaming = false;
    mc.tools = false;
    mc.images = false;
    mc.reasoning = reasoning;
    mc.embeddings = false;
    for cap in caps {
        match cap.trim().to_ascii_lowercase().as_str() {
            "streaming" => mc.streaming = true,
            "tools" | "tool_use" => mc.tools = true,
            "images" | "image_in" | "vision" => mc.images = true,
            "videos" | "video_in" | "video" => mc.videos = true,
            "reasoning" | "thinking" if !mc.reasoning.supports_reasoning() => {
                mc.reasoning = neo_ai::ReasoningCapability::Toggle {
                    disable_supported: true,
                };
            }
            "embeddings" | "embedding" => mc.embeddings = true,
            _ => {}
        }
    }
    mc.max_context_tokens = max_context_tokens;
    mc.max_output_tokens = max_output_tokens;
    mc
}

pub(crate) fn resolve_model_client(
    config: &AppConfig,
    model: &ModelSpec,
) -> anyhow::Result<Arc<dyn ModelClient>> {
    provider_registry_for_config(config)
        .resolver()
        .resolve(model)
        .map_err(anyhow::Error::from)
}

fn provider_registry_for_config(config: &AppConfig) -> ProviderRegistry {
    let mut registry = ProviderRegistry::production();
    apply_configured_provider_overrides(&mut registry, config);
    registry
}

fn apply_configured_provider_overrides(registry: &mut ProviderRegistry, config: &AppConfig) {
    for (provider_id, provider_config) in &config.providers {
        let existing = registry.get(provider_id).cloned();
        let provider = if let Some(mut p) = existing {
            // Override existing built-in provider fields
            if let Some(display_name) = &provider_config.display_name {
                p.display_name.clone_from(display_name);
            }
            if let Some(t) = &provider_config.provider_type {
                p.provider_type = *t;
            }
            if let Some(base_url) = &provider_config.base_url {
                p.base_url = Some(base_url.clone());
            }
            if let Some(key) = &provider_config.api_key {
                p.api_key = Some(key.clone());
            }
            if let Some(env_name) = &provider_config.api_key_env {
                p.api_key_env_vars = vec![env_name.clone()];
            }
            p
        } else {
            let provider_type = provider_config.provider_type;
            let Some(provider_type) = provider_type else {
                tracing::warn!("ignoring provider {provider_id}: missing required `type`");
                continue;
            };
            ProviderSpec {
                id: provider_id.clone(),
                display_name: provider_config
                    .display_name
                    .clone()
                    .unwrap_or_else(|| provider_id.clone()),
                base_url: provider_config.base_url.clone(),
                api_key: provider_config.api_key.clone(),
                api_key_env_vars: provider_config.api_key_env.iter().cloned().collect(),
                provider_type,
            }
        };
        registry.register(provider);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use neo_ai::{ApiKind, ModelCapabilities, ModelSpec, ProviderId};
    use tempfile::TempDir;

    use super::*;
    use crate::config::ConfigOverrides;

    fn load_config_from(content: &str) -> AppConfig {
        let temp = TempDir::new().expect("tempdir");
        let config_path = temp.path().join("config.toml");
        std::fs::write(&config_path, content).expect("write config");
        let project_dir = temp.path().join("project");
        std::fs::create_dir_all(&project_dir).expect("create project");
        AppConfig::load(ConfigOverrides {
            config_path: Some(config_path),
            yolo: false,
            auto: false,
            trust_store: None,
            project_dir: Some(project_dir),
        })
        .expect("load config")
    }

    #[test]
    fn selected_provider_never_inherits_another_provider_credentials() {
        let config = load_config_from(
            r#"
default_model = "openai/gpt"
default_provider = "openai"

[providers.openai]
type = "openai_response"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"

[providers.anthropic]
type = "anthropic"
base_url = "https://api.anthropic.com/v1"
api_key_env = "ANTHROPIC_API_KEY"

[models."openai/gpt"]
provider = "openai"
model = "gpt"

[models."anthropic/claude"]
provider = "anthropic"
model = "claude"
"#,
        );

        let registry = provider_registry_for_config(&config);
        let env = BTreeMap::from([("OPENAI_API_KEY".to_owned(), "openai-secret".to_owned())]);
        let resolver = registry.resolver_from(env);

        let openai_model = ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: "gpt".to_owned(),
            api: ApiKind::OpenAiResponse,
            capabilities: ModelCapabilities::tool_chat(),
        };
        assert!(
            resolver.resolve(&openai_model).is_ok(),
            "openai resolves with its own key"
        );

        let anthropic_model = ModelSpec {
            provider: ProviderId("anthropic".to_owned()),
            model: "claude".to_owned(),
            api: ApiKind::AnthropicMessages,
            capabilities: ModelCapabilities::tool_chat(),
        };
        assert!(
            resolver.resolve(&anthropic_model).is_err(),
            "anthropic does not inherit openai's key"
        );
    }

    fn parsed_capabilities(caps: &[&str]) -> neo_ai::ModelCapabilities {
        parse_model_capabilities(
            &caps.iter().map(|c| (*c).to_owned()).collect::<Vec<_>>(),
            neo_ai::ReasoningCapability::None,
            None,
            None,
        )
    }

    #[test]
    fn unknown_capabilities_default_to_all_media_off() {
        let mc = parsed_capabilities(&["streaming", "tools"]);
        assert!(mc.streaming && mc.tools, "known flags still parse");
        assert!(!mc.images && !mc.videos, "undeclared media stays off");
    }

    #[test]
    fn image_capabilities_parse_without_enabling_video() {
        let mc = parsed_capabilities(&["images", "image_in", "vision"]);
        assert!(mc.images);
        assert!(!mc.videos, "image strings never enable video");
    }

    #[test]
    fn video_capabilities_parse_without_enabling_image() {
        let mc = parsed_capabilities(&["videos", "video_in", "video"]);
        assert!(mc.videos);
        assert!(!mc.images, "video strings never enable image");
    }

    #[test]
    fn dual_media_capabilities_parse_together() {
        let mc = parsed_capabilities(&["images", "videos"]);
        assert!(mc.images && mc.videos);
    }
}
