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

#[tokio::test]
async fn stale_reconnect_cannot_install_into_a_new_generation() {
    let manager = McpConnectionManager::new(ProcessSupervisor::default());
    let mut entry = entry_for_status(McpServerStatus::Reconnecting);
    entry.client = None;
    entry.tools.clear();
    entry.resources.clear();
    entry.reconnect_task = Some(ManagedConnectTask {
        attempt_id: 1,
        expected_status: McpServerStatus::Reconnecting,
        cleanup_handle: None,
        handle: tokio::spawn(async {
            Ok(ConnectOutcome {
                client: Arc::new(MockMcpClient {
                    tool_name: "stale".to_owned(),
                    echo_text: "stale".to_owned(),
                }),
                oauth_identity: None,
                tools: vec![McpToolDefinition::new(
                    "stale",
                    "stale tool",
                    serde_json::json!({"type": "object"}),
                )],
                resources: Vec::new(),
            })
        }),
    });
    while !entry
        .reconnect_task
        .as_ref()
        .expect("old reconnect task exists")
        .handle
        .is_finished()
    {
        tokio::task::yield_now().await;
    }

    // The reconnect belongs to generation 1, but a reconfiguration has
    // already advanced the entry to generation 2.
    entry.attempt_id = 2;
    insert_entry(&manager, entry).await;

    manager.poll_finished_connections().await;

    let state = manager.inner.read().await;
    let entry = state.entries.get("auth-server").unwrap();
    assert_eq!(entry.attempt_id, 2);
    assert_eq!(entry.status, McpServerStatus::Reconnecting);
    assert!(entry.client.is_none());
    assert!(entry.tools.is_empty());
}

#[tokio::test]
async fn poll_does_not_take_a_replacement_task_from_the_same_slot() {
    let manager = McpConnectionManager::new(ProcessSupervisor::default());
    let (observed_finished_tx, observed_finished_rx) = tokio::sync::oneshot::channel();
    let hook = Arc::new(PollPhaseHook {
        observed_finished: std::sync::Mutex::new(Some(observed_finished_tx)),
        continue_poll: tokio::sync::Notify::new(),
    });
    let mut entry = entry_for_status(McpServerStatus::Reconnecting);
    entry.client = None;
    entry.reconnect_task = Some(ManagedConnectTask {
        attempt_id: 1,
        expected_status: McpServerStatus::Reconnecting,
        cleanup_handle: None,
        handle: tokio::spawn(async { Err(McpError::protocol("old attempt")) }),
    });
    while !entry
        .reconnect_task
        .as_ref()
        .expect("old reconnect task exists")
        .handle
        .is_finished()
    {
        tokio::task::yield_now().await;
    }
    insert_entry(&manager, entry).await;
    manager.inner.write().await.poll_phase_hook = Some(Arc::clone(&hook));

    let poll_manager = manager.clone();
    let poll = tokio::spawn(async move { poll_manager.poll_finished_connections().await });
    observed_finished_rx.await.unwrap();
    {
        let mut state = manager.inner.write().await;
        let entry = state.entries.get_mut("auth-server").unwrap();
        entry.attempt_id = 2;
        entry.reconnect_task = Some(ManagedConnectTask {
            attempt_id: 2,
            expected_status: McpServerStatus::Reconnecting,
            cleanup_handle: None,
            handle: tokio::spawn(std::future::pending()),
        });
    }
    hook.continue_poll.notify_one();

    tokio::time::timeout(Duration::from_millis(100), poll)
        .await
        .expect("poll must not await the replacement task")
        .unwrap();

    let state = manager.inner.read().await;
    let task = state
        .entries
        .get("auth-server")
        .unwrap()
        .reconnect_task
        .as_ref()
        .unwrap();
    assert_eq!(task.attempt_id, 2);
    assert!(!task.handle.is_finished());
}

struct ControlledClient {
    list_started: tokio::sync::Notify,
    release_list: tokio::sync::Notify,
    shutdowns: std::sync::atomic::AtomicUsize,
    fail_list: bool,
}

#[async_trait::async_trait]
impl McpClient for ControlledClient {
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpError> {
        self.list_started.notify_one();
        self.release_list.notified().await;
        if self.fail_list {
            return Err(McpError::protocol("refresh failed"));
        }
        Ok(vec![McpToolDefinition::new(
            "fresh",
            "fresh tool",
            serde_json::json!({"type": "object"}),
        )])
    }

    async fn call_tool(
        &self,
        _name: &str,
        _arguments: serde_json::Value,
    ) -> Result<McpToolResponse, McpError> {
        unreachable!()
    }

    async fn list_resources(&self) -> Result<Vec<McpResourceDefinition>, McpError> {
        Ok(Vec::new())
    }

    async fn read_resource(&self, _uri: &str) -> Result<McpResourceRead, McpError> {
        unreachable!()
    }

    async fn shutdown(&self) -> Result<(), McpError> {
        self.shutdowns
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn stale_refresh_cannot_overwrite_a_reconfigured_entry() {
    let supervisor = ProcessSupervisor::default();
    let manager = McpConnectionManager::new(supervisor.clone());
    let client = Arc::new(ControlledClient {
        list_started: tokio::sync::Notify::new(),
        release_list: tokio::sync::Notify::new(),
        shutdowns: std::sync::atomic::AtomicUsize::new(0),
        fail_list: false,
    });
    let mut entry = entry_for_status(McpServerStatus::Connected);
    entry.client = Some(client.clone());
    insert_entry(&manager, entry).await;
    supervisor
        .register(stdio::process_handle("auth-server", 1), {
            let client = client.clone();
            move |_handle| {
                let client = client.clone();
                Box::pin(async move {
                    let _ = client.shutdown().await;
                })
            }
        })
        .await;

    let refresh_manager = manager.clone();
    let refresh = tokio::spawn(async move { refresh_manager.refresh_tools("auth-server").await });
    client.list_started.notified().await;
    {
        let mut state = manager.inner.write().await;
        let entry = state.entries.get_mut("auth-server").unwrap();
        entry.attempt_id = 2;
        entry.status = McpServerStatus::Disabled;
        entry.client = None;
        entry.tools.clear();
    }
    client.release_list.notify_one();

    let _ = refresh.await.unwrap();

    let state = manager.inner.read().await;
    let entry = state.entries.get("auth-server").unwrap();
    assert_eq!(entry.attempt_id, 2);
    assert_eq!(entry.status, McpServerStatus::Disabled);
    assert!(entry.client.is_none());
    assert!(entry.tools.is_empty());
    assert_eq!(
        client.shutdowns.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn scheduled_reconnect_uses_a_fresh_attempt_id() {
    let manager = McpConnectionManager::new(ProcessSupervisor::default());
    let mut entry = entry_for_status(McpServerStatus::Reconnecting);
    entry.config.enabled = true;
    entry.next_retry_ms = Some(60_000);
    insert_entry(&manager, entry).await;
    manager.inner.write().await.next_attempt_id = 2;

    manager.schedule_reconnect("auth-server").await;

    let state = manager.inner.read().await;
    let entry = state.entries.get("auth-server").unwrap();
    assert_eq!(entry.attempt_id, 2);
    assert_eq!(entry.reconnect_task.as_ref().unwrap().attempt_id, 2);
}

#[tokio::test]
async fn stale_successful_outcome_shuts_down_its_client() {
    let manager = McpConnectionManager::new(ProcessSupervisor::default());
    let client = Arc::new(ControlledClient {
        list_started: tokio::sync::Notify::new(),
        release_list: tokio::sync::Notify::new(),
        shutdowns: std::sync::atomic::AtomicUsize::new(0),
        fail_list: false,
    });
    let mut entry = entry_for_status(McpServerStatus::Reconnecting);
    entry.attempt_id = 2;
    entry.client = None;
    entry.reconnect_task = Some(ManagedConnectTask {
        attempt_id: 1,
        expected_status: McpServerStatus::Reconnecting,
        cleanup_handle: None,
        handle: tokio::spawn({
            let client = client.clone();
            async move {
                Ok(ConnectOutcome {
                    client,
                    oauth_identity: None,
                    tools: Vec::new(),
                    resources: Vec::new(),
                })
            }
        }),
    });
    while !entry.reconnect_task.as_ref().unwrap().handle.is_finished() {
        tokio::task::yield_now().await;
    }
    insert_entry(&manager, entry).await;

    manager.poll_finished_connections().await;

    assert_eq!(
        client.shutdowns.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn refresh_failure_without_reconnect_retires_client_and_cleanup() {
    let supervisor = ProcessSupervisor::default();
    let manager = McpConnectionManager::new(supervisor.clone());
    let client = Arc::new(ControlledClient {
        list_started: tokio::sync::Notify::new(),
        release_list: tokio::sync::Notify::new(),
        shutdowns: std::sync::atomic::AtomicUsize::new(0),
        fail_list: true,
    });
    let mut entry = entry_for_status(McpServerStatus::Connected);
    entry.config.reconnect.enabled = false;
    entry.client = Some(client.clone());
    insert_entry(&manager, entry).await;
    supervisor
        .register(stdio::process_handle("auth-server", 1), {
            let client = client.clone();
            move |_handle| {
                let client = client.clone();
                Box::pin(async move {
                    let _ = client.shutdown().await;
                })
            }
        })
        .await;

    let refresh_manager = manager.clone();
    let refresh = tokio::spawn(async move { refresh_manager.refresh_tools("auth-server").await });
    client.list_started.notified().await;
    client.release_list.notify_one();
    refresh.await.unwrap().unwrap();

    assert_eq!(
        client.shutdowns.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(supervisor.active_count().await, 0);
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
