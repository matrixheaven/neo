//! OAuth lifecycle support for MCP transports.

use std::sync::Arc;

use rmcp::transport::auth::{AuthError, AuthorizationManager};
use tokio::sync::Mutex;

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
    service: &McpOAuthService,
    identity: McpOAuthIdentity,
) -> Result<Arc<Mutex<AuthorizationManager>>, AuthError> {
    let state_store = InMemoryStateStore::new();

    let mut manager = AuthorizationManager::new(base_url).await.map_err(|err| {
        AuthError::InternalError(format!("failed to build authorization manager: {err}"))
    })?;

    manager.set_credential_store(service.credential_store(identity));
    manager.set_state_store(state_store);

    Ok(Arc::new(Mutex::new(manager)))
}
