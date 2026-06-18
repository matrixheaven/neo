use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::Context;
use neo_agent_core::session::workspace_sessions_dir as compute_workspace_sessions_dir;
use neo_agent_core::{PermissionPolicy, QueueMode, ToolExecutionMode};
use neo_ai::{ModelRegistry, ModelSpec, ReasoningEffort};
use neo_tui::{ImageProtocolPreference, KeyId, KeybindingAction, KeybindingsManager};
use serde::{Deserialize, Serialize};

use crate::{
    cli::Cli,
    themes::{self, ResolvedTheme},
    trust,
};

const CONFIG_DIR: &str = ".neo";
const CONFIG_FILE: &str = "config.toml";
const DEFAULT_MODEL: &str = "gpt-4.1";
const DEFAULT_PROVIDER: &str = "openai";
const DEFAULT_MODE: &str = "interactive";

#[derive(Debug, Clone, Default)]
pub struct ConfigOverrides {
    pub config_path: Option<PathBuf>,
    pub yolo: bool,
}

impl ConfigOverrides {
    pub fn from_cli(cli: &Cli) -> Self {
        Self {
            config_path: cli.config.clone(),
            yolo: cli.yolo,
        }
    }
}

pub(crate) fn scoped_models<'a>(
    models: impl IntoIterator<Item = &'a ModelSpec>,
    scope: &[String],
) -> Vec<ModelSpec> {
    let scope = scope
        .iter()
        .map(|pattern| pattern.trim())
        .filter(|pattern| !pattern.is_empty())
        .collect::<Vec<_>>();
    models
        .into_iter()
        .filter(|model| {
            scope.is_empty()
                || scope
                    .iter()
                    .any(|pattern| model_matches_scope_pattern(model, pattern))
        })
        .cloned()
        .collect()
}

fn model_matches_scope_pattern(model: &ModelSpec, pattern: &str) -> bool {
    let pattern = strip_thinking_suffix(pattern).trim();
    if pattern.is_empty() {
        return false;
    }
    let qualified = format!("{}/{}", model.provider.0, model.model);
    if pattern == qualified || pattern == model.model {
        return true;
    }
    if has_glob_meta(pattern) {
        return wildcard_match(pattern, &qualified) || wildcard_match(pattern, &model.model);
    }
    fuzzy_match(&qualified, pattern) || fuzzy_match(&model.model, pattern)
}

fn strip_thinking_suffix(pattern: &str) -> &str {
    let Some((model, suffix)) = pattern.rsplit_once(':') else {
        return pattern;
    };
    if matches!(
        suffix,
        "off" | "minimal" | "low" | "medium" | "high" | "xhigh"
    ) {
        model
    } else {
        pattern
    }
}

fn has_glob_meta(pattern: &str) -> bool {
    pattern
        .chars()
        .any(|character| matches!(character, '*' | '?' | '['))
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let text = text.chars().collect::<Vec<_>>();
    let mut matched = vec![vec![false; text.len() + 1]; pattern.len() + 1];
    matched[0][0] = true;
    for index in 1..=pattern.len() {
        if pattern[index - 1] == '*' {
            matched[index][0] = matched[index - 1][0];
        }
    }
    for pattern_index in 1..=pattern.len() {
        for text_index in 1..=text.len() {
            matched[pattern_index][text_index] = match pattern[pattern_index - 1] {
                '*' => {
                    matched[pattern_index - 1][text_index] || matched[pattern_index][text_index - 1]
                }
                '?' => matched[pattern_index - 1][text_index - 1],
                character => {
                    character == text[text_index - 1] && matched[pattern_index - 1][text_index - 1]
                }
            };
        }
    }
    matched[pattern.len()][text.len()]
}

fn fuzzy_match(haystack: &str, needle: &str) -> bool {
    let haystack = haystack.to_lowercase();
    let needle = needle.to_lowercase();
    if haystack.contains(&needle) {
        return true;
    }
    let mut chars = haystack.chars();
    needle
        .chars()
        .all(|needle_char| chars.any(|candidate| candidate == needle_char))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub default_model: String,
    pub default_provider: String,
    pub api_base: Option<String>,
    pub api_key_env: Option<String>,
    pub providers: BTreeMap<String, ProviderConfig>,
    /// Models defined inline in config.toml `[models.<alias>]`.
    pub models: BTreeMap<String, ModelConfig>,
    pub model_catalogs: Vec<PathBuf>,
    #[serde(skip)]
    pub model_scope: Vec<String>,
    pub sessions_dir: PathBuf,
    pub permissions: PermissionPolicy,
    pub defaults: Defaults,
    pub runtime: RuntimeConfig,
    pub tui: TuiConfig,
    #[serde(skip)]
    pub theme: ResolvedTheme,
    pub mcp: McpConfig,
    #[serde(skip)]
    pub prompt_templates: Vec<String>,
    #[serde(skip)]
    pub project_trusted: bool,
    pub project_dir: PathBuf,

    #[serde(skip)]
    pub config_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Defaults {
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub replay_reasoning: bool,
    pub steering_queue_mode: QueueMode,
    pub follow_up_queue_mode: QueueMode,
    pub tool_execution_mode: ToolExecutionMode,
    pub compaction: Option<RuntimeCompactionConfig>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            temperature: None,
            max_tokens: None,
            reasoning_effort: None,
            replay_reasoning: true,
            steering_queue_mode: QueueMode::All,
            follow_up_queue_mode: QueueMode::All,
            tool_execution_mode: ToolExecutionMode::Parallel,
            compaction: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeCompactionConfig {
    pub enabled: bool,
    pub max_estimated_tokens: usize,
    pub keep_recent_messages: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TuiConfig {
    #[serde(default)]
    pub image_protocol: ImageProtocolPreference,
    #[serde(default)]
    pub fetch_remote_images: bool,
    #[serde(default)]
    pub keybindings: BTreeMap<String, Vec<String>>,
}

impl Default for RuntimeCompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_estimated_tokens: default_runtime_compaction_max_estimated_tokens(),
            keep_recent_messages: default_runtime_compaction_keep_recent_messages(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<neo_ai::ApiType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Inline API key stored directly in config.toml.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Environment variable name that holds the API key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// Legacy alias for `api_base` (used by `api_base` top-level override).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_base: Option<String>,
}

impl ProviderConfig {
    /// Returns the effective base URL: `base_url` takes priority, then `api_base`.
    #[must_use]
    pub fn effective_base_url(&self) -> Option<&str> {
        self.base_url.as_deref().or(self.api_base.as_deref())
    }
}

/// A model definition in `config.toml` `[models.<alias>]`.
///
/// Each model references a provider by id and specifies the actual model ID
/// sent to the API, context window, and capabilities.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Provider id — must match a key in `[providers.<id>]`.
    pub provider: String,
    /// Actual model ID sent to the provider API.
    pub model: String,
    /// Maximum context window in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_tokens: Option<u32>,
    /// Maximum output tokens (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// Capability tags: `"streaming"`, `"tools"`, `"images"`, `"reasoning"`.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Human-readable display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub id: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub transport: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enabled_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_timeout_ms: Option<u64>,
}

const fn default_enabled() -> bool {
    true
}

const fn default_runtime_compaction_max_estimated_tokens() -> usize {
    32_000
}

const fn default_runtime_compaction_keep_recent_messages() -> usize {
    20
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct FileConfig {
    pub(crate) default_model: Option<String>,
    pub(crate) default_provider: Option<String>,
    pub(crate) api_base: Option<String>,
    pub(crate) api_key_env: Option<String>,
    pub(crate) providers: Option<BTreeMap<String, ProviderConfig>>,
    /// Models defined inline via `[models.<alias>]` tables.
    pub(crate) models: Option<BTreeMap<String, ModelConfig>>,
    pub(crate) model_scope: Option<Vec<String>>,
    pub(crate) model_catalogs: Option<Vec<PathBuf>>,
    pub(crate) prompt_templates: Option<Vec<String>>,
    pub(crate) sessions_dir: Option<PathBuf>,
    pub(crate) permissions: Option<PermissionPolicy>,
    pub(crate) defaults: Option<FileDefaults>,
    pub(crate) runtime: Option<FileRuntimeConfig>,
    pub(crate) tui: Option<FileTuiConfig>,
    pub(crate) mcp: Option<McpConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct FileDefaults {
    pub(crate) mode: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct FileRuntimeConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reasoning_effort: Option<ReasoningEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) replay_reasoning: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) steering_queue_mode: Option<QueueMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) follow_up_queue_mode: Option<QueueMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tool_execution_mode: Option<ToolExecutionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) compaction: Option<FileRuntimeCompactionConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct FileRuntimeCompactionConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_estimated_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) keep_recent_messages: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct FileTuiConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    image_protocol: Option<ImageProtocolPreference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fetch_remote_images: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    keybindings: Option<BTreeMap<String, Vec<String>>>,
}

impl AppConfig {
    #[allow(clippy::too_many_lines)]
    pub fn load(overrides: ConfigOverrides) -> anyhow::Result<Self> {
        let config_path = overrides.config_path.unwrap_or(find_config_path()?);
        let project_dir = config_path
            .parent()
            .and_then(Path::parent)
            .map_or(env::current_dir()?, Path::to_path_buf);

        let global_config = find_global_config_path()
            .map(|path| read_file_config(&path))
            .transpose()?
            .unwrap_or_default();
        let project_config = read_file_config(&config_path)?;
        let file_config = merge_file_configs(global_config, project_config);
        let project_trusted = project_trusted_from_yolo(&project_dir, overrides.yolo)?;

        let mut default_model = file_config
            .default_model
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        let mut default_provider = file_config
            .default_provider
            .unwrap_or_else(|| DEFAULT_PROVIDER.to_owned());
        let providers = file_config.providers.unwrap_or_default();
        let models = file_config.models.unwrap_or_default();
        let api_base = file_config.api_base;
        let api_key_env = file_config
            .api_key_env
            .or_else(|| provider_api_key_env(&providers, &default_provider));
        let model_catalogs: Vec<PathBuf> = file_config
            .model_catalogs
            .unwrap_or_default()
            .into_iter()
            .map(|path| resolve_project_path(&project_dir, path))
            .collect();
        let model_scope = file_config.model_scope.unwrap_or_default();
        apply_scoped_default_model(
            &mut default_provider,
            &mut default_model,
            &model_catalogs,
            &model_scope,
        )?;
        let prompt_templates = file_config.prompt_templates.unwrap_or_default();
        let sessions_dir = file_config
            .sessions_dir
            .map(expand_user_path)
            .unwrap_or_else(|| {
                neo_home().map_or_else(
                    || project_dir.join(CONFIG_DIR).join("sessions"),
                    |home| home.join("sessions"),
                )
            });
        let permissions = file_config.permissions.unwrap_or_default();
        let runtime = runtime_from_file(file_config.runtime);
        validate_runtime_config(&runtime)?;
        let tui = tui_from_file(file_config.tui);
        validate_tui_config(&tui)?;
        let theme = themes::resolve_theme(&project_dir)?;
        let mcp = file_config.mcp.unwrap_or_default();
        let mode = file_config
            .defaults
            .and_then(|defaults| defaults.mode)
            .unwrap_or_else(|| DEFAULT_MODE.to_owned());

        Ok(Self {
            default_model,
            default_provider,
            api_base,
            api_key_env,
            providers,
            models,
            model_catalogs,
            model_scope,
            sessions_dir,
            permissions,
            defaults: Defaults { mode },
            runtime,
            tui,
            theme,
            mcp,
            prompt_templates,
            project_trusted,
            project_dir,
            config_path,
        })
    }
}

fn project_trusted_from_yolo(project_dir: &Path, yolo: bool) -> anyhow::Result<bool> {
    trust::resolve_project_trust(project_dir, yolo)
}

fn scoped_default_model(catalogs: &[PathBuf], model_scope: &[String]) -> anyhow::Result<ModelSpec> {
    let mut registry = ModelRegistry::seeded();
    for path in catalogs {
        registry
            .load_catalog_path(path)
            .map_err(anyhow::Error::from)?;
    }
    let models = registry.list();
    let scoped = scoped_models(models.iter(), model_scope);
    scoped.first().cloned().with_context(|| {
        format!(
            "no models match model_scope {}; run `neo models list` for supported entries",
            model_scope.join(",")
        )
    })
}

fn apply_scoped_default_model(
    default_provider: &mut String,
    default_model: &mut String,
    model_catalogs: &[PathBuf],
    model_scope: &[String],
) -> anyhow::Result<()> {
    if model_scope.is_empty() {
        return Ok(());
    }
    let scoped_default = scoped_default_model(model_catalogs, model_scope)
        .with_context(|| format!("failed to resolve model_scope {}", model_scope.join(",")))?;
    *default_provider = scoped_default.provider.0;
    *default_model = scoped_default.model;
    Ok(())
}

fn provider_api_key_env(
    providers: &BTreeMap<String, ProviderConfig>,
    provider_id: &str,
) -> Option<String> {
    providers
        .get(provider_id)
        .and_then(|provider| provider.api_key_env.clone())
}

pub fn upsert_mcp_server(server: &McpServerConfig) -> anyhow::Result<String> {
    validate_mcp_server(server)?;
    let config_path = find_config_path()?;
    let mut config = read_file_config(&config_path)?;
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
    write_file_config(&config_path, &config)?;
    Ok(format!("added MCP server {}\n", server.id))
}

pub fn remove_mcp_server(server_id: &str) -> anyhow::Result<String> {
    let config_path = find_config_path()?;
    let mut config = read_file_config(&config_path)?;
    let Some(mcp) = config.mcp.as_mut() else {
        anyhow::bail!("MCP server {server_id} is not configured");
    };
    let original_len = mcp.servers.len();
    mcp.servers.retain(|server| server.id != server_id);
    anyhow::ensure!(
        mcp.servers.len() != original_len,
        "MCP server {server_id} is not configured"
    );
    write_file_config(&config_path, &config)?;
    Ok(format!("removed MCP server {server_id}\n"))
}

pub fn set_mcp_server_enabled(server_id: &str, enabled: bool) -> anyhow::Result<String> {
    let config_path = find_config_path()?;
    let mut config = read_file_config(&config_path)?;
    let Some(server) = config
        .mcp
        .as_mut()
        .and_then(|mcp| mcp.servers.iter_mut().find(|server| server.id == server_id))
    else {
        anyhow::bail!("MCP server {server_id} is not configured");
    };
    server.enabled = enabled;
    write_file_config(&config_path, &config)?;
    let action = if enabled { "enabled" } else { "disabled" };
    Ok(format!("{action} MCP server {server_id}\n"))
}

fn merge_file_configs(base: FileConfig, layer: FileConfig) -> FileConfig {
    FileConfig {
        default_model: layer.default_model.or(base.default_model),
        default_provider: layer.default_provider.or(base.default_provider),
        api_base: layer.api_base.or(base.api_base),
        api_key_env: layer.api_key_env.or(base.api_key_env),
        providers: merge_provider_configs(base.providers, layer.providers),
        models: merge_model_configs(base.models, layer.models),
        model_scope: merge_string_lists(base.model_scope, layer.model_scope),
        model_catalogs: merge_path_lists(base.model_catalogs, layer.model_catalogs),
        prompt_templates: merge_string_lists(base.prompt_templates, layer.prompt_templates),
        sessions_dir: layer.sessions_dir.or(base.sessions_dir),
        permissions: layer.permissions.or(base.permissions),
        defaults: merge_defaults(base.defaults, layer.defaults),
        runtime: merge_runtime_configs(base.runtime, layer.runtime),
        tui: merge_tui_configs(base.tui, layer.tui),
        mcp: merge_mcp_configs(base.mcp, layer.mcp),
    }
}

fn merge_provider_configs(
    base: Option<BTreeMap<String, ProviderConfig>>,
    layer: Option<BTreeMap<String, ProviderConfig>>,
) -> Option<BTreeMap<String, ProviderConfig>> {
    match (base, layer) {
        (None, None) => None,
        (Some(providers), None) | (None, Some(providers)) => Some(providers),
        (Some(mut base), Some(layer)) => {
            for (provider_id, layer_config) in layer {
                base.entry(provider_id)
                    .and_modify(|base_config| {
                        *base_config =
                            merge_provider_config(base_config.clone(), layer_config.clone());
                    })
                    .or_insert(layer_config);
            }
            Some(base)
        }
    }
}

fn merge_provider_config(base: ProviderConfig, layer: ProviderConfig) -> ProviderConfig {
    ProviderConfig {
        provider_type: layer.provider_type.or(base.provider_type),
        base_url: layer.base_url.or(base.base_url),
        api_key: layer.api_key.or(base.api_key),
        api_key_env: layer.api_key_env.or(base.api_key_env),
        api_base: layer.api_base.or(base.api_base),
    }
}

fn merge_model_configs(
    base: Option<BTreeMap<String, ModelConfig>>,
    layer: Option<BTreeMap<String, ModelConfig>>,
) -> Option<BTreeMap<String, ModelConfig>> {
    match (base, layer) {
        (None, None) => None,
        (Some(models), None) | (None, Some(models)) => Some(models),
        (Some(mut base), Some(layer)) => {
            for (alias, cfg) in layer {
                base.insert(alias, cfg);
            }
            Some(base)
        }
    }
}

fn merge_string_lists(
    base: Option<Vec<String>>,
    layer: Option<Vec<String>>,
) -> Option<Vec<String>> {
    match (base, layer) {
        (None, None) => None,
        (Some(values), None) | (None, Some(values)) => Some(values),
        (Some(mut base), Some(layer)) => {
            for value in layer {
                if !base.contains(&value) {
                    base.push(value);
                }
            }
            Some(base)
        }
    }
}

fn merge_path_lists(
    base: Option<Vec<PathBuf>>,
    layer: Option<Vec<PathBuf>>,
) -> Option<Vec<PathBuf>> {
    match (base, layer) {
        (None, None) => None,
        (Some(paths), None) | (None, Some(paths)) => Some(paths),
        (Some(mut base), Some(layer)) => {
            for path in layer {
                if !base.contains(&path) {
                    base.push(path);
                }
            }
            Some(base)
        }
    }
}

fn merge_defaults(base: Option<FileDefaults>, layer: Option<FileDefaults>) -> Option<FileDefaults> {
    match (base, layer) {
        (None, None) => None,
        (Some(defaults), None) | (None, Some(defaults)) => Some(defaults),
        (Some(base), Some(layer)) => Some(FileDefaults {
            mode: layer.mode.or(base.mode),
        }),
    }
}

fn merge_runtime_configs(
    base: Option<FileRuntimeConfig>,
    layer: Option<FileRuntimeConfig>,
) -> Option<FileRuntimeConfig> {
    match (base, layer) {
        (None, None) => None,
        (Some(runtime), None) | (None, Some(runtime)) => Some(runtime),
        (Some(base), Some(layer)) => Some(FileRuntimeConfig {
            temperature: layer.temperature.or(base.temperature),
            max_tokens: layer.max_tokens.or(base.max_tokens),
            reasoning_effort: layer.reasoning_effort.or(base.reasoning_effort),
            replay_reasoning: layer.replay_reasoning.or(base.replay_reasoning),
            steering_queue_mode: layer.steering_queue_mode.or(base.steering_queue_mode),
            follow_up_queue_mode: layer.follow_up_queue_mode.or(base.follow_up_queue_mode),
            tool_execution_mode: layer.tool_execution_mode.or(base.tool_execution_mode),
            compaction: merge_runtime_compaction_configs(base.compaction, layer.compaction),
        }),
    }
}

fn merge_runtime_compaction_configs(
    base: Option<FileRuntimeCompactionConfig>,
    layer: Option<FileRuntimeCompactionConfig>,
) -> Option<FileRuntimeCompactionConfig> {
    match (base, layer) {
        (None, None) => None,
        (Some(compaction), None) | (None, Some(compaction)) => Some(compaction),
        (Some(base), Some(layer)) => Some(FileRuntimeCompactionConfig {
            enabled: layer.enabled.or(base.enabled),
            max_estimated_tokens: layer.max_estimated_tokens.or(base.max_estimated_tokens),
            keep_recent_messages: layer.keep_recent_messages.or(base.keep_recent_messages),
        }),
    }
}

fn merge_tui_configs(
    base: Option<FileTuiConfig>,
    layer: Option<FileTuiConfig>,
) -> Option<FileTuiConfig> {
    match (base, layer) {
        (None, None) => None,
        (Some(tui), None) | (None, Some(tui)) => Some(tui),
        (Some(base), Some(layer)) => {
            let mut keybindings = base.keybindings.unwrap_or_default();
            for (action, keys) in layer.keybindings.unwrap_or_default() {
                keybindings.insert(action, keys);
            }
            Some(FileTuiConfig {
                image_protocol: layer.image_protocol.or(base.image_protocol),
                fetch_remote_images: layer.fetch_remote_images.or(base.fetch_remote_images),
                keybindings: (!keybindings.is_empty()).then_some(keybindings),
            })
        }
    }
}

fn merge_mcp_configs(base: Option<McpConfig>, layer: Option<McpConfig>) -> Option<McpConfig> {
    match (base, layer) {
        (None, None) => None,
        (Some(mcp), None) | (None, Some(mcp)) => Some(mcp),
        (Some(mut base), Some(layer)) => {
            for server in layer.servers {
                base.servers.retain(|candidate| candidate.id != server.id);
                base.servers.push(server);
            }
            Some(base)
        }
    }
}

fn validate_mcp_server(server: &McpServerConfig) -> anyhow::Result<()> {
    anyhow::ensure!(
        !server.id.trim().is_empty(),
        "MCP server id must not be empty"
    );
    anyhow::ensure!(
        !server.id.contains('/'),
        "MCP server id must not contain '/'"
    );
    match server.transport.as_str() {
        "stdio" => {
            anyhow::ensure!(
                server
                    .command
                    .as_deref()
                    .is_some_and(|command| !command.trim().is_empty()),
                "stdio MCP server {} requires --command",
                server.id
            );
        }
        "http" | "sse" => {
            anyhow::ensure!(
                server
                    .url
                    .as_deref()
                    .is_some_and(|url| !url.trim().is_empty()),
                "{} MCP server {} requires --url",
                server.transport,
                server.id
            );
        }
        other => anyhow::bail!("unsupported MCP transport for {}: {other}", server.id),
    }
    Ok(())
}

fn runtime_from_file(runtime: Option<FileRuntimeConfig>) -> RuntimeConfig {
    let Some(runtime) = runtime else {
        return RuntimeConfig::default();
    };
    RuntimeConfig {
        temperature: runtime.temperature,
        max_tokens: runtime.max_tokens,
        reasoning_effort: runtime.reasoning_effort,
        replay_reasoning: runtime.replay_reasoning.unwrap_or(true),
        steering_queue_mode: runtime.steering_queue_mode.unwrap_or(QueueMode::All),
        follow_up_queue_mode: runtime.follow_up_queue_mode.unwrap_or(QueueMode::All),
        tool_execution_mode: runtime
            .tool_execution_mode
            .unwrap_or(ToolExecutionMode::Parallel),
        compaction: runtime
            .compaction
            .map(|compaction| RuntimeCompactionConfig {
                enabled: compaction.enabled.unwrap_or(true),
                max_estimated_tokens: compaction
                    .max_estimated_tokens
                    .unwrap_or_else(default_runtime_compaction_max_estimated_tokens),
                keep_recent_messages: compaction
                    .keep_recent_messages
                    .unwrap_or_else(default_runtime_compaction_keep_recent_messages),
            }),
    }
}

fn tui_from_file(tui: Option<FileTuiConfig>) -> TuiConfig {
    let Some(tui) = tui else {
        return TuiConfig::default();
    };
    TuiConfig {
        image_protocol: tui.image_protocol.unwrap_or_default(),
        fetch_remote_images: tui.fetch_remote_images.unwrap_or(false),
        keybindings: tui.keybindings.unwrap_or_default(),
    }
}

fn validate_runtime_config(config: &RuntimeConfig) -> anyhow::Result<()> {
    if let Some(temperature) = config.temperature {
        anyhow::ensure!(
            temperature.is_finite() && temperature >= 0.0,
            "runtime.temperature must be a finite non-negative number"
        );
    }
    if let Some(max_tokens) = config.max_tokens {
        anyhow::ensure!(max_tokens > 0, "runtime.max_tokens must be greater than 0");
    }
    if let Some(compaction) = &config.compaction
        && compaction.enabled
    {
        anyhow::ensure!(
            compaction.max_estimated_tokens > 0,
            "runtime.compaction.max_estimated_tokens must be greater than 0"
        );
        anyhow::ensure!(
            compaction.keep_recent_messages > 0,
            "runtime.compaction.keep_recent_messages must be greater than 0"
        );
    }
    Ok(())
}

fn validate_tui_config(config: &TuiConfig) -> anyhow::Result<()> {
    let default_manager = KeybindingsManager::default();
    let mut manager = KeybindingsManager::default();
    let overrides = config.keybinding_overrides()?;
    for (_action, keys) in &overrides {
        for key in keys {
            anyhow::ensure!(
                !key.is_text_insertion_key(),
                "tui.keybindings key {key} is reserved for prompt text insertion"
            );
        }
    }
    manager.set_user_bindings(overrides.iter().cloned());
    anyhow::ensure!(
        manager.conflicts().is_empty(),
        "tui.keybindings contains conflicting key assignments"
    );
    validate_tui_context_conflicts(&default_manager, &manager, &overrides)?;
    Ok(())
}

fn validate_tui_context_conflicts(
    default_manager: &KeybindingsManager,
    manager: &KeybindingsManager,
    overrides: &[(KeybindingAction, Vec<KeyId>)],
) -> anyhow::Result<()> {
    for (action, keys) in overrides {
        for context in [TUI_EDITING_ACTIONS, TUI_OVERLAY_ACTIONS] {
            if !context.contains(action) {
                continue;
            }
            for key in keys {
                let current_actions = context_actions_for_key(manager, context, key);
                if current_actions.len() <= 1 {
                    continue;
                }
                let default_actions = context_actions_for_key(default_manager, context, key);
                if current_actions != default_actions {
                    let action_ids = current_actions
                        .iter()
                        .map(|action| action.id())
                        .collect::<Vec<_>>()
                        .join(", ");
                    anyhow::bail!(
                        "tui.keybindings key {key} conflicts within a TUI input context: {action_ids}"
                    );
                }
            }
        }
    }
    Ok(())
}

fn context_actions_for_key(
    manager: &KeybindingsManager,
    context: &[KeybindingAction],
    key: &KeyId,
) -> BTreeSet<KeybindingAction> {
    context
        .iter()
        .filter(|action| {
            manager
                .keys(**action)
                .iter()
                .any(|candidate| candidate == key)
        })
        .copied()
        .collect()
}

const TUI_EDITING_ACTIONS: &[KeybindingAction] = &[
    KeybindingAction::InputSubmit,
    KeybindingAction::InputNewLine,
    KeybindingAction::TranscriptCopySelection,
    KeybindingAction::AppClear,
    KeybindingAction::AppExit,
    KeybindingAction::AppSuspend,
    KeybindingAction::InputCopy,
    KeybindingAction::TranscriptSelectionStart,
    KeybindingAction::TranscriptSelectionClear,
    KeybindingAction::TranscriptSelectionExtendUp,
    KeybindingAction::TranscriptSelectionExtendDown,
    KeybindingAction::TranscriptSelectionExtendPageUp,
    KeybindingAction::TranscriptSelectionExtendPageDown,
    KeybindingAction::CommandPaletteOpen,
    KeybindingAction::SessionPickerOpen,
    KeybindingAction::ModelPickerOpen,
    KeybindingAction::EditorCursorLeft,
    KeybindingAction::EditorCursorRight,
    KeybindingAction::EditorCursorWordLeft,
    KeybindingAction::EditorCursorWordRight,
    KeybindingAction::EditorCursorLineStart,
    KeybindingAction::EditorCursorLineEnd,
    KeybindingAction::EditorDeleteCharBackward,
    KeybindingAction::EditorDeleteCharForward,
    KeybindingAction::EditorDeleteWordBackward,
    KeybindingAction::EditorDeleteWordForward,
    KeybindingAction::EditorDeleteToLineStart,
    KeybindingAction::EditorDeleteToLineEnd,
    KeybindingAction::EditorYank,
    KeybindingAction::EditorUndo,
    KeybindingAction::InputTab,
    KeybindingAction::SelectCancel,
];

const TUI_OVERLAY_ACTIONS: &[KeybindingAction] = &[
    KeybindingAction::SelectConfirm,
    KeybindingAction::SelectCancel,
    KeybindingAction::SessionFork,
    KeybindingAction::SelectUp,
    KeybindingAction::SelectDown,
    KeybindingAction::SelectPageUp,
    KeybindingAction::SelectPageDown,
];

impl TuiConfig {
    pub fn keybinding_overrides(&self) -> anyhow::Result<Vec<(KeybindingAction, Vec<KeyId>)>> {
        self.keybindings
            .iter()
            .map(|(action_id, keys)| {
                let action = KeybindingAction::from_id(action_id)
                    .with_context(|| format!("unsupported TUI keybinding action: {action_id}"))?;
                let keys = keys
                    .iter()
                    .map(|key| KeyId::new(key).map_err(|err| anyhow::anyhow!(err.to_string())))
                    .collect::<anyhow::Result<Vec<_>>>()?;
                Ok((action, keys))
            })
            .collect()
    }
}

pub(crate) fn find_config_path() -> anyhow::Result<PathBuf> {
    Ok(env::current_dir()?.join(CONFIG_DIR).join(CONFIG_FILE))
}

fn find_global_config_path() -> Option<PathBuf> {
    home_dir().map(|home| home.join(CONFIG_DIR).join(CONFIG_FILE))
}

pub(crate) fn global_prompts_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join(CONFIG_DIR).join("prompts"))
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// Resolve the neo home directory: `$NEO_HOME` if set, otherwise `~/.neo`.
pub(crate) fn neo_home() -> Option<PathBuf> {
    env::var_os("NEO_HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(CONFIG_DIR)))
}

/// Compute the workspace-scoped sessions directory for a given config.
///
/// Given the central `sessions_dir` (e.g. `~/.neo/sessions`) and the
/// project directory, returns `<sessions_dir>/wd_<slug>_<hash12>/`.
pub(crate) fn workspace_sessions_dir(config: &AppConfig) -> PathBuf {
    compute_workspace_sessions_dir(&config.sessions_dir, &config.project_dir)
}

fn expand_user_path(path: PathBuf) -> PathBuf {
    let Some(raw) = path.to_str().map(str::to_owned) else {
        return path;
    };
    if raw == "~" {
        return home_dir().unwrap_or(path);
    }
    let Some(rest) = raw.strip_prefix("~/") else {
        return path;
    };
    home_dir().map_or(path, |home| home.join(rest))
}

fn resolve_project_path(project_dir: &Path, path: PathBuf) -> PathBuf {
    let path = expand_user_path(path);
    if path.is_absolute() {
        path
    } else {
        project_dir.join(path)
    }
}

pub(crate) fn read_file_config(path: &Path) -> anyhow::Result<FileConfig> {
    if !path.exists() {
        return Ok(FileConfig::default());
    }

    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    toml::from_str(&content).with_context(|| format!("failed to parse config {}", path.display()))
}

pub(crate) fn write_file_config(path: &Path, config: &FileConfig) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let content = toml::to_string_pretty(config)?;
    fs::write(path, content).with_context(|| format!("failed to write config {}", path.display()))
}
