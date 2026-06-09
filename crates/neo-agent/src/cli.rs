use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};

pub const LIST_MODELS_NO_SEARCH: &str = "__neo_list_models_no_search__";

#[derive(Debug, Parser)]
#[allow(clippy::struct_excessive_bools)]
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

    #[arg(long = "session-dir", global = true, value_name = "DIR")]
    pub session_dir: Option<std::path::PathBuf>,

    #[arg(long = "session-id", global = true, value_name = "ID")]
    pub session_id: Option<String>,

    #[arg(
        long,
        global = true,
        value_name = "ID_OR_PATH",
        conflicts_with = "session_id"
    )]
    pub session: Option<String>,

    #[arg(
        short = 'c',
        long = "continue",
        global = true,
        conflicts_with_all = ["session_id", "session"]
    )]
    pub continue_latest: bool,

    #[arg(
        long,
        global = true,
        value_name = "ID_OR_PATH",
        conflicts_with_all = ["session_id", "session", "continue_latest"]
    )]
    pub fork: Option<String>,

    #[arg(short = 'n', long, global = true, value_name = "NAME")]
    pub name: Option<String>,

    #[arg(long = "no-session", global = true, conflicts_with_all = [
        "session_id",
        "session",
        "continue_latest",
        "fork",
        "name"
    ])]
    pub no_session: bool,

    #[arg(long, global = true, value_name = "SESSION_JSONL", num_args = 1..=2)]
    pub export: Vec<std::path::PathBuf>,

    #[arg(long, global = true, env = "NEO_MODE")]
    pub mode: Option<String>,

    #[arg(short = 'a', long, global = true, conflicts_with = "no_approve")]
    pub approve: bool,

    #[arg(long = "no-approve", alias = "no_approve", global = true)]
    pub no_approve: bool,

    #[arg(long, global = true, value_name = "NAME_OR_PATH")]
    pub prompt_template: Vec<String>,

    #[arg(long, global = true)]
    pub no_prompt_templates: bool,

    #[arg(long, global = true, value_name = "TEXT_OR_PATH")]
    pub system_prompt: Option<String>,

    #[arg(long, global = true, value_name = "TEXT_OR_PATH")]
    pub append_system_prompt: Vec<String>,

    #[arg(long, global = true, value_name = "LEVEL")]
    pub thinking: Option<ThinkingLevel>,

    #[arg(
        long = "list-models",
        global = true,
        num_args = 0..=1,
        value_name = "SEARCH",
        default_missing_value = LIST_MODELS_NO_SEARCH,
        action = ArgAction::Set
    )]
    pub list_models: Option<String>,

    #[command(flatten)]
    pub tool_filters: ToolFilterArgs,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, Default, Args)]
pub struct ToolFilterArgs {
    #[arg(long = "no-tools", alias = "no_tools", global = true)]
    pub no_tools: bool,

    #[arg(long = "no-builtin-tools", alias = "no_builtin_tools", global = true)]
    pub no_builtin_tools: bool,

    #[arg(
        short = 't',
        long,
        global = true,
        value_name = "NAMES",
        value_delimiter = ','
    )]
    pub tools: Vec<String>,

    #[arg(
        long = "exclude-tools",
        alias = "exclude_tools",
        global = true,
        value_name = "NAMES",
        value_delimiter = ','
    )]
    pub exclude_tools: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Print {
        prompt: Vec<String>,
    },
    Run {
        #[arg(long, value_enum)]
        output: Option<RunOutput>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RunOutput {
    Events,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    #[value(name = "xhigh")]
    XHigh,
}

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    List,
    Tree,
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
    Tools {
        server_id: String,
    },
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
