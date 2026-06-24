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

    async fn subscribe_resource(&self, uri: &str) -> Result<(), McpError>;

    async fn unsubscribe_resource(&self, uri: &str) -> Result<(), McpError>;

    async fn next_resource_update(&self) -> Result<McpResourceUpdate, McpError>;
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
`initialize` handshake once per adapter session, calls `tools/list`, invokes
remote tools with `tools/call`, lists/reads resources, and sends
`resources/subscribe` / `resources/unsubscribe` over the same stdio JSON-RPC
process until that process or request stream fails. A background stdout reader
routes JSON-RPC responses by request id and queues real
`notifications/resources/updated` messages as `McpResourceUpdate` values. It
does not provide local fallback behavior.

`McpHttpToolAdapter` is the production remote JSON-RPC adapter for
`transport = "http"` and `transport = "sse"` server entries. It sends one
JSON-RPC POST per MCP request, applies configured headers, performs the
`initialize` handshake before tool/resource requests, accepts JSON responses
and SSE `data:` JSON-RPC responses, and surfaces HTTP/protocol errors without
local fallback behavior. `resources/subscribe` and `resources/unsubscribe` use
the same JSON-RPC transport. A JSON subscribe response is acknowledged as the
server's result; when the subscribe response is a live SSE stream, the adapter
keeps reading it in the background and queues real
`notifications/resources/updated` messages as `McpResourceUpdate` values. When
the subscribe response is a JSON acknowledgement, the adapter opens the same
HTTP endpoint as an SSE event channel and reads resource update notifications
from that stream.

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
- MCP resources are not silently injected into model context.
- Resource update notifications are host/runtime state; they are not exposed as
  model tools or silently appended to the transcript.
- Server stderr and protocol logs are developer diagnostics, not model context.
- Secrets enter through environment variables, not session logs.

## Current Status

`neo-agent-core` has the MCP tool adapter abstraction, stdio JSON-RPC process
adapter, HTTP/SSE JSON-RPC adapter, discovery-to-`ToolSpec` bridge, namespaced
`ToolRegistry` registration, persistent initialized stdio session reuse, and
async call delegation.
`neo-agent print` and `neo-agent run` load enabled `transport = "stdio"`,
`transport = "http"`, and `transport = "sse"` servers from the single Neo config and
advertise their tools to the configured model.

## Connection Manager

`neo-agent-core::tools::mcp_manager::McpConnectionManager` owns the lifecycle of
configured MCP servers. It keeps a snapshot of each server (`Connected`,
`Failed`, `Pending`, `Reconnecting`, or `Disabled`), reconnects failed stdio and
remote servers with exponential backoff, and exposes live snapshots and
resource operations to the CLI and TUI.

The manager is created once per TUI session and kept in sync with the on-disk
config whenever it is reloaded (config edits, enable/disable, add, delete, and
`/reload`). `register_connected_tools_into` lets the runtime register connected
MCP tools into a `ToolRegistry` so the model sees them as
`mcp__<server_id>__<tool_name>`.

## TUI Overlay

In interactive TUI mode, `/mcp` opens the MCP manager overlay. It shows each
configured server with its transport, endpoint, enablement state, and live tool
discovery status. Keys:

- `↑` / `↓` — navigate.
- `Enter` — test/refresh the selected server.
- `a` — add a new server. First choose the transport (`stdio`, `HTTP`, or
  `SSE`), then fill the single-page form.
- `e` — toggle enablement.
- `d` — delete (confirm with `y`).
- `Esc` — close.

The add form collects all fields for the selected transport on one screen:

| Transport | Fields |
|-----------|--------|
| Local stdio | Name · Command · Env (optional) |
| Remote HTTP | Name · URL · Bearer Token (optional) · Headers (optional) |
| Remote SSE  | Name · URL · Bearer Token (optional) · Headers (optional) |

Use `Tab` or `↑` / `↓` to switch fields, `Enter` to submit, and `Esc` to cancel.
`Env` and `Headers` accept multiple `KEY=value` entries separated by commas or
newlines. A bearer token, if provided, is stored as an `Authorization: Bearer`
header.

The overlay reflects the connection manager's live snapshots when the manager is
available; otherwise it falls back to static config summaries.

## CLI

The `neo mcp` CLI surface:

- `neo mcp list` — list configured servers and their advertised tools.
- `neo mcp add <name> -t studio|remote-http|remote-sse ...` — add a server,
  test the connection, and persist the entry to config.
- `neo mcp del <name>` — remove a server from config.
- `neo mcp enable <name>` / `neo mcp disable <name>` — toggle enablement.
- `neo mcp status` — connect to each configured server and print connection
  state, tool count, and the most recent error.
- `neo mcp resources [--server-id <id>]` — list resources exposed by connected
  servers.
- `neo mcp read-resource <server-id> <uri>` — read a single resource.

Studio servers take a shell command string (`-C`), optional working directory
(`--cwd`), and environment variables. Remote servers take a URL (`--url`) and
optional headers. Both kinds support an enabled-tool allowlist
(`--enabled-tools`), a disabled-tool blocklist (`--disabled-tools`), connection
startup timeout (`--startup-timeout-ms`), and per-tool call timeout
(`--tool-timeout-ms`).

Current limitation: Neo supports configured local stdio and explicit HTTP/SSE
MCP endpoints. Hosted MCP registries, OAuth onboarding, hosted server lifecycle
management, and provider-specific discovery beyond configured endpoints remain
out of scope for the local-only surface.
