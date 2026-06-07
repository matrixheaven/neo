use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use neo_ai::ToolSpec;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::Mutex,
};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpResourceDefinition {
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "mimeType", default)]
    pub mime_type: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpResourceRead {
    pub contents: Vec<McpResourceContent>,
}

#[async_trait]
pub trait McpToolAdapter: Send + Sync {
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpError>;

    async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResponse, McpError>;

    async fn list_resources(&self) -> Result<Vec<McpResourceDefinition>, McpError> {
        Err(McpError::protocol(
            "MCP adapter does not support resources/list",
        ))
    }

    async fn read_resource(&self, _uri: &str) -> Result<McpResourceRead, McpError> {
        Err(McpError::protocol(
            "MCP adapter does not support resources/read",
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpStdioConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpHttpConfig {
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Clone)]
pub struct McpHttpToolAdapter {
    config: McpHttpConfig,
    client: reqwest::Client,
    initialized: Arc<Mutex<bool>>,
    next_id: Arc<Mutex<u64>>,
}

impl McpHttpToolAdapter {
    #[must_use]
    pub fn new(config: McpHttpConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
            initialized: Arc::new(Mutex::new(false)),
            next_id: Arc::new(Mutex::new(1)),
        }
    }

    async fn ensure_initialized(&self) -> Result<(), McpError> {
        let mut initialized = self.initialized.lock().await;
        if *initialized {
            return Ok(());
        }
        let _ = self
            .request_raw(
                "initialize",
                Some(json_obj([
                    (
                        "protocolVersion",
                        serde_json::Value::String("2024-11-05".to_owned()),
                    ),
                    (
                        "clientInfo",
                        json_obj([
                            ("name", serde_json::Value::String("neo".to_owned())),
                            (
                                "version",
                                serde_json::Value::String(env!("CARGO_PKG_VERSION").to_owned()),
                            ),
                        ]),
                    ),
                    ("capabilities", json_obj([])),
                ])),
            )
            .await?;
        *initialized = true;
        Ok(())
    }

    async fn request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, McpError> {
        self.ensure_initialized().await?;
        self.request_raw(method, params).await
    }

    async fn request_raw(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, McpError> {
        let id = {
            let mut next_id = self.next_id.lock().await;
            let id = *next_id;
            *next_id = next_id.saturating_add(1);
            id
        };
        let mut request_body = serde_json::Map::from_iter([
            (
                "jsonrpc".to_owned(),
                serde_json::Value::String("2.0".to_owned()),
            ),
            ("id".to_owned(), serde_json::Value::from(id)),
            (
                "method".to_owned(),
                serde_json::Value::String(method.to_owned()),
            ),
        ]);
        if let Some(params) = params {
            request_body.insert("params".to_owned(), params);
        }
        let mut request = self
            .client
            .post(&self.config.url)
            .header("accept", "application/json, text/event-stream")
            .json(&serde_json::Value::Object(request_body));
        for (key, value) in &self.config.headers {
            request = request.header(key, value);
        }
        let response = request
            .send()
            .await
            .map_err(|err| McpError::protocol(format!("failed to send MCP HTTP request: {err}")))?;
        let status = response.status();
        let is_sse = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|content_type| content_type.contains("text/event-stream"));
        let body = response.text().await.map_err(|err| {
            McpError::protocol(format!("failed to read MCP HTTP response: {err}"))
        })?;
        if !status.is_success() {
            return Err(McpError::protocol(format!(
                "MCP HTTP server returned {status}: {body}"
            )));
        }
        let response_value = if is_sse {
            parse_sse_json_rpc_response(&body)?
        } else {
            serde_json::from_str(&body).map_err(|err| McpError::protocol(err.to_string()))?
        };
        if response_value.get("id").and_then(serde_json::Value::as_u64) != Some(id) {
            return Err(McpError::protocol(
                "MCP HTTP response id did not match request",
            ));
        }
        if let Some(error) = response_value.get("error") {
            let message = error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("MCP HTTP server returned JSON-RPC error");
            return Err(McpError::protocol(message));
        }
        response_value
            .get("result")
            .cloned()
            .ok_or_else(|| McpError::protocol("MCP HTTP response missing result"))
    }
}

#[async_trait]
impl McpToolAdapter for McpHttpToolAdapter {
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpError> {
        let result = self.request("tools/list", None).await?;
        let response: ListToolsResponse =
            serde_json::from_value(result).map_err(|err| McpError::protocol(err.to_string()))?;
        Ok(response
            .tools
            .into_iter()
            .map(|tool| {
                McpToolDefinition::new(
                    tool.name,
                    tool.description.unwrap_or_default(),
                    tool.input_schema,
                )
            })
            .collect())
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResponse, McpError> {
        let result = self
            .request(
                "tools/call",
                Some(json_obj([
                    ("name", serde_json::Value::String(name.to_owned())),
                    ("arguments", arguments),
                ])),
            )
            .await?;
        let is_error = result
            .get("isError")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        Ok(McpToolResponse {
            content: extract_text_content(&result),
            is_error,
            details: Some(result),
        })
    }

    async fn list_resources(&self) -> Result<Vec<McpResourceDefinition>, McpError> {
        let result = self.request("resources/list", None).await?;
        let response: ListResourcesResponse =
            serde_json::from_value(result).map_err(|err| McpError::protocol(err.to_string()))?;
        Ok(response.resources)
    }

    async fn read_resource(&self, uri: &str) -> Result<McpResourceRead, McpError> {
        let result = self
            .request(
                "resources/read",
                Some(json_obj([(
                    "uri",
                    serde_json::Value::String(uri.to_owned()),
                )])),
            )
            .await?;
        serde_json::from_value(result).map_err(|err| McpError::protocol(err.to_string()))
    }
}

#[derive(Clone)]
pub struct McpStdioToolAdapter {
    config: McpStdioConfig,
    session: Arc<Mutex<Option<StdioJsonRpcSession>>>,
}

impl McpStdioToolAdapter {
    #[must_use]
    pub fn new(config: McpStdioConfig) -> Self {
        Self {
            config,
            session: Arc::new(Mutex::new(None)),
        }
    }

    async fn request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, McpError> {
        let mut session = self.session.lock().await;
        if session.is_none() {
            *session = Some(StdioJsonRpcSession::connect(&self.config).await?);
        }
        let active = session
            .as_mut()
            .ok_or_else(|| McpError::protocol("MCP stdio session was not initialized"))?;
        let result = active.request(method, params).await;
        if result.is_err() {
            *session = None;
        }
        result
    }
}

#[async_trait]
impl McpToolAdapter for McpStdioToolAdapter {
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpError> {
        let result = self.request("tools/list", None).await?;
        let response: ListToolsResponse =
            serde_json::from_value(result).map_err(|err| McpError::protocol(err.to_string()))?;
        Ok(response
            .tools
            .into_iter()
            .map(|tool| {
                McpToolDefinition::new(
                    tool.name,
                    tool.description.unwrap_or_default(),
                    tool.input_schema,
                )
            })
            .collect())
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResponse, McpError> {
        let result = self
            .request(
                "tools/call",
                Some(json_obj([
                    ("name", serde_json::Value::String(name.to_owned())),
                    ("arguments", arguments),
                ])),
            )
            .await?;
        let is_error = result
            .get("isError")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        Ok(McpToolResponse {
            content: extract_text_content(&result),
            is_error,
            details: Some(result),
        })
    }

    async fn list_resources(&self) -> Result<Vec<McpResourceDefinition>, McpError> {
        let result = self.request("resources/list", None).await?;
        let response: ListResourcesResponse =
            serde_json::from_value(result).map_err(|err| McpError::protocol(err.to_string()))?;
        Ok(response.resources)
    }

    async fn read_resource(&self, uri: &str) -> Result<McpResourceRead, McpError> {
        let result = self
            .request(
                "resources/read",
                Some(json_obj([(
                    "uri",
                    serde_json::Value::String(uri.to_owned()),
                )])),
            )
            .await?;
        serde_json::from_value(result).map_err(|err| McpError::protocol(err.to_string()))
    }
}

struct StdioJsonRpcSession {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
    next_id: u64,
}

impl StdioJsonRpcSession {
    async fn connect(config: &McpStdioConfig) -> Result<Self, McpError> {
        let mut session = Self::spawn(config)?;
        session.initialize().await?;
        Ok(session)
    }

    fn spawn(config: &McpStdioConfig) -> Result<Self, McpError> {
        let mut command = Command::new(&config.command);
        command.args(&config.args);
        command.envs(&config.env);
        command.stdin(std::process::Stdio::piped());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|err| McpError::protocol(format!("failed to start MCP server: {err}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::protocol("failed to open MCP server stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::protocol("failed to open MCP server stdout"))?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    async fn initialize(&mut self) -> Result<(), McpError> {
        let _ = self
            .request(
                "initialize",
                Some(json_obj([
                    (
                        "protocolVersion",
                        serde_json::Value::String("2024-11-05".to_owned()),
                    ),
                    (
                        "clientInfo",
                        json_obj([
                            ("name", serde_json::Value::String("neo".to_owned())),
                            (
                                "version",
                                serde_json::Value::String(env!("CARGO_PKG_VERSION").to_owned()),
                            ),
                        ]),
                    ),
                    ("capabilities", json_obj([])),
                ])),
            )
            .await?;
        self.notify("notifications/initialized").await
    }

    async fn request(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, McpError> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut request = serde_json::Map::from_iter([
            (
                "jsonrpc".to_owned(),
                serde_json::Value::String("2.0".to_owned()),
            ),
            ("id".to_owned(), serde_json::Value::from(id)),
            (
                "method".to_owned(),
                serde_json::Value::String(method.to_owned()),
            ),
        ]);
        if let Some(params) = params {
            request.insert("params".to_owned(), params);
        }
        self.write_message(serde_json::Value::Object(request))
            .await?;
        loop {
            let response = self.read_message().await?;
            if response.get("id").and_then(serde_json::Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = response.get("error") {
                let message = error
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("MCP server returned JSON-RPC error");
                return Err(McpError::protocol(message));
            }
            return response
                .get("result")
                .cloned()
                .ok_or_else(|| McpError::protocol("MCP response missing result"));
        }
    }

    async fn notify(&mut self, method: &str) -> Result<(), McpError> {
        self.write_message(json_obj([
            ("jsonrpc", serde_json::Value::String("2.0".to_owned())),
            ("method", serde_json::Value::String(method.to_owned())),
        ]))
        .await
    }

    async fn write_message(&mut self, message: serde_json::Value) -> Result<(), McpError> {
        let mut line =
            serde_json::to_vec(&message).map_err(|err| McpError::protocol(err.to_string()))?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .await
            .map_err(|err| McpError::protocol(format!("failed to write MCP request: {err}")))?;
        self.stdin
            .flush()
            .await
            .map_err(|err| McpError::protocol(format!("failed to flush MCP request: {err}")))
    }

    async fn read_message(&mut self) -> Result<serde_json::Value, McpError> {
        let mut line = String::new();
        let bytes = self
            .stdout
            .read_line(&mut line)
            .await
            .map_err(|err| McpError::protocol(format!("failed to read MCP response: {err}")))?;
        if bytes == 0 {
            return Err(McpError::protocol("MCP server closed stdout"));
        }
        serde_json::from_str(&line).map_err(|err| McpError::protocol(err.to_string()))
    }
}

impl Drop for StdioJsonRpcSession {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
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

    pub async fn discover_dyn(
        server_id: impl Into<String>,
        adapter: Arc<dyn McpToolAdapter>,
    ) -> Result<Self, McpError> {
        let tools = adapter.list_tools().await?;
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

#[derive(Debug, Deserialize)]
struct ListToolsResponse {
    tools: Vec<RemoteMcpToolDefinition>,
}

#[derive(Debug, Deserialize)]
struct RemoteMcpToolDefinition {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(rename = "inputSchema", default = "empty_object_schema")]
    input_schema: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ListResourcesResponse {
    resources: Vec<McpResourceDefinition>,
}

fn empty_object_schema() -> serde_json::Value {
    json_obj([("type", serde_json::Value::String("object".to_owned()))])
}

fn json_obj<const N: usize>(entries: [(&str, serde_json::Value); N]) -> serde_json::Value {
    serde_json::Value::Object(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

fn extract_text_content(result: &serde_json::Value) -> String {
    result
        .get("content")
        .and_then(serde_json::Value::as_array)
        .map(|content| {
            content
                .iter()
                .filter_map(|item| item.get("text").and_then(serde_json::Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|content| !content.is_empty())
        .unwrap_or_else(|| result.to_string())
}

fn parse_sse_json_rpc_response(body: &str) -> Result<serde_json::Value, McpError> {
    for event in body.split("\n\n") {
        let data = event
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim)
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        return serde_json::from_str(&data).map_err(|err| McpError::protocol(err.to_string()));
    }
    Err(McpError::protocol("MCP SSE response missing data"))
}
