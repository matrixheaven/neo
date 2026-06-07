# Architecture

Neo is organized around a narrow model/provider layer and an agent-core runtime that can be tested without a terminal UI.

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
- `neo-ai` defines request options, environment key helpers, and a small model registry.
- `neo-ai::providers::fake::FakeModelClient` records requests and replays stream events for tests.
- `neo-agent-core` contains a runtime turn loop, fake harness, permissions, built-in tools, and JSONL session helpers.
- `neo-agent` exposes the initial command-line surface.
- `xtask check` verifies the stable developer tooling slice and can validate docs links.

## Intended Runtime Flow

1. `neo-agent` parses CLI arguments and loads configuration.
2. `neo-agent-core` opens or creates a session.
3. The runtime resolves a model provider from config and task needs.
4. The agent loop sends a `neo_ai::ChatRequest` to a `ModelClient`.
5. Stream events are normalized as `AiStreamEvent` values.
6. Tool calls are authorized, executed, and returned as `ChatMessage::ToolResult`.
7. Session events are persisted so `resume` can rebuild conversation and tool state.

The current Rust surface implements parts of this flow. See [Gap Map](gap/INDEX.md)
for the module-by-module parity status.

## Design Principles

- Keep provider-specific code behind `ModelClient`.
- Keep model-facing tool schemas small and stable.
- Treat permissions and session persistence as runtime policy, not provider behavior.
- Prefer typed Rust interfaces first; add wire protocols such as MCP at the boundary.
- Document planned interfaces before wiring them to unstable internals.
