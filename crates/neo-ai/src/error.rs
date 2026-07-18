use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AiError {
    #[error("configuration error: {message}")]
    Configuration { message: String },

    #[error("rate limited: {message}")]
    RateLimit {
        message: String,
        retry_after: Option<Duration>,
    },

    #[error("quota exhausted: {message}")]
    QuotaExhausted { message: String },

    #[error("authentication error: {message}")]
    Auth { message: String },

    #[error("context overflow: {message}")]
    ContextOverflow { message: String },

    #[error("server error ({status}): {message}")]
    Server {
        status: u16,
        message: String,
        retry_after: Option<Duration>,
    },

    #[error("transport error: {message}")]
    Transport { message: String },

    #[error("protocol error: {message}")]
    Protocol { message: String },

    #[error("request was cancelled")]
    Cancelled,
}

impl AiError {
    /// Stable string code for JSONL serialization — `domain.reason` format.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Configuration { .. } => "config.invalid",
            Self::RateLimit { .. } => "provider.rate_limit",
            Self::QuotaExhausted { .. } => "provider.quota_exhausted",
            Self::Auth { .. } => "provider.auth_error",
            Self::ContextOverflow { .. } => "provider.context_overflow",
            Self::Server { .. } => "provider.server_error",
            Self::Transport { .. } => "provider.transport_error",
            Self::Protocol { .. } => "provider.protocol_error",
            Self::Cancelled => "request.cancelled",
        }
    }

    /// Whether a failed request is worth retrying.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        match self {
            Self::RateLimit { .. } | Self::Server { .. } | Self::Transport { .. } => true,
            Self::Configuration { .. }
            | Self::QuotaExhausted { .. }
            | Self::Auth { .. }
            | Self::ContextOverflow { .. }
            | Self::Protocol { .. }
            | Self::Cancelled => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn code_returns_domain_dot_reason() {
        assert_eq!(
            AiError::Configuration {
                message: "x".into()
            }
            .code(),
            "config.invalid"
        );
        assert_eq!(
            AiError::RateLimit {
                message: "x".into(),
                retry_after: None
            }
            .code(),
            "provider.rate_limit"
        );
        assert_eq!(
            AiError::QuotaExhausted {
                message: "buy more credits".into(),
            }
            .code(),
            "provider.quota_exhausted"
        );
        assert_eq!(
            AiError::Auth {
                message: "x".into()
            }
            .code(),
            "provider.auth_error"
        );
        assert_eq!(
            AiError::ContextOverflow {
                message: "x".into()
            }
            .code(),
            "provider.context_overflow"
        );
        assert_eq!(
            AiError::Server {
                status: 500,
                message: "x".into(),
                retry_after: None
            }
            .code(),
            "provider.server_error"
        );
        assert_eq!(
            AiError::Transport {
                message: "x".into()
            }
            .code(),
            "provider.transport_error"
        );
        assert_eq!(
            AiError::Protocol {
                message: "x".into()
            }
            .code(),
            "provider.protocol_error"
        );
        assert_eq!(AiError::Cancelled.code(), "request.cancelled");
    }

    #[test]
    fn is_retryable_for_each_variant() {
        assert!(
            AiError::RateLimit {
                message: String::new(),
                retry_after: Some(Duration::from_secs(5))
            }
            .is_retryable()
        );
        assert!(
            !AiError::QuotaExhausted {
                message: "buy more credits".into(),
            }
            .is_retryable()
        );
        assert!(
            AiError::Transport {
                message: String::new()
            }
            .is_retryable()
        );
        assert!(
            AiError::Server {
                status: 503,
                message: String::new(),
                retry_after: None
            }
            .is_retryable()
        );
        assert!(
            !AiError::Protocol {
                message: "invalid json".into()
            }
            .is_retryable()
        );
        assert!(
            !AiError::Configuration {
                message: String::new()
            }
            .is_retryable()
        );
        assert!(
            !AiError::Auth {
                message: String::new()
            }
            .is_retryable()
        );
        assert!(
            !AiError::ContextOverflow {
                message: String::new()
            }
            .is_retryable()
        );
        assert!(!AiError::Cancelled.is_retryable());
    }
}
