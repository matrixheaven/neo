use std::{collections::BTreeMap, env, sync::Arc};

use anyhow::Context;
use neo_ai::{
    CredentialResolver, ModelClient, ModelRegistry, ModelSpec, ProviderId, ProviderRegistry,
    ProviderSpec, ResolvedCredential,
};

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
    const RESOLVED_API_KEY_ENV: &str = "__NEO_RESOLVED_API_KEY";
    let mut registry = provider_registry_for_config(config);
    if let Some(mut provider) = provider_with_invocation_overrides(config, &model.provider.0) {
        let credential = resolve_provider_credential(&provider);
        let mut env = env::vars().collect::<BTreeMap<_, _>>();
        if let Some(credential) = credential {
            provider.api_key_env_vars = vec![RESOLVED_API_KEY_ENV.to_owned()];
            env.insert(
                RESOLVED_API_KEY_ENV.to_owned(),
                credential.secret().to_owned(),
            );
        }
        registry.register(provider);
        return registry
            .resolver_from(env)
            .resolve(model)
            .map_err(anyhow::Error::from);
    }
    registry
        .resolver()
        .resolve(model)
        .map_err(anyhow::Error::from)
}

fn provider_registry_for_config(config: &AppConfig) -> ProviderRegistry {
    let mut registry = ProviderRegistry::production();
    apply_configured_provider_overrides(&mut registry, config);
    if let Some(env_name) = &config.api_key_env
        && let Some(mut provider) = registry.get(&config.default_provider).cloned()
    {
        provider.api_key_env_vars = vec![env_name.clone()];
        registry.register(provider);
    }
    registry
}

fn provider_with_invocation_overrides(
    config: &AppConfig,
    provider_id: &str,
) -> Option<ProviderSpec> {
    let registry = provider_registry_for_config(config);
    let mut provider = registry.get(provider_id).cloned()?;
    if let Some(env_name) = &config.api_key_env {
        provider.api_key_env_vars = vec![env_name.clone()];
    }
    Some(provider)
}

fn resolve_provider_credential(provider: &ProviderSpec) -> Option<ResolvedCredential> {
    resolve_provider_credential_from_env(provider, &env::vars().collect())
}

fn resolve_provider_credential_from_env(
    provider: &ProviderSpec,
    env: &BTreeMap<String, String>,
) -> Option<ResolvedCredential> {
    CredentialResolver::new(&provider.id)
        .with_env(provider.api_key_env_vars.iter().map(String::as_str), env)
        .with_auth_file_credentials(BTreeMap::new())
        .resolve()
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
                p.provider_type = Some(*t);
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
            let default_api = provider_type.to_api_kind();
            ProviderSpec {
                id: provider_id.clone(),
                display_name: provider_config
                    .display_name
                    .clone()
                    .unwrap_or_else(|| provider_id.clone()),
                api: default_api,
                supported_apis: vec![default_api],
                base_url: provider_config.base_url.clone(),
                api_key: provider_config.api_key.clone(),
                api_key_env_vars: provider_config.api_key_env.iter().cloned().collect(),
                ambient_auth_env_vars: vec![],
                provider_type: Some(provider_type),
            }
        };
        registry.register(provider);
    }
}
