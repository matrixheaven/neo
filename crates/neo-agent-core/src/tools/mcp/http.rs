//! OAuth-aware streamable HTTP MCP client (Task 2.5).

#![allow(dead_code)]

use std::{borrow::Cow, collections::HashMap, sync::Arc, time::Duration};

use futures::{StreamExt, stream::BoxStream};
use http::{HeaderName, HeaderValue};
use reqwest::header::{ACCEPT, CONTENT_TYPE, WWW_AUTHENTICATE};
use rmcp::{
    ServiceExt,
    transport::{
        auth::AuthorizationManager,
        common::http_header::{
            EVENT_STREAM_MIME_TYPE, HEADER_LAST_EVENT_ID, HEADER_SESSION_ID, JSON_MIME_TYPE,
        },
        streamable_http_client::{
            AuthRequiredError, InsufficientScopeError, SseError, StreamableHttpClient,
            StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
            StreamableHttpError, StreamableHttpPostResponse,
        },
    },
};
use sse_stream::{Sse, SseStream};
use thiserror::Error;

use super::{McpError, client::McpClient};

#[derive(Debug, Clone, serde::Deserialize, Default)]
#[allow(dead_code)]
pub struct HttpConfig {
    pub url: String,
    pub headers: std::collections::BTreeMap<String, String>,
    pub startup_timeout_ms: Option<u64>,
    pub request_timeout_ms: Option<u64>,
}

/// Error type for the OAuth-aware streamable HTTP client.
#[derive(Debug, Error)]
pub enum OAuthHttpError {
    #[error("HTTP client error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("OAuth authorization error: {0}")]
    Auth(String),
}

impl From<rmcp::transport::auth::AuthError> for OAuthHttpError {
    fn from(err: rmcp::transport::auth::AuthError) -> Self {
        Self::Auth(err.to_string())
    }
}

/// Custom [`StreamableHttpClient`] that dynamically injects a Bearer token by
/// calling an [`AuthorizationManager`] on every request.
#[derive(Clone)]
pub struct OAuthStreamableHttpClient {
    client: reqwest::Client,
    auth_manager: Option<Arc<tokio::sync::Mutex<AuthorizationManager>>>,
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

    async fn resolve_token(
        &self,
        auth_header: Option<String>,
        custom_headers: &HashMap<HeaderName, HeaderValue>,
    ) -> Result<Option<String>, StreamableHttpError<OAuthHttpError>> {
        let has_custom_auth = custom_headers
            .keys()
            .any(|name| name.as_str().eq_ignore_ascii_case("authorization"));
        if has_custom_auth {
            return Ok(None);
        }
        if let Some(token) = auth_header {
            return Ok(Some(token));
        }
        if let Some(manager) = &self.auth_manager {
            let token = manager
                .lock()
                .await
                .get_access_token()
                .await
                .map_err(|e| StreamableHttpError::Client(OAuthHttpError::Auth(e.to_string())))?;
            return Ok(Some(token));
        }
        Ok(None)
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
        let mut request = self
            .client
            .post(uri.as_ref())
            .header(ACCEPT, [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "));

        if let Some(token) = self.resolve_token(auth_header, &custom_headers).await? {
            request = request.bearer_auth(token);
        }

        request = apply_custom_headers(request, custom_headers)?;

        let session_was_attached = session_id.is_some();
        if let Some(session_id) = session_id {
            request = request.header(HEADER_SESSION_ID, session_id.as_ref());
        }

        let response = request
            .json(&message)
            .send()
            .await
            .map_err(|e| StreamableHttpError::Client(OAuthHttpError::Http(e)))?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            if let Some(header) = response.headers().get(WWW_AUTHENTICATE) {
                let header = header
                    .to_str()
                    .map_err(|_| {
                        StreamableHttpError::UnexpectedServerResponse(Cow::from(
                            "invalid www-authenticate header value",
                        ))
                    })?
                    .to_string();
                return Err(StreamableHttpError::AuthRequired(AuthRequiredError::new(
                    header,
                )));
            }
        }

        if response.status() == reqwest::StatusCode::FORBIDDEN {
            if let Some(header) = response.headers().get(WWW_AUTHENTICATE) {
                let header_str = header.to_str().map_err(|_| {
                    StreamableHttpError::UnexpectedServerResponse(Cow::from(
                        "invalid www-authenticate header value",
                    ))
                })?;
                let scope = extract_scope_from_header(header_str);
                return Err(StreamableHttpError::InsufficientScope(
                    InsufficientScopeError::new(header_str.to_string(), scope),
                ));
            }
        }

        let status = response.status();
        if matches!(
            status,
            reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT
        ) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if status == reqwest::StatusCode::NOT_FOUND && session_was_attached {
            return Err(StreamableHttpError::SessionExpired);
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .map(|ct| String::from_utf8_lossy(ct.as_bytes()).to_string());
        let content_length = response.content_length();
        let session_id = response
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        if status.is_success()
            && content_length == Some(0)
            && matches!(
                message,
                rmcp::model::ClientJsonRpcMessage::Notification(_)
                    | rmcp::model::ClientJsonRpcMessage::Response(_)
                    | rmcp::model::ClientJsonRpcMessage::Error(_)
            )
        {
            return Ok(StreamableHttpPostResponse::Accepted);
        }

        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_owned());
            if content_type
                .as_deref()
                .is_some_and(|ct| ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes()))
            {
                match parse_json_rpc_error(&body) {
                    Some(message) => {
                        return Ok(StreamableHttpPostResponse::Json(message, session_id));
                    }
                    None => tracing::warn!(
                        "HTTP {status}: could not parse JSON body as a JSON-RPC error"
                    ),
                }
            }
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("HTTP {status}: {body}"),
            )));
        }

        match content_type.as_deref() {
            Some(ct) if ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes()) => {
                let event_stream =
                    SseStream::from_byte_stream(response.bytes_stream()).boxed();
                Ok(StreamableHttpPostResponse::Sse(event_stream, session_id))
            }
            Some(ct) if ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes()) => {
                match response
                    .json::<rmcp::model::ServerJsonRpcMessage>()
                    .await
                    .map_err(|e| StreamableHttpError::Client(OAuthHttpError::Http(e)))
                {
                    Ok(message) => Ok(StreamableHttpPostResponse::Json(message, session_id)),
                    Err(e) => {
                        tracing::warn!(
                            "could not parse JSON response as ServerJsonRpcMessage, treating as accepted: {e}"
                        );
                        Ok(StreamableHttpPostResponse::Accepted)
                    }
                }
            }
            _ => {
                tracing::error!("unexpected content type: {:?}", content_type);
                Err(StreamableHttpError::UnexpectedContentType(content_type))
            }
        }
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let mut request = self.client.delete(uri.as_ref());

        if let Some(token) = self.resolve_token(auth_header, &custom_headers).await? {
            request = request.bearer_auth(token);
        }

        request = apply_custom_headers(request, custom_headers)?;
        request = request.header(HEADER_SESSION_ID, session_id.as_ref());

        let response = request
            .send()
            .await
            .map_err(|e| StreamableHttpError::Client(OAuthHttpError::Http(e)))?;

        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportDeleteSession);
        }
        let _response = response
            .error_for_status()
            .map_err(|e| StreamableHttpError::Client(OAuthHttpError::Http(e)))?;
        Ok(())
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        let mut request = self
            .client
            .get(uri.as_ref())
            .header(ACCEPT, EVENT_STREAM_MIME_TYPE)
            .header(HEADER_SESSION_ID, session_id.as_ref());

        if let Some(last_event_id) = last_event_id {
            request = request.header(HEADER_LAST_EVENT_ID, last_event_id);
        }

        if let Some(token) = self.resolve_token(auth_header, &custom_headers).await? {
            request = request.bearer_auth(token);
        }

        request = apply_custom_headers(request, custom_headers)?;

        let response = request
            .send()
            .await
            .map_err(|e| StreamableHttpError::Client(OAuthHttpError::Http(e)))?;

        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        let response = response
            .error_for_status()
            .map_err(|e| StreamableHttpError::Client(OAuthHttpError::Http(e)))?;

        match response.headers().get(CONTENT_TYPE) {
            Some(ct) => {
                if !ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes())
                    && !ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes())
                {
                    return Err(StreamableHttpError::UnexpectedContentType(Some(
                        String::from_utf8_lossy(ct.as_bytes()).to_string(),
                    )));
                }
            }
            None => {
                return Err(StreamableHttpError::UnexpectedContentType(None));
            }
        }

        let event_stream = SseStream::from_byte_stream(response.bytes_stream()).boxed();
        Ok(event_stream)
    }
}

fn apply_custom_headers(
    mut builder: reqwest::RequestBuilder,
    custom_headers: HashMap<HeaderName, HeaderValue>,
) -> Result<reqwest::RequestBuilder, StreamableHttpError<OAuthHttpError>> {
    for (name, value) in custom_headers {
        validate_custom_header(&name)?;
        builder = builder.header(name, value);
    }
    Ok(builder)
}

fn validate_custom_header(
    name: &HeaderName,
) -> Result<(), StreamableHttpError<OAuthHttpError>> {
    let name_str = name.as_str();
    if name_str.eq_ignore_ascii_case("accept")
        || name_str.eq_ignore_ascii_case(HEADER_SESSION_ID)
        || name_str.eq_ignore_ascii_case(HEADER_LAST_EVENT_ID)
    {
        return Err(StreamableHttpError::ReservedHeaderConflict(name.to_string()));
    }
    Ok(())
}

fn parse_json_rpc_error(body: &str) -> Option<rmcp::model::ServerJsonRpcMessage> {
    match serde_json::from_str::<rmcp::model::ServerJsonRpcMessage>(body) {
        Ok(message @ rmcp::model::JsonRpcMessage::Error(_)) => Some(message),
        _ => None,
    }
}

fn extract_scope_from_header(header: &str) -> Option<String> {
    let header_lowercase = header.to_ascii_lowercase();
    let scope_key = "scope=";

    if let Some(pos) = header_lowercase.find(scope_key) {
        let start = pos + scope_key.len();
        let value_slice = &header[start..];

        if let Some(stripped) = value_slice.strip_prefix('"') {
            if let Some(end_quote) = stripped.find('"') {
                return Some(stripped[..end_quote].to_string());
            }
        } else {
            let end = value_slice
                .find(|c: char| c == ',' || c == ';' || c.is_whitespace())
                .unwrap_or(value_slice.len());
            if end > 0 {
                return Some(value_slice[..end].to_string());
            }
        }
    }

    None
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

    let oauth_client = OAuthStreamableHttpClient::new(reqwest::Client::new(), None);

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
