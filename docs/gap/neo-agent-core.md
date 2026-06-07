# neo-agent-core Gap Map

## Implemented Surface

- `AgentConfig::for_model` builds a runtime config with optional system prompt,
  max turns, temperature, max tokens, and tool specs.
- `AgentContext` stores messages, turn count, and cancellation state.
- `AgentRuntime` consumes a `ModelClient`, converts context into
  `neo_ai::ChatRequest`, emits `AgentEvent` values, appends assistant/tool
  messages, and loops after `StopReason::ToolUse` when tools are registered.
- `FakeHarness` supplies a fake model and recorded request inspection for tests.
- `PermissionPolicy` supports `Allow`, `Ask`, and `Deny` decisions for file
  reads, file writes, and shell execution. The current tool executor treats only
  `Allow` as executable.
- `ToolRegistry::with_builtin_tools()` registers `read`, `list`, `grep`,
  `find`, `write`, `edit`, and `bash`.
- `McpToolAdapter` and `McpToolProvider` can discover configured MCP tools as
  namespaced `ToolSpec` values and execute them through an async adapter
  registered in `ToolRegistry`.
- `ToolContext` resolves paths inside the workspace and carries shell timeout
  and output cap settings.
- `session::JsonlSessionWriter`, `session::JsonlSessionReader`, and
  `session::replay_messages` persist and replay `AgentEvent::MessageAppended`
  history.

## Pi Parity Pressure

`pi-agent-core` documents a richer lifecycle: agent start/end, message start/end
barriers, tool execution hooks, steering/follow-up queues, cancellation, and
parallel tool execution. Neo has the smaller Rust runtime core but not the full
interactive behavior.

## High-Priority Gaps

- Finish the `neo_ai::ChatRequest` options migration in runtime code before
  treating broad workspace checks as green.
- Add docs and tests for `Ask` permission behavior once there is a CLI/TUI
  approval path. Today tools only execute on `Allow`.
- Add a real stdio JSON-RPC MCP process adapter and CLI config plumbing on top
  of the agent-core adapter boundary.
- Add hook/steering docs only when the runtime exposes those APIs.
- Define whether `bash` remains foreground-only or grows a background/PTY
  sibling; keep the model-facing schema compact either way.
- Decide whether JSONL event persistence remains the durable session format or
  becomes a compatibility layer over a richer store.
