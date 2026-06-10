mod cli;
mod cloud_commands;
mod config;
mod extension_commands;
mod extension_tools;
mod modes;
mod package_commands;
mod prompt_templates;
mod resources;
mod rpc_mode;
mod session_cloud;
mod session_commands;
mod skill_commands;
mod themes;
mod trust;

use std::{
    io::{self, IsTerminal as _, Read as _},
    path::{Component, Path, PathBuf},
};

use clap::Parser;

use anyhow::Context as _;

use crate::{
    cli::{
        AuthCommand, Cli, CloudCommand, Command, ConfigCommand, ConfigSyncCommand,
        ExtensionCommand, ImageCommand, LIST_MODELS_NO_SEARCH, LoginCommand, McpCommand,
        ModelCommand, PackageSource, PromptPackageCommand, SessionCommand, SkillCommand,
        ThemePackageCommand, TrustCommand,
    },
    config::{AppConfig, ConfigOverrides},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    color_eyre::install().ok();
    tracing_subscriber::fmt().with_target(false).try_init().ok();

    let cli = Cli::parse_from(normalize_pi_style_args(std::env::args_os()));
    let output = dispatch(cli).await?;
    print!("{output}");
    Ok(())
}

fn normalize_pi_style_args(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> Vec<std::ffi::OsString> {
    args.into_iter()
        .map(|arg| match arg.to_str() {
            Some("--print" | "-p") => "print".into(),
            Some("-na") => "--no-approve".into(),
            Some("-nt") => "--no-tools".into(),
            Some("-nbt") => "--no-builtin-tools".into(),
            Some("-np") => "--no-prompt-templates".into(),
            Some("-nc") => "--no-context-files".into(),
            Some("-ns") => "--no-skills".into(),
            Some("-ne") => "--no-extensions".into(),
            Some("-xt") => "--exclude-tools".into(),
            _ => arg,
        })
        .collect()
}

async fn dispatch(cli: Cli) -> anyhow::Result<String> {
    let config = AppConfig::load(ConfigOverrides::from_cli(&cli))?;
    if !cli.export.is_empty() {
        return dispatch_export(&cli.export).await;
    }
    if let Some(search) = &cli.list_models {
        let search = search.trim();
        let search =
            (!search.is_empty() && search != LIST_MODELS_NO_SEARCH && !search.starts_with('@'))
                .then_some(search);
        return modes::run::list_models_filtered(&config, search);
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
    session_id: Option<String>,
    session: Option<String>,
    continue_latest: bool,
    fork: Option<String>,
    name: Option<String>,
    no_session: bool,
}

impl RunSessionOptions {
    fn from_cli(cli: &Cli) -> Self {
        Self {
            session_id: cli.session_id.clone(),
            session: cli.session.clone(),
            continue_latest: cli.continue_latest,
            fork: cli.fork.clone(),
            name: cli.name.clone(),
            no_session: cli.no_session,
        }
    }

    fn target(&self) -> Option<modes::run::SessionTarget<'_>> {
        session_target_for_cli(
            self.session_id.as_deref(),
            self.session.as_deref(),
            self.continue_latest,
            self.fork.as_deref(),
        )
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
        Some(Command::Print { prompt }) => {
            let prompt = prepare_prompt(prompt, config)?;
            modes::print::execute(
                &prompt,
                config,
                session_options.target(),
                session_options.name.as_deref(),
                session_options.no_session,
            )
            .await
        }
        Some(Command::Run { output, prompt }) => {
            let prompt = prepare_prompt(prompt, config)?;
            modes::run::execute(
                &prompt,
                config,
                output.unwrap_or_else(|| run_output_for_mode(config)),
                session_options.target(),
                session_options.name.as_deref(),
                session_options.no_session,
            )
            .await
        }
        Some(Command::Resume { session_id }) => modes::run::resume(&session_id, config).await,
        Some(Command::Sessions { command }) => match command {
            SessionCommand::List => session_commands::list(config),
            SessionCommand::Tree => session_commands::tree(config),
            SessionCommand::Show { session_id } => session_commands::show(&session_id, config),
            SessionCommand::Rename { session_id, name } => {
                session_commands::rename(&session_id, &name, config)
            }
            SessionCommand::Fork { session_id, name } => {
                session_commands::fork(&session_id, name.as_deref(), config)
            }
            SessionCommand::Summarize { session_id } => {
                session_commands::summarize(&session_id, config).await
            }
            SessionCommand::Compact {
                session_id,
                keep_recent,
            } => session_commands::compact(&session_id, keep_recent, config).await,
            SessionCommand::ExportHtml { session_id } => {
                session_commands::export_html(&session_id, config).await
            }
            SessionCommand::ExportJson { session_id } => {
                session_commands::export_json(&session_id, config).await
            }
            SessionCommand::Share { session_id, public } => {
                session_cloud::share(&session_id, public, config).await
            }
            SessionCommand::Sync { command } => match command {
                cli::SessionSyncCommand::Push => session_cloud::sync_push(config).await,
                cli::SessionSyncCommand::Pull => session_cloud::sync_pull(config).await,
                cli::SessionSyncCommand::Status => session_cloud::sync_status(config),
            },
            SessionCommand::Import { share_ref } => {
                session_cloud::import_share(&share_ref, config).await
            }
        },
        Some(Command::Skills { command }) => match command {
            SkillCommand::Show { path } => skill_commands::show(&path),
        },
        Some(Command::Extensions { command }) => dispatch_extensions(config, command).await,
        Some(Command::Prompts { command }) => dispatch_prompts(config, command).await,
        Some(Command::Themes { command }) => dispatch_themes(config, command).await,
        Some(Command::Trust { command }) => match command {
            TrustCommand::Status => trust::status(&config.project_dir),
            TrustCommand::Approve => trust::approve(&config.project_dir),
            TrustCommand::Deny => trust::deny(&config.project_dir),
            TrustCommand::Clear => trust::clear(&config.project_dir),
        },
        Some(Command::Login { command }) => match command {
            LoginCommand::Cloud { server } => cloud_commands::login_cloud(config, &server).await,
        },
        Some(Command::Logout) => cloud_commands::logout(config),
        Some(Command::Auth { command }) => match command {
            AuthCommand::Status => cloud_commands::auth_status(config),
        },
        Some(Command::Cloud { command }) => match command {
            CloudCommand::Status { api_base } => cloud_commands::cloud_status(&api_base).await,
        },
        Some(Command::Config { command }) => match command {
            ConfigCommand::Show => config::show(config),
            ConfigCommand::Set { key, value } => config::set(&key, &value),
            ConfigCommand::Sync { command } => match command {
                ConfigSyncCommand::Push => cloud_commands::sync_push(config).await,
                ConfigSyncCommand::Pull => cloud_commands::sync_pull(config).await,
                ConfigSyncCommand::Status => cloud_commands::sync_status(config).await,
            },
        },
        Some(Command::Models { command }) => match command {
            ModelCommand::List { pricing, json } => {
                modes::run::list_models_with_options(config, pricing, json)
            }
        },
        Some(Command::Images { command }) => match command {
            ImageCommand::Generate {
                prompt,
                model,
                output,
                size,
            } => modes::run::generate_image(config, &prompt, &model, &output, &size).await,
        },
        Some(Command::Mcp { command }) => match command {
            McpCommand::List => Ok(modes::run::list_mcp_servers(config)),
            McpCommand::Tools { server_id } => modes::run::list_mcp_tools(config, &server_id).await,
            McpCommand::Resources { server_id, command } => match command {
                cli::McpResourceCommand::List => {
                    modes::run::list_mcp_resources(config, &server_id).await
                }
                cli::McpResourceCommand::Read { uri } => {
                    modes::run::read_mcp_resource(config, &server_id, &uri).await
                }
                cli::McpResourceCommand::Watch { uri, count } => {
                    modes::run::watch_mcp_resource(config, &server_id, &uri, count).await
                }
            },
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

async fn dispatch_export(paths: &[PathBuf]) -> anyhow::Result<String> {
    anyhow::ensure!(
        paths.len() <= 2,
        "--export accepts a session JSONL path and optional output path"
    );
    let input_path = paths
        .first()
        .expect("non-empty export paths checked by caller");
    let output_path = paths.get(1).cloned().unwrap_or_else(|| {
        let stem = input_path
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .filter(|stem| !stem.is_empty())
            .unwrap_or("session");
        PathBuf::from(format!("neo-session-{stem}.html"))
    });
    session_commands::export_html_file(input_path, &output_path)
        .await
        .with_context(|| format!("failed to export session {}", input_path.display()))?;
    Ok(format!("Exported to: {}\n", output_path.display()))
}

fn session_target_for_cli<'a>(
    session_id: Option<&'a str>,
    session: Option<&'a str>,
    continue_latest: bool,
    fork: Option<&'a str>,
) -> Option<modes::run::SessionTarget<'a>> {
    session_id
        .map(modes::run::SessionTarget::ExactId)
        .or_else(|| session.map(modes::run::SessionTarget::Existing))
        .or(continue_latest.then_some(modes::run::SessionTarget::Latest))
        .or_else(|| fork.map(modes::run::SessionTarget::Fork))
}

fn run_output_for_mode(config: &AppConfig) -> cli::RunOutput {
    if config.defaults.mode.eq_ignore_ascii_case("json") {
        cli::RunOutput::Json
    } else {
        cli::RunOutput::Events
    }
}

fn prepare_prompt(prompt: Vec<String>, config: &AppConfig) -> anyhow::Result<Vec<String>> {
    let mut prompt_template_selectors = config.configured_prompt_templates.clone();
    for selector in &config.prompt_templates {
        if !prompt_template_selectors.contains(selector) {
            prompt_template_selectors.push(selector.clone());
        }
    }
    let prompt = prompt_templates::expand_prompt_template_args(
        prompt,
        &config.project_dir,
        config::global_prompts_dir().as_deref(),
        &prompt_template_selectors,
        config.no_prompt_templates,
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
        ExtensionCommand::Search { query } => {
            package_commands::search(neo_extensions::PackageKind::Extension, &query).await
        }
        ExtensionCommand::List { root } => {
            let paths = extension_paths(config, root);
            extension_commands::list(&paths.root, &paths.state_path, &paths.registry_path)
        }
        ExtensionCommand::Install { source, from, root } => {
            let paths = extension_paths(config, root);
            match from {
                Some(PackageSource::Marketplace) => {
                    let installed = package_commands::install_from_marketplace(
                        neo_extensions::PackageInstallKind::Extension,
                        &source,
                        &paths.root,
                    )
                    .await?;
                    Ok(installed)
                }
                None => extension_commands::install(
                    &paths.root,
                    &paths.state_path,
                    &paths.registry_path,
                    &source,
                ),
            }
        }
        ExtensionCommand::Publish { path } => {
            package_commands::publish(
                neo_extensions::PackageKind::Extension,
                &resolve_package_path(config, path),
            )
            .await
        }
        ExtensionCommand::Update { extension_id, root } => {
            let paths = extension_paths(config, root);
            extension_commands::update(
                &paths.root,
                &paths.state_path,
                &paths.registry_path,
                &extension_id,
                config.offline,
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

async fn dispatch_prompts(
    config: &AppConfig,
    command: PromptPackageCommand,
) -> anyhow::Result<String> {
    match command {
        PromptPackageCommand::Search { query } => {
            package_commands::search(neo_extensions::PackageKind::PromptPack, &query).await
        }
        PromptPackageCommand::Install { package, from } => match from {
            PackageSource::Marketplace => {
                package_commands::install_from_marketplace(
                    neo_extensions::PackageInstallKind::PromptPack,
                    &package,
                    &config.project_dir.join(".neo/prompts"),
                )
                .await
            }
        },
        PromptPackageCommand::Publish { path } => {
            package_commands::publish(
                neo_extensions::PackageKind::PromptPack,
                &resolve_package_path(config, path),
            )
            .await
        }
        PromptPackageCommand::List => prompt_templates::list_project_prompt_templates(
            &config.project_dir,
            config::global_prompts_dir().as_deref(),
        ),
        PromptPackageCommand::Preview { name } => {
            prompt_templates::preview_project_prompt_template(
                &config.project_dir,
                config::global_prompts_dir().as_deref(),
                &name,
            )
        }
    }
}

async fn dispatch_themes(
    config: &AppConfig,
    command: ThemePackageCommand,
) -> anyhow::Result<String> {
    match command {
        ThemePackageCommand::Search { query } => {
            package_commands::search(neo_extensions::PackageKind::Theme, &query).await
        }
        ThemePackageCommand::Install { package, from } => match from {
            PackageSource::Marketplace => {
                package_commands::install_from_marketplace(
                    neo_extensions::PackageInstallKind::Theme,
                    &package,
                    &config.project_dir.join(".neo/themes"),
                )
                .await
            }
        },
        ThemePackageCommand::Publish { path } => {
            package_commands::publish(
                neo_extensions::PackageKind::Theme,
                &resolve_package_path(config, path),
            )
            .await
        }
        ThemePackageCommand::List => themes::list_project_themes(&config.project_dir),
        ThemePackageCommand::Preview { name } => {
            themes::preview_project_theme(&config.project_dir, &name)
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

fn resolve_package_path(config: &AppConfig, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        config.project_dir.join(path)
    }
}
