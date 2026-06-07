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

    async fn list_resources(&self) -> Result<Vec<McpResourceDefinition>, McpError>;

    async fn read_resource(&self, uri: &str) -> Result<McpResourceRead, McpError>;
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

`McpStdioToolAdapter` is the production stdio JSON-RPC adapter. It starts the
configured command with arguments and environment, performs the MCP
`initialize` handshake once per adapter session, calls `tools/list`, and
invokes remote tools with `tools/call` over the same stdio JSON-RPC process
until that process or request stream fails. It does not provide local fallback
behavior.

`McpHttpToolAdapter` is the production remote JSON-RPC adapter for
`transport = "http"` and `transport = "sse"` server entries. It sends one
JSON-RPC POST per MCP request, applies configured headers, performs the
`initialize` handshake before tool/resource requests, accepts JSON responses
and SSE `data:` JSON-RPC responses, and surfaces HTTP/protocol errors without
local fallback behavior.

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
- MCP resources are fetched only through explicit `neo mcp resources` commands;
  they are not silently injected into model context.
- Server stderr and protocol logs are developer diagnostics, not model context.
- Secrets enter through environment variables, not session logs.

## Current Status

`neo-agent-core` has the MCP tool adapter abstraction, stdio JSON-RPC process
adapter, HTTP/SSE JSON-RPC adapter, discovery-to-`ToolSpec` bridge, namespaced
`ToolRegistry` registration, persistent initialized stdio session reuse, and
async call delegation. It also supports explicit MCP `resources/list` and
`resources/read` through the same stdio or HTTP/SSE JSON-RPC adapters.
`neo-agent print` and `neo-agent run` load enabled `transport = "stdio"`,
`transport = "http"`, and `transport = "sse"` servers from project config and
advertise their tools to the configured model. `neo mcp resources <server>
list/read` fetches resource catalogs and content without adding them to model
context automatically.

Current limitation: MCP subscriptions/notifications, OAuth/hosted server
lifecycle, and hosted MCP management remain future work.
