use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::cli::Cli;

const CONFIG_DIR: &str = ".neo";
const CONFIG_FILE: &str = "config.toml";
const DEFAULT_MODEL: &str = "fake";
const DEFAULT_MODE: &str = "interactive";

#[derive(Debug, Clone)]
pub struct ConfigOverrides {
    pub model: Option<String>,
    pub api_base: Option<String>,
    pub config_path: Option<PathBuf>,
}

impl ConfigOverrides {
    pub fn from_cli(cli: &Cli) -> Self {
        Self {
            model: cli.model.clone(),
            api_base: cli.api_base.clone(),
            config_path: cli.config.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub default_model: String,
    pub api_base: Option<String>,
    pub sessions_dir: PathBuf,
    pub defaults: Defaults,

    #[serde(skip)]
    pub config_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Defaults {
    pub mode: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FileConfig {
    default_model: Option<String>,
    api_base: Option<String>,
    sessions_dir: Option<PathBuf>,
    defaults: Option<FileDefaults>,
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
        let env_api_base = env::var("NEO_API_BASE").ok();
        let env_sessions_dir = env::var("NEO_SESSIONS_DIR").ok().map(PathBuf::from);
        let env_mode = env::var("NEO_MODE").ok();

        let default_model = overrides
            .model
            .or(env_model)
            .or(file_config.default_model)
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        let api_base = overrides.api_base.or(env_api_base).or(file_config.api_base);
        let sessions_dir = env_sessions_dir
            .or(file_config.sessions_dir)
            .unwrap_or_else(|| project_dir.join(CONFIG_DIR).join("sessions"));
        let mode = env_mode
            .or(file_config.defaults.and_then(|defaults| defaults.mode))
            .unwrap_or_else(|| DEFAULT_MODE.to_owned());

        Ok(Self {
            default_model,
            api_base,
            sessions_dir,
            defaults: Defaults { mode },
            config_path,
        })
    }
}

pub fn show(config: &AppConfig) -> anyhow::Result<String> {
    let snapshot = FileConfig {
        default_model: Some(config.default_model.clone()),
        api_base: config.api_base.clone(),
        sessions_dir: Some(config.sessions_dir.clone()),
        defaults: Some(FileDefaults {
            mode: Some(config.defaults.mode.clone()),
        }),
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
        "api_base" => config.api_base = Some(value.to_owned()),
        "sessions_dir" => config.sessions_dir = Some(PathBuf::from(value)),
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
