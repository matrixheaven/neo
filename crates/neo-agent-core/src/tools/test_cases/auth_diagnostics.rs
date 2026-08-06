use super::*;

fn http_server(id: &str) -> ManagedMcpServerConfig {
    ManagedMcpServerConfig {
        id: id.to_owned(),
        enabled: true,
        transport: ManagedMcpTransport::Http {
            url: "https://mcp.example.com/mcp#ignored".to_owned(),
            headers: BTreeMap::new(),
        },
        enabled_tools: Vec::new(),
        disabled_tools: Vec::new(),
        startup_timeout_ms: None,
        tool_timeout_ms: None,
        reconnect: McpReconnectPolicy::default(),
    }
}

#[test]
fn http_oauth_identity_uses_server_url_and_transport_kind() {
    let config = http_server("remote-auth");

    let identity = oauth_identity_for_config(&config).unwrap().unwrap();

    assert_eq!(identity.server_id, "remote-auth");
    assert_eq!(
        identity.canonical_resource_url,
        "https://mcp.example.com/mcp"
    );
    assert_eq!(identity.transport_kind, McpOAuthTransportKind::Http);
}

#[test]
fn diagnostic_hint_for_http_auth_mentions_login_command() {
    let config = http_server("remote-auth");
    // Kind drives the hint; presentation text (even with "401") is ignored.
    let err = McpError::needs_auth("presentation only: 401 Unauthorized");

    let hint = diagnostic_hint(&err, &config).unwrap();

    assert!(hint.contains("/mcp-config login <server_id>"));
    assert!(hint.contains("neo mcp auth <server_id>"));
}

#[test]
fn diagnostic_hint_ignores_auth_phrases_in_protocol_message() {
    let config = http_server("remote-auth");
    let err = McpError::protocol("upstream proxy returned 401 Unauthorized");

    assert!(diagnostic_hint(&err, &config).is_none());
}
