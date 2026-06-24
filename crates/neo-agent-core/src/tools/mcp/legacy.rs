// TODO(mcp-rmcp): remove this temporary shim once stdio/http adapters are migrated (Tasks 2.x/4.4).
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use async_trait::async_trait;
use chrono::Utc;
use futures::StreamExt;
use neo_ai::ToolSpec;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{Mutex, RwLock, mpsc, oneshot},
    task::JoinHandle,
};

use super::super::{
    ProcessKind, ProcessSupervisor, Tool, ToolContext, ToolError, ToolFuture, ToolRegistry,
    ToolResult,
};
use super::{
    McpError, McpResourceDefinition, McpResourceRead, McpResourceUpdate, McpToolDefinition,
    McpToolResponse,
};
use crate::oauth::{OAuthProvider, OAuthProviderRegistry, OAuthStore, refresh_access_token};

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

    async fn subscribe_resource(&self, _uri: &str) -> Result<(), McpError> {
        Err(McpError::protocol(
            "MCP adapter does not support resources/subscribe",
        ))
    }

    async fn unsubscribe_resource(&self, _uri: &str) -> Result<(), McpError> {
        Err(McpError::protocol(
            "MCP adapter does not support resources/unsubscribe",
        ))
    }

    async fn next_resource_update(&self) -> Result<McpResourceUpdate, McpError> {
        Err(McpError::protocol(
            "MCP adapter does not support resource update notifications",
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpHttpConfig {
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_timeout_ms: Option<u64>,
    #[serde(skip)]
    pub server_id: Option<String>,
    #[serde(skip)]
    pub oauth_store: Option<Arc<RwLock<OAuthStore>>>,
    #[serde(skip)]
    pub oauth_provider: Option<OAuthProvider>,
    #[serde(skip)]
    pub oauth_provider_registry: Option<Arc<OAuthProviderRegistry>>,
    #[serde(skip)]
    pub oauth_store_path: Option<PathBuf>,
}

#[derive(Clone)]
pub struct McpHttpToolAdapter {
    config: McpHttpConfig,
    client: reqwest::Client,
    initialized: Arc<Mutex<bool>>,
    next_id: Arc<Mutex<u64>>,
    resource_update_tx: mpsc::UnboundedSender<Result<McpResourceUpdate, McpError>>,
    resource_update_rx: Arc<Mutex<mpsc::UnboundedReceiver<Result<McpResourceUpdate, McpError>>>>,
    resource_sse_reader: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl McpHttpToolAdapter {
    #[must_use]
    pub fn new(config: McpHttpConfig) -> Self {
        let (resource_update_tx, resource_update_rx) = mpsc::unbounded_channel();
        Self {
            config,
            client: reqwest::Client::new(),
            initialized: Arc::new(Mutex::new(false)),
            next_id: Arc::new(Mutex::new(1)),
            resource_update_tx,
            resource_update_rx: Arc::new(Mutex::new(resource_update_rx)),
            resource_sse_reader: Arc::new(Mutex::new(None)),
        }
    }

    async fn apply_oauth_header(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, McpError> {
        if self
            .config
            .headers
            .keys()
            .any(|key| key.eq_ignore_ascii_case("authorization"))
        {
            return Ok(request);
        }

        let Some(server_id) = self.config.server_id.as_ref() else {
            return Ok(request);
        };
        let provider = self
            .config
            .oauth_provider
            .clone()
            .or_else(|| {
                self.config
                    .oauth_provider_registry
                    .as_ref()
                    .and_then(|registry| registry.resolve_for_url(&self.config.url))
                    .cloned()
            })
            .or_else(|| {
                OAuthProviderRegistry::with_builtin_providers()
                    .resolve_for_url(&self.config.url)
                    .cloned()
            });
        let Some(provider) = provider else {
            return Ok(request);
        };
        let Some(store) = self.config.oauth_store.as_ref() else {
            return Ok(request);
        };

        let token_key = format!("mcp:{server_id}");
        let mut store_guard = store.write().await;
        let token = store_guard.get_token(&token_key);

        let Some(token) = token else {
            return Ok(request);
        };

        let token = if token
            .expires_at
            .is_some_and(|expires_at| expires_at < Utc::now())
        {
            let Some(refresh_token) = token.refresh_token.as_ref() else {
                return Ok(
                    request.header("Authorization", format!("Bearer {}", token.access_token))
                );
            };
            match refresh_access_token(&provider, refresh_token).await {
                Ok(new_token) => {
                    store_guard.set_token(&token_key, &new_token);
                    if let Some(path) = self.config.oauth_store_path.as_ref() {
                        let _ = store_guard.save(path);
                    }
                    new_token
                }
                Err(err) => {
                    return Err(McpError::protocol(format!(
                        "failed to refresh OAuth token: {err}"
                    )));
                }
            }
        } else {
            token
        };

        Ok(request.header("Authorization", format!("Bearer {}", token.access_token)))
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
        let (id, response) = self.send_raw_request(method, params).await?;
        let status = response.status();
        let is_sse = http_response_is_sse(&response);
        let body = read_http_response_body(response).await?;
        ensure_http_success(status, &body)?;
        let response_value = parse_http_json_rpc_body(&body, is_sse)?;
        validate_json_rpc_result(&response_value, id)
    }

    async fn send_raw_request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(u64, reqwest::Response), McpError> {
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
        request = self.apply_oauth_header(request).await?;
        let response = request
            .send()
            .await
            .map_err(|err| McpError::protocol(format!("failed to send MCP HTTP request: {err}")))?;
        Ok((id, response))
    }

    async fn stop_resource_sse_reader(&self) {
        if let Some(handle) = self.resource_sse_reader.lock().await.take() {
            handle.abort();
        }
    }

    async fn clear_pending_resource_updates(&self) {
        let mut update_rx = self.resource_update_rx.lock().await;
        while update_rx.try_recv().is_ok() {}
    }

    async fn start_resource_event_reader(&self, event_stream_url: &str) -> Result<(), McpError> {
        let mut request = self
            .client
            .get(event_stream_url)
            .header("accept", "text/event-stream");
        for (key, value) in &self.config.headers {
            request = request.header(key, value);
        }
        request = self.apply_oauth_header(request).await?;
        let response = request.send().await.map_err(|err| {
            McpError::protocol(format!("failed to open MCP HTTP event stream: {err}"))
        })?;
        let status = response.status();
        let is_sse = http_response_is_sse(&response);
        if !status.is_success() {
            let body = response.text().await.map_err(|err| {
                McpError::protocol(format!("failed to read MCP HTTP event response: {err}"))
            })?;
            return Err(McpError::protocol(format!(
                "MCP HTTP event stream returned {status}: {body}"
            )));
        }
        if !is_sse {
            return Err(McpError::protocol(
                "MCP HTTP event stream did not return text/event-stream",
            ));
        }
        let update_tx = self.resource_update_tx.clone();
        let reader = tokio::spawn(read_http_sse_messages(response, None, None, update_tx));
        *self.resource_sse_reader.lock().await = Some(reader);
        Ok(())
    }

    async fn handle_subscribe_response(
        &self,
        id: u64,
        response: reqwest::Response,
    ) -> Result<(), McpError> {
        let status = response.status();
        let is_sse = http_response_is_sse(&response);
        if !status.is_success() {
            let body = read_http_response_body(response).await?;
            return Err(McpError::protocol(format!(
                "MCP HTTP server returned {status}: {body}"
            )));
        }
        if is_sse {
            return self.handle_sse_subscribe_response(id, response).await;
        }
        self.handle_json_subscribe_response(id, response).await
    }

    async fn handle_json_subscribe_response(
        &self,
        id: u64,
        response: reqwest::Response,
    ) -> Result<(), McpError> {
        let body = read_http_response_body(response).await?;
        let response_value: serde_json::Value =
            serde_json::from_str(&body).map_err(|err| McpError::protocol(err.to_string()))?;
        let result = validate_json_rpc_result(&response_value, id)?;
        let event_stream_url = resource_event_stream_url(&self.config.url, &result)?;
        self.start_resource_event_reader(event_stream_url.as_str())
            .await
    }

    async fn handle_sse_subscribe_response(
        &self,
        id: u64,
        response: reqwest::Response,
    ) -> Result<(), McpError> {
        let (response_tx, response_rx) = oneshot::channel();
        let update_tx = self.resource_update_tx.clone();
        let reader = tokio::spawn(read_http_sse_messages(
            response,
            Some(id),
            Some(response_tx),
            update_tx,
        ));
        *self.resource_sse_reader.lock().await = Some(reader);
        let result = response_rx.await.map_err(|_| {
            McpError::protocol("MCP HTTP SSE stream ended before subscribe response")
        })?;
        if result.is_err() {
            self.stop_resource_sse_reader().await;
        }
        result
    }
}

fn http_response_is_sse(response: &reqwest::Response) -> bool {
    response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|content_type| content_type.contains("text/event-stream"))
}

async fn read_http_response_body(response: reqwest::Response) -> Result<String, McpError> {
    response
        .text()
        .await
        .map_err(|err| McpError::protocol(format!("failed to read MCP HTTP response: {err}")))
}

fn ensure_http_success(status: reqwest::StatusCode, body: &str) -> Result<(), McpError> {
    if status.is_success() {
        return Ok(());
    }
    Err(McpError::protocol(format!(
        "MCP HTTP server returned {status}: {body}"
    )))
}

fn parse_http_json_rpc_body(body: &str, is_sse: bool) -> Result<serde_json::Value, McpError> {
    if is_sse {
        return parse_sse_json_rpc_response(body);
    }
    serde_json::from_str(body).map_err(|err| McpError::protocol(err.to_string()))
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
        let fut = self.request(
            "tools/call",
            Some(json_obj([
                ("name", serde_json::Value::String(name.to_owned())),
                ("arguments", arguments),
            ])),
        );
        let result = if let Some(ms) = self.config.tool_timeout_ms {
            tokio::time::timeout(std::time::Duration::from_millis(ms), fut)
                .await
                .map_err(|_| McpError::protocol(format!("tool call {name} timed out")))?
        } else {
            fut.await
        }?;
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

    async fn subscribe_resource(&self, uri: &str) -> Result<(), McpError> {
        self.ensure_initialized().await?;
        self.stop_resource_sse_reader().await;
        self.clear_pending_resource_updates().await;
        let (id, response) = self
            .send_raw_request(
                "resources/subscribe",
                Some(json_obj([(
                    "uri",
                    serde_json::Value::String(uri.to_owned()),
                )])),
            )
            .await?;
        self.handle_subscribe_response(id, response).await
    }

    async fn unsubscribe_resource(&self, uri: &str) -> Result<(), McpError> {
        let result = self
            .request(
                "resources/unsubscribe",
                Some(json_obj([(
                    "uri",
                    serde_json::Value::String(uri.to_owned()),
                )])),
            )
            .await
            .map(|_| ());
        self.stop_resource_sse_reader().await;
        result
    }

    async fn next_resource_update(&self) -> Result<McpResourceUpdate, McpError> {
        if self.resource_sse_reader.lock().await.is_none() {
            return Err(McpError::protocol(
                "MCP HTTP resource updates require a live SSE subscribe response",
            ));
        }
        let result = self
            .resource_update_rx
            .lock()
            .await
            .recv()
            .await
            .ok_or_else(|| McpError::protocol("MCP HTTP SSE resource update stream closed"))?;
        if result.is_err() {
            self.stop_resource_sse_reader().await;
        }
        result
    }
}

#[derive(Clone)]
pub struct McpStdioToolAdapter {
    config: McpStdioConfig,
    session: Arc<Mutex<Option<StdioJsonRpcSession>>>,
    supervisor: Option<ProcessSupervisor>,
    supervisor_handle: String,
}

impl McpStdioToolAdapter {
    #[must_use]
    pub fn new(config: McpStdioConfig) -> Self {
        Self {
            config,
            session: Arc::new(Mutex::new(None)),
            supervisor: None,
            supervisor_handle: "mcp-stdio".to_owned(),
        }
    }

    #[must_use]
    pub fn new_supervised(
        config: McpStdioConfig,
        supervisor: ProcessSupervisor,
        handle: impl Into<String>,
    ) -> Self {
        Self {
            config,
            session: Arc::new(Mutex::new(None)),
            supervisor: Some(supervisor),
            supervisor_handle: handle.into(),
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
            if let Some(supervisor) = &self.supervisor {
                let handle = self.supervisor_handle.clone();
                let session = Arc::clone(&self.session);
                supervisor
                    .register(handle, ProcessKind::McpStdio, move |_| {
                        let session = Arc::clone(&session);
                        Box::pin(async move {
                            if let Ok(mut session) = session.try_lock()
                                && let Some(mut active) = session.take()
                            {
                                active.shutdown().await;
                            }
                        })
                    })
                    .await;
            }
        }
        let active = session
            .as_mut()
            .ok_or_else(|| McpError::protocol("MCP stdio session was not initialized"))?;
        let result = active.request(method, params).await;
        if result.is_err() {
            *session = None;
            if let Some(supervisor) = &self.supervisor {
                supervisor.unregister(&self.supervisor_handle).await;
            }
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
        let fut = self.request(
            "tools/call",
            Some(json_obj([
                ("name", serde_json::Value::String(name.to_owned())),
                ("arguments", arguments),
            ])),
        );
        let result = if let Some(ms) = self.config.tool_timeout_ms {
            tokio::time::timeout(std::time::Duration::from_millis(ms), fut)
                .await
                .map_err(|_| McpError::protocol(format!("tool call {name} timed out")))?
        } else {
            fut.await
        }?;
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

    async fn subscribe_resource(&self, uri: &str) -> Result<(), McpError> {
        self.request(
            "resources/subscribe",
            Some(json_obj([(
                "uri",
                serde_json::Value::String(uri.to_owned()),
            )])),
        )
        .await
        .map(|_| ())
    }

    async fn unsubscribe_resource(&self, uri: &str) -> Result<(), McpError> {
        self.request(
            "resources/unsubscribe",
            Some(json_obj([(
                "uri",
                serde_json::Value::String(uri.to_owned()),
            )])),
        )
        .await
        .map(|_| ())
    }

    async fn next_resource_update(&self) -> Result<McpResourceUpdate, McpError> {
        let mut session = self.session.lock().await;
        let Some(active) = session.as_mut() else {
            return Err(McpError::protocol(
                "MCP stdio resource updates require an active subscription",
            ));
        };
        let result = active.next_resource_update().await;
        if result.is_err() {
            *session = None;
        }
        result
    }
}

struct StdioJsonRpcSession {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    response_rx: mpsc::UnboundedReceiver<Result<serde_json::Value, McpError>>,
    resource_update_rx: mpsc::UnboundedReceiver<McpResourceUpdate>,
    reader_task: JoinHandle<()>,
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
        if let Some(cwd) = &config.cwd {
            command.current_dir(cwd);
        }
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
        let (response_tx, response_rx) = mpsc::unbounded_channel();
        let (resource_update_tx, resource_update_rx) = mpsc::unbounded_channel();
        let reader_task =
            tokio::spawn(read_stdio_messages(stdout, response_tx, resource_update_tx));
        Ok(Self {
            child,
            stdin,
            response_rx,
            resource_update_rx,
            reader_task,
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
            let response = self
                .response_rx
                .recv()
                .await
                .ok_or_else(|| McpError::protocol("MCP server closed stdout"))??;
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

    async fn next_resource_update(&mut self) -> Result<McpResourceUpdate, McpError> {
        self.resource_update_rx
            .recv()
            .await
            .ok_or_else(|| McpError::protocol("MCP server closed stdout"))
    }
}

impl Drop for StdioJsonRpcSession {
    fn drop(&mut self) {
        self.reader_task.abort();
        let _ = self.child.start_kill();
    }
}

impl StdioJsonRpcSession {
    async fn shutdown(&mut self) {
        self.reader_task.abort();
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
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

    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|tool| tool.name.clone()).collect()
    }

    #[must_use]
    pub fn with_tool_filter(mut self, enabled: &[String], disabled: &[String]) -> Self {
        let enabled_set: std::collections::HashSet<_> = enabled.iter().cloned().collect();
        let disabled_set: std::collections::HashSet<_> = disabled.iter().cloned().collect();
        self.tools.retain(|tool| {
            if !enabled.is_empty() && !enabled_set.contains(&tool.name) {
                return false;
            }
            if !disabled.is_empty() && disabled_set.contains(&tool.name) {
                return false;
            }
            true
        });
        self
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

#[derive(Debug, Deserialize)]
struct ResourceUpdatedNotificationParams {
    uri: String,
}

async fn read_stdio_messages(
    stdout: tokio::process::ChildStdout,
    response_tx: mpsc::UnboundedSender<Result<serde_json::Value, McpError>>,
    resource_update_tx: mpsc::UnboundedSender<McpResourceUpdate>,
) {
    let mut stdout = BufReader::new(stdout);
    loop {
        let mut line = String::new();
        match stdout.read_line(&mut line).await {
            Ok(0) => {
                let _ = response_tx.send(Err(McpError::protocol("MCP server closed stdout")));
                break;
            }
            Ok(_) => {
                let message = match serde_json::from_str::<serde_json::Value>(&line) {
                    Ok(message) => message,
                    Err(err) => {
                        let _ = response_tx.send(Err(McpError::protocol(err.to_string())));
                        break;
                    }
                };
                if message.get("id").is_some() {
                    if response_tx.send(Ok(message)).is_err() {
                        break;
                    }
                } else if message.get("method").and_then(serde_json::Value::as_str)
                    == Some("notifications/resources/updated")
                {
                    match resource_update_from_notification(&message) {
                        Ok(update) => {
                            let _ = resource_update_tx.send(update);
                        }
                        Err(err) => {
                            let _ = response_tx.send(Err(err));
                            break;
                        }
                    }
                }
            }
            Err(err) => {
                let _ = response_tx.send(Err(McpError::protocol(format!(
                    "failed to read MCP response: {err}"
                ))));
                break;
            }
        }
    }
}

fn resource_update_from_notification(
    message: &serde_json::Value,
) -> Result<McpResourceUpdate, McpError> {
    let params = message
        .get("params")
        .cloned()
        .ok_or_else(|| McpError::protocol("MCP resource update notification missing params"))?;
    let params: ResourceUpdatedNotificationParams =
        serde_json::from_value(params).map_err(|err| McpError::protocol(err.to_string()))?;
    Ok(McpResourceUpdate { uri: params.uri })
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
        let data = sse_event_data(event);
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        return serde_json::from_str(&data).map_err(|err| McpError::protocol(err.to_string()));
    }
    Err(McpError::protocol("MCP SSE response missing data"))
}

async fn read_http_sse_messages(
    response: reqwest::Response,
    expected_response_id: Option<u64>,
    mut response_tx: Option<oneshot::Sender<Result<(), McpError>>>,
    resource_update_tx: mpsc::UnboundedSender<Result<McpResourceUpdate, McpError>>,
) {
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(err) => {
                send_http_sse_error(
                    &mut response_tx,
                    &resource_update_tx,
                    McpError::protocol(format!("failed to read MCP HTTP SSE stream: {err}")),
                );
                return;
            }
        };
        buffer.extend_from_slice(&chunk);
        while let Some(event) = take_sse_event_bytes(&mut buffer) {
            let event = match std::str::from_utf8(&event) {
                Ok(event) => event,
                Err(err) => {
                    send_http_sse_error(
                        &mut response_tx,
                        &resource_update_tx,
                        McpError::protocol(format!("invalid MCP HTTP SSE UTF-8: {err}")),
                    );
                    return;
                }
            };
            let data = sse_event_data(event);
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let message = match serde_json::from_str::<serde_json::Value>(&data) {
                Ok(message) => message,
                Err(err) => {
                    send_http_sse_error(
                        &mut response_tx,
                        &resource_update_tx,
                        McpError::protocol(err.to_string()),
                    );
                    return;
                }
            };
            if expected_response_id
                .is_some_and(|id| message.get("id").and_then(serde_json::Value::as_u64) == Some(id))
            {
                let expected_id = expected_response_id.expect("checked expected id");
                let result = validate_json_rpc_result(&message, expected_id).map(|_| ());
                if let Some(tx) = response_tx.take() {
                    let _ = tx.send(result);
                }
            } else if message.get("method").and_then(serde_json::Value::as_str)
                == Some("notifications/resources/updated")
            {
                match resource_update_from_notification(&message) {
                    Ok(update) => {
                        let _ = resource_update_tx.send(Ok(update));
                    }
                    Err(err) => {
                        send_http_sse_error(&mut response_tx, &resource_update_tx, err);
                        return;
                    }
                }
            }
        }
    }
    send_http_sse_error(
        &mut response_tx,
        &resource_update_tx,
        McpError::protocol("MCP HTTP SSE resource update stream ended"),
    );
}

fn send_http_sse_error(
    response_tx: &mut Option<oneshot::Sender<Result<(), McpError>>>,
    resource_update_tx: &mpsc::UnboundedSender<Result<McpResourceUpdate, McpError>>,
    error: McpError,
) {
    if let Some(tx) = response_tx.take() {
        let _ = tx.send(Err(error));
    } else {
        let _ = resource_update_tx.send(Err(error));
    }
}

fn take_sse_event_bytes(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let lf_index = find_bytes(buffer, b"\n\n");
    let crlf_index = find_bytes(buffer, b"\r\n\r\n");
    let (index, delimiter_len) = match (lf_index, crlf_index) {
        (Some(lf), Some(crlf)) if crlf < lf => (crlf, 4),
        (Some(lf), _) => (lf, 2),
        (None, Some(crlf)) => (crlf, 4),
        (None, None) => return None,
    };
    let event = buffer[..index].to_vec();
    buffer.drain(..index + delimiter_len);
    Some(event)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn sse_event_data(event: &str) -> String {
    event
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n")
}

fn validate_json_rpc_result(
    response_value: &serde_json::Value,
    expected_id: u64,
) -> Result<serde_json::Value, McpError> {
    if response_value.get("id").and_then(serde_json::Value::as_u64) != Some(expected_id) {
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

fn resource_event_stream_url(
    configured_url: &str,
    subscribe_result: &serde_json::Value,
) -> Result<reqwest::Url, McpError> {
    let configured_url = reqwest::Url::parse(configured_url).map_err(|err| {
        McpError::protocol(format!(
            "configured MCP HTTP endpoint URL is invalid: {err}"
        ))
    })?;
    let Some((field, raw_value)) = ["eventStreamUrl", "event_stream_url", "event_url"]
        .into_iter()
        .find_map(|field| subscribe_result.get(field).map(|value| (field, value)))
    else {
        return Ok(configured_url);
    };
    let value = raw_value.as_str().ok_or_else(|| {
        McpError::protocol(format!(
            "MCP HTTP subscribe result {field} must be a string event stream URL"
        ))
    })?;
    if value.trim().is_empty() {
        return Err(McpError::protocol(format!(
            "MCP HTTP subscribe result {field} must not be empty"
        )));
    }
    let event_stream_url = configured_url.join(value).map_err(|err| {
        McpError::protocol(format!(
            "MCP HTTP subscribe result {field} is not a valid event stream URL: {value}: {err}"
        ))
    })?;
    match event_stream_url.scheme() {
        "http" | "https" => Ok(event_stream_url),
        scheme => Err(McpError::protocol(format!(
            "MCP HTTP subscribe result {field} must be an http or https event stream URL, got {scheme}: {value}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::{DateTime, Duration, Utc};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    use super::*;
    use crate::oauth::OAuthTokenSet;

    fn test_oauth_provider(token_url: impl Into<String>) -> OAuthProvider {
        OAuthProvider {
            id: "linear".to_owned(),
            client_id: "test-client-id".to_owned(),
            auth_url: "https://auth.example.com/authorize".to_owned(),
            token_url: token_url.into(),
            scopes: vec!["write".to_owned()],
            default_callback_port: 0,
        }
    }

    fn valid_token_set() -> OAuthTokenSet {
        OAuthTokenSet {
            access_token: "access-123".to_owned(),
            token_type: "Bearer".to_owned(),
            refresh_token: Some("refresh-456".to_owned()),
            expires_at: Some(Utc::now() + Duration::seconds(3600)),
            scopes: vec!["write".to_owned()],
        }
    }

    async fn spawn_token_server(response_body: String) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).await.unwrap() == 0 {
                    break;
                }
                if line.trim().is_empty() {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            writer.write_all(response.as_bytes()).await.unwrap();
        });
        (format!("http://127.0.0.1:{port}/token"), handle)
    }

    #[test]
    fn sse_event_byte_buffer_waits_for_complete_utf8_event() {
        let event = "data: {\"uri\":\"file://docs/雪.md\"}\n\n";
        let split = event.find('雪').expect("snow character");
        let mut buffer = event.as_bytes()[..=split].to_vec();

        assert!(take_sse_event_bytes(&mut buffer).is_none());
        buffer.extend_from_slice(&event.as_bytes()[split + 1..]);
        let event = take_sse_event_bytes(&mut buffer).expect("complete SSE event");

        assert_eq!(
            std::str::from_utf8(&event).expect("valid UTF-8 event"),
            "data: {\"uri\":\"file://docs/雪.md\"}"
        );
        assert!(buffer.is_empty());
    }

    #[tokio::test]
    async fn oauth_token_is_injected_when_store_has_token() {
        let mut store = OAuthStore::default();
        store.set_token("mcp:linear", &valid_token_set());
        let config = McpHttpConfig {
            url: "https://linear.app/mcp".to_owned(),
            headers: BTreeMap::new(),
            server_id: Some("linear".to_owned()),
            oauth_store: Some(Arc::new(RwLock::new(store))),
            oauth_provider: Some(test_oauth_provider("https://token.example.com/token")),
            ..Default::default()
        };
        let adapter = McpHttpToolAdapter::new(config);

        let request = adapter
            .apply_oauth_header(adapter.client.get("https://linear.app/mcp"))
            .await
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(
            request
                .headers()
                .get("Authorization")
                .unwrap()
                .to_str()
                .unwrap(),
            "Bearer access-123"
        );
    }

    #[tokio::test]
    async fn config_authorization_header_is_not_overridden() {
        let mut store = OAuthStore::default();
        store.set_token("mcp:linear", &valid_token_set());
        let mut headers = BTreeMap::new();
        headers.insert("Authorization".to_owned(), "Bearer config-token".to_owned());
        let config = McpHttpConfig {
            url: "https://linear.app/mcp".to_owned(),
            headers,
            server_id: Some("linear".to_owned()),
            oauth_store: Some(Arc::new(RwLock::new(store))),
            oauth_provider: Some(test_oauth_provider("https://token.example.com/token")),
            ..Default::default()
        };
        let adapter = McpHttpToolAdapter::new(config);

        let request = adapter
            .apply_oauth_header(
                adapter
                    .client
                    .get("https://linear.app/mcp")
                    .header("Authorization", "Bearer config-token"),
            )
            .await
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(
            request
                .headers()
                .get("Authorization")
                .unwrap()
                .to_str()
                .unwrap(),
            "Bearer config-token"
        );
    }

    #[tokio::test]
    async fn missing_oauth_token_leaves_request_unchanged() {
        let config = McpHttpConfig {
            url: "https://linear.app/mcp".to_owned(),
            headers: BTreeMap::new(),
            server_id: Some("linear".to_owned()),
            oauth_store: Some(Arc::new(RwLock::new(OAuthStore::default()))),
            oauth_provider: Some(test_oauth_provider("https://token.example.com/token")),
            ..Default::default()
        };
        let adapter = McpHttpToolAdapter::new(config);

        let request = adapter
            .apply_oauth_header(adapter.client.get("https://linear.app/mcp"))
            .await
            .unwrap()
            .build()
            .unwrap();

        assert!(request.headers().get("Authorization").is_none());
    }

    #[tokio::test]
    async fn expired_token_with_refresh_token_updates_header() {
        let (token_url, server) = spawn_token_server(
            r#"{"access_token":"refreshed-token","token_type":"Bearer","expires_in":3600}"#
                .to_owned(),
        )
        .await;

        let mut store = OAuthStore::default();
        store.set_token(
            "mcp:linear",
            &OAuthTokenSet {
                access_token: "expired-token".to_owned(),
                token_type: "Bearer".to_owned(),
                refresh_token: Some("refresh-456".to_owned()),
                expires_at: Some(Utc::now() - Duration::seconds(1)),
                scopes: vec!["write".to_owned()],
            },
        );
        let config = McpHttpConfig {
            url: "https://linear.app/mcp".to_owned(),
            headers: BTreeMap::new(),
            server_id: Some("linear".to_owned()),
            oauth_store: Some(Arc::new(RwLock::new(store))),
            oauth_provider: Some(test_oauth_provider(token_url)),
            ..Default::default()
        };
        let adapter = McpHttpToolAdapter::new(config);

        let request = adapter
            .apply_oauth_header(adapter.client.get("https://linear.app/mcp"))
            .await
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(
            request
                .headers()
                .get("Authorization")
                .unwrap()
                .to_str()
                .unwrap(),
            "Bearer refreshed-token"
        );
        server.abort();
    }

    #[tokio::test]
    async fn refreshed_token_is_persisted_when_store_path_provided() {
        let (token_url, server) = spawn_token_server(
            r#"{"access_token":"refreshed-token","token_type":"Bearer","expires_in":3600}"#
                .to_owned(),
        )
        .await;

        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("oauth.json");
        let mut store = OAuthStore::default();
        store.set_token(
            "mcp:linear",
            &OAuthTokenSet {
                access_token: "expired-token".to_owned(),
                token_type: "Bearer".to_owned(),
                refresh_token: Some("refresh-456".to_owned()),
                expires_at: Some(DateTime::UNIX_EPOCH),
                scopes: vec!["write".to_owned()],
            },
        );
        let config = McpHttpConfig {
            url: "https://linear.app/mcp".to_owned(),
            headers: BTreeMap::new(),
            server_id: Some("linear".to_owned()),
            oauth_store: Some(Arc::new(RwLock::new(store))),
            oauth_provider: Some(test_oauth_provider(token_url)),
            oauth_store_path: Some(store_path.clone()),
            ..Default::default()
        };
        let adapter = McpHttpToolAdapter::new(config);

        let _request = adapter
            .apply_oauth_header(adapter.client.get("https://linear.app/mcp"))
            .await
            .unwrap()
            .build()
            .unwrap();

        let persisted = OAuthStore::load(&store_path).unwrap();
        let token = persisted.get_token("mcp:linear").unwrap();
        assert_eq!(token.access_token, "refreshed-token");
        server.abort();
    }
}
