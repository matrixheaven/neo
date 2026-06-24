//! HTTP/SSE MCP client builder (Task 2.3).

use std::{collections::HashMap, sync::Arc, time::Duration};

use http::{HeaderName, HeaderValue};
use rmcp::{
    ServiceExt,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};

use super::{McpError, client::McpClient};

// TODO: `HttpConfig` is currently unused while the rmcp migration is in
// progress. It will be wired up through `McpConnectionManager` in Task 4.
#[derive(Debug, Clone, serde::Deserialize, Default)]
#[allow(dead_code)]
pub struct HttpConfig {
    pub url: String,
    pub headers: std::collections::BTreeMap<String, String>,
    pub startup_timeout_ms: Option<u64>,
    pub request_timeout_ms: Option<u64>,
}

// TODO: `build_http_client` is currently unused while the rmcp migration is in
// progress. It will be wired up through `McpConnectionManager` in Task 4.
#[allow(dead_code)]
pub async fn build_http_client(config: HttpConfig) -> Result<Arc<dyn McpClient>, McpError> {
    let mut custom_headers = HashMap::with_capacity(config.headers.len());
    for (k, v) in &config.headers {
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|e| McpError::protocol(format!("invalid header name {k}: {e}")))?;
        let value = HeaderValue::from_str(v)
            .map_err(|e| McpError::protocol(format!("invalid header value for {k}: {e}")))?;
        custom_headers.insert(name, value);
    }

    let transport_config = StreamableHttpClientTransportConfig::with_uri(config.url.as_str())
        .custom_headers(custom_headers);
    let transport = StreamableHttpClientTransport::from_config(transport_config);

    let startup_timeout = config.startup_timeout_ms.map(Duration::from_millis);
    let request_timeout = config.request_timeout_ms.map(Duration::from_millis);

    let service = match startup_timeout {
        Some(d) => tokio::time::timeout(d, ().serve(transport))
            .await
            .map_err(|_| McpError::protocol("HTTP MCP server initialization timed out"))?
            .map_err(|e| McpError::protocol(e.to_string()))?,
        None => ().serve(transport).await.map_err(|e| McpError::protocol(e.to_string()))?,
    };

    Ok(Arc::new(super::client::RmcpClient::new(
        service,
        request_timeout,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_config_holds_values() {
        let mut headers = std::collections::BTreeMap::new();
        headers.insert("Authorization".into(), "Bearer token".into());
        let config = HttpConfig {
            url: "http://localhost:8080/mcp".into(),
            headers,
            startup_timeout_ms: Some(5000),
            request_timeout_ms: Some(30000),
        };
        assert_eq!(config.url, "http://localhost:8080/mcp");
        assert_eq!(config.headers.len(), 1);
    }
}
