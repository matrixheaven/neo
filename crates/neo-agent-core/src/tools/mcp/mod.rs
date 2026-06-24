use rmcp::model::{CallToolResult, ReadResourceResult, Resource, Tool as RmcpTool};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod client;
pub mod http;
pub mod legacy;
pub mod oauth;
pub mod stdio;

pub use legacy::{
    McpHttpConfig, McpHttpToolAdapter, McpStdioConfig, McpStdioToolAdapter, McpToolAdapter,
    McpToolProvider,
};

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

impl From<RmcpTool> for McpToolDefinition {
    fn from(tool: RmcpTool) -> Self {
        Self {
            name: tool.name.to_string(),
            description: tool.description.unwrap_or_default().to_string(),
            input_schema: serde_json::Value::Object((*tool.input_schema).clone()),
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

impl From<McpToolResponse> for super::ToolResult {
    fn from(response: McpToolResponse) -> Self {
        let result = if response.is_error {
            super::ToolResult::error(response.content)
        } else {
            super::ToolResult::ok(response.content)
        };
        if let Some(details) = response.details {
            result.with_details(details)
        } else {
            result
        }
    }
}

impl From<CallToolResult> for McpToolResponse {
    fn from(result: CallToolResult) -> Self {
        let is_error = result.is_error.unwrap_or(false);
        let mut texts = Vec::new();
        let mut details: Option<serde_json::Value> = None;
        for content in result.content {
            if let Some(text) = content.as_text() {
                texts.push(text.text.clone());
            } else if let Some(image) = content.as_image() {
                details = Some(serde_json::json!({
                    "type": "image",
                    "data": image.data,
                    "mime_type": image.mime_type,
                }));
            } else if let Some(resource) = content.as_resource() {
                details = Some(serde_json::json!({
                    "type": "resource",
                    "resource": resource,
                }));
            }
        }
        let content = texts.join("\n");
        let mut response = if is_error {
            Self::error(content)
        } else {
            Self::ok(content)
        };
        if let Some(details) = details {
            response = response.with_details(details);
        }
        response
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

impl From<rmcp::ErrorData> for McpError {
    fn from(err: rmcp::ErrorData) -> Self {
        Self::protocol(err.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpResourceDefinition {
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "mimeType", default)]
    pub mime_type: Option<String>,
}

impl From<Resource> for McpResourceDefinition {
    fn from(resource: Resource) -> Self {
        Self {
            uri: resource.uri.to_string(),
            name: resource.name.clone(),
            description: resource.description.clone(),
            mime_type: resource.mime_type.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpResourceContent {
    pub uri: String,
    #[serde(rename = "mimeType", default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub blob: Option<String>,
}

impl From<rmcp::model::ResourceContents> for McpResourceContent {
    fn from(contents: rmcp::model::ResourceContents) -> Self {
        match contents {
            rmcp::model::ResourceContents::TextResourceContents { uri, mime_type, text, .. } => Self {
                uri: uri.to_string(),
                mime_type,
                text: Some(text),
                blob: None,
            },
            rmcp::model::ResourceContents::BlobResourceContents { uri, mime_type, blob, .. } => Self {
                uri: uri.to_string(),
                mime_type,
                text: None,
                blob: Some(blob),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpResourceRead {
    pub contents: Vec<McpResourceContent>,
}

impl From<ReadResourceResult> for McpResourceRead {
    fn from(result: ReadResourceResult) -> Self {
        Self {
            contents: result.contents.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpResourceUpdate {
    pub uri: String,
}

impl From<rmcp::model::CallToolRequestParams> for McpToolCall {
    fn from(param: rmcp::model::CallToolRequestParams) -> Self {
        Self {
            name: param.name.to_string(),
            arguments: param.arguments.map_or(serde_json::Value::Object(Default::default()), |obj| serde_json::Value::Object(obj)),
        }
    }
}

impl From<rmcp::model::ResourceUpdatedNotificationParam> for McpResourceUpdate {
    fn from(param: rmcp::model::ResourceUpdatedNotificationParam) -> Self {
        Self {
            uri: param.uri.to_string(),
        }
    }
}
