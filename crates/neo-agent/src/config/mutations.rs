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
    update_file_config,
};

/// Add or replace a provider in config.toml.
pub fn add_provider(
    config_path: &Path,
    provider_id: &str,
    cfg: ProviderConfig,
) -> anyhow::Result<String> {
    update_file_config(config_path, |file_config| {
        let providers = file_config.providers.get_or_insert_with(BTreeMap::new);
        providers.insert(provider_id.to_owned(), cfg);
        Ok(())
    })?;
    Ok(format!("added provider '{provider_id}'\n"))
}

/// Remove a provider and all its models from config.toml.
pub fn remove_provider(config_path: &Path, provider_id: &str) -> anyhow::Result<String> {
    update_file_config(config_path, |file_config| {
        remove_provider_config(file_config, provider_id);
        Ok(())
    })?;
    Ok(format!("removed provider '{provider_id}' and its models\n"))
}

/// Add or replace a model in config.toml.
pub fn add_model(config_path: &Path, alias: &str, cfg: ModelConfig) -> anyhow::Result<String> {
    update_file_config(config_path, |file_config| {
        let models = file_config.models.get_or_insert_with(BTreeMap::new);
        models.insert(alias.to_owned(), cfg);
        Ok(())
    })?;
    Ok(format!("added model '{alias}'\n"))
}

/// Remove a model from config.toml.
pub fn remove_model(config_path: &Path, alias: &str) -> anyhow::Result<String> {
    update_file_config(config_path, |file_config| {
        if let Some(models) = &mut file_config.models {
            models.remove(alias);
        }
        if file_config.default_model.as_deref() == Some(alias) {
            file_config.default_model = None;
        }
        Ok(())
    })?;
    Ok(format!("removed model '{alias}'\n"))
}

/// Set the default model alias.
pub fn set_default_model(config_path: &Path, alias: &str) -> anyhow::Result<String> {
    update_file_config(config_path, |file_config| {
        file_config.default_model = Some(alias.to_owned());
        Ok(())
    })?;
    Ok(format!("default model set to '{alias}'\n"))
}

/// Persist the model, provider, and reasoning selected in the TUI.
pub fn set_model_selection(
    config_path: &Path,
    alias: &str,
    provider_id: &str,
    reasoning: &neo_ai::ReasoningSelection,
) -> anyhow::Result<()> {
    update_file_config(config_path, |file_config| {
        file_config.default_model = Some(alias.to_owned());
        file_config.default_provider = Some(provider_id.to_owned());
        file_config.runtime.get_or_insert_default().reasoning = Some(reasoning.clone());
        Ok(())
    })
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

    update_file_config(config_path, |file_config| {
        replace_provider_from_catalog(
            file_config,
            provider_id,
            &provider_config,
            api_key,
            default_model,
        );
        Ok(())
    })?;

    Ok(format!(
        "imported provider '{provider_id}' with {count} model{} from models.dev\n",
        if count == 1 { "" } else { "s" }
    ))
}

/// Add or replace a custom endpoint provider and its reviewed model configs.
pub fn add_custom_endpoint_provider(
    config_path: &Path,
    provider_id: &str,
    provider_config: ProviderConfig,
    models: Vec<(String, ModelConfig)>,
    default_model: Option<&str>,
) -> anyhow::Result<String> {
    let count = update_file_config(config_path, |file_config| {
        let supplied_aliases = models
            .iter()
            .map(|(alias, _)| alias.as_str())
            .collect::<Vec<_>>();
        if let Some(default_alias) = default_model {
            anyhow::ensure!(
                supplied_aliases.contains(&default_alias),
                "default model '{default_alias}' is not one of the supplied model aliases"
            );
        }
        clear_provider_default(file_config, provider_id);
        let should_set_default = file_config
            .default_model
            .as_deref()
            .is_none_or(str::is_empty);
        remove_provider_entry(file_config.providers.as_mut(), provider_id);
        remove_provider_models(file_config.models.as_mut(), provider_id);

        let providers = file_config.providers.get_or_insert_with(BTreeMap::new);
        providers.insert(provider_id.to_owned(), provider_config);

        let first_alias = models.first().map(|(alias, _)| alias.clone());
        {
            let model_table = file_config.models.get_or_insert_with(BTreeMap::new);
            for (alias, model) in models {
                anyhow::ensure!(
                    model.provider == provider_id,
                    "model '{alias}' references provider '{}', expected '{provider_id}'",
                    model.provider
                );
                model_table.insert(alias, model);
            }
        }

        if let Some(default_alias) = default_model.map(str::to_owned).or({
            if should_set_default {
                first_alias
            } else {
                None
            }
        }) {
            file_config.default_model = Some(default_alias);
            file_config.default_provider = Some(provider_id.to_owned());
        }

        Ok(file_config.models.as_ref().map_or(0, |models| {
            models
                .values()
                .filter(|model| model.provider == provider_id)
                .count()
        }))
    })?;
    Ok(format!(
        "added provider '{provider_id}' with {count} model{}\n",
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
    update_file_config(config_path, |config| {
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
        Ok(())
    })?;
    Ok(format!("added MCP server {}\n", server.id))
}

pub fn remove_mcp_server(server_id: &str, config_path: &Path) -> anyhow::Result<String> {
    update_file_config(config_path, |config| {
        let Some(mcp) = config.mcp.as_mut() else {
            anyhow::bail!("MCP server {server_id} is not configured");
        };
        let original_len = mcp.servers.len();
        mcp.servers.retain(|server| server.id != server_id);
        anyhow::ensure!(
            mcp.servers.len() != original_len,
            "MCP server {server_id} is not configured"
        );
        Ok(())
    })?;
    Ok(format!("removed MCP server {server_id}\n"))
}

pub fn set_mcp_server_enabled(
    server_id: &str,
    enabled: bool,
    config_path: &Path,
) -> anyhow::Result<String> {
    update_file_config(config_path, |config| {
        let Some(server) = config
            .mcp
            .as_mut()
            .and_then(|mcp| mcp.servers.iter_mut().find(|server| server.id == server_id))
        else {
            anyhow::bail!("MCP server {server_id} is not configured");
        };
        server.enabled = enabled;
        Ok(())
    })?;
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
        file_config.default_provider = None;
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
            display_name: None,
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
        reasoning: model_info.reasoning.clone(),
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
    use std::{
        collections::BTreeMap,
        fs::{self, OpenOptions},
        io::Write as _,
        process::{Command, Stdio},
        sync::mpsc,
        time::{Duration, Instant},
    };

    use neo_ai::{ApiType, catalog};
    use tempfile::TempDir;

    use super::{add_provider, add_provider_from_catalog_entry, list_providers, remove_provider};
    use crate::config::{
        AppConfig, Defaults, McpConfig, ModelConfig, ProviderConfig, RuntimeConfig, TuiConfig,
        config_process_lock_is_available, read_file_config, update_file_config,
        update_file_config_with_lock_hook, update_file_config_with_writer,
    };

    #[test]
    fn concurrent_config_updates_preserve_both_mutations() {
        let temp = TempDir::new().expect("temp dir");
        let config_path = temp.path().join("config.toml");
        let (first_mutation_started_tx, first_mutation_started_rx) = mpsc::sync_channel(0);
        let (release_first_tx, release_first_rx) = mpsc::sync_channel(0);
        let (start_second_tx, start_second_rx) = mpsc::sync_channel(0);
        let (second_attempting_tx, second_attempting_rx) = mpsc::sync_channel(0);

        std::thread::scope(|scope| {
            let first_path = &config_path;
            scope.spawn(move || {
                update_file_config(first_path, |config| {
                    config.default_model = Some("model-a".to_owned());
                    first_mutation_started_tx.send(()).unwrap();
                    release_first_rx.recv().unwrap();
                    Ok(())
                })
                .unwrap();
            });
            let second_path = &config_path;
            scope.spawn(move || {
                start_second_rx.recv().unwrap();
                second_attempting_tx.send(()).unwrap();
                update_file_config(second_path, |config| {
                    config.default_provider = Some("provider-b".to_owned());
                    Ok(())
                })
                .unwrap();
            });

            first_mutation_started_rx.recv().unwrap();
            start_second_tx.send(()).unwrap();
            second_attempting_rx.recv().unwrap();
            assert!(!config_process_lock_is_available(&config_path).unwrap());
            release_first_tx.send(()).unwrap();
        });

        let config = read_file_config(&config_path).unwrap();
        assert_eq!(config.default_model.as_deref(), Some("model-a"));
        assert_eq!(config.default_provider.as_deref(), Some("provider-b"));
    }

    #[test]
    fn failed_atomic_replace_leaves_previous_config_parseable() {
        let temp = TempDir::new().expect("temp dir");
        let config_path = temp.path().join("config.toml");
        fs::write(&config_path, "default_model = \"original\"\n").unwrap();

        let result = update_file_config_with_writer(
            &config_path,
            |config| {
                config.default_model = Some("replacement".to_owned());
                Ok(())
            },
            |file, _content| {
                file.write_all(b"default_model = ")?;
                anyhow::bail!("injected writer failure")
            },
        );

        assert!(result.is_err());
        let config = read_file_config(&config_path).unwrap();
        assert_eq!(config.default_model.as_deref(), Some("original"));
    }

    #[test]
    fn config_lock_helper() {
        let Some(lock_path) = std::env::var_os("NEO_CONFIG_LOCK_HELPER_LOCK") else {
            return;
        };
        let ready_path = std::env::var_os("NEO_CONFIG_LOCK_HELPER_READY").unwrap();
        let release_path = std::env::var_os("NEO_CONFIG_LOCK_HELPER_RELEASE").unwrap();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .unwrap();
        lock.lock().unwrap();
        fs::write(ready_path, b"ready").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !std::path::Path::new(&release_path).exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn advisory_config_lock_blocks_external_writer() {
        let temp = TempDir::new().expect("temp dir");
        let config_path = temp.path().join("config.toml");
        fs::write(&config_path, "default_model = \"original\"\n").unwrap();
        let lock_path = temp.path().join("config.toml.lock");
        let ready_path = temp.path().join("lock-ready");
        let release_path = temp.path().join("lock-release");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "config::mutations::tests::config_lock_helper",
                "--nocapture",
            ])
            .env("NEO_CONFIG_LOCK_HELPER_LOCK", &lock_path)
            .env("NEO_CONFIG_LOCK_HELPER_READY", &ready_path)
            .env("NEO_CONFIG_LOCK_HELPER_RELEASE", &release_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready_path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !ready_path.exists() {
            fs::write(&release_path, b"release").unwrap();
            let _ = child.wait();
            panic!("lock helper did not acquire the advisory lock");
        }

        let (completed_tx, completed_rx) = mpsc::sync_channel(0);
        let (at_lock_tx, at_lock_rx) = mpsc::sync_channel(0);
        let (attempt_lock_tx, attempt_lock_rx) = mpsc::sync_channel(0);
        let blocked = std::thread::scope(|scope| {
            let update_path = &config_path;
            scope.spawn(move || {
                update_file_config_with_lock_hook(
                    update_path,
                    || {
                        at_lock_tx.send(()).unwrap();
                        attempt_lock_rx.recv().unwrap();
                    },
                    |config| {
                        config.default_provider = Some("external-waited".to_owned());
                        Ok(())
                    },
                )
                .unwrap();
                let _ = completed_tx.send(());
            });

            at_lock_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            attempt_lock_tx.send(()).unwrap();
            let blocked = completed_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err();
            fs::write(&release_path, b"release").unwrap();
            completed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            blocked
        });

        assert!(child.wait().unwrap().success());
        assert!(blocked, "config update bypassed the external advisory lock");
        let config = read_file_config(&config_path).unwrap();
        assert_eq!(config.default_provider.as_deref(), Some("external-waited"));
    }

    #[test]
    fn list_providers_formats_text_and_json_entries() {
        let temp = TempDir::new().expect("temp dir");
        let mut config = test_config(temp.path());
        config.default_model = "openai/gpt-4.1".to_owned();
        config.providers.insert(
            "openai".to_owned(),
            ProviderConfig {
                display_name: None,
                provider_type: Some(ApiType::OpenAiResponse),
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
        assert!(text.contains("type=openai_response"));
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
                    "type": "openai_response",
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
type = "openai_response"
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
    fn add_custom_endpoint_provider_writes_provider_models_and_first_default_when_empty() {
        let temp = TempDir::new().expect("temp dir");
        let config_path = temp.path().join(".neo/config.toml");

        let message = super::add_custom_endpoint_provider(
            &config_path,
            "acme",
            ProviderConfig {
                display_name: None,
                provider_type: Some(ApiType::OpenAi),
                base_url: Some("https://gateway.example.com/v1".to_owned()),
                api_key: None,
                api_key_env: Some("ACME_API_KEY".to_owned()),
            },
            vec![(
                "acme/qwen2.5-coder-32b-instruct".to_owned(),
                ModelConfig {
                    provider: "acme".to_owned(),
                    model: "qwen2.5-coder-32b-instruct".to_owned(),
                    max_context_tokens: Some(128_000),
                    max_output_tokens: Some(8_192),
                    capabilities: vec![
                        "streaming".to_owned(),
                        "tools".to_owned(),
                        "reasoning".to_owned(),
                    ],
                    reasoning: neo_ai::ReasoningCapability::Effort {
                        values: vec![
                            neo_ai::ReasoningEffort::low(),
                            neo_ai::ReasoningEffort::medium(),
                            neo_ai::ReasoningEffort::high(),
                        ],
                        disable_supported: true,
                    },
                    display_name: Some("Qwen 2.5 Coder 32B".to_owned()),
                },
            )],
            None,
        )
        .expect("add custom endpoint provider");

        assert_eq!(message, "added provider 'acme' with 1 model\n");
        let written = fs::read_to_string(config_path).expect("read config");
        assert!(written.contains("[providers.acme]"), "{written}");
        assert!(written.contains("type = \"openai\""), "{written}");
        assert!(
            written.contains("api_key_env = \"ACME_API_KEY\""),
            "{written}"
        );
        assert!(
            written.contains("[models.\"acme/qwen2.5-coder-32b-instruct\"]"),
            "{written}"
        );
        assert!(written.contains("max_context_tokens = 128000"), "{written}");
        assert!(written.contains("max_output_tokens = 8192"), "{written}");
        assert!(written.contains("type = \"effort\""), "{written}");
        assert!(
            written.contains("default_model = \"acme/qwen2.5-coder-32b-instruct\""),
            "{written}"
        );
        assert!(written.contains("default_provider = \"acme\""), "{written}");
    }

    #[test]
    fn add_custom_endpoint_provider_replaces_existing_provider_models_only() {
        let temp = TempDir::new().expect("temp dir");
        let config_path = write_project_config(
            temp.path(),
            r#"
default_model = "other/keep"

[providers.acme]
type = "openai"
base_url = "https://old.example.com/v1"

[providers.other]
type = "openai_response"
base_url = "https://api.openai.com/v1"

[models."acme/old"]
provider = "acme"
model = "old"

[models."other/keep"]
provider = "other"
model = "keep"
"#,
        );

        super::add_custom_endpoint_provider(
            &config_path,
            "acme",
            ProviderConfig {
                display_name: None,
                provider_type: Some(ApiType::Google),
                base_url: Some("https://generativelanguage.googleapis.com/v1beta".to_owned()),
                api_key: Some("local".to_owned()),
                api_key_env: None,
            },
            vec![(
                "acme/gemini-custom".to_owned(),
                ModelConfig {
                    provider: "acme".to_owned(),
                    model: "models/gemini-custom".to_owned(),
                    capabilities: vec!["streaming".to_owned()],
                    ..ModelConfig::default()
                },
            )],
            None,
        )
        .expect("replace custom endpoint provider");

        let written = fs::read_to_string(config_path).expect("read config");
        assert!(
            written.contains("default_model = \"other/keep\""),
            "{written}"
        );
        assert!(written.contains("[providers.acme]"), "{written}");
        assert!(written.contains("type = \"google\""), "{written}");
        assert!(!written.contains("[models.\"acme/old\"]"), "{written}");
        assert!(
            written.contains("[models.\"acme/gemini-custom\"]"),
            "{written}"
        );
        assert!(written.contains("[models.\"other/keep\"]"), "{written}");
    }

    #[test]
    fn add_custom_endpoint_provider_accepts_empty_models_and_does_not_invent_default() {
        let temp = TempDir::new().expect("temp dir");
        let config_path = write_project_config(
            temp.path(),
            r#"
[providers.acme]
type = "openai"
base_url = "https://old.example.com/v1"

[providers.other]
type = "openai_response"
base_url = "https://api.openai.com/v1"

[models."acme/old"]
provider = "acme"
model = "old"

[models."other/keep"]
provider = "other"
model = "keep"
"#,
        );

        let message = super::add_custom_endpoint_provider(
            &config_path,
            "acme",
            ProviderConfig {
                display_name: None,
                provider_type: Some(ApiType::Google),
                base_url: Some("https://generativelanguage.googleapis.com/v1beta".to_owned()),
                api_key: Some("local".to_owned()),
                api_key_env: None,
            },
            Vec::new(),
            None,
        )
        .expect("replace custom endpoint provider without models");

        assert_eq!(message, "added provider 'acme' with 0 models\n");
        let written = fs::read_to_string(config_path).expect("read config");
        assert!(written.contains("[providers.acme]"), "{written}");
        assert!(written.contains("type = \"google\""), "{written}");
        assert!(!written.contains("[models.\"acme/old\"]"), "{written}");
        assert!(written.contains("[models.\"other/keep\"]"), "{written}");
        assert!(!written.contains("default_model"), "{written}");
        assert!(!written.contains("default_provider"), "{written}");
    }

    #[test]
    fn add_custom_endpoint_provider_rejects_explicit_default_outside_supplied_aliases() {
        let temp = TempDir::new().expect("temp dir");
        let config_path = write_project_config(
            temp.path(),
            r#"
[providers.other]
type = "openai_response"
base_url = "https://api.openai.com/v1"

[models."other/keep"]
provider = "other"
model = "keep"
"#,
        );

        let err = super::add_custom_endpoint_provider(
            &config_path,
            "acme",
            ProviderConfig {
                display_name: None,
                provider_type: Some(ApiType::Google),
                base_url: Some("https://generativelanguage.googleapis.com/v1beta".to_owned()),
                api_key: Some("local".to_owned()),
                api_key_env: None,
            },
            vec![(
                "acme/gemini-custom".to_owned(),
                ModelConfig {
                    provider: "acme".to_owned(),
                    model: "models/gemini-custom".to_owned(),
                    capabilities: vec!["streaming".to_owned()],
                    ..ModelConfig::default()
                },
            )],
            Some("acme/missing"),
        )
        .expect_err("invalid explicit default should be rejected");

        assert!(
            err.to_string()
                .contains("default model 'acme/missing' is not one of the supplied model aliases"),
            "{err}"
        );
        let written = fs::read_to_string(config_path).expect("read config");
        assert!(!written.contains("[providers.acme]"), "{written}");
        assert!(
            !written.contains("[models.\"acme/gemini-custom\"]"),
            "{written}"
        );
        assert!(!written.contains("default_model"), "{written}");
        assert!(written.contains("[providers.other]"), "{written}");
        assert!(written.contains("[models.\"other/keep\"]"), "{written}");
    }

    #[test]
    fn add_custom_endpoint_provider_invalidated_provider_default_uses_first_alias() {
        let temp = TempDir::new().expect("temp dir");
        let config_path = write_project_config(
            temp.path(),
            r#"
default_model = "acme/old"
default_provider = "acme"

[providers.acme]
type = "openai"
base_url = "https://old.example.com/v1"

[models."acme/old"]
provider = "acme"
model = "old"
"#,
        );

        super::add_custom_endpoint_provider(
            &config_path,
            "acme",
            ProviderConfig {
                display_name: None,
                provider_type: Some(ApiType::Google),
                base_url: Some("https://generativelanguage.googleapis.com/v1beta".to_owned()),
                api_key: Some("local".to_owned()),
                api_key_env: None,
            },
            vec![(
                "acme/gemini-custom".to_owned(),
                ModelConfig {
                    provider: "acme".to_owned(),
                    model: "models/gemini-custom".to_owned(),
                    capabilities: vec!["streaming".to_owned()],
                    ..ModelConfig::default()
                },
            )],
            None,
        )
        .expect("replace custom endpoint provider");

        let written = fs::read_to_string(config_path).expect("read config");
        assert!(
            written.contains("default_model = \"acme/gemini-custom\""),
            "{written}"
        );
        assert!(written.contains("default_provider = \"acme\""), "{written}");
        assert!(!written.contains("[models.\"acme/old\"]"), "{written}");
        assert!(
            written.contains("[models.\"acme/gemini-custom\"]"),
            "{written}"
        );
    }

    #[test]
    fn first_config_write_includes_enabled_compaction_defaults() {
        let temp = TempDir::new().expect("temp dir");
        let config_path = temp.path().join(".neo/config.toml");

        add_provider(
            &config_path,
            "openai",
            ProviderConfig {
                display_name: None,
                provider_type: Some(ApiType::OpenAiResponse),
                base_url: Some("https://api.openai.test/v1".to_owned()),
                api_key: None,
                api_key_env: Some("OPENAI_API_KEY".to_owned()),
            },
        )
        .expect("add provider");

        let written = fs::read_to_string(config_path).expect("read config");
        assert!(written.contains("[runtime.retry]"));
        assert!(written.contains("max_retries = 5"));
        assert!(written.contains("[runtime.compaction]"));
        assert!(written.contains("enabled = true"));
        assert!(written.contains("keep_recent_messages = 20"));
    }

    #[test]
    fn config_write_drops_legacy_reasoning_effort_and_keeps_structured_reasoning() {
        let temp = TempDir::new().expect("temp dir");
        let config_path = write_project_config(
            temp.path(),
            r#"
[runtime]
reasoning_effort = "low"

[runtime.reasoning]
mode = "effort"
effort = "high"
"#,
        );

        add_provider(
            &config_path,
            "openai",
            ProviderConfig {
                display_name: None,
                provider_type: Some(ApiType::OpenAiResponse),
                base_url: Some("https://api.openai.test/v1".to_owned()),
                api_key: None,
                api_key_env: Some("OPENAI_API_KEY".to_owned()),
            },
        )
        .expect("add provider");

        let written = fs::read_to_string(config_path).expect("read config");
        let value: toml::Value = toml::from_str(&written).expect("parse written config");
        let runtime = value
            .get("runtime")
            .and_then(toml::Value::as_table)
            .expect("runtime table");
        assert!(!runtime.contains_key("reasoning_effort"));
        assert_eq!(
            runtime
                .get("reasoning")
                .and_then(toml::Value::as_table)
                .and_then(|reasoning| reasoning.get("mode"))
                .and_then(toml::Value::as_str),
            Some("effort")
        );
        assert_eq!(
            runtime
                .get("reasoning")
                .and_then(toml::Value::as_table)
                .and_then(|reasoning| reasoning.get("effort"))
                .and_then(toml::Value::as_str),
            Some("high")
        );
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
type = "openai_response"

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
type = "openai"
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
        assert!(written.contains("type = \"openai_response\""));
        assert!(written.contains("api_key = \"inline-key\""));
        assert!(written.contains("[models.\"openai/gpt-small\"]"));
        assert!(written.contains("[models.\"openai/gpt-large\"]"));
        let written_toml: toml::Value = toml::from_str(&written).expect("parse written config");
        let reasoning = written_toml
            .get("models")
            .and_then(toml::Value::as_table)
            .and_then(|models| models.get("openai/gpt-large"))
            .and_then(toml::Value::as_table)
            .and_then(|model| model.get("reasoning"))
            .and_then(toml::Value::as_table)
            .expect("typed model reasoning");
        assert_eq!(
            reasoning.get("type").and_then(toml::Value::as_str),
            Some("combined")
        );
        assert_eq!(
            reasoning
                .get("effort")
                .and_then(toml::Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(toml::Value::as_str)
                        .collect::<Vec<_>>()
                }),
            Some(vec!["low", "medium", "high"])
        );
        assert_eq!(
            reasoning.get("toggle").and_then(toml::Value::as_bool),
            Some(true)
        );
        let budget = reasoning
            .get("budget")
            .and_then(toml::Value::as_table)
            .expect("typed combined budget");
        assert_eq!(
            budget.get("min").and_then(toml::Value::as_integer),
            Some(128)
        );
        assert_eq!(
            budget.get("max").and_then(toml::Value::as_integer),
            Some(24_576)
        );
        assert_eq!(
            reasoning
                .get("disable_supported")
                .and_then(toml::Value::as_bool),
            Some(true)
        );
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
type = "openai"
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
            explicit_type: Some("openai_response".to_owned()),
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
                        reasoning_options: Vec::new(),
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
                        reasoning_options: vec![
                            serde_json::json!({ "type": "toggle" }),
                            serde_json::json!({
                                "type": "effort",
                                "values": ["low", "medium", "high"]
                            }),
                            serde_json::json!({
                                "type": "budget_tokens",
                                "min": 128,
                                "max": 24576
                            }),
                        ],
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
            workspace_policy: std::sync::Arc::new(std::sync::RwLock::new(None)),
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
            system_prompt_file: None,
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir: project_dir.to_path_buf(),
            config_path: project_dir.join(".neo/config.toml"),
            config_file_exists: true,
        }
    }
}
