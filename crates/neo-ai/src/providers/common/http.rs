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

/// Gateway-specific conversation header this base URL requires on every
/// request.
///
/// `OpenCode`'s router (Zen and Go) answers `400 MissingSessionID` unless each
/// request carries a stable per-conversation id in `x-opencode-session`. No
/// other endpoint defines such a header, so it is sent only to those hosts.
pub(crate) fn gateway_session_header(base_url: &str) -> Option<HeaderName> {
    let host = reqwest::Url::parse(base_url).ok()?;
    let host = host.host_str()?.to_ascii_lowercase();
    (host == "opencode.ai" || host.ends_with(".opencode.ai"))
        .then(|| HeaderName::from_static("x-opencode-session"))
}

/// Value for a gateway-required session header: the conversation's own id when
/// the run has one (Neo's session directory name), else a process-stable id so
/// a sessionless run still routes as a single conversation.
pub(crate) fn gateway_session_value(session_id: Option<&str>) -> String {
    session_id.map_or_else(process_session_id, str::to_owned)
}

fn process_session_id() -> String {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| format!("neo-{:032x}", rand::random::<u128>()))
        .clone()
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

    #[test]
    fn gateway_session_header_only_covers_opencode_hosts() {
        for base_url in [
            "https://opencode.ai/zen/go/v1",
            "https://opencode.ai/zen/v1",
            "https://zen.opencode.ai/v1",
        ] {
            assert_eq!(
                gateway_session_header(base_url)
                    .as_ref()
                    .map(HeaderName::as_str),
                Some("x-opencode-session"),
                "{base_url} must carry the gateway session header"
            );
        }

        for base_url in [
            "https://api.openai.com/v1",
            "https://api.deepseek.com/anthropic",
            "https://api.anthropic.com/v1",
            "http://127.0.0.1:8080/v1",
            "https://notopencode.ai/v1",
        ] {
            assert!(
                gateway_session_header(base_url).is_none(),
                "{base_url} must not carry a gateway session header"
            );
        }
    }

    #[test]
    fn gateway_session_value_prefers_the_conversation_id() {
        assert_eq!(gateway_session_value(Some("session_0001")), "session_0001");
        let first = gateway_session_value(None);
        assert!(!first.is_empty());
        assert_eq!(first, gateway_session_value(None));
    }
}
