# neo-agent-core Gap Map

## Implemented Surface

- `AgentConfig::for_model` builds a runtime config with optional system prompt,
  max turns, temperature, max tokens, and tool specs.
- `AgentContext` stores messages, turn count, and cancellation state.
- `AgentRuntime` consumes a `ModelClient`, converts context into
  `neo_ai::ChatRequest`, emits `AgentEvent` values, appends assistant/tool
  messages, and loops after `StopReason::ToolUse` when tools are registered.
- Runtime lifecycle events now include `RunStarted`, `MessageStarted`,
  `MessageFinished`, `TurnStarted`, `TurnFinished`, and `RunFinished`, so
  consumers can distinguish streamed provider message boundaries from whole-run
  completion without inferring from transcript records.
- Runtime queue modes and hooks are real Rust APIs: `AgentContext` can queue
  steering and follow-up messages, `AgentConfig::with_queue_modes` controls
  drain behavior, and `with_before_tool_call` / `with_after_tool_call` can
  block, terminate, or rewrite tool results.
- `ToolExecutionMode` supports sequential and parallel tool execution; parallel
  mode emits completion events as tools finish while preserving appended tool
  result messages in source order.
- `FakeHarness` supplies a fake model and recorded request inspection for tests.
- `PermissionPolicy` supports `Allow`, `Ask`, and `Deny` decisions for file
  reads, file writes, shell execution, and general tools. `Ask` emits
  `AgentEvent::ApprovalRequested` and executes only when the configured
  synchronous or async approval handler returns `Allow`; the async handler path
  waits on the returned future before executing or denying the tool.
- `ToolRegistry::with_builtin_tools()` registers `read`, `list`, `grep`,
  `find`, `write`, `edit`, and `bash`.
- `McpToolAdapter` and `McpToolProvider` can discover configured MCP tools as
  namespaced `ToolSpec` values and execute them through an async adapter
  registered in `ToolRegistry`.
- `McpStdioToolAdapter` starts configured stdio MCP commands and speaks
  JSON-RPC for `initialize`, `tools/list`, `tools/call`, `resources/list`,
  `resources/read`, `resources/subscribe`, and `resources/unsubscribe`,
  reusing the initialized stdio session across discovery, tool calls, and
  resource operations without local fallback behavior. It also queues real
  `notifications/resources/updated` messages from the stdio server.
- `McpHttpToolAdapter` sends JSON-RPC POST requests to configured HTTP/SSE MCP
  endpoints, applies configured headers, accepts JSON and SSE `data:`
  JSON-RPC responses, and supports `initialize`, `tools/list`, and
  `tools/call` without local fallback behavior. Resource update subscriptions
  are explicitly unsupported on the one-shot remote transport.
- Stdio and HTTP/SSE MCP adapters also support explicit `resources/list` and
  `resources/read` requests without injecting resource content into model
  context.
- `ToolContext` resolves paths inside the workspace and carries shell timeout
  and output cap settings; `bash` also supports compact non-PTY background
  start/poll handles backed by real child processes.
- `session::JsonlSessionWriter`, `session::JsonlSessionReader`, and
  `session::replay_messages` persist and replay `AgentEvent::MessageAppended`
  history.
- `session::compact_jsonl_session` appends real
  `AgentEvent::CompactionApplied` records using deterministic extractive
  transcript summarization, and `replay_context` applies those records so active
  context matches the compacted JSONL history.

## Pi Parity Pressure

`pi-agent-core` documents a richer lifecycle: agent start/end, message start/end
barriers, hook phases, steering/follow-up queues, cancellation, and parallel
tool execution. Neo has the smaller Rust runtime core and exposes local
lifecycle barrier, hook/queue/parallel primitives, but not the full interactive
or hosted lifecycle behavior.

## High-Priority Gaps

- Add richer interactive approval UI wiring once the raw terminal event loop can
  resume pending tool calls from explicit user choices instead of relying on
  runtime-only approval handler callbacks.
- Add remote HTTP/SSE MCP resource update streams once Neo has a long-lived
  remote MCP transport.
- Add richer hook lifecycle docs only when Neo exposes additional hook phases
  beyond the current before/after tool-call callbacks.
- Decide whether Neo needs full PTY/interactivity later. Current `bash`
  background support is intentionally compact start/poll process management.
- Decide whether JSONL event persistence remains the durable session format or
  becomes a compatibility layer over a richer store with schema versions,
  hosted shares, and branch-level summary metadata.
