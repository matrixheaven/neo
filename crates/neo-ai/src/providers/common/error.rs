//! Unified error type shared across provider wire clients.

use std::time::Duration;

use crate::error::AiError;

/// Maximum number of characters retained from an HTTP error response body.
const MAX_HTTP_ERROR_BODY_CHARS: usize = 4096;
const CONTEXT_OVERFLOW_PATTERNS: &[&str] = &[
    "context_length",
    "context window",
    "maximum context",
    "exceed max tokens",
    "too many tokens",
    "prompt is too long",
    "token count exceeds",
    "token limit",
];

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
    if raw.contains("<title>")
        && let Some(start) = raw.find("<title>")
    {
        let title_start = start + 7;
        if let Some(end) = raw[title_start..].find("</title>") {
            let title = raw[title_start..title_start + end].trim();
            if !title.is_empty() {
                return title.replace('\r', "");
            }
        }
    }
    error_body_excerpt(&raw.replace('\r', ""))
}

/// Detect whether an error message indicates a context-length issue.
fn is_context_overflow(message: &str) -> bool {
    let lower = message.to_lowercase();
    CONTEXT_OVERFLOW_PATTERNS
        .iter()
        .any(|pattern| lower.contains(pattern))
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
        return Some(
            date.duration_since(std::time::SystemTime::now())
                .unwrap_or(Duration::ZERO),
        );
    }
    None
}

/// Unified error type for all provider wire clients.
///
/// Variant set is shared by all provider wire clients.
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
    Protocol(String),
    Url(String),
    Unsupported(String),
}

/// Classify an error reported inside an otherwise successful provider stream.
pub(crate) fn stream_failure(code: Option<&str>, message: impl Into<String>) -> ProviderError {
    let message = message.into();
    let normalized = code
        .unwrap_or_default()
        .trim()
        .replace('-', "_")
        .replace(' ', "_")
        .to_lowercase();
    let status = match normalized.as_str() {
        "408" => Some(408),
        "429"
        | "rate_limit"
        | "rate_limit_error"
        | "rate_limit_exceeded"
        | "too_many_requests"
        | "resource_exhausted"
        | "quota_exceeded" => Some(429),
        "overload" | "overloaded" | "overloaded_error" => Some(529),
        "unavailable" | "service_unavailable" => Some(503),
        "server_error" | "internal" | "internal_server_error" | "api_error" | "5xx" => Some(500),
        "deadline_exceeded" => Some(504),
        value if value.len() == 3 => value.parse::<u16>().ok(),
        _ => None,
    };

    match status {
        Some(status) => ProviderError::HttpStatus {
            status,
            body: Some(message),
            retry_after: None,
        },
        None => ProviderError::Protocol(message),
    }
}

impl ProviderError {
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
                    408 => AiError::Transport { message: excerpt },
                    s if s >= 500 => AiError::Server {
                        status,
                        message: excerpt,
                        retry_after,
                    },
                    _ => AiError::Protocol {
                        message: format!("http status {status}: {excerpt}"),
                    },
                }
            }
            Self::Transport(err) => AiError::Transport {
                message: err.to_string(),
            },
            Self::Header(message)
            | Self::Protocol(message)
            | Self::Url(message)
            | Self::Unsupported(message) => AiError::Protocol { message },
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
            retry_after: Some(Duration::from_secs(7)),
        };
        let ai = err.into_ai_error();
        assert!(matches!(
            ai,
            AiError::Server {
                status: 503,
                retry_after: Some(delay),
                ..
            } if delay == Duration::from_secs(7)
        ));
    }

    #[test]
    fn http_status_408_maps_to_retryable_transport() {
        let err = ProviderError::HttpStatus {
            status: 408,
            body: Some("Request Timeout".into()),
            retry_after: Some(Duration::from_secs(2)),
        };
        let ai = err.into_ai_error();
        assert!(ai.is_retryable());
        assert!(matches!(
            ai,
            AiError::Transport { message } if message == "Request Timeout"
        ));
    }

    #[test]
    fn streamed_status_408_maps_to_retryable_transport() {
        let ai = stream_failure(Some("408"), "request timeout").into_ai_error();
        assert!(matches!(
            ai,
            AiError::Transport { message } if message == "request timeout"
        ));
    }

    #[test]
    fn transport_display_prefixes_underlying_message_once() {
        let transport = reqwest::Client::new()
            .get("://")
            .build()
            .expect_err("invalid URL must fail");
        let underlying = transport.to_string();
        let ai = ProviderError::Transport(transport).into_ai_error();

        assert_eq!(ai.to_string(), format!("transport error: {underlying}"));
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
    fn http_status_413_without_context_pattern_maps_to_protocol() {
        let err = ProviderError::HttpStatus {
            status: 413,
            body: Some("Payload Too Large".into()),
            retry_after: None,
        };
        let ai = err.into_ai_error();
        assert_eq!(ai.code(), "provider.protocol_error");
    }

    #[test]
    fn sanitize_extracts_title_from_html() {
        // Body starts with "413 " not "<", so starts_with('<') would miss this.
        // contains("<title>") detects HTML anywhere in the body.
        let html =
            "413 <html>\r\n<head><title>413 Request Entity Too Large</title></head>\r\n</html>\r\n";
        let result = sanitize_error_body(Some(html));
        assert_eq!(result, "413 Request Entity Too Large");
    }

    #[test]
    fn sanitize_empty_title_falls_back_to_body() {
        let html = "<html><head><title>  </title></head><body>nginx</body></html>";
        let result = sanitize_error_body(Some(html));
        assert!(result.contains("nginx"));
    }

    #[test]
    fn is_context_overflow_detects_patterns() {
        assert!(is_context_overflow("Request exceeds context_length limit"));
        assert!(is_context_overflow(
            "prompt is too long for maximum context"
        ));
        assert!(!is_context_overflow("Payload Too Large"));
    }

    #[test]
    fn parse_retry_after_seconds() {
        assert_eq!(parse_retry_after("30"), Some(Duration::from_secs(30)));
        assert_eq!(parse_retry_after("  5  "), Some(Duration::from_secs(5)));
    }

    #[test]
    fn parse_retry_after_past_http_date_returns_zero() {
        assert_eq!(
            parse_retry_after("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn parse_retry_after_invalid_returns_none() {
        assert_eq!(parse_retry_after("not a number"), None);
        assert_eq!(parse_retry_after(""), None);
    }
}
