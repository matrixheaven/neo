//! Unified error type shared across provider wire clients.

use std::time::Duration;

use crate::error::AiError;

/// Maximum number of characters retained from an HTTP error response body.
const MAX_HTTP_ERROR_BODY_CHARS: usize = 4096;

/// Truncate `body` to [`MAX_HTTP_ERROR_BODY_CHARS`] characters, appending `...`
/// if truncation occurred. Leading/trailing whitespace is trimmed first.
pub(crate) fn error_body_excerpt(body: &str) -> String {
    let trimmed = body.trim();
    let mut chars = trimmed.chars();
    let excerpt = chars
        .by_ref()
        .take(MAX_HTTP_ERROR_BODY_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        format!("{excerpt}...")
    } else {
        excerpt
    }
}

/// Sanitize an HTTP error response body into a human-readable message.
///
/// If the body contains a `<title>` tag (common for nginx/proxy error pages),
/// extract its text. Carriage returns are stripped. The result is truncated
/// to 4096 chars via [`error_body_excerpt`].
///
/// Note: detection uses `contains("<title>")` rather than `starts_with('<')`
/// because some bodies are prefixed with a status code (e.g. `"413 <html>..."`).
pub(crate) fn sanitize_error_body(body: Option<&str>) -> String {
    let raw = body.unwrap_or("").trim();
    if raw.contains("<title>") {
        if let Some(start) = raw.find("<title>") {
            let title_start = start + 7;
            if let Some(end) = raw[title_start..].find("</title>") {
                let title = raw[title_start..title_start + end].trim();
                if !title.is_empty() {
                    return title.replace('\r', "");
                }
            }
        }
    }
    error_body_excerpt(&raw.replace('\r', ""))
}

/// Detect whether an error message indicates a context-length issue.
fn is_context_overflow(message: &str) -> bool {
    let lower = message.to_lowercase();
    const PATTERNS: &[&str] = &[
        "context_length",
        "context window",
        "maximum context",
        "exceed max tokens",
        "too many tokens",
        "prompt is too long",
        "token count exceeds",
        "token limit",
    ];
    PATTERNS.iter().any(|p| lower.contains(p))
}

/// Parse an HTTP `Retry-After` header value into a `Duration`.
///
/// Supports both delta-seconds (integer) and HTTP-date formats per RFC 7231.
pub(crate) fn parse_retry_after(value: &str) -> Option<Duration> {
    // Try integer seconds first (most common)
    if let Ok(secs) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    // Try HTTP-date format
    if let Ok(date) = httpdate::parse_http_date(value.trim()) {
        return date.duration_since(std::time::SystemTime::now()).ok();
    }
    None
}

/// Unified error type for all provider wire clients.
///
/// Variant set is the union of the four legacy private `ProviderError` enums.
/// `HttpStatus` carries an optional response body excerpt and an optional
/// `Retry-After` duration parsed from the response headers.
#[derive(Debug)]
pub(crate) enum ProviderError {
    Header(String),
    HttpStatus {
        status: u16,
        body: Option<String>,
        retry_after: Option<Duration>,
    },
    Transport(reqwest::Error),
    Stream(String),
    Url(String),
    Unsupported(String),
}

impl ProviderError {
    /// Whether a failed request is worth retrying.
    pub(crate) const fn is_retryable(&self) -> bool {
        match self {
            Self::HttpStatus { status, .. } => *status == 429 || *status >= 500,
            Self::Transport(_) => true,
            Self::Header(_) | Self::Stream(_) | Self::Url(_) | Self::Unsupported(_) => false,
        }
    }

    /// Convert into the public [`AiError`] type, branching by HTTP status.
    pub(crate) fn into_ai_error(self) -> AiError {
        match self {
            Self::HttpStatus {
                status,
                body,
                retry_after,
            } => {
                let excerpt = sanitize_error_body(body.as_deref());
                match status {
                    429 => AiError::RateLimit {
                        message: excerpt,
                        retry_after,
                    },
                    401 | 403 => AiError::Auth { message: excerpt },
                    400 | 413 | 422 if is_context_overflow(&excerpt) => {
                        AiError::ContextOverflow { message: excerpt }
                    }
                    s if s >= 500 => AiError::Server {
                        status,
                        message: excerpt,
                    },
                    _ => AiError::Stream {
                        message: format!("http status {status}: {excerpt}"),
                    },
                }
            }
            Self::Transport(err) => AiError::Network {
                message: format!("transport error: {err}"),
            },
            Self::Header(message)
            | Self::Stream(message)
            | Self::Url(message)
            | Self::Unsupported(message) => AiError::Stream { message },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn http_status_429_maps_to_rate_limit() {
        let err = ProviderError::HttpStatus {
            status: 429,
            body: Some("Too Many Requests".into()),
            retry_after: Some(Duration::from_secs(30)),
        };
        let ai = err.into_ai_error();
        assert_eq!(ai.code(), "provider.rate_limit");
    }

    #[test]
    fn http_status_401_maps_to_auth() {
        let err = ProviderError::HttpStatus {
            status: 401,
            body: Some("Unauthorized".into()),
            retry_after: None,
        };
        assert_eq!(err.into_ai_error().code(), "provider.auth_error");
    }

    #[test]
    fn http_status_503_maps_to_server() {
        let err = ProviderError::HttpStatus {
            status: 503,
            body: Some("Service Unavailable".into()),
            retry_after: None,
        };
        let ai = err.into_ai_error();
        assert_eq!(ai.code(), "provider.server_error");
    }

    #[test]
    fn http_status_413_with_context_overflow_maps_to_context_overflow() {
        let err = ProviderError::HttpStatus {
            status: 413,
            body: Some("Request too large: context_length exceeded".into()),
            retry_after: None,
        };
        assert_eq!(err.into_ai_error().code(), "provider.context_overflow");
    }

    #[test]
    fn http_status_413_without_context_pattern_maps_to_stream() {
        let err = ProviderError::HttpStatus {
            status: 413,
            body: Some("Payload Too Large".into()),
            retry_after: None,
        };
        let ai = err.into_ai_error();
        assert_eq!(ai.code(), "provider.stream_error");
    }

    #[test]
    fn sanitize_extracts_title_from_html() {
        // Body starts with "413 " not "<", so starts_with('<') would miss this.
        // contains("<title>") detects HTML anywhere in the body.
        let html = "413 <html>\r\n<head><title>413 Request Entity Too Large</title></head>\r\n</html>\r\n";
        let result = sanitize_error_body(Some(html));
        assert_eq!(result, "413 Request Entity Too Large");
    }

    #[test]
    fn sanitize_strips_carriage_returns() {
        let result = sanitize_error_body(Some("line1\r\nline2\r"));
        assert_eq!(result, "line1\nline2");
    }

    #[test]
    fn sanitize_empty_title_falls_back_to_body() {
        let html = "<html><head><title>  </title></head><body>nginx</body></html>";
        let result = sanitize_error_body(Some(html));
        assert!(result.contains("nginx"));
    }

    #[test]
    fn sanitize_plain_text_unchanged() {
        let result = sanitize_error_body(Some("just text"));
        assert_eq!(result, "just text");
    }

    #[test]
    fn sanitize_none_body_returns_empty() {
        let result = sanitize_error_body(None);
        assert_eq!(result, "");
    }

    #[test]
    fn is_context_overflow_detects_patterns() {
        assert!(is_context_overflow("Request exceeds context_length limit"));
        assert!(is_context_overflow("prompt is too long for maximum context"));
        assert!(!is_context_overflow("Payload Too Large"));
    }

    #[test]
    fn parse_retry_after_seconds() {
        assert_eq!(parse_retry_after("30"), Some(Duration::from_secs(30)));
        assert_eq!(parse_retry_after("  5  "), Some(Duration::from_secs(5)));
    }

    #[test]
    fn parse_retry_after_invalid_returns_none() {
        assert_eq!(parse_retry_after("not a number"), None);
        assert_eq!(parse_retry_after(""), None);
    }
}
