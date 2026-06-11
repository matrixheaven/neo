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
  local JSON catalog loading for existing `ModelSpec` shapes, generated catalog
  entries with structured pricing and image-generation capability metadata, and
  a production-backed custom-model subset of Pi `models.json` that maps
  supported Pi API names and capability fields into Neo `ModelSpec` values
  while rejecting request-affecting Pi metadata that Neo cannot safely migrate
  yet.
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
- `ImageGenerationClient` defines the provider-neutral image-generation
  boundary, and `OpenAiImagesClient` performs real OpenAI-style
  `/images/generations` HTTP requests, returning base64 or URL image data to
  callers without synthesizing image bytes.
- `CredentialResolver` resolves provider credentials in explicit precedence
  order from CLI key, environment variables, and local auth-file credentials,
  while `ResolvedCredential` redacts secrets from debug output.
- `schema_for<T>()` and `ToolSpec::from_schema<T>()` generate JSON Schema from
  Rust input types.
- `providers::fake::FakeModelClient` supports tests.
- `find_env_keys` and `env_api_key` cover a small provider environment-key map.

## Pi Parity Pressure

`pi-ai` documents broader provider catalogs, OAuth/API-key resolution,
provider-native reasoning controls, provider-specific image generation breadth,
cross-provider handoffs, cost accounting, browser notes, and context
serialization. Neo should not copy unsupported surface area until the Rust
contracts exist.

## High-Priority Gaps

- Add docs for new provider APIs only after modules under
  `crates/ai/src/providers` implement network requests and production resolver
  support.
- Generated catalog pricing and image-generation capability fields are
  implemented as local registry metadata. Pi `models.json` pricing metadata,
  upstream generated catalog production, request-affecting provider-metadata
  migration, and provider-native model override formats remain future work. Neo
  provider-specific base URLs and API key env names are available through
  `neo-agent` config. Pi catalog import preserves provider/model `name` as
  display-only metadata, but rejects request-affecting provider/model metadata
  until those fields have explicit Neo runtime contracts.
- CLI/env/local auth-file credential precedence is implemented. Managed OAuth login, profile sync, and hosted account credential sync are out of scope for the local-only surface.
- Provider-native reasoning streams are normalized for OpenAI Responses,
  Anthropic Messages, and Google Generative AI, including opaque signature
  passthrough when providers send one. Signed thinking blocks can be explicitly
  replayed through OpenAI Responses reasoning input items, Anthropic native
  thinking/redacted-thinking blocks, and Google `thoughtSignature` parts.
  Adaptive-thinking controls, display/off-state handling, and model-aware
  cross-provider thinking conversion remain future work.
- Keep request metadata internal-facing. Do not expose provider-native chunk
  formats to `neo-agent-core`.

## Current Drift To Watch

`ChatRequest` uses `options: RequestOptions`. Any docs or examples using direct
`temperature` or `max_tokens` fields are stale.
