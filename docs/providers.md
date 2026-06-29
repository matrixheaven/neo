# Providers

Provider integration starts in `neo-ai`. The crate defines the shared model
contract, request options, model/provider registries, production resolver, and
test client.

## Core Types

- `ProviderId` identifies a configured provider.
- `ApiKind` names the protocol family, including OpenAI Responses, Chat Completions, Anthropic Messages, Google Generative AI, OpenAI-compatible APIs, and local providers.
- `ModelCapabilities` tells the runtime whether a model supports streaming, tools, images, reasoning, embeddings, and an optional context limit.
- `ModelSpec` combines provider, model name, API kind, and capabilities.
- `RequestOptions` carries temperature, max tokens, reasoning effort, custom
  headers, timeout, retry count, cache retention, provider session id, and
  metadata.
- `ChatRequest` combines `model`, `messages`, `tools`, and `options`.
- `ModelClient::stream_chat` accepts a `ChatRequest` and returns normalized
  `AiStreamEvent` values.
- `ModelRegistry` stores available `ModelSpec` values and exposes the first
  registered model as the default unless replaced by configuration.
- `ProviderRegistry::production()` registers the provider catalog used by
  production resolution: OpenAI, Anthropic, Google Generative AI, OpenRouter,
  and Amazon Bedrock credential hints.
- `ProviderResolver` turns a registered `ModelSpec` plus environment credentials
  into a `ModelClient`. It first verifies that the selected provider supports
  the model's `ApiKind`, then constructs `OpenAiResponsesClient`,
  `AnthropicMessagesClient`, `GoogleGenerativeAiClient`, or
  `OpenAiCompatibleClient` for supported APIs and rejects test-only/local
  providers in production resolution.

## Stream Contract

Provider adapters should translate their native streams into:

- `MessageStart`
- `ThinkingStart`
- `ThinkingDelta`
- `ThinkingEnd`
- `TextDelta`
- `ToolCallStart`
- `ToolCallArgsDelta`
- `ToolCallEnd`
- `MessageEnd`
- `Error`

The runtime should not parse provider-native chunks. It should consume these normalized events only.

## Error Types

Neo uses a typed error enum (`AiError`) with 8 variants, each carrying a stable
`code()` string in `domain.reason` format for serialization:

| Variant | Code | Retryable | Description |
|---------|------|-----------|-------------|
| `Configuration { message }` | `config.invalid` | No | Provider/model config error |
| `RateLimit { message, retry_after }` | `provider.rate_limit` | Yes | HTTP 429; carries optional `Retry-After` duration |
| `Auth { message }` | `provider.auth_error` | No | HTTP 401/403 |
| `ContextOverflow { message }` | `provider.context_overflow` | No | Request too large for context window |
| `Server { status, message }` | `provider.server_error` | Yes | HTTP 5xx |
| `Stream { message }` | `provider.stream_error` | No | SSE parse/stream error |
| `Network { message }` | `provider.network_error` | Yes | Transport/network error |
| `Cancelled` | `request.cancelled` | No | Request cancelled by user |

Provider adapters should map HTTP status codes to the appropriate variant:
- 429 → `RateLimit` (parse `Retry-After` header)
- 401/403 → `Auth`
- 5xx → `Server`
- 400/413/422 + context-overflow message → `ContextOverflow`
- Transport failure → `Network`

The shared `ProviderError::HttpStatus` carries a `retry_after: Option<Duration>`
field, parsed from the `Retry-After` response header (both integer-seconds and
HTTP-date formats).

## Retry Behavior

Provider requests are retried automatically with exponential backoff:

- **Base delay:** 300ms, growing by factor 2 each attempt
- **Cap:** 5 seconds
- **Jitter:** ±25% random jitter on each delay
- **Retry-After:** If the provider sends a `Retry-After` header, that value
  takes precedence (capped at 5s)
- **Default attempts:** 3 (configurable via `RequestOptions.retries`)
- **Cancellable:** Backoff sleep aborts immediately on user cancellation

Retryable error types: `RateLimit`, `Server`, `Network`. All other errors
propagate immediately without retry.

## Context Overflow Recovery

When a provider returns `ContextOverflow`, the runtime:
1. Records the observed overflow point (estimated tokens × 0.85) as the new
   effective context limit for that model
2. Triggers forced multi-round compaction
3. Rebuilds the request with the compacted context
4. Retries the model call once

## Credential Hints

`neo_ai::find_env_keys_from(provider, &env)` and
`neo_ai::env_api_key_from(provider, &env)` currently know a small environment-key
map for common provider ids such as `anthropic`, `openai`, `openai-codex`,
`github-copilot`, `google`, `google-vertex`, `mistral`, `openrouter`, and
`amazon-bedrock`.

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

The production resolver requires a registered provider with an explicit
`type`, credentials from the provider's environment-key list, and a base URL.
Built-in provider base URLs and credential environment names can be overridden
from Neo config with `providers.<provider-id>.base_url` and
`providers.<provider-id>.api_key_env`. The provider `type` selects the wire
client. OpenAI supports both Responses and Chat Completions models; OpenRouter
supports OpenAI-compatible and Chat Completions models. Anthropic, Google, and
Amazon Bedrock are restricted to their registered protocol families. Bedrock is
still credential metadata only until a production adapter/base URL contract
exists. The resolver does not resolve `ApiKind::Local` or the fake test
provider.

`ContentPart::Image` is serialized for provider chat requests instead of being
silently dropped. OpenAI Responses and OpenAI-compatible adapters send image
URL parts or base64 data URLs in user messages. Anthropic Messages sends
base64 image sources in user messages and rejects image URLs before issuing a
request. Google Generative AI sends base64 images as `inlineData` and rejects
image URLs before issuing a request.

`ImageGenerationClient` and `OpenAiImagesClient` support OpenAI-style
`/images/generations` requests. `neo images generate` writes base64 provider
image data directly to a workspace-contained output path. If a provider returns
only a remote image URL, Neo refuses to fetch it unless
`tui.fetch_remote_images = true` is set in config. Remote image fetches must use
HTTP(S), return an image content type, and stay under the remote image size
limit.

OpenAI reasoning controls use the typed `RequestOptions::reasoning_effort`
field. OpenAI Responses sends `reasoning: { effort, summary: "auto" }` and
requests `reasoning.encrypted_content` so streamed reasoning items can be
persisted as signed `ContentPart::Thinking` blocks and replayed as provider
native Responses reasoning input items. OpenAI-compatible Chat Completions
sends the flat `reasoning_effort` string and replays thinking blocks as plain
assistant text. Anthropic Messages maps the same typed effort into an explicit
budget-based `thinking` payload with summarized display, omits `temperature`
while extended thinking is enabled, and replays signed or redacted thinking
blocks using Anthropic's native content block shapes. Google Generative AI maps
reasoning effort into `generationConfig.thinkingConfig` with `includeThoughts`
and a deterministic thinking budget, and replays signed thinking as
`thought`/`thoughtSignature` parts. Neo still does not implement adaptive or
off-state thinking controls, model-aware cross-provider thinking conversion, or
provider-specific non-OpenAI-compatible thinking stream mapping; those
contracts remain future work until they can be represented without silently
changing model behavior.

`AgentRuntime` checks `ModelCapabilities` before issuing a provider request.
Requests that include image content, tool schemas, or reasoning effort fail
locally when the selected model does not advertise the matching capability, so
unsupported combinations do not become provider-specific transport failures.

## Test Provider

`neo_ai::providers::fake::FakeModelClient` is available for tests. It stores
incoming `ChatRequest` values and replays a configured list of `AiStreamEvent`
values. `neo_agent_core::FakeHarness` wraps the same idea for runtime tests.

## Adding a Provider

1. Add a provider module under `crates/neo-ai/src/providers`.
2. Implement `ModelClient` for the adapter.
3. Register production provider metadata in `ProviderRegistry` when the adapter
   is ready for production resolution.
4. Normalize provider-specific errors into `AiError`.
5. Emit `ToolCallArgsDelta` fragments and a final `ToolCallEnd` with parsed arguments.
6. Add tests using representative native payloads.

Do not expose provider-native request or response types to `neo-agent-core`.

## Example

See [examples/rust/provider_registry.rs](../examples/rust/provider_registry.rs)
for a small registry and `RequestOptions` snippet.
