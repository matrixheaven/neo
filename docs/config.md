# Neo Provider/Model Configuration

This document describes how to configure providers and models in Neo.

## Quick Start

Define providers and models directly in `config.toml` (usually `~/.neo/config.toml`
or `.neo/config.toml` in your project):

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
| `api_base` | string | No | Legacy alias for `base_url` |

\* `base_url` can be omitted for built-in providers that have a default URL.

### Provider Types

| Type | Wire Protocol | Example Providers |
|------|--------------|-------------------|
| `openai-responses` | OpenAI Responses API | OpenAI |
| `openai-chat` | OpenAI Chat Completions | OpenAI (legacy) |
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

## TUI Slash Commands

In interactive mode:

- `/model` — Open the model picker
- `/provider` — Open the provider list
- `/resume` — Open session picker

## Themes

The default theme is **magenta-dark**: a magenta (`#C678DD`) brand accent with
teal/green status colors, soft-white body text, and an amber user-role hue.
Project themes live under `.neo/themes/*.json` and override individual color
tokens:

```bash
neo themes list
neo themes preview night-owl
neo --theme .neo/themes/night-owl.json
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

## JSON Catalog Files (Legacy)

For backward compatibility, Neo still supports JSON catalog files via
`model_catalogs`:

```toml
model_catalogs = [".neo/models.json"]
```

Models from JSON catalogs are loaded in addition to those defined in
`config.toml`. New users should prefer `[models.<alias>]` tables.

## Environment Variables

| Variable | Description |
|----------|-------------|
| `NEO_MODEL` | Override default model |
| `NEO_PROVIDER` | Override default provider |
| `NEO_API_BASE` | Override base URL for selected provider |
| `NEO_API_KEY_ENV` | Override API key env var name |
| `NEO_CONFIG` | Path to config file |

## Config Precedence

1. CLI flags (`--model`, `--provider`, `--api-key`)
2. Environment variables (`NEO_*`)
3. Project config (`.neo/config.toml`)
4. User-global config (`~/.neo/config.toml`)
5. Built-in defaults
