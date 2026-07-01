use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "neo", version, about = "Rust-native 本地 AI 编程代理")]
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
    /// 在标准输入/文件上运行一次代理任务
    Run {
        #[arg(long, value_enum)]
        output: Option<RunOutput>,
        prompt: Vec<String>,
    },
    /// 恢复指定会话并进入交互模式
    Resume { session_id: Option<String> },
    /// 会话管理
    Sessions {
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// 模型提供商管理
    Provider {
        #[command(subcommand)]
        command: ProviderCommand,
    },
    /// 模型管理
    Models {
        #[command(subcommand)]
        command: ModelCommand,
    },
    /// MCP 服务器管理
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    /// JSONL RPC 服务端模式
    Rpc,
    /// 工作区信任管理
    Trust {
        #[command(subcommand)]
        command: TrustCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum TrustCommand {
    /// 显示当前工作区的信任状态
    Status,
    /// 信任当前工作区
    Approve,
    /// 拒绝信任当前工作区
    Deny,
    /// 清除当前工作区的信任决定
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RunOutput {
    /// 原始事件流
    Events,
    /// JSON 输出
    Json,
    /// 纯文本输出
    Text,
}

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    /// 列出当前工作区会话
    List,
    /// 查看会话详情
    Show { session_id: String },
    /// 重命名会话
    Rename { session_id: String, name: String },
    /// 分叉会话
    Fork {
        session_id: String,
        /// 新会话名称
        #[arg(long)]
        name: Option<String>,
    },
    /// 压缩会话历史
    Compact {
        session_id: String,
        /// 保留最近多少条消息
        #[arg(long, default_value_t = 20)]
        keep_recent: usize,
    },
    /// 导出会话为 HTML
    ExportHtml { session_id: String },
    /// 导出会话为 JSON
    ExportJson { session_id: String },
}

#[derive(Debug, Subcommand)]
pub enum ModelCommand {
    /// 列出可用模型
    List {
        /// 以 JSON 格式输出
        #[arg(long)]
        json: bool,
    },
    /// 添加模型别名
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
    /// 删除模型别名
    Remove { alias: String },
    /// 设置默认模型
    Set { alias: String },
}

#[derive(Debug, Subcommand)]
pub enum ProviderCommand {
    /// 列出已配置或可用的提供商
    List {
        /// 以 JSON 格式输出
        #[arg(long)]
        json: bool,
    },
    /// 添加自定义提供商
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
    /// 删除自定义提供商
    Remove { provider_id: String },
    /// models.dev 目录管理
    Catalog {
        #[command(subcommand)]
        command: CatalogCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum CatalogCommand {
    /// 列出 models.dev 上的提供商
    List {
        /// 只看指定提供商的模型
        provider_id: Option<String>,
        /// 按关键字过滤
        #[arg(long)]
        filter: Option<String>,
        /// 以 JSON 格式输出
        #[arg(long)]
        json: bool,
    },
    /// 从 models.dev 导入提供商及其模型
    Add {
        provider_id: String,
        #[arg(long, value_name = "KEY")]
        api_key: Option<String>,
        /// 导入后设为默认的模型 ID
        #[arg(long, value_name = "MODEL_ID")]
        default_model: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum McpCommand {
    /// 列出所有已配置的 MCP 及其工具名
    List,
    /// 添加并测试一个 MCP
    Add {
        /// MCP 名称
        mcp_name: String,
        /// MCP 类型：studio、remote-http、remote-sse
        #[arg(short = 't', long = "type", value_name = "TYPE")]
        r#type: String,
        /// studio 类型的完整 shell 命令（全局 -c 已被占用，因此用 -C）
        #[arg(short = 'C', long = "command", value_name = "CMD")]
        command: Option<String>,
        /// remote-http / remote-sse 的服务地址
        #[arg(short = 'u', long = "url", value_name = "URL")]
        url: Option<String>,
        /// 环境变量，格式 KEY=VALUE，可多次指定
        #[arg(short = 'e', long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
        /// remote 类型的 HTTP 请求头，格式 KEY=VALUE（-h 已被 help 占用，因此用 -H）
        #[arg(short = 'H', long = "header", value_name = "KEY=VALUE")]
        headers: Vec<String>,
        /// studio 类型子进程的工作目录
        #[arg(long = "cwd", value_name = "DIR")]
        cwd: Option<std::path::PathBuf>,
        /// 工具白名单，逗号分隔
        #[arg(
            long = "enabled-tools",
            value_name = "TOOL1,TOOL2",
            value_delimiter = ','
        )]
        enabled_tools: Vec<String>,
        /// 工具黑名单，逗号分隔
        #[arg(
            long = "disabled-tools",
            value_name = "TOOL1,TOOL2",
            value_delimiter = ','
        )]
        disabled_tools: Vec<String>,
        /// 连接测试超时（毫秒）
        #[arg(long = "startup-timeout-ms", value_name = "MS")]
        startup_timeout_ms: Option<u64>,
        /// 单次工具调用超时（毫秒）
        #[arg(long = "tool-timeout-ms", value_name = "MS")]
        tool_timeout_ms: Option<u64>,
        /// 添加后默认启用；显式传 --enable 保持启用（默认行为）
        #[arg(long = "enable", default_value_t = true)]
        enable: bool,
        /// 添加后默认启用；加此 flag 则禁用
        #[arg(long = "disable", default_value_t = false)]
        disable: bool,
    },
    /// 删除一个 MCP
    Del {
        /// MCP 名称
        mcp_name: String,
    },
    /// 禁用一个 MCP
    Disable {
        /// MCP 名称
        mcp_name: String,
    },
    /// 启用一个 MCP
    Enable {
        /// MCP 名称
        mcp_name: String,
    },
    /// 显示每个 MCP 的连接状态、工具数和最近的错误
    Status,
    /// 列出已连接 MCP 暴露的资源
    Resources {
        /// 仅列出指定 MCP 服务器的资源
        #[arg(short, long, value_name = "SERVER")]
        server_id: Option<String>,
    },
    /// 读取已连接 MCP 暴露的资源内容
    ReadResource {
        /// MCP 服务器名称
        server_id: String,
        /// 资源 URI
        uri: String,
    },
    /// 为 MCP 服务器启动 OAuth 授权流程
    Auth {
        /// MCP 服务器名称
        server_id: String,
    },
}
