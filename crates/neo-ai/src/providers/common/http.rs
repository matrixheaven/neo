//! Shared HTTP helpers for provider wire clients.

use std::collections::BTreeMap;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use super::error::{ProviderError, error_body_excerpt, parse_retry_after};

const ERROR_BODY_LIMIT: usize = 64 * 1024;

pub(crate) fn request_url(base_url: &str, path: &str) -> Result<reqwest::Url, ProviderError> {
    let url = reqwest::Url::parse(&format!("{base_url}{path}"))
        .map_err(|err| ProviderError::Url(format!("invalid provider URL: {err}")))?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err(ProviderError::Url(
            "invalid provider URL: expected HTTP or HTTPS with a host".to_owned(),
        ));
    }
    Ok(url)
}

pub(crate) async fn http_status_error(mut response: reqwest::Response) -> ProviderError {
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after);
    let mut bytes = Vec::new();

    while bytes.len() < ERROR_BODY_LIMIT {
        let Ok(Some(chunk)) = response.chunk().await else {
            break;
        };
        let remaining = ERROR_BODY_LIMIT - bytes.len();
        bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }

    let body = if bytes.is_empty() {
        None
    } else {
        Some(error_body_excerpt(&String::from_utf8_lossy(&bytes)))
    };

    ProviderError::HttpStatus {
        status,
        body,
        retry_after,
    }
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
    fn invalid_request_url_is_not_retryable() {
        for base_url in [
            "not a URL",
            "file:///tmp/provider",
            "mailto:user@example.com",
        ] {
            let error = request_url(base_url, "/messages")
                .unwrap_err()
                .into_ai_error();

            assert!(matches!(error, crate::AiError::Protocol { .. }));
            assert!(!error.is_retryable());
        }
    }
}
