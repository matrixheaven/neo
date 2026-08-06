use super::super::Tool;
use super::super::ToolContext;
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

fn http_server(id: &str) -> ManagedMcpServerConfig {
    ManagedMcpServerConfig {
        id: id.to_owned(),
        enabled: true,
        transport: ManagedMcpTransport::Http {
            url: "https://mcp.example.com/mcp#ignored".to_owned(),
            headers: BTreeMap::new(),
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

async fn insert_entry(manager: &McpConnectionManager, entry: ManagedMcpEntry) {
    manager
        .inner
        .write()
        .await
        .entries
        .insert(entry.config.id.clone(), entry);
}

fn registry_tool_names(registry: &ToolRegistry) -> Vec<String> {
    registry.specs().into_iter().map(|spec| spec.name).collect()
}

#[tokio::test]
async fn needs_auth_entry_registers_authenticate_tool_only() {
    let manager = McpConnectionManager::new(ProcessSupervisor::default());
    let mut entry = entry_for_status(McpServerStatus::NeedsAuth);
    entry.config = http_server("linear");
    entry.error = Some(McpDiagnostic {
        server_id: "linear".to_owned(),
        transport: "http".to_owned(),
        message: "OAuth required".to_owned(),
        hint: Some("authorize".to_owned()),
        stderr_tail: None,
    });
    insert_entry(&manager, entry).await;

    let mut registry = ToolRegistry::new();
    let diagnostics = manager.register_connected_tools_into(&mut registry).await;

    assert_eq!(
        registry_tool_names(&registry),
        vec!["mcp__linear__authenticate"]
    );
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].server_id, "linear");
}

#[tokio::test]
async fn failed_entry_does_not_register_authenticate_tool() {
    let manager = McpConnectionManager::new(ProcessSupervisor::default());
    let mut entry = entry_for_status(McpServerStatus::Failed);
    entry.config = http_server("linear");
    entry.error = Some(McpDiagnostic {
        server_id: "linear".to_owned(),
        transport: "http".to_owned(),
        message: "connect failed".to_owned(),
        hint: None,
        stderr_tail: None,
    });
    insert_entry(&manager, entry).await;

    let mut registry = ToolRegistry::new();
    let diagnostics = manager.register_connected_tools_into(&mut registry).await;

    assert!(registry_tool_names(&registry).is_empty());
    assert_eq!(diagnostics.len(), 1);
}

#[tokio::test]
async fn connected_entry_registers_real_tools_not_authenticate_tool() {
    let manager = McpConnectionManager::new(ProcessSupervisor::default());
    let mut entry = entry_for_status(McpServerStatus::Connected);
    entry.config.id = "linear".to_owned();
    insert_entry(&manager, entry).await;

    let mut registry = ToolRegistry::new();
    let diagnostics = manager.register_connected_tools_into(&mut registry).await;

    assert!(diagnostics.is_empty());
    assert_eq!(registry_tool_names(&registry), vec!["mcp__linear__echo"]);
}

#[test]
fn authenticate_tool_schema_is_empty_object() {
    let tool = McpAuthenticateTool {
        server_id: "linear".to_owned(),
        exposed_name: "mcp__linear__authenticate".to_owned(),
        manager: McpConnectionManager::new(ProcessSupervisor::default()),
    };

    assert_eq!(
        tool.input_schema(),
        serde_json::json!({
            "type": "object",
            "additionalProperties": false
        })
    );
}

#[tokio::test]
async fn authenticate_tool_reports_clear_errors_for_unusable_servers() {
    let manager = McpConnectionManager::new(ProcessSupervisor::default());
    manager
        .apply_config(vec![disabled_server("disabled")])
        .await;
    let ctx = ToolContext::new(std::env::temp_dir()).unwrap();

    let disabled_tool = McpAuthenticateTool {
        server_id: "disabled".to_owned(),
        exposed_name: "mcp__disabled__authenticate".to_owned(),
        manager: manager.clone(),
    };
    let disabled_err = disabled_tool
        .execute(&ctx, serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(disabled_err.to_string().contains("disabled"));

    let stdio_config = ManagedMcpServerConfig {
        enabled: true,
        ..disabled_server("stdio")
    };
    manager
        .apply_config(vec![disabled_server("disabled"), stdio_config])
        .await;
    let stdio_tool = McpAuthenticateTool {
        server_id: "stdio".to_owned(),
        exposed_name: "mcp__stdio__authenticate".to_owned(),
        manager: manager.clone(),
    };
    let stdio_err = stdio_tool
        .execute(&ctx, serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(stdio_err.to_string().contains("HTTP/SSE"));

    let missing_tool = McpAuthenticateTool {
        server_id: "missing".to_owned(),
        exposed_name: "mcp__missing__authenticate".to_owned(),
        manager,
    };
    let missing_err = missing_tool
        .execute(&ctx, serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(missing_err.to_string().contains("not found"));
}

#[tokio::test]
async fn authenticate_tool_reports_unwired_oauth_flow_without_success_claim() {
    let manager = McpConnectionManager::new(ProcessSupervisor::default());
    let mut entry = entry_for_status(McpServerStatus::NeedsAuth);
    entry.config = http_server("linear");
    insert_entry(&manager, entry).await;
    let mut registry = ToolRegistry::new();
    manager.register_connected_tools_into(&mut registry).await;
    let ctx = ToolContext::new(std::env::temp_dir()).unwrap();

    let result = registry
        .run("mcp__linear__authenticate", &ctx, serde_json::json!({}))
        .await
        .unwrap();

    assert!(result.is_error);
    assert!(
        result
            .content
            .contains("Could not start OAuth authentication")
    );
    assert!(result.content.contains("callback completion is not wired"));
    assert!(!result.content.contains("authenticated"));
    assert!(!result.content.contains("reconnected"));
    assert_eq!(
        manager.snapshot("linear").await.unwrap().status,
        McpServerStatus::NeedsAuth
    );
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
