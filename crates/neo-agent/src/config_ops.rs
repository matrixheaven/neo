//! Config mutation operations for provider/model management.
//!
//! All functions read → modify → write `config.toml` atomically.

use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::Context;
use serde_json::json;

use crate::config::{AppConfig, ModelConfig, ProviderConfig, read_file_config, write_file_config};

/// Add or replace a provider in config.toml.
pub fn add_provider(
    config_path: &Path,
    provider_id: &str,
    cfg: ProviderConfig,
) -> anyhow::Result<String> {
    let mut file_config = read_file_config(config_path)?;
    let providers = file_config.providers.get_or_insert_with(BTreeMap::new);
    providers.insert(provider_id.to_owned(), cfg);
    write_file_config(config_path, &file_config)?;
    Ok(format!("added provider '{provider_id}'\n"))
}

/// Remove a provider and all its models from config.toml.
pub fn remove_provider(config_path: &Path, provider_id: &str) -> anyhow::Result<String> {
    let mut file_config = read_file_config(config_path)?;

    // Delete the provider entry
    if let Some(providers) = &mut file_config.providers {
        providers.remove(provider_id);
    }

    // Delete all model aliases that reference this provider
    if let Some(models) = &mut file_config.models {
        models.retain(|_, m| m.provider != provider_id);
    }

    // Clear default_model if it pointed at a deleted model
    if let Some(default) = &file_config.default_model {
        let prefix = format!("{provider_id}/");
        if default.starts_with(&prefix) || default == provider_id {
            file_config.default_model = None;
        }
    }

    write_file_config(config_path, &file_config)?;
    Ok(format!("removed provider '{provider_id}' and its models\n"))
}

/// Add or replace a model in config.toml.
pub fn add_model(config_path: &Path, alias: &str, cfg: ModelConfig) -> anyhow::Result<String> {
    let mut file_config = read_file_config(config_path)?;
    let models = file_config.models.get_or_insert_with(BTreeMap::new);
    models.insert(alias.to_owned(), cfg);
    write_file_config(config_path, &file_config)?;
    Ok(format!("added model '{alias}'\n"))
}

/// Remove a model from config.toml.
pub fn remove_model(config_path: &Path, alias: &str) -> anyhow::Result<String> {
    let mut file_config = read_file_config(config_path)?;
    if let Some(models) = &mut file_config.models {
        models.remove(alias);
    }
    // Clear default if it was this alias
    if file_config.default_model.as_deref() == Some(alias) {
        file_config.default_model = None;
    }
    write_file_config(config_path, &file_config)?;
    Ok(format!("removed model '{alias}'\n"))
}

/// Set the default model alias.
pub fn set_default_model(config_path: &Path, alias: &str) -> anyhow::Result<String> {
    let mut file_config = read_file_config(config_path)?;
    file_config.default_model = Some(alias.to_owned());
    write_file_config(config_path, &file_config)?;
    Ok(format!("default model set to '{alias}'\n"))
}

/// Import a provider from the models.dev catalog into config.toml.
///
/// Fetches the catalog, finds the provider, infers its type, and writes
/// the provider + all its models to config.toml.
pub async fn catalog_add_provider(
    config_path: &Path,
    provider_id: &str,
    api_key: Option<&str>,
    default_model: Option<&str>,
) -> anyhow::Result<String> {
    let catalog = neo_ai::catalog::fetch_catalog()
        .await
        .context("failed to fetch models.dev catalog")?;

    let entry = catalog.get(provider_id).ok_or_else(|| {
        anyhow::anyhow!("provider '{provider_id}' not found in models.dev catalog")
    })?;

    add_provider_from_catalog_entry(config_path, provider_id, entry, api_key, default_model)
}

/// Write a provider from an already-fetched catalog entry into config.toml.
pub fn add_provider_from_catalog_entry(
    config_path: &Path,
    provider_id: &str,
    entry: &neo_ai::catalog::CatalogEntry,
    api_key: Option<&str>,
    default_model: Option<&str>,
) -> anyhow::Result<String> {
    let provider_config =
        neo_ai::catalog::catalog_to_provider_config(entry, api_key).ok_or_else(|| {
            anyhow::anyhow!(
                "provider '{provider_id}' has an unsupported wire type or no usable models"
            )
        })?;

    let mut file_config = read_file_config(config_path)?;

    // Remove existing provider first (full replacement)
    if let Some(providers) = &mut file_config.providers {
        providers.remove(provider_id);
    }
    if let Some(models) = &mut file_config.models {
        let pid = provider_id.to_owned();
        models.retain(|_, m| m.provider != pid);
    }

    // Write provider
    let providers = file_config.providers.get_or_insert_with(BTreeMap::new);
    let pcfg = ProviderConfig {
        provider_type: Some(provider_config.provider_type),
        base_url: provider_config.base_url,
        api_key: api_key.map(str::to_owned),
        api_key_env: provider_config.api_key_env,
        api_base: None,
    };
    providers.insert(provider_id.to_owned(), pcfg);

    // Write models
    let models = file_config.models.get_or_insert_with(BTreeMap::new);
    let mut selected_alias = None;
    for model_info in &provider_config.models {
        let alias = format!("{provider_id}/{}", model_info.id);
        let caps = model_info.capabilities.clone();
        let mc = ModelConfig {
            provider: provider_id.to_owned(),
            model: model_info.id.clone(),
            max_context_tokens: model_info.max_context_tokens,
            max_output_tokens: model_info.max_output_tokens,
            capabilities: caps,
            display_name: model_info.name.clone(),
        };
        if let Some(dm) = default_model
            && model_info.id == dm
        {
            selected_alias = Some(alias.clone());
        }
        models.insert(alias, mc);
    }

    // Set default model
    let chosen = selected_alias.or_else(|| {
        provider_config
            .models
            .first()
            .map(|m| format!("{provider_id}/{}", m.id))
    });
    if let Some(chosen) = chosen {
        file_config.default_model = Some(chosen);
    }

    write_file_config(config_path, &file_config)?;

    let count = provider_config.models.len();
    Ok(format!(
        "imported provider '{provider_id}' with {count} model{} from models.dev\n",
        if count == 1 { "" } else { "s" }
    ))
}

/// List configured providers as a formatted string or JSON.
///
/// Uses the merged `AppConfig` so providers from both user-global and project
/// `config.toml` are visible.
pub fn list_providers(config: &AppConfig, json: bool) -> anyhow::Result<String> {
    let providers = &config.providers;
    let models = &config.models;

    if providers.is_empty() {
        if json {
            return Ok(serde_json::to_string_pretty(&json!({
                "providers": [],
                "default_model": config.default_model,
            }))? + "\n");
        }
        return Ok(
            "no providers configured. Use `neo provider catalog list` to discover providers.\n"
                .to_owned(),
        );
    }

    if json {
        let entries: Vec<_> = providers
            .iter()
            .map(|(id, cfg)| {
                let ptype = cfg
                    .provider_type
                    .as_ref()
                    .map_or("unknown", |t| t.as_config_str());
                let base = cfg.effective_base_url().unwrap_or("(none)");
                let model_count = models.values().filter(|m| m.provider == *id).count();
                let is_default = config.default_model.starts_with(&format!("{id}/"));
                json!({
                    "id": id,
                    "type": ptype,
                    "base_url": base,
                    "credential": provider_credential_label(cfg),
                    "model_count": model_count,
                    "default": is_default,
                })
            })
            .collect();
        return Ok(serde_json::to_string_pretty(&json!({
            "providers": entries,
            "default_model": config.default_model,
        }))? + "\n");
    }

    let mut out = String::new();
    for (id, cfg) in providers {
        let ptype = cfg
            .provider_type
            .as_ref()
            .map_or("unknown", |t| t.as_config_str());
        let base = cfg.effective_base_url().unwrap_or("(none)");
        let model_count = models.values().filter(|m| m.provider == *id).count();
        let current = if config.default_model.starts_with(&format!("{id}/")) {
            " ← current"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "{id:<20} type={ptype:<18} base_url={base:<45} models={model_count:<3} cred={cred}{current}",
            cred = provider_credential_label(cfg)
        );
    }
    if !config.default_model.is_empty() {
        let _ = writeln!(out, "\nDefault model: {}", config.default_model);
    }
    Ok(out)
}

fn provider_credential_label(cfg: &ProviderConfig) -> &'static str {
    if cfg.api_key.is_some() {
        "api_key"
    } else if let Some(env_name) = &cfg.api_key_env {
        if env::var(env_name).is_ok_and(|value| !value.is_empty()) {
            "configured"
        } else {
            "missing"
        }
    } else {
        "(none)"
    }
}
