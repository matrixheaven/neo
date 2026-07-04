# MCP Servers

MCP (Model Context Protocol) is a mechanism that lets an LLM call external tools and resources through a standard protocol. Neo ships with a built-in MCP client that can wire any MCP server's tools into the model's tool table, with unified scheduling, authentication, and rate limiting.

The configuration entry is the `[[mcp.servers]]` array under `~/.neo/config.toml` (`$NEO_HOME` takes precedence). At session startup Neo launches every server with `enabled = true`, discovers its tools, and registers them in the tool table. Reference example: [`examples/config/mcp-server.toml`](../../../examples/config/mcp-server.toml).

## MCP Concepts

| Concept | Description |
| --- | --- |
| **Server** | An independent MCP process or remote endpoint that communicates with Neo over stdio / HTTP / SSE |
| **Tool** | A callable function exposed by a server, with `name`, `description`, and a JSON Schema input |
| **Resource** | A read-only resource exposed by a server (URI + MIME type); list with `neo mcp resources` |
| **Transport** | The underlying transport: `stdio` (local subprocess), `http` (Streamable HTTP), `sse` (HTTP + SSE) |
| **OAuth** | The authorization flow required by remote servers; tokens are persisted under `~/.neo/mcp/` |

Neo's MCP client is built on [`rmcp`](https://crates.io/crates/rmcp) (see `crates/neo-agent-core/src/tools/mcp/`). Connection, reconnection, and OAuth refresh are all managed centrally by `McpConnectionManager`.

## Configuring a Server

All server configuration lives in the `[[mcp.servers]]` table. The common fields are:

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `id` | string | required | Server identifier; determines the tool namespace and must be unique |
| `enabled` | bool | `true` | Whether to start this server |
| `transport` | `"stdio"` / `"http"` / `"sse"` | required | Transport method |
| `command` | string | required for stdio | Executable name |
| `args` | array | `[]` | Subprocess arguments for stdio |
| `env` | table | `{}` | Subprocess environment variables for stdio |
| `cwd` | path | — | Subprocess working directory for stdio |
| `url` | string | required for http/sse | Remote RPC endpoint |
| `headers` | table | `{}` | Custom request headers for http/sse |
| `enabled_tools` | array | `[]` | Tool allowlist; empty means enable all |
| `disabled_tools` | array | `[]` | Tool blocklist |
| `startup_timeout_ms` | int | `5000` | Connection establishment timeout |
| `tool_timeout_ms` | int | — | Per tool-call timeout |

### stdio (local subprocess)

```toml
[[mcp.servers]]
id = "filesystem"
enabled = true
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]

[mcp.servers.env]
RUST_LOG = "info"
```

The stderr of a stdio server is silently discarded in the background so it does not pollute the TUI; the subprocess is closed when Neo exits.

### HTTP (Streamable HTTP)

```toml
[[mcp.servers]]
id = "remote-docs"
enabled = true
transport = "http"
url = "https://mcp.example.test/rpc"

[mcp.servers.headers]
"x-neo-client" = "neo"
```

The default startup timeout is 5 seconds; the server must support the Streamable HTTP protocol (`allow_stateless = true`).

### SSE (HTTP + Server-Sent Events)

```toml
[[mcp.servers]]
id = "linear"
transport = "sse"
url = "https://mcp.linear.app/mcp"
enabled = true
```

Both `http` and `sse` go through the same `OAuthStreamableHttpClient`; they differ only in the transport handshake.

## Tool Naming `mcp__<server>__<tool>`

To help the model distinguish same-named tools from different servers, Neo rewrites every MCP tool into a namespaced form:

```
mcp__<server_id>__<remote_tool_name>
```

For example, a `read_file` tool exposed by the `filesystem` server is registered in Neo's tool table as `mcp__filesystem__read_file`. Illegal characters in the server_id and tool name are normalized. On a name collision, the later registration is skipped and a diagnostic is produced.

## Permissions

MCP tool calls follow the same permission model as regular tools (see [Permission Modes](../configuration/permissions.md)). In Ask mode:

- **Session-level approval** is cached by fully qualified tool name, e.g. `mcp__filesystem__read_file`; the same tool is auto-approved within a session;
- The approval key includes the workspace root path, so it does not leak across workspaces;
- `enabled_tools` / `disabled_tools` can be used to constrain tools at the configuration layer up front.

## OAuth

When a remote server returns `401 Unauthorized`, Neo sets its state to `needs_auth` and exposes an `mcp__<server>__authenticate` tool to trigger the authorization flow. There are two ways to complete login:

| Method | Command | Use case |
| --- | --- | --- |
| TUI | `/mcp-config login <server_id>` or `/mcp` to open the management panel | Interactive |
| CLI | `neo mcp auth <server_id>` | Scripts / headless |

The OAuth token is persisted under `~/.neo/mcp/`, keyed by `<server_id> + <url>` for isolation; tokens are refreshed automatically when they expire, and on refresh failure the state returns to `needs_auth`. stdio servers do not involve OAuth.

## Debugging

| Command / Action | Effect |
| --- | --- |
| `neo mcp list` | List all configured servers and their discovered tools |
| `neo mcp status` | Show each server's connection status, tool count, and most recent error |
| `neo mcp add <name> -t <type> ...` | Add and probe a new server (`--type` accepts `studio`/`remote-http`/`remote-sse`) |
| `neo mcp del <name>` / `enable` / `disable` | Manage servers |
| `neo mcp resources [--server <id>]` | List resources of connected servers |
| `neo mcp read-resource <id> <uri>` | Read the contents of a single resource |
| `/mcp` (TUI) | Open the MCP management panel to view status, trigger reconnect, or log in |

> Configuration changes require restarting Neo or opening a new session to take effect; the `/mcp` panel can refresh and reconnect individual servers online.

Reconnection is controlled by `McpReconnectPolicy`, with exponential backoff enabled by default (`initial_delay_ms = 500`, `max_delay_ms = 30_000`, up to 5 attempts).

## Next Steps

- [Skills](skills.md) — Use the `mcp-config` skill to guide MCP configuration
- [Permission Modes](../configuration/permissions.md) — Approval granularity for MCP tools
- [Configuration Files Overview](../configuration/config-files.md) — Where `[[mcp.servers]]` goes
