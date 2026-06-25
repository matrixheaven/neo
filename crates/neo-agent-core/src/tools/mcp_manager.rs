use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::Context;
use rmcp::transport::auth::AuthorizationManager;
use tokio::{
    sync::{Mutex, RwLock},
    task::JoinHandle,
};

use super::{
    ProcessSupervisor, ToolRegistry,
    mcp::{
        HttpConfig, McpClient, McpError, McpResourceDefinition, McpResourceRead, McpToolDefinition,
        StdioConfig, http, oauth, stdio,
    },
};
use crate::oauth::{OAuthProviderRegistry, OAuthStore};

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
    auth_manager: Option<Arc<Mutex<AuthorizationManager>>>,
    tools: Vec<McpToolDefinition>,
    resources: Vec<McpResourceDefinition>,
    error: Option<McpDiagnostic>,
    reconnect_attempt: u32,
    next_retry_ms: Option<u64>,
    reconnect_task: Option<JoinHandle<()>>,
    connect_task: Option<JoinHandle<Result<ConnectOutcome, McpError>>>,
}

struct McpConnectionManagerState {
    supervisor: ProcessSupervisor,
    entries: BTreeMap<String, ManagedMcpEntry>,
    next_attempt_id: u64,
    oauth_store: Arc<RwLock<OAuthStore>>,
    oauth_store_path: Option<PathBuf>,
    oauth_provider_registry: Arc<OAuthProviderRegistry>,
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
                oauth_store: Arc::new(RwLock::new(OAuthStore::default())),
                oauth_store_path: None,
                oauth_provider_registry: Arc::new(OAuthProviderRegistry::with_builtin_providers()),
            })),
        }
    }

    #[must_use]
    pub fn with_oauth_store(
        supervisor: ProcessSupervisor,
        oauth_store: Arc<RwLock<OAuthStore>>,
        oauth_store_path: Option<PathBuf>,
    ) -> Self {
        Self {
            inner: Arc::new(RwLock::new(McpConnectionManagerState {
                supervisor,
                entries: BTreeMap::new(),
                next_attempt_id: 1,
                oauth_store,
                oauth_store_path,
                oauth_provider_registry: Arc::new(OAuthProviderRegistry::with_builtin_providers()),
            })),
        }
    }

    /// Replace the OAuth store and optional persistence path used for managed
    /// HTTP/SSE adapters.
    pub async fn set_oauth_store(
        &self,
        oauth_store: Arc<RwLock<OAuthStore>>,
        oauth_store_path: Option<PathBuf>,
    ) {
        let mut state = self.inner.write().await;
        state.oauth_store = oauth_store;
        state.oauth_store_path = oauth_store_path;
    }

    /// Replace the OAuth provider registry used for managed HTTP/SSE adapters.
    ///
    /// Custom providers from config can override built-in providers by
    /// registering under the same provider id.
    pub async fn set_oauth_provider_registry(&self, registry: OAuthProviderRegistry) {
        let mut state = self.inner.write().await;
        state.oauth_provider_registry = Arc::new(registry);
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
                existing.auth_manager = None;
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
                    auth_manager: None,
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
                let oauth_store = Arc::clone(&state.oauth_store);
                let oauth_store_path = state.oauth_store_path.clone();
                let handle = spawn_connect(
                    server.clone(),
                    state.supervisor.clone(),
                    oauth_store,
                    oauth_store_path,
                );
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
        let (config, supervisor, oauth_store, oauth_store_path) = {
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
            entry.auth_manager = None;
            entry.tools.clear();
            entry.resources.clear();
            entry.error = None;
            entry.reconnect_attempt = 0;
            entry.next_retry_ms = None;
            let supervisor = state.supervisor.clone();
            let oauth_store = Arc::clone(&state.oauth_store);
            let oauth_store_path = state.oauth_store_path.clone();
            let config = entry.config.clone();
            state.entries.insert(id.to_owned(), entry);
            (config, supervisor, oauth_store, oauth_store_path)
        };

        let handle = spawn_connect(
            config.clone(),
            supervisor,
            oauth_store,
            oauth_store_path,
        );
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

        let mut state = self.inner.write().await;
        let Some(entry) = state.entries.get_mut(id) else {
            anyhow::bail!("MCP server '{id}' disappeared during refresh");
        };
        match result {
            Ok((tools, resources)) => {
                entry.status = McpServerStatus::Connected;
                entry.tools = tools;
                entry.resources = resources;
                entry.error = None;
                entry.reconnect_attempt = 0;
                entry.next_retry_ms = None;
            }
            Err(err) => {
                let diagnostic = diagnostic_from_error(&err, &entry.config, None);
                set_failed(entry, diagnostic);
            }
        }
        Ok(snapshot_for_entry(entry))
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
            entry.auth_manager = None;
            entry.status = McpServerStatus::Disabled;
        }
        state.supervisor.cleanup_all().await;
    }

    /// Poll any finished connect/reconnect tasks and update entry state.
    async fn poll_finished_connections(&self) {
        let mut completed = Vec::new();
        {
            let mut state = self.inner.write().await;
            for (id, entry) in &mut state.entries {
                if let Some(task) = &mut entry.connect_task
                    && task.is_finished()
                {
                    completed.push((id.clone(), entry.attempt_id));
                }
            }
        }

        for (id, attempt_id) in completed {
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
                    entry.auth_manager = outcome.auth_manager;
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
                    let diagnostic = diagnostic_from_error(&err, &entry.config, None);
                    set_failed(entry, diagnostic);
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
                    set_failed(entry, diagnostic);
                }
            }
        }
    }
}

fn spawn_connect(
    config: ManagedMcpServerConfig,
    supervisor: ProcessSupervisor,
    oauth_store: Arc<RwLock<OAuthStore>>,
    oauth_store_path: Option<PathBuf>,
) -> JoinHandle<Result<ConnectOutcome, McpError>> {
    tokio::spawn(async move {
        connect_one(
            config,
            supervisor,
            oauth_store,
            oauth_store_path,
        )
        .await
    })
}

struct ConnectOutcome {
    client: Arc<dyn McpClient>,
    auth_manager: Option<Arc<tokio::sync::Mutex<AuthorizationManager>>>,
    tools: Vec<McpToolDefinition>,
    resources: Vec<McpResourceDefinition>,
}

async fn connect_one(
    config: ManagedMcpServerConfig,
    supervisor: ProcessSupervisor,
    oauth_store: Arc<RwLock<OAuthStore>>,
    oauth_store_path: Option<PathBuf>,
) -> Result<ConnectOutcome, McpError> {
    let (client, auth_manager) = build_client_for_config(
        &config,
        &supervisor,
        &oauth_store,
        oauth_store_path.as_ref(),
    )
    .await?;
    let timeout_ms = config.startup_timeout_ms.unwrap_or(5_000);
    let (tools, resources) = tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        discover_tools(&client, &config),
    )
    .await
    .map_err(|_| McpError::protocol(format!("timeout connecting to MCP server {}", config.id)))??;
    Ok(ConnectOutcome {
        client,
        auth_manager,
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

async fn build_client_for_config(
    config: &ManagedMcpServerConfig,
    supervisor: &ProcessSupervisor,
    _oauth_store: &Arc<RwLock<OAuthStore>>,
    oauth_store_path: Option<&PathBuf>,
) -> Result<(Arc<dyn McpClient>, Option<Arc<Mutex<AuthorizationManager>>>), McpError> {
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
                    request_timeout_ms: config.tool_timeout_ms,
                },
                supervisor,
            )
            .await?;
            Ok((client, None))
        }
        ManagedMcpTransport::Http { url, headers } | ManagedMcpTransport::Sse { url, headers } => {
            // Try to build an AuthorizationManager for OAuth support.
            let auth_manager = match oauth_store_path {
                Some(path) => oauth::build_authorization_manager(url, path, &config.id)
                    .await
                    .ok(),
                None => None,
            };

            let client = http::build_http_client(HttpConfig {
                url: url.clone(),
                headers: headers.clone(),
                startup_timeout_ms: config.startup_timeout_ms,
                request_timeout_ms: config.tool_timeout_ms,
                auth_manager: auth_manager.clone(),
            })
            .await?;
            Ok((client, auth_manager))
        }
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
    if lower.contains("401") || lower.contains("unauthorized") {
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

fn set_failed(entry: &mut ManagedMcpEntry, diagnostic: McpDiagnostic) {
    entry.status = McpServerStatus::Failed;
    entry.error = Some(diagnostic);
    entry.client = None;
    entry.auth_manager = None;
    entry.tools.clear();
    entry.resources.clear();

    if entry.config.reconnect.enabled {
        entry.reconnect_attempt += 1;
        if let Some(max) = entry.config.reconnect.max_attempts
            && entry.reconnect_attempt >= max
        {
            entry.status = McpServerStatus::Failed;
            entry.next_retry_ms = None;
            return;
        }
        let delay = reconnect_delay_ms(entry.config.reconnect, entry.reconnect_attempt);
        entry.next_retry_ms = Some(delay);
        entry.status = McpServerStatus::Reconnecting;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
