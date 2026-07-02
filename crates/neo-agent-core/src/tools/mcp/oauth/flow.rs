use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rmcp::transport::auth::{
    AuthError, AuthorizationManager, StateStore, StoredAuthorizationState,
};
use tokio::sync::{Mutex, RwLock};

use super::{McpOAuthError, McpOAuthIdentity, McpOAuthService};

pub struct McpOAuthFlow {
    authorization_url: reqwest::Url,
    identity: McpOAuthIdentity,
    service: McpOAuthService,
    manager: Arc<Mutex<AuthorizationManager>>,
}

impl McpOAuthFlow {
    #[must_use]
    pub const fn new(
        authorization_url: reqwest::Url,
        identity: McpOAuthIdentity,
        service: McpOAuthService,
        manager: Arc<Mutex<AuthorizationManager>>,
    ) -> Self {
        Self {
            authorization_url,
            identity,
            service,
            manager,
        }
    }

    #[must_use]
    pub const fn authorization_url(&self) -> &reqwest::Url {
        &self.authorization_url
    }

    #[must_use]
    pub const fn identity(&self) -> &McpOAuthIdentity {
        &self.identity
    }

    #[must_use]
    pub const fn service(&self) -> &McpOAuthService {
        &self.service
    }

    #[must_use]
    pub const fn manager(&self) -> &Arc<Mutex<AuthorizationManager>> {
        &self.manager
    }

    pub fn complete(self, _timeout: Duration) -> Result<(), McpOAuthError> {
        let Self {
            identity: _,
            service: _,
            manager: _,
            ..
        } = self;

        Err(McpOAuthError::Flow(
            "OAuth flow completion is not wired yet".to_owned(),
        ))
    }
}

/// In-memory state store for OAuth authorization flows.
///
/// This implementation stores transient OAuth state (PKCE verifiers, CSRF tokens)
/// in memory. The state is lost when the application restarts, which is acceptable
/// for the authorization flow since in-flight flows will simply need to be restarted.
#[derive(Debug, Default, Clone)]
pub struct InMemoryStateStore {
    states: Arc<RwLock<BTreeMap<String, StoredAuthorizationState>>>,
}

impl InMemoryStateStore {
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
    use rmcp::transport::auth::StateStore;

    #[tokio::test]
    async fn in_memory_state_store_load_returns_none_for_missing() {
        let store = InMemoryStateStore::new();
        let result = store.load("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn in_memory_state_store_delete_is_idempotent() {
        let store = InMemoryStateStore::new();
        store.delete("nonexistent").await.unwrap();
    }

    #[tokio::test]
    async fn in_memory_state_store_save_load_delete_roundtrip() {
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
    async fn mcp_oauth_flow_accessors_return_authorization_url_and_identity() {
        let dir = tempfile::tempdir().unwrap();
        let service = McpOAuthService::from_store(crate::tools::mcp::oauth::McpOAuthStore::new(
            dir.path().join("credentials").join("mcp"),
        ));
        let identity = McpOAuthIdentity::new(
            "linear",
            "http://localhost:39876/mcp",
            crate::tools::mcp::oauth::McpOAuthTransportKind::Http,
        )
        .unwrap();
        let manager = AuthorizationManager::new(identity.canonical_resource_url.as_str())
            .await
            .unwrap();
        let manager = Arc::new(Mutex::new(manager));
        let authorization_url = reqwest::Url::parse("http://localhost:39876/authorize").unwrap();

        let flow = McpOAuthFlow::new(
            authorization_url.clone(),
            identity.clone(),
            service,
            manager,
        );

        assert_eq!(flow.authorization_url(), &authorization_url);
        assert_eq!(flow.identity(), &identity);
    }

    #[tokio::test]
    async fn mcp_oauth_flow_complete_returns_not_wired_error() {
        let dir = tempfile::tempdir().unwrap();
        let service = McpOAuthService::from_store(crate::tools::mcp::oauth::McpOAuthStore::new(
            dir.path().join("credentials").join("mcp"),
        ));
        let identity = McpOAuthIdentity::new(
            "linear",
            "http://localhost:39876/mcp",
            crate::tools::mcp::oauth::McpOAuthTransportKind::Http,
        )
        .unwrap();
        let manager = AuthorizationManager::new(identity.canonical_resource_url.as_str())
            .await
            .unwrap();
        let flow = McpOAuthFlow::new(
            reqwest::Url::parse("http://localhost:39876/authorize").unwrap(),
            identity,
            service,
            Arc::new(Mutex::new(manager)),
        );

        let err = flow.complete(Duration::from_millis(1)).unwrap_err();

        assert!(
            matches!(err, McpOAuthError::Flow(message) if message == "OAuth flow completion is not wired yet")
        );
    }
}
