# Configuration

Configuration should let ordinary users start quickly while still giving maintainers explicit provider and MCP controls.

## Intended Precedence

1. CLI flags for a single invocation.
2. Environment variables for secrets and automation.
3. Project config in the current workspace.
4. User config in the home directory.
5. Built-in defaults suitable for local testing.

## Provider Config

Provider entries should include:

- Stable provider id.
- API kind.
- Base URL when relevant.
- Default model.
- Optional capability overrides.
- Environment variable name for the API key.

Do not write raw API keys into config files.

## MCP Config

MCP server entries should include:

- Server id.
- Transport type such as `stdio`.
- Command and arguments for local stdio servers.
- Environment variables required by the server.
- Whether the server is enabled by default.

See [examples/config/mcp-server.toml](../examples/config/mcp-server.toml).

## CLI Surface

The `neo-agent` binary reserves:

```bash
neo config show
neo config set <key> <value>
neo models list
neo mcp list
```

These commands currently print command placeholders. The future config loader should live outside the binary so tests can exercise it without spawning a CLI process.
