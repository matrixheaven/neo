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
        HttpConfig, McpClient, McpError, McpResourceDefinition, McpResourceRead, McpToolDefinition,
        StdioConfig, http,
        http::HttpOAuthConfig,
        oauth::{McpOAuthIdentity, McpOAuthService, McpOAuthServiceConfig, McpOAuthTransportKind},
        stdio,
    },
};

/// Runtime configuration for an MCP server managed by [`McpConnectionManager`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedMcpServerConfig {
    pub id: String,
    pub enabled: bool,
    pub transport: ManagedMcpTransport,
    pub enabled_tools: Vec<String>,
    pub disabled_tools: Vec<String>,
    pub startup_timeout_ms: Option<u64>,
    pub tool_timeout_ms: Option<u64>,
    pub reconnect: McpReconnectPolicy,
}

/// Transport-specific configuration for a managed MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedMcpTransport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<PathBuf>,
    },
    Http {
        url: String,
        headers: BTreeMap<String, String>,
    },
    Sse {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

impl ManagedMcpTransport {
    /// User-facing transport label.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Stdio { .. } => "stdio",
            Self::Http { .. } => "http",
            Self::Sse { .. } => "sse",
        }
    }
}

/// Retry policy for a managed MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpReconnectPolicy {
    pub enabled: bool,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub max_attempts: Option<u32>,
}

impl Default for McpReconnectPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            initial_delay_ms: 500,
            max_delay_ms: 30_000,
            max_attempts: Some(5),
        }
    }
}

/// Lifecycle status of a managed MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerStatus {
    Disabled,
    Pending,
    Connected,
    NeedsAuth,
    Failed,
    Reconnecting,
}

impl McpServerStatus {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Pending => "pending",
            Self::Connected => "connected",
            Self::NeedsAuth => "needs_auth",
            Self::Failed => "failed",
            Self::Reconnecting => "reconnecting",
        }
    }
}

/// Human-readable diagnostic for a failed MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpDiagnostic {
    pub server_id: String,
    pub transport: String,
    pub message: String,
    pub hint: Option<String>,
    pub stderr_tail: Option<String>,
}

/// Snapshot of a managed MCP server suitable for TUI/CLI rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSnapshot {
    pub id: String,
    pub transport: String,
    pub status: McpServerStatus,
    pub tool_count: usize,
    pub tool_names: Vec<String>,
    pub resource_count: Option<usize>,
    pub error: Option<McpDiagnostic>,
    pub reconnect_attempt: u32,
    pub next_retry_ms: Option<u64>,
}

/// Entry in a resource list returned by [`McpConnectionManager::list_resources`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResourceListEntry {
    pub server_id: String,
    pub uri: String,
    pub name: String,
    pub mime_type: Option<String>,
}

struct ManagedMcpEntry {
    config: ManagedMcpServerConfig,
    attempt_id: u64,
    status: McpServerStatus,
    client: Option<Arc<dyn McpClient>>,
    oauth_identity: Option<McpOAuthIdentity>,
    tools: Vec<McpToolDefinition>,
    resources: Vec<McpResourceDefinition>,
    error: Option<McpDiagnostic>,
    reconnect_attempt: u32,
    next_retry_ms: Option<u64>,
    reconnect_task: Option<JoinHandle<Result<ConnectOutcome, McpError>>>,
    connect_task: Option<JoinHandle<Result<ConnectOutcome, McpError>>>,
}

struct McpConnectionManagerState {
    supervisor: ProcessSupervisor,
    entries: BTreeMap<String, ManagedMcpEntry>,
    next_attempt_id: u64,
    oauth_service: McpOAuthService,
}

/// Owns configured MCP server state and exposes snapshots, resource operations,
/// and model-visible MCP tools.
#[derive(Clone)]
pub struct McpConnectionManager {
    inner: Arc<RwLock<McpConnectionManagerState>>,
}

impl McpConnectionManager {
    #[must_use]
    pub fn new(supervisor: ProcessSupervisor) -> Self {
        Self {
            inner: Arc::new(RwLock::new(McpConnectionManagerState {
                supervisor,
                entries: BTreeMap::new(),
                next_attempt_id: 1,
                oauth_service: McpOAuthService::new(McpOAuthServiceConfig { neo_home: None }),
            })),
        }
    }

    #[must_use]
    pub fn with_oauth_service(
        supervisor: ProcessSupervisor,
        oauth_service: McpOAuthService,
    ) -> Self {
        Self {
            inner: Arc::new(RwLock::new(McpConnectionManagerState {
                supervisor,
                entries: BTreeMap::new(),
                next_attempt_id: 1,
                oauth_service,
            })),
        }
    }

    /// Replace the OAuth service used for managed HTTP/SSE adapters.
    pub async fn set_oauth_service(&self, oauth_service: McpOAuthService) {
        let mut state = self.inner.write().await;
        state.oauth_service = oauth_service;
    }

    /// Apply a new set of server configurations. Removed servers are shut down,
    /// new servers are connected, and changed servers are reconnected.
    pub async fn apply_config(
        &self,
        servers: Vec<ManagedMcpServerConfig>,
    ) -> Vec<McpServerSnapshot> {
        let mut state = self.inner.write().await;

        // Remove entries no longer present.
        let new_ids: BTreeSet<String> = servers.iter().map(|s| s.id.clone()).collect();
        state.entries.retain(|_id, entry| {
            if new_ids.contains(&entry.config.id) {
                return true;
            }
            abort_tasks(entry);
            false
        });

        for server in servers {
            let attempt_id = state.next_attempt_id;
            state.next_attempt_id += 1;

            let existing = state.entries.remove(&server.id);
            let mut entry = if let Some(mut existing) = existing {
                if existing.config == server {
                    // Config unchanged: restore and keep it.
                    state.entries.insert(server.id.clone(), existing);
                    continue;
                }
                abort_tasks(&mut existing);
                existing.config = server.clone();
                existing.attempt_id = attempt_id;
                existing.status = McpServerStatus::Pending;
                existing.client = None;
                existing.oauth_identity = None;
                existing.tools.clear();
                existing.resources.clear();
                existing.error = None;
                existing.reconnect_attempt = 0;
                existing.next_retry_ms = None;
                existing
            } else {
                let status = if server.enabled {
                    McpServerStatus::Pending
                } else {
                    McpServerStatus::Disabled
                };
                ManagedMcpEntry {
                    config: server.clone(),
                    attempt_id,
                    status,
                    client: None,
                    oauth_identity: None,
                    tools: Vec::new(),
                    resources: Vec::new(),
                    error: None,
                    reconnect_attempt: 0,
                    next_retry_ms: None,
                    reconnect_task: None,
                    connect_task: None,
                }
            };

            if server.enabled {
                let oauth_service = state.oauth_service.clone();
                let handle = spawn_connect(server.clone(), state.supervisor.clone(), oauth_service);
                entry.connect_task = Some(handle);
            } else {
                entry.status = McpServerStatus::Disabled;
            }
            state.entries.insert(server.id.clone(), entry);
        }

        state.entries.values().map(snapshot_for_entry).collect()
    }

    /// Add or update a single server.
    pub async fn upsert_server(&self, server: ManagedMcpServerConfig) -> McpServerSnapshot {
        let mut servers = {
            let state = self.inner.read().await;
            state
                .entries
                .values()
                .map(|entry| entry.config.clone())
                .collect::<Vec<_>>()
        };
        if let Some(existing) = servers.iter_mut().find(|existing| existing.id == server.id) {
            *existing = server.clone();
        } else {
            servers.push(server.clone());
        }
        self.apply_config(servers).await;
        self.snapshot(&server.id)
            .await
            .expect("upserted MCP server should have a snapshot")
    }

    /// Remove a server. Returns `true` if it existed.
    pub async fn remove_server(&self, id: &str) -> bool {
        let mut state = self.inner.write().await;
        let Some(mut entry) = state.entries.remove(id) else {
            return false;
        };
        abort_tasks(&mut entry);
        true
    }

    /// Force an immediate reconnect for the given server.
    pub async fn reconnect_now(&self, id: &str) -> anyhow::Result<McpServerSnapshot> {
        let (config, supervisor, oauth_service) = {
            let mut state = self.inner.write().await;
            let Some(mut entry) = state.entries.remove(id) else {
                anyhow::bail!("MCP server '{id}' not found");
            };
            if !entry.config.enabled {
                state.entries.insert(id.to_owned(), entry);
                anyhow::bail!("MCP server '{id}' is disabled");
            }
            abort_tasks(&mut entry);
            let attempt_id = state.next_attempt_id;
            state.next_attempt_id += 1;
            entry.attempt_id = attempt_id;
            entry.status = McpServerStatus::Pending;
            entry.client = None;
            entry.oauth_identity = None;
            entry.tools.clear();
            entry.resources.clear();
            entry.error = None;
            entry.reconnect_attempt = 0;
            entry.next_retry_ms = None;
            let supervisor = state.supervisor.clone();
            let oauth_service = state.oauth_service.clone();
            let config = entry.config.clone();
            state.entries.insert(id.to_owned(), entry);
            (config, supervisor, oauth_service)
        };

        let handle = spawn_connect(config.clone(), supervisor, oauth_service);
        {
            let mut state = self.inner.write().await;
            if let Some(entry) = state.entries.get_mut(id) {
                entry.connect_task = Some(handle);
            }
        }

        // Wait briefly for a fast connection; otherwise return pending snapshot.
        let timeout = config.startup_timeout_ms.unwrap_or(5_000);
        tokio::time::sleep(Duration::from_millis(timeout.min(2_000))).await;
        self.snapshot(id)
            .await
            .context("MCP server '{id}' disappeared during reconnect")
    }

    /// Refresh the tool list for a connected server.
    pub async fn refresh_tools(&self, id: &str) -> anyhow::Result<McpServerSnapshot> {
        let (client, config) = {
            let mut state = self.inner.write().await;
            let Some(entry) = state.entries.get_mut(id) else {
                anyhow::bail!("MCP server '{id}' not found");
            };
            let Some(client) = entry.client.clone() else {
                anyhow::bail!("MCP server '{id}' is not connected");
            };
            entry.status = McpServerStatus::Pending;
            (client, entry.config.clone())
        };

        let result = discover_tools(&client, &config).await;

        let (snapshot, need_reconnect) = {
            let mut state = self.inner.write().await;
            let Some(entry) = state.entries.get_mut(id) else {
                anyhow::bail!("MCP server '{id}' disappeared during refresh");
            };
            let need_reconnect = match result {
                Ok((tools, resources)) => {
                    entry.status = McpServerStatus::Connected;
                    entry.tools = tools;
                    entry.resources = resources;
                    entry.error = None;
                    entry.reconnect_attempt = 0;
                    entry.next_retry_ms = None;
                    false
                }
                Err(err) => {
                    let diagnostic = diagnostic_from_error(&err, &entry.config, None);
                    if err.is_needs_auth() {
                        set_needs_auth(entry, diagnostic);
                        false
                    } else {
                        set_failed(entry, diagnostic)
                    }
                }
            };
            (snapshot_for_entry(entry), need_reconnect)
        };

        if need_reconnect {
            self.schedule_reconnect(id).await;
        }

        Ok(snapshot)
    }

    /// Return current snapshots for all managed servers.
    pub async fn snapshots(&self) -> Vec<McpServerSnapshot> {
        self.poll_finished_connections().await;
        let state = self.inner.read().await;
        state.entries.values().map(snapshot_for_entry).collect()
    }

    /// Return the snapshot for a single server, if it exists.
    pub async fn snapshot(&self, id: &str) -> Option<McpServerSnapshot> {
        self.poll_finished_connections().await;
        let state = self.inner.read().await;
        state.entries.get(id).map(snapshot_for_entry)
    }

    /// Get the MCP client for a connected server.
    ///
    /// Returns an error if the server is not found or has no active client.
    pub async fn get_client(&self, server_id: &str) -> Result<Arc<dyn McpClient>, McpError> {
        self.poll_finished_connections().await;
        let state = self.inner.read().await;
        let entry = state
            .entries
            .get(server_id)
            .ok_or_else(|| McpError::protocol(format!("MCP server '{server_id}' not found")))?;
        entry.client.clone().ok_or_else(|| {
            McpError::protocol(format!("MCP server '{server_id}' has no active client"))
        })
    }

    /// Start OAuth authentication for an HTTP/SSE MCP server.
    pub async fn authenticate_oauth(&self, server_id: &str) -> anyhow::Result<super::ToolResult> {
        let (identity, oauth_service) = {
            let state = self.inner.read().await;
            let Some(entry) = state.entries.get(server_id) else {
                anyhow::bail!("MCP server '{server_id}' not found");
            };
            if !entry.config.enabled {
                anyhow::bail!("MCP server '{server_id}' is disabled");
            }
            if !matches!(
                entry.config.transport,
                ManagedMcpTransport::Http { .. } | ManagedMcpTransport::Sse { .. }
            ) {
                anyhow::bail!(
                    "MCP server '{server_id}' does not use an HTTP/SSE OAuth-capable transport"
                );
            }
            let identity = oauth_identity_for_config(&entry.config)?
                .context("HTTP/SSE MCP server is missing an OAuth identity")?;
            (identity, state.oauth_service.clone())
        };

        let flow = match oauth_service.begin_authorization(identity).await {
            Ok(flow) => flow,
            Err(err) => {
                return Ok(super::ToolResult::error(format!(
                    "Could not start OAuth authentication for MCP server '{server_id}': {err}. Authentication may require `/mcp` or CLI completion because callback completion is not wired in core yet."
                )));
            }
        };

        let authorization_url = flow.authorization_url().to_string();
        Ok(super::ToolResult::ok(format!(
            "OAuth authentication started for MCP server '{server_id}'. Open this authorization URL:\n\n{authorization_url}\n\nCallback completion and reconnect are not wired in core yet, so finish authentication through `/mcp` or the CLI when available. Neo will not claim this server is authenticated or reconnect it until credentials are actually persisted."
        ))
        .with_details(serde_json::json!({
            "authorization_url": authorization_url,
            "server_id": server_id,
            "callback_completion_wired": false,
            "reconnected": false
        })))
    }

    /// Register tools from connected servers into the given registry.
    /// Returns diagnostics for any failures or collisions.
    pub async fn register_connected_tools_into(
        &self,
        registry: &mut ToolRegistry,
    ) -> Vec<McpDiagnostic> {
        self.poll_finished_connections().await;
        let state = self.inner.read().await;
        let mut diagnostics = Vec::new();
        let mut taken_names = BTreeSet::<String>::new();

        for entry in state.entries.values() {
            if matches!(entry.status, McpServerStatus::NeedsAuth) {
                let exposed_name = namespaced_tool_name(&entry.config.id, "authenticate");
                if taken_names.insert(exposed_name.clone()) {
                    registry.register(McpAuthenticateTool {
                        server_id: entry.config.id.clone(),
                        exposed_name,
                        manager: self.clone(),
                    });
                } else {
                    diagnostics.push(McpDiagnostic {
                        server_id: entry.config.id.clone(),
                        transport: entry.config.transport.label().to_owned(),
                        message: "authenticate tool collides with an existing tool; skipping"
                            .to_owned(),
                        hint: Some("Rename the MCP server id or adjust configuration.".to_owned()),
                        stderr_tail: None,
                    });
                }
                if let Some(error) = &entry.error {
                    diagnostics.push(error.clone());
                }
                continue;
            }
            if !matches!(entry.status, McpServerStatus::Connected) {
                if let Some(error) = &entry.error {
                    diagnostics.push(error.clone());
                }
                continue;
            }
            let Some(client) = entry.client.clone() else {
                continue;
            };
            for tool in &entry.tools {
                let exposed_name = namespaced_tool_name(&entry.config.id, &tool.name);
                if !taken_names.insert(exposed_name.clone()) {
                    diagnostics.push(McpDiagnostic {
                        server_id: entry.config.id.clone(),
                        transport: entry.config.transport.label().to_owned(),
                        message: format!(
                            "tool '{tool_name}' collides with an existing tool; skipping",
                            tool_name = tool.name
                        ),
                        hint: Some(
                            "Rename the tool on the MCP server or adjust filters.".to_owned(),
                        ),
                        stderr_tail: None,
                    });
                    continue;
                }
                registry.register(ManagedMcpTool {
                    server_id: entry.config.id.clone(),
                    exposed_name,
                    remote_name: tool.name.clone(),
                    description: tool.description.clone(),
                    input_schema: tool.input_schema.clone(),
                    client: Arc::clone(&client),
                });
            }
        }

        diagnostics
    }

    /// List MCP resources across all connected servers or one specific server.
    pub async fn list_resources(
        &self,
        server_id: Option<&str>,
    ) -> anyhow::Result<Vec<McpResourceListEntry>> {
        self.poll_finished_connections().await;
        let state = self.inner.read().await;
        let mut out = Vec::new();

        for entry in state.entries.values() {
            if !matches!(entry.status, McpServerStatus::Connected) {
                continue;
            }
            if let Some(id) = server_id
                && entry.config.id != id
            {
                continue;
            }
            for resource in &entry.resources {
                out.push(McpResourceListEntry {
                    server_id: entry.config.id.clone(),
                    uri: resource.uri.clone(),
                    name: resource.name.clone(),
                    mime_type: resource.mime_type.clone(),
                });
            }
        }
        Ok(out)
    }

    /// Read an MCP resource from the named server.
    pub async fn read_resource(
        &self,
        server_id: &str,
        uri: &str,
    ) -> anyhow::Result<McpResourceRead> {
        self.poll_finished_connections().await;
        let client = {
            let state = self.inner.read().await;
            let Some(entry) = state.entries.get(server_id) else {
                anyhow::bail!("MCP server '{server_id}' not found");
            };
            if !matches!(entry.status, McpServerStatus::Connected) {
                anyhow::bail!("MCP server '{server_id}' is not connected");
            }
            entry
                .client
                .clone()
                .context("MCP server '{server_id}' has no active client")?
        };

        client
            .read_resource(uri)
            .await
            .map_err(|err| anyhow::anyhow!("{}", err.message()))
    }

    /// Shut down all managed servers and cancel pending tasks.
    pub async fn shutdown(&self) {
        let mut state = self.inner.write().await;
        for entry in state.entries.values_mut() {
            abort_tasks(entry);
            entry.client = None;
            entry.oauth_identity = None;
            entry.status = McpServerStatus::Disabled;
        }
        state.supervisor.cleanup_all().await;
    }

    /// Schedule a background reconnect task for a server in `Reconnecting`
    /// state. The task sleeps for the exponential backoff delay, then calls
    /// `connect_one`. Its result is later consumed by
    /// [`poll_finished_connections`].
    async fn schedule_reconnect(&self, id: &str) {
        let (config, supervisor, oauth_service, delay_ms) = {
            let state = self.inner.read().await;
            let Some(entry) = state.entries.get(id) else {
                return;
            };
            if !matches!(entry.status, McpServerStatus::Reconnecting) {
                return;
            }
            let Some(delay_ms) = entry.next_retry_ms else {
                return;
            };
            (
                entry.config.clone(),
                state.supervisor.clone(),
                state.oauth_service.clone(),
                delay_ms,
            )
        };

        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            connect_one(config, supervisor, oauth_service).await
        });

        let mut state = self.inner.write().await;
        if let Some(entry) = state.entries.get_mut(id) {
            if let Some(old) = entry.reconnect_task.take() {
                old.abort();
            }
            entry.reconnect_task = Some(handle);
        }
    }

    /// Poll any finished connect/reconnect tasks and update entry state.
    async fn poll_finished_connections(&self) {
        let mut completed_connects = Vec::new();
        let mut completed_reconnects = Vec::new();
        {
            let mut state = self.inner.write().await;
            for (id, entry) in &mut state.entries {
                if let Some(task) = &mut entry.connect_task
                    && task.is_finished()
                {
                    completed_connects.push((id.clone(), entry.attempt_id));
                }
                if let Some(task) = &mut entry.reconnect_task
                    && task.is_finished()
                {
                    completed_reconnects.push((id.clone(), entry.attempt_id));
                }
            }
        }

        let mut need_reconnect = Vec::new();

        // Process finished connect tasks.
        for (id, attempt_id) in completed_connects {
            let handle = {
                let mut state = self.inner.write().await;
                let Some(entry) = state.entries.get_mut(&id) else {
                    continue;
                };
                entry.connect_task.take()
            };
            let Some(handle) = handle else {
                continue;
            };
            match handle.await {
                Ok(Ok(outcome)) => {
                    let mut state = self.inner.write().await;
                    let Some(entry) = state.entries.get_mut(&id) else {
                        continue;
                    };
                    if entry.attempt_id != attempt_id {
                        continue;
                    }
                    entry.client = Some(outcome.client);
                    entry.oauth_identity = outcome.oauth_identity;
                    entry.tools = outcome.tools;
                    entry.resources = outcome.resources;
                    entry.status = McpServerStatus::Connected;
                    entry.error = None;
                    entry.reconnect_attempt = 0;
                    entry.next_retry_ms = None;
                }
                Ok(Err(err)) => {
                    let mut state = self.inner.write().await;
                    let Some(entry) = state.entries.get_mut(&id) else {
                        continue;
                    };
                    if entry.attempt_id != attempt_id {
                        continue;
                    }
                    if apply_connect_error(entry, &err) {
                        need_reconnect.push(id.clone());
                    }
                }
                Err(join_err) => {
                    let mut state = self.inner.write().await;
                    let Some(entry) = state.entries.get_mut(&id) else {
                        continue;
                    };
                    if entry.attempt_id != attempt_id {
                        continue;
                    }
                    let diagnostic = McpDiagnostic {
                        server_id: entry.config.id.clone(),
                        transport: entry.config.transport.label().to_owned(),
                        message: format!("connect task panicked: {join_err}"),
                        hint: None,
                        stderr_tail: None,
                    };
                    if set_failed(entry, diagnostic) {
                        need_reconnect.push(id.clone());
                    }
                }
            }
        }

        // Process finished reconnect tasks.
        for (id, attempt_id) in completed_reconnects {
            let handle = {
                let mut state = self.inner.write().await;
                let Some(entry) = state.entries.get_mut(&id) else {
                    continue;
                };
                entry.reconnect_task.take()
            };
            let Some(handle) = handle else {
                continue;
            };
            match handle.await {
                Ok(Ok(outcome)) => {
                    let mut state = self.inner.write().await;
                    let Some(entry) = state.entries.get_mut(&id) else {
                        continue;
                    };
                    if entry.attempt_id != attempt_id {
                        continue;
                    }
                    entry.client = Some(outcome.client);
                    entry.oauth_identity = outcome.oauth_identity;
                    entry.tools = outcome.tools;
                    entry.resources = outcome.resources;
                    entry.status = McpServerStatus::Connected;
                    entry.error = None;
                    entry.reconnect_attempt = 0;
                    entry.next_retry_ms = None;
                }
                Ok(Err(err)) => {
                    let mut state = self.inner.write().await;
                    let Some(entry) = state.entries.get_mut(&id) else {
                        continue;
                    };
                    if entry.attempt_id != attempt_id {
                        continue;
                    }
                    if apply_connect_error(entry, &err) {
                        need_reconnect.push(id.clone());
                    }
                }
                Err(join_err) => {
                    let mut state = self.inner.write().await;
                    let Some(entry) = state.entries.get_mut(&id) else {
                        continue;
                    };
                    if entry.attempt_id != attempt_id {
                        continue;
                    }
                    let diagnostic = McpDiagnostic {
                        server_id: entry.config.id.clone(),
                        transport: entry.config.transport.label().to_owned(),
                        message: format!("reconnect task panicked: {join_err}"),
                        hint: None,
                        stderr_tail: None,
                    };
                    if set_failed(entry, diagnostic) {
                        need_reconnect.push(id.clone());
                    }
                }
            }
        }

        // Schedule reconnect tasks for entries that need them.
        for id in &need_reconnect {
            self.schedule_reconnect(id).await;
        }
    }
}

fn spawn_connect(
    config: ManagedMcpServerConfig,
    supervisor: ProcessSupervisor,
    oauth_service: McpOAuthService,
) -> JoinHandle<Result<ConnectOutcome, McpError>> {
    tokio::spawn(async move { connect_one(config, supervisor, oauth_service).await })
}

struct ConnectOutcome {
    client: Arc<dyn McpClient>,
    oauth_identity: Option<McpOAuthIdentity>,
    tools: Vec<McpToolDefinition>,
    resources: Vec<McpResourceDefinition>,
}

async fn connect_one(
    config: ManagedMcpServerConfig,
    supervisor: ProcessSupervisor,
    oauth_service: McpOAuthService,
) -> Result<ConnectOutcome, McpError> {
    let built = build_client_for_config(&config, &supervisor, oauth_service).await?;
    let timeout_ms = config.startup_timeout_ms.unwrap_or(5_000);
    let (tools, resources) = tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        discover_tools(&built.client, &config),
    )
    .await
    .map_err(|_| McpError::protocol(format!("timeout connecting to MCP server {}", config.id)))??;
    Ok(ConnectOutcome {
        client: built.client,
        oauth_identity: built.oauth_identity,
        tools,
        resources,
    })
}

async fn discover_tools(
    client: &Arc<dyn McpClient>,
    config: &ManagedMcpServerConfig,
) -> Result<(Vec<McpToolDefinition>, Vec<McpResourceDefinition>), McpError> {
    let tools = client.list_tools().await?;
    let mut filtered: Vec<McpToolDefinition> = tools;
    if !config.enabled_tools.is_empty() {
        let allow: BTreeSet<String> = config.enabled_tools.iter().cloned().collect();
        filtered.retain(|tool| allow.contains(&tool.name));
    }
    if !config.disabled_tools.is_empty() {
        let deny: BTreeSet<String> = config.disabled_tools.iter().cloned().collect();
        filtered.retain(|tool| !deny.contains(&tool.name));
    }

    // Resource list is best-effort; failure does not mark the server failed.
    let resources = client.list_resources().await.unwrap_or_default();
    Ok((filtered, resources))
}

struct BuiltClient {
    client: Arc<dyn McpClient>,
    oauth_identity: Option<McpOAuthIdentity>,
}

async fn build_client_for_config(
    config: &ManagedMcpServerConfig,
    supervisor: &ProcessSupervisor,
    oauth_service: McpOAuthService,
) -> Result<BuiltClient, McpError> {
    match &config.transport {
        ManagedMcpTransport::Stdio {
            command,
            args,
            env,
            cwd,
        } => {
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
                supervisor,
            )
            .await?;
            Ok(BuiltClient {
                client,
                oauth_identity: None,
            })
        }
        ManagedMcpTransport::Http { url, headers } | ManagedMcpTransport::Sse { url, headers } => {
            let identity = oauth_identity_for_config(config)?.ok_or_else(|| {
                McpError::protocol(format!(
                    "HTTP/SSE MCP server '{}' is missing an OAuth identity",
                    config.id
                ))
            })?;

            let client = http::build_http_client(HttpConfig {
                url: url.clone(),
                headers: headers.clone(),
                startup_timeout_ms: config.startup_timeout_ms,
                request_timeout_ms: config.tool_timeout_ms,
                oauth: Some(HttpOAuthConfig {
                    service: oauth_service,
                    identity: identity.clone(),
                }),
            })
            .await?;
            Ok(BuiltClient {
                client,
                oauth_identity: Some(identity),
            })
        }
    }
}

fn oauth_identity_for_config(
    config: &ManagedMcpServerConfig,
) -> Result<Option<McpOAuthIdentity>, McpError> {
    match &config.transport {
        ManagedMcpTransport::Http { url, .. } => {
            McpOAuthIdentity::new(config.id.clone(), url, McpOAuthTransportKind::Http)
                .map(Some)
                .map_err(|err| McpError::protocol(err.to_string()))
        }
        ManagedMcpTransport::Sse { url, .. } => {
            McpOAuthIdentity::new(config.id.clone(), url, McpOAuthTransportKind::Sse)
                .map(Some)
                .map_err(|err| McpError::protocol(err.to_string()))
        }
        ManagedMcpTransport::Stdio { .. } => Ok(None),
    }
}

fn diagnostic_from_error(
    error: &McpError,
    config: &ManagedMcpServerConfig,
    stderr_tail: Option<String>,
) -> McpDiagnostic {
    let message = error.message().to_owned();
    let hint = diagnostic_hint(&message, config);
    McpDiagnostic {
        server_id: config.id.clone(),
        transport: config.transport.label().to_owned(),
        message,
        hint,
        stderr_tail,
    }
}

fn diagnostic_hint(message: &str, config: &ManagedMcpServerConfig) -> Option<String> {
    let lower = message.to_ascii_lowercase();
    if lower.contains("authrequired")
        || lower.contains("auth required")
        || lower.contains("401")
        || lower.contains("unauthorized")
        || lower.contains("invalid_token")
    {
        if matches!(
            config.transport,
            ManagedMcpTransport::Http { .. } | ManagedMcpTransport::Sse { .. }
        ) {
            return Some(
                "This server requires OAuth. Run `/mcp-config login <server_id>` or `neo mcp auth <server_id>` to authorize."
                    .to_owned(),
            );
        }
        return Some("Check remote MCP authorization headers or disable this server.".to_owned());
    }
    if matches!(config.transport, ManagedMcpTransport::Stdio { .. })
        && lower.contains("failed to start")
    {
        return Some("Check that the command exists and that cwd is valid.".to_owned());
    }
    if lower.contains("timed out") || lower.contains("timeout") {
        return Some(
            "Increase startup_timeout_ms or check that the MCP server starts quickly.".to_owned(),
        );
    }
    None
}

/// Mark an entry as failed. Returns `true` when the reconnect policy allows
/// another attempt and the caller should schedule a reconnect task.
fn set_failed(entry: &mut ManagedMcpEntry, diagnostic: McpDiagnostic) -> bool {
    entry.status = McpServerStatus::Failed;
    entry.error = Some(diagnostic);
    entry.client = None;
    entry.oauth_identity = None;
    entry.tools.clear();
    entry.resources.clear();

    if entry.config.reconnect.enabled {
        entry.reconnect_attempt += 1;
        if let Some(max) = entry.config.reconnect.max_attempts
            && entry.reconnect_attempt >= max
        {
            entry.status = McpServerStatus::Failed;
            entry.next_retry_ms = None;
            return false;
        }
        let delay = reconnect_delay_ms(entry.config.reconnect, entry.reconnect_attempt);
        entry.next_retry_ms = Some(delay);
        entry.status = McpServerStatus::Reconnecting;
        return true;
    }
    false
}

fn set_needs_auth(entry: &mut ManagedMcpEntry, diagnostic: McpDiagnostic) {
    entry.status = McpServerStatus::NeedsAuth;
    entry.error = Some(diagnostic);
    entry.client = None;
    entry.oauth_identity = None;
    entry.tools.clear();
    entry.resources.clear();
    entry.next_retry_ms = None;
}

fn apply_connect_error(entry: &mut ManagedMcpEntry, err: &McpError) -> bool {
    let diagnostic = diagnostic_from_error(err, &entry.config, None);
    if err.is_needs_auth() {
        set_needs_auth(entry, diagnostic);
        false
    } else {
        set_failed(entry, diagnostic)
    }
}

fn reconnect_delay_ms(policy: McpReconnectPolicy, attempt: u32) -> u64 {
    let shift = attempt.saturating_sub(1).min(16);
    let raw = policy.initial_delay_ms.saturating_mul(1_u64 << shift);
    raw.min(policy.max_delay_ms)
}

fn abort_tasks(entry: &mut ManagedMcpEntry) {
    if let Some(task) = entry.connect_task.take() {
        task.abort();
    }
    if let Some(task) = entry.reconnect_task.take() {
        task.abort();
    }
}

fn snapshot_for_entry(entry: &ManagedMcpEntry) -> McpServerSnapshot {
    McpServerSnapshot {
        id: entry.config.id.clone(),
        transport: entry.config.transport.label().to_owned(),
        status: entry.status,
        tool_count: entry.tools.len(),
        tool_names: entry.tools.iter().map(|tool| tool.name.clone()).collect(),
        resource_count: Some(entry.resources.len()),
        error: entry.error.clone(),
        reconnect_attempt: entry.reconnect_attempt,
        next_retry_ms: entry.next_retry_ms,
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

struct ManagedMcpTool {
    server_id: String,
    exposed_name: String,
    remote_name: String,
    description: String,
    input_schema: serde_json::Value,
    client: Arc<dyn McpClient>,
}

impl super::Tool for ManagedMcpTool {
    fn name(&self) -> &str {
        &self.exposed_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> serde_json::Value {
        self.input_schema.clone()
    }

    fn execute<'a>(
        &'a self,
        _ctx: &'a super::ToolContext,
        input: serde_json::Value,
    ) -> super::ToolFuture<'a> {
        let client = Arc::clone(&self.client);
        let server_id = self.server_id.clone();
        let remote_name = self.remote_name.clone();
        Box::pin(async move {
            client
                .call_tool(&remote_name, input)
                .await
                .map(super::ToolResult::from)
                .map_err(|err| super::ToolError::Mcp {
                    server_id,
                    tool_name: remote_name,
                    message: err.message().to_owned(),
                })
        })
    }
}

struct McpAuthenticateTool {
    server_id: String,
    exposed_name: String,
    manager: McpConnectionManager,
}

impl super::Tool for McpAuthenticateTool {
    fn name(&self) -> &str {
        &self.exposed_name
    }

    fn description(&self) -> &str {
        "Starts OAuth authentication for this MCP server and returns an authorization URL. Callback completion and reconnect are not wired in core yet."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false
        })
    }

    fn execute<'a>(
        &'a self,
        _ctx: &'a super::ToolContext,
        _input: serde_json::Value,
    ) -> super::ToolFuture<'a> {
        let manager = self.manager.clone();
        let server_id = self.server_id.clone();
        Box::pin(async move {
            manager
                .authenticate_oauth(&server_id)
                .await
                .map_err(|err| super::ToolError::Mcp {
                    server_id,
                    tool_name: "authenticate".to_owned(),
                    message: err.to_string(),
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Tool, ToolContext, ToolError};
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
    fn http_oauth_identity_uses_server_url_and_transport_kind() {
        let config = http_server("remote-auth");

        let identity = oauth_identity_for_config(&config).unwrap().unwrap();

        assert_eq!(identity.server_id, "remote-auth");
        assert_eq!(
            identity.canonical_resource_url,
            "https://mcp.example.com/mcp"
        );
        assert_eq!(identity.transport_kind, McpOAuthTransportKind::Http);
    }

    #[test]
    fn diagnostic_hint_for_http_auth_mentions_login_command() {
        let config = http_server("remote-auth");

        let hint = diagnostic_hint("OAuth required: missing token", &config).unwrap();

        assert!(hint.contains("/mcp-config login <server_id>"));
        assert!(hint.contains("neo mcp auth <server_id>"));
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

    #[test]
    fn sanitize_replaces_special_chars() {
        assert_eq!(sanitize_tool_name_segment("a/b"), "a_b");
        assert_eq!(sanitize_tool_name_segment(""), "unnamed");
    }

    #[test]
    fn namespaced_tool_name_format() {
        assert_eq!(
            namespaced_tool_name("filesystem", "read_file"),
            "mcp__filesystem__read_file"
        );
    }

    #[tokio::test]
    async fn upsert_server_preserves_other_entries() {
        let manager = McpConnectionManager::new(ProcessSupervisor::default());
        manager
            .apply_config(vec![disabled_server("one"), disabled_server("two")])
            .await;

        manager.upsert_server(disabled_server("three")).await;

        let snapshots = manager.snapshots().await;
        let ids = snapshots
            .into_iter()
            .map(|snapshot| snapshot.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["one", "three", "two"]);
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
        assert_eq!(result.content, format!("mock-echo:echo:{input}"),);
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
}
