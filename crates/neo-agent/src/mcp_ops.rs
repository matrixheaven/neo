use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::Context;
use neo_agent_core::{
    ManagedMcpTransport, McpConnectionManager, McpDiagnostic, McpOAuthIdentity, McpOAuthService,
    McpOAuthServiceConfig, McpOAuthTransportKind, McpReconnectPolicy, McpResourceListEntry,
    McpResourceRead, McpServerSnapshot, McpServerStatus, ProcessSupervisor,
    build_authorization_manager, oauth::callback_server::CallbackServer,
};
use neo_tui::transcript::{McpStartupPhase, McpStartupStatusData};

use crate::config::{McpServerConfig, McpTransport, neo_home};

pub use neo_agent_core::ManagedMcpServerConfig;

/// Input used by both CLI and TUI to add or update an MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddMcpServerInput {
    pub id: String,
    pub cli_type: String,
    pub command: Option<String>,
    pub args: Vec<String>,
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
    NeedsAuth(String),
    Failed(String),
}

/// Convert CLI/user-facing type labels to the persisted transport enum.
pub fn parse_mcp_kind(type_arg: &str) -> anyhow::Result<McpTransport> {
    match type_arg {
        "studio" | "stdio" => Ok(McpTransport::Stdio),
        "remote-http" | "http" => Ok(McpTransport::Http),
        "remote-sse" | "sse" => Ok(McpTransport::Sse),
        other => {
            anyhow::bail!("unknown MCP type '{other}'; expected studio, remote-http, or remote-sse")
        }
    }
}

/// Convert a persisted transport enum back to a CLI/user-facing label.
#[must_use]
pub fn display_mcp_kind(transport: McpTransport) -> &'static str {
    match transport {
        McpTransport::Stdio => "studio",
        McpTransport::Http => "remote-http",
        McpTransport::Sse => "remote-sse",
    }
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
    match server.transport {
        McpTransport::Stdio => {
            anyhow::ensure!(
                server
                    .command
                    .as_deref()
                    .is_some_and(|command| !command.is_empty()),
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
        McpTransport::Http | McpTransport::Sse => {
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
    }
    Ok(())
}

/// Build a persisted `McpServerConfig` from user input.
pub fn build_mcp_server_config(input: AddMcpServerInput) -> anyhow::Result<McpServerConfig> {
    let transport = parse_mcp_kind(&input.cli_type)?;

    let command = if transport == McpTransport::Stdio {
        let Some(command) = input.command else {
            anyhow::bail!("studio MCP requires a command");
        };
        Some(command)
    } else {
        if input.command.is_some() {
            anyhow::bail!("remote MCP uses url, not command");
        }
        anyhow::ensure!(input.args.is_empty(), "remote MCP cannot use arguments");
        None
    };

    let url = if transport == McpTransport::Http || transport == McpTransport::Sse {
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

    if transport != McpTransport::Http
        && transport != McpTransport::Sse
        && !input.headers.is_empty()
    {
        anyhow::bail!("--header is only valid for remote-http / remote-sse");
    }
    if transport != McpTransport::Stdio && input.cwd.is_some() {
        anyhow::bail!("--cwd is only valid for studio");
    }

    Ok(McpServerConfig {
        id: input.id,
        enabled: input.enabled,
        transport,
        command,
        url,
        args: input.args,
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
    let transport = match server.transport {
        McpTransport::Stdio => {
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
        McpTransport::Http => {
            let url = server.url.clone().context("missing MCP url for {}")?;
            ManagedMcpTransport::Http {
                url,
                headers: server.headers.clone(),
            }
        }
        McpTransport::Sse => {
            let url = server.url.clone().context("missing MCP url for {}")?;
            ManagedMcpTransport::Sse {
                url,
                headers: server.headers.clone(),
            }
        }
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
            transport: server.transport.as_str().to_owned(),
            transport_label: display_mcp_kind(server.transport).to_owned(),
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
    let service = mcp_oauth_service_for_current_home();
    manager.set_oauth_service(service).await;

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
            McpServerStatus::NeedsAuth => {
                McpToolDiscovery::NeedsAuth(snapshot.error.as_ref().map_or_else(
                    || "OAuth authentication required".to_owned(),
                    |d| format_mcp_diagnostic(d, false),
                ))
            }
            McpServerStatus::Failed => {
                McpToolDiscovery::Failed(snapshot.error.as_ref().map_or_else(
                    || "connection failed".to_owned(),
                    |d| format_mcp_diagnostic(d, false),
                ))
            }
            McpServerStatus::Pending | McpServerStatus::Reconnecting => {
                McpToolDiscovery::NotRequested
            }
            McpServerStatus::Cancelled => McpToolDiscovery::NotRequested,
            McpServerStatus::Disabled => McpToolDiscovery::SkippedDisabled,
        };
    }
    summaries
}

#[must_use]
pub fn mcp_startup_connecting_status(server: &McpServerConfig) -> Option<McpStartupStatusData> {
    server.enabled.then(|| McpStartupStatusData {
        id: server.id.clone(),
        transport: display_mcp_kind(server.transport).to_owned(),
        phase: McpStartupPhase::Connecting,
    })
}

#[must_use]
pub fn mcp_startup_connecting_statuses(
    config: &crate::config::AppConfig,
) -> Vec<McpStartupStatusData> {
    config
        .mcp
        .servers
        .iter()
        .filter_map(mcp_startup_connecting_status)
        .collect()
}

#[must_use]
pub fn mcp_startup_status_from_snapshot(snapshot: &McpServerSnapshot) -> McpStartupStatusData {
    let phase = match snapshot.status {
        McpServerStatus::Connected => McpStartupPhase::Connected {
            tool_count: snapshot.tool_count,
        },
        McpServerStatus::NeedsAuth => McpStartupPhase::NeedsAuth {
            hint: snapshot.error.as_ref().map_or_else(
                || "Run /mcp to authenticate.".to_owned(),
                |diagnostic| {
                    diagnostic
                        .hint
                        .clone()
                        .unwrap_or_else(|| diagnostic.message.clone())
                },
            ),
        },
        McpServerStatus::Failed => McpStartupPhase::Failed {
            message: snapshot.error.as_ref().map_or_else(
                || "connection failed".to_owned(),
                |d| format_mcp_diagnostic(d, false),
            ),
        },
        McpServerStatus::Pending | McpServerStatus::Reconnecting => McpStartupPhase::Connecting,
        McpServerStatus::Cancelled => McpStartupPhase::Cancelled,
        McpServerStatus::Disabled => McpStartupPhase::Disabled,
    };
    McpStartupStatusData {
        id: snapshot.id.clone(),
        transport: snapshot.transport.clone(),
        phase,
    }
}

#[must_use]
pub fn mcp_startup_failed_statuses(
    config: &crate::config::AppConfig,
    message: &str,
) -> Vec<McpStartupStatusData> {
    config
        .mcp
        .servers
        .iter()
        .filter(|server| server.enabled)
        .map(|server| McpStartupStatusData {
            id: server.id.clone(),
            transport: display_mcp_kind(server.transport).to_owned(),
            phase: McpStartupPhase::Failed {
                message: message.to_owned(),
            },
        })
        .collect()
}

#[must_use]
pub fn format_mcp_startup_message(snapshot: &McpServerSnapshot) -> String {
    match snapshot.status {
        McpServerStatus::Connected => format!(
            "MCP server \"{}\" connected · {} tools ({})",
            snapshot.id, snapshot.tool_count, snapshot.transport
        ),
        McpServerStatus::NeedsAuth => format!(
            "MCP server \"{}\" needs OAuth · {}",
            snapshot.id,
            snapshot
                .error
                .as_ref()
                .map_or("Run /mcp to authenticate.", |diagnostic| {
                    diagnostic
                        .hint
                        .as_deref()
                        .unwrap_or(diagnostic.message.as_str())
                })
        ),
        McpServerStatus::Failed => format!(
            "MCP server \"{}\" failed · {}",
            snapshot.id,
            snapshot.error.as_ref().map_or_else(
                || "connection failed".to_owned(),
                |diagnostic| { format_mcp_diagnostic(diagnostic, false) }
            )
        ),
        McpServerStatus::Pending | McpServerStatus::Reconnecting => format!(
            "MCP server \"{}\" still connecting ({})",
            snapshot.id, snapshot.transport
        ),
        McpServerStatus::Cancelled => format!(
            "MCP server \"{}\" startup interrupted ({})",
            snapshot.id, snapshot.transport
        ),
        McpServerStatus::Disabled => {
            format!(
                "MCP server \"{}\" disabled ({})",
                snapshot.id, snapshot.transport
            )
        }
    }
}

pub async fn wait_for_mcp_manager_probe(
    manager: &McpConnectionManager,
    config: &crate::config::AppConfig,
) -> Vec<McpServerSnapshot> {
    let enabled_count = config
        .mcp
        .servers
        .iter()
        .filter(|server| server.enabled)
        .count();
    if enabled_count == 0 {
        return Vec::new();
    }
    loop {
        let snapshots = manager.snapshots().await;
        let settled = snapshots.iter().all(|snapshot| {
            !matches!(
                snapshot.status,
                McpServerStatus::Pending | McpServerStatus::Reconnecting
            )
        });
        if settled {
            return snapshots
                .into_iter()
                .filter(|snapshot| {
                    config
                        .mcp
                        .servers
                        .iter()
                        .any(|server| server.enabled && server.id == snapshot.id)
                })
                .collect();
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
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
            Some(diag) => format_mcp_diagnostic(diag, true),
            None => String::new(),
        };
        lines.push(format!(
            "{:<20} {:<12} {:<10} {:<8} {}",
            snapshot.id, snapshot.transport, status, snapshot.tool_count, detail
        ));
    }
    lines.join("\n")
}

fn format_mcp_diagnostic(diagnostic: &McpDiagnostic, include_hint: bool) -> String {
    fn sanitize_line(value: &str) -> String {
        neo_tui::utils::shell_output::sanitize_shell_output(value)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" | ")
    }

    let mut parts = vec![sanitize_line(&diagnostic.message)];
    if include_hint && let Some(hint) = diagnostic.hint.as_deref() {
        let hint = sanitize_line(hint);
        if !hint.is_empty() {
            parts.push(hint);
        }
    }
    if let Some(stderr_tail) = diagnostic.stderr_tail.as_deref() {
        let stderr_tail = sanitize_line(stderr_tail);
        if !stderr_tail.is_empty() {
            parts.push(format!("stderr: {stderr_tail}"));
        }
    }
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
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

/// Run the OAuth authorization-code flow for a configured MCP server using
/// rmcp's discovery-based authorization (RFC 8414 / RFC 7591).
///
/// This discovers OAuth metadata from the MCP server URL, dynamically registers
/// a client, and performs a browser-based PKCE authorization-code flow. The
/// resulting token is imported into Neo's per-MCP credential store.
#[allow(clippy::duration_suboptimal_units)]
pub async fn authenticate_mcp_server_oauth(
    server_id: &str,
    server: &McpServerConfig,
    neo_home: &std::path::Path,
) -> Result<(), anyhow::Error> {
    let url = server
        .url
        .as_deref()
        .with_context(|| format!("missing MCP server url for {server_id}"))?;

    let service = McpOAuthService::new(McpOAuthServiceConfig {
        neo_home: Some(neo_home.to_path_buf()),
    });
    let identity = mcp_oauth_identity_for_server(server_id, server)?;

    // Build rmcp AuthorizationManager (performs discovery from server URL).
    let manager = build_authorization_manager(url, &service, identity.clone())
        .await
        .context("failed to initialize OAuth authorization manager")?;

    // Start callback server (rmcp validates state during token exchange, so we
    // skip state validation here).
    let callback_server = CallbackServer::start_unvalidated(Duration::from_secs(300))
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let port = callback_server.local_port;
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    {
        let mut mgr = manager.lock().await;

        // Discover OAuth metadata (RFC 8414 / RFC 9728) from the server.
        // This is REQUIRED before register_client() can succeed.
        let metadata = mgr
            .discover_metadata()
            .await
            .context("failed to discover OAuth metadata from server")?;
        mgr.set_metadata(metadata.clone());

        // Dynamically register the client with the redirect URI.
        let client_config = mgr
            .register_client("neo", &redirect_uri, &[])
            .await
            .context("OAuth dynamic client registration failed")?;
        service
            .persist_client_and_discovery(&identity, &client_config, metadata)
            .context("failed to persist OAuth client metadata to Neo MCP credential store")?;

        // Get the authorization URL (PKCE + CSRF state are generated internally).
        let auth_url = mgr
            .get_authorization_url(&[])
            .await
            .context("failed to build OAuth authorization URL")?;

        let _ = webbrowser::open(&auth_url);
    }

    // Wait for the browser callback.
    let code = callback_server
        .wait_for_code()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Fail fast on obviously malformed callbacks: the CSRF state parameter must
    // be present. rmcp's `exchange_code_for_token` validates state against its
    // internal `StateStore`; this check just prevents calling it with garbage.
    if code.state.is_empty() {
        tracing::warn!("OAuth callback received with empty CSRF state parameter");
        anyhow::bail!("OAuth callback missing CSRF state parameter");
    }

    // Exchange the code for a token (rmcp validates state and persists credentials).
    {
        let mgr = manager.lock().await;
        mgr.exchange_code_for_token(&code.code, &code.state)
            .await
            .context("failed to exchange authorization code for token")?;
    }

    Ok(())
}

#[must_use]
pub(crate) fn mcp_oauth_service_for_current_home() -> McpOAuthService {
    McpOAuthService::new(McpOAuthServiceConfig {
        neo_home: neo_home(),
    })
}

pub(crate) fn mcp_oauth_identity_for_server(
    server_id: &str,
    server: &McpServerConfig,
) -> anyhow::Result<McpOAuthIdentity> {
    let url = server
        .url
        .as_deref()
        .with_context(|| format!("missing MCP server url for {server_id}"))?;
    let transport_kind = match server.transport {
        McpTransport::Http => McpOAuthTransportKind::Http,
        McpTransport::Sse => McpOAuthTransportKind::Sse,
        McpTransport::Stdio => {
            anyhow::bail!(
                "MCP server '{server_id}' does not use an HTTP/SSE OAuth-capable transport"
            )
        }
    };
    McpOAuthIdentity::new(server_id, url, transport_kind)
        .map_err(|err| anyhow::anyhow!("invalid MCP OAuth identity for '{server_id}': {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mcp_kind_maps_aliases() {
        assert_eq!(parse_mcp_kind("studio").unwrap(), McpTransport::Stdio);
        assert_eq!(parse_mcp_kind("stdio").unwrap(), McpTransport::Stdio);
        assert_eq!(parse_mcp_kind("remote-http").unwrap(), McpTransport::Http);
        assert_eq!(parse_mcp_kind("http").unwrap(), McpTransport::Http);
        assert_eq!(parse_mcp_kind("remote-sse").unwrap(), McpTransport::Sse);
        assert_eq!(parse_mcp_kind("sse").unwrap(), McpTransport::Sse);
        assert!(parse_mcp_kind("unknown").is_err());
    }

    #[test]
    fn display_mcp_kind_round_trips() {
        assert_eq!(display_mcp_kind(McpTransport::Stdio), "studio");
        assert_eq!(display_mcp_kind(McpTransport::Http), "remote-http");
        assert_eq!(display_mcp_kind(McpTransport::Sse), "remote-sse");
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
            args: vec![],
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
    fn validate_stdio_program_rejects_empty_without_trimming() {
        let mut server = McpServerConfig {
            id: "fs".to_owned(),
            enabled: true,
            transport: McpTransport::Stdio,
            command: Some(String::new()),
            url: None,
            args: vec![],
            env: BTreeMap::new(),
            headers: BTreeMap::new(),
            cwd: None,
            enabled_tools: vec![],
            disabled_tools: vec![],
            startup_timeout_ms: None,
            tool_timeout_ms: None,
        };
        assert!(validate_mcp_server_config(&server).is_err());

        server.command = Some("  npx  ".to_owned());
        assert!(validate_mcp_server_config(&server).is_ok());
        assert_eq!(server.command.as_deref(), Some("  npx  "));
    }

    #[test]
    fn build_mcp_server_config_http_rejects_command() {
        let input = AddMcpServerInput {
            id: "linear".to_owned(),
            cli_type: "remote-http".to_owned(),
            command: Some("npx".to_owned()),
            args: vec![],
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
            command: Some("npx".to_owned()),
            args: vec!["-y".to_owned(), "@server/filesystem".to_owned()],
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
            transport: McpTransport::Stdio,
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
            workspace_policy: std::sync::Arc::new(std::sync::RwLock::new(None)),
            defaults: crate::config::Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: crate::config::RuntimeConfig::default(),
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            workflow_capability: neo_agent_core::workflow::WorkflowCapability::default(),
            multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
            tui: crate::config::TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: crate::config::McpConfig {
                servers: vec![McpServerConfig {
                    id: "docs".to_owned(),
                    enabled: true,
                    transport: McpTransport::Stdio,
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
            system_prompt_file: None,
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir,
            config_path: temp.path().join("config.toml"),
            config_file_exists: true,
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

    #[test]
    fn snapshot_summary_maps_needs_auth() {
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
            workspace_policy: std::sync::Arc::new(std::sync::RwLock::new(None)),
            defaults: crate::config::Defaults {
                mode: "interactive".to_owned(),
            },
            runtime: crate::config::RuntimeConfig::default(),
            background_tasks: neo_agent_core::BackgroundTaskManager::new(),
            workflow_capability: neo_agent_core::workflow::WorkflowCapability::default(),
            multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
            tui: crate::config::TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
            mcp: crate::config::McpConfig {
                servers: vec![McpServerConfig {
                    id: "linear".to_owned(),
                    enabled: true,
                    transport: McpTransport::Http,
                    command: None,
                    url: Some("https://mcp.example.com/mcp".to_owned()),
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
            system_prompt_file: None,
            extra_skill_dirs: Vec::new(),
            skill_path: Vec::new(),
            project_trusted: true,
            project_trust: crate::trust::ProjectTrustState::NotRequired,
            project_dir,
            config_path: temp.path().join("config.toml"),
            config_file_exists: true,
        };
        let summaries = summarize_mcp_servers_from_snapshots(
            &config,
            &[McpServerSnapshot {
                id: "linear".to_owned(),
                transport: "http".to_owned(),
                status: McpServerStatus::NeedsAuth,
                tool_count: 0,
                tool_names: Vec::new(),
                resource_count: None,
                error: Some(neo_agent_core::McpDiagnostic {
                    server_id: "linear".to_owned(),
                    transport: "http".to_owned(),
                    message: "OAuth authentication required".to_owned(),
                    hint: Some("Run /mcp and authenticate this server.".to_owned()),
                    stderr_tail: Some("\x1b]0;owned\x07authorization failed".to_owned()),
                }),
                reconnect_attempt: 0,
                next_retry_ms: None,
            }],
        );

        assert_eq!(
            summaries[0].tools,
            McpToolDiscovery::NeedsAuth(
                "OAuth authentication required · stderr: authorization failed".to_owned()
            )
        );
    }

    #[test]
    fn startup_message_formats_connected_server_like_kimi() {
        let snapshot = McpServerSnapshot {
            id: "linear".to_owned(),
            transport: "http".to_owned(),
            status: McpServerStatus::Connected,
            tool_count: 38,
            tool_names: Vec::new(),
            resource_count: None,
            error: None,
            reconnect_attempt: 0,
            next_retry_ms: None,
        };

        assert_eq!(
            format_mcp_startup_message(&snapshot),
            "MCP server \"linear\" connected · 38 tools (http)"
        );
    }

    #[test]
    fn startup_status_data_maps_connected_snapshot() {
        let snapshot = McpServerSnapshot {
            id: "linear".to_owned(),
            transport: "http".to_owned(),
            status: McpServerStatus::Connected,
            tool_count: 38,
            tool_names: Vec::new(),
            resource_count: None,
            error: None,
            reconnect_attempt: 0,
            next_retry_ms: None,
        };

        assert_eq!(
            mcp_startup_status_from_snapshot(&snapshot),
            neo_tui::transcript::McpStartupStatusData {
                id: "linear".to_owned(),
                transport: "http".to_owned(),
                phase: neo_tui::transcript::McpStartupPhase::Connected { tool_count: 38 },
            }
        );
    }

    #[test]
    fn startup_message_formats_needs_auth_with_hint() {
        let snapshot = McpServerSnapshot {
            id: "linear".to_owned(),
            transport: "http".to_owned(),
            status: McpServerStatus::NeedsAuth,
            tool_count: 0,
            tool_names: Vec::new(),
            resource_count: None,
            error: Some(neo_agent_core::McpDiagnostic {
                server_id: "linear".to_owned(),
                transport: "http".to_owned(),
                message: "OAuth authentication required".to_owned(),
                hint: Some("Run /mcp to authenticate.".to_owned()),
                stderr_tail: None,
            }),
            reconnect_attempt: 0,
            next_retry_ms: None,
        };

        assert_eq!(
            format_mcp_startup_message(&snapshot),
            "MCP server \"linear\" needs OAuth · Run /mcp to authenticate."
        );
    }

    #[test]
    fn failed_mcp_diagnostics_render_sanitized_stderr_tail() {
        let snapshot = McpServerSnapshot {
            id: "broken".to_owned(),
            transport: "stdio".to_owned(),
            status: McpServerStatus::Failed,
            tool_count: 0,
            tool_names: Vec::new(),
            resource_count: None,
            error: Some(neo_agent_core::McpDiagnostic {
                server_id: "broken".to_owned(),
                transport: "stdio".to_owned(),
                message: "connection closed".to_owned(),
                hint: None,
                stderr_tail: Some("\x1b]0;owned\x07visible failure\nsecond line".to_owned()),
            }),
            reconnect_attempt: 0,
            next_retry_ms: None,
        };

        let status = format_mcp_status(std::slice::from_ref(&snapshot));
        let startup = mcp_startup_status_from_snapshot(&snapshot);

        assert!(status.contains("stderr: visible failure | second line"));
        assert!(!status.contains('\x1b'));
        assert_eq!(
            startup.phase,
            McpStartupPhase::Failed {
                message: "connection closed · stderr: visible failure | second line".to_owned(),
            }
        );
    }
}
