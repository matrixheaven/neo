# neo-agent-core Gap Map

## Implemented Surface

- `AgentConfig::for_model` builds a runtime config with optional system prompt,
  max turns, temperature, max tokens, and tool specs.
- `AgentContext` stores messages, turn count, and cancellation state.
- `AgentRuntime` consumes a `ModelClient`, converts context into
  `neo_ai::ChatRequest`, emits `AgentEvent` values, appends assistant/tool
  messages, and loops after `StopReason::ToolUse` when tools are registered.
- Before calling a provider, `AgentRuntime` validates the selected
  `ModelCapabilities` against the pending request and fails closed for image
  input, tool schemas, or reasoning effort that the model does not advertise.
- Runtime lifecycle events now include `RunStarted`, `MessageStarted`,
  `MessageFinished`, `TurnStarted`, `TurnFinished`, and `RunFinished`, so
  consumers can distinguish streamed provider message boundaries from whole-run
  completion without inferring from transcript records. Fast terminal paths
  such as max-turns exhaustion and already-cancelled contexts still emit
  `RunStarted` and `RunFinished` around their terminal `TurnFinished` barrier
  without calling the model.
- Runtime model-stream events also include `ThinkingStarted`,
  `ThinkingDelta`, and `ThinkingFinished`, so provider-neutral reasoning
  summaries can be persisted as assistant thinking content without mixing into
  ordinary assistant text.
- `AgentRuntime::run_turn_with_cancel` accepts a `CancellationToken` and can
  stop an in-flight model stream promptly, emitting cancelled message, turn,
  and run barriers while updating replayable runtime cancellation state.
- The same runtime cancellation token now races in-flight tool preparation and
  tool execution futures. When cancellation fires during a tool batch, Neo
  emits a cancelled `ToolExecutionFinished`, finishes the turn and run with
  `StopReason::Cancelled`, marks the context cancelled, and does not append the
  cancelled tool result into model/session context.
- Runtime queue modes and hooks are real Rust APIs: `AgentContext` can queue
  steering and follow-up messages, `AgentConfig::with_queue_modes` controls
  drain behavior, synchronous `with_before_tool_call` /
  `with_after_tool_call` can block, terminate, or rewrite tool results, and
  async `with_async_before_tool_call` / `with_async_after_tool_call` hooks race
  the runtime cancellation token so interruption during policy or mediation
  work finishes the tool wrapper with a cancelled result without appending it
  into model context.
- `ToolExecutionMode` supports sequential and parallel tool execution; parallel
  mode emits completion events as tools finish while preserving appended tool
  result messages in source order.
- Tool execution errors that the model can recover from, including invalid
  inputs and unknown tool names, are returned as `ToolResult::error` messages
  with `is_error = true` so the next model turn can retry instead of aborting
  the run.
- `FakeHarness` supplies a fake model and recorded request inspection for tests.
- `PermissionPolicy` supports `Allow`, `Ask`, and `Deny` decisions for file
  reads, file writes, shell execution, and general tools. `Ask` emits
  `AgentEvent::ApprovalRequested` and executes only when the configured
  synchronous or async approval handler returns `Allow`; the async handler path
  waits on the returned future before executing or denying the tool.
- `neo-agent` live interactive mode wires those async approval handlers to the
  `neo-tui` approval overlay, so explicit user choices resume pending tool
  calls using user-provided allow/deny decisions.
- `ToolRegistry::with_builtin_tools()` registers `read`, `list`, `grep`,
  `find`, `write`, `edit`, and `bash`; `edit` returns structured details with
  a stable unified diff for TUI/export consumers.
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
  `tools/call` without local fallback behavior. It also sends
  `resources/subscribe` and `resources/unsubscribe`; JSON subscribe responses
  are acknowledged, and live SSE subscribe responses are read in the background
  so real `notifications/resources/updated` messages are queued for watchers.
- Stdio and HTTP/SSE MCP adapters also support explicit `resources/list` and
  `resources/read` requests without injecting resource content into model
  context.
- `ToolContext` resolves paths inside the workspace and carries shell timeout
  output cap settings, and cancellation state. Foreground `bash` timeout or
  cancellation terminates the shell process group on Unix, and compact non-PTY
  background `bash` handles support real `start`, `poll`, and terminal `stop`
  operations backed by child processes and shell process-group cleanup.
- `session::JsonlSessionWriter`, `session::JsonlSessionReader`, and
  `session::replay_messages` persist and replay `AgentEvent::MessageAppended`
  history. JSONL schema metadata is validated when present: event-only legacy
  files still replay, current v1 metadata is skipped, and future metadata
  schema versions fail closed instead of being silently ignored.
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

- Add richer hosted or alternate-channel remote MCP lifecycle support once Neo
  has backing behavior for servers that do not deliver updates on the subscribe
  response SSE stream.
- Add richer hook lifecycle docs only when Neo exposes additional hook phases
  beyond the current synchronous and async before/after tool-call callbacks.
- Add broader process cleanup only when Neo grows a contract for commands that
  daemonize into a new session or process group. Current cancellation support
  covers runtime state, in-flight model streams, arbitrary in-flight tool
  futures at the runtime scheduling boundary, foreground/background bash shell
  process groups on Unix, and live TUI interruption that drains cooperative
  cancelled message/turn/run barriers before falling back to abort.
- Decide whether Neo needs full PTY/interactivity later. Current `bash`
  background support is intentionally compact start/poll/stop process
  management.
- Decide whether JSONL event persistence remains the durable session format or
  becomes a compatibility layer over a richer store with hosted shares and
  richer branch-level summary metadata.
