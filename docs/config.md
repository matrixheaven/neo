# Configuration

Configuration should let contributors start with a project-local fake model
while leaving enough structure for provider and MCP work to land later.

## Current Precedence

`neo-agent` currently resolves config in this order:

1. CLI flags for a single invocation.
2. `NEO_*` environment variables.
3. Project config at `.neo/config.toml` or the path passed with `--config`.
4. Built-in defaults suitable for local fake-model testing.

There is no user-global Neo config file yet.

## Project Config

The current loader accepts this shape:

```toml
default_provider = "fake"
default_model = "fake"
api_base = "http://127.0.0.1:11434/v1"
api_key_env = "OPENAI_API_KEY"
sessions_dir = ".neo/sessions"

[permissions]
file_read = "Allow"
file_write = "Ask"
shell = "Ask"

[defaults]
mode = "interactive"
```

`api_key_env` names an environment variable. Do not write raw API keys into
config files.

The default permissions mirror `neo_agent_core::PermissionPolicy::default()`:
file reads are allowed, file writes ask, and shell asks. Tool execution currently
treats only `Allow` as executable; an interactive ask/approve flow is still a
runtime gap.

## Environment Variables

| Variable | Maps to |
| --- | --- |
| `NEO_MODEL` | `default_model` |
| `NEO_PROVIDER` | `default_provider` |
| `NEO_API_BASE` | `api_base` |
| `NEO_API_KEY_ENV` | `api_key_env` |
| `NEO_SESSIONS_DIR` | `sessions_dir` |
| `NEO_MODE` | `defaults.mode` |
| `NEO_CONFIG` | config file path |

## MCP Config

MCP server entries are not loaded by the current `neo-agent` config module yet.
The intended shape includes:

- Server id.
- Transport type such as `stdio`.
- Command and arguments for local stdio servers.
- Environment variables required by the server.
- Whether the server is enabled by default.

See [examples/config/mcp-server.toml](../examples/config/mcp-server.toml).

## CLI Surface

The `neo-agent` binary exposes:

```bash
neo config show
neo config set <key> <value>
neo models list
neo mcp list
```

Supported `config set` keys are:

- `default_model` or `model`
- `default_provider` or `provider`
- `api_base`
- `api_key_env`
- `sessions_dir`
- `permissions.file_read` or `file_read`
- `permissions.file_write` or `file_write`
- `permissions.shell` or `shell`
- `defaults.mode` or `mode`

Use TOML enum values `Allow`, `Ask`, or `Deny` for permission settings.
