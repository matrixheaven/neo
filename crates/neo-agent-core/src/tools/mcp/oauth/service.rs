use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use rmcp::transport::auth::{
    AuthError, AuthorizationManager, CredentialStore, OAuthClientConfig, StoredCredentials,
};
use tokio::sync::Mutex;

use super::{
    InMemoryStateStore, InvalidateScope, McpOAuthClientRecord, McpOAuthDiscoveryRecord,
    McpOAuthError, McpOAuthFlow, McpOAuthIdentity, McpOAuthStore, McpOAuthTokenRecord,
};

const TOKEN_EXPIRY_SKEW_SECS: u64 = 60;

#[derive(Debug, Clone)]
pub struct McpOAuthServiceConfig {
    pub neo_home: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct McpOAuthService {
    store: McpOAuthStore,
}

impl McpOAuthService {
    #[must_use]
    pub fn new(config: McpOAuthServiceConfig) -> Self {
        let neo_home = config.neo_home.unwrap_or_else(default_neo_home);
        Self::from_store(McpOAuthStore::new(neo_home.join("credentials").join("mcp")))
    }

    #[must_use]
    pub fn from_store(store: McpOAuthStore) -> Self {
        Self { store }
    }

    #[must_use]
    pub const fn store(&self) -> &McpOAuthStore {
        &self.store
    }

    #[must_use]
    pub fn has_tokens(&self, identity: &McpOAuthIdentity) -> bool {
        self.store
            .load_tokens(identity)
            .is_ok_and(|tokens| tokens.is_some())
    }

    pub async fn access_token(
        &self,
        identity: &McpOAuthIdentity,
    ) -> Result<Option<String>, McpOAuthError> {
        let Some(tokens) = self
            .store
            .load_tokens(identity)
            .map_err(|err| McpOAuthError::Store(err.to_string()))?
        else {
            return Ok(None);
        };

        if token_is_fresh(&tokens) {
            return Ok(Some(tokens.access_token));
        }

        match self.refresh(identity, &tokens).await {
            Ok(tokens) => Ok(Some(tokens.access_token)),
            Err(err @ (McpOAuthError::MissingTokens | McpOAuthError::NeedsAuth(_))) => Err(err),
            Err(err) => Err(McpOAuthError::NeedsAuth(err.to_string())),
        }
    }

    async fn refresh(
        &self,
        identity: &McpOAuthIdentity,
        tokens: &McpOAuthTokenRecord,
    ) -> Result<McpOAuthTokenRecord, McpOAuthError> {
        if tokens.refresh_token.is_none() {
            return Err(McpOAuthError::NeedsAuth(
                "access token expired and no refresh token is available".to_owned(),
            ));
        }

        let client = self
            .store
            .load_client(identity)
            .map_err(|err| McpOAuthError::Store(err.to_string()))?;
        let Some(client) = client else {
            return Err(McpOAuthError::NeedsAuth(
                "OAuth client registration is missing".to_owned(),
            ));
        };

        let discovery = self
            .store
            .load_discovery(identity)
            .map_err(|err| McpOAuthError::Store(err.to_string()))?;
        let Some(discovery) = discovery else {
            return Err(McpOAuthError::NeedsAuth(
                "OAuth discovery metadata is missing".to_owned(),
            ));
        };

        let metadata =
            serde_json::from_value(discovery.authorization_server_metadata).map_err(|err| {
                McpOAuthError::Store(format!("invalid OAuth discovery metadata: {err}"))
            })?;
        let mut manager = AuthorizationManager::new(identity.canonical_resource_url.as_str())
            .await
            .map_err(|err| McpOAuthError::Flow(format!("failed to build OAuth manager: {err}")))?;
        manager.set_metadata(metadata);
        manager.set_credential_store(CanonicalCredentialStore::new(
            self.store.clone(),
            identity.clone(),
        ));
        manager
            .configure_client(oauth_client_config_from_record(&client)?)
            .map_err(|err| {
                McpOAuthError::Flow(format!("stored OAuth client is unusable: {err}"))
            })?;
        manager
            .refresh_token()
            .await
            .map_err(refresh_error_to_oauth)?;

        self.store
            .load_tokens(identity)
            .map_err(|err| McpOAuthError::Store(err.to_string()))?
            .ok_or(McpOAuthError::MissingTokens)
    }

    pub fn invalidate(
        &self,
        identity: &McpOAuthIdentity,
        scope: InvalidateScope,
    ) -> Result<(), McpOAuthError> {
        match scope {
            InvalidateScope::TokensOnly => self.store.clear_tokens(identity),
            InvalidateScope::AllCredentials => {
                self.store.clear_tokens(identity)?;
                remove_optional(&self.store.server_dir(identity).join("client.json"))?;
                remove_optional(&self.store.server_dir(identity).join("discovery.json"))?;
                remove_empty_server_dir(&self.store.server_dir(identity))
            }
        }
    }

    pub async fn begin_authorization(
        &self,
        identity: McpOAuthIdentity,
    ) -> Result<McpOAuthFlow, McpOAuthError> {
        let mut manager = AuthorizationManager::new(identity.canonical_resource_url.as_str())
            .await
            .map_err(|err| McpOAuthError::Flow(format!("failed to build OAuth manager: {err}")))?;
        manager.set_state_store(InMemoryStateStore::new());

        if let Some(client) = self
            .store
            .load_client(&identity)
            .map_err(|err| McpOAuthError::Store(err.to_string()))?
        {
            let metadata = manager.discover_metadata().await.map_err(|err| {
                McpOAuthError::NeedsAuth(format!("OAuth discovery failed: {err}"))
            })?;
            manager.set_metadata(metadata);
            let mut config = OAuthClientConfig::new(
                client.client_id.clone(),
                redirect_uri_from_stored_client(&client)?,
            );
            if let Some(client_secret) = client.client_secret.clone() {
                config = config.with_client_secret(client_secret);
            }
            manager.configure_client(config).map_err(|err| {
                McpOAuthError::Flow(format!("stored OAuth client is unusable: {err}"))
            })?;
        } else {
            let metadata = manager.discover_metadata().await.map_err(|err| {
                McpOAuthError::NeedsAuth(format!("OAuth discovery failed: {err}"))
            })?;
            manager.set_metadata(metadata);
            let redirect_uri = phase_2b_redirect_uri(&identity)?;
            let client_config = manager
                .register_client("Neo", &redirect_uri, &[])
                .await
                .map_err(|err| {
                    McpOAuthError::Flow(format!("OAuth client registration failed: {err}"))
                })?;
            self.store
                .save_client(&identity, &client_record_from_config(&client_config))?;
        }

        let authorization_url = manager.get_authorization_url(&[]).await.map_err(|err| {
            McpOAuthError::Flow(format!("failed to build OAuth authorization URL: {err}"))
        })?;
        let authorization_url = reqwest::Url::parse(&authorization_url).map_err(|err| {
            McpOAuthError::Flow(format!("invalid OAuth authorization URL: {err}"))
        })?;
        let manager = Arc::new(Mutex::new(manager));

        Ok(McpOAuthFlow::new(
            authorization_url,
            identity,
            self.clone(),
            manager,
        ))
    }

    pub fn persist_rmcp_credentials(
        &self,
        identity: &McpOAuthIdentity,
        credentials: &StoredCredentials,
    ) -> Result<(), McpOAuthError> {
        let Some(tokens) = token_record_from_credentials(credentials) else {
            return Err(McpOAuthError::NeedsAuth(
                "OAuth authorization did not return an access token".to_owned(),
            ));
        };

        self.store.save_tokens(identity, &tokens)?;
        if self
            .store
            .load_client(identity)
            .map_err(|err| McpOAuthError::Store(err.to_string()))?
            .is_none()
        {
            self.store
                .save_client(identity, &client_record_from_credentials(credentials))?;
        }
        Ok(())
    }

    pub fn persist_client_and_discovery(
        &self,
        identity: &McpOAuthIdentity,
        client: &OAuthClientConfig,
        discovery: rmcp::transport::auth::AuthorizationMetadata,
    ) -> Result<(), McpOAuthError> {
        self.store
            .save_client(identity, &client_record_from_config(client))?;
        self.store.save_discovery(
            identity,
            &McpOAuthDiscoveryRecord {
                authorization_server_metadata: serde_json::to_value(discovery)
                    .map_err(|err| McpOAuthError::Store(err.to_string()))?,
                discovered_at: unix_now_secs().to_string(),
            },
        )
    }
}

#[derive(Debug, Clone)]
struct CanonicalCredentialStore {
    store: McpOAuthStore,
    identity: McpOAuthIdentity,
}

impl CanonicalCredentialStore {
    const fn new(store: McpOAuthStore, identity: McpOAuthIdentity) -> Self {
        Self { store, identity }
    }
}

#[async_trait]
impl CredentialStore for CanonicalCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let Some(tokens) = self
            .store
            .load_tokens(&self.identity)
            .map_err(|err| AuthError::InternalError(err.to_string()))?
        else {
            return Ok(None);
        };
        let Some(client) = self
            .store
            .load_client(&self.identity)
            .map_err(|err| AuthError::InternalError(err.to_string()))?
        else {
            return Ok(None);
        };
        Ok(Some(stored_credentials_from_records(&client, &tokens)?))
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let Some(tokens) = token_record_from_credentials(&credentials) else {
            return Err(AuthError::InternalError(
                "OAuth credentials did not include a token response".to_owned(),
            ));
        };
        let tokens = self.tokens_preserving_refresh_token(tokens)?;
        self.store
            .save_tokens(&self.identity, &tokens)
            .map_err(|err| AuthError::InternalError(err.to_string()))
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.store
            .clear_tokens(&self.identity)
            .map_err(|err| AuthError::InternalError(err.to_string()))
    }
}

impl CanonicalCredentialStore {
    fn tokens_preserving_refresh_token(
        &self,
        mut tokens: McpOAuthTokenRecord,
    ) -> Result<McpOAuthTokenRecord, AuthError> {
        if tokens.refresh_token.is_none() {
            let previous = self
                .store
                .load_tokens(&self.identity)
                .map_err(|err| AuthError::InternalError(err.to_string()))?;
            if let Some(previous_refresh_token) = previous.and_then(|record| record.refresh_token) {
                tokens.refresh_token = Some(previous_refresh_token.clone());
                if let Some(raw) = tokens.raw.as_object_mut() {
                    raw.insert(
                        "refresh_token".to_owned(),
                        serde_json::Value::String(previous_refresh_token),
                    );
                }
            }
        }
        Ok(tokens)
    }
}

fn default_neo_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map_or_else(|| PathBuf::from(".neo"), |home| home.join(".neo"))
}

#[must_use]
pub fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[must_use]
pub fn token_is_fresh(tokens: &McpOAuthTokenRecord) -> bool {
    let Some(expires_in) = tokens.expires_in else {
        return true;
    };
    let expires_at = tokens.token_received_at.saturating_add(expires_in);
    unix_now_secs().saturating_add(TOKEN_EXPIRY_SKEW_SECS) < expires_at
}

fn token_record_from_credentials(
    credentials: &rmcp::transport::auth::StoredCredentials,
) -> Option<McpOAuthTokenRecord> {
    let token_response = credentials.token_response.as_ref()?;
    let raw = serde_json::to_value(token_response).ok()?;
    let access_token = raw.get("access_token")?.as_str()?.to_owned();
    let token_type = raw
        .get("token_type")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let refresh_token = raw
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let expires_in = raw.get("expires_in").and_then(serde_json::Value::as_u64);
    let granted_scopes = raw
        .get("scope")
        .and_then(serde_json::Value::as_str)
        .map_or_else(
            || credentials.granted_scopes.clone(),
            |scope| scope.split_whitespace().map(str::to_owned).collect(),
        );
    let token_received_at = credentials.token_received_at.unwrap_or_else(unix_now_secs);

    Some(McpOAuthTokenRecord {
        access_token,
        token_type,
        refresh_token,
        expires_in,
        token_received_at,
        granted_scopes,
        raw,
    })
}

fn stored_credentials_from_records(
    client: &McpOAuthClientRecord,
    tokens: &McpOAuthTokenRecord,
) -> Result<StoredCredentials, AuthError> {
    let token_response = serde_json::from_value(tokens.raw.clone())
        .map_err(|err| AuthError::InternalError(format!("invalid stored OAuth token: {err}")))?;
    Ok(StoredCredentials::new(
        client.client_id.clone(),
        Some(token_response),
        tokens.granted_scopes.clone(),
        Some(tokens.token_received_at),
    ))
}

fn client_record_from_credentials(credentials: &StoredCredentials) -> McpOAuthClientRecord {
    McpOAuthClientRecord {
        client_id: credentials.client_id.clone(),
        client_secret: None,
        redirect_uris: Vec::new(),
        token_endpoint_auth_method: None,
        raw: serde_json::json!({
            "client_id": credentials.client_id
        }),
    }
}

fn redirect_uri_from_stored_client(client: &McpOAuthClientRecord) -> Result<String, McpOAuthError> {
    client.redirect_uris.first().cloned().ok_or_else(|| {
        McpOAuthError::Flow("stored OAuth client is missing a redirect URI".to_owned())
    })
}

fn phase_2b_redirect_uri(_identity: &McpOAuthIdentity) -> Result<String, McpOAuthError> {
    Err(McpOAuthError::Flow(
        "OAuth callback server is not wired yet".to_owned(),
    ))
}

fn client_record_from_config(config: &OAuthClientConfig) -> McpOAuthClientRecord {
    McpOAuthClientRecord {
        client_id: config.client_id.clone(),
        client_secret: config.client_secret.clone(),
        redirect_uris: vec![config.redirect_uri.clone()],
        token_endpoint_auth_method: None,
        raw: serde_json::json!({
            "client_id": config.client_id,
            "redirect_uris": [config.redirect_uri],
            "scopes": config.scopes,
            "application_type": config.application_type,
        }),
    }
}

fn oauth_client_config_from_record(
    client: &McpOAuthClientRecord,
) -> Result<OAuthClientConfig, McpOAuthError> {
    let mut config = OAuthClientConfig::new(
        client.client_id.clone(),
        redirect_uri_from_stored_client(client)?,
    );
    if let Some(client_secret) = client.client_secret.clone() {
        config = config.with_client_secret(client_secret);
    }
    Ok(config)
}

fn refresh_error_to_oauth(err: AuthError) -> McpOAuthError {
    match err {
        AuthError::AuthorizationRequired | AuthError::TokenRefreshFailed(_) => {
            McpOAuthError::NeedsAuth(err.to_string())
        }
        other => McpOAuthError::Flow(format!("OAuth token refresh failed: {other}")),
    }
}

fn remove_optional(path: &Path) -> Result<(), McpOAuthError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(McpOAuthError::Store(format!(
            "failed to remove {}: {err}",
            path.display()
        ))),
    }
}

fn remove_empty_server_dir(path: &Path) -> Result<(), McpOAuthError> {
    match fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(err) => Err(McpOAuthError::Store(format!(
            "failed to remove {}: {err}",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::mcp::oauth::store::McpOAuthDiscoveryRecord;
    use crate::tools::mcp::oauth::{
        InvalidateScope, McpOAuthClientRecord, McpOAuthError, McpOAuthIdentity,
        McpOAuthTokenRecord, McpOAuthTransportKind,
    };
    use rmcp::transport::auth::StoredCredentials;

    #[test]
    fn from_store_uses_supplied_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = McpOAuthStore::new(dir.path().to_path_buf());
        let service = McpOAuthService::from_store(store.clone());

        assert_eq!(service.store().root(), store.root());
    }

    #[test]
    fn new_uses_credentials_mcp_store_root() {
        let dir = tempfile::tempdir().unwrap();
        let service = McpOAuthService::new(McpOAuthServiceConfig {
            neo_home: Some(dir.path().to_path_buf()),
        });

        assert_eq!(
            service.store().root(),
            dir.path().join("credentials").join("mcp")
        );
    }

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

    fn authorization_metadata(
        token_endpoint: &str,
    ) -> rmcp::transport::auth::AuthorizationMetadata {
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

    fn stored_credentials(access_token: &str, expires_in: u64) -> StoredCredentials {
        let token_response = serde_json::from_value(serde_json::json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "refresh_token": "refresh-token",
            "expires_in": expires_in,
            "scope": "read write"
        }))
        .unwrap();
        StoredCredentials::new(
            "client-id".to_owned(),
            Some(token_response),
            vec!["read".to_owned(), "write".to_owned()],
            Some(unix_now_secs()),
        )
    }

    fn stored_credentials_without_token_response() -> StoredCredentials {
        StoredCredentials::new("client-id".to_owned(), None, Vec::new(), None)
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

    #[test]
    fn token_is_fresh_accepts_missing_expiry_and_future_expiry() {
        let mut tokens = token_record("fresh-token");
        tokens.expires_in = None;
        assert!(token_is_fresh(&tokens));

        tokens.expires_in = Some(3600);
        tokens.token_received_at = unix_now_secs();
        assert!(token_is_fresh(&tokens));
    }

    #[test]
    fn token_is_fresh_rejects_tokens_expiring_within_sixty_seconds() {
        let mut tokens = token_record("stale-token");
        tokens.expires_in = Some(59);
        tokens.token_received_at = unix_now_secs();

        assert!(!token_is_fresh(&tokens));
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
    async fn access_token_stale_without_refresh_token_needs_auth() {
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

    #[tokio::test]
    async fn persist_rmcp_credentials_writes_tokens_and_minimal_client() {
        let (_dir, service, identity) = service();

        service
            .persist_rmcp_credentials(&identity, &stored_credentials("persisted-token", 3600))
            .unwrap();

        let tokens = service.store().load_tokens(&identity).unwrap().unwrap();
        assert_eq!(tokens.access_token, "persisted-token");
        assert_eq!(tokens.refresh_token.as_deref(), Some("refresh-token"));
        let client = service.store().load_client(&identity).unwrap().unwrap();
        assert_eq!(client.client_id, "client-id");
        assert!(client.client_secret.is_none());
        assert!(client.redirect_uris.is_empty());
        assert!(client.token_endpoint_auth_method.is_none());
        assert!(service.store().load_discovery(&identity).unwrap().is_none());
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

    #[tokio::test]
    async fn persist_rmcp_credentials_preserves_existing_rich_client_record() {
        let (_dir, service, identity) = service();
        let existing_client = client_record();
        service
            .store()
            .save_client(&identity, &existing_client)
            .unwrap();

        service
            .persist_rmcp_credentials(&identity, &stored_credentials("persisted-token", 3600))
            .unwrap();

        let client = service.store().load_client(&identity).unwrap().unwrap();
        assert_eq!(client, existing_client);
        let tokens = service.store().load_tokens(&identity).unwrap().unwrap();
        assert_eq!(tokens.access_token, "persisted-token");
    }

    #[tokio::test]
    async fn persist_rmcp_credentials_without_token_response_needs_auth_and_writes_nothing() {
        let (_dir, service, identity) = service();

        let err = service
            .persist_rmcp_credentials(&identity, &stored_credentials_without_token_response())
            .unwrap_err();

        assert!(
            matches!(err, McpOAuthError::NeedsAuth(message) if message == "OAuth authorization did not return an access token")
        );
        assert!(service.store().load_tokens(&identity).unwrap().is_none());
        assert!(service.store().load_client(&identity).unwrap().is_none());
        assert!(service.store().load_discovery(&identity).unwrap().is_none());
    }

    #[test]
    fn stored_client_redirect_uri_uses_first_redirect_uri() {
        let mut client = client_record();
        client.redirect_uris = vec![
            "http://127.0.0.1:14500/callback".to_owned(),
            "http://127.0.0.1:14501/callback".to_owned(),
        ];

        let redirect_uri = redirect_uri_from_stored_client(&client).unwrap();

        assert_eq!(redirect_uri, "http://127.0.0.1:14500/callback");
    }

    #[test]
    fn stored_client_without_redirect_uri_is_flow_error() {
        let mut client = client_record();
        client.redirect_uris.clear();

        let err = redirect_uri_from_stored_client(&client).unwrap_err();

        assert!(
            matches!(err, McpOAuthError::Flow(message) if message == "stored OAuth client is missing a redirect URI")
        );
    }

    #[test]
    fn phase_2b_dynamic_redirect_uri_is_not_available_without_callback_server() {
        let identity = identity();

        let err = phase_2b_redirect_uri(&identity).unwrap_err();

        assert!(
            matches!(err, McpOAuthError::Flow(message) if message == "OAuth callback server is not wired yet")
        );
    }

    #[test]
    fn client_record_from_config_preserves_redirect_uri_that_is_not_resource_url() {
        let identity = identity();
        let redirect_uri = "http://127.0.0.1:14500/callback";
        let config = OAuthClientConfig::new("client-id", redirect_uri);

        let record = client_record_from_config(&config);

        assert_eq!(record.redirect_uris, vec![redirect_uri.to_owned()]);
        assert_ne!(record.redirect_uris[0], identity.canonical_resource_url);
    }
}
