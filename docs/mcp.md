# Model Context Protocol

MCP support is intended to expose external tools and resources to Neo without coupling the agent loop to a specific server implementation.

## Agent-Core Interface

`neo-agent-core` exposes the client boundary in `tools::mcp`:

```rust
#[async_trait::async_trait]
pub trait McpClient: Send + Sync {
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpError>;
    async fn call_tool(&self, name: &str, arguments: serde_json::Value) -> Result<McpToolResponse, McpError>;
    async fn list_resources(&self) -> Result<Vec<McpResourceDefinition>, McpError>;
    async fn read_resource(&self, uri: &str) -> Result<McpResourceRead, McpError>;
    async fn shutdown(&self) -> Result<(), McpError>;
}
```

The production implementation `RmcpClient` wraps an `rmcp::service::RunningService<RoleClient, ()>`,
delegating `list_tools`, `call_tool`, `list_resources`, and `read_resource` to the rmcp peer with
configurable request timeouts.

Transport-specific builders create the rmcp service:

- **`build_stdio_client`** (`mcp/stdio.rs`) — spawns the configured command via
  `rmcp::transport::TokioChildProcess`, registers the child with `ProcessSupervisor`, and performs
  the MCP `initialize` handshake over stdin/stdout JSON-RPC.
- **`build_http_client`** (`mcp/http.rs`) — creates an `OAuthStreamableHttpClient` (local OAuth integration) backed by
  `rmcp::transport::StreamableHttpClientTransport` for both `"http"` and `"sse"` transport types.
  The client applies configured custom headers and an optional `AuthorizationManager` for OAuth.

`McpConnectionManager` calls `register_connected_tools_into` to convert discovered MCP tools into
normal `ToolSpec` values and register them in `ToolRegistry`. Registered MCP tools execute by
delegating to the rmcp peer's `call_tool` method.

Tool names are exposed to the model as `mcp__<server_id>__<tool_name>` and call the remote MCP tool
by its original unprefixed name. Non-alphanumeric characters in server and tool ids are converted to
`_` so model provider function-name validators can accept the advertised tools.

## Runtime Placement

MCP belongs at the `neo-agent-core` boundary:

```text
MCP server
  <-> rmcp transport (TokioChildProcess / StreamableHttpClient)
  <-> RmcpClient (McpClient impl)
  <-> McpConnectionManager
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
- Server stderr and protocol logs are developer diagnostics, not model context.
- Secrets enter through environment variables, not session logs.

## Current Status

`neo-agent-core` uses the official `rmcp` Rust SDK for all MCP transports (stdio, HTTP, SSE).
The `RmcpClient` wrapper provides a uniform interface over rmcp's `RunningService`, with
configurable request timeouts. `McpConnectionManager` manages connection lifecycle, snapshots,
reconnect/backoff, and tool discovery.

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

Remote HTTP and SSE MCP servers may use OAuth 2.0 bearer tokens. Neo uses the
`rmcp` SDK's `AuthorizationManager` with file-backed credential and in-memory
state stores for OAuth flows.

### Flow overview

OAuth uses dynamic discovery and dynamic client registration (DCR) per
SEP-985 / RFC 8414 / RFC 7591:

1. Neo generates a PKCE code verifier/challenge and a random state value.
2. It starts a temporary callback server on a free local port
   (`127.0.0.1:<port>/callback`).
3. rmcp discovers the server's OAuth metadata via `/.well-known/oauth-authorization-server`.
4. It opens the provider's authorization URL in the default browser.
5. The user approves the request in the browser; the provider redirects back to
   the local callback with an authorization code.
6. rmcp exchanges the code for access/refresh tokens and stores them.
7. Tokens are persisted in `~/.neo/oauth.json` under keys `mcp:<server-id>`.

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

### Custom provider overrides

Add an `[oauth.providers.<id>]` table to `~/.neo/config.toml` for manual OAuth
provider overrides (bypasses discovery):

```toml
[oauth.providers.myprovider]
client_id = "your-client-id"
auth_url = "https://example.com/oauth/authorize"
token_url = "https://example.com/oauth/token"
scopes = ["read", "write"]
default_callback_port = 0
```

- `client_id` is required.
- `auth_url` is the provider's authorization endpoint.
- `token_url` is the provider's token endpoint.
- `scopes` is the list of scopes requested during authorization.
- `default_callback_port` is the port used in the `redirect_uri`. A value of `0`
  tells Neo to bind to a free local port and substitute the real port before
  opening the browser.

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

Before each MCP HTTP/SSE request, the `AuthorizationManager` checks the stored
token. If the token is expired or near-expiry and a refresh token is available,
rmcp calls the provider's token endpoint to refresh the access token
automatically. If the refresh fails, the request may fail with an authorization
error.

### Troubleshooting

| Symptom | Cause / fix |
|---------|-------------|
| "OAuth authorization required" | No valid token for this server. Run `neo mcp auth <server-id>` to start the OAuth flow. |
| Browser does not open | Neo attempts to open the URL automatically but does not block on it. Copy the authorization URL from the log/status and paste it into a browser manually. |
| Callback times out | Neo waits for the browser redirect. Make sure the browser can reach `127.0.0.1` on the callback port and that no firewall or proxy blocks the local loopback request. |
| "MCP server not found" (TUI) | The selected server id is no longer in config. Close the overlay and reopen it to refresh the list. |
| "OAuth only supported for HTTP/SSE servers" | OAuth cannot be used with `stdio` MCP servers. Use a bearer token or env-based auth for those. |
| Token endpoint returns an error | Check that the server supports dynamic client registration (DCR), or add a manual local `[oauth.providers.<id>]` override. |
| Expired token causes requests to fail | Verify the provider returned a `refresh_token`; not all providers issue one. If not, run `neo mcp auth <server-id>` again. |

Current limitation: Neo supports configured local stdio and explicit HTTP/SSE
MCP endpoints. Hosted MCP registries, hosted server lifecycle management, and
provider-specific discovery beyond configured endpoints remain out of scope for
the local-only surface.
