use super::super::Tool;
use super::super::ToolContext;
use super::super::ToolError;
use super::*;
use crate::tools::mcp::McpToolResponse;

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

#[tokio::test]
async fn managed_mcp_tool_routes_call_through_client() {
    let client: Arc<dyn McpClient> = Arc::new(MockMcpClient {
        tool_name: "echo".to_owned(),
        echo_text: "mock-echo".to_owned(),
    });

    let tool = ManagedMcpTool {
        server_id: "test-server".to_owned(),
        exposed_name: "mcp__test-server__echo".to_owned(),
        remote_name: "echo".to_owned(),
        description: "an echo tool".to_owned(),
        input_schema: serde_json::json!({"type": "object"}),
        client: Arc::clone(&client),
    };

    // Verify the tool metadata.
    assert_eq!(tool.name(), "mcp__test-server__echo");
    assert_eq!(tool.description(), "an echo tool");

    // Execute the tool and verify the mock client's response flows through.
    let ctx = ToolContext::new(std::env::temp_dir()).unwrap();
    let input = serde_json::json!({"msg": "hello"});
    let result = tool.execute(&ctx, input.clone()).await.unwrap();

    assert!(!result.is_error);
    assert_eq!(result.content, format!("mock-echo:echo:{input}"));
}

#[tokio::test]
async fn managed_mcp_tool_propagates_client_error() {
    /// A mock client that always fails `call_tool`.
    struct FailingClient;
    #[async_trait::async_trait]
    impl McpClient for FailingClient {
        async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpError> {
            Ok(Vec::new())
        }
        async fn call_tool(
            &self,
            _name: &str,
            _arguments: serde_json::Value,
        ) -> Result<McpToolResponse, McpError> {
            Err(McpError::protocol("boom"))
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

    let client: Arc<dyn McpClient> = Arc::new(FailingClient);
    let tool = ManagedMcpTool {
        server_id: "broken".to_owned(),
        exposed_name: "mcp__broken__do".to_owned(),
        remote_name: "do".to_owned(),
        description: "failing tool".to_owned(),
        input_schema: serde_json::json!({"type": "object"}),
        client,
    };

    let ctx = ToolContext::new(std::env::temp_dir()).unwrap();
    let err = tool.execute(&ctx, serde_json::json!({})).await.unwrap_err();
    match err {
        ToolError::Mcp {
            server_id,
            tool_name,
            message,
        } => {
            assert_eq!(server_id, "broken");
            assert_eq!(tool_name, "do");
            assert_eq!(message, "boom");
        }
        other => panic!("expected Mcp error, got {other:?}"),
    }
}
