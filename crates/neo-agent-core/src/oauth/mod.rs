//! OAuth 2.0 token-store and callback-server types.
//!
//! The hand-rolled PKCE, authorization-URL, code-exchange, and refresh helpers
//! have been removed in favour of `rmcp::transport::auth::AuthorizationManager`.
//! What remains are the token store and the callback server used during
//! interactive OAuth flows.

use thiserror::Error;

pub mod callback_server;
pub mod store;

pub use store::OAuthStore;

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
