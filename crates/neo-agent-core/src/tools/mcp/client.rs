//! MCP client trait and rmcp-backed implementation (Task 2.1).

use std::time::Duration;

use async_trait::async_trait;
use rmcp::{
    model::{
        CallToolRequest, CallToolRequestParams, ClientRequest, ListResourcesRequest,
        ListToolsRequest, ReadResourceRequest, ReadResourceRequestParams, ServerResult,
    },
    service::{Peer, RoleClient, RunningService},
};
use serde_json::Value;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::{sync::Mutex, time::timeout};

use super::{McpError, McpResourceDefinition, McpResourceRead, McpToolDefinition, McpToolResponse};

pub(crate) const MCP_STDERR_TAIL_CAPACITY: usize = 4 * 1024;

#[derive(Debug, Default)]
pub(crate) struct BoundedByteTail(Vec<u8>);

impl BoundedByteTail {
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        if bytes.len() >= MCP_STDERR_TAIL_CAPACITY {
            self.0.clear();
            self.0
                .extend_from_slice(&bytes[bytes.len() - MCP_STDERR_TAIL_CAPACITY..]);
            return;
        }
        let excess = self
            .0
            .len()
            .saturating_add(bytes.len())
            .saturating_sub(MCP_STDERR_TAIL_CAPACITY);
        if excess > 0 {
            self.0.drain(..excess);
        }
        self.0.extend_from_slice(bytes);
    }

    pub(crate) fn snapshot(&self) -> Vec<u8> {
        self.0.clone()
    }
}

pub(crate) type SharedStderrTail = Arc<StdMutex<BoundedByteTail>>;

/// MCP client abstraction.
///
/// This trait exists to enable test doubles (`MockMcpClient`, `FailingClient`
/// in `mcp_manager` tests). The only production implementor is `RmcpClient`.
#[async_trait]
pub trait McpClient: Send + Sync {
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpError>;
    async fn call_tool(&self, name: &str, arguments: Value) -> Result<McpToolResponse, McpError>;
    async fn list_resources(&self) -> Result<Vec<McpResourceDefinition>, McpError>;
    async fn read_resource(&self, uri: &str) -> Result<McpResourceRead, McpError>;
    async fn shutdown(&self) -> Result<(), McpError>;

    fn stderr_tail(&self) -> Option<Vec<u8>> {
        None
    }
}

#[derive(Debug)]
pub struct RmcpClient {
    peer: Peer<RoleClient>,
    service: Mutex<Option<RunningService<RoleClient, ()>>>,
    request_timeout: Option<Duration>,
    stderr_tail: Option<SharedStderrTail>,
}

impl RmcpClient {
    pub fn new(
        service: RunningService<RoleClient, ()>,
        request_timeout: Option<Duration>,
        stderr_tail: Option<SharedStderrTail>,
    ) -> Self {
        Self {
            peer: service.peer().clone(),
            service: Mutex::new(Some(service)),
            request_timeout,
            stderr_tail,
        }
    }

    fn error_with_stderr(&self, message: impl Into<String>) -> McpError {
        McpError::protocol(message).with_stderr_tail(self.stderr_tail())
    }

    async fn with_request_timeout<T, F>(&self, fut: F) -> Result<T, McpError>
    where
        F: std::future::Future<Output = Result<T, rmcp::service::ServiceError>> + Send,
    {
        match self.request_timeout {
            Some(d) => timeout(d, fut)
                .await
                .map_err(|_| self.error_with_stderr("request timed out"))?
                .map_err(|e| self.error_with_stderr(e.to_string())),
            None => fut.await.map_err(|e| self.error_with_stderr(e.to_string())),
        }
    }

    async fn ensure_running(&self) -> Result<(), McpError> {
        let is_running = self.service.lock().await.is_some();
        if is_running {
            Ok(())
        } else {
            Err(McpError::protocol("MCP client already shut down"))
        }
    }
}

#[async_trait]
impl McpClient for RmcpClient {
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpError> {
        let request = ClientRequest::from(ListToolsRequest::default());
        self.ensure_running().await?;
        let result = self
            .with_request_timeout(self.peer.send_request(request))
            .await?;
        match result {
            ServerResult::ListToolsResult(result) => {
                Ok(result.tools.into_iter().map(Into::into).collect())
            }
            other => Err(McpError::protocol(format!(
                "unexpected response to tools/list: {other:?}"
            ))
            .with_stderr_tail(self.stderr_tail())),
        }
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> Result<McpToolResponse, McpError> {
        let args = match arguments {
            Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };
        let params = CallToolRequestParams::new(name.to_owned()).with_arguments(args);
        let request = ClientRequest::from(CallToolRequest::new(params));
        self.ensure_running().await?;
        let result = self
            .with_request_timeout(self.peer.send_request(request))
            .await?;
        match result {
            ServerResult::CallToolResult(result) => Ok(result.into()),
            other => Err(McpError::protocol(format!(
                "unexpected response to tools/call: {other:?}"
            ))
            .with_stderr_tail(self.stderr_tail())),
        }
    }

    async fn list_resources(&self) -> Result<Vec<McpResourceDefinition>, McpError> {
        let request = ClientRequest::from(ListResourcesRequest::default());
        self.ensure_running().await?;
        let result = self
            .with_request_timeout(self.peer.send_request(request))
            .await?;
        match result {
            ServerResult::ListResourcesResult(result) => {
                Ok(result.resources.into_iter().map(Into::into).collect())
            }
            other => Err(McpError::protocol(format!(
                "unexpected response to resources/list: {other:?}"
            ))
            .with_stderr_tail(self.stderr_tail())),
        }
    }

    async fn read_resource(&self, uri: &str) -> Result<McpResourceRead, McpError> {
        let request = ClientRequest::from(ReadResourceRequest::new(
            ReadResourceRequestParams::new(uri),
        ));
        self.ensure_running().await?;
        let result = self
            .with_request_timeout(self.peer.send_request(request))
            .await?;
        match result {
            ServerResult::ReadResourceResult(result) => Ok(result.into()),
            other => Err(McpError::protocol(format!(
                "unexpected response to resources/read: {other:?}"
            ))
            .with_stderr_tail(self.stderr_tail())),
        }
    }

    async fn shutdown(&self) -> Result<(), McpError> {
        let service = {
            let mut guard = self.service.lock().await;
            guard.take()
        };
        if let Some(service) = service {
            let duration = self.request_timeout.unwrap_or(Duration::from_secs(30));
            timeout(duration, service.cancel())
                .await
                .map_err(|_| self.error_with_stderr("MCP client shutdown timed out"))?
                .map_err(|e| self.error_with_stderr(e.to_string()))?;
        }
        Ok(())
    }

    fn stderr_tail(&self) -> Option<Vec<u8>> {
        self.stderr_tail.as_ref().map(|tail| {
            tail.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .snapshot()
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex as StdMutex},
        time::Duration,
    };

    use rmcp::{
        ServerHandler, ServiceExt,
        model::{CallToolRequestParams, CallToolResult},
        service::{RequestContext, RoleServer},
    };
    use tokio::sync::Notify;

    use super::{BoundedByteTail, McpClient, RmcpClient};

    #[derive(Clone)]
    struct HangingServer {
        request_started: Arc<Notify>,
    }

    impl ServerHandler for HangingServer {
        async fn call_tool(
            &self,
            _request: CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<CallToolResult, rmcp::ErrorData> {
            self.request_started.notify_one();
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn pending_request_does_not_hold_shutdown_ownership_lock() {
        let request_started = Arc::new(Notify::new());
        let (server_transport, client_transport) = tokio::io::duplex(4096);
        let server = HangingServer {
            request_started: Arc::clone(&request_started),
        };
        let mut server_task = tokio::spawn(async move {
            let service = server.serve(server_transport).await.expect("server starts");
            service.waiting().await.expect("server task joins");
        });
        let service = ().serve(client_transport).await.expect("client starts");
        let client = Arc::new(RmcpClient::new(service, None, None));

        let pending_client = Arc::clone(&client);
        let pending_call = tokio::spawn(async move {
            pending_client
                .call_tool("hang", serde_json::json!({}))
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), request_started.notified())
            .await
            .expect("server must receive the pending request");

        tokio::time::timeout(Duration::from_millis(200), client.shutdown())
            .await
            .expect("shutdown must acquire service ownership while a request is pending")
            .expect("shutdown succeeds");
        assert!(
            pending_call.await.expect("call task joins").is_err(),
            "cancelling the service must fail the pending request"
        );
        let post_shutdown_error = client
            .list_tools()
            .await
            .expect_err("requests after shutdown must fail");
        assert_eq!(
            post_shutdown_error.to_string(),
            "MCP client already shut down"
        );

        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut server_task)
                .await
                .is_err(),
            "rmcp keeps the server alive while draining the hanging handler"
        );
        server_task.abort();
        assert!(
            server_task
                .await
                .expect_err("server task was aborted")
                .is_cancelled(),
            "server task must finish through cancellation"
        );
    }

    #[tokio::test]
    async fn unexpected_close_error_includes_stderr_tail() {
        let (server_transport, client_transport) = tokio::io::duplex(4096);
        let server_task = tokio::spawn(async move {
            let service = HangingServer {
                request_started: Arc::new(Notify::new()),
            }
            .serve(server_transport)
            .await
            .expect("server starts");
            service.waiting().await.expect("server task joins");
        });
        let service = ().serve(client_transport).await.expect("client starts");
        let tail = Arc::new(StdMutex::new(BoundedByteTail::default()));
        tail.lock().unwrap().push(b"server crashed");
        let client = RmcpClient::new(service, None, Some(tail));

        server_task.abort();
        assert!(
            server_task
                .await
                .expect_err("server task is aborted")
                .is_cancelled()
        );
        let error = client
            .list_tools()
            .await
            .expect_err("closed connection must fail");

        assert_eq!(error.stderr_tail(), Some(b"server crashed".as_slice()));
    }
}
