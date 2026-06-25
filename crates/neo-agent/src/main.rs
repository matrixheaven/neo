mod cli;
mod clipboard;
mod config;
mod config_ops;
mod extension_commands;
mod image_blob;
mod mcp_ops;
mod modes;
mod prompt_history;
mod prompt_parts;
mod prompt_templates;
mod resources;
mod rpc_mode;
mod rpc_types;
mod themes;
mod trust;
mod trust_commands;

use std::{
    collections::BTreeMap,
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
    tracing_subscriber::fmt()
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init()
        .ok();

    let cli = Cli::parse_from(std::env::args_os());
    let output = dispatch(cli).await?;
    print!("{output}");
    Ok(())
}

async fn dispatch(cli: Cli) -> anyhow::Result<String> {
    let config = AppConfig::load(ConfigOverrides::from_cli(&cli))?;

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

async fn dispatch_command(
    command: Option<Command>,
    config: &AppConfig,
    session_options: RunSessionOptions,
    resume_picker: bool,
    interactive_options: modes::interactive::InteractiveOptions,
) -> anyhow::Result<String> {
    match command {
        Some(Command::Run { output, prompt }) => {
            dispatch_run_command(config, session_options, output, prompt).await
        }
        Some(Command::Resume { session_id }) => {
            dispatch_resume_command(config, interactive_options, session_id).await
        }
        Some(Command::Sessions { command }) => dispatch_session_command(config, command).await,
        Some(Command::Extensions { command }) => dispatch_extensions(config, command).await,
        Some(Command::Models { command }) => dispatch_model_command(config, command),
        Some(Command::Provider { command }) => dispatch_provider_command(config, command).await,
        Some(Command::Mcp { command }) => dispatch_mcp_command(config, command).await,
        Some(Command::Rpc) => rpc_mode::execute(config).await,
        Some(Command::Trust { command }) => trust_commands::execute(config, &command),
        None => dispatch_default_command(config, resume_picker, interactive_options).await,
    }
}

async fn dispatch_run_command(
    config: &AppConfig,
    session_options: RunSessionOptions,
    output: Option<cli::RunOutput>,
    prompt: Vec<String>,
) -> anyhow::Result<String> {
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

async fn dispatch_resume_command(
    config: &AppConfig,
    interactive_options: modes::interactive::InteractiveOptions,
    session_id: Option<String>,
) -> anyhow::Result<String> {
    if io::stdout().is_terminal() {
        let startup = session_id.map_or(
            modes::interactive::StartupAction::OpenSessionPicker,
            modes::interactive::StartupAction::LoadSession,
        );
        return Ok(modes::interactive::execute_tty_with_startup(
            config,
            startup,
            interactive_options,
        )
        .await?
        .unwrap_or_default());
    }
    let Some(id) = session_id else {
        anyhow::bail!("`neo resume` requires a terminal; use `neo resume <session-id>` in scripts");
    };
    let transcript = modes::sessions::transcript(&id, config).await?;
    Ok(format!("session {id}\n{transcript}"))
}

async fn dispatch_session_command(
    config: &AppConfig,
    command: SessionCommand,
) -> anyhow::Result<String> {
    match command {
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
    }
}

fn dispatch_model_command(config: &AppConfig, command: ModelCommand) -> anyhow::Result<String> {
    match command {
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
                capabilities: default_model_capabilities(capabilities),
                display_name,
            },
        ),
        ModelCommand::Remove { alias } => config_ops::remove_model(&config.config_path, &alias),
        ModelCommand::Set { alias } => config_ops::set_default_model(&config.config_path, &alias),
    }
}

fn default_model_capabilities(capabilities: Vec<String>) -> Vec<String> {
    if capabilities.is_empty() {
        vec!["streaming".to_owned(), "tools".to_owned()]
    } else {
        capabilities
    }
}

async fn dispatch_provider_command(
    config: &AppConfig,
    command: ProviderCommand,
) -> anyhow::Result<String> {
    match command {
        ProviderCommand::List { json } => config_ops::list_providers(config, json),
        ProviderCommand::Add {
            provider_id,
            r#type,
            base_url,
            api_key,
            api_key_env,
        } => add_provider(
            config,
            &provider_id,
            r#type.as_deref(),
            base_url,
            api_key,
            api_key_env,
        ),
        ProviderCommand::Remove { provider_id } => {
            config_ops::remove_provider(&config.config_path, &provider_id)
        }
        ProviderCommand::Catalog { command } => dispatch_catalog_command(config, command).await,
    }
}

fn add_provider(
    config: &AppConfig,
    provider_id: &str,
    provider_type: Option<&str>,
    base_url: Option<String>,
    api_key: Option<String>,
    api_key_env: Option<String>,
) -> anyhow::Result<String> {
    let provider_type = provider_type
        .map(|value| {
            neo_ai::ApiType::from_config_str(value)
                .ok_or_else(|| anyhow::anyhow!("unsupported provider type: {value}"))
        })
        .transpose()?;
    config_ops::add_provider(
        &config.config_path,
        provider_id,
        config::ProviderConfig {
            provider_type,
            base_url,
            api_key,
            api_key_env,
        },
    )
}

async fn dispatch_catalog_command(
    config: &AppConfig,
    command: CatalogCommand,
) -> anyhow::Result<String> {
    match command {
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
    }
}

async fn dispatch_mcp_command(config: &AppConfig, command: McpCommand) -> anyhow::Result<String> {
    match command {
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
        McpCommand::Del { mcp_name } => {
            Ok(config::remove_mcp_server(&mcp_name, &config.config_path)?)
        }
        McpCommand::Disable { mcp_name } => Ok(config::set_mcp_server_enabled(
            &mcp_name,
            false,
            &config.config_path,
        )?),
        McpCommand::Enable { mcp_name } => Ok(config::set_mcp_server_enabled(
            &mcp_name,
            true,
            &config.config_path,
        )?),
        McpCommand::Status => {
            let snapshots = mcp_ops::probe_mcp_servers(config).await?;
            Ok(mcp_ops::format_mcp_status(&snapshots))
        }
        McpCommand::Resources { server_id } => {
            let entries = mcp_ops::list_mcp_resources(config, server_id.as_deref()).await?;
            if entries.is_empty() {
                return Ok("No MCP resources found.".to_owned());
            }
            let mut lines = Vec::with_capacity(entries.len() + 1);
            lines.push(format!("{:<20} {:<40} {}", "Server", "URI", "Name"));
            for entry in entries {
                lines.push(format!(
                    "{:<20} {:<40} {}",
                    entry.server_id, entry.uri, entry.name
                ));
            }
            Ok(lines.join("\n"))
        }
        McpCommand::ReadResource { server_id, uri } => {
            let read = mcp_ops::read_mcp_resource(config, &server_id, &uri).await?;
            let mut out = Vec::new();
            for content in read.contents {
                if let Some(text) = content.text {
                    out.push(text);
                } else if content.blob.is_some() {
                    out.push(format!("[binary content for {}]", content.uri));
                }
            }
            Ok(out.join("\n"))
        }
        McpCommand::Auth { server_id } => modes::run::auth_mcp_server(server_id, config).await,
    }
}

async fn dispatch_default_command(
    config: &AppConfig,
    resume_picker: bool,
    interactive_options: modes::interactive::InteractiveOptions,
) -> anyhow::Result<String> {
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
        config.project_trusted,
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
    let neo_home = crate::config::neo_home().unwrap_or_else(|| config.project_dir.join(".neo"));
    ExtensionPaths {
        root: resolve_default_extension_root(&neo_home, root),
        state_path: neo_home.join("extensions-state.toml"),
        registry_path: neo_home.join("extensions-sources.toml"),
    }
}

fn resolve_default_extension_root(neo_home: &Path, root: PathBuf) -> PathBuf {
    if root == Path::new("extensions") || root == Path::new(".neo/extensions") {
        neo_home.join("extensions")
    } else {
        root
    }
}

/// Fetch and display providers from the models.dev catalog.
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
        return format_catalog_provider_detail(pid, entry, json);
    }

    format_catalog_provider_list(&catalog, filter, json)
}

fn format_catalog_provider_detail(
    provider_id: &str,
    entry: &neo_ai::catalog::CatalogEntry,
    json_output: bool,
) -> anyhow::Result<String> {
    let wire = neo_ai::catalog::infer_api_type(entry);
    let name = entry.name.as_deref().unwrap_or(provider_id);
    let wire_str = wire.as_ref().map_or("unsupported", |t| t.as_config_str());
    let models = neo_ai::catalog::catalog_provider_models(entry);

    if json_output {
        return Ok(serde_json::to_string_pretty(&json!({
            "id": provider_id,
            "name": name,
            "wire": wire_str,
            "models": catalog_models_json(&models),
        }))? + "\n");
    }

    let mut out = format!(
        "{name} ({provider_id})  wire={wire_str}  models={}\n",
        models.len()
    );
    for model in &models {
        let _ = writeln!(out, "{}", catalog_model_text(model));
    }
    Ok(out)
}

fn format_catalog_provider_list(
    catalog: &BTreeMap<String, neo_ai::catalog::CatalogEntry>,
    filter: Option<&str>,
    json_output: bool,
) -> anyhow::Result<String> {
    let mut entries: Vec<_> = catalog.values().collect();
    entries.sort_by_key(|e| e.id.clone());

    if json_output {
        let providers_json = entries
            .iter()
            .filter(|entry| catalog_provider_matches_filter(entry, filter))
            .filter_map(|entry| catalog_provider_list_json(entry))
            .collect::<Vec<_>>();
        return Ok(serde_json::to_string_pretty(&json!({ "providers": providers_json }))? + "\n");
    }

    let mut out = String::new();
    for entry in entries {
        if let Some(line) = catalog_provider_list_text(entry, filter) {
            out.push_str(&line);
        }
    }
    Ok(out)
}

fn catalog_models_json(models: &[neo_ai::catalog::CatalogModelInfo]) -> Vec<serde_json::Value> {
    models
        .iter()
        .map(|model| {
            json!({
                "id": model.id,
                "name": model.name,
                "max_context_tokens": model.max_context_tokens,
                "capabilities": model.capabilities,
            })
        })
        .collect()
}

fn catalog_model_text(model: &neo_ai::catalog::CatalogModelInfo) -> String {
    let ctx = model
        .max_context_tokens
        .map_or("?".to_owned(), |n| n.to_string());
    let caps = model.capabilities.join(",");
    let display = model.name.as_deref().unwrap_or(&model.id);
    format!(
        "  {id:<40} {display:<30} ctx={ctx:<10} [{caps}]",
        id = model.id
    )
}

fn catalog_provider_list_json(entry: &neo_ai::catalog::CatalogEntry) -> Option<serde_json::Value> {
    let wire = neo_ai::catalog::infer_api_type(entry)?;
    Some(json!({
        "id": entry.id,
        "name": entry.name,
        "wire": wire.as_config_str(),
        "model_count": entry.models.len(),
    }))
}

fn catalog_provider_list_text(
    entry: &neo_ai::catalog::CatalogEntry,
    filter: Option<&str>,
) -> Option<String> {
    if !catalog_provider_matches_filter(entry, filter) {
        return None;
    }
    let wire = neo_ai::catalog::infer_api_type(entry)?;
    let name = entry.name.as_deref().unwrap_or(&entry.id);
    Some(format!(
        "{id:<25} wire={wire:<18} models={model_count:<4} {name}\n",
        id = entry.id,
        wire = wire.as_config_str(),
        model_count = entry.models.len()
    ))
}

fn catalog_provider_matches_filter(
    entry: &neo_ai::catalog::CatalogEntry,
    filter: Option<&str>,
) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    let filter = filter.to_ascii_lowercase();
    entry.id.to_ascii_lowercase().contains(&filter)
        || entry
            .name
            .as_deref()
            .is_some_and(|name| name.to_ascii_lowercase().contains(&filter))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use neo_ai::catalog;

    #[test]
    fn catalog_provider_list_json_filters_and_skips_unsupported_entries() {
        let catalog = BTreeMap::from([
            (
                "openai".to_owned(),
                catalog_entry("openai", Some("OpenAI"), true),
            ),
            (
                "unknown".to_owned(),
                catalog_entry("unknown", Some("Unknown"), false),
            ),
        ]);

        let output = super::format_catalog_provider_list(&catalog, Some("open"), true)
            .expect("json catalog providers");
        let value: serde_json::Value = serde_json::from_str(&output).expect("json");

        assert_eq!(
            value,
            serde_json::json!({
                "providers": [{
                    "id": "openai",
                    "name": "OpenAI",
                    "wire": "openai-responses",
                    "model_count": 1,
                }],
            })
        );
    }

    #[test]
    fn catalog_provider_detail_text_includes_model_metadata() {
        let entry = catalog_entry("openai", Some("OpenAI"), true);

        let output = super::format_catalog_provider_detail("openai", &entry, false)
            .expect("text catalog detail");

        assert!(output.contains("OpenAI (openai)  wire=openai-responses  models=1"));
        assert!(output.contains("gpt-4.1"));
        assert!(output.contains("GPT 4.1"));
        assert!(output.contains("ctx=1000000"));
        assert!(output.contains("[streaming,tools,reasoning,images]"));
    }

    fn catalog_entry(id: &str, name: Option<&str>, supported: bool) -> catalog::CatalogEntry {
        catalog::CatalogEntry {
            id: id.to_owned(),
            name: name.map(str::to_owned),
            api: Some(format!("https://api.{id}.test/v1")),
            env: Vec::new(),
            npm: None,
            explicit_type: supported.then(|| "openai-responses".to_owned()),
            models: BTreeMap::from([(
                "gpt-4.1".to_owned(),
                catalog::CatalogModel {
                    id: "gpt-4.1".to_owned(),
                    name: Some("GPT 4.1".to_owned()),
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
            )]),
        }
    }
}
