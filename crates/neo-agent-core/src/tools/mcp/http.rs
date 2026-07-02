//! OAuth-aware streamable HTTP MCP client.

use std::{collections::HashMap, fmt, sync::Arc, time::Duration};

use http::{HeaderName, HeaderValue};
use rmcp::{
    ServiceExt,
    transport::streamable_http_client::{
        SseError, StreamableHttpClient, StreamableHttpClientTransport,
        StreamableHttpClientTransportConfig, StreamableHttpError, StreamableHttpPostResponse,
    },
};
use sse_stream::Sse;
use thiserror::Error;

use super::{
    McpError,
    client::McpClient,
    oauth::{McpOAuthError, McpOAuthIdentity, McpOAuthService},
};

#[derive(Clone)]
pub struct HttpOAuthConfig {
    pub service: McpOAuthService,
    pub identity: McpOAuthIdentity,
}

#[derive(Clone, serde::Deserialize, Default)]
pub struct HttpConfig {
    pub url: String,
    pub headers: std::collections::BTreeMap<String, String>,
    pub startup_timeout_ms: Option<u64>,
    pub request_timeout_ms: Option<u64>,
    #[serde(skip)]
    pub oauth: Option<HttpOAuthConfig>,
}

impl fmt::Debug for HttpConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let header_keys: Vec<&String> = self.headers.keys().collect();
        f.debug_struct("HttpConfig")
            .field("url", &self.url)
            .field("header_keys", &header_keys)
            .field("startup_timeout_ms", &self.startup_timeout_ms)
            .field("request_timeout_ms", &self.request_timeout_ms)
            .field("oauth", &self.oauth.is_some())
            .finish()
    }
}

/// Error type for the OAuth-aware streamable HTTP client.
#[derive(Debug, Error)]
pub enum OAuthHttpError {
    #[error("reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("OAuth required: {0}")]
    NeedsAuth(String),
    #[error("OAuth error: {0}")]
    Auth(String),
}

#[derive(Clone)]
pub struct OAuthStreamableHttpClient {
    client: reqwest::Client,
    oauth: Option<HttpOAuthConfig>,
}

impl fmt::Debug for OAuthStreamableHttpClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthStreamableHttpClient")
            .field("oauth", &self.oauth.is_some())
            .finish_non_exhaustive()
    }
}

impl OAuthStreamableHttpClient {
    pub fn new(client: reqwest::Client, oauth: Option<HttpOAuthConfig>) -> Self {
        Self { client, oauth }
    }

    async fn auth_header(
        &self,
        custom_headers: &HashMap<HeaderName, HeaderValue>,
    ) -> Result<Option<String>, OAuthHttpError> {
        if custom_headers.contains_key(&http::header::AUTHORIZATION) {
            return Ok(None);
        }
        let Some(oauth) = &self.oauth else {
            return Ok(None);
        };

        oauth
            .service
            .access_token(&oauth.identity)
            .await
            .map_err(oauth_error_to_http)
    }
}

#[allow(clippy::needless_pass_by_value)]
fn oauth_error_to_http(err: McpOAuthError) -> OAuthHttpError {
    if err.is_needs_auth() {
        OAuthHttpError::NeedsAuth(err.to_string())
    } else {
        OAuthHttpError::Auth(err.to_string())
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

    let oauth_client = OAuthStreamableHttpClient::new(reqwest::Client::new(), config.oauth);

    let mut transport_config = StreamableHttpClientTransportConfig::with_uri(config.url.as_str())
        .custom_headers(custom_headers)
        .reinit_on_expired_session(true);
    transport_config.allow_stateless = true;
    let transport = StreamableHttpClientTransport::with_client(oauth_client, transport_config);

    let startup_timeout = Duration::from_millis(config.startup_timeout_ms.unwrap_or(5_000));
    let request_timeout = config.request_timeout_ms.map(Duration::from_millis);

    let service = tokio::time::timeout(startup_timeout, ().serve(transport))
        .await
        .map_err(|_| McpError::protocol("HTTP MCP server initialization timed out"))?
        .map_err(friendly_http_init_error)?;

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
#[allow(clippy::needless_pass_by_value)]
fn friendly_http_init_error(err: rmcp::service::ClientInitializeError) -> McpError {
    let display = err.to_string();

    // OAuth required — the most common case for remote MCP servers.
    if display.contains("AuthRequired")
        || display.contains("AuthRequiredError")
        || display.contains("Auth required")
        || display.contains("auth_required")
        || display.contains("401")
        || display.contains("Unauthorized")
    {
        return McpError::needs_auth(
            "Server requires OAuth authorization. Run /mcp-config login <server_id> to authenticate.",
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
    use super::super::McpErrorKind;
    use super::super::oauth::{
        McpOAuthIdentity, McpOAuthService, McpOAuthStore, McpOAuthTokenRecord,
        McpOAuthTransportKind,
    };
    use super::*;

    #[test]
    fn http_config_debug_reports_oauth_boolean_without_tokens() {
        let mut headers = std::collections::BTreeMap::new();
        headers.insert("Authorization".into(), "Bearer token".into());
        let config = HttpConfig {
            url: "http://localhost:8080/mcp".into(),
            headers,
            startup_timeout_ms: Some(5000),
            request_timeout_ms: Some(30000),
            oauth: None,
        };
        assert_eq!(config.url, "http://localhost:8080/mcp");
        assert_eq!(config.headers.len(), 1);

        let debug = format!("{config:?}");
        assert!(debug.contains("oauth: false"));
        assert!(debug.contains("header_keys"));
        assert!(debug.contains("Authorization"));
        assert!(!debug.contains("Bearer token"));
        assert!(!debug.contains("headers"));
        assert!(!debug.contains("auth_manager"));
    }

    #[test]
    fn http_config_debug_hides_oauth_token_values() {
        let (_dir, oauth) = oauth_config("secret-access-token");
        let config = HttpConfig {
            url: "http://localhost:8080/mcp".into(),
            headers: std::collections::BTreeMap::new(),
            startup_timeout_ms: None,
            request_timeout_ms: None,
            oauth: Some(oauth),
        };

        let debug = format!("{config:?}");
        assert!(debug.contains("oauth: true"));
        assert!(!debug.contains("secret-access-token"));
    }

    #[test]
    fn friendly_http_init_error_maps_auth_required_to_needs_auth() {
        let err = friendly_http_init_error(rmcp::service::ClientInitializeError::ConnectionClosed(
            "AuthRequiredError: auth_required 401 Unauthorized".to_owned(),
        ));

        assert_eq!(err.kind(), McpErrorKind::NeedsAuth);
        assert!(err.is_needs_auth());
        assert_eq!(
            err.message(),
            "Server requires OAuth authorization. Run /mcp-config login <server_id> to authenticate."
        );
    }

    #[test]
    fn friendly_http_init_error_maps_rmcp_auth_required_text_to_needs_auth() {
        let err = friendly_http_init_error(rmcp::service::ClientInitializeError::ConnectionClosed(
            "Send message error Transport [rmcp::transport::worker::WorkerTransport<rmcp::transport::streamable_http_client::StreamableHttpClientWorker<neo_agent_core::tools::mcp::http::OAuthStreamableHttpClient>>] error: Auth required, when send initialize request".to_owned(),
        ));

        assert_eq!(err.kind(), McpErrorKind::NeedsAuth);
        assert!(err.is_needs_auth());
    }

    #[test]
    fn friendly_http_init_error_keeps_non_auth_errors_as_protocol() {
        let err = friendly_http_init_error(rmcp::service::ClientInitializeError::ConnectionClosed(
            "server closed connection".to_owned(),
        ));

        assert_eq!(err.kind(), McpErrorKind::Protocol);
        assert!(!err.is_needs_auth());
    }

    #[tokio::test]
    async fn auth_header_returns_none_when_custom_authorization_exists() {
        let (_dir, oauth) = oauth_config("secret-access-token");
        let client = OAuthStreamableHttpClient::new(reqwest::Client::new(), Some(oauth));
        let mut headers = HashMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer custom"),
        );

        assert_eq!(client.auth_header(&headers).await.unwrap(), None);
    }

    #[tokio::test]
    async fn auth_header_returns_none_without_oauth_config() {
        let client = OAuthStreamableHttpClient::new(reqwest::Client::new(), None);

        assert_eq!(client.auth_header(&HashMap::new()).await.unwrap(), None);
    }

    #[tokio::test]
    async fn auth_header_maps_missing_refresh_to_needs_auth() {
        let (_dir, oauth) = oauth_config_with_expired_token_without_refresh();
        let client = OAuthStreamableHttpClient::new(reqwest::Client::new(), Some(oauth));

        let err = client.auth_header(&HashMap::new()).await.unwrap_err();

        assert!(
            matches!(err, OAuthHttpError::NeedsAuth(message) if message.contains("access token expired"))
        );
    }

    fn oauth_config(access_token: &str) -> (tempfile::TempDir, HttpOAuthConfig) {
        let dir = tempfile::tempdir().unwrap();
        let identity = McpOAuthIdentity::new(
            "linear",
            "https://mcp.example.com/sse",
            McpOAuthTransportKind::Http,
        )
        .unwrap();
        let service = McpOAuthService::from_store(McpOAuthStore::new(dir.path().join("mcp")));
        service
            .store()
            .save_tokens(
                &identity,
                &McpOAuthTokenRecord {
                    access_token: access_token.to_owned(),
                    token_type: Some("Bearer".to_owned()),
                    refresh_token: None,
                    expires_in: Some(3600),
                    token_received_at: now_seconds(),
                    granted_scopes: Vec::new(),
                    raw: serde_json::json!({ "access_token": access_token }),
                },
            )
            .unwrap();
        (dir, HttpOAuthConfig { service, identity })
    }

    fn oauth_config_with_expired_token_without_refresh() -> (tempfile::TempDir, HttpOAuthConfig) {
        let (dir, oauth) = oauth_config("expired-token");
        oauth
            .service
            .store()
            .save_tokens(
                &oauth.identity,
                &McpOAuthTokenRecord {
                    access_token: "expired-token".to_owned(),
                    token_type: Some("Bearer".to_owned()),
                    refresh_token: None,
                    expires_in: Some(1),
                    token_received_at: 0,
                    granted_scopes: Vec::new(),
                    raw: serde_json::json!({ "access_token": "expired-token" }),
                },
            )
            .unwrap();
        (dir, oauth)
    }

    fn now_seconds() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }
}
