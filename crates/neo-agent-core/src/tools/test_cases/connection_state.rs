use super::*;
use crate::tools::mcp::McpToolResponse;

fn disabled_server(id: &str) -> ManagedMcpServerConfig {
    ManagedMcpServerConfig {
        id: id.to_owned(),
        enabled: false,
        transport: ManagedMcpTransport::Stdio {
            command: "noop".to_owned(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
        },
        enabled_tools: Vec::new(),
        disabled_tools: Vec::new(),
        startup_timeout_ms: None,
        tool_timeout_ms: None,
        reconnect: McpReconnectPolicy::default(),
    }
}

fn entry_for_status(status: McpServerStatus) -> ManagedMcpEntry {
    ManagedMcpEntry {
        config: disabled_server("auth-server"),
        attempt_id: 1,
        status,
        client: Some(Arc::new(MockMcpClient {
            tool_name: "echo".to_owned(),
            echo_text: "mock".to_owned(),
        })),
        oauth_identity: McpOAuthIdentity::new(
            "auth-server",
            "https://mcp.example.com/mcp",
            McpOAuthTransportKind::Http,
        )
        .ok(),
        tools: vec![McpToolDefinition::new(
            "echo",
            "mock tool",
            serde_json::json!({"type": "object"}),
        )],
        resources: vec![McpResourceDefinition {
            uri: "file:///tmp/mock".to_owned(),
            name: "mock".to_owned(),
            description: None,
            mime_type: None,
        }],
        error: None,
        reconnect_attempt: 0,
        next_retry_ms: Some(250),
        reconnect_task: None,
        connect_task: None,
    }
}

#[test]
fn needs_auth_status_has_stable_string() {
    assert_eq!(McpServerStatus::NeedsAuth.as_str(), "needs_auth");
}

#[test]
fn set_needs_auth_clears_runtime_state_without_retry() {
    let mut entry = entry_for_status(McpServerStatus::Connected);
    let diagnostic = McpDiagnostic {
        server_id: "auth-server".to_owned(),
        transport: "http".to_owned(),
        message: "OAuth required".to_owned(),
        hint: Some("login".to_owned()),
        stderr_tail: None,
    };

    set_needs_auth(&mut entry, diagnostic.clone());

    assert_eq!(entry.status, McpServerStatus::NeedsAuth);
    assert_eq!(entry.error, Some(diagnostic));
    assert!(entry.client.is_none());
    assert!(entry.oauth_identity.is_none());
    assert!(entry.tools.is_empty());
    assert!(entry.resources.is_empty());
    assert_eq!(entry.next_retry_ms, None);
}

#[test]
fn set_failed_schedules_reconnect_for_non_auth_failure() {
    let mut entry = entry_for_status(McpServerStatus::Connected);
    entry.config.reconnect = McpReconnectPolicy {
        enabled: true,
        initial_delay_ms: 100,
        max_delay_ms: 1_000,
        max_attempts: Some(3),
    };
    let diagnostic = McpDiagnostic {
        server_id: "auth-server".to_owned(),
        transport: "stdio".to_owned(),
        message: "boom".to_owned(),
        hint: None,
        stderr_tail: None,
    };

    assert!(set_failed(&mut entry, diagnostic));

    assert_eq!(entry.status, McpServerStatus::Reconnecting);
    assert_eq!(entry.next_retry_ms, Some(100));
    assert_eq!(entry.reconnect_attempt, 1);
}

#[test]
fn needs_auth_connect_error_settles_without_reconnect() {
    let mut entry = entry_for_status(McpServerStatus::Pending);
    entry.config.reconnect = McpReconnectPolicy {
        enabled: true,
        initial_delay_ms: 100,
        max_delay_ms: 1_000,
        max_attempts: Some(3),
    };
    let err = McpError::needs_auth("OAuth required: missing token");

    assert!(!apply_connect_error(&mut entry, &err));

    assert_eq!(entry.status, McpServerStatus::NeedsAuth);
    assert_eq!(entry.next_retry_ms, None);
    assert_eq!(entry.reconnect_attempt, 0);
}

#[test]
fn reconnect_delay_is_capped() {
    let policy = McpReconnectPolicy {
        enabled: true,
        initial_delay_ms: 500,
        max_delay_ms: 10_000,
        max_attempts: None,
    };
    assert_eq!(reconnect_delay_ms(policy, 1), 500);
    assert_eq!(reconnect_delay_ms(policy, 2), 1_000);
    assert_eq!(reconnect_delay_ms(policy, 20), 10_000);
}

/// A minimal mock MCP client used to verify that `ManagedMcpTool` correctly
/// routes `execute()` through `McpClient::call_tool` and converts the
/// response into a `ToolResult`. This exercises the trait-to-tool integration
/// without requiring a live MCP server.
struct MockMcpClient {
    tool_name: String,
    echo_text: String,
}

#[async_trait::async_trait]
impl McpClient for MockMcpClient {
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpError> {
        Ok(vec![McpToolDefinition::new(
            &self.tool_name,
            "mock tool",
            serde_json::json!({"type": "object"}),
        )])
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResponse, McpError> {
        assert_eq!(name, self.tool_name);
        Ok(McpToolResponse::ok(format!(
            "{}:{}:{}",
            self.echo_text, name, arguments
        )))
    }

    async fn list_resources(&self) -> Result<Vec<McpResourceDefinition>, McpError> {
        Ok(Vec::new())
    }

    async fn read_resource(&self, _uri: &str) -> Result<McpResourceRead, McpError> {
        Ok(McpResourceRead {
            contents: Vec::new(),
        })
    }

    async fn shutdown(&self) -> Result<(), McpError> {
        Ok(())
    }
}
