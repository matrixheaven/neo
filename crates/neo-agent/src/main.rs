mod cli;
mod config;
mod modes;
mod session_commands;

use clap::Parser;

use crate::{
    cli::{Cli, Command, ConfigCommand, McpCommand, ModelCommand, SessionCommand},
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
        None => Ok(modes::interactive::execute(&config)),
    }
}
