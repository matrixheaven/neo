//! Shared HTTP helpers: retry loop and extra-header injection.
//!
//! These are byte-for-byte identical across the four provider wire clients
//! (`openai::responses`, `anthropic`, `openai::compatible`, `google`), so they
//! live here to avoid duplication.

use std::collections::BTreeMap;

use futures::future::BoxFuture;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use super::error::ProviderError;
use crate::{AiError, ChatRequest};

/// Retry loop shared by all provider wire clients.
///
/// Calls `once` up to `request.options.retries + 1` times, retrying on
/// [`ProviderError::is_retryable`] errors.
pub(crate) async fn open_response<'a>(
    request: &'a ChatRequest,
    once: impl Fn(&'a ChatRequest) -> BoxFuture<'a, Result<reqwest::Response, ProviderError>>,
) -> Result<reqwest::Response, AiError>
{
    let attempts = request.options.retries.unwrap_or(0).saturating_add(1);
    let mut last_error = None;

    for attempt in 0..attempts {
        match once(request).await {
            Ok(response) => return Ok(response),
            Err(err) if attempt + 1 < attempts && err.is_retryable() => {
                last_error = Some(err);
            }
            Err(err) => return Err(err.into_ai_error()),
        }
    }

    Err(last_error.map_or_else(
        || AiError::Stream { message: "provider request failed without an error".to_owned() },
        ProviderError::into_ai_error,
    ))
}

/// Insert each `(name, value)` pair from `extra` into `headers`.
///
/// Returns [`ProviderError::Header`] on an invalid header name or value.
pub(crate) fn inject_extra_headers(
    headers: &mut HeaderMap,
    extra: &BTreeMap<String, String>,
) -> Result<(), ProviderError> {
    for (name, value) in extra {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|err| ProviderError::Header(format!("invalid header name {name}: {err}")))?;
        let value = HeaderValue::from_str(value)
            .map_err(|err| ProviderError::Header(format!("invalid header value {name}: {err}")))?;
        headers.insert(name, value);
    }
    Ok(())
}
