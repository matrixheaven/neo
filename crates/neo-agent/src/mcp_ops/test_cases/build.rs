//! build behavior (moved from `mcp_ops.rs`).

use super::super::*;
use super::*;

#[test]
fn key_value_pairs_parses_and_trims() {
    let pairs = key_value_pairs(
        vec!["KEY=value".to_owned(), "OTHER =  spaced  ".to_owned()],
        "--env",
    )
    .unwrap();
    assert_eq!(pairs.get("KEY").unwrap(), "value");
    assert_eq!(pairs.get("OTHER").unwrap(), "spaced");
}

#[test]
fn key_value_pairs_rejects_missing_equals() {
    assert!(key_value_pairs(vec!["KEYVALUE".to_owned()], "--env").is_err());
}

#[test]
fn build_mcp_server_config_stdio_requires_command() {
    let input = AddMcpServerInput {
        id: "fs".to_owned(),
        cli_type: "studio".to_owned(),
        command: None,
        args: vec![],
        url: None,
        env: vec![],
        headers: vec![],
        cwd: None,
        enabled_tools: vec![],
        disabled_tools: vec![],
        startup_timeout_ms: None,
        tool_timeout_ms: None,
        enabled: true,
    };
    assert!(build_mcp_server_config(input).is_err());
}

#[test]
fn validate_stdio_program_rejects_empty_without_trimming() {
    let mut server = McpServerConfig {
        id: "fs".to_owned(),
        enabled: true,
        transport: McpTransport::Stdio,
        command: Some(String::new()),
        url: None,
        args: vec![],
        env: BTreeMap::new(),
        headers: BTreeMap::new(),
        cwd: None,
        enabled_tools: vec![],
        disabled_tools: vec![],
        startup_timeout_ms: None,
        tool_timeout_ms: None,
    };
    assert!(validate_mcp_server_config(&server).is_err());

    server.command = Some("  npx  ".to_owned());
    assert!(validate_mcp_server_config(&server).is_ok());
    assert_eq!(server.command.as_deref(), Some("  npx  "));
}

#[test]
fn build_mcp_server_config_http_rejects_command() {
    let input = AddMcpServerInput {
        id: "linear".to_owned(),
        cli_type: "remote-http".to_owned(),
        command: Some("npx".to_owned()),
        args: vec![],
        url: Some("https://example.invalid/mcp".to_owned()),
        env: vec![],
        headers: vec![],
        cwd: None,
        enabled_tools: vec![],
        disabled_tools: vec![],
        startup_timeout_ms: None,
        tool_timeout_ms: None,
        enabled: true,
    };
    assert!(build_mcp_server_config(input).is_err());
}

#[test]
fn build_mcp_server_config_stdio_rejects_headers() {
    let input = AddMcpServerInput {
        id: "fs".to_owned(),
        cli_type: "studio".to_owned(),
        command: Some("npx".to_owned()),
        args: vec!["-y".to_owned(), "@server/filesystem".to_owned()],
        url: None,
        env: vec![],
        headers: vec!["Authorization=secret".to_owned()],
        cwd: None,
        enabled_tools: vec![],
        disabled_tools: vec![],
        startup_timeout_ms: None,
        tool_timeout_ms: None,
        enabled: true,
    };
    assert!(build_mcp_server_config(input).is_err());
}

#[test]
fn to_managed_config_preserves_filters_and_timeouts() {
    let server = McpServerConfig {
        id: "fs".to_owned(),
        enabled: true,
        transport: McpTransport::Stdio,
        command: Some("npx".to_owned()),
        url: None,
        args: vec!["-y".to_owned()],
        env: BTreeMap::new(),
        headers: BTreeMap::new(),
        cwd: None,
        enabled_tools: vec!["read".to_owned()],
        disabled_tools: vec!["write".to_owned()],
        startup_timeout_ms: Some(5_000),
        tool_timeout_ms: Some(10_000),
    };
    let managed = to_managed_config(&server).unwrap();
    assert_eq!(managed.enabled_tools, vec!["read"]);
    assert_eq!(managed.disabled_tools, vec!["write"]);
    assert_eq!(managed.startup_timeout_ms, Some(5_000));
    assert_eq!(managed.tool_timeout_ms, Some(10_000));
}
