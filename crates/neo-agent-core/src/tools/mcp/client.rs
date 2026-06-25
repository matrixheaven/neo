//! MCP client trait and rmcp-backed implementation (Task 2.1).

use std::time::Duration;

use async_trait::async_trait;
use rmcp::{
    model::{
        CallToolRequest, CallToolRequestParams, ClientRequest, ListResourcesRequest,
        ListToolsRequest, ReadResourceRequest, ReadResourceRequestParams, ServerResult,
    },
    service::{RoleClient, RunningService},
};
use serde_json::Value;
use tokio::{sync::Mutex, time::timeout};

use super::{McpError, McpResourceDefinition, McpResourceRead, McpToolDefinition, McpToolResponse};

#[async_trait]
pub trait McpClient: Send + Sync {
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpError>;
    async fn call_tool(&self, name: &str, arguments: Value) -> Result<McpToolResponse, McpError>;
    async fn list_resources(&self) -> Result<Vec<McpResourceDefinition>, McpError>;
    async fn read_resource(&self, uri: &str) -> Result<McpResourceRead, McpError>;
    async fn shutdown(&self) -> Result<(), McpError>;
}

#[derive(Debug)]
pub struct RmcpClient {
    service: Mutex<Option<RunningService<RoleClient, ()>>>,
    request_timeout: Option<Duration>,
}

impl RmcpClient {
    pub fn new(service: RunningService<RoleClient, ()>, request_timeout: Option<Duration>) -> Self {
        Self {
            service: Mutex::new(Some(service)),
            request_timeout,
        }
    }

    async fn with_request_timeout<T, F>(&self, fut: F) -> Result<T, McpError>
    where
        F: std::future::Future<Output = Result<T, rmcp::service::ServiceError>> + Send,
    {
        match self.request_timeout {
            Some(d) => timeout(d, fut)
                .await
                .map_err(|_| McpError::protocol("request timed out"))?
                .map_err(|e| McpError::protocol(e.to_string())),
            None => fut.await.map_err(|e| McpError::protocol(e.to_string())),
        }
    }
}

#[async_trait]
impl McpClient for RmcpClient {
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpError> {
        let request = ClientRequest::from(ListToolsRequest::default());
        let service = self.service.lock().await;
        let service = service
            .as_ref()
            .ok_or_else(|| McpError::protocol("MCP client already shut down"))?;
        let result = self
            .with_request_timeout(service.peer().send_request(request))
            .await?;
        match result {
            ServerResult::ListToolsResult(result) => {
                Ok(result.tools.into_iter().map(Into::into).collect())
            }
            other => Err(McpError::protocol(format!(
                "unexpected response to tools/list: {other:?}"
            ))),
        }
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> Result<McpToolResponse, McpError> {
        let args = match arguments {
            Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };
        let params = CallToolRequestParams::new(name.to_owned()).with_arguments(args);
        let request = ClientRequest::from(CallToolRequest::new(params));
        let service = self.service.lock().await;
        let service = service
            .as_ref()
            .ok_or_else(|| McpError::protocol("MCP client already shut down"))?;
        let result = self
            .with_request_timeout(service.peer().send_request(request))
            .await?;
        match result {
            ServerResult::CallToolResult(result) => Ok(result.into()),
            other => Err(McpError::protocol(format!(
                "unexpected response to tools/call: {other:?}"
            ))),
        }
    }

    async fn list_resources(&self) -> Result<Vec<McpResourceDefinition>, McpError> {
        let request = ClientRequest::from(ListResourcesRequest::default());
        let service = self.service.lock().await;
        let service = service
            .as_ref()
            .ok_or_else(|| McpError::protocol("MCP client already shut down"))?;
        let result = self
            .with_request_timeout(service.peer().send_request(request))
            .await?;
        match result {
            ServerResult::ListResourcesResult(result) => {
                Ok(result.resources.into_iter().map(Into::into).collect())
            }
            other => Err(McpError::protocol(format!(
                "unexpected response to resources/list: {other:?}"
            ))),
        }
    }

    async fn read_resource(&self, uri: &str) -> Result<McpResourceRead, McpError> {
        let request = ClientRequest::from(ReadResourceRequest::new(
            ReadResourceRequestParams::new(uri),
        ));
        let service = self.service.lock().await;
        let service = service
            .as_ref()
            .ok_or_else(|| McpError::protocol("MCP client already shut down"))?;
        let result = self
            .with_request_timeout(service.peer().send_request(request))
            .await?;
        match result {
            ServerResult::ReadResourceResult(result) => Ok(result.into()),
            other => Err(McpError::protocol(format!(
                "unexpected response to resources/read: {other:?}"
            ))),
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
                .map_err(|_| McpError::protocol("MCP client shutdown timed out"))?
                .map_err(|e| McpError::protocol(e.to_string()))?;
        }
        Ok(())
    }
}
