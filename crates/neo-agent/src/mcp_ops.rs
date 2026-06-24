use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::Context;
use neo_agent_core::{
    ManagedMcpTransport, McpConnectionManager, McpReconnectPolicy, McpResourceListEntry,
    McpResourceRead, McpServerSnapshot, McpServerStatus, ProcessSupervisor,
    oauth::{
        OAuthError, OAuthProvider, OAuthTokenSet, build_authorization_url,
        callback_server::CallbackServer, exchange_code_for_token, generate_pkce, store::OAuthStore,
    },
};

use crate::config::McpServerConfig;

pub use neo_agent_core::ManagedMcpServerConfig;

/// Input used by both CLI and TUI to add or update an MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddMcpServerInput {
    pub id: String,
    pub cli_type: String,
    pub command: Option<String>,
    pub url: Option<String>,
    pub env: Vec<String>,
    pub headers: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub enabled_tools: Vec<String>,
    pub disabled_tools: Vec<String>,
    pub startup_timeout_ms: Option<u64>,
    pub tool_timeout_ms: Option<u64>,
    pub enabled: bool,
}

/// Summary of an MCP server for TUI rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSummary {
    pub id: String,
    pub transport: String,
    pub transport_label: String,
    pub enabled: bool,
    pub endpoint_summary: String,
    pub cwd: Option<PathBuf>,
    pub env_keys: Vec<String>,
    pub header_keys: Vec<String>,
    pub enabled_tools: Vec<String>,
    pub disabled_tools: Vec<String>,
    pub startup_timeout_ms: Option<u64>,
    pub tool_timeout_ms: Option<u64>,
    pub tools: McpToolDiscovery,
}

/// Tool discovery state for a summary row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpToolDiscovery {
    SkippedDisabled,
    NotRequested,
    Success(Vec<String>),
    Failed(String),
}

/// Convert CLI/user-facing type labels to the persisted transport value.
pub fn parse_mcp_kind(type_arg: &str) -> anyhow::Result<&'static str> {
    match type_arg {
        "studio" | "stdio" => Ok("stdio"),
        "remote-http" | "http" => Ok("http"),
        "remote-sse" | "sse" => Ok("sse"),
        other => {
            anyhow::bail!("unknown MCP type '{other}'; expected studio, remote-http, or remote-sse")
        }
    }
}

/// Convert a persisted transport value back to a CLI/user-facing label.
#[must_use]
pub fn display_mcp_kind(transport: &str) -> &str {
    match transport {
        "stdio" => "studio",
        "http" => "remote-http",
        "sse" => "remote-sse",
        other => other,
    }
}

/// Parse a shell-style command string into a program and arguments.
pub fn parse_command_string(cmd: &str) -> anyhow::Result<(String, Vec<String>)> {
    let parts =
        shell_words::split(cmd).with_context(|| format!("invalid command string: {cmd}"))?;
    let (command, args) = parts.split_first().context("command string is empty")?;
    Ok((command.clone(), args.to_vec()))
}

/// Parse `KEY=VALUE` strings into a map.
pub fn key_value_pairs(
    values: Vec<String>,
    flag: &str,
) -> anyhow::Result<BTreeMap<String, String>> {
    let mut pairs = BTreeMap::new();
    for value in values {
        let Some((key, val)) = value.split_once('=') else {
            anyhow::bail!("{flag} values must use KEY=VALUE");
        };
        let key = key.trim();
        anyhow::ensure!(!key.is_empty(), "{flag} key must not be empty");
        pairs.insert(key.to_owned(), val.trim().to_owned());
    }
    Ok(pairs)
}

/// Validate an MCP server config before persistence.
pub fn validate_mcp_server_config(server: &McpServerConfig) -> anyhow::Result<()> {
    anyhow::ensure!(!server.id.is_empty(), "MCP server id must not be empty");
    anyhow::ensure!(
        !server.id.contains('/'),
        "MCP server id must not contain '/'"
    );
    match server.transport.as_str() {
        "stdio" => {
            anyhow::ensure!(
                server.command.is_some(),
                "studio MCP server '{}' requires a command",
                server.id
            );
            anyhow::ensure!(
                server.url.is_none(),
                "studio MCP server '{}' uses command, not url",
                server.id
            );
            anyhow::ensure!(
                server.headers.is_empty(),
                "studio MCP server '{}' cannot use headers",
                server.id
            );
        }
        "http" | "sse" => {
            anyhow::ensure!(
                server.url.is_some(),
                "remote MCP server '{}' requires a url",
                server.id
            );
            anyhow::ensure!(
                server.command.is_none(),
                "remote MCP server '{}' uses url, not command",
                server.id
            );
            anyhow::ensure!(
                server.cwd.is_none(),
                "remote MCP server '{}' cannot use cwd",
                server.id
            );
        }
        other => anyhow::bail!("unsupported MCP transport for {}: {other}", server.id),
    }
    Ok(())
}

/// Build a persisted `McpServerConfig` from user input.
pub fn build_mcp_server_config(input: AddMcpServerInput) -> anyhow::Result<McpServerConfig> {
    let transport = parse_mcp_kind(&input.cli_type)?;

    let (command, args) = if transport == "stdio" {
        let Some(cmd) = input.command else {
            anyhow::bail!("studio MCP requires a command");
        };
        let (cmd, args) = parse_command_string(&cmd)?;
        (Some(cmd), args)
    } else {
        if input.command.is_some() {
            anyhow::bail!("remote MCP uses url, not command");
        }
        (None, Vec::new())
    };

    let url = if transport == "http" || transport == "sse" {
        let Some(url) = input.url else {
            anyhow::bail!("remote MCP requires a url");
        };
        Some(url)
    } else {
        if input.url.is_some() {
            anyhow::bail!("studio MCP uses command, not url");
        }
        None
    };

    if transport != "http" && transport != "sse" && !input.headers.is_empty() {
        anyhow::bail!("--header is only valid for remote-http / remote-sse");
    }
    if transport != "stdio" && input.cwd.is_some() {
        anyhow::bail!("--cwd is only valid for studio");
    }

    Ok(McpServerConfig {
        id: input.id,
        enabled: input.enabled,
        transport: transport.to_owned(),
        command,
        url,
        args,
        env: key_value_pairs(input.env, "env")?,
        headers: key_value_pairs(input.headers, "headers")?,
        cwd: input.cwd,
        enabled_tools: input.enabled_tools,
        disabled_tools: input.disabled_tools,
        startup_timeout_ms: input.startup_timeout_ms,
        tool_timeout_ms: input.tool_timeout_ms,
    })
}

/// Convert a persisted config to a runtime-managed config.
pub fn to_managed_config(server: &McpServerConfig) -> anyhow::Result<ManagedMcpServerConfig> {
    validate_mcp_server_config(server)?;
    let transport = match server.transport.as_str() {
        "stdio" => {
            let command = server
                .command
                .clone()
                .context("missing MCP command for {}")?;
            ManagedMcpTransport::Stdio {
                command,
                args: server.args.clone(),
                env: server.env.clone(),
                cwd: server.cwd.clone(),
            }
        }
        "http" => {
            let url = server.url.clone().context("missing MCP url for {}")?;
            ManagedMcpTransport::Http {
                url,
                headers: server.headers.clone(),
            }
        }
        "sse" => {
            let url = server.url.clone().context("missing MCP url for {}")?;
            ManagedMcpTransport::Sse {
                url,
                headers: server.headers.clone(),
            }
        }
        other => anyhow::bail!("unsupported MCP transport for {}: {other}", server.id),
    };
    Ok(ManagedMcpServerConfig {
        id: server.id.clone(),
        enabled: server.enabled,
        transport,
        enabled_tools: server.enabled_tools.clone(),
        disabled_tools: server.disabled_tools.clone(),
        startup_timeout_ms: server.startup_timeout_ms,
        tool_timeout_ms: server.tool_timeout_ms,
        reconnect: McpReconnectPolicy::default(),
    })
}

/// Convert many persisted configs to runtime configs.
pub fn to_managed_configs(
    servers: &[McpServerConfig],
) -> anyhow::Result<Vec<ManagedMcpServerConfig>> {
    servers.iter().map(to_managed_config).collect()
}

/// Summarize configured servers without starting any connections.
#[must_use]
pub fn summarize_mcp_servers_without_discovery(
    config: &crate::config::AppConfig,
) -> Vec<McpServerSummary> {
    config
        .mcp
        .servers
        .iter()
        .map(|server| McpServerSummary {
            id: server.id.clone(),
            transport: server.transport.clone(),
            transport_label: display_mcp_kind(&server.transport).to_owned(),
            enabled: server.enabled,
            endpoint_summary: endpoint_summary(server),
            cwd: server.cwd.clone(),
            env_keys: server.env.keys().cloned().collect(),
            header_keys: server.headers.keys().cloned().collect(),
            enabled_tools: server.enabled_tools.clone(),
            disabled_tools: server.disabled_tools.clone(),
            startup_timeout_ms: server.startup_timeout_ms,
            tool_timeout_ms: server.tool_timeout_ms,
            tools: if server.enabled {
                McpToolDiscovery::NotRequested
            } else {
                McpToolDiscovery::SkippedDisabled
            },
        })
        .collect()
}

/// Refresh the connection manager from the current application config.
pub async fn reload_mcp_manager_from_config(
    config: &crate::config::AppConfig,
    manager: &McpConnectionManager,
) -> anyhow::Result<Vec<McpServerSnapshot>> {
    let managed_configs = to_managed_configs(&config.mcp.servers)?;
    Ok(manager.apply_config(managed_configs).await)
}

/// Merge configured server details with live connection-manager snapshots.
///
/// This produces the same [`McpServerSummary`] shape used by the static
/// summary helpers, but replaces the undiscovered tool sentinel with the
/// manager's actual status. Servers present only in config (no snapshot yet)
/// fall back to the static behavior.
#[must_use]
pub fn summarize_mcp_servers_from_snapshots(
    config: &crate::config::AppConfig,
    snapshots: &[McpServerSnapshot],
) -> Vec<McpServerSummary> {
    let mut summaries = summarize_mcp_servers_without_discovery(config);
    for summary in &mut summaries {
        let Some(snapshot) = snapshots.iter().find(|s| s.id == summary.id) else {
            continue;
        };
        summary.tools = match snapshot.status {
            McpServerStatus::Connected => McpToolDiscovery::Success(snapshot.tool_names.clone()),
            McpServerStatus::Failed => McpToolDiscovery::Failed(
                snapshot
                    .error
                    .as_ref()
                    .map_or_else(|| "connection failed".to_owned(), |d| d.message.clone()),
            ),
            McpServerStatus::Pending | McpServerStatus::Reconnecting => {
                McpToolDiscovery::NotRequested
            }
            McpServerStatus::Disabled => McpToolDiscovery::SkippedDisabled,
        };
    }
    summaries
}

/// Connect to every configured MCP server and return settled snapshots.
///
/// This creates a temporary connection manager, applies the current config, and
/// waits until every server is either connected or failed (or the overall
/// timeout elapses). It is intended for short-lived CLI diagnostics such as
/// `neo mcp status`.
pub async fn probe_mcp_servers(
    config: &crate::config::AppConfig,
) -> anyhow::Result<Vec<McpServerSnapshot>> {
    let manager = McpConnectionManager::new(ProcessSupervisor::default());
    reload_mcp_manager_from_config(config, &manager).await?;
    let timeout = Duration::from_secs(10);
    let start = Instant::now();
    loop {
        let snapshots = manager.snapshots().await;
        let all_settled = snapshots.iter().all(|s| {
            !matches!(
                s.status,
                McpServerStatus::Pending | McpServerStatus::Reconnecting
            )
        });
        if all_settled || start.elapsed() >= timeout {
            return Ok(snapshots);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// List MCP resources exposed by connected servers.
pub async fn list_mcp_resources(
    config: &crate::config::AppConfig,
    server_id: Option<&str>,
) -> anyhow::Result<Vec<McpResourceListEntry>> {
    let manager = McpConnectionManager::new(ProcessSupervisor::default());
    reload_mcp_manager_from_config(config, &manager).await?;
    let timeout = Duration::from_secs(10);
    let start = Instant::now();
    loop {
        let snapshots = manager.snapshots().await;
        let all_settled = snapshots.iter().all(|s| {
            !matches!(
                s.status,
                McpServerStatus::Pending | McpServerStatus::Reconnecting
            )
        });
        if all_settled || start.elapsed() >= timeout {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    manager.list_resources(server_id).await
}

/// Read a single MCP resource from the named server.
pub async fn read_mcp_resource(
    config: &crate::config::AppConfig,
    server_id: &str,
    uri: &str,
) -> anyhow::Result<McpResourceRead> {
    let manager = McpConnectionManager::new(ProcessSupervisor::default());
    reload_mcp_manager_from_config(config, &manager).await?;
    let timeout = Duration::from_secs(10);
    let start = Instant::now();
    loop {
        let snapshots = manager.snapshots().await;
        let all_settled = snapshots.iter().all(|s| {
            !matches!(
                s.status,
                McpServerStatus::Pending | McpServerStatus::Reconnecting
            )
        });
        if all_settled || start.elapsed() >= timeout {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    manager.read_resource(server_id, uri).await
}

/// Render MCP server snapshots as a human-readable status table.
#[must_use]
pub fn format_mcp_status(snapshots: &[McpServerSnapshot]) -> String {
    if snapshots.is_empty() {
        return "No MCP servers configured.".to_owned();
    }
    let mut lines = Vec::with_capacity(snapshots.len() + 1);
    lines.push(format!(
        "{:<20} {:<12} {:<10} {:<8} {}",
        "ID", "Transport", "Status", "Tools", "Details"
    ));
    for snapshot in snapshots {
        let status = snapshot.status.as_str();
        let detail = match &snapshot.error {
            Some(diag) => format!("{} — {}", diag.message, diag.hint.as_deref().unwrap_or(""))
                .trim_end_matches(" — ")
                .to_owned(),
            None => String::new(),
        };
        lines.push(format!(
            "{:<20} {:<12} {:<10} {:<8} {}",
            snapshot.id, snapshot.transport, status, snapshot.tool_count, detail
        ));
    }
    lines.join("\n")
}

fn endpoint_summary(server: &McpServerConfig) -> String {
    if let Some(command) = &server.command {
        let mut summary = command.clone();
        if !server.args.is_empty() {
            summary.push(' ');
            summary.push_str(&server.args.join(" "));
        }
        return summary;
    }
    server.url.clone().unwrap_or_default()
}

/// Hardcoded Linear OAuth provider for the local OAuth authenticator MVP.
pub fn linear_oauth_provider() -> OAuthProvider {
    OAuthProvider {
        id: "linear".to_owned(),
        client_id: std::env::var("NEO_OAUTH_LINEAR_CLIENT_ID").unwrap_or_else(|_| "neo".to_owned()),
        auth_url: "https://api.linear.app/oauth/authorize".to_owned(),
        token_url: "https://api.linear.app/oauth/token".to_owned(),
        scopes: vec!["write".to_owned()],
        default_callback_port: 0,
    }
}

/// Pick an OAuth provider for an MCP server based on its transport and URL.
///
/// Returns `None` for non-remote transports or when no provider is known for
/// the server's URL.
pub fn detect_oauth_provider_for_server(server: &McpServerConfig) -> Option<OAuthProvider> {
    if server.transport != "http" && server.transport != "sse" {
        return None;
    }
    server
        .url
        .as_deref()
        .filter(|url| url.contains("linear.app"))
        .map(|_| linear_oauth_provider())
}

/// Run the OAuth authorization-code flow for a configured MCP server, save the
/// resulting token to `~/.neo/oauth.json`, and return it.
///
/// The caller is responsible for opening a browser and surfacing status to the
/// user. This helper performs PKCE generation, callback-server setup, token
/// exchange, and persistent storage.
pub async fn authenticate_mcp_server_oauth(
    server_id: &str,
    server: &McpServerConfig,
    neo_home: &Path,
) -> Result<OAuthTokenSet, OAuthError> {
    let mut provider =
        detect_oauth_provider_for_server(server).ok_or(OAuthError::ProviderDetection)?;

    let (verifier, challenge) = generate_pkce();
    let state = uuid::Uuid::new_v4().to_string();

    let callback_server = CallbackServer::start(state.clone(), Duration::from_secs(300)).await?;
    provider.default_callback_port = callback_server.local_port;

    let auth_url = build_authorization_url(&provider, &state, &challenge)?;
    let _ = webbrowser::open(auth_url.as_str());

    let code = callback_server.wait_for_code().await?;
    let token = exchange_code_for_token(&provider, &code.code, &verifier).await?;

    let store_path = neo_home.join("oauth.json");
    let token_key = format!("mcp:{server_id}");
    let mut store = OAuthStore::load(&store_path)?;
    store.set_token(&token_key, token.clone());
    store.save(&store_path)?;

    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mcp_kind_maps_aliases() {
        assert_eq!(parse_mcp_kind("studio").unwrap(), "stdio");
        assert_eq!(parse_mcp_kind("stdio").unwrap(), "stdio");
        assert_eq!(parse_mcp_kind("remote-http").unwrap(), "http");
        assert_eq!(parse_mcp_kind("http").unwrap(), "http");
        assert_eq!(parse_mcp_kind("remote-sse").unwrap(), "sse");
        assert_eq!(parse_mcp_kind("sse").unwrap(), "sse");
        assert!(parse_mcp_kind("unknown").is_err());
    }

    #[test]
    fn display_mcp_kind_round_trips() {
        assert_eq!(display_mcp_kind("stdio"), "studio");
        assert_eq!(display_mcp_kind("http"), "remote-http");
        assert_eq!(display_mcp_kind("sse"), "remote-sse");
    }

    #[test]
    fn parse_command_string_splits_args() {
        let (cmd, args) = parse_command_string("npx -y @server/filesystem /repo").unwrap();
        assert_eq!(cmd, "npx");
        assert_eq!(args, vec!["-y", "@server/filesystem", "/repo"]);
    }

    #[test]
    fn key_value_pairs_parses_and_trims() {
        let pairs = key_value_pairs(
            vec!["KEY=value".to_owned(), "OTHER =  spaced  ".to_owned()],
            "--env",
        )
        .unwrap();
        assert_eq!(pairs.get("KEY").unwrap(), "value");
        assert_eq!(pairs.get("OTHER").unwrap(), "spaced");
    }

    #[test]
    fn key_value_pairs_rejects_missing_equals() {
        assert!(key_value_pairs(vec!["KEYVALUE".to_owned()], "--env").is_err());
    }

    #[test]
    fn build_mcp_server_config_stdio_requires_command() {
        let input = AddMcpServerInput {
            id: "fs".to_owned(),
            cli_type: "studio".to_owned(),
            command: None,
            url: None,
            env: vec![],
            headers: vec![],
            cwd: None,
            enabled_tools: vec![],
            disabled_tools: vec![],
            startup_timeout_ms: None,
            tool_timeout_ms: None,
            enabled: true,
        };
        assert!(build_mcp_server_config(input).is_err());
    }

    #[test]
    fn build_mcp_server_config_http_rejects_command() {
        let input = AddMcpServerInput {
            id: "linear".to_owned(),
            cli_type: "remote-http".to_owned(),
            command: Some("npx".to_owned()),
            url: Some("https://example.invalid/mcp".to_owned()),
            env: vec![],
            headers: vec![],
            cwd: None,
            enabled_tools: vec![],
            disabled_tools: vec![],
            startup_timeout_ms: None,
            tool_timeout_ms: None,
            enabled: true,
        };
        assert!(build_mcp_server_config(input).is_err());
    }

    #[test]
    fn build_mcp_server_config_stdio_rejects_headers() {
        let input = AddMcpServerInput {
            id: "fs".to_owned(),
            cli_type: "studio".to_owned(),
            command: Some("npx -y @server/filesystem".to_owned()),
            url: None,
            env: vec![],
            headers: vec!["Authorization=secret".to_owned()],
            cwd: None,
            enabled_tools: vec![],
            disabled_tools: vec![],
            startup_timeout_ms: None,
            tool_timeout_ms: None,
            enabled: true,
        };
        assert!(build_mcp_server_config(input).is_err());
    }

    #[test]
    fn to_managed_config_preserves_filters_and_timeouts() {
        let server = McpServerConfig {
            id: "fs".to_owned(),
            enabled: true,
            transport: "stdio".to_owned(),
            command: Some("npx".to_owned()),
            url: None,
            args: vec!["-y".to_owned()],
            env: BTreeMap::new(),
            headers: BTreeMap::new(),
            cwd: None,
            enabled_tools: vec!["read".to_owned()],
            disabled_tools: vec!["write".to_owned()],
            startup_timeout_ms: Some(5_000),
            tool_timeout_ms: Some(10_000),
        };
        let managed = to_managed_config(&server).unwrap();
        assert_eq!(managed.enabled_tools, vec!["read"]);
        assert_eq!(managed.disabled_tools, vec!["write"]);
        assert_eq!(managed.startup_timeout_ms, Some(5_000));
        assert_eq!(managed.tool_timeout_ms, Some(10_000));
    }

    #[test]
    fn snapshot_summary_uses_real_tool_names() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project_dir = temp.path().join("project");
        let config = crate::config::AppConfig {
            default_model: "gpt-4.1".to_owned(),
            default_provider: "openai".to_owned(),
            api_key_env: None,
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
            model_scope: Vec::new(),
            sessions_dir: project_dir.join(".neo/sessions"),
            permission_mode: neo_agent_core::PermissionMode::Ask,
            live_permission_mode: std::sync::Arc::new(std::sync::RwLock::new(
                neo_agent_core::PermissionMode::Ask,
            )),
            defaults: crate::config::Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: crate::config::RuntimeConfig::default(),
            tui: crate::config::TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: crate::config::McpConfig {
                servers: vec![McpServerConfig {
                    id: "docs".to_owned(),
                    enabled: true,
                    transport: "stdio".to_owned(),
                    command: Some("docs-mcp".to_owned()),
                    url: None,
                    args: Vec::new(),
                    env: BTreeMap::new(),
                    headers: BTreeMap::new(),
                    cwd: None,
                    enabled_tools: Vec::new(),
                    disabled_tools: Vec::new(),
                    startup_timeout_ms: None,
                    tool_timeout_ms: None,
                }],
            },
            prompt_templates: Vec::new(),
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir,
            config_path: temp.path().join("config.toml"),
        };
        let summaries = summarize_mcp_servers_from_snapshots(
            &config,
            &[McpServerSnapshot {
                id: "docs".to_owned(),
                transport: "stdio".to_owned(),
                status: McpServerStatus::Connected,
                tool_count: 2,
                tool_names: vec!["read_doc".to_owned(), "search_doc".to_owned()],
                resource_count: Some(0),
                error: None,
                reconnect_attempt: 0,
                next_retry_ms: None,
            }],
        );

        assert_eq!(
            summaries[0].tools,
            McpToolDiscovery::Success(vec!["read_doc".to_owned(), "search_doc".to_owned()])
        );
    }
}
