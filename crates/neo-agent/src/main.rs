mod cli;
mod config;
mod extension_commands;
mod modes;
mod rpc_mode;
mod session_commands;
mod skill_commands;

use clap::Parser;

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
        Some(Command::Print { prompt }) => modes::print::execute(&prompt, &config).await,
        Some(Command::Run { prompt }) => modes::run::execute(&prompt, &config).await,
        Some(Command::Resume { session_id }) => modes::run::resume(&session_id, &config).await,
        Some(Command::Sessions { command }) => match command {
            SessionCommand::List => session_commands::list(&config),
            SessionCommand::Show { session_id } => session_commands::show(&session_id, &config),
            SessionCommand::Rename { session_id, name } => {
                session_commands::rename(&session_id, &name, &config)
            }
            SessionCommand::Fork { session_id, name } => {
                session_commands::fork(&session_id, name.as_deref(), &config)
            }
            SessionCommand::ExportHtml { session_id } => {
                session_commands::export_html(&session_id, &config).await
            }
        },
        Some(Command::Skills { command }) => match command {
            SkillCommand::Show { path } => skill_commands::show(&path),
        },
        Some(Command::Extensions { command }) => match command {
            ExtensionCommand::List { root } => extension_commands::list(&root),
            ExtensionCommand::Status { extension_id, root } => {
                extension_commands::status(&root, &extension_id)
            }
            ExtensionCommand::Enable { extension_id, root } => {
                extension_commands::enable(&root, &extension_id)
            }
            ExtensionCommand::Disable { extension_id, root } => {
                extension_commands::disable(&root, &extension_id)
            }
            ExtensionCommand::Call {
                extension_id,
                method,
                params,
                root,
            } => extension_commands::call(&root, &extension_id, &method, &params).await,
        },
        Some(Command::Config { command }) => match command {
            ConfigCommand::Show => config::show(&config),
            ConfigCommand::Set { key, value } => config::set(&key, &value),
        },
        Some(Command::Models { command }) => match command {
            ModelCommand::List => Ok(modes::run::list_models(&config)),
        },
        Some(Command::Mcp { command }) => match command {
            McpCommand::List => Ok(modes::run::list_mcp_servers(&config)),
        },
        Some(Command::Rpc) => rpc_mode::execute(&config).await,
        None => Ok(modes::interactive::execute(&config)),
    }
}
