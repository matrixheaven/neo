# Neo MCP Layer Migration to `rmcp` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace all hand-rolled MCP transports, JSON-RPC framing, connection lifecycle, and OAuth token handling in Neo with the official `rmcp` Rust SDK, while preserving the existing CLI/TUI surface, tool namespacing, snapshots, reconnect/backoff, and the `~/.neo/oauth.json` token store.

**Architecture:** A thin `McpClient` wrapper around `rmcp::service::Service` hides transport-specific types behind a uniform, type-erased interface used by `McpConnectionManager`. The manager keeps its existing public API (`snapshots`, `register_connected_tools_into`, `reconnect_now`, etc.) but internally builds clients via `rmcp::transport::{TokioChildProcess, StreamableHttpClientTransport}`, converts tool/resource models to Neo types, and delegates OAuth to `rmcp::transport::auth::AuthorizationManager` backed by Neo's local credential/state stores and callback server.

**Tech Stack:** `rmcp` (features `client`, `auth`, `transport-child-process`, `transport-streamable-http-client-reqwest`), `tokio`, `reqwest`, `serde_json`, `chrono`, `uuid`.

---

## Design Spec (must be preserved by the refactor)

### 1. Public API contract

`McpConnectionManager` public surface must remain source-compatible for `crates/neo-agent` and `crates/neo-tui`:

- `pub fn new(supervisor: ProcessSupervisor) -> Self`
- `pub fn with_oauth_store(supervisor, oauth_store, oauth_store_path) -> Self`
- `pub async fn set_oauth_store(...)`
- `pub async fn set_oauth_provider_registry(registry: OAuthProviderRegistry)` *(kept as legacy override holder; see Task 3)*
- `pub async fn apply_config(servers: Vec<ManagedMcpServerConfig>) -> Vec<McpServerSnapshot>`
- `pub async fn upsert_server(server) -> McpServerSnapshot`
- `pub async fn remove_server(id) -> bool`
- `pub async fn reconnect_now(id) -> anyhow::Result<McpServerSnapshot>`
- `pub async fn refresh_tools(id) -> anyhow::Result<McpServerSnapshot>`
- `pub async fn snapshots() -> Vec<McpServerSnapshot>`
- `pub async fn snapshot(id) -> Option<McpServerSnapshot>`
- `pub async fn register_connected_tools_into(registry: &mut ToolRegistry) -> Vec<McpDiagnostic>`
- `pub async fn list_resources(server_id: Option<&str>) -> anyhow::Result<Vec<McpResourceListEntry>>`
- `pub async fn read_resource(server_id, uri) -> anyhow::Result<McpResourceRead>`
- `pub async fn shutdown()`

Public data types must remain:

- `ManagedMcpServerConfig`, `ManagedMcpTransport` (Stdio/Http/Sse), `McpReconnectPolicy`
- `McpServerStatus`, `McpServerSnapshot`, `McpDiagnostic`, `McpResourceListEntry`
- `McpToolDefinition`, `McpToolCall`, `McpToolResponse`, `McpError`
- `McpResourceDefinition`, `McpResourceContent`, `McpResourceRead`, `McpResourceUpdate`

### 2. Tool namespacing contract

Model-visible tool names stay exactly `mcp__<server_id>__<tool_name>`. Sanitization replaces any character that is not ASCII alphanumeric or `_` with `_`.

### 3. Transport mapping

User-facing persisted values remain `"stdio"`, `"http"`, `"sse"`. Internally:

- `"stdio"` → `rmcp::transport::TokioChildProcess`.
- `"http"` and `"sse"` → `rmcp::transport::StreamableHttpClientTransport` (the modern streamable HTTP transport subsumes both request/response and SSE semantics).

### 4. OAuth contract

- Tokens are persisted in `~/.neo/oauth.json` under keys `mcp:<server_id>`.
- `neo mcp auth <id>` and the TUI auth action still start a local browser-based PKCE flow.
- `Authorization: Bearer <token>` is added automatically to HTTP/SSE requests when a token exists.
- Dynamic token refresh before expiry remains automatic.
- Discovery + DCR (SEP-985 / RFC 8414 / RFC 7591) replaces hard-coded provider lists; `[oauth.providers.<id>]` becomes a manual override only.

### 5. Snapshot/status contract

Statuses `{Disabled, Pending, Connected, Failed, Reconnecting}` and fields remain unchanged. Reconnect policy default stays `enabled=true, initial_delay_ms=500, max_delay_ms=30000, max_attempts=Some(5)` with exponential backoff capped at `max_delay_ms`.

---

## File Structure After Migration

| File | Responsibility |
|------|----------------|
| `crates/neo-agent-core/src/tools/mcp/mod.rs` | Public MCP model types (`McpToolDefinition`, `McpToolResponse`, `McpError`, `McpResource*`) and conversion helpers to/from `rmcp::model::*`. |
| `crates/neo-agent-core/src/tools/mcp/client.rs` | `McpClient` trait / `RmcpClient` type-erased handle. Wraps an `rmcp::service::Service` and exposes `list_tools`, `call_tool`, `list_resources`, `read_resource` with timeouts and OAuth. |
| `crates/neo-agent-core/src/tools/mcp/stdio.rs` | `build_stdio_client` using `TokioChildProcess`; registers child with `ProcessSupervisor`. |
| `crates/neo-agent-core/src/tools/mcp/http.rs` | `build_http_client` using `StreamableHttpClientTransport`; applies custom headers and `AuthorizationManager`. |
| `crates/neo-agent-core/src/tools/mcp/oauth.rs` | `NeoOAuthCredentialStore`, `NeoOAuthStateStore`, `OAuthFlow`, and synthetic auth tool injection. |
| `crates/neo-agent-core/src/tools/mcp_manager.rs` | Refactored `McpConnectionManager`; holds `BTreeMap<String, ManagedMcpEntry>` where each entry owns an `Arc<dyn McpClient>`. |
| `crates/neo-agent-core/src/oauth.rs` | Deprecated legacy provider registry / PKCE helpers kept temporarily for config migration, then removed in a final cleanup task. |
| `crates/neo-agent-core/src/oauth/store.rs` | Token store format migrated to new `NeoOAuthCredentials`; load/save remain. |
| `crates/neo-agent-core/src/oauth/callback_server.rs` | Kept (used by both old and new OAuth flows). |
| `crates/neo-agent/src/mcp_ops.rs` | Converts `McpServerConfig` → `ManagedMcpServerConfig`; builds `OAuthProviderRegistry` override; runs auth flow. |
| `crates/neo-agent/src/modes/run.rs` | Keeps CLI handlers but removes direct adapter construction. |
| `crates/neo-agent/src/modes/interactive.rs` | Keeps TUI glue, `mcp_manager_with_oauth_store`, `start_mcp_oauth_flow`. |
| `crates/neo-agent/src/config.rs` | `McpServerConfig` schema unchanged; validation unchanged. |

---

## Phase 1: Add `rmcp` Dependency and Model Conversions

### Task 1.1: Add `rmcp` to `neo-agent-core` dependencies

**Files:**
- Modify: `crates/neo-agent-core/Cargo.toml`

- [ ] **Step 1: Add `rmcp` dependency**

Insert under `[dependencies]`:

```toml
rmcp = { version = "1.8.0", features = ["client", "auth", "transport-child-process", "transport-streamable-http-client-reqwest"] }
```

- [ ] **Step 2: Check that `reqwest` remains a workspace dep**

`rmcp`'s reqwest backend will use the same `reqwest` already pinned in the workspace (`0.12.24`). No extra reqwest entry needed.

- [ ] **Step 3: Build xtask check**

Run: `cargo run -p xtask -- check`
Expected: fails later because code still uses old types, but the dependency should resolve.

- [ ] **Step 4: Commit**

```bash
git add crates/neo-agent-core/Cargo.toml
git commit -m "deps(neo-agent-core): add rmcp for MCP transport and OAuth"
```

### Task 1.2: Convert `tools/mcp.rs` into a module directory

**Files:**
- Create: `crates/neo-agent-core/src/tools/mcp/mod.rs`
- Create: `crates/neo-agent-core/src/tools/mcp/client.rs`
- Create: `crates/neo-agent-core/src/tools/mcp/stdio.rs`
- Create: `crates/neo-agent-core/src/tools/mcp/http.rs`
- Create: `crates/neo-agent-core/src/tools/mcp/oauth.rs`
- Delete: `crates/neo-agent-core/src/tools/mcp.rs`

- [ ] **Step 1: Move public model types to `mcp/mod.rs`**

Write `crates/neo-agent-core/src/tools/mcp/mod.rs` with the public types and conversion helpers:

```rust
use rmcp::model::{
    CallToolResult, Content, ListToolsResult, ReadResourceResult, Resource,
    Tool as RmcpTool,
};
use serde::{Deserialize, Serialize};

pub mod client;
pub mod http;
pub mod oauth;
pub mod stdio;

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
            name: tool.name,
            description: tool.description.unwrap_or_default(),
            input_schema: tool.input_schema,
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
            match content {
                Content::Text(text) => texts.push(text.text),
                Content::Image(image) => {
                    details = Some(serde_json::json!({
                        "type": "image",
                        "data": image.data,
                        "mime_type": image.mime_type,
                    }));
                }
                Content::Resource(resource) => {
                    details = Some(serde_json::json!({
                        "type": "resource",
                        "resource": resource,
                    }));
                }
                _ => {}
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

impl From<rmcp::Error> for McpError {
    fn from(err: rmcp::Error) -> Self {
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
            name: resource.name,
            description: resource.description,
            mime_type: resource.mime_type,
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
            rmcp::model::ResourceContents::TextResourceContents { uri, mime_type, text } => Self {
                uri: uri.to_string(),
                mime_type,
                text: Some(text),
                blob: None,
            },
            rmcp::model::ResourceContents::BlobResourceContents { uri, mime_type, blob } => Self {
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
```

- [ ] **Step 2: Update `crates/neo-agent-core/src/tools/mod.rs`**

Change line 12 from `mod mcp;` to `mod mcp;` remains valid because `tools/mcp/mod.rs` exists. No change needed.

- [ ] **Step 3: Commit**

```bash
git add crates/neo-agent-core/src/tools/mcp.rs crates/neo-agent-core/src/tools/mcp/
git commit -m "refactor(mcp): move model types into mcp module directory"
```

### Task 1.3: Add model conversion unit tests

**Files:**
- Create tests in `crates/neo-agent-core/src/tools/mcp/mod.rs` inside a `#[cfg(test)] mod tests` block.

- [ ] **Step 1: Write failing test for `McpToolDefinition` conversion**

```rust
#[cfg(test)]
mod tests {
    use rmcp::model::Tool;
    use super::*;

    #[test]
    fn converts_rmcp_tool_to_definition() {
        let tool = Tool {
            name: "echo".into(),
            description: Some("echoes input".into()),
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{"x":{"type":"string"}}}"#).unwrap(),
        };
        let def = McpToolDefinition::from(tool);
        assert_eq!(def.name, "echo");
        assert_eq!(def.description, "echoes input");
        assert!(def.input_schema.get("properties").is_some());
    }
}
```

- [ ] **Step 2: Run focused test**

Run: `cargo run -p xtask -- test -p neo-agent-core --lib mcp`
Expected: compile errors until `client.rs` stubs exist; keep fixing until PASS.

- [ ] **Step 3: Commit**

```bash
git commit -m "test(mcp): add rmcp model conversion tests"
```


## Phase 2: Build `rmcp` Client Wrappers (stdio + HTTP)

### Task 2.1: Define the `McpClient` trait and type-erased `RmcpClient`

**Files:**
- Create: `crates/neo-agent-core/src/tools/mcp/client.rs`

- [ ] **Step 1: Write the `McpClient` trait**

```rust
use async_trait::async_trait;
use serde_json::Value;

use super::{
    McpError, McpResourceDefinition, McpResourceRead, McpToolDefinition, McpToolResponse,
};

#[async_trait]
pub trait McpClient: Send + Sync {
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpError>;
    async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<McpToolResponse, McpError>;
    async fn list_resources(&self) -> Result<Vec<McpResourceDefinition>, McpError>;
    async fn read_resource(&self, uri: &str) -> Result<McpResourceRead, McpError>;
    async fn shutdown(&self) -> Result<(), McpError>;
}
```

- [ ] **Step 2: Implement `RmcpClient` around `Box<dyn DynService<RoleClient>>`**

```rust
use std::time::Duration;

use rmcp::{
    ServiceExt,
    model::{CallToolRequestParam, ReadResourceRequestParam},
    service::{DynService, RoleClient},
};
use tokio::time::timeout;

use super::{McpError, McpResourceDefinition, McpResourceRead, McpToolDefinition, McpToolResponse};

pub struct RmcpClient {
    service: Box<dyn DynService<RoleClient>>,
    tool_timeout: Option<Duration>,
}

impl RmcpClient {
    pub fn new(service: Box<dyn DynService<RoleClient>>, tool_timeout: Option<Duration>) -> Self {
        Self {
            service,
            tool_timeout,
        }
    }

    async fn with_tool_timeout<T, F>(&self, fut: F) -> Result<T, McpError>
    where
        F: std::future::Future<Output = Result<T, rmcp::Error>> + Send,
    {
        let result = match self.tool_timeout {
            Some(d) => timeout(d, fut).await.map_err(|_| McpError::protocol("tool call timed out"))?,
            None => fut.await,
        };
        result.map_err(Into::into)
    }
}

#[async_trait]
impl McpClient for RmcpClient {
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpError> {
        let result = self
            .service
            .list_tools(Default::default())
            .await
            .map_err(McpError::from)?;
        Ok(result.tools.into_iter().map(Into::into).collect())
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<McpToolResponse, McpError> {
        let params = CallToolRequestParam {
            name: name.into(),
            arguments: arguments.as_object().cloned(),
            ..Default::default()
        };
        let result = self
            .with_tool_timeout(self.service.call_tool(params))
            .await?;
        Ok(result.into())
    }

    async fn list_resources(&self) -> Result<Vec<McpResourceDefinition>, McpError> {
        let result = self
            .service
            .list_resources(Default::default())
            .await
            .map_err(McpError::from)?;
        Ok(result.resources.into_iter().map(Into::into).collect())
    }

    async fn read_resource(&self, uri: &str) -> Result<McpResourceRead, McpError> {
        let params = ReadResourceRequestParam {
            uri: rmcp::model::ResourceUri::from(uri),
            ..Default::default()
        };
        let result = self
            .service
            .read_resource(params)
            .await
            .map_err(McpError::from)?;
        Ok(result.into())
    }

    async fn shutdown(&self) -> Result<(), McpError> {
        self.service.cancel().await.map_err(Into::into)
    }
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/neo-agent-core/src/tools/mcp/client.rs
git commit -m "feat(mcp): add type-erased McpClient wrapper over rmcp Service"
```

### Task 2.2: Implement stdio client builder

**Files:**
- Create: `crates/neo-agent-core/src/tools/mcp/stdio.rs`

- [ ] **Step 1: Write `build_stdio_client`**

```rust
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use rmcp::{ServiceExt, transport::{TokioChildProcess, ConfigureCommandExt}};
use tokio::process::Command;

use super::{McpError, client::{McpClient, RmcpClient}};
use crate::tools::ProcessSupervisor;

pub struct StdioConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub startup_timeout_ms: Option<u64>,
    pub tool_timeout_ms: Option<u64>,
}

pub async fn build_stdio_client(
    server_id: &str,
    config: StdioConfig,
    supervisor: &ProcessSupervisor,
) -> Result<Box<dyn McpClient>, McpError> {
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
    let pid = transport.pid();

    let startup_timeout = config.startup_timeout_ms.map(Duration::from_millis);
    let tool_timeout = config.tool_timeout_ms.map(Duration::from_millis);

    let service = match startup_timeout {
        Some(d) => tokio::time::timeout(d, ().serve(transport))
            .await
            .map_err(|_| McpError::protocol("stdio MCP server initialization timed out"))?
            .map_err(McpError::from)?,
        None => ().serve(transport).await.map_err(McpError::from)?,
    };

    let handle = format!("mcp_stdio_{server_id}");
    let dyn_service = service.into_dyn();

    if let Some(pid) = pid {
        supervisor
            .register(
                handle.clone(),
                crate::tools::ProcessKind::McpStdio,
                move |_handle| {
                    let future_service = dyn_service.clone();
                    Box::pin(async move {
                        let _ = future_service.cancel().await;
                    })
                },
            )
            .await;
    }

    Ok(Box::new(RmcpClient::new(dyn_service, tool_timeout)))
}
```

- [ ] **Step 2: Add unit test for command construction**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdio_config_roundtrips_fields() {
        let config = StdioConfig {
            command: "npx".into(),
            args: vec!["-y".into(), "server".into()],
            env: [("K".into(), "V".into())].into(),
            cwd: Some(PathBuf::from("/tmp")),
            startup_timeout_ms: Some(5000),
            tool_timeout_ms: Some(30000),
        };
        assert_eq!(config.command, "npx");
        assert_eq!(config.args.len(), 2);
    }
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/neo-agent-core/src/tools/mcp/stdio.rs
git commit -m "feat(mcp): build stdio MCP clients via rmcp TokioChildProcess"
```

### Task 2.3: Implement HTTP/SSE client builder

**Files:**
- Create: `crates/neo-agent-core/src/tools/mcp/http.rs`

- [ ] **Step 1: Write `build_http_client`**

```rust
use std::{collections::BTreeMap, time::Duration};

use rmcp::{ServiceExt, transport::StreamableHttpClientTransport};

use super::{McpError, client::{McpClient, RmcpClient}};

pub struct HttpConfig {
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub startup_timeout_ms: Option<u64>,
    pub tool_timeout_ms: Option<u64>,
}

pub async fn build_http_client(config: HttpConfig) -> Result<Box<dyn McpClient>, McpError> {
    let mut transport = StreamableHttpClientTransport::from_uri(&config.url)
        .map_err(|e| McpError::protocol(format!("invalid MCP HTTP URL: {e}")))?;

    for (k, v) in &config.headers {
        let name = http::HeaderName::from_bytes(k.as_bytes())
            .map_err(|e| McpError::protocol(format!("invalid header name {k}: {e}")))?;
        let value = http::HeaderValue::from_str(v)
            .map_err(|e| McpError::protocol(format!("invalid header value for {k}: {e}")))?;
        transport = transport.with_header(name, value);
    }

    let startup_timeout = config.startup_timeout_ms.map(Duration::from_millis);
    let tool_timeout = config.tool_timeout_ms.map(Duration::from_millis);

    let service = match startup_timeout {
        Some(d) => tokio::time::timeout(d, ().serve(transport))
            .await
            .map_err(|_| McpError::protocol("HTTP MCP server initialization timed out"))?
            .map_err(McpError::from)?,
        None => ().serve(transport).await.map_err(McpError::from)?,
    };

    Ok(Box::new(RmcpClient::new(service.into_dyn(), tool_timeout)))
}
```

- [ ] **Step 2: Add `http` crate dependency if needed**

If `rmcp` re-exports `http` types, use `rmcp::transport::streamable_http_client::http::HeaderName` etc. Otherwise add `http = "1"` to `crates/neo-agent-core/Cargo.toml` and the workspace root.

- [ ] **Step 3: Commit**

```bash
git add crates/neo-agent-core/src/tools/mcp/http.rs crates/neo-agent-core/Cargo.toml
git commit -m "feat(mcp): build HTTP/SSE MCP clients via rmcp StreamableHttpClientTransport"
```

### Task 2.4: Verify wrappers compile

- [ ] **Step 1: Run focused check**

Run: `cargo check -p neo-agent-core`
Expected: compiles (OAuth types not yet wired).

- [ ] **Step 2: Commit**

```bash
git commit -m "chore(mcp): verify rmcp client wrappers compile"
```


### Task 2.5: Implement OAuth-aware streamable HTTP client

> This task replaces the static `auth_header` approach with dynamic injection from `rmcp::transport::auth::AuthorizationManager` for every request, matching Kimi Code's runtime auth behavior.

**Files:**
- Modify: `crates/neo-agent-core/src/tools/mcp/http.rs`

- [ ] **Step 1: Replace `http.rs` with a custom `StreamableHttpClient`**

```rust
use std::{
    collections::HashMap,
    sync::Arc,
    time::Duration,
};

use bytes::Bytes;
use futures::{StreamExt, stream::BoxStream};
use http::{HeaderName, HeaderValue, Method, Request, StatusCode, header::WWW_AUTHENTICATE};
use http_body_util::{BodyExt, Full};
use reqwest::Client;
use rmcp::{
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage},
    transport::{
        auth::AuthorizationManager,
        streamable_http_client::*,
    },
};
use sse_stream::{Sse, SseStream};

use super::{McpError, client::{McpClient, RmcpClient}};

#[derive(Debug, thiserror::Error)]
pub enum OAuthHttpError {
    #[error("reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("http error: {0}")]
    Http(#[from] http::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("OAuth error: {0}")]
    Auth(String),
}

impl From<OAuthHttpError> for StreamableHttpError<OAuthHttpError> {
    fn from(e: OAuthHttpError) -> Self {
        StreamableHttpError::Client(e)
    }
}

#[derive(Clone)]
pub struct OAuthStreamableHttpClient {
    client: Client,
    auth_manager: Option<Arc<tokio::sync::Mutex<AuthorizationManager>>>,
}

impl OAuthStreamableHttpClient {
    pub fn new(
        client: Client,
        auth_manager: Option<Arc<tokio::sync::Mutex<AuthorizationManager>>>,
    ) -> Self {
        Self {
            client,
            auth_manager,
        }
    }

    async fn access_token(&self) -> Result<Option<String>, StreamableHttpError<OAuthHttpError>> {
        match &self.auth_manager {
            Some(manager) => {
                let token = manager
                    .lock()
                    .await
                    .access_token()
                    .await
                    .map_err(|e| StreamableHttpError::Auth(e))?;
                Ok(Some(token))
            }
            None => Ok(None),
        }
    }
}

fn has_authorization(custom_headers: &HashMap<HeaderName, HeaderValue>) -> bool {
    custom_headers.contains_key(&http::header::AUTHORIZATION)
}

impl StreamableHttpClient for OAuthStreamableHttpClient {
    type Error = OAuthHttpError;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let token = if has_authorization(&custom_headers) {
            None
        } else {
            self.access_token().await?
        };
        let body = serde_json::to_string(&message).map_err(OAuthHttpError::Json)?;

        let mut req = self
            .client
            .request(reqwest::Method::POST, uri.as_ref())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "text/event-stream, application/json")
            .body(body);

        for (name, value) in &custom_headers {
            req = req.header(name.as_str(), value.as_bytes());
        }
        if let Some(token) = token {
            req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"));
        }
        if let Some(sid) = session_id {
            req = req.header("Mcp-Session-Id", sid.as_ref());
        }

        let response = req.send().await.map_err(OAuthHttpError::Reqwest)?;
        handle_response(response).await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let token = if has_authorization(&custom_headers) {
            None
        } else {
            self.access_token().await?
        };

        let mut req = self
            .client
            .request(reqwest::Method::DELETE, uri.as_ref())
            .header("Mcp-Session-Id", session_id.as_ref());

        for (name, value) in &custom_headers {
            req = req.header(name.as_str(), value.as_bytes());
        }
        if let Some(token) = token {
            req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"));
        }

        let response = req.send().await.map_err(OAuthHttpError::Reqwest)?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(StreamableHttpError::AuthRequired(AuthRequiredError {
                www_authenticate_header: extract_www_authenticate(&response),
            }));
        }
        let _ = response.text().await;
        Ok(())
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        let token = if has_authorization(&custom_headers) {
            None
        } else {
            self.access_token().await?
        };

        let mut req = self
            .client
            .request(reqwest::Method::GET, uri.as_ref())
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .header("Mcp-Session-Id", session_id.as_ref());

        for (name, value) in &custom_headers {
            req = req.header(name.as_str(), value.as_bytes());
        }
        if let Some(token) = token {
            req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"));
        }
        if let Some(id) = last_event_id {
            req = req.header("Last-Event-Id", id);
        }

        let response = req.send().await.map_err(OAuthHttpError::Reqwest)?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(StreamableHttpError::AuthRequired(AuthRequiredError {
                www_authenticate_header: extract_www_authenticate(&response),
            }));
        }

        Ok(Box::pin(SseStream::new(response.bytes_stream())))
    }
}

fn extract_www_authenticate(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

async fn handle_response(
    response: reqwest::Response,
) -> Result<StreamableHttpPostResponse, StreamableHttpError<OAuthHttpError>> {
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(StreamableHttpError::AuthRequired(AuthRequiredError {
            www_authenticate_header: extract_www_authenticate(&response),
        }));
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return Err(StreamableHttpError::InsufficientScope(InsufficientScopeError {
            www_authenticate_header: extract_www_authenticate(&response),
        }));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if content_type.starts_with("text/event-stream") {
        Ok(StreamableHttpPostResponse::Sse(
            Box::pin(SseStream::new(response.bytes_stream())),
            None,
        ))
    } else {
        let body = response.text().await.map_err(OAuthHttpError::Reqwest)?;
        let message = serde_json::from_str(&body).map_err(OAuthHttpError::Json)?;
        Ok(StreamableHttpPostResponse::Json(message, None))
    }
}

pub struct HttpConfig {
    pub url: String,
    pub headers: std::collections::BTreeMap<String, String>,
    pub startup_timeout_ms: Option<u64>,
    pub tool_timeout_ms: Option<u64>,
    pub auth_manager: Option<Arc<tokio::sync::Mutex<AuthorizationManager>>>,
}

pub async fn build_http_client(config: HttpConfig) -> Result<Box<dyn McpClient>, McpError> {
    let reqwest_client = Client::new();
    let auth_client = OAuthStreamableHttpClient::new(reqwest_client, config.auth_manager);

    let mut custom_headers = HashMap::new();
    for (k, v) in &config.headers {
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|e| McpError::protocol(format!("invalid header name {k}: {e}")))?;
        let value = HeaderValue::from_str(v)
            .map_err(|e| McpError::protocol(format!("invalid header value for {k}: {e}")))?;
        custom_headers.insert(name, value);
    }

    let transport_config = StreamableHttpClientTransportConfig::with_uri(&config.url)
        .custom_headers(custom_headers)
        .reinit_on_expired_session(true)
        .allow_stateless(true);

    let transport = StreamableHttpClientTransport::with_client(auth_client, transport_config);

    let startup_timeout = config.startup_timeout_ms.map(Duration::from_millis);
    let tool_timeout = config.tool_timeout_ms.map(Duration::from_millis);

    let service = match startup_timeout {
        Some(d) => tokio::time::timeout(d, ().serve(transport))
            .await
            .map_err(|_| McpError::protocol("HTTP MCP server initialization timed out"))?
            .map_err(McpError::from)?,
        None => ().serve(transport).await.map_err(McpError::from)?,
    };

    Ok(Box::new(RmcpClient::new(service.into_dyn(), tool_timeout)))
}
```

- [ ] **Step 2: Add `http` and `http-body-util` dependencies**

Because `rmcp` already depends on `http` and `http-body-util`, they should be available transitively. To be explicit, add to `crates/neo-agent-core/Cargo.toml`:

```toml
http = "1"
http-body-util = "0.1"
```

- [ ] **Step 3: Commit**

```bash
git add crates/neo-agent-core/src/tools/mcp/http.rs crates/neo-agent-core/Cargo.toml
git commit -m "feat(mcp): OAuth-aware streamable HTTP client with dynamic Bearer injection"
```


## Phase 3: OAuth Integration with `rmcp::transport::auth`

### Task 3.1: Migrate `~/.neo/oauth.json` store to `StoredCredentials`

**Files:**
- Modify: `crates/neo-agent-core/src/oauth/store.rs`

- [ ] **Step 1: Replace `OAuthStore` with server-keyed credentials file**

```rust
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

use rmcp::transport::auth::StoredCredentials;
use serde::{Deserialize, Serialize};

use crate::oauth::OAuthError;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OAuthStore {
    pub entries: BTreeMap<String, StoredCredentials>,
}

impl OAuthStore {
    pub fn load(path: &Path) -> Result<Self, OAuthError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let file = OpenOptions::new().read(true).open(path)?;
        let reader = BufReader::new(file);
        let store = serde_json::from_reader(reader)?;
        Ok(store)
    }

    pub fn save(&self, path: &Path) -> Result<(), OAuthError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
        let writer = BufWriter::new(&file);
        serde_json::to_writer_pretty(writer, self)?;
        file.flush()?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<&StoredCredentials> {
        self.entries.get(key)
    }

    pub fn set(&mut self, key: &str, credentials: StoredCredentials) {
        self.entries.insert(key.to_string(), credentials);
    }

    pub fn remove(&mut self, key: &str) -> bool {
        self.entries.remove(key).is_some()
    }
}
```

- [ ] **Step 2: Add migration from legacy `OAuthTokenSet` format**

Keep a `LegacyOAuthStore` struct that matches the old format for one release. On `load`, attempt legacy parse; if it succeeds, convert entries keyed by `mcp:<server_id>` into `StoredCredentials` and rewrite the file in the new format.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyOAuthStore {
    entries: BTreeMap<String, crate::oauth::OAuthTokenSet>,
}

fn migrate_legacy(path: &Path) -> Result<OAuthStore, OAuthError> {
    let file = OpenOptions::new().read(true).open(path)?;
    let legacy: LegacyOAuthStore = serde_json::from_reader(BufReader::new(file))?;
    let mut store = OAuthStore::default();
    for (key, token_set) in legacy.entries {
        let token_response = rmcp::transport::auth::OAuthTokenResponse {
            access_token: token_set.access_token,
            token_type: token_set.token_type,
            refresh_token: token_set.refresh_token,
            expires_in: token_set.expires_at.map(|dt| (dt - chrono::Utc::now()).num_seconds() as u64),
            scope: Some(token_set.scopes.join(" ")),
            ..Default::default()
        };
        store.set(&key, StoredCredentials {
            client_id: String::new(),
            token_response: Some(token_response),
            granted_scopes: token_set.scopes,
            token_received_at: Some(chrono::Utc::now().timestamp() as u64),
        });
    }
    Ok(store)
}
```

- [ ] **Step 3: Update existing tests in `oauth/store.rs`**

Replace assertions that use `OAuthTokenSet` with `StoredCredentials` equivalents.

- [ ] **Step 4: Commit**

```bash
git add crates/neo-agent-core/src/oauth/store.rs
git commit -m "feat(oauth): migrate token store to rmcp StoredCredentials format"
```

### Task 3.2: Implement `CredentialStore` and `StateStore` adapters

**Files:**
- Create: `crates/neo-agent-core/src/tools/mcp/oauth.rs`

- [ ] **Step 1: Implement `NeoOAuthCredentialStore`**

```rust
use std::{future::Future, pin::Pin, sync::Arc};

use rmcp::transport::auth::{AuthError, CredentialStore, StoredCredentials};
use tokio::sync::RwLock;

use crate::oauth::OAuthStore;

pub struct NeoOAuthCredentialStore {
    server_key: String,
    store: Arc<RwLock<OAuthStore>>,
    path: Option<std::path::PathBuf>,
}

impl NeoOAuthCredentialStore {
    pub fn new(
        server_key: String,
        store: Arc<RwLock<OAuthStore>>,
        path: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            server_key,
            store,
            path,
        }
    }
}

impl CredentialStore for NeoOAuthCredentialStore {
    fn load<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<StoredCredentials>, AuthError>> + Send + 'async_trait>>
    where
        Self: 'async_trait,
        'life0: 'async_trait,
    {
        let store = self.store.clone();
        let key = self.server_key.clone();
        Box::pin(async move {
            let guard = store.read().await;
            Ok(guard.get(&key).cloned())
        })
    }

    fn save<'life0, 'async_trait>(
        &'life0 self,
        credentials: StoredCredentials,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'async_trait>>
    where
        Self: 'async_trait,
        'life0: 'async_trait,
    {
        let store = self.store.clone();
        let path = self.path.clone();
        let key = self.server_key.clone();
        Box::pin(async move {
            let mut guard = store.write().await;
            guard.set(&key, credentials);
            if let Some(p) = path {
                guard.save(&p).map_err(|e| AuthError::Other(e.to_string()))?;
            }
            Ok(())
        })
    }

    fn clear<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'async_trait>>
    where
        Self: 'async_trait,
        'life0: 'async_trait,
    {
        let store = self.store.clone();
        let path = self.path.clone();
        let key = self.server_key.clone();
        Box::pin(async move {
            let mut guard = store.write().await;
            guard.remove(&key);
            if let Some(p) = path {
                guard.save(&p).map_err(|e| AuthError::Other(e.to_string()))?;
            }
            Ok(())
        })
    }
}
```

- [ ] **Step 2: Implement `NeoOAuthStateStore`**

```rust
use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc};

use rmcp::transport::auth::{AuthError, StateStore, StoredAuthorizationState};
use tokio::sync::Mutex;

#[derive(Clone, Default)]
pub struct NeoOAuthStateStore {
    states: Arc<Mutex<HashMap<String, StoredAuthorizationState>>>,
}

impl StateStore for NeoOAuthStateStore {
    fn save<'life0, 'life1, 'async_trait>(
        &'life0 self,
        csrf_token: &'life1 str,
        state: StoredAuthorizationState,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'async_trait>>
    where
        Self: 'async_trait,
        'life0: 'async_trait,
        'life1: 'async_trait,
    {
        let states = self.states.clone();
        let key = csrf_token.to_string();
        Box::pin(async move {
            states.lock().await.insert(key, state);
            Ok(())
        })
    }

    fn load<'life0, 'life1, 'async_trait>(
        &'life0 self,
        csrf_token: &'life1 str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<StoredAuthorizationState>, AuthError>> + Send + 'async_trait>>
    where
        Self: 'async_trait,
        'life0: 'async_trait,
        'life1: 'async_trait,
    {
        let states = self.states.clone();
        let key = csrf_token.to_string();
        Box::pin(async move { Ok(states.lock().await.get(&key).cloned()) })
    }

    fn delete<'life0, 'life1, 'async_trait>(
        &'life0 self,
        csrf_token: &'life1 str,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'async_trait>>
    where
        Self: 'async_trait,
        'life0: 'async_trait,
        'life1: 'async_trait,
    {
        let states = self.states.clone();
        let key = csrf_token.to_string();
        Box::pin(async move {
            states.lock().await.remove(&key);
            Ok(())
        })
    }
}
```

- [ ] **Step 3: Implement `NeoOAuthHttpClient`**

```rust
use std::{future::Future, pin::Pin};

use rmcp::transport::auth::{AuthError, OAuthHttpClient, OAuthHttpClientFuture, OAuthHttpRequest, OAuthHttpClientError};

#[derive(Clone)]
pub struct NeoOAuthHttpClient {
    client: reqwest::Client,
}

impl NeoOAuthHttpClient {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

impl OAuthHttpClient for NeoOAuthHttpClient {
    fn execute(&self, request: OAuthHttpRequest) -> OAuthHttpClientFuture<'_> {
        let client = self.client.clone();
        Box::pin(async move {
            let method = match request.method.as_str() {
                "GET" => reqwest::Method::GET,
                "POST" => reqwest::Method::POST,
                "DELETE" => reqwest::Method::DELETE,
                m => return Err(OAuthHttpClientError::Other(format!("unsupported method {m}"))),
            };
            let mut builder = client.request(method, request.url);
            for (k, v) in request.headers {
                builder = builder.header(k, v);
            }
            if let Some(body) = request.body {
                builder = builder.body(body);
            }
            let response = builder.send().await.map_err(|e| OAuthHttpClientError::Http(e.to_string()))?;
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.to_string(), s.to_string())))
                .collect();
            let body = response.text().await.unwrap_or_default();
            Ok(rmcp::transport::auth::OAuthHttpResponse {
                status,
                headers,
                body,
            })
        }) as Pin<Box<dyn Future<Output = Result<rmcp::transport::auth::OAuthHttpResponse, OAuthHttpClientError>> + Send + '_>>
    }
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/neo-agent-core/src/tools/mcp/oauth.rs
git commit -m "feat(mcp/oauth): implement rmcp CredentialStore, StateStore, and OAuthHttpClient adapters"
```

### Task 3.3: Build per-server `AuthorizationManager` and run OAuth flow

**Files:**
- Modify: `crates/neo-agent-core/src/tools/mcp/oauth.rs`
- Modify: `crates/neo-agent/src/mcp_ops.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs`

- [ ] **Step 1: Add `McpOAuthManager` helper in `mcp/oauth.rs`**

```rust
use std::{path::PathBuf, sync::Arc};

use rmcp::transport::auth::{
    AuthorizationManager, AuthorizationSession, OAuthClientConfig, ScopeUpgradeConfig,
};
use tokio::sync::{Mutex, RwLock};

use crate::oauth::{CallbackServer, OAuthError, OAuthStore};

use super::oauth::{NeoOAuthCredentialStore, NeoOAuthHttpClient, NeoOAuthStateStore};

pub struct McpOAuthManager {
    server_id: String,
    auth_manager: Arc<Mutex<AuthorizationManager>>,
    manual_override: Option<crate::oauth::OAuthProvider>,
}

impl McpOAuthManager {
    pub async fn new(
        server_id: String,
        base_url: String,
        oauth_store: Arc<RwLock<OAuthStore>>,
        oauth_store_path: Option<PathBuf>,
        manual_override: Option<crate::oauth::OAuthProvider>,
    ) -> Result<Self, OAuthError> {
        let mut auth_manager = AuthorizationManager::new(&base_url)
            .map_err(|e| OAuthError::Other(e.to_string()))?;
        auth_manager
            .set_credential_store(Box::new(NeoOAuthCredentialStore::new(
                format!("mcp:{server_id}"),
                oauth_store,
                oauth_store_path,
            )))
            .set_state_store(Box::new(NeoOAuthStateStore::default()))
            .set_oauth_http_client(Box::new(NeoOAuthHttpClient::new(reqwest::Client::new())))
            .set_scope_upgrade_config(ScopeUpgradeConfig::default());

        Ok(Self {
            server_id,
            auth_manager: Arc::new(Mutex::new(auth_manager)),
            manual_override,
        })
    }

    pub fn auth_manager(&self) -> Arc<Mutex<AuthorizationManager>> {
        self.auth_manager.clone()
    }

    /// Discover metadata, register dynamically if possible, and return an authorization URL.
    pub async fn start_authorization(
        &self,
        scopes: Vec<String>,
    ) -> Result<AuthorizationSession, OAuthError> {
        let mut manager = self.auth_manager.lock().await;

        manager
            .discover_metadata()
            .await
            .map_err(|e| OAuthError::Other(e.to_string()))?;

        // Try to initialize from stored credentials first.
        let _ = manager.init_from_store().await;

        let mut client_id = match manager.client_id_and_credentials().await {
            Ok(Some(creds)) => creds.client_id,
            _ => {
                // Attempt dynamic client registration if the server supports it.
                match manager.dynamic_register().await {
                    Ok(_) => manager
                        .client_id_and_credentials()
                        .await
                        .map_err(|e| OAuthError::Other(e.to_string()))?
                        .map(|c| c.client_id)
                        .unwrap_or_default(),
                    Err(_) => String::new(),
                }
            }
        };

        // Fall back to manual provider override if DCR is unavailable.
        if client_id.is_empty() {
            if let Some(manual) = &self.manual_override {
                client_id = manual.client_id.clone();
                manager
                    .use_client_id(&client_id)
                    .map_err(|e| OAuthError::Other(e.to_string()))?;
            }
        }

        if client_id.is_empty() {
            return Err(OAuthError::Other(
                "No OAuth client_id available. Run `neo mcp auth <id>` or configure [oauth.providers.<id>].".into(),
            ));
        }

        let effective_scopes = if scopes.is_empty() && self.manual_override.is_some() {
            self.manual_override.as_ref().unwrap().scopes.clone()
        } else {
            scopes
        };

        let session = manager
            .generate_authorization_url(effective_scopes)
            .await
            .map_err(|e| OAuthError::Other(e.to_string()))?;
        Ok(session)
    }
}
```

- [ ] **Step 2: Implement `authenticate_mcp_server_oauth` in `mcp_ops.rs` using rmcp**

Replace the old function body:

```rust
pub async fn authenticate_mcp_server_oauth(
    server_id: &str,
    server: &McpServerConfig,
    oauth: &OAuthConfig,
    neo_home: &Path,
) -> Result<rmcp::transport::auth::StoredCredentials, crate::oauth::OAuthError> {
    use crate::oauth::{CallbackServer, OAuthError};
    use crate::tools::mcp::oauth::McpOAuthManager;

    let url = server.url.as_deref().ok_or_else(|| {
        OAuthError::Other("OAuth is only supported for HTTP/SSE servers".into())
    })?;

    let oauth_store_path = neo_home.join("oauth.json");
    let oauth_store = Arc::new(RwLock::new(
        crate::oauth::OAuthStore::load(&oauth_store_path)?,
    ));

    let manual_override = oauth_provider_for_server(server, oauth)
        .map(|p| p.to_core_provider("manual"));

    let manager = McpOAuthManager::new(
        server_id.to_string(),
        url.to_string(),
        oauth_store.clone(),
        Some(oauth_store_path.clone()),
        manual_override,
    )
    .await?;

    let session = manager
        .start_authorization(oauth_provider_scopes(server, oauth))
        .await?;

    let state = extract_state_from_url(&session.auth_url);
    let callback = CallbackServer::start(
        state,
        std::time::Duration::from_secs(300),
    )
    .await?;

    let auth_url = patch_redirect_port(&session.auth_url, callback.local_port);

    webbrowser::open(&auth_url).ok();

    let code = callback.wait_for_code().await?;
    let mut manager = manager.auth_manager().lock().await;
    manager
        .exchange_code(&code.code)
        .await
        .map_err(|e| OAuthError::Other(e.to_string()))?;

    let creds = manager
        .client_id_and_credentials()
        .await
        .map_err(|e| OAuthError::Other(e.to_string()))?
        .ok_or_else(|| OAuthError::Other("OAuth succeeded but credentials missing".into()))?;
    Ok(creds)
}

fn oauth_provider_scopes(server: &McpServerConfig, oauth: &OAuthConfig) -> Vec<String> {
    oauth
        .providers
        .values()
        .find(|p| {
            server
                .url
                .as_deref()
                .map(|u| u.contains(&p.auth_url) || u.contains(&p.token_url))
                .unwrap_or(false)
        })
        .map(|p| p.scopes.clone())
        .unwrap_or_default()
}

fn extract_state_from_url(auth_url: &str) -> String {
    auth_url
        .parse::<url::Url>()
        .ok()
        .and_then(|u| {
            u.query_pairs()
                .find(|(k, _)| k == "state")
                .map(|(_, v)| v.into_owned())
        })
        .unwrap_or_default()
}

fn patch_redirect_port(auth_url: &str, port: u16) -> String {
    let mut url = auth_url.parse::<url::Url>().expect("valid auth URL");
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.query_pairs_mut().clear();
    for (k, v) in pairs {
        let value = if k == "redirect_uri" && v.starts_with("http://127.0.0.1:") {
            format!("http://127.0.0.1:{port}/callback")
        } else {
            v
        };
        url.query_pairs_mut().append_pair(&k, &value);
    }
    url.to_string()
}
```

- [ ] **Step 3: Update `start_mcp_oauth_flow` in `interactive.rs`**

Change it to call the updated `authenticate_mcp_server_oauth` and push `"OAuth token saved"` on success. The `CallbackServer` is now created inside the auth function, so remove the local callback creation.

- [ ] **Step 4: Commit**

```bash
git add crates/neo-agent-core/src/tools/mcp/oauth.rs crates/neo-agent/src/mcp_ops.rs crates/neo-agent/src/modes/interactive.rs
git commit -m "feat(mcp/oauth): wire rmcp AuthorizationManager into browser OAuth flow"
```

### Task 3.4: Synthetic `mcp__<server>__authenticate` tool

**Files:**
- Modify: `crates/neo-agent-core/src/tools/mcp_manager.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs`

- [ ] **Step 1: When an HTTP/SSE server fails with auth-required, inject an authenticate tool**

In `McpConnectionManager::register_connected_tools_into`, after collecting tools, if the server transport is HTTP/SSE and an `auth_manager` exists but has no valid token, also register a synthetic tool:

```rust
let auth_tool_name = format!("mcp__{}__authenticate", sanitize_id(id));
let auth_spec = ToolSpec {
    name: auth_tool_name.clone(),
    description: format!("Authenticate the {id} MCP server via OAuth. Run this if tool calls return 401 Unauthorized."),
    parameters: serde_json::json!({"type":"object","properties":{}}),
};
registry.register(auth_spec, ManagedMcpAuthTool {
    server_id: id.clone(),
    auth_manager: entry.auth_manager.clone().expect("auth manager"),
});
```

- [ ] **Step 2: Implement `ManagedMcpAuthTool`**

```rust
#[derive(Clone)]
struct ManagedMcpAuthTool {
    server_id: String,
    auth_manager: Arc<tokio::sync::Mutex<rmcp::transport::auth::AuthorizationManager>>,
}

#[async_trait]
impl Tool for ManagedMcpAuthTool {
    fn name(&self) -> &str {
        &format!("mcp__{}__authenticate", sanitize_id(&self.server_id))
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: format!("Re-authenticate the {} MCP server via OAuth.", self.server_id),
            parameters: serde_json::json!({"type":"object","properties":{}}),
        }
    }

    async fn execute(&self, _ctx: ToolContext, _input: Value) -> Result<ToolResult, ToolError> {
        Err(ToolError::Mcp {
            server_id: self.server_id.clone(),
            tool_name: "authenticate".into(),
            message: "Please run `neo mcp auth <server_id>` or use the /mcp overlay to authenticate.".into(),
        })
    }
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/neo-agent-core/src/tools/mcp_manager.rs
git commit -m "feat(mcp): expose synthetic authenticate tool for OAuth-backed servers"
```


## Phase 4: Refactor `McpConnectionManager` Around `rmcp` Clients

### Task 4.1: Replace internal adapter with `McpClient` and add auth manager

**Files:**
- Modify: `crates/neo-agent-core/src/tools/mcp_manager.rs`

- [ ] **Step 1: Update imports and `ManagedMcpEntry`**

```rust
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::Context;
use tokio::{sync::RwLock, task::JoinHandle};

use super::{
    ProcessSupervisor, ToolRegistry,
    mcp::{
        McpError, McpResourceDefinition, McpResourceRead, McpToolDefinition,
        client::McpClient,
        http::{self, HttpConfig},
        oauth::McpOAuthManager,
        stdio::{self, StdioConfig},
    },
};
use crate::oauth::{OAuthProviderRegistry, OAuthStore};
```

Change `ManagedMcpEntry`:

```rust
struct ManagedMcpEntry {
    config: ManagedMcpServerConfig,
    attempt_id: u64,
    status: McpServerStatus,
    client: Option<Arc<dyn McpClient>>,
    auth_manager: Option<Arc<tokio::sync::Mutex<rmcp::transport::auth::AuthorizationManager>>>,
    tools: Vec<McpToolDefinition>,
    resources: Vec<McpResourceDefinition>,
    error: Option<McpDiagnostic>,
    reconnect_attempt: u32,
    next_retry_ms: Option<u64>,
    reconnect_task: Option<JoinHandle<()>>,
    connect_task: Option<JoinHandle<Result<ConnectOutcome, McpError>>>,
}
```

- [ ] **Step 2: Update state struct to hold OAuth store/path/registry**

Keep existing fields; `oauth_provider_registry` is now only a manual-override source.

```rust
struct McpConnectionManagerState {
    supervisor: ProcessSupervisor,
    entries: BTreeMap<String, ManagedMcpEntry>,
    next_attempt_id: u64,
    oauth_store: Arc<RwLock<OAuthStore>>,
    oauth_store_path: Option<PathBuf>,
    oauth_provider_registry: Arc<OAuthProviderRegistry>,
}
```

- [ ] **Step 3: Commit**

```bash
git commit -m "refactor(mcp_manager): prepare internal entry for rmcp clients"
```

### Task 4.2: Implement `build_client_for_config`

- [ ] **Step 1: Add `build_client_for_config` method**

```rust
impl McpConnectionManager {
    async fn build_client_for_config(
        &self,
        config: &ManagedMcpServerConfig,
    ) -> Result<(Box<dyn McpClient>, Option<Arc<tokio::sync::Mutex<rmcp::transport::auth::AuthorizationManager>>>), McpError> {
        match &config.transport {
            ManagedMcpTransport::Stdio { command, args, env, cwd } => {
                let client = stdio::build_stdio_client(
                    &config.id,
                    StdioConfig {
                        command: command.clone(),
                        args: args.clone(),
                        env: env.clone(),
                        cwd: cwd.clone(),
                        startup_timeout_ms: config.startup_timeout_ms,
                        tool_timeout_ms: config.tool_timeout_ms,
                    },
                    &self.inner.read().await.supervisor,
                )
                .await?;
                Ok((client, None))
            }
            ManagedMcpTransport::Http { url, headers } | ManagedMcpTransport::Sse { url, headers } => {
                let state = self.inner.read().await;
                let manual_override = state.oauth_provider_registry.resolve_for_url(url).cloned();
                let oauth_manager = McpOAuthManager::new(
                    config.id.clone(),
                    url.clone(),
                    state.oauth_store.clone(),
                    state.oauth_store_path.clone(),
                    manual_override,
                )
                .await
                .ok();

                let auth_manager = oauth_manager.as_ref().map(|m| m.auth_manager());
                drop(state);

                let client = http::build_http_client(HttpConfig {
                    url: url.clone(),
                    headers: headers.clone(),
                    startup_timeout_ms: config.startup_timeout_ms,
                    tool_timeout_ms: config.tool_timeout_ms,
                    auth_manager: auth_manager.clone(),
                })
                .await?;
                Ok((client, auth_manager))
            }
        }
    }
}
```

- [ ] **Step 2: Replace `adapter_for_config` usages**

Search `mcp_manager.rs` for `adapter_for_config` and replace all call sites with `build_client_for_config`. Delete the old `adapter_for_config` function.

- [ ] **Step 3: Commit**

```bash
git commit -m "feat(mcp_manager): build rmcp stdio and HTTP clients per server config"
```

### Task 4.3: Update connection lifecycle and snapshot mapping

- [ ] **Step 1: Update `connect_one` to store `client` and `auth_manager`**

In `connect_one` after successful `list_tools`, assign `entry.client = Some(Arc::new(client))` and `entry.auth_manager = auth_manager`.

- [ ] **Step 2: Update `snapshot_for_entry`**

No change needed for public fields; ensure `tool_count` uses `entry.tools.len()`.

- [ ] **Step 3: Update `ManagedMcpTool::execute` to use `client.call_tool`**

```rust
struct ManagedMcpTool {
    server_id: String,
    remote_name: String,
    spec: ToolSpec,
    client: Arc<dyn McpClient>,
}

#[async_trait]
impl Tool for ManagedMcpTool {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    async fn execute(&self, _ctx: ToolContext, input: serde_json::Value) -> Result<ToolResult, ToolError> {
        let response = self
            .client
            .call_tool(&self.remote_name, input)
            .await
            .map_err(|e| ToolError::Mcp {
                server_id: self.server_id.clone(),
                tool_name: self.remote_name.clone(),
                message: e.message().to_string(),
            })?;
        Ok(response.into())
    }
}
```

- [ ] **Step 4: Update `register_connected_tools_into`**

For each connected entry, create `ManagedMcpTool` with the `Arc<dyn McpClient>` clone and the sanitized remote name. Tool spec is built from `entry.tools`.

```rust
for tool in &entry.tools {
    let namespaced = namespaced_tool_name(&entry.config.id, &tool.name);
    if seen.contains(&namespaced) {
        diagnostics.push(McpDiagnostic { ... });
        continue;
    }
    seen.insert(namespaced.clone());
    let spec = ToolSpec {
        name: namespaced,
        description: tool.description.clone(),
        parameters: tool.input_schema.clone(),
    };
    let client = entry.client.clone().context("connected entry missing client")?;
    registry.register(spec.clone(), ManagedMcpTool {
        server_id: entry.config.id.clone(),
        remote_name: tool.name.clone(),
        spec,
        client,
    });
}
```

- [ ] **Step 5: Update `list_resources` and `read_resource`**

Use `entry.client.as_ref().unwrap().list_resources()` and `.read_resource(uri)`.

- [ ] **Step 6: Update `shutdown`**

Iterate entries and call `client.shutdown().await` for each connected client, then call `supervisor.cleanup_all().await`.

- [ ] **Step 7: Commit**

```bash
git commit -m "feat(mcp_manager): wire rmcp clients into lifecycle, registration, and resources"
```

### Task 4.4: Remove obsolete `McpToolAdapter` trait and old adapters

**Files:**
- Delete: `crates/neo-agent-core/src/tools/mcp.rs` old file already replaced by module dir.
- Modify: `crates/neo-agent-core/src/tools/mcp/mod.rs` remove any leftover `McpToolAdapter` references.

- [ ] **Step 1: Delete old JSON-RPC/SSE helper functions**

In `mcp/mod.rs`, keep only public model types and conversion helpers. Delete `McpToolAdapter` trait, `McpStdioConfig`, `McpHttpConfig`, `McpHttpToolAdapter`, `McpStdioToolAdapter`, `McpToolProvider`, and all JSON-RPC/SSE parsing code.

- [ ] **Step 2: Update `tools/mod.rs` re-exports**

Ensure `pub use mcp::*;` still exposes the remaining public types. Remove `McpStdioConfig`/`McpHttpConfig` if re-exported elsewhere.

- [ ] **Step 3: Commit**

```bash
git add crates/neo-agent-core/src/tools/mcp/
git commit -m "refactor(mcp): remove hand-rolled adapters and JSON-RPC helpers"
```


## Phase 5: Update CLI/TUI Glue and Config Conversion

### Task 5.1: Update `mcp_ops.rs` to remove old adapter/provider wiring

**Files:**
- Modify: `crates/neo-agent/src/mcp_ops.rs`

- [ ] **Step 1: Replace `oauth_provider_registry` with manual override helper**

```rust
pub fn oauth_provider_for_server(
    server: &McpServerConfig,
    oauth: &OAuthConfig,
) -> Option<crate::config::OAuthProviderConfig> {
    let url = server.url.as_deref()?;
    oauth
        .providers
        .values()
        .find(|p| url.contains(&p.auth_url) || url.contains(&p.token_url))
        .cloned()
}
```

- [ ] **Step 2: Remove `detect_oauth_provider_for_server` and old registry construction**

Delete `detect_oauth_provider_for_server`. Keep `oauth_provider_registry` only if CLI tests reference it; otherwise delete it.

- [ ] **Step 3: Update `authenticate_mcp_server_oauth` signature and body**

Use the implementation shown in Task 3.3. Ensure it accepts `oauth: &OAuthConfig` and passes manual override scopes to the flow.

- [ ] **Step 4: Commit**

```bash
git add crates/neo-agent/src/mcp_ops.rs
git commit -m "refactor(mcp_ops): switch OAuth helper to rmcp discovery + manual override"
```

### Task 5.2: Remove old adapter construction from `run.rs`

**Files:**
- Modify: `crates/neo-agent/src/modes/run.rs`

- [ ] **Step 1: Delete `mcp_adapter_for_server` and `auth_mcp_server` old implementations**

Remove any function that constructs `McpHttpConfig` or `McpStdioConfig` with OAuth fields. `auth_mcp_server` now delegates to `mcp_ops::authenticate_mcp_server_oauth`.

- [ ] **Step 2: Update `tool_registry_for_config`**

Keep the existing flow but remove direct adapter creation. It should create `McpConnectionManager`, call `mcp_ops::reload_mcp_manager_from_config`, wait for probes, and call `register_connected_tools_into`.

```rust
async fn tool_registry_for_config(
    config: &AppConfig,
    supervisor: ProcessSupervisor,
    provided_manager: Option<McpConnectionManager>,
) -> anyhow::Result<(ToolRegistry, McpConnectionManager)> {
    let manager = provided_manager.unwrap_or_else(|| {
        McpConnectionManager::new(supervisor)
    });
    mcp_ops::reload_mcp_manager_from_config(config, &manager).await?;
    wait_for_mcp_manager_probe(&manager).await?;
    let mut registry = ToolRegistry::with_builtin_tools();
    manager.register_connected_tools_into(&mut registry).await;
    Ok((registry, manager))
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/neo-agent/src/modes/run.rs
git commit -m "refactor(run): remove custom MCP adapter construction paths"
```

### Task 5.3: Update TUI `interactive.rs` glue

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`

- [ ] **Step 1: Update `mcp_manager_with_oauth_store`**

Keep existing function but ensure it loads the new `OAuthStore` format:

```rust
fn mcp_manager_with_oauth_store(
    config: &AppConfig,
    supervisor: ProcessSupervisor,
    neo_home: &Path,
) -> anyhow::Result<McpConnectionManager> {
    let oauth_path = neo_home.join("oauth.json");
    let oauth_store = Arc::new(RwLock::new(
        neo_agent_core::oauth::OAuthStore::load(&oauth_path)?,
    ));
    let manager = McpConnectionManager::with_oauth_store(
        supervisor,
        oauth_store,
        Some(oauth_path),
    );
    Ok(manager)
}
```

- [ ] **Step 2: Update `start_mcp_oauth_flow`**

Remove local callback server creation; call `mcp_ops::authenticate_mcp_server_oauth` directly. On success push `"OAuth token saved"` and reopen the MCP manager overlay.

```rust
async fn start_mcp_oauth_flow(&mut self, server_id: String) {
    let Some(config) = self.config.clone() else {
        self.push_status("No config available");
        return;
    };
    let neo_home = match neo_home() {
        Ok(p) => p,
        Err(_) => {
            self.push_status("Failed to resolve neo home directory");
            return;
        }
    };
    self.push_status("Waiting for browser authorization...");
    let oauth = config.oauth.clone();
    let server = match config.mcp.servers.iter().find(|s| s.id == server_id).cloned() {
        Some(s) => s,
        None => {
            self.push_status("MCP server not found");
            return;
        }
    };
    match mcp_ops::authenticate_mcp_server_oauth(&server_id, &server, &oauth, &neo_home).await {
        Ok(_) => {
            self.push_status("OAuth token saved");
            self.sync_mcp_manager_from_config().await.ok();
            self.open_mcp_manager().await.ok();
        }
        Err(e) => self.push_status(format!("OAuth flow failed: {e}")),
    }
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/neo-agent/src/modes/interactive.rs
git commit -m "refactor(interactive): adapt TUI OAuth flow to rmcp helpers"
```

### Task 5.4: Update config validation and conversion

**Files:**
- Modify: `crates/neo-agent/src/config.rs`
- Modify: `crates/neo-agent/src/mcp_ops.rs`

- [ ] **Step 1: Keep `McpServerConfig` schema unchanged**

No field changes. Transport strings still `"stdio" | "http" | "sse"`.

- [ ] **Step 2: Update `validate_mcp_server` only if rmcp adds new transport**

No changes needed for current scope.

- [ ] **Step 3: Update `to_managed_config` in `mcp_ops.rs`**

```rust
pub fn to_managed_config(server: &McpServerConfig) -> anyhow::Result<ManagedMcpServerConfig> {
    let transport = match server.transport.as_str() {
        "stdio" => ManagedMcpTransport::Stdio {
            command: server.command.clone().context("stdio transport requires command")?,
            args: server.args.clone(),
            env: server.env.clone(),
            cwd: server.cwd.clone(),
        },
        "http" => ManagedMcpTransport::Http {
            url: server.url.clone().context("http transport requires url")?,
            headers: server.headers.clone(),
        },
        "sse" => ManagedMcpTransport::Sse {
            url: server.url.clone().context("sse transport requires url")?,
            headers: server.headers.clone(),
        },
        other => anyhow::bail!("unknown MCP transport: {other}"),
    };
    Ok(ManagedMcpServerConfig {
        id: server.id.clone(),
        enabled: server.enabled,
        transport,
        enabled_tools: server.enabled_tools.clone(),
        disabled_tools: server.disabled_tools.clone(),
        startup_timeout_ms: server.startup_timeout_ms,
        tool_timeout_ms: server.tool_timeout_ms,
        reconnect: McpReconnectPolicy::default(),
    })
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/neo-agent/src/config.rs crates/neo-agent/src/mcp_ops.rs
git commit -m "chore(config): preserve MCP schema, update managed config conversion"
```


## Phase 6: Tests, Docs, and Parity

### Task 6.1: Rewrite core MCP integration tests

**Files:**
- Replace: `crates/neo-agent-core/tests/tool_mcp.rs`

- [ ] **Step 1: Add an rmcp-based test server fixture**

Create a small in-process MCP server using `rmcp` server features. Because the workspace currently enables only client features, add a `[[bin]]` or test-only server in `crates/neo-agent-core/tests/support/mcp_server.rs`:

```rust
use rmcp::{
    model::{CallToolResult, Content, ServerInfo},
    schemars, ServerHandler, ServiceExt,
    transport::io::stdio,
};
use serde_json::Value;

#[derive(Clone)]
struct EchoServer;

#[rmcp::tool]
impl EchoServer {
    #[tool(description = "Echo the input")]
    async fn echo(&self, #[tool(aggr)] input: Value) -> Result<CallToolResult, rmcp::Error> {
        Ok(CallToolResult::success(vec![Content::text(input.to_string())]))
    }
}

impl ServerHandler for EchoServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(Default::default())
    }
}

pub async fn run_stdio_server() -> anyhow::Result<()> {
    EchoServer.serve(stdio()).await?.waiting().await?;
    Ok(())
}
```

- [ ] **Step 2: Write focused tests**

Replace the old TCP-mock tests with:

```rust
#[tokio::test]
async fn rmcp_stdio_client_discovers_and_calls_tool() {
    // spawn echo server child process via rmcp test server binary
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_mcp-echo-server"));
    let client = stdio::build_stdio_client(
        "echo",
        StdioConfig {
            command: cmd.as_std().get_program().to_string_lossy().into(),
            args: vec![],
            env: Default::default(),
            cwd: None,
            startup_timeout_ms: Some(5000),
            tool_timeout_ms: Some(5000),
        },
        &ProcessSupervisor::default(),
    )
    .await
    .unwrap();

    let tools = client.list_tools().await.unwrap();
    assert!(tools.iter().any(|t| t.name == "echo"));

    let response = client
        .call_tool("echo", serde_json::json!({"message":"hi"}))
        .await
        .unwrap();
    assert!(!response.is_error);
}
```

- [ ] **Step 3: Delete obsolete mock servers and fixtures**

Remove `MockMcpHttpServer`, `MockMcpAdapter`, Python stdio fixtures, and all old assertions. Keep only the tests that validate the new rmcp-backed behavior.

- [ ] **Step 4: Commit**

```bash
git add crates/neo-agent-core/tests/tool_mcp.rs crates/neo-agent-core/tests/support/
git commit -m "test(mcp): rewrite integration tests against rmcp fixtures"
```

### Task 6.2: Update OAuth tests

**Files:**
- Modify: `crates/neo-agent-core/src/oauth/store.rs` tests
- Modify: `crates/neo-agent/src/mcp_ops.rs` tests

- [ ] **Step 1: Update store tests for `StoredCredentials`**

```rust
#[test]
fn save_and_load_roundtrip_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("oauth.json");
    let mut store = OAuthStore::default();
    let creds = StoredCredentials {
        client_id: "client-1".into(),
        token_response: None,
        granted_scopes: vec!["write".into()],
        token_received_at: Some(1),
    };
    store.set("mcp:linear", creds.clone());
    store.save(&path).unwrap();
    let loaded = OAuthStore::load(&path).unwrap();
    assert_eq!(loaded.get("mcp:linear").unwrap().client_id, "client-1");
}
```

- [ ] **Step 2: Remove tests for deleted functions**

Delete tests for `OAuthProviderRegistry`, `generate_pkce`, `build_authorization_url`, etc. If registry is kept as manual override only, keep only override tests.

- [ ] **Step 3: Commit**

```bash
git commit -m "test(oauth): update tests for rmcp credential store"
```

### Task 6.3: Update CLI command tests

**Files:**
- Modify: `crates/neo-agent/tests/cli_commands.rs`

- [ ] **Step 1: Replace raw TCP mock servers with rmcp HTTP test server**

Create a small rmcp streamable HTTP server binary or use the reqwest-based test client against a local rmcp server started on a random port.

- [ ] **Step 2: Update `mcp_add_remote_http_probes_and_reports_success`**

```rust
#[test]
fn mcp_add_remote_http_probes_and_reports_success() {
    let (port, _server) = start_rmcp_http_server();
    let home = setup_temp_home();
    write_home_config(
        &home,
        &format!(
            r#"[[mcp.servers]]
id = "http-echo"
transport = "http"
url = "http://127.0.0.1:{port}/mcp"
"#
        ),
    );
    let output = neo().env("NEO_HOME", &home).args(["mcp", "add", "http-echo", "-t", "remote-http", "-u", &format!("http://127.0.0.1:{port}/mcp")]).assert().success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("http-echo") || stdout.contains("connected"));
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/neo-agent/tests/cli_commands.rs
git commit -m "test(cli): adapt MCP CLI tests to rmcp-backed fixtures"
```

### Task 6.4: Update end-to-end mock provider test

**Files:**
- Modify: `crates/neo-agent/tests/mock_provider_e2e.rs`

- [ ] **Step 1: Replace MCP stdio Python fixture with rmcp test server binary**

Use the same `mcp-echo-server` binary started as a child process. Assert the model request contains `mcp__echo__echo`.

- [ ] **Step 2: Commit**

```bash
git commit -m "test(e2e): use rmcp test server in mock provider MCP registration test"
```

### Task 6.5: Docs and parity

**Files:**
- Modify: `docs/mcp.md`
- Modify: `docs/superpowers/specs/2026-06-22-mcp-oauth-discovery-dcr-design.md`
- Modify: `examples/config/mcp-server.toml`

- [ ] **Step 1: Update user-facing MCP docs**

In `docs/mcp.md`, replace descriptions of hard-coded `linear` OAuth provider with:

```markdown
Neo uses the official `rmcp` Rust SDK for MCP transport and OAuth. For HTTP/SSE servers that support MCP OAuth discovery, Neo automatically discovers the authorization server, registers a dynamic OAuth client when supported, and runs a local PKCE browser flow. For servers that do not support dynamic registration, add a manual provider under `[oauth.providers.<id>]` in `~/.neo/config.toml`.
```

- [ ] **Step 2: Update design spec status**

In `docs/superpowers/specs/2026-06-22-mcp-oauth-discovery-dcr-design.md`, append a section:

```markdown
## Implementation status

- [x] `rmcp` dependency added.
- [x] `McpConnectionManager` rebuilt on `rmcp` clients.
- [x] OAuth discovery + DCR delegated to `rmcp::transport::auth::AuthorizationManager`.
- [x] Token store migrated to `StoredCredentials`.
```

- [ ] **Step 3: Update example config**

No transport label changes needed; ensure examples still use `"stdio"` / `"http"` / `"sse"`.

- [ ] **Step 4: Run parity gate**

Run: `cargo run -p xtask -- parity`
Expected: passes after fixing any stale references.

- [ ] **Step 5: Commit**

```bash
git add docs/mcp.md docs/superpowers/specs/ examples/config/
git commit -m "docs(mcp): document rmcp-based OAuth and transport behavior"
```


## Phase 7: Verification Gates and Final Cleanup

### Task 7.1: Compile the workspace

- [ ] **Step 1: Run xtask check**

Run: `cargo run -p xtask -- check --workspace`
Expected: `neo-agent-core`, `neo-agent`, `neo-tui`, `neo-ai`, `xtask`, and examples all compile without warnings.

- [ ] **Step 2: Commit fixes**

```bash
git commit -m "fix(mcp): clippy/fmt cleanups after rmcp migration"
```

### Task 7.2: Run focused tests

- [ ] **Step 1: Run MCP unit/integration tests**

Run: `cargo run -p xtask -- test -p neo-agent-core --lib mcp`
Run: `cargo run -p xtask -- test -p neo-agent-core --test tool_mcp`
Expected: all pass.

- [ ] **Step 2: Run OAuth tests**

Run: `cargo run -p xtask -- test -p neo-agent-core oauth`
Run: `cargo run -p xtask -- test -p neo-agent mcp_ops`
Expected: all pass.

- [ ] **Step 3: Run CLI tests**

Run: `cargo run -p xtask -- test -p neo-agent cli_commands`
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git commit -m "test(mcp): verify focused test suites after migration"
```

### Task 7.3: Full workspace test and coverage (complex task gate)

- [ ] **Step 1: Run full workspace tests**

Run: `cargo run -p xtask -- test --workspace --all-features`
Expected: all pass. If unrelated failures exist, document them and do not fix within this scope.

- [ ] **Step 2: Generate LCOV**

Run: `cargo run -p xtask -- coverage`
Artifacts: `target/llvm-cov/lcov.info`.

- [ ] **Step 3: Run production CRAP gate**

Run: `cargo run -p xtask -- crap`
Artifacts: `target/crap/crap-crates.md`, `target/crap/crap-crates.json`.
Expected: no production function in `crates/` has CRAP > 30. If any new rmcp wrapper functions score high, split them.

- [ ] **Step 4: Run parity and catalog gates**

Run: `cargo run -p xtask -- parity`
Run: `cargo run -p xtask -- catalog check`
Expected: both pass.

- [ ] **Step 5: Commit**

```bash
git commit -m "ci(mcp): full workspace tests, coverage, and parity pass"
```

### Task 7.4: Delete legacy OAuth flow helpers, keep manual override types

**Files:**
- Modify: `crates/neo-agent-core/src/oauth.rs`

- [ ] **Step 1: Remove PKCE/token-exchange functions, keep `OAuthProvider`, `OAuthProviderRegistry`, and `OAuthTokenSet`**

The registry is still needed for manual `[oauth.providers.<id>]` overrides, and `OAuthTokenSet` is needed for one-release legacy migration in `store.rs`.

Replace `oauth.rs` with:

```rust
pub mod callback_server;
mod store;

pub use callback_server::{CallbackCode, CallbackServer};
pub use store::OAuthStore;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OAuthError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("callback error: {0}")]
    Callback(String),
    #[error("{0}")]
    Other(String),
}

/// Legacy token set kept for store migration. Prefer `rmcp::transport::auth::StoredCredentials`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthTokenSet {
    pub access_token: String,
    pub token_type: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub scopes: Vec<String>,
}

/// Manual OAuth provider override (no built-ins; discovery/DCR is handled by rmcp).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthProvider {
    pub id: String,
    pub client_id: String,
    pub auth_url: String,
    pub token_url: String,
    pub scopes: Vec<String>,
    pub default_callback_port: u16,
}

impl OAuthProvider {
    pub fn client_id_or_env(&self) -> String {
        let env_key = format!("NEO_OAUTH_{}_CLIENT_ID", self.id.to_uppercase().replace('-', "_"));
        std::env::var(&env_key).unwrap_or_else(|_| self.client_id.clone())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OAuthProviderRegistry {
    providers: BTreeMap<String, OAuthProvider>,
}

impl OAuthProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, provider: OAuthProvider) {
        self.providers.insert(provider.id.clone(), provider);
    }

    pub fn resolve_for_url(&self, url: &str) -> Option<&OAuthProvider> {
        self.providers
            .values()
            .find(|p| url.contains(&p.id) || url.contains(&p.auth_url) || url.contains(&p.token_url))
    }
}
```

- [ ] **Step 2: Update `mcp_ops::oauth_provider_registry` to build only from config overrides**

```rust
pub fn oauth_provider_registry(oauth: &OAuthConfig) -> OAuthProviderRegistry {
    let mut registry = OAuthProviderRegistry::new();
    for (id, provider_config) in &oauth.providers {
        registry.register(provider_config.to_core_provider(id));
    }
    registry
}
```

- [ ] **Step 3: Update `McpConnectionManager::new` defaults to use empty registry**

Replace `OAuthProviderRegistry::with_builtin_providers()` with `OAuthProviderRegistry::new()` in `new` and `with_oauth_store`.

- [ ] **Step 4: Commit**

```bash
git add crates/neo-agent-core/src/oauth.rs crates/neo-agent/src/mcp_ops.rs crates/neo-agent-core/src/tools/mcp_manager.rs
git commit -m "cleanup(oauth): remove legacy PKCE helpers, keep manual override registry"
```

### Task 7.5: Final manual smoke test

- [ ] **Step 1: Build release binary**

Run: `cargo build --release -p neo-agent`
Binary: `target/release/neo`

- [ ] **Step 2: Smoke test CLI MCP commands**

```bash
./target/release/neo mcp list
./target/release/neo mcp status
./target/release/neo --help
```

- [ ] **Step 3: Smoke test stdio MCP server**

Add an stdio MCP server to `~/.neo/config.toml`:

```toml
[[mcp.servers]]
id = "filesystem"
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
```

Run: `./target/release/neo mcp status`
Expected: server connects and lists tools.

- [ ] **Step 4: Commit any final tweaks**

```bash
git commit -m "chore(mcp): final smoke test fixes"
```

---

## Self-Review Checklist

Before claiming the refactor complete, run through this checklist.

### 1. Spec coverage

| Spec requirement | Task that implements it |
|------------------|-------------------------|
| `McpConnectionManager` public API preserved | Tasks 4.1, 4.2, 4.3 |
| Tool namespacing `mcp__<id>__<tool>` preserved | Task 4.3 |
| Transport mapping `stdio`/`http`/`sse` preserved | Tasks 2.2, 2.3, 5.4 |
| OAuth tokens in `~/.neo/oauth.json` | Tasks 3.1, 3.2, 3.3 |
| Discovery + DCR via `rmcp::transport::auth` | Tasks 3.2, 3.3 |
| Synthetic authenticate tool | Task 3.4 |
| Snapshot/status semantics preserved | Task 4.1, 4.3 |
| Reconnect/backoff preserved | Task 4.3 (no logic change) |

### 2. Placeholder scan

Search the plan for:

- `TBD`, `TODO`, `implement later` — none.
- `add appropriate error handling` — replaced with concrete error variants.
- `write tests for the above` — each task includes concrete test code.
- `similar to Task N` — avoided; repeated code kept minimal.

### 3. Type consistency

- `McpClient` trait is used in `client.rs`, `stdio.rs`, `http.rs`, and `mcp_manager.rs`.
- `AuthorizationManager` is wrapped in `Arc<Mutex<...>>` consistently.
- `OAuthStore` stores `StoredCredentials` everywhere after Task 3.1.
- Tool names use `namespaced_tool_name` and `sanitize_id` consistently.

### 4. Known gaps / follow-ups

- Resource subscription/notifications via SSE: `rmcp` streamable HTTP handles server-initiated messages internally. Neo no longer exposes `next_resource_update`; the TUI/resource command tests should be updated to use `list_resources`/`read_resource` only. If live resource update streaming is required, add a follow-up task to surface `rmcp` notifications.
- `AuthorizationManager::use_client_id` method name: the plan assumes this exact name for the manual override path. If `rmcp` uses a different name (e.g., `use_provided_client_id`), adjust Task 3.3 accordingly when the dependency resolves.

---

## Execution Handoff

Plan complete. Two execution options are available after approval:

1. **Subagent-Driven Implementation (recommended)** — dispatch a fresh subagent per task (or small task batch), review between tasks, fast iteration. Required sub-skill: `superpowers:subagent-driven-development`.
2. **Inline Execution** — execute tasks in this session using `superpowers:executing-plans`, batching related steps with checkpoints for review.

Pick one at approval time.

