# Configuration Files

Neo uses a **single configuration file** `~/.neo/config.toml` (TOML format) to manage all global settings, providers, models, runtime parameters, and MCP servers. All workspaces share the same configuration — Neo no longer reads project-level configuration files.

## Configuration File Location

| Location | Description |
| --- | --- |
| `$NEO_HOME/config.toml` | Used first when the `NEO_HOME` environment variable is set |
| `~/.neo/config.toml` | Default path (recommended) |
| `--config <path>` | CLI argument, temporarily overrides the path (see `neo --help`) |

> Neo can start without a `.neo/config.toml` — every field has a default value. Create one on demand the first time you run `neo`.

## Top-Level Field Overview

The top-level fields of `config.toml` come from `FileConfig`:

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `default_model` | string | `"gpt-4.1"` | Default model alias; may be an alias from `[models.<alias>]`, or a direct `<provider>/<model>` |
| `default_provider` | string | `"openai"` | Default provider id, used to compose the display label when `default_model` does not contain `/` |
| `api_key_env` | string | — | Global API key environment variable name (the provider's own `api_key_env` overrides this value) |
| `permission_mode` | `"ask"` \| `"auto"` \| `"yolo"` | `"ask"` | Default permission mode, see [Permission Modes](permissions.md) |
| `sessions_dir` | path | `~/.neo/sessions` | Session storage root directory, supports `~` expansion |
| `model_scope` | string[] | `[]` (i.e. all) | List of model globs restricting available models, e.g. `["openai/gpt-*", "claude-sonnet-4:high"]` |
| `skill_path` | string \| string[] | `[]` | Extra skill directories; may be written as a single string or an array of strings |
| `extra_skill_dirs` | string[] | `[]` | Extra skill directories (equivalent to `skill_path`, list form) |
| `prompt_templates` | string[] | `[]` | List of custom prompt template directories |
| `system_prompt_file` | path | `~/.neo/SYSTEM.md` when present | Custom system prompt file. Equivalent to `~/.neo/SYSTEM.md`: it replaces Neo's built-in system prompt and supports `~` expansion |
| `providers` | table | — | `[providers.<id>]` table, see [Provider Configuration](providers.md) |
| `models` | table | — | `[models.<alias>]` table |
| `runtime` | table | — | `[runtime]` inference parameters |
| `tui` | table | — | `[tui]` terminal UI settings |
| `mcp` | table | — | MCP server configuration |

```toml
# config.toml top-level example
default_model = "openai/gpt-4.1"
default_provider = "openai"
permission_mode = "ask"
sessions_dir = "~/.neo/sessions"
system_prompt_file = "~/.neo/SYSTEM.md"
```

## System Prompt Files

Neo builds the model system message in this order:

1. Base system prompt: `system_prompt_file` when configured, otherwise `~/.neo/SYSTEM.md` when it exists, otherwise Neo's built-in prompt.
2. `~/.neo/APPEND_SYSTEM.md` when it exists.
3. Available skill metadata.

`SYSTEM.md` and `system_prompt_file` replace the built-in base prompt. `APPEND_SYSTEM.md` is the append-only hook for keeping Neo's built-in prompt and adding user instructions after it.

Project instructions (`AGENTS.md`) are no longer part of the system message. Neo loads the trust-gated, path-scoped instruction chain as durable, session-scoped instruction epochs stored in the session event stream, so they never mutate `system_prompt` or earlier request bytes. `CLAUDE.md` is no longer a fallback filename. See [AGENTS.md](../customization/agents.md#agentsmd).

## `[providers.<id>]` Table

Each provider is declared with a `[providers.<id>]` sub-table. The `<id>` is a name you choose and is referenced by `default_provider` and each model's `provider` field.

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `type` | `openai` \| `openai_response` \| `anthropic` \| `google` | `openai` | Provider protocol type, determines which wire client is used |
| `base_url` | string | — | API base URL, e.g. `https://api.openai.com/v1` |
| `api_key` | string | — | Inline API key (stored in plaintext in the config file) |
| `api_key_env` | string | — | Name of the environment variable holding the API key, e.g. `OPENAI_API_KEY` |

> `api_key_env` and `api_key` may coexist; at runtime the environment variable is read first, falling back to the inline value only if it is unavailable. For the exact strategy, see [Provider Configuration](providers.md#environment-variable-precedence).

## `[models.<alias>]` Table

Each model is declared with `[models."<alias>"]`. The alias is conventionally `<provider>/<model-name>`, but this is not enforced.

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `provider` | string | **required** | The provider id being referenced (must already exist) |
| `model` | string | **required** | The actual model id sent to the API, e.g. `gpt-4.1`, `claude-sonnet-4-5-20250514` |
| `max_context_tokens` | u32 | — | Context window size (in tokens) |
| `max_output_tokens` | u32 | — | Maximum output tokens per turn; uses the model's built-in value when unset |
| `capabilities` | string[] | `[]` | Capability tags: `streaming` / `tools` / `images` / `reasoning` |
| `display_name` | string | — | Friendly name shown in the picker |

```toml
[models."openai/gpt-4.1"]
provider = "openai"
model = "gpt-4.1"
max_context_tokens = 1047576
capabilities = ["streaming", "tools", "images", "reasoning"]
display_name = "GPT-4.1"
```

Capability tags are protocol-agnostic and are used only for UI hints and capability routing; when omitted, Neo infers them from the model's default capabilities.

## `[runtime]` Table

Controls inference request parameters:

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `temperature` | f64 | — | Sampling temperature, must be a finite non-negative number |
| `max_tokens` | u32 | — | Maximum output tokens, must be > 0 |
| `reasoning` | table | `mode = "off"` | Structured reasoning control (only effective for models that support reasoning) |
| `replay_reasoning` | bool | `true` | Whether to include reasoning fragments when replaying history |
| `steering_queue_mode` | `all`\|`one_at_a_time` | `all` | Steering message queue mode |
| `follow_up_queue_mode` | `all`\|`one_at_a_time` | `all` | Follow-up message queue mode |
| `tool_execution_mode` | `sequential`\|`parallel` | `parallel` | Execution mode for multiple tool calls within the same turn |

```toml
[runtime]
temperature = 0.2
max_tokens = 4096

[runtime.reasoning]
mode = "effort"
effort = "high"
```

### `[runtime.reasoning]` Sub-Table

`mode = "off"` disables reasoning (default), `mode = "on"` enables the provider/model default, `mode = "effort"` sends an explicit provider-defined `effort` when supported, and `mode = "budget_tokens"` sends an explicit `budget_tokens` count when supported. Common effort values include `minimal`, `low`, `medium`, `high`, `xhigh`, and `max`; providers may declare other non-empty, case-sensitive values.

### `[runtime.retry]` Sub-Table

Neo retries retryable `Transport`, `RateLimit`, and `Server` failures at runtime; permanent `QuotaExhausted` is terminal:

```toml
[runtime.retry]
max_retries = 5
first_event_timeout_secs = 60
stream_idle_timeout_secs = 120
```

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `max_retries` | u32 | `5` | Retry requests allowed after the initial request |
| `first_event_timeout_secs` | u64 | `60` | Deadline for the first normalized stream event |
| `stream_idle_timeout_secs` | u64 | `120` | Maximum silence between later normalized stream events |

The three `0` values are independent: `max_retries = 0` disables retries, `first_event_timeout_secs = 0` disables only the first-event deadline, and `stream_idle_timeout_secs = 0` disables only the idle deadline. Neo always makes the initial request; `max_retries` counts additional requests, so `max_retries = 100` permits up to 101 total requests.

The first-event deadline runs until Neo receives the first normalized stream event. After that, the idle deadline measures silence between later normalized events. Provider keepalive comments do not reset either deadline. A deadline expiry is a retryable `Transport` failure.

Ordinary retries resend the same frozen request, keeping prompt and cache identity stable. Deltas from a failed attempt are not persisted to canonical context and are not included in replay. A valid `Retry-After` overrides local backoff and is capped at 24 hours. Permanent `QuotaExhausted` is terminal: Neo does not retry it or show a reconnect Card.

Press `Esc` to cancel an active stream or retry wait. The inline Card animates while waiting or connecting; replay restores only an exhausted state, never an in-progress animation.

### `[runtime.shell]` Sub-Table

Shared shell admission and per-command resource limits for `Bash` and `Terminal`:

```toml
[runtime.shell]
max_active_commands = 8
max_command_parallelism = 4
max_command_descendant_processes = 32
max_command_memory_percent = 25
max_output_bytes = 65536
max_background_log_bytes = 10485760
```

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `max_active_commands` | usize | `8` | Maximum concurrently running shell commands across the shared scheduler |
| `max_command_parallelism` | usize | `4` | Per-command advisory parallelism budget (for example `CARGO_BUILD_JOBS` when unset) |
| `max_command_descendant_processes` | usize | `32` | Maximum observed descendant processes for each command tree |
| `max_command_memory_percent` | u8 | `25` | Maximum resident-memory percentage for each command tree (`1..=100`) |
| `max_output_bytes` | usize | `65536` | Maximum captured shell output bytes retained for tool results |
| `max_background_log_bytes` | u64 | `10485760` | Maximum background command log size on disk |

`max_active_commands` controls scheduler capacity only. At capacity, new shell calls wait transparently instead of returning a capacity error. Agent-started background Bash and Terminal sessions share a fixed cap of three, leaving five default slots available to user and foreground agent work. The three `max_command_*` values are direct per-command budgets and are never divided by capacity. All integer limits must be positive.

### `[runtime.compaction]` Sub-Table

Context compaction is enabled by default. Fresh config writes include this table; if the table is missing from an older config, Neo still uses the enabled defaults. Set `enabled = false` explicitly to disable it. All other sub-fields are optional:

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | Whether automatic compaction is enabled |
| `max_estimated_tokens` | usize | `32000` | Target upper token limit after compaction |
| `keep_recent_messages` | usize | `20` | Number of recent messages to preserve during compaction |
| `trigger_ratio` | f64 | `0.85` | Context occupancy threshold that triggers compaction |
| `reserved_context_tokens` | usize | `50000` | Reserved trailing token margin |
| `max_recent_messages` | usize | `4` | Number of very recent messages preserved during automatic compaction |
| `micro_enabled` | bool | `false` | Whether micro compaction (truncation of old tool results) is enabled |
| `micro_keep_recent` | usize | `20` | Number of recent messages exempt from micro compaction |
| `max_rounds` | usize | `5` | Maximum rounds in a single compaction |
| `max_retry_attempts` | u32 | `5` | Maximum retry attempts for empty/truncated summaries |

## `[tui]` Table

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `image_protocol` | `auto`\|`kitty`\|`iterm2`\|`sixel`\|`none` | `auto` | Image rendering protocol preference |
| `keybindings` | map<string, string[]> | `{}` | Custom keybindings (action → list of keys) |
| `completion_notification` | `none`\|`bell`\|`system`\|`all` | `bell` | Task completion notification method |
| `question_notification` | `none`\|`bell`\|`system`\|`all` | `none` | Notification method triggered by `AskUserQuestion` |

## `[defaults]` Table

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `mode` | string | `"interactive"` | Default startup mode (`interactive` / `run`, etc.) |

## About Project-Level Configuration

Neo **no longer supports** project-level `.neo/config.toml` or `local.toml`. All providers, models, settings, skills, prompts, and themes are unified under `~/.neo/` and shared across workspaces. If you want to differentiate models or permission modes per project, you can:

- Set `export NEO_HOME=/path/to/project-neo` in your shell startup script so each project points to a different neo home;
- Or use `neo --config /path/to/custom.toml` to explicitly specify a configuration file.

## Complete Example

The repository's `examples/config/` directory provides ready-to-copy templates:

- [`examples/config/providers-models.toml`](../../../examples/config/providers-models.toml) — covers the full provider/model syntax for OpenAI, Anthropic, Google, OpenRouter, and Ollama
- [`examples/config/mcp-server.toml`](../../../examples/config/mcp-server.toml) — MCP server configuration reference

```toml
# ~/.neo/config.toml — minimal working configuration
default_model = "openai/gpt-4.1"

[providers.openai]
type = "openai_response"
api_key_env = "OPENAI_API_KEY"

[models."openai/gpt-4.1"]
provider = "openai"
model = "gpt-4.1"
max_context_tokens = 1047576
capabilities = ["streaming", "tools", "images", "reasoning"]
```

## Next Steps

- [Provider Configuration](providers.md) — the four provider types and complete syntax for custom endpoints
- [Permission Modes](permissions.md) — Ask / Auto / Yolo modes and approval granularity
- [Data Storage Locations](data-locations.md) — `~/.neo/` directory structure and cleanup guide
