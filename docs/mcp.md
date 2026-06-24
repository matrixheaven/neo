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
- `O` — authenticate the selected HTTP/SSE server with OAuth (if a provider
  is configured).
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
- `neo mcp auth <server-id>` — start the OAuth authorization-code flow for an
  HTTP/SSE server.

Studio servers take a shell command string (`-C`), optional working directory
(`--cwd`), and environment variables. Remote servers take a URL (`--url`) and
optional headers. Both kinds support an enabled-tool allowlist
(`--enabled-tools`), a disabled-tool blocklist (`--disabled-tools`), connection
startup timeout (`--startup-timeout-ms`), and per-tool call timeout
(`--tool-timeout-ms`).

## OAuth Authentication

Remote HTTP and SSE MCP servers may use OAuth 2.0 bearer tokens. Neo includes
a local, provider-agnostic OAuth authenticator for obtaining tokens without
leaving the command line.

### Flow overview

For each configured HTTP/SSE server, Neo looks up a matching OAuth provider
(either built-in or defined in config) by the server URL. When authentication
starts:

1. Neo generates a PKCE code verifier/challenge and a random state value.
2. It starts a temporary callback server on a free local port
   (`127.0.0.1:<port>/callback`).
3. It opens the provider's authorization URL in the default browser.
4. The user approves the request in the browser; the provider redirects back to
   the local callback with an authorization code.
5. Neo exchanges the code for access/refresh tokens at the provider's token
   endpoint.
6. Tokens are stored under `~/.neo/oauth.json` keyed by `mcp:<server-id>`.

The flow is authorization-code with PKCE (`code_challenge_method=S256`).

### CLI usage

```bash
neo mcp auth <server-id>
```

`<server-id>` must be an existing HTTP or SSE server in `~/.neo/config.toml`.
Neo will open the browser, wait for the callback, and save the resulting token.

### TUI usage

In interactive TUI mode:

1. Type `/mcp` to open the MCP manager overlay.
2. Use `↑` / `↓` to select an HTTP or SSE server.
3. Press `O` to start OAuth authentication for that server.
4. Authenticate in the browser; Neo saves the token and shows a status message.

`O` is ignored for `stdio` servers.

### Built-in providers

Neo ships with one built-in OAuth provider:

| Provider | Server URL pattern | Authorization URL | Scopes |
|----------|-------------------|-------------------|--------|
| `linear` | URL containing `linear` | `https://linear.app/oauth/authorize` | `write` |

For Linear MCP servers (for example `https://mcp.linear.app/mcp`):

- Set the environment variable `NEO_OAUTH_LINEAR_CLIENT_ID` to the client ID of
  your Linear OAuth app.
- If the variable is unset, Neo falls back to the default client ID `neo`.

### Custom providers

Add an `[oauth.providers.<id>]` table to `~/.neo/config.toml` for any
OAuth 2.0 provider that uses authorization-code with PKCE:

```toml
[oauth.providers.myprovider]
client_id = "your-client-id"
auth_url = "https://example.com/oauth/authorize"
token_url = "https://example.com/oauth/token"
scopes = ["read", "write"]
default_callback_port = 0
```

- `client_id` is required. It can be overridden at runtime by
  `NEO_OAUTH_<ID_UPPER>_CLIENT_ID` (for example `NEO_OAUTH_MYPROVIDER_CLIENT_ID`).
- `auth_url` is the provider's authorization endpoint.
- `token_url` is the provider's token endpoint.
- `scopes` is the list of scopes requested during authorization.
- `default_callback_port` is the port used in the `redirect_uri`. A value of `0`
  tells Neo to bind to a free local port and substitute the real port before
  opening the browser.

A custom provider with the same `id` as a built-in provider overrides the
built-in definition.

### Security model

- **Local-only**: there is no hosted OAuth proxy, cloud identity service, or
  third-party token exchange. The entire flow runs on the user's machine.
- **Plain JSON storage**: tokens are stored as plain JSON in `~/.neo/oauth.json`.
  Encryption, keychain integration, and OS credential stores are out of scope
  for the MVP.
- **User-only permissions**: on Unix the file is created with mode `0o600`
  (read/write for the owner only). Other platforms use default permissions.
- **Runtime state only**: OAuth tokens are used for MCP HTTP/SSE requests. They
  are not injected into the model context, session transcript, or tool schemas.
- **No override of explicit headers**: if an MCP server config already contains
  an `Authorization` header, the OAuth header is not applied.

### Automatic token refresh

Before each MCP HTTP/SSE request, Neo checks the stored token for the server. If
`expires_at` is in the past and a `refresh_token` is available, Neo calls the
provider's token endpoint to refresh the access token, updates the stored entry,
and persists the file. If the refresh fails, the request fails with a protocol
error. If no refresh token is available, the expired access token is still sent
(as a last-resort fallback).

### Troubleshooting

| Symptom | Cause / fix |
|---------|-------------|
| "No OAuth provider configured for this server" | Neo could not match the server URL to a built-in or custom provider. Add `[oauth.providers.<id>]` with matching URL semantics, or verify the URL contains the provider id. |
| Browser does not open | Neo attempts to open the URL automatically but does not block on it. Copy the authorization URL from the log/status and paste it into a browser manually. |
| Callback times out | Neo waits 5 minutes for the browser redirect. Make sure the browser can reach `127.0.0.1` on the callback port and that no firewall or proxy blocks the local loopback request. |
| "MCP server not found" (TUI) | The selected server id is no longer in config. Close the overlay and reopen it to refresh the list. |
| "OAuth only supported for HTTP/SSE servers" | OAuth cannot be used with `stdio` MCP servers. Use a bearer token or env-based auth for those. |
| Token endpoint returns an error | Check that `client_id`, `auth_url`, and `token_url` are correct and that the OAuth app allows PKCE and the requested redirect URI. |
| Expired token causes requests to fail | Verify the provider returned a `refresh_token`; not all providers issue one. If not, run `neo mcp auth <server-id>` again. |

Current limitation: Neo supports configured local stdio and explicit HTTP/SSE
MCP endpoints. Hosted MCP registries, hosted server lifecycle management, and
provider-specific discovery beyond configured endpoints remain out of scope for
the local-only surface.
