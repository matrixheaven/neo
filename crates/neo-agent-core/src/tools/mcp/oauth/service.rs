use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rmcp::transport::auth::{AuthorizationManager, OAuthClientConfig, StoredCredentials};
use tokio::sync::Mutex;

use super::{
    InMemoryStateStore, InvalidateScope, McpOAuthClientRecord, McpOAuthError, McpOAuthFlow,
    McpOAuthIdentity, McpOAuthStore, McpOAuthTokenRecord,
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

    pub async fn has_tokens(&self, identity: &McpOAuthIdentity) -> bool {
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
        if client.is_none() {
            return Err(McpOAuthError::NeedsAuth(
                "OAuth client registration is missing".to_owned(),
            ));
        }

        let discovery = self
            .store
            .load_discovery(identity)
            .map_err(|err| McpOAuthError::Store(err.to_string()))?;
        if discovery.is_none() {
            return Err(McpOAuthError::NeedsAuth(
                "OAuth discovery metadata is missing".to_owned(),
            ));
        }

        Err(McpOAuthError::NeedsAuth(
            "OAuth token refresh is not implemented yet".to_owned(),
        ))
    }

    pub async fn invalidate(
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

    pub async fn persist_rmcp_credentials(
        &self,
        identity: &McpOAuthIdentity,
        credentials: StoredCredentials,
    ) -> Result<(), McpOAuthError> {
        let Some(tokens) = token_record_from_credentials(&credentials) else {
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
                .save_client(identity, &client_record_from_credentials(&credentials))?;
        }
        Ok(())
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
        .map(|scope| scope.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_else(|| credentials.granted_scopes.clone());
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
            resource_metadata: serde_json::json!({"resource": "https://mcp.example.com/sse"}),
            authorization_server_metadata: serde_json::json!({
                "issuer": "https://auth.example.com"
            }),
            discovered_at: "2026-06-29T00:00:00Z".to_owned(),
        }
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
        assert!(!service.has_tokens(&identity).await);
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
        assert!(service.has_tokens(&identity).await);
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
            .await
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
            .await
            .unwrap();
        service
            .invalidate(&identity, InvalidateScope::AllCredentials)
            .await
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
            .persist_rmcp_credentials(&identity, stored_credentials("persisted-token", 3600))
            .await
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

    #[tokio::test]
    async fn persist_rmcp_credentials_preserves_existing_rich_client_record() {
        let (_dir, service, identity) = service();
        let existing_client = client_record();
        service
            .store()
            .save_client(&identity, &existing_client)
            .unwrap();

        service
            .persist_rmcp_credentials(&identity, stored_credentials("persisted-token", 3600))
            .await
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
            .persist_rmcp_credentials(&identity, stored_credentials_without_token_response())
            .await
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
