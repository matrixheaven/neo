//! Stdio MCP client builder (Task 2.2).

use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use rmcp::{ServiceExt, transport::TokioChildProcess};
use tokio::process::Command;

use super::{McpError, client::McpClient};
use crate::tools::ProcessSupervisor;

// TODO: `StdioConfig` is currently unused while the rmcp migration is in
// progress. It will be wired up through `McpConnectionManager` in Task 4.
#[derive(Debug, Clone)]
pub struct StdioConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    /// Kept for API compatibility; the actual startup timeout is owned by
    /// `connect_one` in `mcp_manager.rs` so that serve + list_tools share a
    /// single deadline.
    pub startup_timeout_ms: Option<u64>,
    pub tool_timeout_ms: Option<u64>,
}

// TODO: `build_stdio_client` is currently unused while the rmcp migration is in
// progress. It will be wired up through `McpConnectionManager` in Task 4.
pub async fn build_stdio_client(
    server_id: &str,
    config: StdioConfig,
    supervisor: &ProcessSupervisor,
) -> Result<Arc<dyn McpClient>, McpError> {
    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args);
    for (k, v) in &config.env {
        cmd.env(k, v);
    }
    if let Some(cwd) = &config.cwd {
        cmd.current_dir(cwd);
    }

    let transport = TokioChildProcess::new(cmd)
        .map_err(|e| McpError::protocol(format!("failed to spawn stdio MCP server: {e}")))?;

    let request_timeout = config.tool_timeout_ms.map(Duration::from_millis);

    // Note: no internal startup timeout here. The caller (`connect_one` in
    // `mcp_manager.rs`) wraps the entire serve + discover_tools sequence in a
    // single timeout so the deadline is not double-counted.
    let service = ().serve(transport).await.map_err(|e| McpError::protocol(e.to_string()))?;

    let client: Arc<dyn McpClient> =
        Arc::new(super::client::RmcpClient::new(service, request_timeout));

    let handle = format!("mcp_stdio_{server_id}");
    let client_for_cleanup = Arc::clone(&client);
    supervisor
        .register(
            handle,
            crate::tools::ProcessKind::McpStdio,
            move |_handle| {
                let client = Arc::clone(&client_for_cleanup);
                Box::pin(async move {
                    let _ = client.shutdown().await;
                })
            },
        )
        .await;

    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdio_config_holds_values() {
        let config = StdioConfig {
            command: "npx".into(),
            args: vec!["-y".into(), "server".into()],
            env: [("K".into(), "V".into())].into_iter().collect(),
            cwd: Some(PathBuf::from("/tmp")),
            startup_timeout_ms: Some(5000),
            tool_timeout_ms: Some(30000),
        };
        assert_eq!(config.command, "npx");
        assert_eq!(config.args.len(), 2);
    }
}
