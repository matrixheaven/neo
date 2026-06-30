use serde_json::{Value, json};

use crate::config::{AppConfig, ModelConfig};

/// List only the models explicitly configured in `config.toml`.
///
/// Unlike `list_models_with_options`, built-in seeded models are excluded so
/// the output reflects exactly what the user has configured.
pub(crate) fn list_configured_models(
    config: &AppConfig,
    json_output: bool,
) -> anyhow::Result<String> {
    if config.models.is_empty() {
        return list_empty_configured_models(config, json_output);
    }

    let entries = configured_model_entries(config);
    if json_output {
        return configured_models_json(config, &entries);
    }
    Ok(configured_models_text(&entries))
}

#[derive(Debug)]
struct ConfiguredModelEntry<'a> {
    alias: &'a str,
    provider: &'a str,
    model: &'a str,
    provider_type: &'a str,
    capabilities: &'a [String],
    max_context_tokens: Option<u32>,
    display_name: Option<&'a str>,
    is_default: bool,
}

fn list_empty_configured_models(config: &AppConfig, json_output: bool) -> anyhow::Result<String> {
    if json_output {
        return Ok(serde_json::to_string_pretty(&json!({
            "models": [],
            "default_model": config.default_model,
        }))? + "\n");
    }
    Ok("no models configured\n".to_owned())
}

fn configured_model_entries(config: &AppConfig) -> Vec<ConfiguredModelEntry<'_>> {
    config
        .models
        .iter()
        .map(|(alias, model_cfg)| configured_model_entry(alias, model_cfg, config))
        .collect()
}

fn configured_model_entry<'a>(
    alias: &'a str,
    model_cfg: &'a ModelConfig,
    config: &'a AppConfig,
) -> ConfiguredModelEntry<'a> {
    let provider_type = config
        .providers
        .get(&model_cfg.provider)
        .and_then(|cfg| cfg.provider_type)
        .map_or("unknown", |t| t.as_config_str());
    ConfiguredModelEntry {
        alias,
        provider: &model_cfg.provider,
        model: &model_cfg.model,
        provider_type,
        capabilities: &model_cfg.capabilities,
        max_context_tokens: model_cfg.max_context_tokens,
        display_name: model_cfg.display_name.as_deref(),
        is_default: configured_model_is_default(alias, model_cfg, config),
    }
}

fn configured_model_is_default(alias: &str, model_cfg: &ModelConfig, config: &AppConfig) -> bool {
    super::runtime::model_config_matches_default(alias, model_cfg, config)
}

fn configured_models_json(
    config: &AppConfig,
    entries: &[ConfiguredModelEntry<'_>],
) -> anyhow::Result<String> {
    let models_json: Vec<_> = entries.iter().map(configured_model_json).collect();
    Ok(serde_json::to_string_pretty(&json!({
        "models": models_json,
        "default_model": config.default_model,
    }))? + "\n")
}

fn configured_model_json(entry: &ConfiguredModelEntry<'_>) -> Value {
    json!({
        "alias": entry.alias,
        "provider": entry.provider,
        "model": entry.model,
        "type": entry.provider_type,
        "capabilities": entry.capabilities,
        "max_context_tokens": entry.max_context_tokens,
        "display_name": entry.display_name,
        "default": entry.is_default,
    })
}

fn configured_models_text(entries: &[ConfiguredModelEntry<'_>]) -> String {
    let mut out = "models:\n".to_owned();
    for entry in entries {
        out.push_str(&configured_model_text(entry));
    }
    out
}

fn configured_model_text(entry: &ConfiguredModelEntry<'_>) -> String {
    let default_marker = if entry.is_default { " default" } else { "" };
    let display = entry
        .display_name
        .map(|display_name| format!(" - {display_name}"))
        .unwrap_or_default();
    let caps = entry.capabilities.join(",");
    let ctx = entry
        .max_context_tokens
        .map_or("?".to_owned(), |tokens| tokens.to_string());
    let alias_label = configured_model_alias_label(entry);
    format!(
        "- {alias_label} ({ptype}{default_marker}) ctx={ctx} [{caps}]{display}\n",
        ptype = entry.provider_type,
    )
}

fn configured_model_alias_label(entry: &ConfiguredModelEntry<'_>) -> String {
    if entry.alias.contains('/') {
        entry.alias.to_owned()
    } else {
        format!("{} -> {}/{}", entry.alias, entry.provider, entry.model)
    }
}
