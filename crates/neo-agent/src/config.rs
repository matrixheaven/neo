use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use neo_agent_core::{PermissionPolicy, QueueMode, ToolExecutionMode};
use neo_ai::{ModelRegistry, ModelSpec, ReasoningEffort};
use neo_tui::{ImageProtocolPreference, KeyId, KeybindingAction, KeybindingsManager};
use serde::{Deserialize, Serialize};

use crate::{
    cli::{Cli, ThinkingLevel, ToolFilterArgs},
    themes::{self, ResolvedTheme},
    trust,
};

const CONFIG_DIR: &str = ".neo";
const CONFIG_FILE: &str = "config.toml";
const DEFAULT_MODEL: &str = "gpt-4.1";
const DEFAULT_PROVIDER: &str = "openai";
const DEFAULT_MODE: &str = "interactive";

#[derive(Debug, Default)]
struct EnvOverrides {
    model: Option<String>,
    provider: Option<String>,
    api_base: Option<String>,
    api_key_env: Option<String>,
    sessions_dir: Option<PathBuf>,
    mode: Option<String>,
    offline: bool,
}

#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct ConfigOverrides {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub api_base: Option<String>,
    pub api_key: Option<String>,
    pub config_path: Option<PathBuf>,
    pub sessions_dir: Option<PathBuf>,
    pub mode: Option<String>,
    pub model_scope: Vec<String>,
    pub approve: bool,
    pub no_approve: bool,
    pub prompt_templates: Vec<String>,
    pub skill_paths: Vec<PathBuf>,
    pub extension_paths: Vec<PathBuf>,
    pub theme_paths: Vec<PathBuf>,
    pub no_extensions: bool,
    pub no_themes: bool,
    pub no_prompt_templates: bool,
    pub no_skills: bool,
    pub no_context_files: bool,
    pub offline: bool,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Vec<String>,
    pub thinking: Option<ThinkingLevel>,
    pub tool_filters: ToolFilterConfig,
}

impl ConfigOverrides {
    pub fn from_cli(cli: &Cli) -> Self {
        Self {
            model: cli.model.clone(),
            provider: cli.provider.clone(),
            api_base: cli.api_base.clone(),
            api_key: cli.api_key.clone(),
            config_path: cli.config.clone(),
            sessions_dir: cli.session_dir.clone(),
            mode: cli.mode.clone(),
            model_scope: clean_model_scope(&cli.models),
            approve: cli.approve,
            no_approve: cli.no_approve,
            prompt_templates: cli.prompt_template.clone(),
            skill_paths: cli.skill.clone(),
            extension_paths: cli.extension.clone(),
            theme_paths: cli.theme.clone(),
            no_extensions: cli.no_extensions,
            no_themes: cli.no_themes,
            no_prompt_templates: cli.no_prompt_templates,
            no_skills: cli.no_skills,
            no_context_files: cli.no_context_files,
            offline: cli.offline,
            system_prompt: cli.system_prompt.clone(),
            append_system_prompt: cli.append_system_prompt.clone(),
            thinking: cli.thinking,
            tool_filters: ToolFilterConfig::from_cli(&cli.tool_filters),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolFilterConfig {
    pub no_tools: bool,
    pub no_builtin_tools: bool,
    pub allow: Vec<String>,
    pub exclude: Vec<String>,
}

impl ToolFilterConfig {
    fn from_cli(filters: &ToolFilterArgs) -> Self {
        Self {
            no_tools: filters.no_tools,
            no_builtin_tools: filters.no_builtin_tools,
            allow: clean_tool_names(&filters.tools),
            exclude: clean_tool_names(&filters.exclude_tools),
        }
    }
}

fn clean_tool_names(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn clean_model_scope(values: &[String]) -> Vec<String> {
    values
        .iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
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
#[allow(clippy::struct_excessive_bools)]
pub struct AppConfig {
    pub default_model: String,
    pub default_provider: String,
    pub api_base: Option<String>,
    #[serde(skip)]
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    pub providers: BTreeMap<String, ProviderConfig>,
    pub model_catalogs: Vec<PathBuf>,
    #[serde(skip)]
    pub model_scope: Vec<String>,
    #[serde(skip)]
    pub model_selection: ModelSelection,
    pub sessions_dir: PathBuf,
    pub permissions: PermissionPolicy,
    pub defaults: Defaults,
    pub runtime: RuntimeConfig,
    pub tui: TuiConfig,
    #[serde(skip)]
    pub theme: ResolvedTheme,
    pub mcp: McpConfig,
    pub approve: bool,
    pub no_approve: bool,
    #[serde(skip)]
    pub prompt_templates: Vec<String>,
    #[serde(skip)]
    pub skill_paths: Vec<PathBuf>,
    #[serde(skip)]
    pub extension_paths: Vec<PathBuf>,
    #[serde(skip)]
    pub no_extensions: bool,
    #[serde(skip)]
    pub configured_prompt_templates: Vec<String>,
    #[serde(skip)]
    pub no_prompt_templates: bool,
    #[serde(skip)]
    pub no_skills: bool,
    #[serde(skip)]
    pub no_context_files: bool,
    #[serde(skip)]
    pub offline: bool,
    #[serde(skip)]
    pub system_prompt: Option<String>,
    #[serde(skip)]
    pub append_system_prompt: Vec<String>,
    #[serde(skip)]
    pub tool_filters: ToolFilterConfig,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ModelSelection {
    #[default]
    Default,
    Explicit,
}

impl ModelSelection {
    const fn from_explicit(explicit: bool) -> Self {
        if explicit {
            Self::Explicit
        } else {
            Self::Default
        }
    }
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
    pub api_base: Option<String>,
    pub api_key_env: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

impl McpConfig {
    fn redacted(&self) -> Self {
        Self {
            servers: self.servers.iter().map(McpServerConfig::redacted).collect(),
        }
    }
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
}

impl McpServerConfig {
    fn redacted(&self) -> Self {
        let mut server = self.clone();
        server.env = server
            .env
            .keys()
            .map(|key| (key.clone(), "[REDACTED]".to_owned()))
            .collect();
        server.headers = server
            .headers
            .keys()
            .map(|key| (key.clone(), "[REDACTED]".to_owned()))
            .collect();
        server
    }
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
struct FileConfig {
    default_model: Option<String>,
    default_provider: Option<String>,
    api_base: Option<String>,
    api_key_env: Option<String>,
    providers: Option<BTreeMap<String, ProviderConfig>>,
    model_scope: Option<Vec<String>>,
    model_catalogs: Option<Vec<PathBuf>>,
    prompt_templates: Option<Vec<String>>,
    sessions_dir: Option<PathBuf>,
    permissions: Option<PermissionPolicy>,
    defaults: Option<FileDefaults>,
    runtime: Option<FileRuntimeConfig>,
    tui: Option<FileTuiConfig>,
    mcp: Option<McpConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FileDefaults {
    mode: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FileRuntimeConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<ReasoningEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    replay_reasoning: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    steering_queue_mode: Option<QueueMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    follow_up_queue_mode: Option<QueueMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_execution_mode: Option<ToolExecutionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compaction: Option<FileRuntimeCompactionConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FileRuntimeCompactionConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_estimated_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    keep_recent_messages: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FileTuiConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    image_protocol: Option<ImageProtocolPreference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fetch_remote_images: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    keybindings: Option<BTreeMap<String, Vec<String>>>,
}

impl FileTuiConfig {
    fn from_tui(tui: &TuiConfig) -> Self {
        Self {
            image_protocol: (tui.image_protocol != ImageProtocolPreference::Auto)
                .then_some(tui.image_protocol),
            fetch_remote_images: tui.fetch_remote_images.then_some(tui.fetch_remote_images),
            keybindings: (!tui.keybindings.is_empty()).then(|| tui.keybindings.clone()),
        }
    }
}

impl FileRuntimeConfig {
    fn from_runtime(runtime: &RuntimeConfig) -> Self {
        Self {
            temperature: runtime.temperature,
            max_tokens: runtime.max_tokens,
            reasoning_effort: runtime.reasoning_effort,
            replay_reasoning: Some(runtime.replay_reasoning),
            steering_queue_mode: Some(runtime.steering_queue_mode),
            follow_up_queue_mode: Some(runtime.follow_up_queue_mode),
            tool_execution_mode: Some(runtime.tool_execution_mode),
            compaction: runtime
                .compaction
                .as_ref()
                .map(FileRuntimeCompactionConfig::from_runtime),
        }
    }
}

impl FileRuntimeCompactionConfig {
    fn from_runtime(compaction: &RuntimeCompactionConfig) -> Self {
        Self {
            enabled: Some(compaction.enabled),
            max_estimated_tokens: (compaction.max_estimated_tokens
                != default_runtime_compaction_max_estimated_tokens())
            .then_some(compaction.max_estimated_tokens),
            keep_recent_messages: Some(compaction.keep_recent_messages),
        }
    }
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
        let env_overrides = env_overrides();
        let thinking_override = overrides.thinking;
        let project_trusted =
            project_trusted_from_overrides(&project_dir, overrides.approve, overrides.no_approve)?;

        let explicit_model = overrides.model.is_some();
        let mut default_model = overrides
            .model
            .or(env_overrides.model)
            .or(file_config.default_model)
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        let mut default_provider = overrides
            .provider
            .or(env_overrides.provider)
            .or(file_config.default_provider)
            .unwrap_or_else(|| DEFAULT_PROVIDER.to_owned());
        let providers = file_config.providers.unwrap_or_default();
        let api_base = overrides
            .api_base
            .or(env_overrides.api_base)
            .or(file_config.api_base);
        let api_key_env = env_overrides
            .api_key_env
            .or(file_config.api_key_env)
            .or_else(|| provider_api_key_env(&providers, &default_provider));
        let model_catalogs: Vec<PathBuf> = file_config
            .model_catalogs
            .unwrap_or_default()
            .into_iter()
            .map(|path| resolve_project_path(&project_dir, path))
            .collect();
        let cli_model_scope = overrides.model_scope;
        let explicit_model_scope = !cli_model_scope.is_empty();
        let model_scope = if explicit_model_scope {
            apply_scoped_default_model(
                &mut default_provider,
                &mut default_model,
                &model_catalogs,
                &cli_model_scope,
                explicit_model,
            )?;
            cli_model_scope
        } else {
            file_config.model_scope.unwrap_or_default()
        };
        let configured_prompt_templates = file_config.prompt_templates.unwrap_or_default();
        let sessions_dir = overrides
            .sessions_dir
            .map(expand_user_path)
            .or(env_overrides.sessions_dir)
            .or_else(|| file_config.sessions_dir.map(expand_user_path))
            .unwrap_or_else(|| project_dir.join(CONFIG_DIR).join("sessions"));
        let permissions = file_config.permissions.unwrap_or_default();
        let runtime = runtime_from_file(file_config.runtime);
        let runtime = apply_runtime_overrides(runtime, thinking_override);
        validate_runtime_config(&runtime)?;
        let tui = tui_from_file(file_config.tui);
        validate_tui_config(&tui)?;
        let theme =
            themes::resolve_theme(&project_dir, &overrides.theme_paths, overrides.no_themes)?;
        let mcp = file_config.mcp.unwrap_or_default();
        let mode = overrides
            .mode
            .or(env_overrides.mode)
            .or(file_config.defaults.and_then(|defaults| defaults.mode))
            .unwrap_or_else(|| DEFAULT_MODE.to_owned());

        Ok(Self {
            default_model,
            default_provider,
            api_base,
            api_key: overrides.api_key,
            api_key_env,
            providers,
            model_catalogs,
            model_scope,
            model_selection: ModelSelection::from_explicit(explicit_model),
            sessions_dir,
            permissions,
            defaults: Defaults { mode },
            runtime,
            tui,
            theme,
            mcp,
            approve: overrides.approve,
            no_approve: overrides.no_approve,
            prompt_templates: overrides.prompt_templates,
            skill_paths: overrides.skill_paths,
            extension_paths: overrides.extension_paths,
            no_extensions: overrides.no_extensions,
            configured_prompt_templates,
            no_prompt_templates: overrides.no_prompt_templates,
            no_skills: overrides.no_skills,
            no_context_files: overrides.no_context_files,
            offline: overrides.offline || env_overrides.offline,
            system_prompt: overrides.system_prompt,
            append_system_prompt: overrides.append_system_prompt,
            tool_filters: overrides.tool_filters,
            project_trusted,
            project_dir,
            config_path,
        })
    }
}

fn env_overrides() -> EnvOverrides {
    EnvOverrides {
        model: env::var("NEO_MODEL").ok(),
        provider: env::var("NEO_PROVIDER").ok(),
        api_base: env::var("NEO_API_BASE").ok(),
        api_key_env: env::var("NEO_API_KEY_ENV").ok(),
        sessions_dir: env::var("NEO_SESSIONS_DIR")
            .ok()
            .map(PathBuf::from)
            .map(expand_user_path),
        mode: env::var("NEO_MODE").ok(),
        offline: env::var("NEO_OFFLINE").is_ok_and(|value| truthy_env_flag(&value)),
    }
}

fn truthy_env_flag(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes"
    )
}

fn project_trusted_from_overrides(
    project_dir: &Path,
    approve: bool,
    no_approve: bool,
) -> anyhow::Result<bool> {
    trust::resolve_project_trust(project_dir, approve, no_approve)
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
            "no models match --models {}; run `neo --list-models` for supported catalog entries",
            model_scope.join(",")
        )
    })
}

fn apply_scoped_default_model(
    default_provider: &mut String,
    default_model: &mut String,
    model_catalogs: &[PathBuf],
    model_scope: &[String],
    explicit_model: bool,
) -> anyhow::Result<()> {
    if model_scope.is_empty() || explicit_model {
        return Ok(());
    }
    let scoped_default = scoped_default_model(model_catalogs, model_scope)
        .with_context(|| format!("failed to resolve --models {}", model_scope.join(",")))?;
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

pub fn show(config: &AppConfig) -> anyhow::Result<String> {
    let snapshot = FileConfig {
        default_model: Some(config.default_model.clone()),
        default_provider: Some(config.default_provider.clone()),
        api_base: config.api_base.clone(),
        api_key_env: config.api_key_env.clone(),
        providers: (!config.providers.is_empty()).then(|| config.providers.clone()),
        model_scope: (!config.model_scope.is_empty()).then(|| config.model_scope.clone()),
        model_catalogs: (!config.model_catalogs.is_empty()).then(|| config.model_catalogs.clone()),
        prompt_templates: (!config.configured_prompt_templates.is_empty())
            .then(|| config.configured_prompt_templates.clone()),
        sessions_dir: Some(config.sessions_dir.clone()),
        permissions: Some(config.permissions.clone()),
        defaults: Some(FileDefaults {
            mode: Some(config.defaults.mode.clone()),
        }),
        runtime: Some(FileRuntimeConfig::from_runtime(&config.runtime)),
        tui: Some(FileTuiConfig::from_tui(&config.tui)),
        mcp: Some(config.mcp.redacted()),
    };

    Ok(format!(
        "# path = {}\n{}\n",
        config.config_path.display(),
        toml::to_string_pretty(&snapshot)?
    ))
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

pub fn set(key: &str, value: &str) -> anyhow::Result<String> {
    let config_path = find_config_path()?;
    let mut config = read_file_config(&config_path)?;

    match key {
        "default_model" | "model" => config.default_model = Some(value.to_owned()),
        "default_provider" | "provider" => config.default_provider = Some(value.to_owned()),
        "api_base" => config.api_base = Some(value.to_owned()),
        "api_key_env" => config.api_key_env = Some(value.to_owned()),
        "model_scope" => config.model_scope = Some(parse_string_list(value)?),
        key if key.starts_with("providers.") && key.ends_with(".api_base") => {
            let provider_id = parse_provider_key(key, ".api_base")?;
            let provider = config
                .providers
                .get_or_insert_with(BTreeMap::new)
                .entry(provider_id.to_owned())
                .or_default();
            provider.api_base = Some(value.to_owned());
        }
        key if key.starts_with("providers.") && key.ends_with(".api_key_env") => {
            let provider_id = parse_provider_key(key, ".api_key_env")?;
            let provider = config
                .providers
                .get_or_insert_with(BTreeMap::new)
                .entry(provider_id.to_owned())
                .or_default();
            provider.api_key_env = Some(value.to_owned());
        }
        "sessions_dir" => config.sessions_dir = Some(PathBuf::from(value)),
        "prompt_templates" => {
            config.prompt_templates = Some(parse_string_list(value)?);
        }
        "permissions.file_read" | "file_read" => {
            let permissions = config
                .permissions
                .get_or_insert_with(PermissionPolicy::default);
            permissions.file_read = toml::from_str(&format!("\"{value}\""))?;
        }
        "permissions.file_write" | "file_write" => {
            let permissions = config
                .permissions
                .get_or_insert_with(PermissionPolicy::default);
            permissions.file_write = toml::from_str(&format!("\"{value}\""))?;
        }
        "permissions.shell" | "shell" => {
            let permissions = config
                .permissions
                .get_or_insert_with(PermissionPolicy::default);
            permissions.shell = toml::from_str(&format!("\"{value}\""))?;
        }
        "permissions.tool" | "tool" => {
            let permissions = config
                .permissions
                .get_or_insert_with(PermissionPolicy::default);
            permissions.tool = toml::from_str(&format!("\"{value}\""))?;
        }
        "defaults.mode" | "mode" => {
            let defaults = config.defaults.get_or_insert_with(FileDefaults::default);
            defaults.mode = Some(value.to_owned());
        }
        key if set_runtime_config(&mut config, key, value)? => {}
        key if set_tui_config(&mut config, key, value)? => {}
        unknown => bail!("unsupported config key: {unknown}"),
    }

    if let Some(runtime) = &config.runtime {
        validate_runtime_config(&runtime_from_file(Some(runtime.clone())))?;
    }
    if let Some(tui) = &config.tui {
        validate_tui_config(&tui_from_file(Some(tui.clone())))?;
    }
    write_file_config(&config_path, &config)?;
    Ok(format!("set {key}\n"))
}

fn set_runtime_config(config: &mut FileConfig, key: &str, value: &str) -> anyhow::Result<bool> {
    match key {
        "runtime.temperature" | "temperature" => {
            runtime_config_mut(config).temperature = Some(value.parse()?);
        }
        "runtime.max_tokens" | "max_tokens" => {
            runtime_config_mut(config).max_tokens = Some(value.parse()?);
        }
        "runtime.reasoning_effort" | "reasoning_effort" => {
            runtime_config_mut(config).reasoning_effort = Some(parse_reasoning_effort(value)?);
        }
        "runtime.steering_queue_mode" | "steering_queue_mode" => {
            runtime_config_mut(config).steering_queue_mode = Some(parse_queue_mode(value)?);
        }
        "runtime.follow_up_queue_mode" | "follow_up_queue_mode" => {
            runtime_config_mut(config).follow_up_queue_mode = Some(parse_queue_mode(value)?);
        }
        "runtime.tool_execution_mode" | "tool_execution_mode" => {
            runtime_config_mut(config).tool_execution_mode =
                Some(parse_tool_execution_mode(value)?);
        }
        "runtime.compaction.enabled" | "compaction.enabled" => {
            compaction_config_mut(config).enabled = Some(value.parse()?);
        }
        "runtime.compaction.max_estimated_tokens" | "compaction.max_estimated_tokens" => {
            compaction_config_mut(config).max_estimated_tokens = Some(value.parse()?);
        }
        "runtime.compaction.keep_recent_messages" | "compaction.keep_recent_messages" => {
            compaction_config_mut(config).keep_recent_messages = Some(value.parse()?);
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn set_tui_config(config: &mut FileConfig, key: &str, value: &str) -> anyhow::Result<bool> {
    match key {
        "tui.image_protocol" | "image_protocol" => {
            tui_config_mut(config).image_protocol = Some(parse_image_protocol(value)?);
            return Ok(true);
        }
        "tui.fetch_remote_images" | "fetch_remote_images" => {
            tui_config_mut(config).fetch_remote_images = Some(value.parse()?);
            return Ok(true);
        }
        _ => {}
    }

    let Some(action_id) = key.strip_prefix("tui.keybindings.") else {
        return Ok(false);
    };
    tui_config_mut(config)
        .keybindings
        .get_or_insert_with(BTreeMap::new)
        .insert(action_id.to_owned(), parse_string_list(value)?);
    Ok(true)
}

fn parse_provider_key<'a>(key: &'a str, suffix: &str) -> anyhow::Result<&'a str> {
    key.strip_prefix("providers.")
        .and_then(|key| key.strip_suffix(suffix))
        .filter(|provider_id| !provider_id.is_empty())
        .with_context(|| format!("invalid provider config key: {key}"))
}

fn merge_file_configs(base: FileConfig, layer: FileConfig) -> FileConfig {
    FileConfig {
        default_model: layer.default_model.or(base.default_model),
        default_provider: layer.default_provider.or(base.default_provider),
        api_base: layer.api_base.or(base.api_base),
        api_key_env: layer.api_key_env.or(base.api_key_env),
        providers: merge_provider_configs(base.providers, layer.providers),
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
        api_base: layer.api_base.or(base.api_base),
        api_key_env: layer.api_key_env.or(base.api_key_env),
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

fn apply_runtime_overrides(
    mut runtime: RuntimeConfig,
    thinking_override: Option<ThinkingLevel>,
) -> RuntimeConfig {
    if let Some(thinking) = thinking_override {
        runtime.reasoning_effort = reasoning_effort_from_thinking(thinking);
        if matches!(thinking, ThinkingLevel::Off) {
            runtime.replay_reasoning = false;
        }
    }
    runtime
}

const fn reasoning_effort_from_thinking(thinking: ThinkingLevel) -> Option<ReasoningEffort> {
    match thinking {
        ThinkingLevel::Off => None,
        ThinkingLevel::Minimal => Some(ReasoningEffort::Minimal),
        ThinkingLevel::Low => Some(ReasoningEffort::Low),
        ThinkingLevel::Medium => Some(ReasoningEffort::Medium),
        ThinkingLevel::High => Some(ReasoningEffort::High),
        ThinkingLevel::XHigh => Some(ReasoningEffort::XHigh),
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

fn runtime_config_mut(config: &mut FileConfig) -> &mut FileRuntimeConfig {
    config
        .runtime
        .get_or_insert_with(FileRuntimeConfig::default)
}

fn compaction_config_mut(config: &mut FileConfig) -> &mut FileRuntimeCompactionConfig {
    runtime_config_mut(config)
        .compaction
        .get_or_insert_with(FileRuntimeCompactionConfig::default)
}

fn tui_config_mut(config: &mut FileConfig) -> &mut FileTuiConfig {
    config.tui.get_or_insert_with(FileTuiConfig::default)
}

fn parse_queue_mode(value: &str) -> anyhow::Result<QueueMode> {
    match value {
        "All" => Ok(QueueMode::All),
        "OneAtATime" => Ok(QueueMode::OneAtATime),
        other => bail!("unsupported queue mode: {other}"),
    }
}

fn parse_tool_execution_mode(value: &str) -> anyhow::Result<ToolExecutionMode> {
    match value {
        "Sequential" => Ok(ToolExecutionMode::Sequential),
        "Parallel" => Ok(ToolExecutionMode::Parallel),
        other => bail!("unsupported tool execution mode: {other}"),
    }
}

fn parse_reasoning_effort(value: &str) -> anyhow::Result<ReasoningEffort> {
    match value {
        "minimal" | "Minimal" => Ok(ReasoningEffort::Minimal),
        "low" | "Low" => Ok(ReasoningEffort::Low),
        "medium" | "Medium" => Ok(ReasoningEffort::Medium),
        "high" | "High" => Ok(ReasoningEffort::High),
        "xhigh" | "XHigh" => Ok(ReasoningEffort::XHigh),
        other => bail!("unsupported reasoning effort: {other}"),
    }
}

fn parse_image_protocol(value: &str) -> anyhow::Result<ImageProtocolPreference> {
    #[derive(Deserialize)]
    struct ImageProtocolValue {
        value: ImageProtocolPreference,
    }

    toml::from_str::<ImageProtocolValue>(&format!("value = \"{value}\""))
        .map(|parsed| parsed.value)
        .with_context(|| format!("unsupported TUI image protocol: {value}"))
}

fn parse_string_list(value: &str) -> anyhow::Result<Vec<String>> {
    let trimmed = value.trim();
    if trimmed.starts_with('[') {
        #[derive(Deserialize)]
        struct StringListValue {
            value: Vec<String>,
        }

        return toml::from_str::<StringListValue>(&format!("value = {trimmed}"))
            .map(|parsed| parsed.value)
            .with_context(|| format!("failed to parse string list: {value}"));
    }
    Ok(vec![value.to_owned()])
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

fn find_config_path() -> anyhow::Result<PathBuf> {
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

fn read_file_config(path: &Path) -> anyhow::Result<FileConfig> {
    if !path.exists() {
        return Ok(FileConfig::default());
    }

    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    toml::from_str(&content).with_context(|| format!("failed to parse config {}", path.display()))
}

fn write_file_config(path: &Path, config: &FileConfig) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let content = toml::to_string_pretty(config)?;
    fs::write(path, content).with_context(|| format!("failed to write config {}", path.display()))
}
