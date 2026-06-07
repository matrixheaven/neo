# Configuration

Configuration currently covers the wired `neo-agent` CLI surface and keeps
provider/MCP extension points visible without treating development fixtures as
production defaults.

## Current Precedence

`neo-agent` currently resolves config in this order:

1. CLI flags for a single invocation.
2. `NEO_*` environment variables.
3. Project config at `.neo/config.toml` or the path passed with `--config`.
4. Built-in `openai/gpt-4.1` defaults.

There is no user-global Neo config file yet.

## Project Config

The current loader accepts this shape:

```toml
default_provider = "openai"
default_model = "gpt-4.1"
api_key_env = "OPENAI_API_KEY"
sessions_dir = ".neo/sessions"

[providers.openai]
api_key_env = "PROJECT_OPENAI_KEY"

[permissions]
file_read = "Allow"
file_write = "Ask"
shell = "Ask"
tool = "Allow"

[defaults]
mode = "interactive"

[[mcp.servers]]
id = "filesystem"
enabled = true
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
```

`api_key_env` names an environment variable. Provider-specific entries such as
`[providers.openai].api_key_env` name the environment variable for that provider
without storing the secret value. Do not write raw API keys into config files.
`neo-agent` resolves the configured model through `ModelRegistry::seeded()` and
`ProviderRegistry::production()`. With the built-in defaults, set
`OPENAI_API_KEY` before running provider-backed commands. Custom
OpenAI-compatible deployments can override `api_base` and `api_key_env` for the
selected provider.

The default permissions mirror `neo_agent_core::PermissionPolicy::default()`:
file reads are allowed, file writes ask, shell asks, and tools are allowed.
Tool approval request events exist in `neo-agent-core`; a full interactive
ask/approve UI remains a runtime gap.

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

`neo mcp list` reads configured server entries from `.neo/config.toml`.
`neo print` and `neo run` discover enabled `transport = "stdio"` entries,
register their remote tools as `mcp__<server>__<tool>` model functions, and
call the original remote MCP tool names over stdio JSON-RPC. The current shape
includes:

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
neo skills show <path>
neo extensions list [root]
neo extensions status <extension-id> --root <root>
neo extensions enable <extension-id> --root <root>
neo extensions disable <extension-id> --root <root>
neo extensions call <extension-id> <method> [params] --root <root>
```

Supported `config set` keys are:

- `default_model` or `model`
- `default_provider` or `provider`
- `api_base`
- `api_key_env`
- `providers.<provider-id>.api_key_env`
- `sessions_dir`
- `permissions.file_read` or `file_read`
- `permissions.file_write` or `file_write`
- `permissions.shell` or `shell`
- `permissions.tool` or `tool`
- `defaults.mode` or `mode`

Use TOML enum values `Allow`, `Ask`, or `Deny` for permission settings.
