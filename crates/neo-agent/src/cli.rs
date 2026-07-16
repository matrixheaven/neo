use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "neo", version, about = "Rust-native local AI coding agent")]
#[allow(clippy::struct_excessive_bools)]
pub struct Cli {
    #[arg(
        short = 'r',
        long = "resume",
        conflicts_with_all = ["continue_latest", "no_session"]
    )]
    pub resume_picker: bool,

    #[arg(
        short = 'c',
        long = "continue",
        conflicts_with_all = ["resume_picker", "no_session"]
    )]
    pub continue_latest: bool,

    #[arg(
        long = "no-session",
        conflicts_with_all = ["continue_latest", "resume_picker"]
    )]
    pub no_session: bool,

    #[arg(long = "yolo")]
    pub yolo: bool,

    #[arg(long = "auto", conflicts_with = "yolo")]
    pub auto: bool,

    #[arg(long, env = "NEO_CONFIG")]
    pub config: Option<std::path::PathBuf>,

    #[arg(long)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(name = "__process-guard", hide = true)]
    ProcessGuard,
    /// Run a single agent task on stdin/file input
    Run {
        #[arg(long, value_enum)]
        output: Option<RunOutput>,
        prompt: Vec<String>,
    },
    /// Resume a specific session and enter interactive mode
    Resume { session_id: Option<String> },
    /// Session management
    Sessions {
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// Model provider management
    Provider {
        #[command(subcommand)]
        command: ProviderCommand,
    },
    /// Model management
    Models {
        #[command(subcommand)]
        command: ModelCommand,
    },
    /// MCP server management
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    /// JSONL RPC server mode
    Rpc,
    /// Workspace trust management
    Trust {
        #[command(subcommand)]
        command: TrustCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum TrustCommand {
    /// Show trust status for the current workspace
    Status,
    /// Trust the current workspace
    Approve,
    /// Deny trust for the current workspace
    Deny,
    /// Clear the trust decision for the current workspace
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RunOutput {
    /// Raw event stream
    Events,
    /// JSON output
    Json,
    /// Plain text output
    Text,
}

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    /// List sessions in the current workspace
    List,
    /// Show session details
    Show { session_id: String },
    /// Rename a session
    Rename { session_id: String, name: String },
    /// Fork a session
    Fork {
        session_id: String,
        /// Name for the forked session
        #[arg(long)]
        name: Option<String>,
    },
    /// Compact session history
    Compact {
        session_id: String,
        /// Number of recent messages to keep
        #[arg(long, default_value_t = 20)]
        keep_recent: usize,
    },
    /// Export a session as HTML
    ExportHtml { session_id: String },
    /// Export a session as JSON
    ExportJson { session_id: String },
}

#[derive(Debug, Subcommand)]
pub enum ModelCommand {
    /// List available models
    List {
        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },
    /// Add a model alias
    Add {
        alias: String,
        #[arg(long)]
        provider: String,
        #[arg(long)]
        model: String,
        #[arg(long, value_name = "TOKENS")]
        max_context_tokens: Option<u32>,
        #[arg(long, value_delimiter = ',')]
        capabilities: Vec<String>,
        #[arg(long)]
        display_name: Option<String>,
    },
    /// Remove a model alias
    Remove { alias: String },
    /// Set the default model
    Set { alias: String },
}

#[derive(Debug, Subcommand)]
pub enum ProviderCommand {
    /// List configured or available providers
    List {
        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },
    /// Add a custom provider
    Add {
        provider_id: String,
        #[arg(long, value_name = "TYPE")]
        r#type: Option<String>,
        #[arg(long, value_name = "URL")]
        base_url: Option<String>,
        #[arg(long, value_name = "KEY")]
        api_key: Option<String>,
        #[arg(long, value_name = "ENV_VAR")]
        api_key_env: Option<String>,
    },
    /// Remove a custom provider
    Remove { provider_id: String },
    /// models.dev catalog management
    Catalog {
        #[command(subcommand)]
        command: CatalogCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum CatalogCommand {
    /// List providers on models.dev
    List {
        /// Show models for a specific provider only
        provider_id: Option<String>,
        /// Filter by keyword
        #[arg(long)]
        filter: Option<String>,
        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },
    /// Import a provider and its models from models.dev
    Add {
        provider_id: String,
        #[arg(long, value_name = "KEY")]
        api_key: Option<String>,
        /// Model ID to set as default after import
        #[arg(long, value_name = "MODEL_ID")]
        default_model: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum McpCommand {
    /// List all configured MCP servers and their tool names
    List,
    /// Add and test an MCP server
    Add {
        /// MCP server name
        mcp_name: String,
        /// MCP type: studio, remote-http, remote-sse
        #[arg(short = 't', long = "type", value_name = "TYPE")]
        r#type: String,
        /// Full shell command for studio type (global -c is taken, so -C)
        #[arg(short = 'C', long = "command", value_name = "CMD")]
        command: Option<String>,
        /// Service URL for remote-http / remote-sse
        #[arg(short = 'u', long = "url", value_name = "URL")]
        url: Option<String>,
        /// Environment variables in KEY=VALUE format, may be specified multiple times
        #[arg(short = 'e', long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
        /// HTTP headers for remote types in KEY=VALUE format (-h is taken by help, so -H)
        #[arg(short = 'H', long = "header", value_name = "KEY=VALUE")]
        headers: Vec<String>,
        /// Working directory for studio subprocess
        #[arg(long = "cwd", value_name = "DIR")]
        cwd: Option<std::path::PathBuf>,
        /// Tool allowlist, comma-separated
        #[arg(
            long = "enabled-tools",
            value_name = "TOOL1,TOOL2",
            value_delimiter = ','
        )]
        enabled_tools: Vec<String>,
        /// Tool denylist, comma-separated
        #[arg(
            long = "disabled-tools",
            value_name = "TOOL1,TOOL2",
            value_delimiter = ','
        )]
        disabled_tools: Vec<String>,
        /// Connection test timeout in milliseconds
        #[arg(long = "startup-timeout-ms", value_name = "MS")]
        startup_timeout_ms: Option<u64>,
        /// Per-tool-call timeout in milliseconds
        #[arg(long = "tool-timeout-ms", value_name = "MS")]
        tool_timeout_ms: Option<u64>,
        /// Enable after adding (default behavior)
        #[arg(long = "enable", default_value_t = true)]
        enable: bool,
        /// Disable after adding
        #[arg(long = "disable", default_value_t = false)]
        disable: bool,
    },
    /// Remove an MCP server
    Del {
        /// MCP server name
        mcp_name: String,
    },
    /// Disable an MCP server
    Disable {
        /// MCP server name
        mcp_name: String,
    },
    /// Enable an MCP server
    Enable {
        /// MCP server name
        mcp_name: String,
    },
    /// Show connection status, tool count, and recent errors for each MCP server
    Status,
    /// List resources exposed by connected MCP servers
    Resources {
        /// Only list resources for the specified MCP server
        #[arg(short, long, value_name = "SERVER")]
        server_id: Option<String>,
    },
    /// Read the content of a resource exposed by a connected MCP server
    ReadResource {
        /// MCP server name
        server_id: String,
        /// Resource URI
        uri: String,
    },
    /// Start the OAuth authorization flow for an MCP server
    Auth {
        /// MCP server name
        server_id: String,
    },
}
