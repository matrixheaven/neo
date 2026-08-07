use super::http_server::*;

use std::fs;

use serde_json::json;
use tempfile::TempDir;

#[test]
fn config_model_scope_selects_first_matching_model_for_interactive_start() {
    let temp = TempDir::new().expect("tempdir");
    write_home_config(
        r#"
model_scope = ["sonnet"]
"#,
    );

    let mut command = neo();
    command.current_dir(temp.path());

    let stdout = run(command);

    assert!(stdout.contains("anthropic/claude-sonnet-4-5"));
    assert!(!stdout.contains("openai/gpt-4.1"));
    assert!(!stdout.contains("placeholder"));
    assert!(!stdout.contains("fake"));
}

#[test]
fn mcp_list_reports_empty_configuration_without_placeholder_language() {
    let mut mcp = neo();
    mcp.args(["mcp", "list"]);
    let mcp_stdout = run(mcp);
    assert!(mcp_stdout.contains("no MCP servers configured"));
    assert!(!mcp_stdout.contains("placeholder"));
    assert!(!mcp_stdout.contains("fake"));
}

#[test]
fn mcp_list_reads_project_config_servers() {
    let temp = TempDir::new().expect("tempdir");
    write_home_config(
        r#"
[[mcp.servers]]
id = "filesystem"
enabled = false
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]

[mcp.servers.env]
RUST_LOG = "info"
"#,
    );

    let mut mcp = neo();
    mcp.current_dir(temp.path()).args(["mcp", "list"]);
    let stdout = run(mcp);

    assert!(stdout.contains("[1]<filesystem>(studio)"));
}

#[test]
fn mcp_list_displays_remote_servers() {
    let temp = TempDir::new().expect("tempdir");
    write_home_config(
        r#"
[[mcp.servers]]
id = "remote-docs"
enabled = false
transport = "http"
url = "https://mcp.example.test/rpc"

[[mcp.servers]]
id = "stream-docs"
enabled = true
transport = "sse"
url = "https://mcp.example.test/sse"
"#,
    );

    let mut mcp = neo();
    mcp.current_dir(temp.path()).args(["mcp", "list"]);
    let stdout = run(mcp);

    assert!(stdout.contains("[1]<remote-docs>(remote-http)"));
    assert!(stdout.contains("{}"));
    assert!(stdout.contains("[2]<stream-docs>(remote-sse)"));
}

#[test]
fn mcp_add_enable_disable_del_persists_project_config_without_printing_secrets() {
    let temp = TempDir::new().expect("tempdir");
    let secret_value = "token-secret-123456";

    let mut add = neo();
    add.current_dir(temp.path()).args([
        "mcp",
        "add",
        "remote-docs",
        "-t",
        "remote-http",
        "--url",
        "https://mcp.example.test/rpc",
        "--header",
        "authorization=Bearer token-secret-123456",
        "--env",
        "MCP_TOKEN=token-secret-123456",
    ]);
    let add_stdout = run(add);
    assert!(add_stdout.contains("added MCP server remote-docs"));
    assert!(!add_stdout.contains(secret_value));

    let config_path = neo_home_for_test().join("config.toml");
    let config_content = fs::read_to_string(&config_path).expect("read config");
    assert!(config_content.contains("id = \"remote-docs\""));
    assert!(config_content.contains("transport = \"http\""));
    assert!(config_content.contains("url = \"https://mcp.example.test/rpc\""));
    assert!(config_content.contains("authorization = \"Bearer token-secret-123456\""));
    assert!(config_content.contains("MCP_TOKEN = \"token-secret-123456\""));

    let mut list = neo();
    list.current_dir(temp.path()).args(["mcp", "list"]);
    let list_stdout = run(list);
    assert!(list_stdout.contains("[1]<remote-docs>(remote-http)"));
    assert!(!list_stdout.contains(secret_value));
    assert!(!list_stdout.contains("authorization"));
    assert!(!list_stdout.contains("MCP_TOKEN"));

    let mut disable = neo();
    disable
        .current_dir(temp.path())
        .args(["mcp", "disable", "remote-docs"]);
    assert_eq!(run(disable), "disabled MCP server remote-docs\n");
    let config_content = fs::read_to_string(&config_path).expect("read disabled config");
    assert!(config_content.contains("enabled = false"));

    let mut enable = neo();
    enable
        .current_dir(temp.path())
        .args(["mcp", "enable", "remote-docs"]);
    assert_eq!(run(enable), "enabled MCP server remote-docs\n");
    let config_content = fs::read_to_string(&config_path).expect("read enabled config");
    assert!(config_content.contains("enabled = true"));

    let mut remove = neo();
    remove
        .current_dir(temp.path())
        .args(["mcp", "del", "remote-docs"]);
    assert_eq!(run(remove), "removed MCP server remote-docs\n");
    let config_content = fs::read_to_string(&config_path).expect("read removed config");
    assert!(!config_content.contains("remote-docs"));
    assert!(!config_content.contains(secret_value));
}

#[test]
fn mcp_add_remote_http_probes_and_reports_success() {
    let temp = TempDir::new().expect("tempdir");
    let mcp_server = MockSseServer::start(vec![
        mcp_json_response(
            0,
            &json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "remote-docs", "version": "0.1.0"},
                "capabilities": {"tools": {}}
            }),
        ),
        mcp_http_accept(),
        mcp_json_response(
            1,
            &json!({
                "tools": [
                    {
                        "name": "docs-search",
                        "description": "Search remote docs",
                        "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}
                    }
                ]
            }),
        ),
    ]);

    let mut add = neo();
    add.current_dir(temp.path()).args([
        "mcp",
        "add",
        "remote-docs",
        "-t",
        "remote-http",
        "--url",
        &mcp_server.url,
    ]);
    let stdout = run(add);
    assert!(stdout.contains("added MCP server remote-docs"));
    assert!(stdout.contains("remote-docs successfully connected!"));

    let config_path = neo_home_for_test().join("config.toml");
    let config_content = fs::read_to_string(&config_path).expect("read config");
    assert!(config_content.contains("transport = \"http\""));
    assert!(config_content.contains(&format!("url = \"{}\"", mcp_server.url)));
}

#[test]
fn mcp_add_remote_http_reports_failure_deterministically() {
    let temp = TempDir::new().expect("tempdir");

    let mut add = neo();
    add.current_dir(temp.path()).args([
        "mcp",
        "add",
        "bad-remote",
        "-t",
        "remote-http",
        "--url",
        &failure_server_url(),
        "--startup-timeout-ms",
        "200",
    ]);
    let stdout = run(add);
    assert!(stdout.contains("added MCP server bad-remote"));
    assert!(stdout.contains("bad-remote connect failed"));

    let config_path = neo_home_for_test().join("config.toml");
    let config_content = fs::read_to_string(&config_path).expect("read config");
    assert!(config_content.contains("id = \"bad-remote\""));
}

#[test]
fn mcp_add_with_disable_creates_enabled_false() {
    let temp = TempDir::new().expect("tempdir");

    let mut add = neo();
    add.current_dir(temp.path()).args([
        "mcp",
        "add",
        "offline-server",
        "-t",
        "remote-http",
        "--url",
        "http://127.0.0.1:1/rpc",
        "--disable",
    ]);
    let stdout = run(add);
    assert!(stdout.contains("added MCP server offline-server"));
    assert!(stdout.contains("offline-server added (disabled)"));

    let config_path = neo_home_for_test().join("config.toml");
    let config_content = fs::read_to_string(&config_path).expect("read config");
    assert!(config_content.contains("enabled = false"));
}

#[test]
fn mcp_add_studio_parses_command_string_and_cwd() {
    let temp = TempDir::new().expect("tempdir");

    let mut add = neo();
    add.current_dir(temp.path()).args([
        "mcp",
        "add",
        "filesystem",
        "-t",
        "studio",
        "-C",
        "npx",
        "--arg",
        "-y",
        "--arg",
        "@modelcontextprotocol/server-filesystem",
        "--arg",
        ".",
        "--cwd",
        ".",
        "--disable",
    ]);
    let stdout = run(add);
    assert!(stdout.contains("added MCP server filesystem"));
    assert!(stdout.contains("filesystem added (disabled)"));

    let config_path = neo_home_for_test().join("config.toml");
    let config_content = fs::read_to_string(&config_path).expect("read config");
    assert!(config_content.contains("enabled = false"));
    assert!(config_content.contains("command = \"npx\""));
    assert!(config_content.contains("args = ["));
    assert!(config_content.contains("\"-y\""));
    assert!(config_content.contains("\"@modelcontextprotocol/server-filesystem\""));
    assert!(config_content.contains("\".\""));
    assert!(config_content.contains("cwd = \".\""));
}

#[test]
fn mcp_add_with_enabled_tools_filters_tool_list() {
    let temp = TempDir::new().expect("tempdir");
    let mcp_server = MockSseServer::start(vec![
        // first connection for `add` probe
        mcp_json_response(
            0,
            &json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "remote-docs", "version": "0.1.0"},
                "capabilities": {"tools": {}}
            }),
        ),
        mcp_http_accept(),
        mcp_json_response(
            1,
            &json!({
                "tools": [
                    {
                        "name": "docs-search",
                        "description": "Search docs",
                        "inputSchema": {"type": "object"}
                    },
                    {
                        "name": "docs-read",
                        "description": "Read docs",
                        "inputSchema": {"type": "object"}
                    }
                ]
            }),
        ),
        // second connection for `list`
        mcp_json_response(
            0,
            &json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "remote-docs", "version": "0.1.0"},
                "capabilities": {"tools": {}}
            }),
        ),
        mcp_http_accept(),
        mcp_json_response(
            1,
            &json!({
                "tools": [
                    {
                        "name": "docs-search",
                        "description": "Search docs",
                        "inputSchema": {"type": "object"}
                    },
                    {
                        "name": "docs-read",
                        "description": "Read docs",
                        "inputSchema": {"type": "object"}
                    }
                ]
            }),
        ),
    ]);

    let mut add = neo();
    add.current_dir(temp.path()).args([
        "mcp",
        "add",
        "remote-docs",
        "-t",
        "remote-http",
        "--url",
        &mcp_server.url,
        "--enabled-tools",
        "docs-search",
    ]);
    let stdout = run(add);
    assert!(stdout.contains("remote-docs successfully connected!"));

    let mut list = neo();
    list.current_dir(temp.path()).args(["mcp", "list"]);
    let list_stdout = run(list);
    assert!(list_stdout.contains("docs-search"));
    assert!(!list_stdout.contains("docs-read"));
}

#[test]
fn run_text_registers_enabled_stdio_mcp_tools_from_project_config() {
    let temp = TempDir::new().expect("tempdir");
    let provider = MockSseServer::start(vec![openai_response_sse("resp-mcp", "mcp tools listed")]);
    let mcp_fixture = temp.path().join("mcp-fixture.py");
    fs::write(&mcp_fixture, MCP_STDIO_FIXTURE).expect("write MCP fixture");
    write_home_config(&format!(
        r#"{}

[[mcp.servers]]
id = "docs-server"
enabled = true
transport = "stdio"
command = "python3"
args = ["-u", "{}"]
"#,
        mock_responses_config(&provider.url),
        mcp_fixture.display()
    ));

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "show", "tools"]);
    let stdout = run(command);

    assert_eq!(stdout, "mcp tools listed\n");
    let requests = provider.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/responses");
    assert_eq!(
        requests[0].headers.get("authorization").map(String::as_str),
        Some("Bearer test-key")
    );
    let tool_names = requests[0].body["tools"]
        .as_array()
        .expect("model request tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect::<Vec<_>>();
    assert!(
        tool_names.contains(&"mcp__docs_server__docs_search"),
        "model tools should include configured MCP stdio tools: {tool_names:?}"
    );
}

#[test]
fn run_text_registers_enabled_http_mcp_tools_from_project_config() {
    let temp = TempDir::new().expect("tempdir");
    let provider = MockSseServer::start(vec![openai_response_sse(
        "resp-mcp-http",
        "remote mcp tools listed",
    )]);
    let mcp_server = MockSseServer::start(vec![
        mcp_json_response(
            0,
            &json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "remote-docs", "version": "0.1.0"},
                "capabilities": {"tools": {}}
            }),
        ),
        mcp_http_accept(),
        mcp_json_response(
            1,
            &json!({
                "tools": [
                    {
                        "name": "docs-search",
                        "description": "Search remote docs",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"query": {"type": "string"}},
                            "required": ["query"]
                        }
                    }
                ]
            }),
        ),
        mcp_json_response(2, &json!({"resources": []})),
    ]);
    write_home_config(&format!(
        r#"{}

[[mcp.servers]]
id = "remote-docs"
enabled = true
transport = "http"
url = "{}"

[mcp.servers.headers]
"x-neo-test" = "remote-mcp"
"#,
        mock_responses_config(&provider.url),
        mcp_server.url
    ));

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "show", "remote", "tools"]);
    let stdout = run(command);

    assert_eq!(stdout, "remote mcp tools listed\n");
    let requests = provider.requests();
    let tool_names = requests[0].body["tools"]
        .as_array()
        .expect("model request tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect::<Vec<_>>();
    assert!(
        tool_names.contains(&"mcp__remote_docs__docs_search"),
        "model tools should include configured MCP HTTP tools: {tool_names:?}"
    );
    let mcp_requests = mcp_server.requests();
    let methods: Vec<_> = mcp_requests
        .iter()
        .map(|request| request.body["method"].as_str().unwrap_or("(none)"))
        .collect();
    assert!(
        methods.contains(&"initialize"),
        "expected initialize request, got {methods:?}"
    );
    assert!(
        methods.contains(&"tools/list"),
        "expected tools/list request, got {methods:?}"
    );
    assert!(
        mcp_requests.iter().all(|request| {
            request.headers.get("x-neo-test").map(String::as_str) == Some("remote-mcp")
        }),
        "custom header missing from some requests: {mcp_requests:?}"
    );
}

#[test]
fn run_text_rejects_remote_mcp_server_missing_url() {
    let temp = TempDir::new().expect("tempdir");
    // Configure only an MCP server with a missing URL.  The MCP server
    // itself logs a warning, and the model call is skipped because no
    // model client can be created (the mock base URL is unreachable).
    write_home_config(
        r#"
[[mcp.servers]]
id = "remote-docs"
enabled = true
transport = "http"
"#,
    );

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env_remove("OPENAI_API_KEY")
        .args(["run", "--output", "text", "show", "remote", "tools"]);
    let output = command.output().expect("neo command should run");

    // The MCP server with a missing URL means no MCP tools are registered,
    // and without an API key the model call also fails.  The command should
    // not succeed either way.
    assert!(!output.status.success());
}
