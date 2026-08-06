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
    Cancelled,
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
            Self::Cancelled => "cancelled",
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
    reconnect_task: Option<ManagedConnectTask>,
    connect_task: Option<ManagedConnectTask>,
}

struct ManagedConnectTask {
    attempt_id: u64,
    expected_status: McpServerStatus,
    cleanup_handle: Option<String>,
    handle: JoinHandle<Result<ConnectOutcome, McpError>>,
}

struct McpConnectionManagerState {
    supervisor: ProcessSupervisor,
    entries: BTreeMap<String, ManagedMcpEntry>,
    next_attempt_id: u64,
    oauth_service: McpOAuthService,
    #[cfg(test)]
    poll_phase_hook: Option<Arc<PollPhaseHook>>,
}

#[cfg(test)]
struct PollPhaseHook {
    observed_finished: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    continue_poll: tokio::sync::Notify,
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
                #[cfg(test)]
                poll_phase_hook: None,
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
                #[cfg(test)]
                poll_phase_hook: None,
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
        let mut retirement = ConnectionRetirement::default();

        // Remove entries no longer present.
        let new_ids: BTreeSet<String> = servers.iter().map(|s| s.id.clone()).collect();
        state.entries.retain(|_id, entry| {
            if new_ids.contains(&entry.config.id) {
                return true;
            }
            retirement.collect_entry(entry);
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
                retirement.collect_entry(&mut existing);
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
                entry.connect_task = Some(spawn_connect(
                    server.clone(),
                    state.supervisor.clone(),
                    oauth_service,
                    attempt_id,
                    McpServerStatus::Pending,
                ));
            } else {
                entry.status = McpServerStatus::Disabled;
            }
            state.entries.insert(server.id.clone(), entry);
        }

        let supervisor = state.supervisor.clone();
        let snapshots = state.entries.values().map(snapshot_for_entry).collect();
        drop(state);
        retirement.retire(&supervisor).await;
        snapshots
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
        let mut retirement = ConnectionRetirement::default();
        retirement.collect_entry(&mut entry);
        let supervisor = state.supervisor.clone();
        drop(state);
        retirement.retire(&supervisor).await;
        true
    }

    /// Force an immediate reconnect for the given server.
    pub async fn reconnect_now(&self, id: &str) -> anyhow::Result<McpServerSnapshot> {
        let (config, supervisor, oauth_service, attempt_id, retirement) = {
            let mut state = self.inner.write().await;
            let Some(mut entry) = state.entries.remove(id) else {
                anyhow::bail!("MCP server '{id}' not found");
            };
            if !entry.config.enabled {
                state.entries.insert(id.to_owned(), entry);
                anyhow::bail!("MCP server '{id}' is disabled");
            }
            let mut retirement = ConnectionRetirement::default();
            retirement.collect_entry(&mut entry);
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
            (config, supervisor, oauth_service, attempt_id, retirement)
        };

        retirement.retire(&supervisor).await;
        let task = spawn_connect(
            config.clone(),
            supervisor.clone(),
            oauth_service,
            attempt_id,
            McpServerStatus::Pending,
        );
        let rejected_task = {
            let mut state = self.inner.write().await;
            if let Some(entry) = state.entries.get_mut(id)
                && entry.attempt_id == attempt_id
                && entry.status == McpServerStatus::Pending
            {
                entry.connect_task = Some(task);
                None
            } else {
                Some(task)
            }
        };
        if let Some(task) = rejected_task {
            let mut retirement = ConnectionRetirement::default();
            retirement.push_task(task);
            retirement.retire(&supervisor).await;
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
        let (client, config, attempt_id, supervisor) = {
            let mut state = self.inner.write().await;
            let supervisor = state.supervisor.clone();
            let Some(entry) = state.entries.get_mut(id) else {
                anyhow::bail!("MCP server '{id}' not found");
            };
            if entry.status != McpServerStatus::Connected {
                anyhow::bail!("MCP server '{id}' is not connected");
            }
            let Some(client) = entry.client.clone() else {
                anyhow::bail!("MCP server '{id}' is not connected");
            };
            let attempt_id = entry.attempt_id;
            entry.status = McpServerStatus::Pending;
            (client, entry.config.clone(), attempt_id, supervisor)
        };

        let result = discover_tools(&client, &config).await;

        let (snapshot, need_reconnect, refresh_failed) = {
            let mut state = self.inner.write().await;
            let Some(entry) = state.entries.get_mut(id) else {
                drop(state);
                let mut retirement = ConnectionRetirement::default();
                if matches!(config.transport, ManagedMcpTransport::Stdio { .. }) {
                    retirement.push_cleanup(stdio_cleanup_handle_for_config(&config, attempt_id));
                } else {
                    retirement.push_client(config.id.clone(), client);
                }
                retirement.retire(&supervisor).await;
                anyhow::bail!("MCP server '{id}' disappeared during refresh");
            };
            if entry.attempt_id != attempt_id
                || entry.status != McpServerStatus::Pending
                || !entry
                    .client
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &client))
            {
                let snapshot = snapshot_for_entry(entry);
                drop(state);
                let mut retirement = ConnectionRetirement::default();
                if matches!(config.transport, ManagedMcpTransport::Stdio { .. }) {
                    retirement.push_cleanup(stdio_cleanup_handle_for_config(&config, attempt_id));
                } else {
                    retirement.push_client(config.id.clone(), client);
                }
                retirement.retire(&supervisor).await;
                return Ok(snapshot);
            }
            let refresh_failed = result.is_err();
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
                    let diagnostic = diagnostic_from_error(&err, &entry.config);
                    if err.is_needs_auth() {
                        set_needs_auth(entry, diagnostic);
                        false
                    } else {
                        set_failed(entry, diagnostic)
                    }
                }
            };
            (snapshot_for_entry(entry), need_reconnect, refresh_failed)
        };

        if refresh_failed {
            let mut retirement = ConnectionRetirement::default();
            if matches!(config.transport, ManagedMcpTransport::Stdio { .. }) {
                retirement.push_cleanup(stdio_cleanup_handle_for_config(&config, attempt_id));
            } else {
                retirement.push_client(config.id.clone(), client);
            }
            retirement.retire(&supervisor).await;
        }
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

    /// Cancel pending startup and reconnect tasks without disconnecting ready servers.
    pub async fn cancel_startup(&self) {
        let mut state = self.inner.write().await;
        let mut retirement = ConnectionRetirement::default();
        for entry in state.entries.values_mut().filter(|entry| {
            matches!(
                entry.status,
                McpServerStatus::Pending | McpServerStatus::Reconnecting
            )
        }) {
            retirement.collect_tasks(entry);
            entry.status = McpServerStatus::Cancelled;
            entry.error = None;
            entry.next_retry_ms = None;
        }
        let supervisor = state.supervisor.clone();
        drop(state);
        retirement.retire(&supervisor).await;
    }

    /// Shut down all managed servers and cancel pending tasks.
    pub async fn shutdown(&self) {
        let mut state = self.inner.write().await;
        let mut retirement = ConnectionRetirement::default();
        for entry in state.entries.values_mut() {
            retirement.collect_entry(entry);
            entry.oauth_identity = None;
            entry.status = McpServerStatus::Disabled;
        }
        let supervisor = state.supervisor.clone();
        drop(state);
        retirement.retire(&supervisor).await;
        supervisor.cleanup_all().await;
    }

    /// Schedule a background reconnect task for a server in `Reconnecting`
    /// state. The task sleeps for the exponential backoff delay, then calls
    /// `connect_one`. Its result is later consumed by
    /// [`poll_finished_connections`].
    async fn schedule_reconnect(&self, id: &str) {
        let (config, supervisor, oauth_service, delay_ms, attempt_id, retirement) = {
            let mut state = self.inner.write().await;
            let supervisor = state.supervisor.clone();
            let oauth_service = state.oauth_service.clone();
            let Some(mut entry) = state.entries.remove(id) else {
                return;
            };
            if !matches!(entry.status, McpServerStatus::Reconnecting) {
                state.entries.insert(id.to_owned(), entry);
                return;
            }
            let Some(delay_ms) = entry.next_retry_ms else {
                state.entries.insert(id.to_owned(), entry);
                return;
            };
            let mut retirement = ConnectionRetirement::default();
            retirement.collect_entry(&mut entry);
            let attempt_id = state.next_attempt_id;
            state.next_attempt_id += 1;
            entry.attempt_id = attempt_id;
            let config = entry.config.clone();
            state.entries.insert(id.to_owned(), entry);
            (
                config,
                supervisor,
                oauth_service,
                delay_ms,
                attempt_id,
                retirement,
            )
        };
        retirement.retire(&supervisor).await;

        let cleanup_handle = stdio_cleanup_handle_for_config(&config, attempt_id);
        let task_supervisor = supervisor.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            connect_one(config, task_supervisor, oauth_service, attempt_id).await
        });
        let task = ManagedConnectTask {
            attempt_id,
            expected_status: McpServerStatus::Reconnecting,
            cleanup_handle,
            handle,
        };

        let rejected_task = {
            let mut state = self.inner.write().await;
            if let Some(entry) = state.entries.get_mut(id)
                && entry.attempt_id == attempt_id
                && entry.status == McpServerStatus::Reconnecting
                && entry.reconnect_task.is_none()
            {
                entry.reconnect_task = Some(task);
                None
            } else {
                Some(task)
            }
        };
        if let Some(task) = rejected_task {
            let mut retirement = ConnectionRetirement::default();
            retirement.push_task(task);
            retirement.retire(&supervisor).await;
        }
    }

    /// Poll any finished connect/reconnect tasks and update entry state.
    async fn poll_finished_connections(&self) {
        let mut completed_connects = Vec::new();
        let mut completed_reconnects = Vec::new();
        {
            let mut state = self.inner.write().await;
            for (id, entry) in &mut state.entries {
                if let Some(task) = &entry.connect_task
                    && task.handle.is_finished()
                {
                    completed_connects.push((
                        id.clone(),
                        entry.connect_task.take().expect("finished task exists"),
                    ));
                }
                if let Some(task) = &entry.reconnect_task
                    && task.handle.is_finished()
                {
                    completed_reconnects.push((
                        id.clone(),
                        entry.reconnect_task.take().expect("finished task exists"),
                    ));
                }
            }
        }

        #[cfg(test)]
        let poll_phase_hook = {
            let state = self.inner.read().await;
            state.poll_phase_hook.clone()
        };
        #[cfg(test)]
        if let Some(hook) = poll_phase_hook {
            let observed = hook
                .observed_finished
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(observed_finished) = observed {
                let _ = observed_finished.send(());
                hook.continue_poll.notified().await;
            }
        }

        let mut need_reconnect: Vec<String> = Vec::new();
        let mut cleanup_handles: Vec<String> = Vec::new();
        let supervisor = self.inner.read().await.supervisor.clone();

        for (id, task) in completed_connects {
            process_finished_connection(
                self,
                id,
                task,
                false,
                &supervisor,
                &mut need_reconnect,
                &mut cleanup_handles,
            )
            .await;
        }
        for (id, task) in completed_reconnects {
            process_finished_connection(
                self,
                id,
                task,
                true,
                &supervisor,
                &mut need_reconnect,
                &mut cleanup_handles,
            )
            .await;
        }

        for handle in cleanup_handles {
            supervisor.remove_and_cleanup(&handle).await;
        }

        // Schedule reconnect tasks for entries that need them.
        for id in &need_reconnect {
            self.schedule_reconnect(id).await;
        }
    }
}

async fn process_finished_connection(
    manager: &McpConnectionManager,
    id: String,
    task: ManagedConnectTask,
    reconnect: bool,
    supervisor: &ProcessSupervisor,
    need_reconnect: &mut Vec<String>,
    cleanup_handles: &mut Vec<String>,
) {
    let attempt_id = task.attempt_id;
    let expected_status = task.expected_status;
    let cleanup_handle = task.cleanup_handle;
    match task.handle.await {
        Ok(Ok(outcome)) => {
            let mut state = manager.inner.write().await;
            let accepted = install_connect_outcome(
                state.entries.get_mut(&id),
                attempt_id,
                expected_status,
                &outcome,
            );
            drop(state);
            if !accepted {
                retire_rejected_outcome(&id, outcome, cleanup_handle, supervisor).await;
            }
        }
        Ok(Err(err)) => {
            cleanup_handles.extend(cleanup_handle);
            let mut state = manager.inner.write().await;
            let Some(entry) = state.entries.get_mut(&id) else {
                return;
            };
            if entry.attempt_id != attempt_id || entry.status != expected_status {
                return;
            }
            if apply_connect_error(entry, &err) {
                need_reconnect.push(id);
            }
        }
        Err(join_err) => {
            cleanup_handles.extend(cleanup_handle);
            let mut state = manager.inner.write().await;
            let Some(entry) = state.entries.get_mut(&id) else {
                return;
            };
            if entry.attempt_id != attempt_id || entry.status != expected_status {
                return;
            }
            let diagnostic = McpDiagnostic {
                server_id: entry.config.id.clone(),
                transport: entry.config.transport.label().to_owned(),
                message: format!(
                    "{}connect task panicked: {join_err}",
                    if reconnect { "re" } else { "" }
                ),
                hint: None,
                stderr_tail: None,
            };
            if set_failed(entry, diagnostic) {
                need_reconnect.push(id);
            }
        }
    }
}

fn spawn_connect(
    config: ManagedMcpServerConfig,
    supervisor: ProcessSupervisor,
    oauth_service: McpOAuthService,
    attempt_id: u64,
    expected_status: McpServerStatus,
) -> ManagedConnectTask {
    let cleanup_handle = stdio_cleanup_handle_for_config(&config, attempt_id);
    let handle =
        tokio::spawn(
            async move { connect_one(config, supervisor, oauth_service, attempt_id).await },
        );
    ManagedConnectTask {
        attempt_id,
        expected_status,
        cleanup_handle,
        handle,
    }
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
    attempt_id: u64,
) -> Result<ConnectOutcome, McpError> {
    let built = build_client_for_config(&config, &supervisor, oauth_service, attempt_id).await?;
    complete_connection(built.client, built.oauth_identity, &config).await
}

async fn complete_connection(
    client: Arc<dyn McpClient>,
    oauth_identity: Option<McpOAuthIdentity>,
    config: &ManagedMcpServerConfig,
) -> Result<ConnectOutcome, McpError> {
    let timeout_ms = config.startup_timeout_ms.unwrap_or(5_000);
    let discovery = match tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        discover_tools(&client, config),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(McpError::protocol(format!(
            "timeout connecting to MCP server {}",
            config.id
        ))
        .with_stderr_tail(client.stderr_tail())),
    };
    match discovery {
        Ok((tools, resources)) => Ok(ConnectOutcome {
            client,
            oauth_identity,
            tools,
            resources,
        }),
        Err(error) => {
            if !matches!(config.transport, ManagedMcpTransport::Stdio { .. })
                && let Err(shutdown_error) = client.shutdown().await
            {
                tracing::warn!(
                    server_id = %config.id,
                    error = %shutdown_error.message(),
                    "failed to shut down MCP client after discovery failure"
                );
            }
            Err(error)
        }
    }
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
    attempt_id: u64,
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
                attempt_id,
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

fn diagnostic_from_error(error: &McpError, config: &ManagedMcpServerConfig) -> McpDiagnostic {
    let message = error.message().to_owned();
    let hint = diagnostic_hint(error, config);
    McpDiagnostic {
        server_id: config.id.clone(),
        transport: config.transport.label().to_owned(),
        message,
        hint,
        stderr_tail: error
            .stderr_tail()
            .map(|tail| String::from_utf8_lossy(tail).into_owned()),
    }
}

/// Build a presentation hint from the typed error kind (auth) and residual
/// protocol diagnostics (start failure / timeout). Auth phrases in the message
/// body never select the auth hint.
fn diagnostic_hint(error: &McpError, config: &ManagedMcpServerConfig) -> Option<String> {
    if error.is_needs_auth() {
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

    let lower = error.message().to_ascii_lowercase();
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
    let diagnostic = diagnostic_from_error(err, &entry.config);
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

#[derive(Default)]
struct ConnectionRetirement {
    tasks: Vec<ManagedConnectTask>,
    direct_clients: Vec<(String, Arc<dyn McpClient>)>,
    cleanup_handles: Vec<String>,
}

impl ConnectionRetirement {
    fn collect_entry(&mut self, entry: &mut ManagedMcpEntry) {
        let client = entry.client.take();
        match &entry.config.transport {
            ManagedMcpTransport::Stdio { .. } => {
                self.push_cleanup(stdio_cleanup_handle(entry));
            }
            ManagedMcpTransport::Http { .. } | ManagedMcpTransport::Sse { .. } => {
                if let Some(client) = client {
                    self.push_client(entry.config.id.clone(), client);
                }
            }
        }
        self.collect_tasks(entry);
    }

    fn collect_tasks(&mut self, entry: &mut ManagedMcpEntry) {
        if let Some(task) = entry.connect_task.take() {
            self.tasks.push(task);
        }
        if let Some(task) = entry.reconnect_task.take() {
            self.tasks.push(task);
        }
    }

    fn push_client(&mut self, server_id: String, client: Arc<dyn McpClient>) {
        self.direct_clients.push((server_id, client));
    }

    fn push_task(&mut self, task: ManagedConnectTask) {
        self.tasks.push(task);
    }

    fn push_cleanup(&mut self, handle: Option<String>) {
        self.cleanup_handles.extend(handle);
    }

    async fn retire(self, supervisor: &ProcessSupervisor) {
        for task in &self.tasks {
            task.handle.abort();
        }
        for task in self.tasks {
            let cleanup_handle = task.cleanup_handle;
            let _ = task.handle.await;
            if let Some(handle) = cleanup_handle {
                supervisor.remove_and_cleanup(&handle).await;
            }
        }
        for (server_id, client) in self.direct_clients {
            if let Err(error) = client.shutdown().await {
                tracing::warn!(
                    %server_id,
                    error = %error.message(),
                    "failed to shut down retired MCP client"
                );
            }
        }
        for handle in self.cleanup_handles {
            supervisor.remove_and_cleanup(&handle).await;
        }
    }
}

fn install_connect_outcome(
    entry: Option<&mut ManagedMcpEntry>,
    attempt_id: u64,
    expected_status: McpServerStatus,
    outcome: &ConnectOutcome,
) -> bool {
    let Some(entry) = entry else {
        return false;
    };
    if entry.attempt_id != attempt_id || entry.status != expected_status {
        return false;
    }
    entry.client = Some(Arc::clone(&outcome.client));
    entry.oauth_identity.clone_from(&outcome.oauth_identity);
    entry.tools.clone_from(&outcome.tools);
    entry.resources.clone_from(&outcome.resources);
    entry.status = McpServerStatus::Connected;
    entry.error = None;
    entry.reconnect_attempt = 0;
    entry.next_retry_ms = None;
    true
}

async fn retire_rejected_outcome(
    server_id: &str,
    outcome: ConnectOutcome,
    cleanup_handle: Option<String>,
    supervisor: &ProcessSupervisor,
) {
    let mut retirement = ConnectionRetirement::default();
    if cleanup_handle.is_some() {
        retirement.push_cleanup(cleanup_handle);
    } else {
        retirement.push_client(server_id.to_owned(), outcome.client);
    }
    retirement.retire(supervisor).await;
}

fn stdio_cleanup_handle(entry: &ManagedMcpEntry) -> Option<String> {
    stdio_cleanup_handle_for_config(&entry.config, entry.attempt_id)
}

fn stdio_cleanup_handle_for_config(
    config: &ManagedMcpServerConfig,
    attempt_id: u64,
) -> Option<String> {
    matches!(config.transport, ManagedMcpTransport::Stdio { .. })
        .then(|| stdio::process_handle(&config.id, attempt_id))
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

    fn description(&self) -> &'static str {
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
#[path = "test_cases/connection_state.rs"]
mod connection_state;

#[cfg(test)]
#[path = "test_cases/auth_diagnostics.rs"]
mod auth_diagnostics;

#[cfg(test)]
#[path = "test_cases/formatting.rs"]
mod formatting;

#[cfg(test)]
#[path = "test_cases/server_lifecycle.rs"]
mod server_lifecycle;

#[cfg(test)]
#[path = "test_cases/stale_races.rs"]
mod stale_races;

#[cfg(test)]
#[path = "test_cases/discovery_failures.rs"]
mod discovery_failures;

#[cfg(test)]
#[path = "test_cases/authenticate_tool.rs"]
mod authenticate_tool;

#[cfg(test)]
#[path = "test_cases/managed_tool.rs"]
mod managed_tool;
