# Configuration

Configuration currently covers the wired `neo-agent` CLI surface and keeps
provider/MCP extension points visible without treating development fixtures as
production defaults.

## Current Precedence

`neo-agent` currently resolves config in this order:

1. CLI flags for a single invocation.
2. `NEO_*` environment variables.
3. Project config at `.neo/config.toml` or the path passed with `--config`.
4. User-global config at `~/.neo/config.toml`.
5. Built-in `openai/gpt-4.1` defaults.

Project config is merged over user-global config field by field. Provider maps
are merged by provider id, MCP servers are merged by server id with project
entries taking precedence, and runtime options preserve global values when the
project only overrides a subset. `sessions_dir` supports `~` expansion.

## Project Config

The current loader accepts this shape:

```toml
default_provider = "openai"
default_model = "gpt-4.1"
api_key_env = "OPENAI_API_KEY"
sessions_dir = ".neo/sessions"
model_catalogs = [".neo/models.json"]
prompt_templates = ["prompts"]

[providers.openai]
api_base = "https://api.openai.com/v1"
api_key_env = "PROJECT_OPENAI_KEY"

[permissions]
file_read = "Allow"
file_write = "Ask"
shell = "Ask"
tool = "Allow"

[defaults]
mode = "interactive"

[runtime]
temperature = 0.2
max_tokens = 4096
steering_queue_mode = "All"
follow_up_queue_mode = "All"
tool_execution_mode = "Parallel"

[runtime.compaction]
enabled = true
max_estimated_tokens = 32000
keep_recent_messages = 20

[tui.keybindings]
"tui.command.open" = ["ctrl+p"]
"tui.session.open" = ["ctrl+r"]
"tui.model.open" = ["ctrl+o"]

[[mcp.servers]]
id = "filesystem"
enabled = true
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]

[[mcp.servers]]
id = "remote-docs"
enabled = true
transport = "http"
url = "https://mcp.example.test/rpc"

[mcp.servers.headers]
"x-neo-client" = "neo"
```

`api_key_env` names an environment variable. Provider-specific entries such as
`[providers.openai].api_base` and `[providers.openai].api_key_env` override the
base URL and environment variable name for that provider without storing secret
values. Do not write raw API keys into config files.
`neo-agent` resolves the configured model through the built-in
`ModelRegistry::seeded()` entries plus any strict JSON `model_catalogs`, then
uses `ProviderRegistry::production()` for provider clients. With the built-in
defaults, set `OPENAI_API_KEY` before running provider-backed commands. Custom
OpenAI-compatible deployments can add a model catalog entry and override
`providers.<provider-id>.api_base` and `providers.<provider-id>.api_key_env`
for a built-in provider id. Top-level `api_base` and `api_key_env`, plus
`--api-base`, `NEO_API_BASE`, and `NEO_API_KEY_ENV`, remain selected-provider
overrides and take precedence over provider-specific config for that invocation.

User defaults can live in `~/.neo/config.toml`; project `.neo/config.toml`
overrides them for that workspace.

System prompt resources are plain local Markdown files. `neo print`, `neo run`,
RPC `prompt`, and live TUI turns read `.neo/SYSTEM.md` as the system message
sent before the user prompt. If the project file is absent, Neo falls back to
`~/.neo/SYSTEM.md`. `.neo/APPEND_SYSTEM.md` and then `~/.neo/APPEND_SYSTEM.md`
follow the same project-over-global precedence and append a second paragraph to
the system prompt. Empty files are ignored, and no hosted trust or marketplace
state is inferred from these files.

`prompt_templates` accepts the same local selector shape as repeatable
`--prompt-template`: template names, project-contained `.md` files, or
non-recursive directories of `.md` prompt templates. Selectors from user-global
and project config are merged, with duplicate selector strings loaded once, and
CLI `--prompt-template` selectors are added for the current invocation.
Prefix a selector with `-` to filter an auto-discovered local prompt template,
for example `"-prompts/review.md"` excludes `.neo/prompts/review.md` from slash
discovery and RPC command listing without requiring the file to exist. Negative
selectors do not disable explicitly included positive selectors.

The default permissions mirror `neo_agent_core::PermissionPolicy::default()`:
file reads are allowed, file writes ask, shell asks, and tools are allowed.
Ask-mode operations emit approval request events in `neo-agent-core`; CLI modes
can use static approve/deny handlers, and live interactive mode routes approval
overlay choices back into the runtime's pending async approval handler.

The `[runtime]` table maps directly to `neo_agent_core::AgentConfig` for real
provider-backed runs. `temperature`, `max_tokens`, and `reasoning_effort` are
sent through `neo_ai::RequestOptions`; queue modes use `All` or `OneAtATime`;
tool execution uses `Parallel` or `Sequential`; and optional compaction
settings control the runtime's deterministic context compaction trigger.
Supported reasoning effort values are `minimal`, `low`, `medium`, `high`, and
`xhigh`. OpenAI Responses serializes this as a Responses `reasoning` object;
OpenAI-compatible Chat Completions serializes it as `reasoning_effort`.

The `[tui.keybindings]` table maps `neo-tui` action IDs to arrays of normalized
key IDs. TOML keys must be quoted because action IDs contain dots. Project
bindings override user-global bindings per action, and each action's configured
array replaces that action's default key list. Config loading and `config set`
validate action IDs, key syntax, text-insertion reserved keys, and same-context
conflicts before the interactive controller receives the resolved keybinding
manager.

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
`neo print` and `neo run` discover enabled `transport = "stdio"`,
`transport = "http"`, and `transport = "sse"` entries, register their remote
tools as `mcp__<server>__<tool>` model functions, and call the original remote
MCP tool names over a real JSON-RPC transport. Stdio servers reuse an
initialized process session; HTTP/SSE servers send JSON-RPC POST requests and
accept JSON or SSE `data:` JSON-RPC responses. `neo mcp resources <server>
list` and `neo mcp resources <server> read <uri>` explicitly fetch resource
catalogs and content from a configured enabled server. `neo mcp resources
<server> watch <uri>` subscribes over stdio or over remote HTTP/SSE servers that
return a live SSE subscribe response, waits for real
`notifications/resources/updated` messages, prints updated URIs, and
unsubscribes before exiting. The current shape includes:

- Server id.
- Transport type: `stdio`, `http`, or `sse`.
- Command and arguments for local stdio servers.
- URL and optional headers for remote HTTP/SSE servers.
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
neo mcp resources <server-id> list
neo mcp resources <server-id> read <uri>
neo mcp resources <server-id> watch <uri> [--count <n>]
neo skills show <path>
neo extensions list [root]
neo extensions install <path-or-git-url> --root <root>
neo extensions update <extension-id> --root <root>
neo extensions uninstall <extension-id> --root <root>
neo extensions status <extension-id> --root <root>
neo extensions enable <extension-id> --root <root>
neo extensions disable <extension-id> --root <root>
neo extensions call <extension-id> <method> [params] --root <root>
neo sessions summarize <session-id>
```

Supported `config set` keys are:

- `default_model` or `model`
- `default_provider` or `provider`
- `api_base`
- `api_key_env`
- `providers.<provider-id>.api_base`
- `providers.<provider-id>.api_key_env`
- `prompt_templates`
- `sessions_dir`
- `permissions.file_read` or `file_read`
- `permissions.file_write` or `file_write`
- `permissions.shell` or `shell`
- `permissions.tool` or `tool`
- `defaults.mode` or `mode`
- `runtime.temperature` or `temperature`
- `runtime.max_tokens` or `max_tokens`
- `runtime.steering_queue_mode` or `steering_queue_mode`
- `runtime.follow_up_queue_mode` or `follow_up_queue_mode`
- `runtime.tool_execution_mode` or `tool_execution_mode`
- `runtime.compaction.enabled` or `compaction.enabled`
- `runtime.compaction.max_estimated_tokens` or `compaction.max_estimated_tokens`
- `runtime.compaction.keep_recent_messages` or `compaction.keep_recent_messages`
- `tui.keybindings.<tui-action-id>`

Use TOML enum values `Allow`, `Ask`, or `Deny` for permission settings.
Use `All` or `OneAtATime` for queue modes and `Parallel` or `Sequential` for
tool execution mode.
Use a TOML string array for keybinding overrides, for example
`neo config set tui.keybindings.tui.command.open '["ctrl+g", "ctrl+p"]'`.
