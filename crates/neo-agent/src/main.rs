mod cli;
mod config;
mod config_ops;
mod extension_commands;
mod extension_tools;
mod modes;
mod prompt_templates;
mod resources;
mod rpc_mode;
mod themes;
mod trust;

use std::{
    fmt::Write as _,
    io::{self, IsTerminal as _, Read as _},
    path::{Component, Path, PathBuf},
};

use clap::Parser;

use anyhow::Context as _;
use serde_json::json;

use crate::{
    cli::{
        CatalogCommand, Cli, Command, ExtensionCommand, McpCommand, ModelCommand, ProviderCommand,
        SessionCommand,
    },
    config::{AppConfig, ConfigOverrides},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    color_eyre::install().ok();
    tracing_subscriber::fmt().with_target(false).try_init().ok();

    let cli = Cli::parse_from(std::env::args_os());
    let output = dispatch(cli).await?;
    print!("{output}");
    Ok(())
}

async fn dispatch(cli: Cli) -> anyhow::Result<String> {
    let config = AppConfig::load(ConfigOverrides::from_cli(&cli))?;

    // Migrate legacy sessions from {project_dir}/.neo/sessions/ to the new
    // workspace-scoped layout. Idempotent — no-op if already migrated.
    if let Err(error) = modes::session_migrate::migrate_legacy_sessions(&config) {
        tracing::warn!("session migration failed (continuing): {error:#}");
    }

    if cli.resume_picker && cli.command.is_some() {
        anyhow::bail!(
            "--resume/-r starts the interactive session picker and cannot be combined with a subcommand"
        );
    }

    let session_options = RunSessionOptions::from_cli(&cli);
    let interactive_options = modes::interactive::InteractiveOptions {
        verbose_startup: cli.verbose,
    };
    dispatch_command(
        cli.command,
        &config,
        session_options,
        cli.resume_picker,
        interactive_options,
    )
    .await
}

#[derive(Clone)]
struct RunSessionOptions {
    continue_latest: bool,
    no_session: bool,
}

impl RunSessionOptions {
    fn from_cli(cli: &Cli) -> Self {
        Self {
            continue_latest: cli.continue_latest,
            no_session: cli.no_session,
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn dispatch_command(
    command: Option<Command>,
    config: &AppConfig,
    session_options: RunSessionOptions,
    resume_picker: bool,
    interactive_options: modes::interactive::InteractiveOptions,
) -> anyhow::Result<String> {
    match command {
        Some(Command::Run { output, prompt }) => {
            let prompt = prepare_prompt(prompt, config)?;
            modes::run::execute(
                &prompt,
                config,
                output.unwrap_or_else(|| run_output_for_mode(config)),
                session_options.continue_latest,
                session_options.no_session,
            )
            .await
        }
        Some(Command::Resume { session_id }) => {
            if io::stdout().is_terminal() {
                let startup = match session_id {
                    Some(id) => modes::interactive::StartupAction::LoadSession(id),
                    None => modes::interactive::StartupAction::OpenSessionPicker,
                };
                Ok(modes::interactive::execute_tty_with_startup(
                    config,
                    startup,
                    interactive_options,
                )
                .await?
                .unwrap_or_default())
            } else if let Some(id) = session_id {
                let transcript = modes::sessions::transcript(&id, config).await?;
                Ok(format!("session {id}\n{transcript}"))
            } else {
                anyhow::bail!(
                    "`neo resume` requires a terminal; use `neo resume <session-id>` in scripts"
                )
            }
        }
        Some(Command::Sessions { command }) => match command {
            SessionCommand::List => modes::sessions::list(config),
            SessionCommand::Show { session_id } => modes::sessions::show(&session_id, config),
            SessionCommand::Rename { session_id, name } => {
                modes::sessions::rename(&session_id, &name, config)
            }
            SessionCommand::Fork { session_id, name } => {
                modes::sessions::fork(&session_id, name.as_deref(), config)
            }
            SessionCommand::Compact {
                session_id,
                keep_recent,
            } => modes::sessions::compact(&session_id, keep_recent, config).await,
            SessionCommand::ExportHtml { session_id } => {
                modes::sessions::export_html(&session_id, config).await
            }
            SessionCommand::ExportJson { session_id } => {
                modes::sessions::export_json(&session_id, config).await
            }
        },
        Some(Command::Extensions { command }) => dispatch_extensions(config, command).await,
        Some(Command::Models { command }) => match command {
            ModelCommand::List { json } => modes::run::list_configured_models(config, json),
            ModelCommand::Add {
                alias,
                provider,
                model,
                max_context_tokens,
                capabilities,
                display_name,
            } => config_ops::add_model(
                &config.config_path,
                &alias,
                config::ModelConfig {
                    provider,
                    model,
                    max_context_tokens,
                    max_output_tokens: None,
                    capabilities: if capabilities.is_empty() {
                        vec!["streaming".to_owned(), "tools".to_owned()]
                    } else {
                        capabilities
                    },
                    display_name,
                },
            ),
            ModelCommand::Remove { alias } => config_ops::remove_model(&config.config_path, &alias),
            ModelCommand::Set { alias } => {
                config_ops::set_default_model(&config.config_path, &alias)
            }
        },
        Some(Command::Provider { command }) => match command {
            ProviderCommand::List { json } => config_ops::list_providers(config, json),
            ProviderCommand::Add {
                provider_id,
                r#type,
                base_url,
                api_key,
                api_key_env,
            } => {
                let provider_type = r#type
                    .as_deref()
                    .map(|t| {
                        neo_ai::ApiType::from_config_str(t)
                            .ok_or_else(|| anyhow::anyhow!("unsupported provider type: {t}"))
                    })
                    .transpose()?;
                config_ops::add_provider(
                    &config.config_path,
                    &provider_id,
                    config::ProviderConfig {
                        provider_type,
                        base_url,
                        api_key,
                        api_key_env,
                        api_base: None,
                    },
                )
            }
            ProviderCommand::Remove { provider_id } => {
                config_ops::remove_provider(&config.config_path, &provider_id)
            }
            ProviderCommand::Catalog { command } => match command {
                CatalogCommand::List {
                    provider_id,
                    filter,
                    json,
                } => list_catalog_providers(provider_id.as_deref(), filter.as_deref(), json).await,
                CatalogCommand::Add {
                    provider_id,
                    api_key,
                    default_model,
                } => {
                    config_ops::catalog_add_provider(
                        &config.config_path,
                        &provider_id,
                        api_key.as_deref(),
                        default_model.as_deref(),
                    )
                    .await
                }
            },
        },
        Some(Command::Mcp { command }) => match command {
            McpCommand::List => Ok(modes::run::list_mcp(config).await),
            McpCommand::Add {
                mcp_name,
                r#type,
                command,
                url,
                env,
                headers,
                cwd,
                enabled_tools,
                disabled_tools,
                startup_timeout_ms,
                tool_timeout_ms,
                enable,
                disable,
            } => {
                let enabled = enable && !disable;
                Ok(modes::run::add_mcp_server(
                    mcp_name,
                    r#type,
                    command,
                    url,
                    env,
                    headers,
                    cwd,
                    enabled_tools,
                    disabled_tools,
                    startup_timeout_ms,
                    tool_timeout_ms,
                    enabled,
                    config,
                )
                .await?)
            }
            McpCommand::Del { mcp_name } => Ok(config::remove_mcp_server(&mcp_name)?),
            McpCommand::Disable { mcp_name } => {
                Ok(config::set_mcp_server_enabled(&mcp_name, false)?)
            }
            McpCommand::Enable { mcp_name } => Ok(config::set_mcp_server_enabled(&mcp_name, true)?),
        },
        Some(Command::Rpc) => rpc_mode::execute(config).await,
        None => {
            if config.defaults.mode.eq_ignore_ascii_case("rpc") {
                return rpc_mode::execute(config).await;
            }
            let startup = if resume_picker {
                modes::interactive::StartupAction::OpenSessionPicker
            } else {
                modes::interactive::StartupAction::None
            };
            Ok(
                modes::interactive::execute_tty_with_startup(config, startup, interactive_options)
                    .await?
                    .unwrap_or_default(),
            )
        }
    }
}

fn run_output_for_mode(config: &AppConfig) -> cli::RunOutput {
    if config.defaults.mode.eq_ignore_ascii_case("json") {
        cli::RunOutput::Json
    } else {
        cli::RunOutput::Events
    }
}

fn prepare_prompt(prompt: Vec<String>, config: &AppConfig) -> anyhow::Result<Vec<String>> {
    let prompt = prompt_templates::expand_prompt_template_args(
        prompt,
        &config.project_dir,
        config::global_prompts_dir().as_deref(),
        &config.prompt_templates,
        false,
    )?;
    let prompt = expand_prompt_files(prompt, &config.project_dir)?;
    prompt_with_piped_stdin(prompt)
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

/// Fetch and display providers from the models.dev catalog.
#[allow(clippy::too_many_lines)]
async fn list_catalog_providers(
    provider_id: Option<&str>,
    filter: Option<&str>,
    json: bool,
) -> anyhow::Result<String> {
    let catalog = neo_ai::catalog::fetch_catalog()
        .await
        .context("failed to fetch models.dev catalog")?;

    // If a specific provider is requested, show its models
    if let Some(pid) = provider_id {
        let entry = catalog
            .get(pid)
            .ok_or_else(|| anyhow::anyhow!("provider '{pid}' not found in models.dev catalog"))?;
        let wire = neo_ai::catalog::infer_api_type(entry);
        let name = entry.name.as_deref().unwrap_or(pid);
        let wire_str = wire.as_ref().map_or("unsupported", |t| t.as_config_str());

        let models = neo_ai::catalog::catalog_provider_models(entry);
        if json {
            let models_json: Vec<_> = models
                .iter()
                .map(|m| {
                    json!({
                        "id": m.id,
                        "name": m.name,
                        "max_context_tokens": m.max_context_tokens,
                        "capabilities": m.capabilities,
                    })
                })
                .collect();
            return Ok(serde_json::to_string_pretty(&json!({
                "id": pid,
                "name": name,
                "wire": wire_str,
                "models": models_json,
            }))? + "\n");
        }

        let mut out = format!("{name} ({pid})  wire={wire_str}  models={}\n", models.len());
        for m in &models {
            let ctx = m
                .max_context_tokens
                .map_or("?".to_owned(), |n| n.to_string());
            let caps = m.capabilities.join(",");
            let display = m.name.as_deref().unwrap_or(&m.id);
            let _ = writeln!(
                out,
                "  {id:<40} {display:<30} ctx={ctx:<10} [{caps}]",
                id = m.id
            );
        }
        return Ok(out);
    }

    // List all providers
    let mut entries: Vec<_> = catalog.values().collect();
    entries.sort_by_key(|e| e.id.clone());

    if json {
        let providers_json: Vec<_> = entries
            .iter()
            .filter_map(|entry| {
                if let Some(f) = filter {
                    let f_lower = f.to_ascii_lowercase();
                    let id_match = entry.id.to_ascii_lowercase().contains(&f_lower);
                    let name_match = entry
                        .name
                        .as_deref()
                        .is_some_and(|n| n.to_ascii_lowercase().contains(&f_lower));
                    if !id_match && !name_match {
                        return None;
                    }
                }
                let wire = neo_ai::catalog::infer_api_type(entry)?;
                Some(json!({
                    "id": entry.id,
                    "name": entry.name,
                    "wire": wire.as_config_str(),
                    "model_count": entry.models.len(),
                }))
            })
            .collect();
        return Ok(serde_json::to_string_pretty(&json!({ "providers": providers_json }))? + "\n");
    }

    let mut out = String::new();
    for entry in entries {
        // Apply filter
        if let Some(f) = filter {
            let f_lower = f.to_ascii_lowercase();
            let id_match = entry.id.to_ascii_lowercase().contains(&f_lower);
            let name_match = entry
                .name
                .as_deref()
                .is_some_and(|n| n.to_ascii_lowercase().contains(&f_lower));
            if !id_match && !name_match {
                continue;
            }
        }

        let wire = neo_ai::catalog::infer_api_type(entry);
        if wire.is_none() {
            continue; // Skip unsupported providers
        }
        let wire_str = wire.as_ref().map_or("?", |t| t.as_config_str());
        let name = entry.name.as_deref().unwrap_or(&entry.id);
        let model_count = entry.models.len();
        let _ = writeln!(
            out,
            "{id:<25} wire={wire_str:<18} models={model_count:<4} {name}",
            id = entry.id
        );
    }
    Ok(out)
}
