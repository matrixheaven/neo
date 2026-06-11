# Architecture

Neo is organized as a local agent around a narrow model/provider layer and an
agent-core runtime that can be tested without a terminal UI.

## Crate Boundaries

```text
neo-agent CLI/TUI
  -> neo-agent-core runtime, sessions, permissions, tools, MCP
      -> neo-ai provider-neutral model and stream contracts
  -> neo-tui terminal UI primitives
xtask maintenance commands
```

## Implemented Today

- `neo-ai` defines provider-neutral request, message, model, capability, tool, and stream event types.
- `neo-ai` defines request options, environment key helpers, model/provider registries, and production provider resolution.
- `neo-ai` includes OpenAI Responses, Anthropic Messages, Google Generative AI,
  OpenAI-compatible, and OpenAI-style image generation network clients.
- `neo-ai::providers::fake::FakeModelClient` records requests and replays stream events for tests.
- `neo-agent-core` contains a runtime turn loop, fake harness, permissions,
  built-in tools, MCP adapters, reasoning event persistence, and JSONL session
  helpers.
- `neo-agent` exposes the local command-line and TUI surface.
- `neo-tui` owns terminal rendering, transcript selection, and conservative
  inline image protocol rendering.
- `xtask check` verifies the stable developer tooling slice, and
  `xtask release-smoke` exercises local-only CLI surfaces.

## Intended Runtime Flow

1. `neo-agent` parses CLI arguments and loads configuration.
2. `neo-agent-core` opens or creates a session.
3. The runtime resolves a model provider from config and the production provider registry.
4. The agent loop sends a `neo_ai::ChatRequest` to a `ModelClient`.
5. Stream events are normalized as `AiStreamEvent` values.
6. Tool calls are authorized, executed, and returned as `ChatMessage::ToolResult`.
7. Reasoning events are preserved as thinking content instead of being mixed
   into plain assistant text.
8. Session events are persisted so `resume` can rebuild conversation and tool
   state from local JSONL history.

The current Rust surface implements parts of this flow. See [Gap Map](gap/INDEX.md)
for the module-by-module parity status.

## Design Principles

- Keep provider-specific code behind `ModelClient`.
- Keep model-facing tool schemas small and stable.
- Treat permissions and session persistence as runtime policy, not provider behavior.
- Prefer typed Rust interfaces first; add wire protocols such as MCP at the boundary.
- Keep hosted/cloud distribution, profile sync, and managed collaboration out
  of the supported local-agent surface until the product deliberately reopens
  those boundaries.
