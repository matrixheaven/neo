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
async fn cancel_startup_preserves_connected_servers() {
    let manager = McpConnectionManager::new(ProcessSupervisor::default());
    insert_entry(&manager, entry_for_status(McpServerStatus::Connected)).await;

    let mut pending = entry_for_status(McpServerStatus::Pending);
    pending.config.id = "pending-server".to_owned();
    pending.client = None;
    pending.tools.clear();
    pending.resources.clear();
    pending.connect_task = Some(ManagedConnectTask {
        attempt_id: pending.attempt_id,
        expected_status: McpServerStatus::Pending,
        cleanup_handle: None,
        handle: tokio::spawn(std::future::pending()),
    });
    insert_entry(&manager, pending).await;

    manager.cancel_startup().await;

    assert_eq!(
        manager
            .snapshot("auth-server")
            .await
            .expect("connected server exists")
            .status,
        McpServerStatus::Connected
    );
    assert_eq!(
        manager
            .snapshot("pending-server")
            .await
            .expect("pending server exists")
            .status,
        McpServerStatus::Cancelled
    );
    assert!(manager.get_client("auth-server").await.is_ok());
}

#[tokio::test]
async fn removing_stdio_server_awaits_registered_cleanup() {
    let supervisor = ProcessSupervisor::default();
    let manager = McpConnectionManager::new(supervisor.clone());
    manager.apply_config(vec![disabled_server("removed")]).await;
    let cleaned = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let cleaned_for_task = Arc::clone(&cleaned);
    supervisor
        .register(stdio::process_handle("removed", 1), move |_handle| {
            let cleaned = Arc::clone(&cleaned_for_task);
            Box::pin(async move {
                tokio::task::yield_now().await;
                cleaned.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
        })
        .await;

    assert!(manager.remove_server("removed").await);

    assert_eq!(cleaned.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(supervisor.active_count().await, 0);
}

#[tokio::test]
async fn removing_server_awaits_task_before_late_cleanup_registration() {
    struct RegisterCleanupOnDrop {
        supervisor: ProcessSupervisor,
        cleaned: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Drop for RegisterCleanupOnDrop {
        fn drop(&mut self) {
            let cleaned = Arc::clone(&self.cleaned);
            self.supervisor.register_immediately(
                stdio::process_handle("late", 1),
                move |_handle| {
                    let cleaned = Arc::clone(&cleaned);
                    Box::pin(async move {
                        cleaned.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    })
                },
            );
        }
    }

    let supervisor = ProcessSupervisor::default();
    let manager = McpConnectionManager::new(supervisor.clone());
    manager.apply_config(vec![disabled_server("late")]).await;
    let cleaned = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let guard = RegisterCleanupOnDrop {
        supervisor: supervisor.clone(),
        cleaned: Arc::clone(&cleaned),
    };
    {
        let mut state = manager.inner.write().await;
        let entry = state.entries.get_mut("late").unwrap();
        entry.connect_task = Some(ManagedConnectTask {
            attempt_id: 1,
            expected_status: McpServerStatus::Pending,
            cleanup_handle: Some(stdio::process_handle("late", 1)),
            handle: tokio::spawn(async move {
                let _guard = guard;
                std::future::pending::<Result<ConnectOutcome, McpError>>().await
            }),
        });
    }
    tokio::task::yield_now().await;

    assert!(manager.remove_server("late").await);

    assert_eq!(cleaned.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(supervisor.active_count().await, 0);
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
async fn removing_connected_stdio_server_shuts_down_client_once() {
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

    assert!(manager.remove_server("auth-server").await);

    assert_eq!(
        client.shutdowns.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(supervisor.active_count().await, 0);
}

#[tokio::test]
async fn removing_connected_http_server_awaits_client_shutdown() {
    let manager = McpConnectionManager::new(ProcessSupervisor::default());
    let client = Arc::new(ControlledClient {
        list_started: tokio::sync::Notify::new(),
        release_list: tokio::sync::Notify::new(),
        shutdowns: std::sync::atomic::AtomicUsize::new(0),
        fail_list: false,
    });
    let mut entry = entry_for_status(McpServerStatus::Connected);
    entry.config = http_server("http-remove");
    entry.client = Some(client.clone());
    insert_entry(&manager, entry).await;

    assert!(manager.remove_server("http-remove").await);

    assert_eq!(
        client.shutdowns.load(std::sync::atomic::Ordering::SeqCst),
        1
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
