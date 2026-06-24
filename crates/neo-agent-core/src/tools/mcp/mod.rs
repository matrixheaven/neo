use rmcp::model::{CallToolResult, ReadResourceResult, Resource, Tool as RmcpTool};
use serde::{Deserialize, Serialize};
use thiserror::Error;

mod client;
mod http;
mod legacy;
mod oauth;
mod stdio;

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
        let mut extra = Vec::new();
        for content in result.content {
            if let Some(text) = content.as_text() {
                texts.push(text.text.clone());
            } else {
                extra.push(
                    serde_json::to_value(&content)
                        .unwrap_or_else(|_| serde_json::json!({"type": "unknown"})),
                );
            }
        }
        let content = texts.join("\n");
        let mut response = if is_error {
            Self::error(content)
        } else {
            Self::ok(content)
        };
        let mut details = None;
        if !extra.is_empty() || result.structured_content.is_some() {
            let mut map = serde_json::Map::new();
            if !extra.is_empty() {
                map.insert("content".to_string(), serde_json::Value::Array(extra));
            }
            if let Some(structured_content) = result.structured_content {
                map.insert("structured_content".to_string(), structured_content);
            }
            details = Some(serde_json::Value::Object(map));
        }
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
            uri: resource.uri.clone(),
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
            rmcp::model::ResourceContents::TextResourceContents {
                uri,
                mime_type,
                text,
                ..
            } => Self {
                uri,
                mime_type,
                text: Some(text),
                blob: None,
            },
            rmcp::model::ResourceContents::BlobResourceContents {
                uri,
                mime_type,
                blob,
                ..
            } => Self {
                uri,
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
            arguments: serde_json::Value::Object(param.arguments.unwrap_or_default()),
        }
    }
}

impl From<rmcp::model::ResourceUpdatedNotificationParam> for McpResourceUpdate {
    fn from(param: rmcp::model::ResourceUpdatedNotificationParam) -> Self {
        Self {
            uri: param.uri.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rmcp::model::{AnnotateAble, CallToolResult, Content, Resource, Tool};

    use super::*;

    fn sample_schema() -> Arc<serde_json::Map<String, serde_json::Value>> {
        let schema: serde_json::Value =
            serde_json::from_str(r#"{"type":"object","properties":{"x":{"type":"string"}}}"#)
                .unwrap();
        Arc::new(schema.as_object().unwrap().clone())
    }

    #[test]
    fn converts_rmcp_tool_to_definition() {
        let tool = Tool::new("echo", "echoes input", sample_schema());
        let def = McpToolDefinition::from(tool);
        assert_eq!(def.name, "echo");
        assert_eq!(def.description, "echoes input");
        assert!(def.input_schema.get("properties").is_some());
    }

    #[test]
    fn converts_rmcp_call_tool_result_to_response() {
        let result = CallToolResult::success(vec![Content::text("hello")]);
        let response = McpToolResponse::from(result);
        assert!(!response.is_error);
        assert_eq!(response.content, "hello");
    }

    #[test]
    fn converts_rmcp_resource_to_definition() {
        let resource: Resource = rmcp::model::RawResource::new("file:///tmp/foo", "foo")
            .with_description("a file")
            .with_mime_type("text/plain")
            .no_annotation();
        let def = McpResourceDefinition::from(resource);
        assert_eq!(def.uri, "file:///tmp/foo");
        assert_eq!(def.name, "foo");
        assert_eq!(def.description, Some("a file".to_string()));
        assert_eq!(def.mime_type, Some("text/plain".to_string()));
    }
}
