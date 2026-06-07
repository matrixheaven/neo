# neo-ai Gap Map

## Implemented Surface

- `ProviderId`, `ApiKind`, `ModelCapabilities`, and `ModelSpec` define the
  provider/model identity contract.
- `ChatMessage`, `ContentPart`, `ToolCall`, `ToolSpec`, and `ChatRequest`
  define the model-facing request shape.
- `RequestOptions` carries temperature, max tokens, headers, timeout, retries,
  cache retention, session id, and metadata.
- `AiStreamEvent` normalizes provider streams into message, text, tool-call,
  completion, and error events.
- `ModelClient::stream_chat` is the provider adapter trait.
- `ModelRegistry` stores configured models and a first-registered default.
- `ProviderRegistry::production()` stores built-in production provider metadata.
- `ProviderResolver` resolves registered models to production clients when
  credentials and base URLs are available.
- `OpenAiResponsesClient`, `AnthropicMessagesClient`, and
  `OpenAiCompatibleClient` implement network provider adapters.
- `schema_for<T>()` and `ToolSpec::from_schema<T>()` generate JSON Schema from
  Rust input types.
- `providers::fake::FakeModelClient` supports tests.
- `find_env_keys` and `env_api_key` cover a small provider environment-key map.

## Pi Parity Pressure

`pi-ai` documents broader provider catalogs, OAuth/API-key resolution, reasoning
controls, image generation, cross-provider handoffs, cost accounting, browser
notes, and context serialization. Neo should not copy unsupported surface area
until the Rust contracts exist.

## High-Priority Gaps

- Add docs for provider APIs only after modules under `crates/ai/src/providers`
  implement network requests and production resolver support.
- Add external model catalog docs only after `ModelRegistry` has a loader or
  generated source of truth.
- Keep provider credentials environment-only for now; auth-file and OAuth
  flows are future work.
- Keep request metadata internal-facing. Do not expose provider-native chunk
  formats to `neo-agent-core`.

## Current Drift To Watch

`ChatRequest` uses `options: RequestOptions`. Any docs or examples using direct
`temperature` or `max_tokens` fields are stale.
