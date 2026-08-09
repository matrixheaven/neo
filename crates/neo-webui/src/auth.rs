//! Loopback-only authentication: one-time access token, in-memory session
//! credentials, exact Host/Origin validation, and security response headers.
//!
//! Tokens, cookie credentials, claim bodies, web frames and `Set-Cookie`
//! values are sensitive: nothing here ever logs, echoes, or persists them.

use std::collections::HashSet;
use std::sync::{Mutex, RwLock};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http::HeaderMap;
use http::header;
use rand::RngCore;

/// Fixed session cookie name.
pub const SESSION_COOKIE_NAME: &str = "neo_webui_session";
/// Fixed content security policy for every page and API.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; form-action 'self'";

const TOKEN_BYTES: usize = 32;

/// In-memory authentication state. Restart or explicit clearing invalidates
/// every credential immediately; stale cookies then get a generic `401` and
/// are cleared.
#[derive(Debug)]
pub struct AuthState {
    token: Mutex<Option<[u8; TOKEN_BYTES]>>,
    credentials: RwLock<HashSet<[u8; TOKEN_BYTES]>>,
}

impl AuthState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            token: Mutex::new(Some(random_bytes())),
            credentials: RwLock::new(HashSet::new()),
        }
    }

    /// URL-safe base64 of the one-time token, for the startup address
    /// fragment. `None` once claimed.
    #[must_use]
    pub fn access_token(&self) -> Option<String> {
        self.token
            .lock()
            .expect("auth token lock poisoned")
            .as_ref()
            .map(|token| URL_SAFE_NO_PAD.encode(token))
    }

    /// Compare-and-consume the one-time token inside one lock, then issue an
    /// independent random in-memory session credential. Exactly one of two
    /// concurrent claims with the same token succeeds.
    pub fn claim(&self, token: [u8; TOKEN_BYTES]) -> Option<String> {
        let mut guard = self.token.lock().expect("auth token lock poisoned");
        match guard.as_ref() {
            Some(actual) if ct_eq(actual, &token) => {
                *guard = None;
                let credential = random_bytes();
                self.credentials
                    .write()
                    .expect("auth credentials lock poisoned")
                    .insert(credential);
                Some(URL_SAFE_NO_PAD.encode(credential))
            }
            _ => None,
        }
    }

    /// Whether a raw cookie value is a live session credential.
    #[must_use]
    pub fn verify_credential(&self, cookie_value: &str) -> bool {
        let Some(decoded) = decode_urlsafe_32(cookie_value) else {
            return false;
        };
        self.credentials
            .read()
            .expect("auth credentials lock poisoned")
            .contains(&decoded)
    }
}

impl Default for AuthState {
    fn default() -> Self {
        Self::new()
    }
}

/// Decode a `URL_SAFE_NO_PAD` value into exactly 32 bytes.
#[must_use]
pub fn decode_urlsafe_32(value: &str) -> Option<[u8; TOKEN_BYTES]> {
    let decoded = URL_SAFE_NO_PAD.decode(value).ok()?;
    decoded.try_into().ok()
}

/// Exact single-value Host check: `127.0.0.1:<actual port>` only.
#[must_use]
pub fn host_matches(headers: &HeaderMap, port: u16) -> bool {
    let expected = format!("127.0.0.1:{port}");
    let mut values = headers.get_all(header::HOST).iter();
    match (values.next(), values.next()) {
        (Some(value), None) => value.as_bytes() == expected.as_bytes(),
        _ => false,
    }
}

/// Exact single-value Origin check: `http://127.0.0.1:<actual port>` only.
#[must_use]
pub fn origin_matches(headers: &HeaderMap, port: u16) -> bool {
    let expected = format!("http://127.0.0.1:{port}");
    let mut values = headers.get_all(header::ORIGIN).iter();
    match (values.next(), values.next()) {
        (Some(value), None) => value.as_bytes() == expected.as_bytes(),
        _ => false,
    }
}

/// Extract the session cookie value; the last of multiple same-name cookies
/// wins (RFC 6265 semantics). Names must match exactly: a cookie whose name
/// merely starts with `neo_webui_session` (e.g. a spoofing
/// `neo_webui_sessionX` from another loopback origin) is never confused with
/// the real one.
#[must_use]
pub fn session_cookie_value(headers: &HeaderMap) -> Option<&str> {
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    let mut found: Option<&str> = None;
    for part in cookie.split(';') {
        let part = part.trim();
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        if name.trim() == SESSION_COOKIE_NAME {
            found = Some(value);
        }
    }
    found
}

/// `Set-Cookie` value for a live session credential:
/// `HttpOnly; SameSite=Strict; Path=/` — no `Secure`, no `Max-Age`.
#[must_use]
pub fn session_cookie_header(credential: &str) -> http::HeaderValue {
    http::HeaderValue::from_str(&format!(
        "{SESSION_COOKIE_NAME}={credential}; HttpOnly; SameSite=Strict; Path=/"
    ))
    .expect("cookie value is valid header bytes")
}

/// `Set-Cookie` value clearing a stale credential. Expiry requires
/// `Max-Age=0`; the live session cookie itself never sets one.
#[must_use]
pub fn clear_cookie_header() -> http::HeaderValue {
    http::HeaderValue::from_str(&format!(
        "{SESSION_COOKIE_NAME}=; Max-Age=0; HttpOnly; SameSite=Strict; Path=/"
    ))
    .expect("cookie value is valid header bytes")
}

/// Apply the fixed security headers to a response.
pub fn apply_security_headers(response: &mut http::Response<axum::body::Body>) {
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        http::HeaderValue::from_static("no-store"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        http::HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        http::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        http::HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
}

fn random_bytes() -> [u8; TOKEN_BYTES] {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::rng().fill_bytes(&mut bytes);
    bytes
}

/// Fixed-time byte comparison.
fn ct_eq(a: &[u8; TOKEN_BYTES], b: &[u8; TOKEN_BYTES]) -> bool {
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_time_compare_detects_every_difference() {
        let a = [0x5Au8; 32];
        assert!(ct_eq(&a, &a));
        for index in 0..32 {
            let mut b = a;
            b[index] ^= 0x01;
            assert!(!ct_eq(&a, &b), "bit {index} must differ");
        }
    }

    #[test]
    fn claim_consumes_the_token_exactly_once() {
        let auth = AuthState::new();
        let token = auth.access_token().expect("token present before claim");
        let decoded = decode_urlsafe_32(&token).expect("valid token");
        assert!(auth.claim(decoded).is_some());
        assert!(auth.claim(decoded).is_none());
        assert!(auth.access_token().is_none());
    }

    #[test]
    fn wrong_length_or_padded_tokens_never_decode() {
        assert!(decode_urlsafe_32("").is_none());
        assert!(decode_urlsafe_32("AAAA").is_none());
        assert!(decode_urlsafe_32(&"a".repeat(44)).is_none());
        assert!(decode_urlsafe_32(&format!("{}+/=", "A".repeat(42))).is_none());
        assert!(decode_urlsafe_32(&URL_SAFE_NO_PAD.encode([7u8; 32])).is_some());
    }
}
