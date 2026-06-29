//! Shared HTTP helpers: retry loop with exponential backoff, and header injection.
//!
//! These are byte-for-byte identical across the four provider wire clients
//! (`openai::responses`, `anthropic`, `openai::compatible`, `google`), so they
//! live here to avoid duplication.

use std::collections::BTreeMap;
use std::time::Duration;

use futures::future::BoxFuture;
use rand::Rng;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio_util::sync::CancellationToken;

use super::error::ProviderError;
use crate::{AiError, ChatRequest};

/// Minimum backoff delay (first retry attempt).
const RETRY_MIN_MS: u64 = 300;
/// Maximum backoff delay cap.
const RETRY_MAX_MS: u64 = 5_000;
/// Exponential growth factor.
const RETRY_FACTOR: f64 = 2.0;
/// Default total attempts when `request.options.retries` is unset.
const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// Compute the backoff delay for a given attempt number.
///
/// 1. If the error carries a `Retry-After` value, use it (capped at `RETRY_MAX_MS`).
/// 2. Otherwise use exponential backoff: `RETRY_MIN_MS * 2^attempt`, capped at `RETRY_MAX_MS`.
/// 3. Apply ±25% jitter to the computed value.
fn compute_backoff_delay(err: &ProviderError, attempt: u32) -> Duration {
    // 1. Retry-After header takes precedence (no jitter — server told us to wait)
    if let ProviderError::HttpStatus {
        retry_after: Some(ra),
        ..
    } = err
    {
        return (*ra).min(Duration::from_millis(RETRY_MAX_MS));
    }
    // 2. Exponential backoff
    let base = RETRY_MIN_MS as f64;
    let exp = RETRY_FACTOR.powi(attempt as i32);
    let raw = (base * exp).min(RETRY_MAX_MS as f64);
    // 3. Jitter: ±25% (rand 0.9 API: rand::rng())
    let jitter = rand::rng().random_range(0.75..1.25);
    Duration::from_millis((raw * jitter) as u64)
}

/// Sleep for `duration`, but abort early if `cancel_token` is cancelled.
async fn sleep_cancellable(duration: Duration, token: &CancellationToken) -> Result<(), ()> {
    tokio::select! {
        biased;
        _ = token.cancelled() => Err(()),
        _ = tokio::time::sleep(duration) => Ok(()),
    }
}

/// Retry loop shared by all provider wire clients.
///
/// Calls `once` up to `request.options.retries + 1` times (or
/// `DEFAULT_MAX_ATTEMPTS` if unset), retrying on
/// [`ProviderError::is_retryable`] errors with exponential backoff + jitter.
///
/// The backoff sleep is cancellable: if `request.options.cancel_token` is set
/// and the token is cancelled during a backoff sleep, the loop aborts early
/// with [`AiError::Cancelled`].
pub(crate) async fn open_response<'a>(
    request: &'a ChatRequest,
    once: impl Fn(&'a ChatRequest) -> BoxFuture<'a, Result<reqwest::Response, ProviderError>>,
) -> Result<reqwest::Response, AiError> {
    let max_attempts = request
        .options
        .retries
        .map(|r| r.saturating_add(1))
        .unwrap_or(DEFAULT_MAX_ATTEMPTS);

    // Extract cancel token from RequestOptions (set by the runtime).
    // If unset, create a standalone token that never fires.
    let cancel_token = request
        .options
        .cancel_token
        .as_deref()
        .cloned()
        .unwrap_or_default();

    let mut last_error = None;

    for attempt in 0..max_attempts {
        match once(request).await {
            Ok(response) => return Ok(response),
            Err(err) if attempt + 1 < max_attempts && err.is_retryable() => {
                let delay = compute_backoff_delay(&err, attempt);
                last_error = Some(err);
                if sleep_cancellable(delay, &cancel_token).await.is_err() {
                    return Err(AiError::Cancelled);
                }
            }
            Err(err) => return Err(err.into_ai_error()),
        }
    }

    Err(last_error.map_or_else(
        || AiError::Stream {
            message: "provider request failed without an error".to_owned(),
        },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_delay_grows_exponentially() {
        let err = ProviderError::HttpStatus {
            status: 429,
            body: None,
            retry_after: None,
        };
        let d0 = compute_backoff_delay(&err, 0).as_millis();
        let d2 = compute_backoff_delay(&err, 2).as_millis();
        // d0 should be roughly 300ms (225–375ms with ±25% jitter)
        assert!(d0 >= 200 && d0 <= 400, "d0={d0}");
        // d2 should be roughly 1200ms (900–1500ms with ±25% jitter)
        assert!(d2 >= 800 && d2 <= 1600, "d2={d2}");
    }

    #[test]
    fn backoff_delay_capped_at_max() {
        let err = ProviderError::HttpStatus {
            status: 429,
            body: None,
            retry_after: None,
        };
        let d = compute_backoff_delay(&err, 10).as_millis();
        // Even at attempt 10, should be capped at 5s (±25%: 3750–6250ms)
        assert!(d <= 6500, "d={d}");
    }

    #[test]
    fn retry_after_takes_precedence() {
        let err = ProviderError::HttpStatus {
            status: 429,
            body: None,
            retry_after: Some(std::time::Duration::from_secs(10)),
        };
        let d = compute_backoff_delay(&err, 0);
        // Retry-After of 10s should be capped at RETRY_MAX_MS (5s)
        assert_eq!(d, std::time::Duration::from_millis(5_000));
    }
}
