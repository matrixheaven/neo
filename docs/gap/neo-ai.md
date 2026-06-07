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
- `schema_for<T>()` and `ToolSpec::from_schema<T>()` generate JSON Schema from
  Rust input types.
- `providers::fake::FakeModelClient` supports tests.
- `find_env_keys` and `env_api_key` cover a small provider environment-key map.

## Pi Parity Pressure

`pi-ai` documents production provider catalogs, OAuth/API-key resolution,
reasoning controls, image generation, cross-provider handoffs, cost accounting,
browser notes, and context serialization. Neo should not copy that surface until
the Rust adapters exist.

## High-Priority Gaps

- Add real provider adapter docs only after modules under `crates/ai/src/providers`
  implement network requests.
- Add model catalog docs only after `ModelRegistry` has a loader or generated
  source of truth.
- Keep provider credentials environment-only for now; auth-file and OAuth
  flows are future work.
- Keep request metadata internal-facing. Do not expose provider-native chunk
  formats to `neo-agent-core`.

## Current Drift To Watch

`ChatRequest` uses `options: RequestOptions`. Any docs or examples using direct
`temperature` or `max_tokens` fields are stale.
