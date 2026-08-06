use super::*;
use crate::tools::mcp::oauth::McpOAuthClientRecord;
use crate::tools::mcp::oauth::McpOAuthError;
use crate::tools::mcp::oauth::McpOAuthIdentity;
use crate::tools::mcp::oauth::McpOAuthTransportKind;

fn identity() -> McpOAuthIdentity {
    McpOAuthIdentity::new(
        "linear",
        "https://mcp.example.com/sse?workspace=neo",
        McpOAuthTransportKind::Sse,
    )
    .unwrap()
}

fn client_record() -> McpOAuthClientRecord {
    McpOAuthClientRecord {
        client_id: "client-id".to_owned(),
        client_secret: Some("client-secret".to_owned()),
        redirect_uris: vec!["http://127.0.0.1:14500/callback".to_owned()],
        token_endpoint_auth_method: Some("client_secret_post".to_owned()),
        raw: serde_json::json!({"client_id": "client-id"}),
    }
}

#[test]
fn stored_client_redirect_uri_uses_first_redirect_uri() {
    let mut client = client_record();
    client.redirect_uris = vec![
        "http://127.0.0.1:14500/callback".to_owned(),
        "http://127.0.0.1:14501/callback".to_owned(),
    ];

    let redirect_uri = redirect_uri_from_stored_client(&client).unwrap();

    assert_eq!(redirect_uri, "http://127.0.0.1:14500/callback");
}

#[test]
fn stored_client_without_redirect_uri_is_flow_error() {
    let mut client = client_record();
    client.redirect_uris.clear();

    let err = redirect_uri_from_stored_client(&client).unwrap_err();

    assert!(
        matches!(err, McpOAuthError::Flow(message) if message == "stored OAuth client is missing a redirect URI")
    );
}

#[test]
fn phase_2b_dynamic_redirect_uri_is_not_available_without_callback_server() {
    let identity = identity();

    let err = phase_2b_redirect_uri(&identity).unwrap_err();

    assert!(
        matches!(err, McpOAuthError::Flow(message) if message == "OAuth callback server is not wired yet")
    );
}

#[test]
fn client_record_from_config_preserves_redirect_uri_that_is_not_resource_url() {
    let identity = identity();
    let redirect_uri = "http://127.0.0.1:14500/callback";
    let config = OAuthClientConfig::new("client-id", redirect_uri);

    let record = client_record_from_config(&config);

    assert_eq!(record.redirect_uris, vec![redirect_uri.to_owned()]);
    assert_ne!(record.redirect_uris[0], identity.canonical_resource_url);
}
