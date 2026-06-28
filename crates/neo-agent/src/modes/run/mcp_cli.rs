use std::{
    collections::BTreeMap,
    fmt::Write as _,
    path::PathBuf,
};

use anyhow::Context;
use neo_agent_core::ProcessSupervisor;

use crate::mcp_ops::{
    authenticate_mcp_server_oauth, display_mcp_kind, parse_command_string, parse_mcp_kind,
};
use crate::config::{self, AppConfig, McpServerConfig, McpTransport, neo_home};

pub(crate) async fn list_mcp(config: &AppConfig) -> String {
    if config.mcp.servers.is_empty() {
        return "no MCP servers configured\n".to_owned();
    }

    let mut out = String::new();
    for (idx, server) in config.mcp.servers.iter().enumerate() {
        let kind = display_mcp_kind(server.transport);
        let _ = writeln!(out, "[{}]<{}>({})", idx + 1, server.id, kind);

        if !server.enabled {
            let _ = writeln!(out, "{{}}");
            continue;
        }

        match list_mcp_tools_for_server(server).await {
            Ok(tools) => {
                let map: serde_json::Map<String, serde_json::Value> = tools
                    .into_iter()
                    .enumerate()
                    .map(|(i, name)| ((i + 1).to_string(), serde_json::Value::String(name)))
                    .collect();
                let _ = writeln!(
                    out,
                    "{}",
                    serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_owned())
                );
            }
            Err(_) => {
                let _ = writeln!(out, "{{}}");
            }
        }
    }
    out
}

async fn list_mcp_tools_for_server(server: &McpServerConfig) -> anyhow::Result<Vec<String>> {
    let supervisor = ProcessSupervisor::default();
    let client = super::runtime::build_mcp_client(server, &supervisor).await?;
    let tools = client
        .list_tools()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut tools: Vec<String> = tools.into_iter().map(|t| t.name).collect();
    apply_tool_filter(&mut tools, &server.enabled_tools, &server.disabled_tools);
    Ok(tools)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn add_mcp_server(
    mcp_name: String,
    r#type: String,
    command: Option<String>,
    url: Option<String>,
    env: Vec<String>,
    headers: Vec<String>,
    cwd: Option<PathBuf>,
    enabled_tools: Vec<String>,
    disabled_tools: Vec<String>,
    startup_timeout_ms: Option<u64>,
    tool_timeout_ms: Option<u64>,
    enabled: bool,
    config: &AppConfig,
) -> anyhow::Result<String> {
    let transport = parse_mcp_kind(&r#type)?;

    let (command, args) = if transport == McpTransport::Stdio {
        let Some(cmd) = command else {
            anyhow::bail!("studio MCP requires --command");
        };
        let (cmd, args) = parse_command_string(&cmd)?;
        (Some(cmd), args)
    } else {
        if command.is_some() {
            anyhow::bail!("remote MCP uses --url, not --command");
        }
        (None, Vec::new())
    };

    let url = if transport == McpTransport::Http || transport == McpTransport::Sse {
        let Some(url) = url else {
            anyhow::bail!("remote MCP requires --url");
        };
        Some(url)
    } else {
        if url.is_some() {
            anyhow::bail!("studio MCP uses --command, not --url");
        }
        None
    };

    if transport != McpTransport::Http && transport != McpTransport::Sse && !headers.is_empty() {
        anyhow::bail!("--header is only valid for remote-http / remote-sse");
    }
    if transport != McpTransport::Stdio && cwd.is_some() {
        anyhow::bail!("--cwd is only valid for studio");
    }

    let server = McpServerConfig {
        id: mcp_name.clone(),
        enabled,
        transport,
        command,
        url,
        args,
        env: key_value_pairs(env, "--env")?,
        headers: key_value_pairs(headers, "--header")?,
        cwd,
        enabled_tools,
        disabled_tools,
        startup_timeout_ms,
        tool_timeout_ms,
    };

    let saved = config::mutations::upsert_mcp_server(&server, &config.config_path)?;

    if !enabled {
        return Ok(format!("{saved}{mcp_name} added (disabled)\n"));
    }

    let probe_result = probe_mcp_server(&server, startup_timeout_ms).await;
    let probe_msg = match probe_result {
        Ok(()) => format!("{mcp_name} successfully connected!\n"),
        Err(_) => format!("{mcp_name} connect failed\n"),
    };
    Ok(format!("{saved}{probe_msg}"))
}

/// Run the OAuth authorization-code flow for a configured MCP server and save
/// the resulting token to `~/.neo/oauth.json`.
pub(crate) async fn auth_mcp_server(server_id: String, config: &AppConfig) -> anyhow::Result<String> {
    let server = config
        .mcp
        .servers
        .iter()
        .find(|server| server.id == server_id)
        .context("MCP server not found")?;

    if server.transport != McpTransport::Http && server.transport != McpTransport::Sse {
        anyhow::bail!("OAuth is limited to HTTP/SSE servers");
    }

    let neo_home = neo_home().context("failed to resolve neo home directory")?;
    authenticate_mcp_server_oauth(&server_id, server, &neo_home).await?;

    Ok(format!("OAuth token saved for MCP server {server_id}\n"))
}

async fn probe_mcp_server(server: &McpServerConfig, timeout_ms: Option<u64>) -> anyhow::Result<()> {
    let supervisor = ProcessSupervisor::default();
    let client = super::runtime::build_mcp_client(server, &supervisor).await?;
    let fut = client.list_tools();
    let tools = if let Some(ms) = timeout_ms {
        tokio::time::timeout(std::time::Duration::from_millis(ms), fut)
            .await
            .with_context(|| format!("timeout connecting to MCP server {}", server.id))?
            .map_err(|e| anyhow::anyhow!("{e}"))?
    } else {
        fut.await
            .map_err(|e| anyhow::anyhow!("{e}"))
            .with_context(|| format!("failed to list tools from {}", server.id))?
    };
    let mut names: Vec<String> = tools.into_iter().map(|t| t.name).collect();
    apply_tool_filter(&mut names, &server.enabled_tools, &server.disabled_tools);
    Ok(())
}

fn apply_tool_filter(tools: &mut Vec<String>, enabled_tools: &[String], disabled_tools: &[String]) {
    if !enabled_tools.is_empty() {
        let allow: std::collections::HashSet<_> = enabled_tools.iter().cloned().collect();
        tools.retain(|name| allow.contains(name));
    }
    if !disabled_tools.is_empty() {
        let deny: std::collections::HashSet<_> = disabled_tools.iter().cloned().collect();
        tools.retain(|name| !deny.contains(name));
    }
}

fn key_value_pairs(values: Vec<String>, flag: &str) -> anyhow::Result<BTreeMap<String, String>> {
    let mut pairs = BTreeMap::new();
    for value in values {
        let Some((key, value)) = value.split_once('=') else {
            anyhow::bail!("{flag} values must use KEY=VALUE");
        };
        let key = key.trim();
        anyhow::ensure!(!key.is_empty(), "{flag} key must not be empty");
        pairs.insert(key.to_owned(), value.trim().to_owned());
    }
    Ok(pairs)
}
