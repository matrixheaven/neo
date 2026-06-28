//! Unified error type shared across provider wire clients.

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

/// Format a human-readable HTTP status error message.
///
/// When `body` is `Some` and non-empty it is appended after the status code.
pub(crate) fn format_http_status_error(status: u16, body: &Option<String>) -> String {
    match body {
        Some(b) if !b.is_empty() => format!("http status {status}: {b}"),
        _ => format!("http status {status}"),
    }
}

/// Unified error type for all provider wire clients.
///
/// Variant set is the union of the four legacy private `ProviderError` enums.
/// `HttpStatus` carries an optional response body excerpt so providers that
/// capture error bodies (e.g. anthropic) can pass `Some(...)`, while those
/// that don't pass `None`.
#[derive(Debug)]
pub(crate) enum ProviderError {
    Header(String),
    HttpStatus { status: u16, body: Option<String> },
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

    /// Convert into the public [`AiError`] type.
    pub(crate) fn into_ai_error(self) -> AiError {
        match self {
            Self::Header(message)
            | Self::Stream(message)
            | Self::Url(message)
            | Self::Unsupported(message) => AiError::Stream(message),
            Self::HttpStatus { status, body } => {
                AiError::Stream(format_http_status_error(status, &body))
            }
            Self::Transport(err) => AiError::Stream(format!("transport error: {err}")),
        }
    }
}
