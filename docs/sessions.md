# Sessions

Sessions are the durable record that make agent work resumable.

## Intended Responsibilities

A session should store:

- Session id, creation time, and current working directory.
- User, assistant, and tool-result messages.
- Normalized provider stream milestones needed for replay.
- Tool authorization decisions and execution results.
- Config snapshot needed to explain which provider/model was used.

## Resume Flow

1. `neo-agent resume <session-id>` asks `neo-agent-core` to load the session.
2. The runtime reconstructs conversation history as `ChatMessage` values.
3. Pending or incomplete tool calls are surfaced to the user instead of silently replayed.
4. New model events append to the same session log.

## Storage Expectations

The storage format is not implemented in this slice. The intended constraints are:

- Append-only event records for auditability.
- A stable schema version in every session file or database row.
- Human-inspectable data where practical.
- No secrets in session logs; store provider references, not raw keys.

## CLI Surface

The `neo-agent` binary already reserves:

```bash
neo sessions list
neo sessions show <session-id>
neo resume <session-id>
```

These commands currently print placeholders until the session store lands in `neo-agent-core`.
