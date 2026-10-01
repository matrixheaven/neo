//! OpenAI-family provider wire clients.
//!
//! This module groups the three `OpenAI`-compatible API flavors:
//! - [`responses`]: `OpenAI` Responses API (`/responses` endpoint)
//! - [`compatible`]: `OpenAI` Chat Completions API (`/chat/completions` endpoint)
//! - [`images`]: `OpenAI` Images API (image generation)

use std::collections::BTreeMap;

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};

use crate::ImageData;
use crate::providers::common::error::ProviderError;
use crate::providers::common::http::{gateway_session_value, inject_extra_headers};

pub mod compatible;
pub mod images;
pub mod responses;

/// Build HTTP headers for `OpenAI` API requests.
///
/// Inserts the `Authorization: Bearer {api_key}` header, applies any extra
/// headers from the request options, and optionally sets `x-client-request-id`
/// when a session id is provided. `gateway_session_header` comes from
/// [`crate::providers::common::http::gateway_session_header`]: a gateway that
/// routes on its own session header receives the same conversation id there.
pub(crate) fn headers(
    api_key: &str,
    extra_headers: &BTreeMap<String, String>,
    session_id: Option<&str>,
    gateway_session_header: Option<&HeaderName>,
) -> Result<HeaderMap, ProviderError> {
    let mut headers = HeaderMap::new();
    let mut authorization = HeaderValue::from_str(&format!("Bearer {api_key}"))
        .map_err(|err| ProviderError::Header(format!("invalid authorization header: {err}")))?;
    authorization.set_sensitive(true);
    headers.insert(AUTHORIZATION, authorization);

    inject_extra_headers(&mut headers, extra_headers)?;
    headers
        .get_mut(AUTHORIZATION)
        .expect("authorization header is always present")
        .set_sensitive(true);
    if let Some(name) = gateway_session_header {
        let value = HeaderValue::from_str(&gateway_session_value(session_id))
            .map_err(|err| ProviderError::Header(format!("invalid {name} header: {err}")))?;
        headers.insert(name.clone(), value);
    }
    if let Some(session_id) = session_id {
        let value = HeaderValue::from_str(session_id).map_err(|err| {
            ProviderError::Header(format!("invalid x-client-request-id header: {err}"))
        })?;
        headers.insert(HeaderName::from_static("x-client-request-id"), value);
    }

    Ok(headers)
}

/// Construct an `OpenAI` image URL value from base64-encoded data or a URL.
pub(crate) fn image_url(mime_type: &str, data: &ImageData) -> String {
    match data {
        ImageData::Base64(data) => format!("data:{mime_type};base64,{data}"),
        ImageData::Url(url) => url.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_header_remains_sensitive_when_overridden() {
        let extra_headers = BTreeMap::from([
            ("authorization".to_owned(), "Bearer override-key".to_owned()),
            ("x-test".to_owned(), "ordinary".to_owned()),
        ]);

        let headers = headers("api-key", &extra_headers, None, None).unwrap();
        let authorization = headers.get(AUTHORIZATION).unwrap();

        assert_eq!(authorization, "Bearer override-key");
        assert!(authorization.is_sensitive());
        assert!(!headers.get("x-test").unwrap().is_sensitive());
    }

    #[test]
    fn gateway_session_header_carries_the_conversation_id() {
        let name = HeaderName::from_static("x-opencode-session");
        let headers = headers(
            "api-key",
            &BTreeMap::new(),
            Some("session_0001"),
            Some(&name),
        )
        .expect("headers");

        assert_eq!(headers.get(&name).unwrap(), "session_0001");
        assert_eq!(headers.get("x-client-request-id").unwrap(), "session_0001");
    }

    #[test]
    fn gateway_session_header_is_present_without_a_persisted_session() {
        let name = HeaderName::from_static("x-opencode-session");
        let headers = headers("api-key", &BTreeMap::new(), None, Some(&name)).expect("headers");

        assert!(
            headers.get(&name).is_some_and(|value| !value.is_empty()),
            "a gateway that requires the header must never receive a request without it"
        );
        assert!(headers.get("x-client-request-id").is_none());
    }
}
