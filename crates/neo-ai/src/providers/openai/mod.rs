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
use crate::providers::common::http::inject_extra_headers;

pub mod compatible;
pub mod images;
pub mod responses;

/// Build HTTP headers for `OpenAI` API requests.
///
/// Inserts the `Authorization: Bearer {api_key}` header, applies any extra
/// headers from the request options, and optionally sets `x-client-request-id`
/// when a session id is provided.
pub(crate) fn headers(
    api_key: &str,
    extra_headers: &BTreeMap<String, String>,
    session_id: Option<&str>,
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

        let headers = headers("api-key", &extra_headers, None).unwrap();
        let authorization = headers.get(AUTHORIZATION).unwrap();

        assert_eq!(authorization, "Bearer override-key");
        assert!(authorization.is_sensitive());
        assert!(!headers.get("x-test").unwrap().is_sensitive());
    }
}
