use super::*;
use crate::tools::mcp::oauth::InvalidateScope;
use crate::tools::mcp::oauth::McpOAuthClientRecord;
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

fn discovery_record() -> McpOAuthDiscoveryRecord {
    McpOAuthDiscoveryRecord {
        authorization_server_metadata: authorization_metadata_json(
            "https://auth.example.com/token",
        ),
        discovered_at: "2026-06-29T00:00:00Z".to_owned(),
    }
}

fn authorization_metadata(token_endpoint: &str) -> rmcp::transport::auth::AuthorizationMetadata {
    serde_json::from_value(authorization_metadata_json(token_endpoint)).unwrap()
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

#[tokio::test]
async fn invalidate_tokens_only_removes_only_tokens() {
    let (_dir, service, identity) = service();
    service
        .store()
        .save_tokens(&identity, &token_record("token"))
        .unwrap();
    service
        .store()
        .save_client(&identity, &client_record())
        .unwrap();
    service
        .store()
        .save_discovery(&identity, &discovery_record())
        .unwrap();

    service
        .invalidate(&identity, InvalidateScope::TokensOnly)
        .unwrap();

    assert!(service.store().load_tokens(&identity).unwrap().is_none());
    assert!(service.store().load_client(&identity).unwrap().is_some());
    assert!(service.store().load_discovery(&identity).unwrap().is_some());
}

#[tokio::test]
async fn invalidate_all_credentials_removes_credentials_and_is_idempotent() {
    let (_dir, service, identity) = service();
    service
        .store()
        .save_tokens(&identity, &token_record("token"))
        .unwrap();
    service
        .store()
        .save_client(&identity, &client_record())
        .unwrap();
    service
        .store()
        .save_discovery(&identity, &discovery_record())
        .unwrap();

    service
        .invalidate(&identity, InvalidateScope::AllCredentials)
        .unwrap();
    service
        .invalidate(&identity, InvalidateScope::AllCredentials)
        .unwrap();

    assert!(service.store().load_tokens(&identity).unwrap().is_none());
    assert!(service.store().load_client(&identity).unwrap().is_none());
    assert!(service.store().load_discovery(&identity).unwrap().is_none());
    assert!(!service.store().server_dir(&identity).exists());
    assert!(service.store().root().exists());
}

#[test]
fn persist_client_and_discovery_writes_refresh_prerequisites() {
    let (_dir, service, identity) = service();
    let config = OAuthClientConfig::new("client-id", "http://127.0.0.1:14500/callback")
        .with_client_secret("client-secret");
    let metadata = authorization_metadata("https://auth.example.com/token");

    service
        .persist_client_and_discovery(&identity, &config, metadata.clone())
        .unwrap();

    let client = service.store().load_client(&identity).unwrap().unwrap();
    assert_eq!(client.client_id, "client-id");
    assert_eq!(client.client_secret.as_deref(), Some("client-secret"));
    assert_eq!(
        client.redirect_uris,
        vec!["http://127.0.0.1:14500/callback".to_owned()]
    );
    let discovery = service.store().load_discovery(&identity).unwrap().unwrap();
    assert_eq!(
        discovery
            .authorization_server_metadata
            .get("token_endpoint")
            .and_then(serde_json::Value::as_str),
        Some(metadata.token_endpoint.as_str())
    );
}
