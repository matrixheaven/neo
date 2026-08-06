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
async fn failing_taken_task_cleans_up_after_entry_is_removed() {
    let supervisor = ProcessSupervisor::default();
    let manager = McpConnectionManager::new(supervisor.clone());
    let (observed_finished_tx, observed_finished_rx) = tokio::sync::oneshot::channel();
    let hook = Arc::new(PollPhaseHook {
        observed_finished: std::sync::Mutex::new(Some(observed_finished_tx)),
        continue_poll: tokio::sync::Notify::new(),
    });
    let cleaned = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let cleaned_for_cleanup = cleaned.clone();
    supervisor
        .register(stdio::process_handle("auth-server", 1), move |_handle| {
            let cleaned = cleaned_for_cleanup.clone();
            Box::pin(async move {
                cleaned.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
        })
        .await;
    let mut entry = entry_for_status(McpServerStatus::Reconnecting);
    entry.attempt_id = 2;
    entry.client = None;
    entry.reconnect_task = Some(ManagedConnectTask {
        attempt_id: 1,
        expected_status: McpServerStatus::Reconnecting,
        cleanup_handle: Some(stdio::process_handle("auth-server", 1)),
        handle: tokio::spawn(async { Err(McpError::protocol("old failure")) }),
    });
    while !entry.reconnect_task.as_ref().unwrap().handle.is_finished() {
        tokio::task::yield_now().await;
    }
    insert_entry(&manager, entry).await;
    manager.inner.write().await.poll_phase_hook = Some(Arc::clone(&hook));

    let poll_manager = manager.clone();
    let poll = tokio::spawn(async move { poll_manager.poll_finished_connections().await });
    observed_finished_rx.await.unwrap();
    assert!(manager.remove_server("auth-server").await);
    hook.continue_poll.notify_one();
    poll.await.unwrap();

    assert_eq!(cleaned.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(supervisor.active_count().await, 0);
}

#[tokio::test]
async fn failed_stdio_discovery_cleans_up_supervised_process() {
    let supervisor = ProcessSupervisor::default();
    let manager = McpConnectionManager::new(supervisor.clone());
    let cleaned = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let cleaned_for_cleanup = Arc::clone(&cleaned);
    supervisor
        .register(stdio::process_handle("failed", 1), move |_handle| {
            let cleaned = Arc::clone(&cleaned_for_cleanup);
            Box::pin(async move {
                cleaned.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
        })
        .await;
    let mut entry = entry_for_status(McpServerStatus::Pending);
    entry.config = disabled_server("failed");
    entry.config.enabled = true;
    entry.config.reconnect.enabled = false;
    entry.client = None;
    entry.connect_task = Some(ManagedConnectTask {
        attempt_id: 1,
        expected_status: McpServerStatus::Pending,
        cleanup_handle: Some(stdio::process_handle("failed", 1)),
        handle: tokio::spawn(async { Err(McpError::protocol("discovery failed")) }),
    });
    while !entry.connect_task.as_ref().unwrap().handle.is_finished() {
        tokio::task::yield_now().await;
    }
    insert_entry(&manager, entry).await;

    manager.poll_finished_connections().await;

    assert_eq!(cleaned.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(supervisor.active_count().await, 0);
    assert_eq!(
        manager.snapshot("failed").await.unwrap().status,
        McpServerStatus::Failed
    );
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
async fn failed_discovery_shuts_down_client_before_returning_error() {
    let client = Arc::new(ControlledClient {
        list_started: tokio::sync::Notify::new(),
        release_list: tokio::sync::Notify::new(),
        shutdowns: std::sync::atomic::AtomicUsize::new(0),
        fail_list: true,
    });
    client.release_list.notify_one();

    let Err(error) = complete_connection(client.clone(), None, &http_server("discovery")).await
    else {
        panic!("discovery should fail")
    };

    assert_eq!(error.message(), "refresh failed");
    assert_eq!(
        client.shutdowns.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn startup_timeout_diagnostic_includes_stdio_stderr_tail() {
    struct HangingTailClient;

    #[async_trait::async_trait]
    impl McpClient for HangingTailClient {
        async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpError> {
            std::future::pending().await
        }

        async fn call_tool(
            &self,
            _name: &str,
            _arguments: serde_json::Value,
        ) -> Result<McpToolResponse, McpError> {
            unreachable!()
        }

        async fn list_resources(&self) -> Result<Vec<McpResourceDefinition>, McpError> {
            unreachable!()
        }

        async fn read_resource(&self, _uri: &str) -> Result<McpResourceRead, McpError> {
            unreachable!()
        }

        async fn shutdown(&self) -> Result<(), McpError> {
            Ok(())
        }

        fn stderr_tail(&self) -> Option<Vec<u8>> {
            Some(b"startup stalled".to_vec())
        }
    }

    let mut config = disabled_server("slow-stdio");
    config.startup_timeout_ms = Some(1);
    let Err(error) = complete_connection(Arc::new(HangingTailClient), None, &config).await else {
        panic!("discovery should time out")
    };
    let diagnostic = diagnostic_from_error(&error, &config);

    assert_eq!(diagnostic.stderr_tail.as_deref(), Some("startup stalled"));
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
