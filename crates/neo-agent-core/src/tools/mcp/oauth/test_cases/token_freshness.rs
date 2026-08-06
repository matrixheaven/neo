use super::*;
use crate::tools::mcp::oauth::McpOAuthTokenRecord;

fn token_record(access_token: &str) -> McpOAuthTokenRecord {
    McpOAuthTokenRecord {
        access_token: access_token.to_owned(),
        token_type: Some("Bearer".to_owned()),
        refresh_token: Some("refresh-token".to_owned()),
        expires_in: Some(3600),
        token_received_at: unix_now_secs(),
        granted_scopes: vec!["read".to_owned(), "write".to_owned()],
        raw: serde_json::json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "refresh_token": "refresh-token",
            "expires_in": 3600,
            "scope": "read write"
        }),
    }
}

#[test]
fn token_is_fresh_accepts_missing_expiry_and_future_expiry() {
    let mut tokens = token_record("fresh-token");
    tokens.expires_in = None;
    assert!(token_is_fresh(&tokens));

    tokens.expires_in = Some(3600);
    tokens.token_received_at = unix_now_secs();
    assert!(token_is_fresh(&tokens));
}

#[test]
fn token_is_fresh_rejects_tokens_expiring_within_sixty_seconds() {
    let mut tokens = token_record("stale-token");
    tokens.expires_in = Some(59);
    tokens.token_received_at = unix_now_secs();

    assert!(!token_is_fresh(&tokens));
}
