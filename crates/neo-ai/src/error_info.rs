//! Metadata registry for error codes — provides title, retryability hint,
//! and user-facing remediation action for each [`crate::AiError::code`].

/// Metadata for a specific error code.
#[derive(Debug, Clone)]
pub struct NeoErrorInfo {
    /// Short human-readable title (e.g. "Rate Limited").
    pub title: &'static str,
    /// Whether this error category is retryable.
    pub retryable: bool,
    /// Optional user-facing remediation hint (e.g. "Check API key").
    pub action: Option<&'static str>,
}

impl NeoErrorInfo {
    /// Fallback info for unknown error codes.
    #[must_use]
    pub const fn fallback() -> Self {
        Self {
            title: "Error",
            retryable: false,
            action: None,
        }
    }
}

/// Look up metadata for a stable error code string.
///
/// See [`crate::AiError::code`] for the code format (`domain.reason`).
#[must_use]
pub fn error_info(code: &str) -> NeoErrorInfo {
    match code {
        "config.invalid" => info("Configuration Error", false, Some("Check ~/.neo/config.toml")),
        "provider.rate_limit" => info("Rate Limited", true, Some("Will auto-retry with backoff")),
        "provider.auth_error" => info("Authentication Failed", false, Some("Check API key")),
        "provider.context_overflow" => info("Context Overflow", false, Some("Run /compact")),
        "provider.server_error" => info("Server Error", true, Some("Will auto-retry")),
        "provider.network_error" => info("Network Error", true, Some("Check connection")),
        "provider.stream_error" => info("Stream Error", false, None),
        "request.cancelled" => info("Cancelled", false, None),
        _ => NeoErrorInfo::fallback(),
    }
}

const fn info(
    title: &'static str,
    retryable: bool,
    action: Option<&'static str>,
) -> NeoErrorInfo {
    NeoErrorInfo {
        title,
        retryable,
        action,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_codes_return_specific_info() {
        let info = error_info("provider.rate_limit");
        assert_eq!(info.title, "Rate Limited");
        assert!(info.retryable);
        assert_eq!(info.action, Some("Will auto-retry with backoff"));
    }

    #[test]
    fn unknown_code_returns_fallback() {
        let info = error_info("something.unknown");
        assert_eq!(info.title, "Error");
        assert!(!info.retryable);
        assert_eq!(info.action, None);
    }

    #[test]
    fn context_overflow_has_compact_action() {
        let info = error_info("provider.context_overflow");
        assert_eq!(info.action, Some("Run /compact"));
    }
}
