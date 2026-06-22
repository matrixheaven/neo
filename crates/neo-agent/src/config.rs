use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use anyhow::Context;
use neo_agent_core::session::workspace_sessions_dir as compute_workspace_sessions_dir;
use neo_agent_core::{PermissionMode, QueueMode, ToolExecutionMode};
use neo_ai::{ModelSpec, ReasoningEffort};
use neo_tui::{
    image::ImageProtocolPreference,
    input::{KeyId, KeybindingAction, KeybindingsManager},
};
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

fn deserialize_string_or_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{Deserialize, SeqAccess, Visitor};
    struct StringOrVec;

    impl<'de> Visitor<'de> for StringOrVec {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or a list of strings")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(vec![value.to_owned()])
        }

        fn visit_seq<A>(self, seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            Vec::<String>::deserialize(serde::de::value::SeqAccessDeserializer::new(seq))
        }
    }

    deserializer.deserialize_any(StringOrVec)
}

#[derive(Debug, Clone, Default)]
pub struct ConfigOverrides {
    pub config_path: Option<PathBuf>,
    pub yolo: bool,
    pub auto: bool,
    pub(crate) trust_store: Option<trust::ProjectTrustStore>,
    pub(crate) project_dir: Option<PathBuf>,
}

impl ConfigOverrides {
    pub fn from_cli(cli: &Cli) -> Self {
        Self {
            config_path: cli.config.clone(),
            yolo: cli.yolo,
            auto: cli.auto,
            trust_store: None,
            project_dir: None,
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
    let mut row = wildcard_initial_row(&pattern);
    for character in text.chars() {
        row = wildcard_advance_row(&pattern, &row, character);
    }
    row.last().copied().unwrap_or(false)
}

fn wildcard_initial_row(pattern: &[char]) -> Vec<bool> {
    let mut row = vec![false; pattern.len() + 1];
    row[0] = true;
    for (index, character) in pattern.iter().enumerate() {
        row[index + 1] = row[index] && *character == '*';
    }
    row
}

fn wildcard_advance_row(pattern: &[char], previous: &[bool], text_character: char) -> Vec<bool> {
    let mut current = vec![false; pattern.len() + 1];
    for (index, pattern_character) in pattern.iter().copied().enumerate() {
        current[index + 1] = wildcard_cell_matches(
            pattern_character,
            text_character,
            previous[index],
            previous[index + 1],
            current[index],
        );
    }
    current
}

fn wildcard_cell_matches(
    pattern_character: char,
    text_character: char,
    diagonal: bool,
    previous_row: bool,
    current_row: bool,
) -> bool {
    match pattern_character {
        '*' => previous_row || current_row,
        '?' => diagonal,
        literal => diagonal && literal == text_character,
    }
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
    pub api_key_env: Option<String>,
    pub providers: BTreeMap<String, ProviderConfig>,
    /// Models defined inline in config.toml `[models.<alias>]`.
    pub models: BTreeMap<String, ModelConfig>,
    #[serde(skip)]
    pub model_scope: Vec<String>,
    pub sessions_dir: PathBuf,
    pub permission_mode: PermissionMode,
    /// Shared live permission state for the interactive TUI. Updated by
    /// `/ask`, `/auto`, `/yolo` (and `/permissions`) even while a turn is
    /// running, and read at every tool-call approval so the active turn honors
    /// the latest mode without needing to be cancelled. Seeded from
    /// `permission_mode` at construction.
    #[serde(skip)]
    pub live_permission_mode: Arc<RwLock<PermissionMode>>,
    pub defaults: Defaults,
    pub runtime: RuntimeConfig,
    pub tui: TuiConfig,
    #[serde(skip)]
    pub theme: ResolvedTheme,
    pub mcp: McpConfig,
    #[serde(skip)]
    pub prompt_templates: Vec<String>,
    #[serde(skip)]
    pub extra_skill_dirs: Vec<String>,
    #[serde(skip)]
    pub skill_path: Vec<String>,
    #[serde(skip)]
    pub project_trusted: bool,
    #[serde(skip)]
    pub project_trust: trust::ProjectTrustState,
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
    pub(crate) api_key_env: Option<String>,
    pub(crate) providers: Option<BTreeMap<String, ProviderConfig>>,
    /// Models defined inline via `[models.<alias>]` tables.
    pub(crate) models: Option<BTreeMap<String, ModelConfig>>,
    pub(crate) model_scope: Option<Vec<String>>,
    pub(crate) prompt_templates: Option<Vec<String>>,
    pub(crate) extra_skill_dirs: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_string_or_vec")]
    pub(crate) skill_path: Vec<String>,
    pub(crate) sessions_dir: Option<PathBuf>,
    pub(crate) permission_mode: Option<PermissionMode>,
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
        // There is exactly one config file: `~/.neo/config.toml` (or wherever
        // `NEO_HOME` points). There is no project-local config anymore —
        // providers/models/settings/skills/prompts/themes all live under the
        // single neo home and are shared across every workspace.
        let config_path = overrides.config_path.unwrap_or_else(default_config_path);
        // `project_dir` is the *workspace identity* (used for trust keying,
        // session bucketing, git status, `@file` sandboxing). It is NOT a config
        // location. Default to the current working directory.
        let project_dir = overrides
            .project_dir
            .map(Ok)
            .unwrap_or_else(env::current_dir)?;

        let file_config = read_file_config(&config_path)?;
        let (project_trusted, project_trust) =
            resolve_project_trust_state(&project_dir, overrides.yolo, overrides.trust_store)?;
        anyhow::ensure!(
            !(overrides.yolo && overrides.auto),
            "--yolo and --auto cannot be used together"
        );

        let default_model = file_config
            .default_model
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        let default_provider = file_config
            .default_provider
            .unwrap_or_else(|| DEFAULT_PROVIDER.to_owned());
        let providers = file_config.providers.unwrap_or_default();
        let models = file_config.models.unwrap_or_default();
        let api_key_env = file_config
            .api_key_env
            .or_else(|| provider_api_key_env(&providers, &default_provider));
        let model_scope = file_config.model_scope.unwrap_or_default();
        let prompt_templates = file_config.prompt_templates.unwrap_or_default();
        let extra_skill_dirs = file_config.extra_skill_dirs.unwrap_or_default();
        let skill_path = file_config.skill_path;
        let sessions_dir = file_config.sessions_dir.map_or_else(
            || {
                neo_home().map_or_else(
                    || project_dir.join("sessions"),
                    |home| home.join("sessions"),
                )
            },
            expand_user_path,
        );
        let permission_mode = if overrides.yolo {
            PermissionMode::Yolo
        } else if overrides.auto {
            PermissionMode::Auto
        } else {
            file_config.permission_mode.unwrap_or_default()
        };
        let runtime = runtime_from_file(file_config.runtime);
        validate_runtime_config(&runtime)?;
        let tui = tui_from_file(file_config.tui);
        validate_tui_config(&tui)?;
        let theme = themes::resolve_theme()?;
        let mcp = file_config.mcp.unwrap_or_default();
        let mode = file_config
            .defaults
            .and_then(|defaults| defaults.mode)
            .unwrap_or_else(|| DEFAULT_MODE.to_owned());

        Ok(Self {
            default_model,
            default_provider,
            api_key_env,
            providers,
            models,
            model_scope,
            sessions_dir,
            permission_mode,
            live_permission_mode: Arc::new(RwLock::new(permission_mode)),
            defaults: Defaults { mode },
            runtime,
            tui,
            theme,
            mcp,
            prompt_templates,
            extra_skill_dirs,
            skill_path,
            project_trusted,
            project_trust,
            project_dir,
            config_path,
        })
    }

    /// The canonical `provider/model` display label for the configured default
    /// model. This is the single source of truth for label formatting.
    ///
    /// `default_model` stores the model alias. If that alias exists in
    /// `[models.*]`, the label is derived from the referenced provider/model.
    /// Otherwise built-in bare model ids such as `gpt-4.1` are prefixed with
    /// `default_provider`, while already-qualified values are used as-is.
    #[must_use]
    pub fn default_model_label(&self) -> String {
        if let Some(model) = self.models.get(&self.default_model) {
            return format!("{}/{}", model.provider, model.model);
        }
        if self.default_model.contains('/') {
            self.default_model.clone()
        } else {
            format!("{}/{}", self.default_provider, self.default_model)
        }
    }
}

fn resolve_project_trust_state(
    project_dir: &Path,
    yolo: bool,
    trust_store: Option<trust::ProjectTrustStore>,
) -> anyhow::Result<(bool, trust::ProjectTrustState)> {
    let project_dir = project_dir.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize project dir {}",
            project_dir.display()
        )
    })?;

    if yolo {
        return Ok((false, trust::ProjectTrustState::NotRequired));
    }

    let inputs = trust::collect_project_trust_inputs(&project_dir)?;
    if inputs.detected.is_empty() && inputs.parent_candidates.is_empty() {
        return Ok((true, trust::ProjectTrustState::NotRequired));
    }

    let store = trust_store
        .map(Ok)
        .unwrap_or_else(trust::ProjectTrustStore::from_home)?;
    match trust::resolve_project_trust_decision(&project_dir, false, &store)? {
        trust::ProjectTrustDecision::Trusted { source } => Ok((
            true,
            trust::ProjectTrustState::Trusted {
                target: source.target(&project_dir),
            },
        )),
        trust::ProjectTrustDecision::Untrusted { source } => Ok((
            false,
            trust::ProjectTrustState::Untrusted {
                target: source.target(&project_dir),
            },
        )),
        trust::ProjectTrustDecision::Unknown { inputs } => {
            Ok((false, trust::ProjectTrustState::Unknown { inputs }))
        }
    }
}

fn provider_api_key_env(
    providers: &BTreeMap<String, ProviderConfig>,
    provider_id: &str,
) -> Option<String> {
    providers
        .get(provider_id)
        .and_then(|provider| provider.api_key_env.clone())
}

pub fn upsert_mcp_server(server: &McpServerConfig, config_path: &Path) -> anyhow::Result<String> {
    validate_mcp_server(server)?;
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

/// Resolve the neo home directory: `$NEO_HOME` if set, otherwise `~/.neo`.
/// This is the single source of truth for the neo home directory — every
/// config file, skill, prompt, theme, and extension lives under here.
pub(crate) fn neo_home() -> Option<PathBuf> {
    env::var_os("NEO_HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .map(|home| PathBuf::from(home).join(CONFIG_DIR))
        })
}

/// The single config file path: `<neo_home>/config.toml`.
pub(crate) fn default_config_path() -> PathBuf {
    neo_home()
        .unwrap_or_else(|| PathBuf::from(".").join(CONFIG_DIR))
        .join(CONFIG_FILE)
}

pub(crate) fn global_prompts_dir() -> Option<PathBuf> {
    neo_home().map(|home| home.join("prompts"))
}

/// Compute the workspace-scoped sessions directory for a given config.
///
/// Given the central `sessions_dir` (e.g. `~/.neo/sessions`) and the
/// project directory, returns `<sessions_dir>/wd_<slug>_<hash12>/`.
pub(crate) fn workspace_sessions_dir(config: &AppConfig) -> PathBuf {
    compute_workspace_sessions_dir(&config.sessions_dir, &config.project_dir)
}

fn expand_user_path(path: PathBuf) -> PathBuf {
    expand_user_path_with_home(path, user_home().as_deref())
}

fn expand_user_path_with_home(path: PathBuf, home: Option<&Path>) -> PathBuf {
    let Some(raw) = path.to_str().map(str::to_owned) else {
        return path;
    };
    if raw == "~" {
        return home.map(Path::to_path_buf).unwrap_or(path);
    }
    let Some(rest) = raw.strip_prefix("~/") else {
        return path;
    };
    home.map_or(path, |home| home.join(rest))
}

fn user_home() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
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

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use neo_ai::{ApiKind, ModelCapabilities, ModelSpec, ProviderId};
    use tempfile::TempDir;

    use crate::config::{AppConfig, ConfigOverrides, PermissionMode};
    use crate::trust::{ProjectTrustState, ProjectTrustStore};

    fn temp_project_config(content: &str) -> (TempDir, PathBuf, PathBuf) {
        let temp = TempDir::new().expect("temp dir");
        let config_path = temp.path().join("config.toml");
        fs::write(&config_path, content).expect("write config");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project");
        (temp, config_path, project_dir)
    }

    fn load_config(config_path: PathBuf, project_dir: PathBuf) -> AppConfig {
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
    fn config_defaults_to_ask_permission_mode() {
        let (_temp, config_path, project_dir) = temp_project_config("");
        let config = load_config(config_path, project_dir);
        assert_eq!(config.permission_mode, PermissionMode::Ask);
    }

    /// Regression: the model display label must never stitch the provider onto
    /// an already-qualified alias. When `default_model` is a `<provider>/<model>`
    /// alias, `default_model_label()` returns it as-is; otherwise it prefixes
    /// `default_provider/`. This avoids both
    /// `deepseek/minimax-.../MiniMax-M2` (stale provider) and
    /// `minimax-.../minimax-.../MiniMax-M2` (double prefix).
    #[test]
    fn default_model_label_never_double_prefixes() {
        let (_temp, config_path, project_dir) = temp_project_config(
            r#"
default_model = "minimax-cn-coding-plan/MiniMax-M2"
default_provider = "deepseek"

[providers.deepseek]
type = "openai-chat"
base_url = "https://deepseek.example/v1"

[providers."minimax-cn-coding-plan"]
type = "anthropic"
base_url = "https://api.minimaxi.com/anthropic/v1"

[models."minimax-cn-coding-plan/MiniMax-M2"]
provider = "minimax-cn-coding-plan"
model = "MiniMax-M2"
"#,
        );
        let config = load_config(config_path, project_dir);
        // Alias is used as-is: no stale deepseek prefix, no double minimax prefix.
        assert_eq!(
            config.default_model_label(),
            "minimax-cn-coding-plan/MiniMax-M2"
        );
    }

    /// When `default_model` has no `/` (a plain model id), the label prefixes
    /// `default_provider/`.
    #[test]
    fn default_model_label_prefixes_provider_for_plain_model_id() {
        let (_temp, config_path, project_dir) = temp_project_config(
            r#"
default_model = "deepseek-v4-pro"
default_provider = "deepseek"

[providers.deepseek]
type = "openai-chat"
base_url = "https://deepseek.example/v1"
"#,
        );
        let config = load_config(config_path, project_dir);
        assert_eq!(config.default_model_label(), "deepseek/deepseek-v4-pro");
    }

    #[test]
    fn default_model_label_resolves_unqualified_alias() {
        let (_temp, config_path, project_dir) = temp_project_config(
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
        let config = load_config(config_path, project_dir);
        assert_eq!(config.default_model_label(), "openai/gpt-4.1");
    }

    #[test]
    fn tilde_expansion_uses_user_home_semantics() {
        let home = PathBuf::from("/home/alice");

        assert_eq!(
            super::expand_user_path_with_home(PathBuf::from("~/neo-sessions"), Some(&home)),
            PathBuf::from("/home/alice/neo-sessions")
        );
        assert_eq!(
            super::expand_user_path_with_home(PathBuf::from("relative/path"), Some(&home)),
            PathBuf::from("relative/path")
        );
    }

    #[test]
    fn config_loads_permission_mode_auto() {
        let (_temp, config_path, project_dir) = temp_project_config("permission_mode = \"auto\"\n");
        let config = load_config(config_path, project_dir);
        assert_eq!(config.permission_mode, PermissionMode::Auto);
    }

    #[test]
    fn cli_yolo_overrides_config_permission_mode() {
        let (_temp, config_path, project_dir) = temp_project_config("permission_mode = \"ask\"\n");
        let config = AppConfig::load(ConfigOverrides {
            config_path: Some(config_path),
            yolo: true,
            auto: false,
            trust_store: None,
            project_dir: Some(project_dir),
        })
        .expect("load config");
        assert_eq!(config.permission_mode, PermissionMode::Yolo);
    }

    #[test]
    fn cli_auto_overrides_config_permission_mode() {
        let (_temp, config_path, project_dir) = temp_project_config("permission_mode = \"ask\"\n");
        let config = AppConfig::load(ConfigOverrides {
            config_path: Some(config_path),
            yolo: false,
            auto: true,
            trust_store: None,
            project_dir: Some(project_dir),
        })
        .expect("load config");
        assert_eq!(config.permission_mode, PermissionMode::Auto);
    }

    #[test]
    fn scoped_models_matches_globs_against_qualified_and_model_ids() {
        let openai = ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: "gpt-4.1".to_owned(),
            api: ApiKind::OpenAiResponses,
            capabilities: ModelCapabilities::tool_chat(),
        };
        let anthropic = ModelSpec {
            provider: ProviderId("anthropic".to_owned()),
            model: "claude-sonnet-4".to_owned(),
            api: ApiKind::AnthropicMessages,
            capabilities: ModelCapabilities::tool_chat(),
        };

        let models = [openai, anthropic];
        let scoped = super::scoped_models(
            models.iter(),
            &["openai/gpt-*".to_owned(), "claude-??????-4:high".to_owned()],
        );

        assert_eq!(
            scoped
                .iter()
                .map(|model| format!("{}/{}", model.provider.0, model.model))
                .collect::<Vec<_>>(),
            vec!["openai/gpt-4.1", "anthropic/claude-sonnet-4"]
        );
    }

    fn load_config_with_store(
        config_path: PathBuf,
        project_dir: PathBuf,
        store: ProjectTrustStore,
    ) -> AppConfig {
        AppConfig::load(ConfigOverrides {
            config_path: Some(config_path),
            yolo: false,
            auto: false,
            trust_store: Some(store),
            project_dir: Some(project_dir),
        })
        .expect("load config")
    }

    #[test]
    fn config_trust_is_not_required_for_directory_without_inputs() {
        let (_temp, config_path, project_dir) = temp_project_config("");
        let config = load_config(config_path, project_dir.clone());
        assert!(config.project_trusted);
        assert_eq!(config.project_trust, ProjectTrustState::NotRequired);
    }

    #[test]
    fn config_trust_is_unknown_when_inputs_exist_without_decision() {
        let (_temp, config_path, project_dir) = temp_project_config("");
        fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");
        let store = ProjectTrustStore::new(_temp.path().join("trust.json"));

        let config = load_config_with_store(config_path, project_dir.clone(), store);

        assert!(!config.project_trusted);
        assert!(matches!(
            config.project_trust,
            ProjectTrustState::Unknown { .. }
        ));
    }

    #[test]
    fn config_trust_is_trusted_when_store_approves_current_dir() {
        let (_temp, config_path, project_dir) = temp_project_config("");
        fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");
        let store = ProjectTrustStore::new(_temp.path().join("trust.json"));
        store.set(&project_dir, Some(true)).expect("approve");

        let config = load_config_with_store(config_path, project_dir.clone(), store);

        assert!(config.project_trusted);
        assert_eq!(
            config.project_trust,
            ProjectTrustState::Trusted {
                target: project_dir.canonicalize().expect("canonicalize"),
            }
        );
    }

    #[test]
    fn config_trust_is_untrusted_when_store_denies_current_dir() {
        let (_temp, config_path, project_dir) = temp_project_config("");
        fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");
        let store = ProjectTrustStore::new(_temp.path().join("trust.json"));
        store.set(&project_dir, Some(false)).expect("deny");

        let config = load_config_with_store(config_path, project_dir.clone(), store);

        assert!(!config.project_trusted);
        assert_eq!(
            config.project_trust,
            ProjectTrustState::Untrusted {
                target: project_dir.canonicalize().expect("canonicalize"),
            }
        );
    }

    #[test]
    fn config_yolo_sets_not_required_and_untrusted() {
        let (_temp, config_path, project_dir) = temp_project_config("");
        fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");

        let config = AppConfig::load(ConfigOverrides {
            config_path: Some(config_path),
            yolo: true,
            auto: false,
            trust_store: None,
            project_dir: Some(project_dir),
        })
        .expect("load config");

        assert!(!config.project_trusted);
        assert_eq!(config.project_trust, ProjectTrustState::NotRequired);
    }
}
