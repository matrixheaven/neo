use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use neo_agent_core::{
    ManagedMcpTransport, McpConnectionManager, McpHttpConfig, McpHttpToolAdapter,
    McpReconnectPolicy, McpResourceListEntry, McpResourceRead, McpServerSnapshot, McpServerStatus,
    McpStdioConfig, McpStdioToolAdapter, McpToolAdapter, ProcessSupervisor,
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

/// Apply enabled/disabled tool filters to a list of tool names.
pub fn apply_tool_filter(
    tools: &mut Vec<String>,
    enabled_tools: &[String],
    disabled_tools: &[String],
) {
    if !enabled_tools.is_empty() {
        let allow: BTreeSet<_> = enabled_tools.iter().cloned().collect();
        tools.retain(|name| allow.contains(name));
    }
    if !disabled_tools.is_empty() {
        let deny: BTreeSet<_> = disabled_tools.iter().cloned().collect();
        tools.retain(|name| !deny.contains(name));
    }
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

/// Build an unsupervised adapter for short-lived CLI probe/list operations.
pub fn mcp_adapter_for_server(server: &McpServerConfig) -> anyhow::Result<Arc<dyn McpToolAdapter>> {
    match server.transport.as_str() {
        "stdio" => {
            let command = server
                .command
                .clone()
                .with_context(|| format!("missing MCP command for {}", server.id))?;
            Ok(Arc::new(McpStdioToolAdapter::new(McpStdioConfig {
                command,
                args: server.args.clone(),
                env: server.env.clone(),
                cwd: server.cwd.clone(),
                tool_timeout_ms: server.tool_timeout_ms,
            })))
        }
        "http" | "sse" => {
            let url = server
                .url
                .clone()
                .with_context(|| format!("missing MCP url for {}", server.id))?;
            Ok(Arc::new(McpHttpToolAdapter::new(McpHttpConfig {
                url,
                headers: server.headers.clone(),
                tool_timeout_ms: server.tool_timeout_ms,
            })))
        }
        other => anyhow::bail!("unsupported MCP transport for {}: {other}", server.id),
    }
}

/// Probe a server by listing its tools with an optional timeout.
pub async fn probe_mcp_server(
    server: &McpServerConfig,
    timeout_ms: Option<u64>,
) -> anyhow::Result<Vec<String>> {
    let adapter = mcp_adapter_for_server(server)?;
    let fut = adapter.list_tools();
    let tools = if let Some(ms) = timeout_ms {
        tokio::time::timeout(Duration::from_millis(ms), fut)
            .await
            .with_context(|| format!("timeout connecting to MCP server {}", server.id))??
    } else {
        fut.await
            .with_context(|| format!("failed to list tools from {}", server.id))?
    };
    let mut names: Vec<String> = tools.into_iter().map(|t| t.name).collect();
    apply_tool_filter(&mut names, &server.enabled_tools, &server.disabled_tools);
    Ok(names)
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
            McpServerStatus::Connected => McpToolDiscovery::Success(
                (0..snapshot.tool_count)
                    .map(|i| format!("tool-{i}"))
                    .collect(),
            ),
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
    fn apply_tool_filter_allows_and_blocks() {
        let mut tools = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        apply_tool_filter(&mut tools, &["a".to_owned(), "c".to_owned()], &[]);
        assert_eq!(tools, vec!["a", "c"]);

        let mut tools = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        apply_tool_filter(&mut tools, &[], &["b".to_owned()]);
        assert_eq!(tools, vec!["a", "c"]);
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
}
