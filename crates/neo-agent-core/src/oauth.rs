//! OAuth 2.0 authorization-code flow with PKCE.
//!
//! This module implements the provider-agnostic pieces of the local OAuth
//! authenticator: PKCE verifier/challenge generation, authorization URL
//! construction, code exchange, and token refresh. It intentionally does not
//! start a callback server or open a browser; those live in higher-level glue.

use base64::{Engine, prelude::BASE64_URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use thiserror::Error;

pub mod callback_server;

/// A set of OAuth tokens returned by the token endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthTokenSet {
    pub access_token: String,
    pub token_type: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub scopes: Vec<String>,
}

/// Provider configuration for the OAuth flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthProvider {
    pub id: String,
    pub client_id: String,
    pub auth_url: String,
    pub token_url: String,
    pub scopes: Vec<String>,
    pub default_callback_port: u16,
}

/// Errors that can occur during OAuth operations.
#[derive(Debug, Error)]
pub enum OAuthError {
    #[error("failed to build authorization URL: {0}")]
    AuthorizationUrl(String),
    #[error("token endpoint request failed: {0}")]
    TokenRequest(#[from] reqwest::Error),
    #[error("token endpoint returned error: {status} {body}")]
    TokenEndpoint {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("failed to parse token response: {0}")]
    TokenParse(#[from] serde_json::Error),
    #[error("callback server error: {0}")]
    CallbackServer(String),
    #[error("callback timed out after {0:?}")]
    CallbackTimeout(std::time::Duration),
    #[error("callback state mismatch: expected {expected}, got {got}")]
    CallbackStateMismatch { expected: String, got: String },
    #[error("callback request missing authorization code")]
    CallbackMissingCode,
}

/// Raw token response from a standard OAuth 2.0 token endpoint.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    scope: Option<String>,
}

/// Number of random bytes in a PKCE code verifier.
const PKCE_VERIFIER_LEN: usize = 32;

/// Generate a PKCE code verifier and the corresponding S256 code challenge.
///
/// Returns `(verifier, challenge)`. The verifier is a 32-byte random value
/// base64-url encoded without padding (43 characters). The challenge is the
/// SHA-256 digest of the verifier, base64-url encoded without padding.
#[must_use]
pub fn generate_pkce() -> (String, String) {
    let mut verifier_bytes = [0u8; PKCE_VERIFIER_LEN];
    rand::rng().fill(&mut verifier_bytes);
    let verifier = BASE64_URL_SAFE_NO_PAD.encode(verifier_bytes);
    let challenge = BASE64_URL_SAFE_NO_PAD.encode(Sha256::digest(&verifier));
    (verifier, challenge)
}

fn callback_uri(port: u16) -> String {
    format!("http://127.0.0.1:{port}/callback")
}

/// Build the provider authorization URL for the first step of the flow.
///
/// The URL includes `response_type=code`, the provider's `client_id`, a local
/// `redirect_uri`, the requested `scope`, the caller-supplied `state`, and the
/// PKCE `code_challenge` with method `S256`.
pub fn build_authorization_url(
    provider: &OAuthProvider,
    state: &str,
    challenge: &str,
) -> Result<reqwest::Url, OAuthError> {
    let port = if provider.default_callback_port == 0 {
        // A port of zero means the caller will bind to a free port and must
        // substitute the real port before opening the URL in a browser.
        0
    } else {
        provider.default_callback_port
    };
    let redirect_uri = callback_uri(port);

    let mut url = reqwest::Url::parse(&provider.auth_url)
        .map_err(|err| OAuthError::AuthorizationUrl(err.to_string()))?;

    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &provider.client_id)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("scope", &provider.scopes.join(" "))
            .append_pair("state", state)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256");
    }

    Ok(url)
}

fn token_set_from_response(response: TokenResponse) -> OAuthTokenSet {
    let expires_at = response
        .expires_in
        .map(|seconds| Utc::now() + Duration::seconds(seconds));
    let scopes = response
        .scope
        .map(|s| s.split_whitespace().map(String::from).collect())
        .unwrap_or_default();

    OAuthTokenSet {
        access_token: response.access_token,
        token_type: response.token_type.unwrap_or_else(|| "Bearer".to_string()),
        refresh_token: response.refresh_token,
        expires_at,
        scopes,
    }
}

/// Exchange an authorization code for tokens.
///
/// This function creates its own `reqwest` client and POSTs to the provider's
/// `token_url` with the required PKCE parameters.
pub async fn exchange_code_for_token(
    provider: &OAuthProvider,
    code: &str,
    verifier: &str,
) -> Result<OAuthTokenSet, OAuthError> {
    let redirect_uri = callback_uri(provider.default_callback_port);
    let mut params = HashMap::new();
    params.insert("grant_type", "authorization_code");
    params.insert("code", code);
    params.insert("redirect_uri", &redirect_uri);
    params.insert("client_id", &provider.client_id);
    params.insert("code_verifier", verifier);

    post_token_request(&provider.token_url, params).await
}

/// Refresh an access token using a refresh token.
///
/// The provider's scopes are sent along with the request so the token endpoint
/// can narrow or preserve the originally granted scopes.
pub async fn refresh_access_token(
    provider: &OAuthProvider,
    refresh_token: &str,
) -> Result<OAuthTokenSet, OAuthError> {
    let mut params = HashMap::new();
    params.insert("grant_type", "refresh_token");
    params.insert("refresh_token", refresh_token);
    params.insert("client_id", &provider.client_id);
    let scope = provider.scopes.join(" ");
    if !scope.is_empty() {
        params.insert("scope", &scope);
    }

    post_token_request(&provider.token_url, params).await
}

async fn post_token_request(
    token_url: &str,
    params: HashMap<&str, &str>,
) -> Result<OAuthTokenSet, OAuthError> {
    let client = reqwest::Client::new();
    let response = client
        .post(token_url)
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .form(&params)
        .send()
        .await?;

    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(OAuthError::TokenEndpoint { status, body });
    }

    let token_response: TokenResponse = serde_json::from_str(&body)?;
    Ok(token_set_from_response(token_response))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_provider() -> OAuthProvider {
        OAuthProvider {
            id: "linear".to_string(),
            client_id: "test-client-id".to_string(),
            auth_url: "https://auth.example.com/authorize".to_string(),
            token_url: "https://token.example.com/oauth/token".to_string(),
            scopes: vec!["read".to_string(), "write".to_string()],
            default_callback_port: 8765,
        }
    }

    #[test]
    fn pkce_verifier_has_expected_length() {
        let (verifier, _) = generate_pkce();
        // 32 bytes base64-url encoded without padding => ceil(32 / 3) * 4 - padding = 43.
        assert_eq!(verifier.len(), 43);
        assert!(verifier.bytes().all(|b| is_base64_url_char(b)));
    }

    #[test]
    fn pkce_challenge_is_sha256_of_verifier() {
        let (verifier, challenge) = generate_pkce();
        let expected = BASE64_URL_SAFE_NO_PAD.encode(Sha256::digest(&verifier));
        assert_eq!(challenge, expected);
        assert_eq!(challenge.len(), 43);
    }

    #[test]
    fn pkce_random_verifiers_are_different() {
        let (v1, _) = generate_pkce();
        let (v2, _) = generate_pkce();
        assert_ne!(v1, v2);
    }

    #[test]
    fn authorization_url_contains_required_params() {
        let provider = test_provider();
        let state = "test-state-123";
        let (_, challenge) = generate_pkce();

        let url = build_authorization_url(&provider, state, &challenge).unwrap();
        let query: HashMap<String, String> = url.query_pairs().into_owned().collect();

        assert_eq!(url.host_str(), Some("auth.example.com"));
        assert_eq!(url.path(), "/authorize");
        assert_eq!(query.get("response_type"), Some(&"code".to_string()));
        assert_eq!(query.get("client_id"), Some(&provider.client_id));
        assert_eq!(
            query.get("redirect_uri"),
            Some(&"http://127.0.0.1:8765/callback".to_string())
        );
        assert_eq!(query.get("scope"), Some(&"read write".to_string()));
        assert_eq!(query.get("state"), Some(&state.to_string()));
        assert_eq!(query.get("code_challenge"), Some(&challenge));
        assert_eq!(
            query.get("code_challenge_method"),
            Some(&"S256".to_string())
        );
    }

    #[test]
    fn authorization_url_uses_zero_port_when_unconfigured() {
        let mut provider = test_provider();
        provider.default_callback_port = 0;

        let url = build_authorization_url(&provider, "state", "challenge").unwrap();
        let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(
            query.get("redirect_uri"),
            Some(&"http://127.0.0.1:0/callback".to_string())
        );
    }

    #[test]
    fn token_set_parses_expires_in_and_scopes() {
        let json = r#"{
            "access_token": "at",
            "token_type": "Bearer",
            "refresh_token": "rt",
            "expires_in": 3600,
            "scope": "read write"
        }"#;

        let response: TokenResponse = serde_json::from_str(json).unwrap();
        let token_set = token_set_from_response(response);

        assert_eq!(token_set.access_token, "at");
        assert_eq!(token_set.token_type, "Bearer");
        assert_eq!(token_set.refresh_token, Some("rt".to_string()));
        assert!(token_set.expires_at.is_some());
        assert_eq!(token_set.scopes, vec!["read", "write"]);
    }

    #[test]
    fn token_set_defaults_when_optional_fields_missing() {
        let json = r#"{"access_token":"at"}"#;

        let response: TokenResponse = serde_json::from_str(json).unwrap();
        let token_set = token_set_from_response(response);

        assert_eq!(token_set.access_token, "at");
        assert_eq!(token_set.token_type, "Bearer");
        assert!(token_set.refresh_token.is_none());
        assert!(token_set.expires_at.is_none());
        assert!(token_set.scopes.is_empty());
    }

    fn is_base64_url_char(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
    }
}
