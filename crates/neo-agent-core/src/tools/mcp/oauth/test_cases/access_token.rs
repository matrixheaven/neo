use super::*;
use crate::tools::mcp::oauth::McpOAuthClientRecord;
use crate::tools::mcp::oauth::McpOAuthError;
use crate::tools::mcp::oauth::McpOAuthIdentity;
use crate::tools::mcp::oauth::McpOAuthTokenRecord;
use crate::tools::mcp::oauth::McpOAuthTransportKind;
use crate::tools::mcp::oauth::store::McpOAuthDiscoveryRecord;

fn identity() -> McpOAuthIdentity {
    McpOAuthIdentity::new(
        "linear",
        "https://mcp.example.com/sse?workspace=neo",
        McpOAuthTransportKind::Sse,
    )
    .unwrap()
}

fn token_record(access_token: &str) -> McpOAuthTokenRecord {
    McpOAuthTokenRecord {
        access_token: access_token.to_owned(),
        token_type: Some("Bearer".to_owned()),
        refresh_token: Some("refresh-token".to_owned()),
        expires_in: Some(3600),
        token_received_at: unix_now_secs(),
        granted_scopes: vec!["read".to_owned(), "write".to_owned()],
        raw: serde_json::json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "refresh_token": "refresh-token",
            "expires_in": 3600,
            "scope": "read write"
        }),
    }
}

fn client_record() -> McpOAuthClientRecord {
    McpOAuthClientRecord {
        client_id: "client-id".to_owned(),
        client_secret: Some("client-secret".to_owned()),
        redirect_uris: vec!["http://127.0.0.1:14500/callback".to_owned()],
        token_endpoint_auth_method: Some("client_secret_post".to_owned()),
        raw: serde_json::json!({"client_id": "client-id"}),
    }
}

fn authorization_metadata_json(token_endpoint: &str) -> serde_json::Value {
    serde_json::json!({
        "authorization_endpoint": "https://auth.example.com/authorize",
        "token_endpoint": token_endpoint,
        "registration_endpoint": "https://auth.example.com/register",
        "issuer": "https://auth.example.com",
        "scopes_supported": ["read", "write"],
        "response_types_supported": ["code"],
        "code_challenge_methods_supported": ["S256"]
    })
}

fn service() -> (tempfile::TempDir, McpOAuthService, McpOAuthIdentity) {
    let dir = tempfile::tempdir().unwrap();
    let store = McpOAuthStore::new(dir.path().join("credentials").join("mcp"));
    let service = McpOAuthService::from_store(store);
    (dir, service, identity())
}

async fn token_endpoint(response: &'static str) -> (String, tokio::task::JoinHandle<String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/token", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0; 1024];
        loop {
            let n = stream.read(&mut chunk).await.unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            let request = String::from_utf8_lossy(&buf);
            let header_end = request.find("\r\n\r\n");
            let content_length = request
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .map(str::trim)
                .and_then(|value| value.parse::<usize>().ok())
                .or_else(|| {
                    request
                        .lines()
                        .find_map(|line| line.strip_prefix("Content-Length: "))
                        .map(str::trim)
                        .and_then(|value| value.parse::<usize>().ok())
                });
            if let (Some(header_end), Some(content_length)) = (header_end, content_length)
                && buf.len() >= header_end + 4 + content_length
            {
                break;
            }
        }
        let request = String::from_utf8_lossy(&buf).into_owned();
        let body = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            response.len(),
            response
        );
        stream.write_all(body.as_bytes()).await.unwrap();
        request
    });
    (url, handle)
}

#[tokio::test]
async fn access_token_without_tokens_returns_none() {
    let (_dir, service, identity) = service();

    assert_eq!(service.access_token(&identity).await.unwrap(), None);
    assert!(!service.has_tokens(&identity));
}

#[tokio::test]
async fn access_token_returns_fresh_token() {
    let (_dir, service, identity) = service();
    service
        .store()
        .save_tokens(&identity, &token_record("fresh-token"))
        .unwrap();

    assert_eq!(
        service.access_token(&identity).await.unwrap(),
        Some("fresh-token".to_owned())
    );
    assert!(service.has_tokens(&identity));
}

#[tokio::test]
async fn missing_refresh_token_still_needs_auth() {
    let (_dir, service, identity) = service();
    let mut tokens = token_record("expired-token");
    tokens.refresh_token = None;
    tokens.expires_in = Some(1);
    tokens.token_received_at = unix_now_secs().saturating_sub(120);
    service.store().save_tokens(&identity, &tokens).unwrap();

    let err = service.access_token(&identity).await.unwrap_err();

    assert!(
        matches!(err, McpOAuthError::NeedsAuth(message) if message == "access token expired and no refresh token is available")
    );
}

#[tokio::test]
async fn store_failure_during_oauth_refresh_is_not_needs_auth() {
    let (_dir, service, identity) = service();
    let mut tokens = token_record("expired-token");
    tokens.expires_in = Some(1);
    tokens.token_received_at = unix_now_secs().saturating_sub(120);
    service.store().save_tokens(&identity, &tokens).unwrap();
    service
        .store()
        .save_client(&identity, &client_record())
        .unwrap();
    service
        .store()
        .save_discovery(
            &identity,
            &McpOAuthDiscoveryRecord {
                authorization_server_metadata: serde_json::json!({"not": "authorization-metadata"}),
                discovered_at: "2026-06-29T00:00:00Z".to_owned(),
            },
        )
        .unwrap();

    let err = service.access_token(&identity).await.unwrap_err();

    assert!(
        matches!(err, McpOAuthError::Store(ref message) if message.contains("invalid OAuth discovery metadata")),
        "store/transport failures must remain typed Store, not NeedsAuth: {err:?}"
    );
    assert!(!err.is_needs_auth());
}

#[tokio::test]
async fn access_token_stale_with_refresh_token_but_missing_client_needs_auth() {
    let (_dir, service, identity) = service();
    let mut tokens = token_record("expired-token");
    tokens.expires_in = Some(1);
    tokens.token_received_at = unix_now_secs().saturating_sub(120);
    service.store().save_tokens(&identity, &tokens).unwrap();

    let err = service.access_token(&identity).await.unwrap_err();

    assert!(
        matches!(err, McpOAuthError::NeedsAuth(message) if message == "OAuth client registration is missing")
    );
}

#[tokio::test]
async fn access_token_stale_with_client_but_missing_discovery_needs_auth() {
    let (_dir, service, identity) = service();
    let mut tokens = token_record("expired-token");
    tokens.expires_in = Some(1);
    tokens.token_received_at = unix_now_secs().saturating_sub(120);
    service.store().save_tokens(&identity, &tokens).unwrap();
    service
        .store()
        .save_client(&identity, &client_record())
        .unwrap();

    let err = service.access_token(&identity).await.unwrap_err();

    assert!(
        matches!(err, McpOAuthError::NeedsAuth(message) if message == "OAuth discovery metadata is missing")
    );
}

#[tokio::test]
async fn access_token_refreshes_stale_token_and_persists_rotated_credentials() {
    let (_dir, service, identity) = service();
    let (token_url, request) = token_endpoint(
        r#"{"access_token":"rotated-token","token_type":"Bearer","refresh_token":"rotated-refresh-token","expires_in":7200,"scope":"read write"}"#,
    )
    .await;
    let mut tokens = token_record("expired-token");
    tokens.expires_in = Some(1);
    tokens.token_received_at = unix_now_secs().saturating_sub(120);
    service.store().save_tokens(&identity, &tokens).unwrap();
    service
        .store()
        .save_client(&identity, &client_record())
        .unwrap();
    service
        .store()
        .save_discovery(
            &identity,
            &McpOAuthDiscoveryRecord {
                authorization_server_metadata: authorization_metadata_json(&token_url),
                discovered_at: unix_now_secs().to_string(),
            },
        )
        .unwrap();

    let access_token = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        service.access_token(&identity),
    )
    .await
    .expect("refresh should complete within timeout");
    if access_token.is_err() {
        request.abort();
    }
    let access_token = access_token.unwrap();

    assert_eq!(access_token, Some("rotated-token".to_owned()));
    let stored = service.store().load_tokens(&identity).unwrap().unwrap();
    assert_eq!(stored.access_token, "rotated-token");
    assert_eq!(
        stored.refresh_token.as_deref(),
        Some("rotated-refresh-token")
    );
    assert_eq!(stored.expires_in, Some(7200));
    let request = request.await.unwrap();
    assert!(request.contains("grant_type=refresh_token"));
    assert!(request.contains("refresh_token=refresh-token"));
}

#[tokio::test]
async fn access_token_refresh_preserves_existing_refresh_token_when_response_omits_one() {
    let (_dir, service, identity) = service();
    let (token_url, request) = token_endpoint(
        r#"{"access_token":"rotated-token","token_type":"Bearer","expires_in":7200,"scope":"read write"}"#,
    )
    .await;
    let mut tokens = token_record("expired-token");
    tokens.expires_in = Some(1);
    tokens.token_received_at = unix_now_secs().saturating_sub(120);
    service.store().save_tokens(&identity, &tokens).unwrap();
    service
        .store()
        .save_client(&identity, &client_record())
        .unwrap();
    service
        .store()
        .save_discovery(
            &identity,
            &McpOAuthDiscoveryRecord {
                authorization_server_metadata: authorization_metadata_json(&token_url),
                discovered_at: unix_now_secs().to_string(),
            },
        )
        .unwrap();

    let access_token = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        service.access_token(&identity),
    )
    .await
    .expect("refresh should complete within timeout")
    .unwrap();

    assert_eq!(access_token, Some("rotated-token".to_owned()));
    let stored = service.store().load_tokens(&identity).unwrap().unwrap();
    assert_eq!(stored.refresh_token.as_deref(), Some("refresh-token"));
    assert_eq!(
        stored
            .raw
            .get("refresh_token")
            .and_then(serde_json::Value::as_str),
        Some("refresh-token")
    );
    let request = request.await.unwrap();
    assert!(request.contains("refresh_token=refresh-token"));
}
