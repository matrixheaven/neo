# Providers

Provider integration starts in `neo-ai`. The crate defines the shared model
contract, request options, model/provider registries, production resolver, and
test client.

## Core Types

- `ProviderId` identifies a configured provider.
- `ApiKind` names the protocol family, including OpenAI Responses, Chat Completions, Anthropic Messages, Google Generative AI, OpenAI-compatible APIs, and local providers.
- `ModelCapabilities` tells the runtime whether a model supports streaming, tools, images, reasoning, embeddings, and an optional context limit.
- `ModelSpec` combines provider, model name, API kind, and capabilities.
- `RequestOptions` carries temperature, max tokens, custom headers, timeout,
  retry count, cache retention, provider session id, and metadata.
- `ChatRequest` combines `model`, `messages`, `tools`, and `options`.
- `ModelClient::stream_chat` accepts a `ChatRequest` and returns normalized
  `AiStreamEvent` values.
- `ModelRegistry` stores available `ModelSpec` values and exposes the first
  registered model as the default unless replaced by configuration.
- `ModelRegistry::load_catalog_path` and `ModelRegistry::load_catalog_str` load
  strict local JSON catalogs that use either the existing `ModelSpec` wire
  shape or the supported custom-model subset of Pi `models.json`.
- `ProviderRegistry::production()` registers the provider catalog used by
  production resolution: OpenAI, Anthropic, Google Generative AI, OpenRouter,
  and Amazon Bedrock credential hints.
- `ProviderResolver` turns a registered `ModelSpec` plus environment credentials
  into a `ModelClient`. It constructs `OpenAiResponsesClient`,
  `AnthropicMessagesClient`, `GoogleGenerativeAiClient`, or
  `OpenAiCompatibleClient` for supported APIs and rejects test-only/local
  providers in production resolution.

## Stream Contract

Provider adapters should translate their native streams into:

- `MessageStart`
- `TextDelta`
- `ToolCallStart`
- `ToolCallArgsDelta`
- `ToolCallEnd`
- `MessageEnd`
- `Error`

The runtime should not parse provider-native chunks. It should consume these normalized events only.

## Credential Hints

`neo_ai::find_env_keys(provider)` and `neo_ai::env_api_key(provider)` currently
know a small environment-key map for common provider ids such as `anthropic`,
`openai`, `openai-codex`, `github-copilot`, `google`, `google-vertex`,
`mistral`, `openrouter`, and `amazon-bedrock`.

Neo does not have pi-style auth-file login flows yet. Keep provider secrets in
environment variables or external secret managers. Anthropic can use
`ANTHROPIC_OAUTH_TOKEN` or `ANTHROPIC_API_KEY`; OpenAI uses `OPENAI_API_KEY`;
Google Generative AI uses `GEMINI_API_KEY` or `GOOGLE_API_KEY`.
`ProviderCredentialStatus` reports configured environment variable names,
ambient auth labels, and explicit missing-key reasons without including secret
values. If a provider needs a browser or external backend for login, Neo should
surface that as unsupported or environment-only rather than pretending a login
succeeded.

## Production Adapters

`neo-ai` includes production network clients for:

- `OpenAiResponsesClient` for OpenAI Responses models such as `openai/gpt-4.1`.
- `AnthropicMessagesClient` for Anthropic Messages models such as
  `anthropic/claude-sonnet-4-5`.
- `GoogleGenerativeAiClient` for Google Generative AI models such as
  `google/gemini-2.5-pro`.
- `OpenAiCompatibleClient` for OpenAI-compatible Chat Completions providers
  such as OpenRouter.

The production resolver requires a registered provider, a supported API kind,
credentials from the provider's environment-key list, and a base URL. Built-in
provider base URLs and credential environment names can be overridden from
`neo-agent` project config with `providers.<provider-id>.api_base` and
`providers.<provider-id>.api_key_env`. It does not resolve `ApiKind::Local` or
the fake test provider.

`ContentPart::Image` is serialized for provider chat requests instead of being
silently dropped. OpenAI Responses and OpenAI-compatible adapters send image
URL parts or base64 data URLs in user messages. Anthropic Messages sends
base64 image sources in user messages and rejects image URLs before issuing a
request. Google Generative AI sends base64 images as `inlineData` and rejects
image URLs before issuing a request. This is chat image-input support only; Neo
does not implement image generation.

## Local Model Catalogs

Neo supports strict local JSON model catalogs. They extend or replace
`ModelRegistry::seeded()` entries by `provider` and `model`.

The native Neo shape stores the exact `ModelSpec` fields:

```json
{
  "models": [
    {
      "provider": "openrouter",
      "model": "anthropic/claude-sonnet-4.5",
      "api": "OpenAiCompatible",
      "capabilities": {
        "streaming": true,
        "tools": true,
        "images": false,
        "reasoning": true,
        "embeddings": false,
        "max_context_tokens": 200000
      }
    }
  ],
  "default": {
    "provider": "openrouter",
    "model": "anthropic/claude-sonnet-4.5"
  }
}
```

Catalog loading fails on missing files, invalid JSON, empty model lists, empty
provider/model strings, zero `max_context_tokens`, or a `default` that does not
match a registered model. `neo-agent` project config can reference catalog files
with `model_catalogs = [".neo/models.json"]`; relative paths resolve from the
project root.

Neo also accepts the custom-model subset of Pi `models.json`, detected by a
top-level `providers` object:

```json
{
  "providers": {
    "ollama": {
      "api": "openai-completions",
      "models": [
        {
          "id": "llama3.1:8b",
          "input": ["text"],
          "contextWindow": 128000
        }
      ]
    }
  }
}
```

Supported Pi API names are `openai-responses`, `openai-completions`,
`openai-compatible`, `anthropic-messages`, `google-generative-ai`, and `local`.
The loader maps Pi `id`, provider map key, `api`, `reasoning`, `input`, and
`contextWindow` into `ModelSpec`. It rejects unsupported Pi APIs such as
`bedrock-converse-stream` instead of silently downgrading them.

Pi provider endpoint, credential, pricing, `maxTokens`, `headers`, `compat`,
and `modelOverrides` metadata are not migrated into `ModelSpec`. Configure Neo
provider credentials and base URLs through Neo provider config such as
`providers.<provider-id>.api_base` and `providers.<provider-id>.api_key_env`;
Pi catalog import still does not automatically migrate provider metadata into
that config.

## Test Provider

`neo_ai::providers::fake::FakeModelClient` is available for tests. It stores
incoming `ChatRequest` values and replays a configured list of `AiStreamEvent`
values. `neo_agent_core::FakeHarness` wraps the same idea for runtime tests.

## Adding a Provider

1. Add a provider module under `crates/ai/src/providers`.
2. Implement `ModelClient` for the adapter.
3. Register production provider metadata in `ProviderRegistry` when the adapter
   is ready for production resolution.
4. Normalize provider-specific errors into `AiError`.
5. Emit `ToolCallArgsDelta` fragments and a final `ToolCallEnd` with parsed arguments.
6. Add tests using representative native payloads.

Do not expose provider-native request or response types to `neo-agent-core`.

## Example

See [examples/rust/provider_registry.rs](../examples/rust/provider_registry.rs)
for a small registry and `RequestOptions` snippet, and
[examples/rust/model_catalog.rs](../examples/rust/model_catalog.rs) for loading
a local JSON catalog.
