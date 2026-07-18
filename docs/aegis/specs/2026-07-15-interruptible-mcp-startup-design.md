# Interruptible MCP Startup

## Problem

Neo starts MCP connections in background tasks, but `run_tty_lifecycle_with_event_factory`
awaits `connect_mcp_at_startup` before entering the terminal input loop. A slow or
reconnecting MCP server therefore leaves the startup status animating while keyboard input,
including Esc, is never polled. New sessions and resumed sessions share this same blocked path.

## Approaches Considered

1. Keep the pre-loop wait and add a second input loop inside it. This duplicates input routing,
   queue behavior, rendering cadence, and interrupt precedence.
2. Move MCP startup tracking into the existing terminal loop. This reuses the manager's current
   connection tasks and Neo's single input owner. This is the selected approach.
3. Add a new MCP event channel and lifecycle subsystem. This matches Codex's distributed
   app-server architecture but adds machinery Neo's local snapshot-based manager does not need.

## Design

`connect_mcp_at_startup` becomes a non-blocking initializer: it inserts the existing connecting
rows, applies MCP configuration to start the manager's current background tasks, marks MCP
startup active, and returns. The normal terminal loop starts immediately.

The controller owns one explicit MCP-startup-active state. Each terminal-loop tick polls the
manager snapshots, updates the existing transcript rows, and clears the state when all enabled
servers leave `Pending` and `Reconnecting`. The existing manager and transcript status enums gain
one `Cancelled` terminal state; no new connection task, event protocol, or duplicate status model
is introduced.

The composer remains active during MCP startup. Enter follows the existing submit path and may
start a turn. The turn runtime already waits for the shared manager to settle before registering
connected MCP tools, so no model request races ahead with a partial tool registry.

## Interrupt Semantics

Esc keeps Neo's existing priority order:

- If an agent turn or shell command is active, interrupt it first and leave MCP startup running.
- Otherwise, if MCP startup is active, ask the manager to retire only `Pending` and
  `Reconnecting` tasks, mark those entries `Cancelled`, retain connected servers, clear the MCP
  startup state, and keep the session open.
- Otherwise, preserve the current idle Esc behavior.

This applies equally to a fresh interactive session and command-line resume because both use
`run_tty_lifecycle_with_event_factory`.

## Error Handling

Normal connection failures continue through the manager's existing diagnostics and transcript
status mapping. Explicit interruption maps to a neutral interrupted transcript row rather than a
connection failure. Late task results must not revive startup state after interruption; the
manager's existing task retirement and attempt IDs remain the single stale-result guard. A later
explicit reconnect uses the existing `/mcp` reconnect path.

## Non-Goals

- Changing MCP startup timeouts or reconnect policy.
- Making session transcript loading itself interruptible.
- Adding compatibility paths or retaining the blocking startup loop.
- Copying Codex's app-server event buffering architecture.

## Verification

- Add one exact interactive lifecycle test proving typed input is processed while an MCP server
  remains pending.
- Add one exact interrupt test proving Esc with no active turn cancels MCP startup and leaves the
  terminal loop usable.
- Add one manager test proving cancellation retires pending work without disconnecting an already
  connected server.
- Add one exact priority test only if existing active-turn Esc coverage cannot demonstrate that a
  turn remains the first interrupt target while MCP startup is active.
- Run the narrow `neo-agent` binary tests, formatting, and diff checks.
