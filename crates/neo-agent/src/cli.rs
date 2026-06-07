use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "neo", version, about = "Rust-native coding agent")]
pub struct Cli {
    #[arg(long, global = true, env = "NEO_MODEL")]
    pub model: Option<String>,

    #[arg(long, global = true, env = "NEO_PROVIDER")]
    pub provider: Option<String>,

    #[arg(long, global = true, env = "NEO_API_BASE")]
    pub api_base: Option<String>,

    #[arg(long, global = true, env = "NEO_CONFIG")]
    pub config: Option<std::path::PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Print {
        prompt: Vec<String>,
    },
    Run {
        prompt: Vec<String>,
    },
    Resume {
        session_id: String,
    },
    Sessions {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    Models {
        #[command(subcommand)]
        command: ModelCommand,
    },
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    List,
    Show { session_id: String },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    Show,
    Set { key: String, value: String },
}

#[derive(Debug, Subcommand)]
pub enum ModelCommand {
    List,
}

#[derive(Debug, Subcommand)]
pub enum McpCommand {
    List,
}
