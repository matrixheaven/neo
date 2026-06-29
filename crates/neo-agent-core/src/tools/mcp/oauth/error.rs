#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidateScope {
    TokensOnly,
    AllCredentials,
}

#[derive(Debug, thiserror::Error)]
pub enum McpOAuthError {
    #[error("MCP server requires OAuth")]
    MissingTokens,
    #[error("MCP server requires OAuth reauthentication: {0}")]
    NeedsAuth(String),
    #[error("OAuth is not supported for this MCP transport")]
    UnsupportedTransport,
    #[error("invalid OAuth identity: {0}")]
    InvalidIdentity(String),
    #[error("OAuth store error: {0}")]
    Store(String),
    #[error("OAuth flow error: {0}")]
    Flow(String),
}

impl McpOAuthError {
    #[must_use]
    pub const fn is_needs_auth(&self) -> bool {
        matches!(self, Self::MissingTokens | Self::NeedsAuth(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_auth_matches_missing_and_reauth() {
        assert!(McpOAuthError::MissingTokens.is_needs_auth());
        assert!(McpOAuthError::NeedsAuth("expired".to_owned()).is_needs_auth());
        assert!(!McpOAuthError::UnsupportedTransport.is_needs_auth());
    }
}
