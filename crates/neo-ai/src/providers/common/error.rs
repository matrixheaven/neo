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
const PERMANENT_QUOTA_CODES: &[&str] = &[
    "insufficient_quota",
    "insufficient_balance",
    "billing_limit_exceeded",
    "usage_limit_exceeded",
    "payment_required",
];
const PERMANENT_QUOTA_PHRASES: &[&str] = &[
    "usage limit for this billing cycle",
    "purchase extra usage",
    "insufficient balance",
    "insufficient credits",
    "quota exhausted",
    "billing limit exceeded",
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

fn is_permanent_quota(message: &str) -> bool {
    let lower = message.to_lowercase();
    PERMANENT_QUOTA_PHRASES
        .iter()
        .any(|phrase| lower.contains(phrase))
        || lower
            .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .any(|token| PERMANENT_QUOTA_CODES.contains(&token))
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
    let raw_code = code.unwrap_or_default().trim().to_lowercase();
    let normalized = raw_code.replace(['-', ' '], "_");
    let status = if PERMANENT_QUOTA_CODES.contains(&raw_code.as_str()) {
        Some(402)
    } else {
        match normalized.as_str() {
            "408" | "stream_read_error" => Some(408),
            "429"
            | "rate_limit"
            | "rate_limit_error"
            | "rate_limit_exceeded"
            | "too_many_requests"
            | "resource_exhausted"
            | "quota_exceeded" => Some(429),
            "overload" | "overloaded" | "overloaded_error" => Some(529),
            "unavailable" | "service_unavailable" => Some(503),
            "server_error" | "internal" | "internal_server_error" | "api_error" | "5xx" => {
                Some(500)
            }
            "deadline_exceeded" => Some(504),
            value if value.len() == 3 => value.parse::<u16>().ok(),
            _ => None,
        }
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
                    402 => AiError::QuotaExhausted { message: excerpt },
                    403 | 429 if is_permanent_quota(&excerpt) => {
                        AiError::QuotaExhausted { message: excerpt }
                    }
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
#[path = "test_cases/classification.rs"]
mod classification;

#[cfg(test)]
#[path = "test_cases/retry_after.rs"]
mod retry_after;
