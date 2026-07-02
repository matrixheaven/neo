//! OAuth lifecycle support for MCP transports.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use rmcp::transport::auth::{AuthError, AuthorizationManager, StoredCredentials};
use tokio::sync::Mutex;

use crate::oauth::OAuthStore;

mod error;
mod flow;
mod identity;
mod service;
mod store;

pub use error::{InvalidateScope, McpOAuthError};
pub use flow::{InMemoryStateStore, McpOAuthFlow};
pub use identity::{McpOAuthIdentity, McpOAuthTransportKind};
pub use service::{McpOAuthService, McpOAuthServiceConfig};
pub use store::{
    McpOAuthClientRecord, McpOAuthDiscoveryRecord, McpOAuthStore, McpOAuthTokenRecord,
};

/// Build an `AuthorizationManager` configured with Neo's current rmcp OAuth bridge.
///
/// This bridge is used by the manual CLI/TUI browser OAuth flow so rmcp can
/// perform discovery, dynamic client registration, and code exchange. Runtime
/// HTTP/SSE MCP transports use [`McpOAuthService`] directly instead.
pub async fn build_authorization_manager(
    base_url: &str,
    oauth_store_path: &Path,
    server_id: &str,
) -> Result<Arc<Mutex<AuthorizationManager>>, AuthError> {
    let shared_store = Arc::new(Mutex::new(OAuthStore::default()));
    let credential_store = FileCredentialStore::with_shared_store(
        oauth_store_path.to_path_buf(),
        server_id.to_string(),
        Arc::clone(&shared_store),
    );
    let state_store = InMemoryStateStore::new();

    let mut manager = AuthorizationManager::new(base_url).await.map_err(|err| {
        AuthError::InternalError(format!("failed to build authorization manager: {err}"))
    })?;

    manager.set_credential_store(credential_store);
    manager.set_state_store(state_store);

    Ok(Arc::new(Mutex::new(manager)))
}

#[must_use]
fn key_for_server(server_id: &str) -> String {
    format!("mcp:{server_id}")
}

#[derive(Debug, Clone)]
struct FileCredentialStore {
    path: PathBuf,
    server_id: String,
    store: Arc<Mutex<OAuthStore>>,
}

impl FileCredentialStore {
    #[cfg(test)]
    #[must_use]
    fn new(path: PathBuf, server_id: String) -> Self {
        Self {
            path,
            server_id,
            store: Arc::new(Mutex::new(OAuthStore::default())),
        }
    }

    #[must_use]
    fn with_shared_store(path: PathBuf, server_id: String, store: Arc<Mutex<OAuthStore>>) -> Self {
        Self {
            path,
            server_id,
            store,
        }
    }

    fn key(&self) -> String {
        key_for_server(&self.server_id)
    }
}

#[async_trait]
impl rmcp::transport::auth::CredentialStore for FileCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let mut store = self.store.lock().await;
        let key = self.key();

        *store = OAuthStore::load(&self.path).map_err(|err| {
            AuthError::InternalError(format!("failed to load OAuth store: {err}"))
        })?;
        Ok(store.get(&key).cloned())
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let mut store = self.store.lock().await;
        let key = self.key();

        *store = OAuthStore::load(&self.path).map_err(|err| {
            AuthError::InternalError(format!("failed to load OAuth store: {err}"))
        })?;
        store.set(&key, credentials);
        store.save(&self.path).map_err(|err| {
            AuthError::InternalError(format!("failed to save OAuth store: {err}"))
        })?;
        Ok(())
    }

    async fn clear(&self) -> Result<(), AuthError> {
        let mut store = self.store.lock().await;
        let key = self.key();

        *store = OAuthStore::load(&self.path).map_err(|err| {
            AuthError::InternalError(format!("failed to load OAuth store: {err}"))
        })?;
        let _ = store.remove(&key);
        store.save(&self.path).map_err(|err| {
            AuthError::InternalError(format!("failed to save OAuth store: {err}"))
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::transport::auth::CredentialStore;
    use std::time::SystemTime;

    fn sample_credentials() -> StoredCredentials {
        let mut value = serde_json::Map::new();
        value.insert("access_token".to_owned(), serde_json::json!("test-token"));
        value.insert("token_type".to_owned(), serde_json::json!("Bearer"));

        let token_response: rmcp::transport::auth::OAuthTokenResponse =
            serde_json::from_value(serde_json::Value::Object(value)).unwrap();

        StoredCredentials::new(
            "test-client".to_owned(),
            Some(token_response),
            vec!["read".to_owned(), "write".to_owned()],
            Some(
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map_or(0, |duration| duration.as_secs()),
            ),
        )
    }

    #[test]
    fn key_for_server_returns_expected_format() {
        assert_eq!(key_for_server("linear"), "mcp:linear");
        assert_eq!(key_for_server("my-server"), "mcp:my-server");
    }

    #[tokio::test]
    async fn file_credential_store_load_returns_none_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oauth.json");
        let store = FileCredentialStore::new(path, "linear".to_owned());

        let result = store.load().await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn file_credential_store_save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oauth.json");
        let store = FileCredentialStore::new(path, "linear".to_owned());

        let credentials = sample_credentials();
        store.save(credentials.clone()).await.unwrap();

        let loaded = store.load().await.unwrap().unwrap();
        assert_eq!(loaded.client_id, credentials.client_id);
        assert_eq!(loaded.granted_scopes, credentials.granted_scopes);
    }

    #[tokio::test]
    async fn file_credential_store_clear_removes_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oauth.json");
        let store = FileCredentialStore::new(path, "linear".to_owned());

        store.save(sample_credentials()).await.unwrap();
        assert!(store.load().await.unwrap().is_some());

        store.clear().await.unwrap();
        assert!(store.load().await.unwrap().is_none());
    }
}
