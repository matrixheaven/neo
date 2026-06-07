# Providers

Provider integration starts in `neo-ai`. The crate defines the shared model contract and leaves network adapters to provider modules.

## Core Types

- `ProviderId` identifies a configured provider.
- `ApiKind` names the protocol family, including OpenAI Responses, Chat Completions, Anthropic Messages, Google Generative AI, OpenAI-compatible APIs, and local providers.
- `ModelCapabilities` tells the runtime whether a model supports streaming, tools, images, reasoning, embeddings, and an optional context limit.
- `ModelSpec` combines provider, model name, API kind, and capabilities.
- `ModelClient::stream_chat` accepts a `ChatRequest` and returns normalized `AiStreamEvent` values.

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

## Fake Provider

`neo_ai::providers::fake::FakeModelClient` is available for tests. It stores incoming `ChatRequest` values and replays a configured list of `AiStreamEvent` values.

## Adding a Provider

1. Add a provider module under `crates/ai/src/providers`.
2. Implement `ModelClient` for the adapter.
3. Normalize provider-specific errors into `AiError`.
4. Emit `ToolCallArgsDelta` fragments and a final `ToolCallEnd` with parsed arguments.
5. Add tests using representative native payloads.

Do not expose provider-native request or response types to `neo-agent-core`.
