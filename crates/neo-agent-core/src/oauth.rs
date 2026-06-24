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
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use thiserror::Error;

pub mod callback_server;
pub mod store;

pub use store::OAuthStore;

/// A set of OAuth tokens returned by the token endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

impl OAuthProvider {
    /// Return the configured `client_id`, allowing an environment variable
    /// override of the form `NEO_OAUTH_<PROVIDER_ID_UPPER>_CLIENT_ID`.
    #[must_use]
    pub fn client_id_or_env(&self) -> String {
        self.client_id_with_env_lookup(|key| std::env::var(key).ok())
    }

    fn client_id_with_env_lookup<F>(&self, lookup: F) -> String
    where
        F: FnOnce(&str) -> Option<String>,
    {
        let key = format!("NEO_OAUTH_{}_CLIENT_ID", self.id.to_uppercase());
        lookup(&key).unwrap_or_else(|| self.client_id.clone())
    }
}

/// Built-in OAuth provider definitions shipped with Neo.
///
/// The list starts with Linear; additional providers can be added here or via
/// user config (`[oauth.providers.<id>]`).
#[must_use]
pub fn builtin_oauth_providers() -> Vec<OAuthProvider> {
    vec![OAuthProvider {
        id: "linear".to_owned(),
        client_id: "neo".to_owned(),
        auth_url: "https://linear.app/oauth/authorize".to_owned(),
        token_url: "https://api.linear.app/oauth/token".to_owned(),
        scopes: vec!["write".to_owned()],
        default_callback_port: 0,
    }]
}

/// Registry of OAuth providers keyed by provider id.
///
/// Built-in providers are seeded with [`OAuthProviderRegistry::with_builtin_providers`];
/// custom providers from config are registered via [`OAuthProviderRegistry::register`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OAuthProviderRegistry {
    providers: BTreeMap<String, OAuthProvider>,
}

impl OAuthProviderRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            providers: BTreeMap::new(),
        }
    }

    /// Create a registry seeded with [`builtin_oauth_providers`].
    #[must_use]
    pub fn with_builtin_providers() -> Self {
        let mut registry = Self::new();
        for provider in builtin_oauth_providers() {
            registry.register(provider);
        }
        registry
    }

    /// Register (or replace) a provider in the registry.
    pub fn register(&mut self, provider: OAuthProvider) {
        self.providers.insert(provider.id.clone(), provider);
    }

    /// Look up a provider by its id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&OAuthProvider> {
        self.providers.get(id)
    }

    /// Find a provider whose id or URL pattern matches the given server URL.
    ///
    /// For the MVP the matching is intentionally simple: a provider matches if
    /// its id is contained in the URL. Custom providers registered later take
    /// precedence because they can replace built-ins with the same id.
    #[must_use]
    pub fn resolve_for_url(&self, url: &str) -> Option<&OAuthProvider> {
        self.providers
            .values()
            .find(|provider| url.contains(&provider.id))
    }
}

/// Errors that may occur during OAuth operations.
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
    #[error("no OAuth provider configured for this server")]
    ProviderDetection,
    #[error("failed to load token store: {0}")]
    StoreLoad(std::io::Error),
    #[error("failed to save token store: {0}")]
    StoreSave(std::io::Error),
    #[error("failed to parse token store: {0}")]
    StoreParse(serde_json::Error),
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
        let client_id = provider.client_id_or_env();
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &client_id)
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
    let client_id = provider.client_id_or_env();
    let mut params = HashMap::new();
    params.insert("grant_type", "authorization_code");
    params.insert("code", code);
    params.insert("redirect_uri", &redirect_uri);
    params.insert("client_id", &client_id);
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
    let client_id = provider.client_id_or_env();
    let scope = provider.scopes.join(" ");
    let mut params = HashMap::new();
    params.insert("grant_type", "refresh_token");
    params.insert("refresh_token", refresh_token);
    params.insert("client_id", &client_id);
    if !scope.is_empty() {
        params.insert("scope", &scope);
    }

    post_token_request(&provider.token_url, params).await
}

/// Detect a known OAuth provider for an HTTP/SSE MCP server URL.
///
/// Convenience wrapper that uses a registry seeded with the built-in providers.
/// For custom providers, build an [`OAuthProviderRegistry`] and call
/// [`OAuthProviderRegistry::resolve_for_url`] directly.
#[must_use]
pub fn provider_for_url(url: &str) -> Option<OAuthProvider> {
    OAuthProviderRegistry::with_builtin_providers()
        .resolve_for_url(url)
        .cloned()
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
        assert!(verifier.bytes().all(is_base64_url_char));
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

    #[test]
    fn registry_seeds_builtin_linear_provider() {
        let registry = OAuthProviderRegistry::with_builtin_providers();
        let provider = registry.get("linear").expect("linear should be registered");
        assert_eq!(provider.id, "linear");
        assert_eq!(provider.auth_url, "https://linear.app/oauth/authorize");
        assert_eq!(provider.token_url, "https://api.linear.app/oauth/token");
        assert_eq!(provider.scopes, vec!["write"]);
    }

    #[test]
    fn registry_resolve_for_url_finds_linear() {
        let registry = OAuthProviderRegistry::with_builtin_providers();
        let provider = registry
            .resolve_for_url("https://mcp.linear.app/mcp")
            .expect("linear URL should resolve");
        assert_eq!(provider.id, "linear");
    }

    #[test]
    fn registry_resolve_for_url_returns_none_for_unknown() {
        let registry = OAuthProviderRegistry::with_builtin_providers();
        assert!(
            registry
                .resolve_for_url("https://api.unknown-provider.example")
                .is_none()
        );
    }

    #[test]
    fn registry_register_overrides_builtin() {
        let mut registry = OAuthProviderRegistry::with_builtin_providers();
        let custom = OAuthProvider {
            id: "linear".to_owned(),
            client_id: "custom-client".to_owned(),
            auth_url: "https://custom.example/authorize".to_owned(),
            token_url: "https://custom.example/token".to_owned(),
            scopes: vec!["read".to_owned()],
            default_callback_port: 0,
        };
        registry.register(custom.clone());
        let provider = registry.get("linear").expect("linear should exist");
        assert_eq!(provider.client_id, "custom-client");
        assert_eq!(provider.auth_url, "https://custom.example/authorize");
    }

    #[test]
    fn client_id_or_env_prefers_environment_variable() {
        let provider = OAuthProvider {
            id: "linear".to_owned(),
            client_id: "fallback".to_owned(),
            auth_url: "https://example.com/authorize".to_owned(),
            token_url: "https://example.com/token".to_owned(),
            scopes: Vec::new(),
            default_callback_port: 0,
        };
        temp_env::with_var("NEO_OAUTH_LINEAR_CLIENT_ID", Some("env-client-id"), || {
            assert_eq!(provider.client_id_or_env(), "env-client-id");
        });
    }

    #[test]
    fn client_id_or_env_falls_back_to_configured_client_id() {
        let provider = OAuthProvider {
            id: "foo".to_owned(),
            client_id: "configured".to_owned(),
            auth_url: "https://example.com/authorize".to_owned(),
            token_url: "https://example.com/token".to_owned(),
            scopes: Vec::new(),
            default_callback_port: 0,
        };
        temp_env::with_var_unset("NEO_OAUTH_FOO_CLIENT_ID", || {
            assert_eq!(provider.client_id_or_env(), "configured");
        });
    }

    async fn spawn_mock_token_server(
        response_body: String,
    ) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut headers = Vec::new();
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).await.unwrap() == 0 {
                    break;
                }
                if line.trim().is_empty() {
                    break;
                }
                headers.push(line.clone());
            }
            let content_length = headers
                .iter()
                .find_map(|h| {
                    h.to_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|s| s.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            let mut body = vec![0u8; content_length];
            if content_length > 0 {
                reader.read_exact(&mut body).await.unwrap();
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            writer.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8(body).unwrap()
        });
        (format!("http://127.0.0.1:{port}/token"), handle)
    }

    #[tokio::test]
    async fn exchange_code_for_token_hits_mock_token_endpoint() {
        let response_body = r#"{"access_token":"mock-access","token_type":"Bearer","refresh_token":"mock-refresh","expires_in":3600,"scope":"read write"}"#;
        let (token_url, server) = spawn_mock_token_server(response_body.to_owned()).await;

        let provider = OAuthProvider {
            id: "linear".to_owned(),
            client_id: "test-client-id".to_owned(),
            auth_url: "https://auth.example.com/authorize".to_owned(),
            token_url,
            scopes: vec!["write".to_owned()],
            default_callback_port: 0,
        };

        let token = exchange_code_for_token(&provider, "auth-code-123", "verifier-xyz")
            .await
            .unwrap();

        assert_eq!(token.access_token, "mock-access");
        assert_eq!(token.token_type, "Bearer");
        assert_eq!(token.refresh_token, Some("mock-refresh".to_owned()));
        assert!(token.expires_at.is_some());
        assert_eq!(token.scopes, vec!["read", "write"]);

        let request_body = server.await.unwrap();
        assert!(request_body.contains("grant_type=authorization_code"));
        assert!(request_body.contains("code=auth-code-123"));
        assert!(request_body.contains("code_verifier=verifier-xyz"));
        assert!(request_body.contains("client_id=test-client-id"));
        assert!(request_body.contains("redirect_uri="));
    }
}
