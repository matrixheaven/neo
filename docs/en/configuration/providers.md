# Provider Configuration

Neo declares any number of LLM backends via the `[providers.<id>]` table in `config.toml`, then attaches models to providers with `[models.<alias>]`. A provider's protocol is determined by the `type` field, and Neo selects the corresponding wire client based on it.

## Supported Provider Types

| `type` value | Protocol | Use case |
| --- | --- | --- |
| `openai` | OpenAI Chat Completions (`/chat/completions`) | OpenAI official, OpenRouter, Ollama, vLLM, DeepSeek, or any OpenAI-compatible endpoint |
| `openai_response` | OpenAI Responses API (`/responses`) | OpenAI official Responses API (supports native reasoning, tool calls, etc.) |
| `anthropic` | Anthropic Messages API | Claude family models |
| `google` | Google Generative AI | Gemini family models |

> The legacy `openai-chat` / `openai-compatible` / `openai-responses` types have been removed. Use `openai` for Chat Completions compatible endpoints; use `openai_response` for the official OpenAI Responses API.

## TOML Snippets per Provider

### OpenAI Responses

```toml
[providers.openai]
type = "openai_response"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
```

### OpenAI Chat Completions

```toml
[providers.openai-chat]
type = "openai"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
```

### Anthropic

```toml
[providers.anthropic]
type = "anthropic"
base_url = "https://api.anthropic.com/v1"
api_key_env = "ANTHROPIC_API_KEY"
```

### Google Gemini

```toml
[providers.google]
type = "google"
base_url = "https://generativelanguage.googleapis.com/v1beta"
api_key_env = "GEMINI_API_KEY"
```

## Environment Variable Precedence

A provider's API key is resolved in the following order, returning on the first hit:

1. **`api_key_env`** — reads this environment variable (recommended, to avoid writing plaintext into the config);
2. **`api_key`** — the inline key string in the config file;
3. If neither is set → an unauthorized error is returned when calling the API.

When both are present, **the environment variable takes precedence** — Neo falls back to `api_key` only when the variable named by `api_key_env` has no value.

```toml
# Recommended: inject via environment variable
[providers.openai]
type = "openai_response"
api_key_env = "OPENAI_API_KEY"

# Or inline directly (keep it secret)
[providers.openrouter]
type = "openai"
base_url = "https://openrouter.ai/api/v1"
api_key = "sk-or-v1-xxxxxxxxxxxx"
```

> There is also a top-level global `api_key_env` field, used only as a fallback; a provider's own `api_key_env` overrides it.

## Custom Providers

Any OpenAI-compatible endpoint can be used with `type = "openai"` — simply point `base_url` at your service.

### Ollama (local)

```toml
[providers."local-ollama"]
type = "openai"
base_url = "http://localhost:11434/v1"
api_key = "ollama"   # Ollama does not validate the key, any string works
```

### OpenRouter

```toml
[providers.openrouter]
type = "openai"
base_url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"

[models."openrouter/deepseek-r1"]
provider = "openrouter"
model = "deepseek/deepseek-r1"
max_context_tokens = 128000
capabilities = ["streaming", "tools", "reasoning"]
```

### DeepSeek / vLLM / Other Compatible Endpoints

```toml
[providers.deepseek]
type = "openai"
base_url = "https://api.deepseek.com/v1"
api_key_env = "DEEPSEEK_API_KEY"

[models."deepseek-chat"]
provider = "deepseek"
model = "deepseek-chat"
max_context_tokens = 64000
capabilities = ["streaming", "tools"]
```

## Model Capability Declarations

Each model declares its supported capability tags via the `capabilities` field:

| Tag | Meaning |
| --- | --- |
| `streaming` | Supports streaming output |
| `tools` | Supports tool / function calling |
| `images` | Supports image input (vision models) |
| `reasoning` | Supports reasoning / thinking content |

```toml
[models."anthropic/claude-sonnet-4-5"]
provider = "anthropic"
model = "claude-sonnet-4-5-20250514"
max_context_tokens = 200000
capabilities = ["streaming", "tools", "images", "reasoning"]
display_name = "Claude Sonnet 4.5"
```

Capability tags are used for UI hints and capability routing (e.g. reasoning effort only takes effect on models that declare `reasoning`). When omitted, Neo infers them from the model's default capabilities.

### Provider-defined reasoning efforts

Providers may define effort values beyond Neo's common presets:

```toml
[runtime]
reasoning = { mode = "effort", effort = "UltraMax" }
```

Effort values are provider-defined and case-sensitive. Providers with a native
effort field receive the value exactly as written; budget- or toggle-based
adapters reject values they cannot map. Empty or whitespace-only values are
invalid. Consult the provider's model documentation for supported values.

## Next Steps

- [Configuration Files](config-files.md) — full field table for `config.toml`
- [Permission Modes](permissions.md) — Ask / Auto / Yolo mode descriptions
- `examples/config/providers-models.toml` — complete, copy-ready provider/model configuration examples
