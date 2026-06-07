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
- `ProviderRegistry::production()` registers the provider catalog used by
  production resolution: OpenAI, Anthropic, OpenRouter, and Amazon Bedrock
  credential hints.
- `ProviderResolver` turns a registered `ModelSpec` plus environment credentials
  into a `ModelClient`. It constructs `OpenAiResponsesClient`,
  `AnthropicMessagesClient`, or `OpenAiCompatibleClient` for supported APIs and
  rejects test-only/local providers in production resolution.

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
`ANTHROPIC_OAUTH_TOKEN` or `ANTHROPIC_API_KEY`; OpenAI uses `OPENAI_API_KEY`.
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
- `OpenAiCompatibleClient` for OpenAI-compatible Chat Completions providers
  such as OpenRouter.

The production resolver requires a registered provider, a supported API kind,
credentials from the provider's environment-key list, and a base URL. It does
not resolve `ApiKind::Local` or the fake test provider.

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
for a small registry and `RequestOptions` snippet.
