use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use neo_agent_core::{
    HostedMcpClient, McpError, McpHostedConfig, McpHostedToolAdapter, McpHttpConfig,
    McpHttpToolAdapter, McpResourceDefinition, McpResourceRead, McpResourceUpdate, McpStdioConfig,
    McpStdioToolAdapter, McpToolAdapter, McpToolCall, McpToolDefinition, McpToolProvider,
    McpToolResponse, PermissionPolicy, ToolContext, ToolRegistry,
};
use serde_json::{Value, json};

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

const MCP_STDIO_RESOURCE_UPDATE_FIXTURE: &str = r#"
import json
import os
import sys

method_log = os.environ["MCP_METHOD_LOG"]

def log_method(method):
    with open(method_log, "a", encoding="utf-8") as log:
        log.write(method + "\n")

for line in sys.stdin:
    request = json.loads(line)
    method = request["method"]
    log_method(method)
    if method == "initialize":
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "resource-fixture", "version": "0.1.0"},
                "capabilities": {"resources": {"subscribe": True}},
            },
        }
    elif method == "notifications/initialized":
        continue
    elif method == "resources/subscribe":
        assert request["params"]["uri"] == "file://docs/readme.md"
        response = {"jsonrpc": "2.0", "id": request["id"], "result": {}}
        print(json.dumps(response), flush=True)
        notification = {
            "jsonrpc": "2.0",
            "method": "notifications/resources/updated",
            "params": {"uri": "file://docs/readme.md"},
        }
        print(json.dumps(notification), flush=True)
        continue
    elif method == "resources/unsubscribe":
        assert request["params"]["uri"] == "file://docs/readme.md"
        response = {"jsonrpc": "2.0", "id": request["id"], "result": {}}
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
async fn mcp_stdio_adapter_subscribes_and_receives_resource_updates() {
    let workspace = tempfile::tempdir().expect("workspace");
    let fixture_path = workspace.path().join("mcp-resource-update-fixture.py");
    let method_log = workspace.path().join("mcp-methods.log");
    fs::write(&fixture_path, MCP_STDIO_RESOURCE_UPDATE_FIXTURE)
        .expect("write MCP resource fixture");
    let adapter = McpStdioToolAdapter::new(McpStdioConfig {
        command: "python3".to_owned(),
        args: vec!["-u".to_owned(), fixture_path.display().to_string()],
        env: BTreeMap::from([(
            "MCP_METHOD_LOG".to_owned(),
            method_log.display().to_string(),
        )]),
    });

    adapter
        .subscribe_resource("file://docs/readme.md")
        .await
        .expect("subscribe to MCP resource");
    let update = adapter
        .next_resource_update()
        .await
        .expect("receive resource update notification");
    adapter
        .unsubscribe_resource("file://docs/readme.md")
        .await
        .expect("unsubscribe from MCP resource");

    assert_eq!(update.uri, "file://docs/readme.md");
    let methods = fs::read_to_string(method_log).expect("read method log");
    assert_eq!(
        methods.lines().collect::<Vec<_>>(),
        vec![
            "initialize",
            "notifications/initialized",
            "resources/subscribe",
            "resources/unsubscribe"
        ]
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
async fn mcp_http_adapter_subscribes_to_sse_resource_updates() {
    let server = MockMcpHttpServer::start(vec![
        mcp_json_response(json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {"name": "http-resource-fixture", "version": "0.1.0"},
            "capabilities": {"resources": {"subscribe": true}}
        })),
        mcp_sse_resource_update_response(json!({}), "file://docs/readme.md"),
        mcp_json_response(json!({})),
    ]);
    let adapter = McpHttpToolAdapter::new(McpHttpConfig {
        url: server.url.clone(),
        headers: BTreeMap::new(),
    });

    adapter
        .subscribe_resource("file://docs/readme.md")
        .await
        .expect("subscribe over HTTP SSE");
    let update = adapter
        .next_resource_update()
        .await
        .expect("receive HTTP SSE resource update");
    adapter
        .unsubscribe_resource("file://docs/readme.md")
        .await
        .expect("unsubscribe over HTTP");

    assert_eq!(update.uri, "file://docs/readme.md");
    assert_eq!(
        server
            .requests()
            .iter()
            .map(|request| request.body["method"].as_str().expect("method"))
            .collect::<Vec<_>>(),
        vec!["initialize", "resources/subscribe", "resources/unsubscribe"]
    );
}

#[tokio::test]
async fn mcp_http_adapter_reads_resource_updates_from_event_channel_after_json_subscribe_ack() {
    let server = MockMcpHttpServer::start(vec![
        mcp_json_response(json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {"name": "http-resource-fixture", "version": "0.1.0"},
            "capabilities": {"resources": {"subscribe": true}}
        })),
        mcp_json_response(json!({})),
        mcp_sse_notification_response("file://docs/readme.md"),
        mcp_json_response(json!({})),
    ]);
    let adapter = McpHttpToolAdapter::new(McpHttpConfig {
        url: server.url.clone(),
        headers: BTreeMap::new(),
    });

    adapter
        .subscribe_resource("file://docs/readme.md")
        .await
        .expect("JSON subscribe response is acknowledged");
    let update = adapter
        .next_resource_update()
        .await
        .expect("receive resource update from alternate event channel");
    adapter
        .unsubscribe_resource("file://docs/readme.md")
        .await
        .expect("unsubscribe over HTTP");

    assert_eq!(update.uri, "file://docs/readme.md");
    let requests = server.requests();
    assert_eq!(
        requests
            .iter()
            .filter_map(|request| request.body["method"].as_str())
            .collect::<Vec<_>>(),
        vec!["initialize", "resources/subscribe", "resources/unsubscribe"]
    );
    assert_eq!(
        requests
            .iter()
            .map(|request| request.method.as_str())
            .collect::<Vec<_>>(),
        vec!["POST", "POST", "GET", "POST"]
    );
    assert_eq!(
        requests
            .iter()
            .map(|request| request.path.as_str())
            .collect::<Vec<_>>(),
        vec!["/", "/", "/", "/"]
    );
}

#[tokio::test]
async fn mcp_http_adapter_uses_event_stream_url_from_json_subscribe_ack() {
    let server = MockMcpHttpServer::start(vec![
        mcp_json_response(json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {"name": "http-resource-fixture", "version": "0.1.0"},
            "capabilities": {"resources": {"subscribe": true}}
        })),
        mcp_json_response(json!({
            "eventStreamUrl": "/events"
        })),
        mcp_sse_notification_response_for_path("/events", "file://docs/readme.md"),
        mcp_json_response(json!({})),
    ]);
    let adapter = McpHttpToolAdapter::new(McpHttpConfig {
        url: server.url.clone(),
        headers: BTreeMap::new(),
    });

    adapter
        .subscribe_resource("file://docs/readme.md")
        .await
        .expect("JSON subscribe response can provide alternate event stream URL");
    let update = adapter
        .next_resource_update()
        .await
        .expect("receive resource update from alternate event stream URL");
    adapter
        .unsubscribe_resource("file://docs/readme.md")
        .await
        .expect("unsubscribe over HTTP");

    assert_eq!(update.uri, "file://docs/readme.md");
    let requests = server.requests();
    assert_eq!(
        requests
            .iter()
            .filter_map(|request| request.body["method"].as_str())
            .collect::<Vec<_>>(),
        vec!["initialize", "resources/subscribe", "resources/unsubscribe"]
    );
    assert_eq!(
        requests
            .iter()
            .map(|request| request.method.as_str())
            .collect::<Vec<_>>(),
        vec!["POST", "POST", "GET", "POST"]
    );
    assert_eq!(
        requests
            .iter()
            .map(|request| request.path.as_str())
            .collect::<Vec<_>>(),
        vec!["/", "/", "/events", "/"]
    );
}

#[tokio::test]
async fn mcp_http_adapter_requires_sse_event_channel_after_json_subscribe_ack() {
    let server = MockMcpHttpServer::start(vec![
        mcp_json_response(json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {"name": "http-resource-fixture", "version": "0.1.0"},
            "capabilities": {"resources": {"subscribe": true}}
        })),
        mcp_json_response(json!({})),
        mcp_json_response(json!({})),
    ]);
    let adapter = McpHttpToolAdapter::new(McpHttpConfig {
        url: server.url.clone(),
        headers: BTreeMap::new(),
    });

    let error = adapter
        .subscribe_resource("file://docs/readme.md")
        .await
        .expect_err("JSON subscribe ack needs an SSE event channel");

    assert_eq!(
        error.message(),
        "MCP HTTP event stream did not return text/event-stream"
    );
    let requests = server.requests();
    assert_eq!(
        requests
            .iter()
            .filter_map(|request| request.body["method"].as_str())
            .collect::<Vec<_>>(),
        vec!["initialize", "resources/subscribe"]
    );
    assert_eq!(
        requests
            .iter()
            .map(|request| request.method.as_str())
            .collect::<Vec<_>>(),
        vec!["POST", "POST", "GET"]
    );
}

#[tokio::test]
async fn mcp_http_adapter_rejects_non_http_event_stream_url_from_json_subscribe_ack() {
    let server = MockMcpHttpServer::start(vec![
        mcp_json_response(json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {"name": "http-resource-fixture", "version": "0.1.0"},
            "capabilities": {"resources": {"subscribe": true}}
        })),
        mcp_json_response(json!({
            "eventStreamUrl": "file:///tmp/events"
        })),
    ]);
    let adapter = McpHttpToolAdapter::new(McpHttpConfig {
        url: server.url.clone(),
        headers: BTreeMap::new(),
    });

    let error = adapter
        .subscribe_resource("file://docs/readme.md")
        .await
        .expect_err("subscribe ACK event stream URL must be http or https");

    assert_eq!(
        error.message(),
        "MCP HTTP subscribe result eventStreamUrl must be an http or https event stream URL, got file: file:///tmp/events"
    );
    let requests = server.requests();
    assert_eq!(
        requests
            .iter()
            .filter_map(|request| request.body["method"].as_str())
            .collect::<Vec<_>>(),
        vec!["initialize", "resources/subscribe"]
    );
    assert_eq!(
        requests
            .iter()
            .map(|request| request.method.as_str())
            .collect::<Vec<_>>(),
        vec!["POST", "POST"]
    );
}

#[tokio::test]
async fn mcp_http_adapter_reports_sse_stream_end_after_subscribe_response() {
    let server = MockMcpHttpServer::start(vec![
        mcp_json_response(json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {"name": "http-resource-fixture", "version": "0.1.0"},
            "capabilities": {"resources": {"subscribe": true}}
        })),
        mcp_sse_response(json!({})),
    ]);
    let adapter = McpHttpToolAdapter::new(McpHttpConfig {
        url: server.url.clone(),
        headers: BTreeMap::new(),
    });

    adapter
        .subscribe_resource("file://docs/readme.md")
        .await
        .expect("subscribe over HTTP SSE");
    let error = tokio::time::timeout(Duration::from_millis(250), adapter.next_resource_update())
        .await
        .expect("stream EOF should not hang")
        .expect_err("closed SSE stream should report an error");

    assert_eq!(error.message(), "MCP HTTP SSE resource update stream ended");
}

#[tokio::test]
async fn mcp_http_adapter_discovers_and_calls_json_rpc_tools() {
    let workspace = tempfile::tempdir().expect("workspace");
    let server = MockMcpHttpServer::start(vec![
        mcp_json_response(json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {"name": "http-fixture", "version": "0.1.0"},
            "capabilities": {"tools": {}}
        })),
        mcp_json_response(json!({
            "tools": [
                {
                    "name": "docs-search",
                    "description": "Search project docs over HTTP",
                    "inputSchema": {
                        "type": "object",
                        "properties": {"query": {"type": "string"}},
                        "required": ["query"]
                    }
                }
            ]
        })),
        mcp_json_response(json!({
            "content": [{"type": "text", "text": "found http runtime"}],
            "isError": false
        })),
    ]);
    let adapter = Arc::new(McpHttpToolAdapter::new(McpHttpConfig {
        url: server.url.clone(),
        headers: BTreeMap::from([("x-neo-test".to_owned(), "mcp-http".to_owned())]),
    }));

    let provider = McpToolProvider::discover("remote-docs", Arc::clone(&adapter))
        .await
        .expect("discover provider over HTTP");
    let specs = provider.specs();
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].name, "mcp__remote_docs__docs_search");
    assert_eq!(specs[0].description, "Search project docs over HTTP");

    let mut registry = ToolRegistry::new();
    provider.register_into(&mut registry);
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all());
    let result = registry
        .run(
            "mcp__remote_docs__docs_search",
            &context,
            json!({ "query": "runtime" }),
        )
        .await
        .expect("run mcp tool over HTTP");

    assert_eq!(result.content, "found http runtime");
    assert!(!result.is_error);
    let requests = server.requests();
    assert_eq!(
        requests
            .iter()
            .map(|request| request.body["method"].as_str().expect("method"))
            .collect::<Vec<_>>(),
        vec!["initialize", "tools/list", "tools/call"]
    );
    assert!(
        requests.iter().all(
            |request| request.headers.get("x-neo-test").map(String::as_str) == Some("mcp-http")
        )
    );
}

#[tokio::test]
async fn mcp_http_adapter_accepts_sse_json_rpc_responses() {
    let server = MockMcpHttpServer::start(vec![
        mcp_sse_response(json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {"name": "sse-fixture", "version": "0.1.0"},
            "capabilities": {"tools": {}}
        })),
        mcp_sse_response(json!({
            "tools": [
                {
                    "name": "echo",
                    "description": "Echo over SSE",
                    "inputSchema": {"type": "object"}
                }
            ]
        })),
    ]);
    let adapter = McpHttpToolAdapter::new(McpHttpConfig {
        url: server.url.clone(),
        headers: BTreeMap::new(),
    });

    let tools = adapter
        .list_tools()
        .await
        .expect("list tools from SSE JSON-RPC response");

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[0].description, "Echo over SSE");
}

#[tokio::test]
async fn mcp_http_adapter_lists_and_reads_resources() {
    let server = MockMcpHttpServer::start(vec![
        mcp_json_response(json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {"name": "resource-fixture", "version": "0.1.0"},
            "capabilities": {"resources": {}}
        })),
        mcp_json_response(json!({
            "resources": [
                {
                    "uri": "file://docs/readme.md",
                    "name": "README",
                    "description": "Project readme",
                    "mimeType": "text/markdown"
                }
            ]
        })),
        mcp_json_response(json!({
            "contents": [
                {
                    "uri": "file://docs/readme.md",
                    "mimeType": "text/markdown",
                    "text": "# Neo"
                }
            ]
        })),
    ]);
    let adapter = McpHttpToolAdapter::new(McpHttpConfig {
        url: server.url.clone(),
        headers: BTreeMap::new(),
    });

    let resources = adapter.list_resources().await.expect("list MCP resources");
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].uri, "file://docs/readme.md");
    assert_eq!(resources[0].name, "README");
    assert_eq!(resources[0].mime_type.as_deref(), Some("text/markdown"));

    let content = adapter
        .read_resource("file://docs/readme.md")
        .await
        .expect("read MCP resource");
    assert_eq!(content.contents.len(), 1);
    assert_eq!(content.contents[0].uri, "file://docs/readme.md");
    assert_eq!(content.contents[0].text.as_deref(), Some("# Neo"));

    assert_eq!(
        server
            .requests()
            .iter()
            .map(|request| request.body["method"].as_str().expect("method"))
            .collect::<Vec<_>>(),
        vec!["initialize", "resources/list", "resources/read"]
    );
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

#[tokio::test]
async fn mcp_hosted_adapter_delegates_discovery_and_calls_to_cloud_client() {
    let workspace = tempfile::tempdir().expect("workspace");
    let client = Arc::new(MockHostedMcpClient::new(vec![McpToolDefinition::new(
        "search",
        "Search hosted docs",
        json!({ "type": "object" }),
    )]));
    client.push_response(McpToolResponse::ok("found hosted architecture"));

    let adapter = Arc::new(McpHostedToolAdapter::new(
        McpHostedConfig {
            server_id: "hosted-docs".to_owned(),
        },
        Arc::clone(&client) as Arc<dyn HostedMcpClient>,
    ));
    let provider = McpToolProvider::discover("hosted_docs", Arc::clone(&adapter))
        .await
        .expect("discover hosted provider");
    let mut registry = ToolRegistry::new();
    provider.register_into(&mut registry);
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all());

    let result = registry
        .run(
            "mcp__hosted_docs__search",
            &context,
            json!({ "query": "runtime tools" }),
        )
        .await
        .expect("run hosted mcp tool");

    assert_eq!(result.content, "found hosted architecture");
    assert_eq!(client.listed_servers(), vec!["hosted-docs".to_owned()]);
    assert_eq!(
        client.calls(),
        vec![HostedMcpCall {
            server_id: "hosted-docs".to_owned(),
            name: "search".to_owned(),
            arguments: json!({ "query": "runtime tools" }),
        }]
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

#[derive(Debug, Clone, PartialEq)]
struct HostedMcpCall {
    server_id: String,
    name: String,
    arguments: Value,
}

#[derive(Debug)]
struct MockHostedMcpClient {
    tools: Vec<McpToolDefinition>,
    listed_servers: Mutex<Vec<String>>,
    calls: Mutex<Vec<HostedMcpCall>>,
    responses: Mutex<Vec<Result<McpToolResponse, McpError>>>,
}

impl MockHostedMcpClient {
    fn new(tools: Vec<McpToolDefinition>) -> Self {
        Self {
            tools,
            listed_servers: Mutex::new(Vec::new()),
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

    fn listed_servers(&self) -> Vec<String> {
        self.listed_servers
            .lock()
            .expect("listed servers lock")
            .clone()
    }

    fn calls(&self) -> Vec<HostedMcpCall> {
        self.calls.lock().expect("calls lock").clone()
    }
}

#[async_trait]
impl HostedMcpClient for MockHostedMcpClient {
    async fn list_tools(&self, server_id: &str) -> Result<Vec<McpToolDefinition>, McpError> {
        self.listed_servers
            .lock()
            .expect("listed servers lock")
            .push(server_id.to_owned());
        Ok(self.tools.clone())
    }

    async fn call_tool(
        &self,
        server_id: &str,
        name: &str,
        arguments: Value,
    ) -> Result<McpToolResponse, McpError> {
        self.calls.lock().expect("calls lock").push(HostedMcpCall {
            server_id: server_id.to_owned(),
            name: name.to_owned(),
            arguments,
        });
        self.responses
            .lock()
            .expect("responses lock")
            .pop()
            .unwrap_or_else(|| Err(McpError::protocol("missing hosted mock response")))
    }

    async fn list_resources(
        &self,
        _server_id: &str,
    ) -> Result<Vec<McpResourceDefinition>, McpError> {
        Err(McpError::protocol("resources not used in this test"))
    }

    async fn read_resource(
        &self,
        _server_id: &str,
        _uri: &str,
    ) -> Result<McpResourceRead, McpError> {
        Err(McpError::protocol("resources not used in this test"))
    }

    async fn next_resource_update(&self, _server_id: &str) -> Result<McpResourceUpdate, McpError> {
        Err(McpError::protocol("resources not used in this test"))
    }
}

#[derive(Debug, Clone)]
struct McpHttpRecordedRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Value,
}

struct MockMcpHttpServer {
    url: String,
    requests: Arc<Mutex<Vec<McpHttpRecordedRequest>>>,
}

impl MockMcpHttpServer {
    fn start(responses: Vec<MockMcpHttpResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock MCP HTTP server");
        let url = format!("http://{}", listener.local_addr().expect("local addr"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured_requests = Arc::clone(&requests);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            ready_tx.send(()).expect("signal mock MCP HTTP readiness");
            for response in responses {
                let (mut socket, _) = listener.accept().expect("accept MCP HTTP request");
                let request = read_http_json_request(&mut socket);
                let id = request.body.get("id").cloned().unwrap_or(Value::Null);
                captured_requests
                    .lock()
                    .expect("requests lock")
                    .push(request.clone());
                socket
                    .write_all(response.render(&id, &request.path).as_bytes())
                    .expect("write MCP HTTP response");
            }
        });
        ready_rx.recv().expect("mock MCP HTTP server ready");

        Self { url, requests }
    }

    fn requests(&self) -> Vec<McpHttpRecordedRequest> {
        self.requests.lock().expect("requests lock").clone()
    }
}

enum MockMcpHttpResponse {
    Json(Value),
    Sse(Value),
    SseResourceUpdate { result: Value, uri: String },
    SseNotification { uri: String },
    ExpectedPathSseNotification { expected_path: String, uri: String },
}

impl MockMcpHttpResponse {
    fn render(self, id: &Value, request_path: &str) -> String {
        match self {
            Self::Json(result) => {
                let rpc = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result,
                });
                http_response("application/json", &rpc.to_string())
            }
            Self::Sse(result) => {
                let rpc = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result,
                });
                let body = format!("data: {rpc}\n\n");
                http_response("text/event-stream", &body)
            }
            Self::SseResourceUpdate { result, uri } => {
                let rpc = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result,
                });
                let notification = json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/resources/updated",
                    "params": { "uri": uri },
                });
                let body = format!("data: {rpc}\n\ndata: {notification}\n\n");
                http_response("text/event-stream", &body)
            }
            Self::SseNotification { uri } => {
                let notification = json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/resources/updated",
                    "params": { "uri": uri },
                });
                let body = format!("data: {notification}\n\n");
                http_response("text/event-stream", &body)
            }
            Self::ExpectedPathSseNotification { expected_path, uri } => {
                assert_eq!(request_path, expected_path);
                let notification = json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/resources/updated",
                    "params": { "uri": uri },
                });
                let body = format!("data: {notification}\n\n");
                http_response("text/event-stream", &body)
            }
        }
    }
}

fn mcp_json_response(result: Value) -> MockMcpHttpResponse {
    MockMcpHttpResponse::Json(result)
}

fn mcp_sse_response(result: Value) -> MockMcpHttpResponse {
    MockMcpHttpResponse::Sse(result)
}

fn mcp_sse_resource_update_response(result: Value, uri: impl Into<String>) -> MockMcpHttpResponse {
    MockMcpHttpResponse::SseResourceUpdate {
        result,
        uri: uri.into(),
    }
}

fn mcp_sse_notification_response_for_path(
    expected_path: impl Into<String>,
    uri: impl Into<String>,
) -> MockMcpHttpResponse {
    MockMcpHttpResponse::ExpectedPathSseNotification {
        expected_path: expected_path.into(),
        uri: uri.into(),
    }
}

fn mcp_sse_notification_response(uri: impl Into<String>) -> MockMcpHttpResponse {
    MockMcpHttpResponse::SseNotification { uri: uri.into() }
}

fn http_response(content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn read_http_json_request(socket: &mut TcpStream) -> McpHttpRecordedRequest {
    let mut buffer = Vec::new();
    let mut temp = [0_u8; 1024];
    let header_end;
    loop {
        let read = socket.read(&mut temp).expect("read MCP HTTP request");
        assert_ne!(read, 0, "client closed before sending headers");
        buffer.extend_from_slice(&temp[..read]);
        if let Some(index) = find_header_end(&buffer) {
            header_end = index;
            break;
        }
    }

    let headers_text = String::from_utf8(buffer[..header_end].to_vec()).expect("headers utf8");
    let path = headers_text
        .lines()
        .next()
        .and_then(|request_line| request_line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_owned();
    let method = headers_text
        .lines()
        .next()
        .and_then(|request_line| request_line.split_whitespace().next())
        .unwrap_or("GET")
        .to_owned();
    let headers = headers_text
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect::<BTreeMap<_, _>>();
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let body_start = header_end + 4;
    while buffer.len() < body_start + content_length {
        let read = socket.read(&mut temp).expect("read body");
        assert_ne!(read, 0, "client closed before sending body");
        buffer.extend_from_slice(&temp[..read]);
    }
    let body = if content_length == 0 {
        Value::Null
    } else {
        serde_json::from_slice(&buffer[body_start..body_start + content_length])
            .expect("MCP request body json")
    };
    McpHttpRecordedRequest {
        method,
        path,
        headers,
        body,
    }
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}
