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
- `JsonlSessionReader::replay_context(path)`
- `compact_jsonl_session(path, options)`
- `replay_messages(events.iter())`
- `SessionMetadataStore::list()`
- `SessionMetadataStore::fork(parent_id, name)`
- `SessionMetadataStore::rename(session_id, name)`
- `SessionMetadataStore::summarize(session_id, summary)`

New JSONL files start with a `session_metadata` record containing the
`neo.session.jsonl` format name, schema version, and creation timestamp.
Subsequent lines are serialized `AgentEvent` records. Readers skip the metadata
record during replay, and existing event-only JSONL files remain readable.
`replay_messages` reconstructs conversation history from
`AgentEvent::MessageAppended` entries.
Session tree metadata is stored next to JSONL records in
`sessions.metadata.json`. Fork and rename entries decorate real `.jsonl`
session files. Local branch summaries are stored in the same metadata file and
can be regenerated from replayed JSONL messages with `sessions summarize`.
These records do not create hosted or remote share records.

`compact_jsonl_session` replays the JSONL file into an `AgentContext`, builds a
deterministic extractive transcript summary from messages that will no longer be
kept in active context, and appends an `AgentEvent::CompactionApplied` record to
the same JSONL file. It does not call a model and does not synthesize AI prose.
The summary text is labeled as an algorithmic transcript summary.

## Resume Flow

The current local replay flow is:

1. `neo-agent resume <session-id>` resolves the session file from `sessions_dir`.
2. `JsonlSessionReader` loads event history.
3. `replay_context` reconstructs `AgentContext`, including any stored
   `CompactionApplied` event.
4. The CLI prints the replayed transcript, compaction summary, and stored local
   branch summary.

In live interactive TTY mode, `ctrl+r` opens a local session picker backed by
`SessionMetadataStore::list()` and the configured `sessions_dir`. Selecting a
session replays its JSONL context into the TUI as a read-only transcript view,
updates the session label, and blocks new prompt submission from that historical
view. This avoids pretending that the selected JSONL file will be appended.

## Storage Expectations

The current constraints are:

- Append-only event records for auditability.
- Human-inspectable data where practical.
- No secrets in session logs; store provider/config references, not raw keys.

Still missing from pi parity:

- Hosted share targets beyond local HTML export.
- Hosted or model-generated branch summaries beyond local metadata summaries.
- Model-generated compaction summaries; current compaction is deterministic
  local transcript extraction only.
- Live continuation that appends new turns to a selected existing JSONL session
  or creates an explicit fork before continuing.

## CLI Surface

The `neo-agent` binary exposes:

```bash
neo sessions list
neo sessions show <session-id>
neo sessions rename <session-id> <name>
neo sessions fork <session-id> --name <name>
neo sessions summarize <session-id>
neo sessions compact <session-id> --keep-recent 20
neo sessions export-html <session-id>
neo resume <session-id>
```

Session directory defaults to `.neo/sessions` and can be changed with
`sessions_dir` or `NEO_SESSIONS_DIR`.

`export-html` replays `MessageAppended` events and renders a standalone HTML
conversation with `neo-sdk`'s safe Markdown renderer.

`sessions summarize` stores a deterministic local branch summary in
`sessions.metadata.json` and surfaces it in `sessions list` and `resume`.
`sessions compact` appends a `CompactionApplied` event to the session JSONL.
`resume` and transcript rendering replay compacted context honestly: compacted
messages are omitted from the active message list, while the stored algorithmic
summary is displayed separately.

See [examples/rust/session_replay.rs](../examples/rust/session_replay.rs) for a
JSONL replay snippet.
