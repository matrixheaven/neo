use std::{
    collections::BTreeMap,
    fs,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use neo_agent_core::{
    McpError, McpStdioConfig, McpStdioToolAdapter, McpToolAdapter, McpToolCall, McpToolDefinition,
    McpToolProvider, McpToolResponse, PermissionPolicy, ToolContext, ToolRegistry,
};
use serde_json::json;

const MCP_STDIO_FIXTURE: &str = r#"
import json
import sys

for line in sys.stdin:
    request = json.loads(line)
    method = request["method"]
    if method == "initialize":
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "fixture", "version": "0.1.0"},
                "capabilities": {"tools": {}},
            },
        }
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "tools": [
                    {
                        "name": "docs-search",
                        "description": "Search project docs",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"query": {"type": "string"}},
                            "required": ["query"],
                        },
                    }
                ]
            },
        }
    elif method == "tools/call":
        assert request["params"]["name"] == "docs-search"
        query = request["params"]["arguments"]["query"]
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "content": [{"type": "text", "text": f"found {query}"}],
                "isError": False,
            },
        }
    else:
        response = {
            "jsonrpc": "2.0",
            "id": request.get("id"),
            "error": {"code": -32601, "message": f"unknown method {method}"},
        }
    print(json.dumps(response), flush=True)
"#;

const MCP_STDIO_REUSE_FIXTURE: &str = r#"
import json
import os
import sys

startup_log = os.environ["MCP_STARTUP_LOG"]
with open(startup_log, "a", encoding="utf-8") as log:
    log.write("started\n")

for line in sys.stdin:
    request = json.loads(line)
    method = request["method"]
    if method == "initialize":
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "reuse-fixture", "version": "0.1.0"},
                "capabilities": {"tools": {}},
            },
        }
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "tools": [
                    {
                        "name": "echo",
                        "description": "Echo a message",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"message": {"type": "string"}},
                            "required": ["message"],
                        },
                    }
                ]
            },
        }
    elif method == "tools/call":
        message = request["params"]["arguments"]["message"]
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "content": [{"type": "text", "text": message}],
                "isError": False,
            },
        }
    else:
        response = {
            "jsonrpc": "2.0",
            "id": request.get("id"),
            "error": {"code": -32601, "message": f"unknown method {method}"},
        }
    print(json.dumps(response), flush=True)
"#;

const MCP_STDIO_RECONNECT_FIXTURE: &str = r#"
import json
import os
import sys

startup_log = os.environ["MCP_STARTUP_LOG"]
with open(startup_log, "a", encoding="utf-8") as log:
    log.write("started\n")

for line in sys.stdin:
    request = json.loads(line)
    method = request["method"]
    if method == "initialize":
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "reconnect-fixture", "version": "0.1.0"},
                "capabilities": {"tools": {}},
            },
        }
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "tools": [
                    {
                        "name": "unstable",
                        "description": "Returns once per process",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"message": {"type": "string"}},
                            "required": ["message"],
                        },
                    }
                ]
            },
        }
    elif method == "tools/call":
        if request["params"]["arguments"]["message"] == "drop":
            sys.exit(0)
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "content": [{"type": "text", "text": request["params"]["arguments"]["message"]}],
                "isError": False,
            },
        }
    else:
        response = {
            "jsonrpc": "2.0",
            "id": request.get("id"),
            "error": {"code": -32601, "message": f"unknown method {method}"},
        }
    print(json.dumps(response), flush=True)
"#;

#[tokio::test]
async fn mcp_stdio_adapter_discovers_and_calls_json_rpc_tools() {
    let workspace = tempfile::tempdir().expect("workspace");
    let fixture_path = workspace.path().join("mcp-fixture.py");
    fs::write(&fixture_path, MCP_STDIO_FIXTURE).expect("write MCP fixture");
    let adapter = Arc::new(McpStdioToolAdapter::new(McpStdioConfig {
        command: "python3".to_owned(),
        args: vec!["-u".to_owned(), fixture_path.display().to_string()],
        env: BTreeMap::new(),
    }));

    let provider = McpToolProvider::discover("docs-server", Arc::clone(&adapter))
        .await
        .expect("discover provider over stdio");
    let specs = provider.specs();
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].name, "mcp__docs_server__docs_search");
    assert_eq!(specs[0].description, "Search project docs");
    assert_eq!(
        specs[0].input_schema,
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            },
            "required": ["query"]
        })
    );

    let mut registry = ToolRegistry::new();
    provider.register_into(&mut registry);
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all());

    let result = registry
        .run(
            "mcp__docs_server__docs_search",
            &context,
            json!({ "query": "runtime tools" }),
        )
        .await
        .expect("run mcp tool over stdio");

    assert_eq!(result.content, "found runtime tools");
    assert!(!result.is_error);
    assert_eq!(
        result.details,
        Some(json!({
            "content": [
                {
                    "type": "text",
                    "text": "found runtime tools"
                }
            ],
            "isError": false
        }))
    );
}

#[tokio::test]
async fn mcp_stdio_adapter_reuses_initialized_session_across_operations() {
    let workspace = tempfile::tempdir().expect("workspace");
    let fixture_path = workspace.path().join("mcp-reuse-fixture.py");
    let startup_log = workspace.path().join("mcp-startups.log");
    fs::write(&fixture_path, MCP_STDIO_REUSE_FIXTURE).expect("write MCP reuse fixture");
    let adapter = Arc::new(McpStdioToolAdapter::new(McpStdioConfig {
        command: "python3".to_owned(),
        args: vec!["-u".to_owned(), fixture_path.display().to_string()],
        env: BTreeMap::from([(
            "MCP_STARTUP_LOG".to_owned(),
            startup_log.display().to_string(),
        )]),
    }));

    let provider = McpToolProvider::discover("echo-server", Arc::clone(&adapter))
        .await
        .expect("discover provider over stdio");
    let mut registry = ToolRegistry::new();
    provider.register_into(&mut registry);
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all());

    let first = registry
        .run(
            "mcp__echo_server__echo",
            &context,
            json!({ "message": "first call" }),
        )
        .await
        .expect("first MCP call");
    let second = registry
        .run(
            "mcp__echo_server__echo",
            &context,
            json!({ "message": "second call" }),
        )
        .await
        .expect("second MCP call");

    assert_eq!(first.content, "first call");
    assert_eq!(second.content, "second call");
    let startups = fs::read_to_string(startup_log).expect("read startup log");
    assert_eq!(
        startups.lines().count(),
        1,
        "discovery and calls should reuse one initialized stdio MCP process"
    );
}

#[tokio::test]
async fn mcp_stdio_adapter_reconnects_after_cached_session_closes() {
    let workspace = tempfile::tempdir().expect("workspace");
    let fixture_path = workspace.path().join("mcp-reconnect-fixture.py");
    let startup_log = workspace.path().join("mcp-startups.log");
    fs::write(&fixture_path, MCP_STDIO_RECONNECT_FIXTURE).expect("write MCP reconnect fixture");
    let adapter = Arc::new(McpStdioToolAdapter::new(McpStdioConfig {
        command: "python3".to_owned(),
        args: vec!["-u".to_owned(), fixture_path.display().to_string()],
        env: BTreeMap::from([(
            "MCP_STARTUP_LOG".to_owned(),
            startup_log.display().to_string(),
        )]),
    }));

    let provider = McpToolProvider::discover("unstable-server", Arc::clone(&adapter))
        .await
        .expect("discover provider over stdio");
    let mut registry = ToolRegistry::new();
    provider.register_into(&mut registry);
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all());

    let error = registry
        .run(
            "mcp__unstable_server__unstable",
            &context,
            json!({ "message": "drop" }),
        )
        .await
        .expect_err("closed stdio session should fail the in-flight request");
    assert_eq!(
        error.to_string(),
        "mcp error from unstable-server/unstable: MCP server closed stdout"
    );

    let recovered = registry
        .run(
            "mcp__unstable_server__unstable",
            &context,
            json!({ "message": "after reconnect" }),
        )
        .await
        .expect("next request should reconnect");

    assert_eq!(recovered.content, "after reconnect");
    let startups = fs::read_to_string(startup_log).expect("read startup log");
    assert_eq!(startups.lines().count(), 2);
}

#[tokio::test]
async fn mcp_provider_discovers_namespaced_tool_specs() {
    let adapter = Arc::new(MockMcpAdapter::new(vec![McpToolDefinition::new(
        "search",
        "Search project docs",
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            },
            "required": ["query"]
        }),
    )]));

    let provider = McpToolProvider::discover("docs", adapter)
        .await
        .expect("discover provider");

    let specs = provider.specs();
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].name, "mcp__docs__search");
    assert!(
        specs[0]
            .name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_'),
        "MCP tool names must be safe for production model function-name APIs"
    );
    assert_eq!(specs[0].description, "Search project docs");
    assert_eq!(
        specs[0].input_schema,
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            },
            "required": ["query"]
        })
    );
}

#[tokio::test]
async fn mcp_tool_execution_delegates_to_async_adapter() {
    let workspace = tempfile::tempdir().expect("workspace");
    let adapter = Arc::new(MockMcpAdapter::new(vec![McpToolDefinition::new(
        "search",
        "Search project docs",
        json!({ "type": "object" }),
    )]));
    adapter.push_response(McpToolResponse::ok("found: architecture.md"));

    let provider = McpToolProvider::discover("docs", Arc::clone(&adapter))
        .await
        .expect("discover provider");
    let mut registry = ToolRegistry::new();
    provider.register_into(&mut registry);
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all());

    let result = registry
        .run(
            "mcp__docs__search",
            &context,
            json!({ "query": "runtime tools" }),
        )
        .await
        .expect("run mcp tool");

    assert_eq!(result.content, "found: architecture.md");
    assert!(!result.is_error);
    assert_eq!(
        adapter.calls(),
        vec![McpToolCall {
            name: "search".to_owned(),
            arguments: json!({ "query": "runtime tools" }),
        }]
    );
}

#[tokio::test]
async fn mcp_tool_execution_surfaces_adapter_errors() {
    let workspace = tempfile::tempdir().expect("workspace");
    let adapter = Arc::new(MockMcpAdapter::new(vec![McpToolDefinition::new(
        "broken",
        "Broken remote tool",
        json!({ "type": "object" }),
    )]));
    adapter.push_error(McpError::protocol(
        "server returned invalid JSON-RPC response",
    ));

    let provider = McpToolProvider::discover("remote", Arc::clone(&adapter))
        .await
        .expect("discover provider");
    let mut registry = ToolRegistry::new();
    provider.register_into(&mut registry);
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all());

    let error = registry
        .run("mcp__remote__broken", &context, json!({}))
        .await
        .expect_err("adapter error should not be faked into success");

    assert_eq!(
        error.to_string(),
        "mcp error from remote/broken: server returned invalid JSON-RPC response"
    );
}

#[derive(Debug)]
struct MockMcpAdapter {
    tools: Vec<McpToolDefinition>,
    calls: Mutex<Vec<McpToolCall>>,
    responses: Mutex<Vec<Result<McpToolResponse, McpError>>>,
}

impl MockMcpAdapter {
    fn new(tools: Vec<McpToolDefinition>) -> Self {
        Self {
            tools,
            calls: Mutex::new(Vec::new()),
            responses: Mutex::new(Vec::new()),
        }
    }

    fn push_response(&self, response: McpToolResponse) {
        self.responses
            .lock()
            .expect("responses lock")
            .push(Ok(response));
    }

    fn push_error(&self, error: McpError) {
        self.responses
            .lock()
            .expect("responses lock")
            .push(Err(error));
    }

    fn calls(&self) -> Vec<McpToolCall> {
        self.calls.lock().expect("calls lock").clone()
    }
}

#[async_trait]
impl McpToolAdapter for MockMcpAdapter {
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpError> {
        Ok(self.tools.clone())
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResponse, McpError> {
        self.calls.lock().expect("calls lock").push(McpToolCall {
            name: name.to_owned(),
            arguments,
        });
        self.responses
            .lock()
            .expect("responses lock")
            .pop()
            .unwrap_or_else(|| Err(McpError::protocol("missing mock response")))
    }
}
