# neo-ai Gap Map

## Implemented Surface

- `ProviderId`, `ApiKind`, `ModelCapabilities`, and `ModelSpec` define the
  provider/model identity contract.
- `ChatMessage`, `ContentPart`, `ToolCall`, `ToolSpec`, and `ChatRequest`
  define the model-facing request shape.
- `RequestOptions` carries temperature, max tokens, typed reasoning effort,
  headers, timeout, retries, cache retention, session id, and metadata.
- `AiStreamEvent` normalizes provider streams into message, OpenAI Responses
  reasoning-summary thinking, text, tool-call, completion, and error events.
- `ModelClient::stream_chat` is the provider adapter trait.
- `ModelRegistry` stores configured models, a first-registered default, strict
  local JSON catalog loading for existing `ModelSpec` shapes, and a
  production-backed custom-model subset of Pi `models.json` that maps supported
  Pi API names and capability fields into Neo `ModelSpec` values while
  rejecting request-affecting Pi metadata that Neo cannot safely migrate yet.
- `ProviderRegistry::production()` stores built-in production provider metadata.
- `ProviderResolver` resolves registered models to production clients when
  the provider supports the model API kind and credentials/base URLs are
  available. Provider/API mismatches fail before credential lookup.
- `OpenAiResponsesClient`, `AnthropicMessagesClient`,
  `GoogleGenerativeAiClient`, and `OpenAiCompatibleClient` implement network
  provider adapters, including native chat image-input serialization for
  supported user-message image forms and explicit preflight rejection for
  unsupported image URL formats. OpenAI Responses and OpenAI-compatible Chat
  Completions also serialize typed `reasoning_effort` options into their
  provider-native payload shapes. Anthropic Messages and Google Generative AI
  serialize the same typed effort into explicit provider-native budget-based
  thinking request payloads with local adapter tests. OpenAI Responses maps
  streamed reasoning-summary SSE events, Anthropic Messages maps
  extended-thinking SSE chunks, and Google Generative AI maps streamed
  `thought` parts into provider-neutral thinking start/delta/end events.
- `schema_for<T>()` and `ToolSpec::from_schema<T>()` generate JSON Schema from
  Rust input types.
- `providers::fake::FakeModelClient` supports tests.
- `find_env_keys` and `env_api_key` cover a small provider environment-key map.

## Pi Parity Pressure

`pi-ai` documents broader provider catalogs, OAuth/API-key resolution,
provider-native reasoning controls, image generation, cross-provider handoffs,
cost accounting, browser notes, and context serialization. Neo should not copy
unsupported surface area until the Rust contracts exist.

## High-Priority Gaps

- Add docs for new provider APIs only after modules under
  `crates/ai/src/providers` implement network requests and production resolver
  support.
- Pi `models.json` pricing metadata, generated catalog sources,
  provider-metadata migration, and provider-native model override formats
  remain future work. Neo provider-specific base URLs and API key env names are
  available through `neo-agent` config. Pi catalog import rejects
  request-affecting provider/model metadata until those fields have explicit
  Neo runtime contracts.
- Keep provider credentials environment-only for now; auth-file and OAuth
  flows are future work.
- Provider-native reasoning streams are normalized for OpenAI Responses,
  Anthropic Messages, and Google Generative AI, including opaque signature
  passthrough when providers send one. Adaptive-thinking controls,
  display/off-state handling, encrypted reasoning handoff, and signature replay
  remain future work.
- Keep request metadata internal-facing. Do not expose provider-native chunk
  formats to `neo-agent-core`.

## Current Drift To Watch

`ChatRequest` uses `options: RequestOptions`. Any docs or examples using direct
`temperature` or `max_tokens` fields are stale.
