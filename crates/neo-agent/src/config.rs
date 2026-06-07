use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use neo_agent_core::PermissionPolicy;
use serde::{Deserialize, Serialize};

use crate::cli::Cli;

const CONFIG_DIR: &str = ".neo";
const CONFIG_FILE: &str = "config.toml";
const DEFAULT_MODEL: &str = "gpt-4.1";
const DEFAULT_PROVIDER: &str = "openai";
const DEFAULT_MODE: &str = "interactive";

#[derive(Debug, Clone)]
pub struct ConfigOverrides {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub api_base: Option<String>,
    pub config_path: Option<PathBuf>,
    pub approve: bool,
    pub no_approve: bool,
}

impl ConfigOverrides {
    pub fn from_cli(cli: &Cli) -> Self {
        Self {
            model: cli.model.clone(),
            provider: cli.provider.clone(),
            api_base: cli.api_base.clone(),
            config_path: cli.config.clone(),
            approve: cli.approve,
            no_approve: cli.no_approve,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub default_model: String,
    pub default_provider: String,
    pub api_base: Option<String>,
    pub api_key_env: Option<String>,
    pub providers: BTreeMap<String, ProviderConfig>,
    pub sessions_dir: PathBuf,
    pub permissions: PermissionPolicy,
    pub defaults: Defaults,
    pub mcp: McpConfig,
    pub approve: bool,
    pub no_approve: bool,
    pub project_dir: PathBuf,

    #[serde(skip)]
    pub config_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Defaults {
    pub mode: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub api_key_env: Option<String>,
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
}

const fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FileConfig {
    default_model: Option<String>,
    default_provider: Option<String>,
    api_base: Option<String>,
    api_key_env: Option<String>,
    providers: Option<BTreeMap<String, ProviderConfig>>,
    sessions_dir: Option<PathBuf>,
    permissions: Option<PermissionPolicy>,
    defaults: Option<FileDefaults>,
    mcp: Option<McpConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FileDefaults {
    mode: Option<String>,
}

impl AppConfig {
    pub fn load(overrides: ConfigOverrides) -> anyhow::Result<Self> {
        let config_path = overrides.config_path.unwrap_or(find_config_path()?);
        let project_dir = config_path
            .parent()
            .and_then(Path::parent)
            .map_or(env::current_dir()?, Path::to_path_buf);

        let file_config = read_file_config(&config_path)?;
        let env_model = env::var("NEO_MODEL").ok();
        let env_provider = env::var("NEO_PROVIDER").ok();
        let env_api_base = env::var("NEO_API_BASE").ok();
        let env_api_key = env::var("NEO_API_KEY_ENV").ok();
        let env_sessions_dir = env::var("NEO_SESSIONS_DIR").ok().map(PathBuf::from);
        let env_mode = env::var("NEO_MODE").ok();

        let default_model = overrides
            .model
            .or(env_model)
            .or(file_config.default_model)
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        let default_provider = overrides
            .provider
            .or(env_provider)
            .or(file_config.default_provider)
            .unwrap_or_else(|| DEFAULT_PROVIDER.to_owned());
        let providers = file_config.providers.unwrap_or_default();
        let api_base = overrides.api_base.or(env_api_base).or(file_config.api_base);
        let api_key_env = env_api_key
            .or(file_config.api_key_env)
            .or_else(|| provider_api_key_env(&providers, &default_provider));
        let sessions_dir = env_sessions_dir
            .or(file_config.sessions_dir)
            .unwrap_or_else(|| project_dir.join(CONFIG_DIR).join("sessions"));
        let permissions = file_config.permissions.unwrap_or_default();
        let mcp = file_config.mcp.unwrap_or_default();
        let mode = env_mode
            .or(file_config.defaults.and_then(|defaults| defaults.mode))
            .unwrap_or_else(|| DEFAULT_MODE.to_owned());

        Ok(Self {
            default_model,
            default_provider,
            api_base,
            api_key_env,
            providers,
            sessions_dir,
            permissions,
            defaults: Defaults { mode },
            mcp,
            approve: overrides.approve,
            no_approve: overrides.no_approve,
            project_dir,
            config_path,
        })
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

pub fn show(config: &AppConfig) -> anyhow::Result<String> {
    let snapshot = FileConfig {
        default_model: Some(config.default_model.clone()),
        default_provider: Some(config.default_provider.clone()),
        api_base: config.api_base.clone(),
        api_key_env: config.api_key_env.clone(),
        providers: (!config.providers.is_empty()).then(|| config.providers.clone()),
        sessions_dir: Some(config.sessions_dir.clone()),
        permissions: Some(config.permissions.clone()),
        defaults: Some(FileDefaults {
            mode: Some(config.defaults.mode.clone()),
        }),
        mcp: Some(config.mcp.clone()),
    };

    Ok(format!(
        "# path = {}\n{}\n",
        config.config_path.display(),
        toml::to_string_pretty(&snapshot)?
    ))
}

pub fn set(key: &str, value: &str) -> anyhow::Result<String> {
    let config_path = find_config_path()?;
    let mut config = read_file_config(&config_path)?;

    match key {
        "default_model" | "model" => config.default_model = Some(value.to_owned()),
        "default_provider" | "provider" => config.default_provider = Some(value.to_owned()),
        "api_base" => config.api_base = Some(value.to_owned()),
        "api_key_env" => config.api_key_env = Some(value.to_owned()),
        key if key.starts_with("providers.") && key.ends_with(".api_key_env") => {
            let provider_id = key
                .strip_prefix("providers.")
                .and_then(|key| key.strip_suffix(".api_key_env"))
                .filter(|provider_id| !provider_id.is_empty())
                .with_context(|| format!("invalid provider config key: {key}"))?;
            let provider = config
                .providers
                .get_or_insert_with(BTreeMap::new)
                .entry(provider_id.to_owned())
                .or_default();
            provider.api_key_env = Some(value.to_owned());
        }
        "sessions_dir" => config.sessions_dir = Some(PathBuf::from(value)),
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
        unknown => bail!("unsupported config key: {unknown}"),
    }

    write_file_config(&config_path, &config)?;
    Ok(format!("set {key}\n"))
}

fn find_config_path() -> anyhow::Result<PathBuf> {
    Ok(env::current_dir()?.join(CONFIG_DIR).join(CONFIG_FILE))
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
