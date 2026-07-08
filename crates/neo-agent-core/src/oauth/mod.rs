//! OAuth callback-server support.
//!
//! Credential persistence lives in `tools::mcp::oauth::McpOAuthStore`; this
//! module only hosts the short-lived browser callback server used by interactive
//! authorization flows.

use thiserror::Error;

pub mod callback_server;

/// Errors that may occur while waiting for the local OAuth callback.
#[derive(Debug, Error)]
pub enum OAuthError {
    #[error("callback server error: {0}")]
    CallbackServer(String),
    #[error("callback timed out after {0:?}")]
    CallbackTimeout(std::time::Duration),
    #[error("callback state mismatch: expected {expected}, got {got}")]
    CallbackStateMismatch { expected: String, got: String },
    #[error("callback request missing authorization code")]
    CallbackMissingCode,
}
