use thiserror::Error;

use crate::{ToolError, compaction};

#[derive(Debug, Error)]
pub enum AgentRuntimeError {
    #[error("model stream failed: {0}")]
    Model(#[from] neo_ai::AiError),
    #[error("tool execution failed: {0}")]
    Tool(#[from] ToolError),
    #[error("runtime I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("compaction failed: {0}")]
    Compaction(#[from] compaction::CompactionError),
    #[error("turn cancelled")]
    Cancelled,
}

impl AgentRuntimeError {
    /// Return the stable error code if this is a model-level error.
    ///
    /// Delegates to [`neo_ai::AiError::code`] for the [`Self::Model`] variant.
    /// Returns `None` for all other variants (tool errors, I/O, compaction,
    /// cancellation) which don't have provider-level codes.
    #[must_use]
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Model(ai) => Some(ai.code()),
            Self::Tool(_) | Self::Io(_) | Self::Compaction(_) | Self::Cancelled => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_error_code_delegates_to_ai_error() {
        let err = AgentRuntimeError::Model(neo_ai::AiError::RateLimit {
            message: "too many requests".into(),
            retry_after: None,
        });
        assert_eq!(err.code(), Some("provider.rate_limit"));
    }

    #[test]
    fn model_error_code_for_network() {
        let err = AgentRuntimeError::Model(neo_ai::AiError::Network {
            message: "timeout".into(),
        });
        assert_eq!(err.code(), Some("provider.network_error"));
    }

    #[test]
    fn non_model_error_returns_none() {
        assert_eq!(AgentRuntimeError::Cancelled.code(), None);
    }
}
