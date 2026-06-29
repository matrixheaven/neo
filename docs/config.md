# Neo Provider/Model Configuration

This document describes how to configure providers and models in Neo.

## Quick Start

Define providers and models directly in the single Neo config file
(`~/.neo/config.toml`, or `$NEO_HOME/config.toml` when `NEO_HOME` is set):

```toml
default_model = "openai/gpt-4.1"

# ─── Providers ───
[providers.openai]
type = "openai-responses"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"

[providers.anthropic]
type = "anthropic"
base_url = "https://api.anthropic.com/v1"
api_key_env = "ANTHROPIC_API_KEY"

# Custom provider with inline API key
[providers."my-local-llm"]
type = "openai-compatible"
base_url = "http://localhost:11434/v1"
api_key = "sk-local-key"

# ─── Models ───
[models."openai/gpt-4.1"]
provider = "openai"
model = "gpt-4.1"
max_context_tokens = 1047576
capabilities = ["streaming", "tools", "images", "reasoning"]
display_name = "GPT-4.1"

[models."my-local-llm/llama3"]
provider = "my-local-llm"
model = "llama3.1:8b"
max_context_tokens = 128000
capabilities = ["streaming", "tools"]
display_name = "Llama 3.1 8B"
```

## Provider Configuration

Each provider is defined in a `[providers.<id>]` table. The `<id>` is an
arbitrary name you choose — it can be anything (e.g. `openai`, `anthropic`,
`my-local-llm`, `work-gateway`).

### Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | string | Yes | Protocol type (see below) |
| `base_url` | string | Yes* | API base URL |
| `api_key` | string | No | Inline API key (stored in config) |
| `api_key_env` | string | No | Environment variable name for the key |

\* `base_url` can be omitted for built-in providers that have a default URL.

### Provider Types

| Type | Wire Protocol | Example Providers |
|------|--------------|-------------------|
| `openai-responses` | OpenAI Responses API | OpenAI |
| `openai-chat` | OpenAI Chat Completions | OpenAI |
| `openai-compatible` | OpenAI-compatible Chat Completions | OpenRouter, Ollama, vLLM, local LLMs |
| `anthropic` | Anthropic Messages API | Anthropic, Amazon Bedrock |
| `google` | Google Generative AI | Google Gemini |

### API Key

You can specify the API key in two ways:

1. **Inline** (`api_key = "sk-..."`) — stored directly in config.toml.
2. **Environment variable** (`api_key_env = "OPENAI_API_KEY"`) — reads from the
   named environment variable at runtime.

If both are specified, `api_key` takes priority.

## Model Configuration

Each model is defined in a `[models.<alias>]` table. The alias is typically
`"provider/model"` but can be any string.

### Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `provider` | string | Yes | Must match a provider id |
| `model` | string | Yes | Model ID sent to the API |
| `max_context_tokens` | number | No | Context window size |
| `max_output_tokens` | number | No | Max output tokens |
| `capabilities` | string[] | No | Capability tags |
| `display_name` | string | No | Human-readable name |

### Capability Tags

- `streaming` — supports SSE streaming
- `tools` / `tool_use` — supports function/tool calling
- `images` / `image_in` / `vision` — supports image inputs
- `reasoning` / `thinking` — supports reasoning/thinking
- `embeddings` / `embedding` — embedding model

## CLI Commands

### Provider management

```bash
# List configured providers
neo provider list

# Add a custom provider
neo provider add my-llm --type openai-compatible --base-url http://localhost:11434/v1 --api-key sk-test

# Remove a provider (also removes its models)
neo provider remove my-llm

# Browse models.dev catalog
neo provider catalog list
neo provider catalog list openai    # show models for a specific provider

# Import a provider from models.dev
neo provider catalog add openai --api-key sk-...
neo provider catalog add anthropic --api-key sk-ant-... --default-model claude-sonnet-4-5
```

### Model management

```bash
# List configured models
neo models list

# Add a model
neo models add "my-llm/codellama" --provider my-llm --model "codellama:13b" \
  --max-context-tokens 4000 --capabilities streaming,tools,reasoning

# Remove a model
neo models remove "my-llm/codellama"

# Set default model
neo models set "openai/gpt-4.1"
```

## Permission Mode

Neo uses a single top-level `permission_mode` setting that controls how
risky tool actions are approved:

```toml
permission_mode = "ask"
```

Allowed values:

- `"ask"` — Ask before commands, edits, and other risky actions.
  Read/search tools run directly, and session approval rules are respected.
- `"auto"` — Run fully non-interactively. Tool actions are approved
  automatically after hard safety policies; agent questions are skipped.
- `"yolo"` — Skip normal confirmations. Tool actions are approved
  automatically after hard safety policies, but explicit user questions
  are still allowed.

Development modes are separate from permissions. Plan mode adds a hard guard on
top of the active permission mode: `Write`/`Edit` may only modify the active
plan file, and some disruptive tools are denied. Goal mode is the structured
goal-authoring workflow and uses a review dialog before starting a durable goal.

## TUI Slash Commands

In interactive mode:

- `/ask` — Switch to ask permission mode
- `/auto` — Switch to auto permission mode
- `/yolo` — Switch to yolo permission mode
- `/permissions` — Open the permission mode selector
- `/plan` — Toggle plan mode
- Shift+Tab — Cycle development mode: normal → plan → goal → normal
- Shift+Enter, Alt+Enter, Ctrl+J — Insert a newline
- `/model` — Open the model picker
- `/provider` — Open the provider list
- `/resume` — Open session picker
- `/compact` — Manually trigger LLM-driven context compaction (summarizes older messages into a structured summary and injects it as context)
- `/new`, `/clear` — Start a fresh unsaved session in the current workspace (preserves model, permission, and development mode; the old session remains in `/resume`)

## Themes

The default theme is **magenta-dark**: a magenta (`#C678DD`) brand accent with
teal/green status colors, soft-white body text, and an amber user-role hue.
Themes live under `~/.neo/themes/*.json` (or `$NEO_HOME/themes/*.json`) and
override individual color tokens:

```bash
neo themes list
neo themes preview night-owl
neo --theme ~/.neo/themes/night-owl.json
```

Theme JSON files use a `colors` object with named color values (hex, ANSI
names, or `Reset`). The key tokens are:

- `accent` — brand color for tool names, running bullets, footer badges.
- `header` / `prompt` — body and prompt text (soft white by default).
- `user` — user message hue (amber). Only the user role has its own color;
  assistant text reuses `header`.
- `success` / `danger` / `warning` — completion, failure, and warning states.
- `muted` — secondary text, chips, overflow hints.
- `thinking` / `notice` — reasoning and system notice text.
- `diff_added` / `diff_removed` / `diff_hunk` / `diff_context` — edit diff colors.
- `footer_*` — footer badge and context-counter colors.

A reference `magenta-dark` theme is checked in at
`examples/config/magenta-dark.json`.

## Keybindings

Custom keybindings can be configured in `config.toml`:

```toml
[tui.keybindings]
# Maps key combinations to actions
```

Available actions include `session_picker_open`, `model_picker_open`,
`transcript_copy_selection`, and `session_fork`.

`tui.todo.toggle` defaults to `ctrl+t` and expands or collapses the todo panel
when more than five todo items exist.

## Importing from models.dev

Neo integrates with [models.dev](https://models.dev) for provider discovery:

```bash
# See all available providers
neo provider catalog list

# Import a provider with all its models
neo provider catalog add deepseek --api-key sk-...
```

This fetches the catalog, infers the wire type, and writes the provider +
all its models to `config.toml` automatically.

## Skill search paths

Use `skill_path` to add extra directories where Neo looks for skills. It can be
a single string or a list of strings. `~/.neo/skills/` and the built-in
`.builtin/` release are always searched; `skill_path` entries are searched as
additional global skill directories.

```toml
# single path
skill_path = "~/.agents/skills"

# multiple paths
skill_path = ["~/.agents/skills", "~/.claude/skills"]
```

## Environment Variables

| Variable | Description |
|----------|-------------|
| `NEO_CONFIG` | Path to config file |
| `NEO_HOME` | Override Neo's home directory |

## Config Precedence

1. CLI flags (`--model`, `--provider`, `--api-key`)
2. User-global config (`~/.neo/config.toml`, or `$NEO_HOME/config.toml`)
3. Built-in defaults
