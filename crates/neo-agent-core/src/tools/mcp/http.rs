//! OAuth-aware streamable HTTP MCP client (Task 2.5).

use std::{collections::HashMap, fmt, sync::Arc, time::Duration};

use http::{HeaderName, HeaderValue};
use rmcp::{
    ServiceExt,
    transport::{
        auth::{AuthError, AuthorizationManager},
        streamable_http_client::{
            SseError, StreamableHttpClient, StreamableHttpClientTransport,
            StreamableHttpClientTransportConfig, StreamableHttpError, StreamableHttpPostResponse,
        },
    },
};
use sse_stream::Sse;
use thiserror::Error;

use super::{McpError, client::McpClient};

#[derive(Clone, serde::Deserialize, Default)]
pub struct HttpConfig {
    pub url: String,
    pub headers: std::collections::BTreeMap<String, String>,
    pub startup_timeout_ms: Option<u64>,
    pub request_timeout_ms: Option<u64>,
    #[serde(skip)]
    pub auth_manager: Option<Arc<tokio::sync::Mutex<AuthorizationManager>>>,
}

impl fmt::Debug for HttpConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpConfig")
            .field("url", &self.url)
            .field("headers", &self.headers)
            .field("startup_timeout_ms", &self.startup_timeout_ms)
            .field("request_timeout_ms", &self.request_timeout_ms)
            .field("auth_manager", &self.auth_manager.is_some())
            .finish()
    }
}

/// Error type for the OAuth-aware streamable HTTP client.
#[derive(Debug, Error)]
pub enum OAuthHttpError {
    #[error("reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("OAuth error: {0}")]
    Auth(String),
}

#[derive(Clone)]
pub struct OAuthStreamableHttpClient {
    client: reqwest::Client,
    auth_manager: Option<Arc<tokio::sync::Mutex<AuthorizationManager>>>,
}

impl fmt::Debug for OAuthStreamableHttpClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthStreamableHttpClient")
            .field("auth_manager", &self.auth_manager.is_some())
            .finish_non_exhaustive()
    }
}

impl OAuthStreamableHttpClient {
    pub fn new(
        client: reqwest::Client,
        auth_manager: Option<Arc<tokio::sync::Mutex<AuthorizationManager>>>,
    ) -> Self {
        Self {
            client,
            auth_manager,
        }
    }

    async fn auth_header(
        &self,
        custom_headers: &HashMap<HeaderName, HeaderValue>,
    ) -> Result<Option<String>, OAuthHttpError> {
        if custom_headers.contains_key(&http::header::AUTHORIZATION) {
            return Ok(None);
        }
        match &self.auth_manager {
            Some(manager) => {
                let mgr = manager.lock().await;
                match mgr.get_access_token().await {
                    Ok(token) => Ok(Some(token)),
                    Err(AuthError::AuthorizationRequired) => {
                        // AuthorizationRequired can mean either "never
                        // authorized" or "token expired and refresh failed".
                        // Use get_credentials() to distinguish: if the OAuth
                        // client is configured and a token was previously
                        // stored, this is a refresh failure that must be
                        // surfaced instead of silently sending the request
                        // unauthenticated.
                        match mgr.get_credentials().await {
                            Ok((_, Some(_))) => Err(OAuthHttpError::Auth(
                                "OAuth token expired. \
                                 Run `neo mcp auth <server_id>` to re-authenticate."
                                    .into(),
                            )),
                            // No stored credentials (or client not yet
                            // configured) — the server may not require auth,
                            // so let the request go out without an auth header.
                            _ => Ok(None),
                        }
                    }
                    // Any other error (refresh failure, network error, etc.)
                    // must be propagated so the user gets a clear message.
                    Err(e) => Err(OAuthHttpError::Auth(e.to_string())),
                }
            }
            None => Ok(None),
        }
    }
}

fn map_error(e: StreamableHttpError<reqwest::Error>) -> StreamableHttpError<OAuthHttpError> {
    match e {
        StreamableHttpError::Client(err) => {
            StreamableHttpError::Client(OAuthHttpError::Reqwest(err))
        }
        StreamableHttpError::Sse(err) => StreamableHttpError::Sse(err),
        StreamableHttpError::Io(err) => StreamableHttpError::Io(err),
        StreamableHttpError::UnexpectedEndOfStream => StreamableHttpError::UnexpectedEndOfStream,
        StreamableHttpError::UnexpectedServerResponse(msg) => {
            StreamableHttpError::UnexpectedServerResponse(msg)
        }
        StreamableHttpError::UnexpectedContentType(ct) => {
            StreamableHttpError::UnexpectedContentType(ct)
        }
        StreamableHttpError::ServerDoesNotSupportSse => {
            StreamableHttpError::ServerDoesNotSupportSse
        }
        StreamableHttpError::ServerDoesNotSupportDeleteSession => {
            StreamableHttpError::ServerDoesNotSupportDeleteSession
        }
        StreamableHttpError::TokioJoinError(err) => StreamableHttpError::TokioJoinError(err),
        StreamableHttpError::Deserialize(err) => StreamableHttpError::Deserialize(err),
        StreamableHttpError::TransportChannelClosed => StreamableHttpError::TransportChannelClosed,
        StreamableHttpError::MissingSessionIdInResponse => {
            StreamableHttpError::MissingSessionIdInResponse
        }
        StreamableHttpError::Auth(err) => StreamableHttpError::Auth(err),
        StreamableHttpError::AuthRequired(err) => StreamableHttpError::AuthRequired(err),
        StreamableHttpError::InsufficientScope(err) => StreamableHttpError::InsufficientScope(err),
        StreamableHttpError::ReservedHeaderConflict(name) => {
            StreamableHttpError::ReservedHeaderConflict(name)
        }
        StreamableHttpError::SessionExpired => StreamableHttpError::SessionExpired,
        other => StreamableHttpError::UnexpectedServerResponse(
            format!("unknown streamable HTTP error: {other:?}").into(),
        ),
    }
}

impl StreamableHttpClient for OAuthStreamableHttpClient {
    type Error = OAuthHttpError;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: rmcp::model::ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let auth_header = match auth_header {
            Some(h) => Some(h),
            None => self
                .auth_header(&custom_headers)
                .await
                .map_err(StreamableHttpError::Client)?,
        };
        <reqwest::Client as StreamableHttpClient>::post_message(
            &self.client,
            uri,
            message,
            session_id,
            auth_header,
            custom_headers,
        )
        .await
        .map_err(map_error)
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let auth_header = match auth_header {
            Some(h) => Some(h),
            None => self
                .auth_header(&custom_headers)
                .await
                .map_err(StreamableHttpError::Client)?,
        };
        <reqwest::Client as StreamableHttpClient>::delete_session(
            &self.client,
            uri,
            session_id,
            auth_header,
            custom_headers,
        )
        .await
        .map_err(map_error)
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<Sse, SseError>>,
        StreamableHttpError<Self::Error>,
    > {
        let auth_header = match auth_header {
            Some(h) => Some(h),
            None => self
                .auth_header(&custom_headers)
                .await
                .map_err(StreamableHttpError::Client)?,
        };
        <reqwest::Client as StreamableHttpClient>::get_stream(
            &self.client,
            uri,
            session_id,
            last_event_id,
            auth_header,
            custom_headers,
        )
        .await
        .map_err(map_error)
    }
}

pub async fn build_http_client(config: HttpConfig) -> Result<Arc<dyn McpClient>, McpError> {
    let mut custom_headers = HashMap::with_capacity(config.headers.len());
    for (k, v) in &config.headers {
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|e| McpError::protocol(format!("invalid header name {k}: {e}")))?;
        let value = HeaderValue::from_str(v)
            .map_err(|e| McpError::protocol(format!("invalid header value for {k}: {e}")))?;
        custom_headers.insert(name, value);
    }

    let oauth_client = OAuthStreamableHttpClient::new(reqwest::Client::new(), config.auth_manager);

    let mut transport_config = StreamableHttpClientTransportConfig::with_uri(config.url.as_str())
        .custom_headers(custom_headers)
        .reinit_on_expired_session(true);
    transport_config.allow_stateless = true;
    let transport = StreamableHttpClientTransport::with_client(oauth_client, transport_config);

    let startup_timeout = config.startup_timeout_ms.map(Duration::from_millis);
    let request_timeout = config.request_timeout_ms.map(Duration::from_millis);

    let service = match startup_timeout {
        Some(d) => tokio::time::timeout(d, ().serve(transport))
            .await
            .map_err(|_| McpError::protocol("HTTP MCP server initialization timed out"))?
            .map_err(friendly_http_init_error)?,
        None => ().serve(transport).await.map_err(friendly_http_init_error)?,
    };

    Ok(Arc::new(super::client::RmcpClient::new(
        service,
        request_timeout,
    )))
}

/// Convert rmcp initialization errors into user-friendly messages.
///
/// rmcp's raw error `Display` output is verbose and leaks internal type names
/// (`StreamableHttpError`, `AuthRequiredError`, etc.) which are confusing in
/// the TUI.  This helper detects common failure patterns and returns a clear,
/// actionable message instead.
fn friendly_http_init_error(err: rmcp::service::ClientInitializeError) -> McpError {
    let display = err.to_string();

    // OAuth required — the most common case for remote MCP servers.
    if display.contains("AuthRequired") || display.contains("auth_required") {
        return McpError::protocol(
            "Server requires OAuth authorization. Run `neo mcp auth <server_id>` to authenticate.",
        );
    }

    // Connection refused / unreachable host.
    if display.contains("ConnectionClosed") || display.contains("connection closed") {
        return McpError::protocol(format!(
            "Could not connect to MCP server. Check that the URL is reachable. ({display})"
        ));
    }

    // Fallback: include the raw error for less common failures.
    McpError::protocol(display)
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
            auth_manager: None,
        };
        assert_eq!(config.url, "http://localhost:8080/mcp");
        assert_eq!(config.headers.len(), 1);
    }
}
