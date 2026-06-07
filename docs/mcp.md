# Model Context Protocol

MCP support is intended to expose external tools and resources to Neo without coupling the agent loop to a specific server implementation.

## Agent-Core Interface

`neo-agent-core` exposes the production adapter boundary in `tools::mcp`:

```rust
#[async_trait::async_trait]
pub trait McpToolAdapter: Send + Sync {
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpError>;

    async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResponse, McpError>;
}
```

`McpToolProvider::discover(server_id, adapter)` calls `list_tools`, converts
the returned definitions to normal `ToolSpec` values, and can register those
tools into `ToolRegistry`. Registered MCP tools execute by delegating to
`adapter.call_tool`; production code must provide a real adapter implementation.

Tool names are exposed to the model as `mcp__<server_id>__<tool_name>` and call
the remote MCP tool by its original unprefixed name. Non-alphanumeric
characters in server and tool ids are converted to `_` so model provider
function-name validators can accept the advertised tools.

Transport configuration is intentionally outside this core slice. A stdio or
other JSON-RPC client should implement `McpToolAdapter` without adding local
fallback behavior.

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
- Tool names are namespaced by server id and use provider-safe characters.
- MCP tool calls pass through the same permission policy as built-in tools.
- Server stderr and protocol logs are developer diagnostics, not model context.
- Secrets enter through environment variables, not session logs.

## Current Status

`neo-agent-core` has the MCP tool adapter abstraction, discovery-to-`ToolSpec`
bridge, namespaced `ToolRegistry` registration, and async call delegation. It
does not yet spawn external MCP server processes or load MCP CLI config.
