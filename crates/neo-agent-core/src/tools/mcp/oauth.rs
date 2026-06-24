//! OAuth adapters and flow helpers for rmcp (Task 3.2).
//!
//! This module provides implementations of the rmcp `CredentialStore` and
//! `StateStore` traits that integrate with Neo's file-backed `OAuthStore` for
//! persistent credentials and an in-memory store for transient OAuth state.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use rmcp::transport::auth::{AuthError, AuthorizationManager, StateStore, StoredAuthorizationState, StoredCredentials};
use tokio::sync::{Mutex, RwLock};

use crate::oauth::OAuthStore;

/// Build an `AuthorizationManager` configured with Neo's persistent credential and state stores.
///
/// This function creates an `AuthorizationManager` with a `FileCredentialStore` and
/// `InMemoryStateStore`, which can be used to perform OAuth authorization flows for
/// MCP servers.
///
/// # Arguments
///
/// * `base_url` - The base URL of the MCP server to authorize.
/// * `oauth_store_path` - Path to Neo's `oauth.json` file for persistent credential storage.
/// * `server_id` - Identifier for the MCP server (used as the credential key prefix).
///
/// # Returns
///
/// * `Arc<Mutex<AuthorizationManager>>` - The configured authorization manager.
///
/// # Example
///
/// ```no_run
/// use std::path::Path;
/// use tokio::sync::Arc;
/// use rmcp::transport::auth::AuthorizationManager;
///
/// let manager = build_authorization_manager(
///     "https://example.com/mcp",
///     Path::new("/home/user/.neo/oauth.json"),
///     "my-server",
/// )?;
/// ```
#[allow(dead_code)] // TODO: unlink from MCP tool on Task 3.4
pub async fn build_authorization_manager(
    base_url: &str,
    oauth_store_path: &Path,
    server_id: &str,
) -> Result<Arc<Mutex<AuthorizationManager>>, AuthError> {
    // Create file-backed credential store
    let credential_store = FileCredentialStore::new(
        oauth_store_path.to_path_buf(),
        server_id.to_string(),
    );

    // Create in-memory state store
    let state_store = InMemoryStateStore::new();

    // Build AuthorizationManager with base URL
    let mut manager = AuthorizationManager::new(base_url)
        .await
        .map_err(|e| {
            AuthError::InternalError(format!("failed to build authorization manager: {e}"))
        })?;

    // Replace default credential store with file-backed store
    manager.set_credential_store(credential_store);

    // Replace default state store with in-memory store
    manager.set_state_store(state_store);

    Ok(Arc::new(Mutex::new(manager)))
}

/// Helper function to generate the storage key for an MCP server.
///
/// Returns `mcp:<server_id>` which matches the key format expected by
/// [`OAuthStore`].
#[must_use]
pub fn key_for_server(server_id: &str) -> String {
    format!("mcp:{server_id}")
}

/// File-backed credential store for rmcp OAuth flows.
///
/// This adapter wraps Neo's [`OAuthStore`] to implement the rmcp
/// `CredentialStore` trait. Credentials are persisted to a JSON file
/// (typically `~/.neo/oauth.json`) under keys of the form `mcp:<server_id>`.
#[derive(Debug, Clone)]
pub struct FileCredentialStore {
    /// Path to the `oauth.json` file.
    path: PathBuf,
    /// Server identifier used as the storage key prefix.
    server_id: String,
}

impl FileCredentialStore {
    /// Create a new file-backed credential store for the given server.
    ///
    /// The `path` should point to the OAuth store file (e.g., `~/.neo/oauth.json`).
    /// The `server_id` is used to construct the storage key via [`key_for_server`].
    #[must_use]
    pub fn new(path: PathBuf, server_id: String) -> Self {
        Self { path, server_id }
    }

    /// Returns the storage key for this server.
    fn key(&self) -> String {
        key_for_server(&self.server_id)
    }
}

#[async_trait]
impl rmcp::transport::auth::CredentialStore for FileCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let path = self.path.clone();
        let key = self.key();

        tokio::task::spawn_blocking(move || {
            let store = OAuthStore::load(&path).map_err(|e| {
                AuthError::InternalError(format!("failed to load OAuth store: {e}"))
            })?;
            Ok(store.get(&key).cloned())
        })
        .await
        .map_err(|e| AuthError::InternalError(format!("blocking task failed: {e}")))?
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let path = self.path.clone();
        let key = self.key();

        tokio::task::spawn_blocking(move || {
            let mut store = OAuthStore::load(&path).map_err(|e| {
                AuthError::InternalError(format!("failed to load OAuth store: {e}"))
            })?;
            store.set(&key, credentials);
            store.save(&path).map_err(|e| {
                AuthError::InternalError(format!("failed to save OAuth store: {e}"))
            })?;
            Ok(())
        })
        .await
        .map_err(|e| AuthError::InternalError(format!("blocking task failed: {e}")))?
    }

    async fn clear(&self) -> Result<(), AuthError> {
        let path = self.path.clone();
        let key = self.key();

        tokio::task::spawn_blocking(move || {
            let mut store = OAuthStore::load(&path).map_err(|e| {
                AuthError::InternalError(format!("failed to load OAuth store: {e}"))
            })?;
            let _ = store.remove(&key);
            store.save(&path).map_err(|e| {
                AuthError::InternalError(format!("failed to save OAuth store: {e}"))
            })?;
            Ok(())
        })
        .await
        .map_err(|e| AuthError::InternalError(format!("blocking task failed: {e}")))?
    }
}

/// In-memory state store for OAuth authorization flows.
///
/// This implementation stores transient OAuth state (PKCE verifiers, CSRF tokens)
/// in memory. The state is lost when the application restarts, which is acceptable
/// for the authorization flow since in-flight flows will simply need to be restarted.
#[derive(Debug, Default, Clone)]
pub struct InMemoryStateStore {
    /// Internal storage protected by a read-write lock.
    states: Arc<RwLock<BTreeMap<String, StoredAuthorizationState>>>,
}

impl InMemoryStateStore {
    /// Create a new empty in-memory state store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl StateStore for InMemoryStateStore {
    async fn save(
        &self,
        csrf_token: &str,
        state: StoredAuthorizationState,
    ) -> Result<(), AuthError> {
        self.states
            .write()
            .await
            .insert(csrf_token.to_string(), state);
        Ok(())
    }

    async fn load(&self, csrf_token: &str) -> Result<Option<StoredAuthorizationState>, AuthError> {
        Ok(self.states.read().await.get(csrf_token).cloned())
    }

    async fn delete(&self, csrf_token: &str) -> Result<(), AuthError> {
        self.states.write().await.remove(csrf_token);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::transport::auth::{CredentialStore, StateStore};
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
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
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
        let store = FileCredentialStore::new(path.clone(), "linear".to_owned());

        let credentials = sample_credentials();
        store.save(credentials.clone()).await.unwrap();

        let loaded = store.load().await.unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.client_id, credentials.client_id);
        assert_eq!(loaded.granted_scopes, credentials.granted_scopes);
    }

    #[tokio::test]
    async fn file_credential_store_clear_removes_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oauth.json");
        let store = FileCredentialStore::new(path.clone(), "linear".to_owned());

        let credentials = sample_credentials();
        store.save(credentials).await.unwrap();
        assert!(store.load().await.unwrap().is_some());

        store.clear().await.unwrap();
        assert!(store.load().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn file_credential_store_isolates_by_server_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oauth.json");

        let store_a = FileCredentialStore::new(path.clone(), "server-a".to_owned());
        let store_b = FileCredentialStore::new(path.clone(), "server-b".to_owned());

        let mut credentials_a = sample_credentials();
        credentials_a.client_id = "client-a".to_owned();
        let mut credentials_b = sample_credentials();
        credentials_b.client_id = "client-b".to_owned();

        store_a.save(credentials_a).await.unwrap();
        store_b.save(credentials_b).await.unwrap();

        let loaded_a = store_a.load().await.unwrap().unwrap();
        let loaded_b = store_b.load().await.unwrap().unwrap();
        assert_eq!(loaded_a.client_id, "client-a");
        assert_eq!(loaded_b.client_id, "client-b");

        // Verify persistence via raw OAuthStore
        let raw = OAuthStore::load(&path).unwrap();
        assert!(raw.get("mcp:server-a").is_some());
        assert!(raw.get("mcp:server-b").is_some());
    }

    #[tokio::test]
    async fn in_memory_state_store_save_and_load() {
        let store = InMemoryStateStore::new();

        let state = StoredAuthorizationState::new(
            &oauth2::PkceCodeVerifier::new("verifier".to_owned()),
            &oauth2::CsrfToken::new("csrf".to_owned()),
        );

        store.save("token-123", state.clone()).await.unwrap();
        let loaded = store.load("token-123").await.unwrap();
        assert!(loaded.is_some());
    }

    #[tokio::test]
    async fn in_memory_state_store_delete() {
        let store = InMemoryStateStore::new();

        let state = StoredAuthorizationState::new(
            &oauth2::PkceCodeVerifier::new("verifier".to_owned()),
            &oauth2::CsrfToken::new("csrf".to_owned()),
        );

        store.save("token-123", state).await.unwrap();
        assert!(store.load("token-123").await.unwrap().is_some());

        store.delete("token-123").await.unwrap();
        assert!(store.load("token-123").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn in_memory_state_store_load_returns_none_for_missing() {
        let store = InMemoryStateStore::new();
        let result = store.load("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn in_memory_state_store_delete_is_idempotent() {
        let store = InMemoryStateStore::new();
        // Deleting a non-existent key should not error
        store.delete("nonexistent").await.unwrap();
    }
}
