# Sessions

Sessions are the durable event record that make agent work inspectable and
resumable from local JSONL history.

## Implemented Storage

`neo-agent-core` currently provides JSONL helpers under
`neo_agent_core::session`:

- `JsonlSessionWriter::create(path)`
- `JsonlSessionWriter::open_append(path)`
- `append_event(&AgentEvent)`
- `JsonlSessionReader::read_all(path)`
- `JsonlSessionReader::replay_messages(path)`
- `replay_messages(events.iter())`
- `SessionMetadataStore::list()`
- `SessionMetadataStore::fork(parent_id, name)`
- `SessionMetadataStore::rename(session_id, name)`

Each line is a serialized `AgentEvent`. `replay_messages` reconstructs
conversation history from `AgentEvent::MessageAppended` entries.
Session tree metadata is stored next to JSONL records in
`sessions.metadata.json`. Fork and rename entries decorate real `.jsonl`
session files; they do not create hosted or remote share records.

## Resume Flow

The intended flow is:

1. `neo-agent resume <session-id>` resolves the session file from `sessions_dir`.
2. `JsonlSessionReader` loads event history.
3. `replay_messages` reconstructs `AgentMessage` values.
4. The runtime converts those messages into `neo_ai::ChatMessage` values for
   the next model turn.
5. Pending or incomplete tool calls are surfaced to the user instead of silently
   replayed.

## Storage Expectations

The current constraints are:

- Append-only event records for auditability.
- Human-inspectable data where practical.
- No secrets in session logs; store provider/config references, not raw keys.

Still missing from pi parity:

- Compaction and branch summaries.
- Share targets beyond local HTML export.
- A stable schema version field.

## CLI Surface

The `neo-agent` binary exposes:

```bash
neo sessions list
neo sessions show <session-id>
neo sessions rename <session-id> <name>
neo sessions fork <session-id> --name <name>
neo sessions export-html <session-id>
neo resume <session-id>
```

Session directory defaults to `.neo/sessions` and can be changed with
`sessions_dir` or `NEO_SESSIONS_DIR`.

`export-html` replays `MessageAppended` events and renders a standalone HTML
conversation with `neo-sdk`'s safe Markdown renderer.

See [examples/rust/session_replay.rs](../examples/rust/session_replay.rs) for a
JSONL replay snippet.
