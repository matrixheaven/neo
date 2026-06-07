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

    #[arg(short = 'a', long, global = true, conflicts_with = "no_approve")]
    pub approve: bool,

    #[arg(long = "no-approve", alias = "no_approve", global = true)]
    pub no_approve: bool,

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
    Skills {
        #[command(subcommand)]
        command: SkillCommand,
    },
    Extensions {
        #[command(subcommand)]
        command: ExtensionCommand,
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
    Rpc,
}

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    List,
    Show {
        session_id: String,
    },
    Rename {
        session_id: String,
        name: String,
    },
    Fork {
        session_id: String,
        #[arg(long)]
        name: Option<String>,
    },
    Summarize {
        session_id: String,
    },
    Compact {
        session_id: String,
        #[arg(long, default_value_t = 20)]
        keep_recent: usize,
    },
    ExportHtml {
        session_id: String,
    },
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
    Resources {
        server_id: String,
        #[command(subcommand)]
        command: McpResourceCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum McpResourceCommand {
    List,
    Read {
        uri: String,
    },
    Watch {
        uri: String,
        #[arg(long, default_value_t = 1)]
        count: usize,
    },
}

#[derive(Debug, Subcommand)]
pub enum SkillCommand {
    Show { path: std::path::PathBuf },
}

#[derive(Debug, Subcommand)]
pub enum ExtensionCommand {
    List {
        #[arg(default_value = ".neo/extensions")]
        root: std::path::PathBuf,
    },
    Install {
        source: String,
        #[arg(long, default_value = ".neo/extensions")]
        root: std::path::PathBuf,
    },
    Update {
        extension_id: String,
        #[arg(long, default_value = ".neo/extensions")]
        root: std::path::PathBuf,
    },
    Uninstall {
        extension_id: String,
        #[arg(long, default_value = ".neo/extensions")]
        root: std::path::PathBuf,
    },
    Status {
        extension_id: String,
        #[arg(long, default_value = ".neo/extensions")]
        root: std::path::PathBuf,
    },
    Enable {
        extension_id: String,
        #[arg(long, default_value = ".neo/extensions")]
        root: std::path::PathBuf,
    },
    Disable {
        extension_id: String,
        #[arg(long, default_value = ".neo/extensions")]
        root: std::path::PathBuf,
    },
    Call {
        extension_id: String,
        method: String,
        #[arg(default_value = "{}")]
        params: String,
        #[arg(long, default_value = ".neo/extensions")]
        root: std::path::PathBuf,
    },
}
