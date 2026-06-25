//! OAuth 2.0 legacy types retained for migration and public API compatibility.
//!
//! The hand-rolled PKCE, authorization-URL, code-exchange, and refresh helpers
//! have been removed in favour of `rmcp::transport::auth::AuthorizationManager`.
//! What remains are the data types still referenced by the MCP connection
//! manager (`OAuthProviderRegistry` as a legacy override holder), the token
//! store, and the callback server.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

pub mod callback_server;
pub mod store;

pub use store::OAuthStore;

/// A set of OAuth tokens returned by the token endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthTokenSet {
    pub access_token: String,
    pub token_type: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub scopes: Vec<String>,
}

/// Provider configuration retained for the legacy `OAuthProviderRegistry`
/// override holder on [`crate::tools::McpConnectionManager`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthProvider {
    pub id: String,
    pub client_id: String,
    pub auth_url: String,
    pub token_url: String,
    pub scopes: Vec<String>,
    pub default_callback_port: u16,
}

/// Registry of OAuth providers keyed by provider id.
///
/// After the `rmcp` migration this registry is kept only as a legacy override
/// holder so that `McpConnectionManager::set_oauth_provider_registry` remains
/// source-compatible. The actual OAuth flow is handled by `rmcp`'s
/// discovery-based `AuthorizationManager`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OAuthProviderRegistry {
    providers: BTreeMap<String, OAuthProvider>,
}

impl OAuthProviderRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            providers: BTreeMap::new(),
        }
    }

    /// Create a registry seeded with the built-in Linear provider.
    #[must_use]
    pub fn with_builtin_providers() -> Self {
        let mut registry = Self::new();
        registry.register(OAuthProvider {
            id: "linear".to_owned(),
            client_id: "neo".to_owned(),
            auth_url: "https://linear.app/oauth/authorize".to_owned(),
            token_url: "https://api.linear.app/oauth/token".to_owned(),
            scopes: vec!["write".to_owned()],
            default_callback_port: 0,
        });
        registry
    }

    /// Register (or replace) a provider in the registry.
    pub fn register(&mut self, provider: OAuthProvider) {
        self.providers.insert(provider.id.clone(), provider);
    }

    /// Look up a provider by its id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&OAuthProvider> {
        self.providers.get(id)
    }
}

/// Errors that may occur during OAuth operations.
#[derive(Debug, Error)]
pub enum OAuthError {
    #[error("token endpoint request failed: {0}")]
    TokenRequest(#[from] reqwest::Error),
    #[error("token endpoint returned error: {status} {body}")]
    TokenEndpoint {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("failed to parse token response: {0}")]
    TokenParse(#[from] serde_json::Error),
    #[error("callback server error: {0}")]
    CallbackServer(String),
    #[error("callback timed out after {0:?}")]
    CallbackTimeout(std::time::Duration),
    #[error("callback state mismatch: expected {expected}, got {got}")]
    CallbackStateMismatch { expected: String, got: String },
    #[error("callback request missing authorization code")]
    CallbackMissingCode,
    #[error("no OAuth provider configured for this server")]
    ProviderDetection,
    #[error("failed to load token store: {0}")]
    StoreLoad(std::io::Error),
    #[error("failed to save token store: {0}")]
    StoreSave(std::io::Error),
    #[error("failed to parse token store: {0}")]
    StoreParse(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_seeds_builtin_linear_provider() {
        let registry = OAuthProviderRegistry::with_builtin_providers();
        let provider = registry.get("linear").expect("linear should be registered");
        assert_eq!(provider.id, "linear");
        assert_eq!(provider.auth_url, "https://linear.app/oauth/authorize");
        assert_eq!(provider.token_url, "https://api.linear.app/oauth/token");
        assert_eq!(provider.scopes, vec!["write"]);
    }

    #[test]
    fn registry_register_overrides_builtin() {
        let mut registry = OAuthProviderRegistry::with_builtin_providers();
        let custom = OAuthProvider {
            id: "linear".to_owned(),
            client_id: "custom-client".to_owned(),
            auth_url: "https://custom.example/authorize".to_owned(),
            token_url: "https://custom.example/token".to_owned(),
            scopes: vec!["read".to_owned()],
            default_callback_port: 0,
        };
        registry.register(custom.clone());
        let provider = registry.get("linear").expect("linear should exist");
        assert_eq!(provider.client_id, "custom-client");
        assert_eq!(provider.auth_url, "https://custom.example/authorize");
    }
}
