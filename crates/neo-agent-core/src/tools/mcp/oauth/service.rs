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

    pub(super) fn credential_store(&self, identity: McpOAuthIdentity) -> CanonicalCredentialStore {
        CanonicalCredentialStore::new(self.store.clone(), identity)
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

        let tokens = self.refresh(identity, &tokens).await?;
        Ok(Some(tokens.access_token))
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
pub(super) struct CanonicalCredentialStore {
    store: McpOAuthStore,
    identity: McpOAuthIdentity,
}

impl CanonicalCredentialStore {
    pub(super) const fn new(store: McpOAuthStore, identity: McpOAuthIdentity) -> Self {
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
#[path = "test_cases/access_token.rs"]
mod access_token;

#[cfg(test)]
#[path = "test_cases/token_freshness.rs"]
mod token_freshness;

#[cfg(test)]
#[path = "test_cases/credentials.rs"]
mod credentials;

#[cfg(test)]
#[path = "test_cases/stored_client.rs"]
mod stored_client;

#[cfg(test)]
#[path = "test_cases/store.rs"]
mod store;
