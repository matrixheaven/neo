mod cli;
mod config;
mod extension_commands;
mod modes;
mod rpc_mode;
mod session_commands;
mod skill_commands;

use std::{
    io::{self, IsTerminal as _, Read as _},
    path::{Component, Path, PathBuf},
};

use clap::Parser;

use anyhow::Context as _;

use crate::{
    cli::{
        Cli, Command, ConfigCommand, ExtensionCommand, McpCommand, ModelCommand, SessionCommand,
        SkillCommand,
    },
    config::{AppConfig, ConfigOverrides},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    color_eyre::install().ok();
    tracing_subscriber::fmt().with_target(false).try_init().ok();

    let cli = Cli::parse();
    let output = dispatch(cli).await?;
    print!("{output}");
    Ok(())
}

async fn dispatch(cli: Cli) -> anyhow::Result<String> {
    let config = AppConfig::load(ConfigOverrides::from_cli(&cli))?;

    match cli.command {
        Some(Command::Print { prompt }) => {
            let prompt = prepare_prompt(prompt, &config)?;
            modes::print::execute(&prompt, &config).await
        }
        Some(Command::Run { prompt }) => {
            let prompt = prepare_prompt(prompt, &config)?;
            modes::run::execute(&prompt, &config).await
        }
        Some(Command::Resume { session_id }) => modes::run::resume(&session_id, &config).await,
        Some(Command::Sessions { command }) => match command {
            SessionCommand::List => session_commands::list(&config),
            SessionCommand::Tree => session_commands::tree(&config),
            SessionCommand::Show { session_id } => session_commands::show(&session_id, &config),
            SessionCommand::Rename { session_id, name } => {
                session_commands::rename(&session_id, &name, &config)
            }
            SessionCommand::Fork { session_id, name } => {
                session_commands::fork(&session_id, name.as_deref(), &config)
            }
            SessionCommand::Summarize { session_id } => {
                session_commands::summarize(&session_id, &config).await
            }
            SessionCommand::Compact {
                session_id,
                keep_recent,
            } => session_commands::compact(&session_id, keep_recent, &config).await,
            SessionCommand::ExportHtml { session_id } => {
                session_commands::export_html(&session_id, &config).await
            }
        },
        Some(Command::Skills { command }) => match command {
            SkillCommand::Show { path } => skill_commands::show(&path),
        },
        Some(Command::Extensions { command }) => dispatch_extensions(&config, command).await,
        Some(Command::Config { command }) => match command {
            ConfigCommand::Show => config::show(&config),
            ConfigCommand::Set { key, value } => config::set(&key, &value),
        },
        Some(Command::Models { command }) => match command {
            ModelCommand::List => modes::run::list_models(&config),
        },
        Some(Command::Mcp { command }) => match command {
            McpCommand::List => Ok(modes::run::list_mcp_servers(&config)),
            McpCommand::Resources { server_id, command } => match command {
                cli::McpResourceCommand::List => {
                    modes::run::list_mcp_resources(&config, &server_id).await
                }
                cli::McpResourceCommand::Read { uri } => {
                    modes::run::read_mcp_resource(&config, &server_id, &uri).await
                }
                cli::McpResourceCommand::Watch { uri, count } => {
                    modes::run::watch_mcp_resource(&config, &server_id, &uri, count).await
                }
            },
        },
        Some(Command::Rpc) => rpc_mode::execute(&config).await,
        None => Ok(modes::interactive::execute_tty(&config)
            .await?
            .unwrap_or_default()),
    }
}

fn prepare_prompt(prompt: Vec<String>, config: &AppConfig) -> anyhow::Result<Vec<String>> {
    prompt_with_piped_stdin(expand_prompt_files(prompt, &config.project_dir)?)
}

fn expand_prompt_files(prompt: Vec<String>, project_dir: &Path) -> anyhow::Result<Vec<String>> {
    if prompt.is_empty() {
        return Ok(Vec::new());
    }

    let mut blocks = Vec::new();
    let mut text = Vec::new();
    for arg in prompt {
        let Some(path) = arg.strip_prefix('@').filter(|path| !path.is_empty()) else {
            text.push(arg);
            continue;
        };
        if !text.is_empty() {
            blocks.push(std::mem::take(&mut text).join(" "));
        }
        let content = read_prompt_file(project_dir, Path::new(path))?;
        if !content.is_empty() {
            blocks.push(content);
        }
    }
    if !text.is_empty() {
        blocks.push(text.join(" "));
    }
    if blocks.is_empty() {
        Ok(Vec::new())
    } else {
        Ok(vec![blocks.join("\n")])
    }
}

fn read_prompt_file(project_dir: &Path, path: &Path) -> anyhow::Result<String> {
    anyhow::ensure!(
        !path
            .components()
            .any(|component| matches!(component, Component::ParentDir)),
        "prompt file must stay inside project directory"
    );
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_dir.join(path)
    };
    let project_dir = project_dir.canonicalize().with_context(|| {
        format!(
            "failed to resolve project directory {}",
            project_dir.display()
        )
    })?;
    let candidate = candidate
        .canonicalize()
        .with_context(|| format!("failed to resolve prompt file {}", candidate.display()))?;
    anyhow::ensure!(
        candidate.starts_with(&project_dir),
        "prompt file must stay inside project directory"
    );
    anyhow::ensure!(
        candidate.is_file(),
        "prompt file must be a regular file: {}",
        candidate.display()
    );
    std::fs::read_to_string(&candidate)
        .map(|content| content.trim_end_matches(['\r', '\n']).to_owned())
        .with_context(|| format!("failed to read prompt file {}", candidate.display()))
}

fn prompt_with_piped_stdin(prompt: Vec<String>) -> anyhow::Result<Vec<String>> {
    let mut stdin = io::stdin();
    if stdin.is_terminal() {
        return Ok(prompt);
    }

    let mut piped = String::new();
    stdin.read_to_string(&mut piped)?;
    if !piped.is_empty() {
        let piped = piped.trim_end_matches(['\r', '\n']).to_owned();
        if !piped.is_empty() {
            if prompt.is_empty() {
                return Ok(vec![piped]);
            }
            return Ok(vec![format!("{piped}\n{}", prompt.join(" "))]);
        }
    }
    Ok(prompt)
}

async fn dispatch_extensions(
    config: &AppConfig,
    command: ExtensionCommand,
) -> anyhow::Result<String> {
    match command {
        ExtensionCommand::List { root } => {
            let paths = extension_paths(config, root);
            extension_commands::list(&paths.root, &paths.state_path, &paths.registry_path)
        }
        ExtensionCommand::Install { source, root } => {
            let paths = extension_paths(config, root);
            extension_commands::install(
                &paths.root,
                &paths.state_path,
                &paths.registry_path,
                &source,
            )
        }
        ExtensionCommand::Update { extension_id, root } => {
            let paths = extension_paths(config, root);
            extension_commands::update(
                &paths.root,
                &paths.state_path,
                &paths.registry_path,
                &extension_id,
            )
        }
        ExtensionCommand::Uninstall { extension_id, root } => {
            let paths = extension_paths(config, root);
            extension_commands::uninstall(
                &paths.root,
                &paths.state_path,
                &paths.registry_path,
                &extension_id,
            )
        }
        ExtensionCommand::Status { extension_id, root } => {
            let paths = extension_paths(config, root);
            extension_commands::status(&paths.root, &paths.state_path, &extension_id)
        }
        ExtensionCommand::Enable { extension_id, root } => {
            let paths = extension_paths(config, root);
            extension_commands::enable(&paths.root, &paths.state_path, &extension_id)
        }
        ExtensionCommand::Disable { extension_id, root } => {
            let paths = extension_paths(config, root);
            extension_commands::disable(&paths.root, &paths.state_path, &extension_id)
        }
        ExtensionCommand::Call {
            extension_id,
            method,
            params,
            root,
        } => {
            let paths = extension_paths(config, root);
            extension_commands::call(
                &paths.root,
                &paths.state_path,
                &extension_id,
                &method,
                &params,
            )
            .await
        }
    }
}

struct ExtensionPaths {
    root: PathBuf,
    state_path: PathBuf,
    registry_path: PathBuf,
}

fn extension_paths(config: &AppConfig, root: PathBuf) -> ExtensionPaths {
    let project_neo_dir = config.project_dir.join(".neo");
    ExtensionPaths {
        root: resolve_default_extension_root(config, root),
        state_path: project_neo_dir.join("extensions-state.toml"),
        registry_path: project_neo_dir.join("extensions-sources.toml"),
    }
}

fn resolve_default_extension_root(config: &AppConfig, root: PathBuf) -> PathBuf {
    if root == Path::new(".neo/extensions") {
        config.project_dir.join(root)
    } else {
        root
    }
}
