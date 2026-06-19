---
name: mcp-config
description: Help the user add, remove, enable, or disable MCP servers in their configuration.
type: prompt
whenToUse: When the user wants to configure an MCP server or asks about MCP integration.
disableModelInvocation: false
---

Help the user configure MCP servers.

An MCP server entry looks like:

```toml
[mcp.servers.my-server]
type = "stdio"  # or "http" / "sse"
command = "uvx"
args = ["my-mcp-server"]
# env = { KEY = "VALUE" }
```

Steps:
1. Ask which server they want to configure and its transport type.
2. Show the exact TOML snippet before writing.
3. Use the config editing tool only after user confirmation.
4. Remind the user that MCP server changes take effect in new sessions.
