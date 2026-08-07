//! Run-mode MCP context behavior (split from `context.rs`).

use super::*;
use std::sync::Arc;

use super::super::mcp_cli::auth_mcp_server;
use super::super::runtime::tool_registry_for_config;
use crate::config::McpTransport;
use neo_agent_core::{McpConnectionManager, ProcessSupervisor};

#[tokio::test]
async fn tool_registry_ignores_failed_mcp_server_startup() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path());
    let mut server = test_mcp_server("bad", McpTransport::Stdio, None);
    server.command = Some("neo-missing-mcp-binary-for-test".to_owned());
    server.startup_timeout_ms = Some(50);
    config.mcp.servers.push(server);

    let registry =
        tool_registry_for_config(&config, Arc::new(std::sync::Mutex::new(Vec::new())), None)
            .await
            .expect("bad MCP server should not abort registry construction");

    assert!(
        registry
            .specs()
            .iter()
            .all(|spec| !spec.name.starts_with("mcp__bad__")),
        "failed MCP tools must not be exposed"
    );
}

#[tokio::test]
async fn shared_mcp_manager_does_not_relog_startup_failure_during_tool_registration() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path());
    let mut server = test_mcp_server("bad", McpTransport::Stdio, None);
    server.command = Some("neo-missing-mcp-binary-for-test".to_owned());
    server.startup_timeout_ms = Some(50);
    config.mcp.servers.push(server);
    let manager = McpConnectionManager::new(ProcessSupervisor::default());
    let (layer, mut event_rx) = crate::log_capture::capture_channel(8);
    let _guard = tracing_subscriber::registry().with(layer).set_default();

    tool_registry_for_config(
        &config,
        Arc::new(std::sync::Mutex::new(Vec::new())),
        Some(&manager),
    )
    .await
    .expect("bad MCP server should not abort registry construction");

    let events = std::iter::from_fn(|| event_rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.is_empty(),
        "startup failure was already surfaced by the shared MCP manager: {events:?}"
    );
}

#[tokio::test]
async fn auth_mcp_server_errors_for_missing_server() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path());
    let result = auth_mcp_server("missing".to_owned(), &config).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}

#[tokio::test]
async fn auth_mcp_server_errors_for_non_remote_transport() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path());
    config
        .mcp
        .servers
        .push(test_mcp_server("fs", McpTransport::Stdio, None));
    let result = auth_mcp_server("fs".to_owned(), &config).await;
    assert!(result.is_err());
    let message = result.unwrap_err().to_string();
    assert!(message.contains("HTTP/SSE"), "unexpected error: {message}");
}
