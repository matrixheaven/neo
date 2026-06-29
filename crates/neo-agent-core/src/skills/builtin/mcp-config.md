---
name: mcp-config
description: Use when the user wants to configure MCP servers, inspect MCP status, or log in to an MCP server that needs OAuth.
type: prompt
disableModelInvocation: false
---

# MCP Configuration

Handle MCP work locally in this turn. Do not delegate. Choose the flow from the user message and available tools.

## Login

If the user asks to login/auth/sign in, invokes `/mcp-config login <server>`, or mentions an OAuth/needs-auth MCP server:

1. Call the matching `mcp__<server>__authenticate` tool when it is available.
2. Surface the tool output exactly enough for the user to open the authorization URL. Do not edit the URL.
3. If the named server has no authenticate tool, say it is not currently waiting for OAuth and stop.
4. If multiple authenticate tools exist and the user did not name a server, ask which server to authenticate.

Manual OAuth is also available from `/mcp` in the MCP manager UI. Prefer that path when the user wants an interactive UI flow or the authenticate tool says callback completion is not wired in core.

## Config Edit

Neo MCP config lives in `~/.neo/config.toml` under `[[mcp.servers]]`. Resolve Neo home with `$NEO_HOME` if set, otherwise `~/.neo`; do not assume a project-local MCP config.

Example:

```toml
[[mcp.servers]]
id = "linear"
transport = "http"
url = "https://mcp.linear.app/mcp"
enabled = true
```

For stdio:

```toml
[[mcp.servers]]
id = "filesystem"
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/root"]
enabled = true
```

Rules:

1. For list/status requests, inspect config and current MCP status, then stop.
2. For changes, show the target file and exact TOML change before writing.
3. Preserve unrelated providers, models, and MCP entries.
4. Do not write literal secrets. Prefer env vars or headers that reference env-managed values.
5. MCP servers connect at session start. After config changes, tell the user to restart Neo or start a new session; `/mcp` can also refresh/test configured servers.
