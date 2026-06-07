# Model Context Protocol

MCP support is intended to expose external tools and resources to Neo without coupling the agent loop to a specific server implementation.

## Conceptual Interface

The stable boundary should look like:

```rust
pub struct McpServerConfig {
    pub id: String,
    pub transport: McpTransport,
    pub enabled: bool,
}

pub enum McpTransport {
    Stdio { command: String, args: Vec<String>, env: Vec<(String, String)> },
}

pub trait McpClient {
    fn list_tools(&self) -> impl Future<Output = Result<Vec<McpTool>, McpError>> + Send;
    fn call_tool(&self, name: &str, arguments: serde_json::Value)
        -> impl Future<Output = Result<McpToolResult, McpError>> + Send;
}
```

The exact Rust trait can change once `neo-agent-core` has stable runtime and async trait conventions. The important contract is that MCP discovery becomes tool metadata, and MCP invocation becomes an authorized tool execution.

## Runtime Placement

MCP belongs at the `neo-agent-core` boundary:

```text
MCP server
  <-> MCP client adapter
  <-> ToolRegistry and ToolExecutor
  <-> Agent loop
  <-> ModelClient
```

The model should only see normal `ToolSpec` values. It should not know whether a tool came from built-in Rust code or an MCP server.

## Safety Rules

- Disabled MCP servers are not started.
- Tool names are namespaced by server id when needed.
- MCP tool calls pass through the same permission policy as built-in tools.
- Server stderr and protocol logs are developer diagnostics, not model context.
- Secrets enter through environment variables, not session logs.

## Current Status

This slice documents the intended interface and adds no runtime dependency on MCP crates. A compile-time Rust stub should only be added after `neo-agent-core` has stable adjacent modules to export alongside it.
