//! Config mutation operations for provider/model/MCP management.
//!
//! All functions read → modify → write `config.toml` atomically.

use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::Context;
use serde_json::json;

use super::{
    AppConfig, FileConfig, McpConfig, McpServerConfig, ModelConfig, ProviderConfig,
    read_file_config, write_file_config,
};

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
    remove_provider_config(&mut file_config, provider_id);
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
    let count = provider_config.models.len();

    let mut file_config = read_file_config(config_path)?;
    replace_provider_from_catalog(
        &mut file_config,
        provider_id,
        &provider_config,
        api_key,
        default_model,
    );

    write_file_config(config_path, &file_config)?;

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
    if config.providers.is_empty() {
        if json {
            return providers_json(&[], &config.default_model);
        }
        return Ok(
            "no providers configured. Use `neo provider catalog list` to discover providers.\n"
                .to_owned(),
        );
    }

    if json {
        let entries = config
            .providers
            .iter()
            .map(|(id, cfg)| provider_entry_json(id, cfg, config))
            .collect::<Vec<_>>();
        return providers_json(&entries, &config.default_model);
    }

    let mut out = String::new();
    for (id, cfg) in &config.providers {
        let _ = writeln!(out, "{}", provider_entry_text(id, cfg, config));
    }
    if !config.default_model.is_empty() {
        let _ = writeln!(out, "\nDefault model: {}", config.default_model);
    }
    Ok(out)
}

pub fn upsert_mcp_server(server: &McpServerConfig, config_path: &Path) -> anyhow::Result<String> {
    crate::mcp_ops::validate_mcp_server_config(server)?;
    let mut config = read_file_config(config_path)?;
    let mcp = config.mcp.get_or_insert_with(McpConfig::default);
    if let Some(existing) = mcp
        .servers
        .iter_mut()
        .find(|existing| existing.id == server.id)
    {
        *existing = server.clone();
    } else {
        mcp.servers.push(server.clone());
    }
    write_file_config(config_path, &config)?;
    Ok(format!("added MCP server {}\n", server.id))
}

pub fn remove_mcp_server(server_id: &str, config_path: &Path) -> anyhow::Result<String> {
    let mut config = read_file_config(config_path)?;
    let Some(mcp) = config.mcp.as_mut() else {
        anyhow::bail!("MCP server {server_id} is not configured");
    };
    let original_len = mcp.servers.len();
    mcp.servers.retain(|server| server.id != server_id);
    anyhow::ensure!(
        mcp.servers.len() != original_len,
        "MCP server {server_id} is not configured"
    );
    write_file_config(config_path, &config)?;
    Ok(format!("removed MCP server {server_id}\n"))
}

pub fn set_mcp_server_enabled(
    server_id: &str,
    enabled: bool,
    config_path: &Path,
) -> anyhow::Result<String> {
    let mut config = read_file_config(config_path)?;
    let Some(server) = config
        .mcp
        .as_mut()
        .and_then(|mcp| mcp.servers.iter_mut().find(|server| server.id == server_id))
    else {
        anyhow::bail!("MCP server {server_id} is not configured");
    };
    server.enabled = enabled;
    write_file_config(config_path, &config)?;
    let action = if enabled { "enabled" } else { "disabled" };
    Ok(format!("{action} MCP server {server_id}\n"))
}

fn remove_provider_config(file_config: &mut FileConfig, provider_id: &str) {
    clear_provider_default(file_config, provider_id);
    remove_provider_entry(file_config.providers.as_mut(), provider_id);
    remove_provider_models(file_config.models.as_mut(), provider_id);
}

fn remove_provider_entry(
    providers: Option<&mut BTreeMap<String, ProviderConfig>>,
    provider_id: &str,
) {
    if let Some(providers) = providers {
        providers.remove(provider_id);
    }
}

fn remove_provider_models(models: Option<&mut BTreeMap<String, ModelConfig>>, provider_id: &str) {
    if let Some(models) = models {
        models.retain(|_, model| model.provider != provider_id);
    }
}

fn clear_provider_default(file_config: &mut FileConfig, provider_id: &str) {
    let Some(default_model) = file_config.default_model.as_deref() else {
        return;
    };
    if file_config
        .models
        .as_ref()
        .and_then(|models| models.get(default_model))
        .is_some_and(|model| model.provider == provider_id)
        || provider_owns_default(provider_id, default_model)
    {
        file_config.default_model = None;
    }
}

fn provider_owns_default(provider_id: &str, default_model: &str) -> bool {
    default_model == provider_id || default_model.starts_with(&format!("{provider_id}/"))
}

fn replace_provider_from_catalog(
    file_config: &mut FileConfig,
    provider_id: &str,
    provider_config: &neo_ai::catalog::CatalogProviderConfig,
    api_key: Option<&str>,
    default_model: Option<&str>,
) {
    remove_provider_config(file_config, provider_id);
    insert_catalog_provider(file_config, provider_id, provider_config, api_key);
    insert_catalog_models(file_config, provider_id, provider_config);
    if let Some(default_alias) = catalog_default_alias(provider_id, provider_config, default_model)
    {
        file_config.default_model = Some(default_alias);
        // Keep default_provider in sync with the new default model alias
        // (which is `<provider_id>/<model>`). Otherwise the label formatter
        // (`{default_provider}/{default_model}`) would stitch the stale provider
        // onto the new alias, producing e.g. `deepseek/minimax-.../MiniMax-M2`.
        file_config.default_provider = Some(provider_id.to_owned());
    }
}

fn insert_catalog_provider(
    file_config: &mut FileConfig,
    provider_id: &str,
    provider_config: &neo_ai::catalog::CatalogProviderConfig,
    api_key: Option<&str>,
) {
    let providers = file_config.providers.get_or_insert_with(BTreeMap::new);
    providers.insert(
        provider_id.to_owned(),
        ProviderConfig {
            provider_type: Some(provider_config.provider_type),
            base_url: provider_config.base_url.clone(),
            api_key: api_key.map(str::to_owned),
            api_key_env: provider_config.api_key_env.clone(),
        },
    );
}

fn insert_catalog_models(
    file_config: &mut FileConfig,
    provider_id: &str,
    provider_config: &neo_ai::catalog::CatalogProviderConfig,
) {
    let models = file_config.models.get_or_insert_with(BTreeMap::new);
    for model_info in &provider_config.models {
        models.insert(
            catalog_model_alias(provider_id, &model_info.id),
            catalog_model_config(provider_id, model_info),
        );
    }
}

fn catalog_model_config(
    provider_id: &str,
    model_info: &neo_ai::catalog::CatalogModelInfo,
) -> ModelConfig {
    ModelConfig {
        provider: provider_id.to_owned(),
        model: model_info.id.clone(),
        max_context_tokens: model_info.max_context_tokens,
        max_output_tokens: model_info.max_output_tokens,
        capabilities: model_info.capabilities.clone(),
        display_name: model_info.name.clone(),
    }
}

fn catalog_default_alias(
    provider_id: &str,
    provider_config: &neo_ai::catalog::CatalogProviderConfig,
    default_model: Option<&str>,
) -> Option<String> {
    default_model
        .and_then(|model_id| {
            catalog_alias_for_model(provider_id, &provider_config.models, model_id)
        })
        .or_else(|| {
            provider_config
                .models
                .first()
                .map(|model| catalog_model_alias(provider_id, &model.id))
        })
}

fn catalog_alias_for_model(
    provider_id: &str,
    models: &[neo_ai::catalog::CatalogModelInfo],
    model_id: &str,
) -> Option<String> {
    models
        .iter()
        .find(|model| model.id == model_id)
        .map(|model| catalog_model_alias(provider_id, &model.id))
}

fn catalog_model_alias(provider_id: &str, model_id: &str) -> String {
    format!("{provider_id}/{model_id}")
}

fn providers_json(entries: &[serde_json::Value], default_model: &str) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(&json!({
        "providers": entries,
        "default_model": default_model,
    }))? + "\n")
}

fn provider_entry_json(id: &str, cfg: &ProviderConfig, config: &AppConfig) -> serde_json::Value {
    json!({
        "id": id,
        "type": provider_type_label(cfg),
        "base_url": provider_base_url_label(cfg),
        "credential": provider_credential_label(cfg),
        "model_count": provider_model_count(config, id),
        "default": provider_is_current(config, id),
    })
}

fn provider_entry_text(id: &str, cfg: &ProviderConfig, config: &AppConfig) -> String {
    format!(
        "{id:<20} type={ptype:<18} base_url={base:<45} models={model_count:<3} cred={cred}{current}",
        ptype = provider_type_label(cfg),
        base = provider_base_url_label(cfg),
        model_count = provider_model_count(config, id),
        cred = provider_credential_label(cfg),
        current = provider_current_marker(config, id)
    )
}

fn provider_type_label(cfg: &ProviderConfig) -> &str {
    cfg.provider_type
        .as_ref()
        .map_or("unknown", |provider_type| provider_type.as_config_str())
}

fn provider_base_url_label(cfg: &ProviderConfig) -> &str {
    cfg.base_url.as_deref().unwrap_or("(none)")
}

fn provider_model_count(config: &AppConfig, provider_id: &str) -> usize {
    config
        .models
        .values()
        .filter(|model| model.provider == provider_id)
        .count()
}

fn provider_current_marker(config: &AppConfig, provider_id: &str) -> &'static str {
    if provider_is_current(config, provider_id) {
        " ← current"
    } else {
        ""
    }
}

fn provider_is_current(config: &AppConfig, provider_id: &str) -> bool {
    config
        .models
        .get(&config.default_model)
        .is_some_and(|model| model.provider == provider_id)
        || provider_owns_default(provider_id, &config.default_model)
        || (config.default_provider == provider_id && !config.default_model.contains('/'))
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

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs};

    use neo_ai::{ApiType, catalog};
    use tempfile::TempDir;

    use super::{add_provider_from_catalog_entry, list_providers, remove_provider};
    use crate::config::{
        AppConfig, Defaults, McpConfig, ModelConfig, ProviderConfig, RuntimeConfig, TuiConfig,
    };

    #[test]
    fn list_providers_formats_text_and_json_entries() {
        let temp = TempDir::new().expect("temp dir");
        let mut config = test_config(temp.path());
        config.default_model = "openai/gpt-4.1".to_owned();
        config.providers.insert(
            "openai".to_owned(),
            ProviderConfig {
                provider_type: Some(ApiType::OpenAiResponses),
                base_url: Some("https://api.openai.test/v1".to_owned()),
                api_key: Some("secret".to_owned()),
                api_key_env: None,
            },
        );
        config.models.insert(
            "openai/gpt-4.1".to_owned(),
            ModelConfig {
                provider: "openai".to_owned(),
                model: "gpt-4.1".to_owned(),
                ..ModelConfig::default()
            },
        );

        let text = list_providers(&config, false).expect("provider text");
        assert!(text.contains("openai"));
        assert!(text.contains("type=openai-responses"));
        assert!(text.contains("models=1"));
        assert!(text.contains("cred=api_key"));
        assert!(text.contains("current"));
        assert!(text.contains("Default model: openai/gpt-4.1"));

        let json_output = list_providers(&config, true).expect("provider json");
        let value: serde_json::Value = serde_json::from_str(&json_output).expect("json");
        assert_eq!(
            value,
            serde_json::json!({
                "providers": [{
                    "id": "openai",
                    "type": "openai-responses",
                    "base_url": "https://api.openai.test/v1",
                    "credential": "api_key",
                    "model_count": 1,
                    "default": true,
                }],
                "default_model": "openai/gpt-4.1",
            })
        );
    }

    #[test]
    fn remove_provider_drops_models_and_default_for_that_provider_only() {
        let temp = TempDir::new().expect("temp dir");
        let config_path = write_project_config(
            temp.path(),
            r#"
default_model = "openai/gpt-4.1"

[providers.openai]
type = "openai-responses"
base_url = "https://api.openai.test/v1"

[providers.anthropic]
type = "anthropic"
base_url = "https://api.anthropic.test"

[models."openai/gpt-4.1"]
provider = "openai"
model = "gpt-4.1"

[models."anthropic/sonnet"]
provider = "anthropic"
model = "claude-sonnet-4"
"#,
        );

        let message = remove_provider(&config_path, "openai").expect("remove provider");
        assert_eq!(message, "removed provider 'openai' and its models\n");

        let written = fs::read_to_string(config_path).expect("read config");
        assert!(!written.contains("[providers.openai]"));
        assert!(written.contains("[providers.anthropic]"));
        assert!(!written.contains("[models.\"openai/gpt-4.1\"]"));
        assert!(written.contains("[models.\"anthropic/sonnet\"]"));
        assert!(!written.contains("default_model"));
    }

    #[test]
    fn remove_provider_clears_unqualified_default_alias_owned_by_provider() {
        let temp = TempDir::new().expect("temp dir");
        let config_path = write_project_config(
            temp.path(),
            r#"
default_model = "fast"
default_provider = "openai"

[providers.openai]
type = "openai-responses"

[models.fast]
provider = "openai"
model = "gpt-4.1"
"#,
        );

        let message = remove_provider(&config_path, "openai").expect("remove provider");
        assert_eq!(message, "removed provider 'openai' and its models\n");

        let written = fs::read_to_string(config_path).expect("read config");
        assert!(!written.contains("default_model"));
        assert!(!written.contains("[models.fast]"));
    }

    #[test]
    fn add_provider_from_catalog_entry_replaces_existing_provider_models() {
        let temp = TempDir::new().expect("temp dir");
        let config_path = write_project_header(
            temp.path(),
            r#"
default_model = "openai/old"

[providers.openai]
type = "openai-chat"
base_url = "https://old.example/v1"

[models."openai/old"]
provider = "openai"
model = "old"

[models."other/stays"]
provider = "other"
model = "stays"
"#,
        );
        let entry = catalog_entry();

        let message = add_provider_from_catalog_entry(
            &config_path,
            "openai",
            &entry,
            Some("inline-key"),
            Some("gpt-large"),
        )
        .expect("import provider");

        assert_eq!(
            message,
            "imported provider 'openai' with 2 models from models.dev\n"
        );
        let written = fs::read_to_string(config_path).expect("read config");
        assert!(written.contains("default_model = \"openai/gpt-large\""));
        assert!(written.contains("default_provider = \"openai\""));
        assert!(written.contains("[providers.openai]"));
        assert!(written.contains("type = \"openai-responses\""));
        assert!(written.contains("api_key = \"inline-key\""));
        assert!(written.contains("[models.\"openai/gpt-small\"]"));
        assert!(written.contains("[models.\"openai/gpt-large\"]"));
        assert!(written.contains("[models.\"other/stays\"]"));
        assert!(!written.contains("[models.\"openai/old\"]"));
        assert!(!written.contains("OPENAI_API_KEY"));
    }

    /// Regression: importing a *new* provider must update `default_provider` to
    /// match the new default model alias (`<provider>/<model>`). Otherwise the
    /// model label formatter (`{default_provider}/{default_model}`) stitches the
    /// stale provider onto the new alias, producing e.g.
    /// `deepseek/minimax-.../MiniMax-M2`.
    #[test]
    fn add_provider_syncs_default_provider_to_new_provider() {
        let temp = TempDir::new().expect("temp dir");
        let config_path = write_project_header(
            temp.path(),
            r#"
default_model = "deepseek/old"
default_provider = "deepseek"

[providers.deepseek]
type = "openai-chat"
base_url = "https://deepseek.example/v1"

[models."deepseek/old"]
provider = "deepseek"
model = "old"
"#,
        );
        let entry = catalog_entry();

        let message = add_provider_from_catalog_entry(
            &config_path,
            "openai",
            &entry,
            Some("inline-key"),
            Some("gpt-large"),
        )
        .expect("import provider");

        assert_eq!(
            message,
            "imported provider 'openai' with 2 models from models.dev\n"
        );
        let written = fs::read_to_string(config_path).expect("read config");
        // The new provider's default alias and provider must be consistent so
        // the label is `openai/gpt-large`, not `deepseek/openai/gpt-large`.
        assert!(written.contains("default_model = \"openai/gpt-large\""));
        assert!(written.contains("default_provider = \"openai\""));
        assert!(!written.contains("default_provider = \"deepseek\""));
    }

    fn write_project_header(project_dir: &std::path::Path, content: &str) -> std::path::PathBuf {
        let config_dir = project_dir.join(".neo");
        fs::create_dir_all(&config_dir).expect("create .neo");
        let config_path = config_dir.join("config.toml");
        fs::write(&config_path, content).expect("write config");
        config_path
    }

    /// Original test helper name preserved for the catalog tests that were
    /// migrated from `config_ops.rs`.
    fn write_project_config(project_dir: &std::path::Path, content: &str) -> std::path::PathBuf {
        write_project_header(project_dir, content)
    }

    fn catalog_entry() -> catalog::CatalogEntry {
        catalog::CatalogEntry {
            id: "openai".to_owned(),
            name: Some("OpenAI".to_owned()),
            api: Some("https://api.openai.test/v1".to_owned()),
            env: vec!["OPENAI_API_KEY".to_owned()],
            npm: None,
            explicit_type: Some("openai-responses".to_owned()),
            models: BTreeMap::from([
                (
                    "gpt-small".to_owned(),
                    catalog::CatalogModel {
                        id: "gpt-small".to_owned(),
                        name: Some("GPT Small".to_owned()),
                        family: None,
                        limit: Some(catalog::CatalogLimit {
                            context: Some(128_000),
                            output: Some(16_000),
                        }),
                        tool_call: Some(true),
                        reasoning: Some(false),
                        interleaved: None,
                        modalities: None,
                    },
                ),
                (
                    "gpt-large".to_owned(),
                    catalog::CatalogModel {
                        id: "gpt-large".to_owned(),
                        name: Some("GPT Large".to_owned()),
                        family: None,
                        limit: Some(catalog::CatalogLimit {
                            context: Some(1_000_000),
                            output: Some(32_000),
                        }),
                        tool_call: Some(true),
                        reasoning: Some(true),
                        interleaved: None,
                        modalities: Some(catalog::CatalogModalities {
                            input: vec!["text".to_owned(), "image".to_owned()],
                            output: vec!["text".to_owned()],
                        }),
                    },
                ),
            ]),
        }
    }

    fn test_config(project_dir: &std::path::Path) -> AppConfig {
        AppConfig {
            default_model: "test-model".to_owned(),
            default_provider: "openai".to_owned(),
            api_key_env: None,
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
            model_scope: Vec::new(),
            sessions_dir: project_dir.join(".neo/sessions"),
            permission_mode: neo_agent_core::PermissionMode::default(),
            live_permission_mode: std::sync::Arc::new(std::sync::RwLock::new(
                neo_agent_core::PermissionMode::default(),
            )),
            defaults: Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: RuntimeConfig::default(),
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: McpConfig::default(),
            prompt_templates: Vec::new(),
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: project_dir.to_path_buf(),
            config_path: project_dir.join(".neo/config.toml"),
        }
    }
}
