use std::sync::Arc;

use async_trait::async_trait;
use neo_ai::ToolSpec;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{Tool, ToolContext, ToolError, ToolFuture, ToolRegistry, ToolResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

impl McpToolDefinition {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolResponse {
    pub content: String,
    pub is_error: bool,
    pub details: Option<serde_json::Value>,
}

impl McpToolResponse {
    #[must_use]
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            details: None,
        }
    }

    #[must_use]
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            details: None,
        }
    }

    #[must_use]
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

impl From<McpToolResponse> for ToolResult {
    fn from(response: McpToolResponse) -> Self {
        let result = if response.is_error {
            ToolResult::error(response.content)
        } else {
            ToolResult::ok(response.content)
        };
        if let Some(details) = response.details {
            result.with_details(details)
        } else {
            result
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct McpError {
    message: String,
}

impl McpError {
    #[must_use]
    pub fn protocol(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

#[async_trait]
pub trait McpToolAdapter: Send + Sync {
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpError>;

    async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResponse, McpError>;
}

pub struct McpToolProvider {
    server_id: String,
    tools: Vec<McpToolDefinition>,
    adapter: Arc<dyn McpToolAdapter>,
}

impl McpToolProvider {
    pub async fn discover<A>(
        server_id: impl Into<String>,
        adapter: Arc<A>,
    ) -> Result<Self, McpError>
    where
        A: McpToolAdapter + 'static,
    {
        let tools = adapter.list_tools().await?;
        let adapter: Arc<dyn McpToolAdapter> = adapter;
        Ok(Self {
            server_id: server_id.into(),
            tools,
            adapter,
        })
    }

    #[must_use]
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools
            .iter()
            .map(|tool| ToolSpec {
                name: namespaced_tool_name(&self.server_id, &tool.name),
                description: tool.description.clone(),
                input_schema: tool.input_schema.clone(),
            })
            .collect()
    }

    pub fn register_into(self, registry: &mut ToolRegistry) {
        for tool in self.tools {
            registry.register(McpTool {
                server_id: self.server_id.clone(),
                exposed_name: namespaced_tool_name(&self.server_id, &tool.name),
                remote_name: tool.name,
                description: tool.description,
                input_schema: tool.input_schema,
                adapter: Arc::clone(&self.adapter),
            });
        }
    }
}

struct McpTool {
    server_id: String,
    exposed_name: String,
    remote_name: String,
    description: String,
    input_schema: serde_json::Value,
    adapter: Arc<dyn McpToolAdapter>,
}

impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.exposed_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> serde_json::Value {
        self.input_schema.clone()
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            self.adapter
                .call_tool(&self.remote_name, input)
                .await
                .map(ToolResult::from)
                .map_err(|err| ToolError::Mcp {
                    server_id: self.server_id.clone(),
                    tool_name: self.remote_name.clone(),
                    message: err.message().to_owned(),
                })
        })
    }
}

fn namespaced_tool_name(server_id: &str, tool_name: &str) -> String {
    format!(
        "mcp__{}__{}",
        sanitize_tool_name_segment(server_id),
        sanitize_tool_name_segment(tool_name)
    )
}

fn sanitize_tool_name_segment(value: &str) -> String {
    let mut sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        sanitized.push_str("unnamed");
    }
    sanitized
}
