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
`sessions tree` renders the local parent/child metadata as an indented tree.
These records do not create hosted or remote share records.

`compact_jsonl_session` replays the JSONL file into an `AgentContext`, builds a
deterministic extractive transcript summary from messages that will no longer be
kept in active context, and appends an `AgentEvent::CompactionApplied` record to
the same JSONL file. It does not call a model and does not synthesize AI prose.
The summary text is labeled as an algorithmic transcript summary.

## Resume Flow

The current local replay flow is:

1. `neo-agent resume <session-ref>` resolves the session file from `sessions_dir`.
2. `JsonlSessionReader` loads event history.
3. `replay_context` reconstructs `AgentContext`, including any stored
   `CompactionApplied` event.
4. The CLI prints the replayed transcript, compaction summary, and stored local
   branch summary.

CLI session references can be an exact session id, a unique id prefix, or a
`.jsonl` path inside the configured `sessions_dir`. Ambiguous prefixes are
rejected with the matching candidates, and paths outside `sessions_dir` remain
invalid.

In live interactive TTY mode, `ctrl+r` opens a local session picker backed by
`SessionMetadataStore::list()` and the configured `sessions_dir`. Selecting a
session replays its JSONL context into the TUI, updates the session label, and
uses that same replayed context for the next prompt. New events from that turn
are appended to the selected JSONL file. With the session picker focused,
`ctrl+n` forks the selected session through `SessionMetadataStore::fork()`,
loads the forked JSONL transcript, and appends subsequent prompts to the child
session.

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
- Hosted session tree continuation and share flows beyond local JSONL
  fork-before-continue controls.

## CLI Surface

The `neo-agent` binary exposes:

```bash
neo sessions list
neo sessions tree
neo sessions show <session-ref>
neo sessions rename <session-ref> <name>
neo sessions fork <session-ref> --name <name>
neo sessions summarize <session-ref>
neo sessions compact <session-ref> --keep-recent 20
neo sessions export-html <session-ref>
neo resume <session-ref>
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

## RPC Surface

`neo rpc` accepts JSONL request frames for local session clients:

```json
{"type":"request","id":"sessions","method":"sessions.list","params":{}}
{"type":"request","id":"tree","method":"sessions.tree","params":{}}
{"type":"request","id":"messages","method":"get_messages","params":{"session_id":"alpha"}}
```

`sessions.list` returns typed local metadata records with `id`, optional
`name`, optional `summary`, optional `parent_id`, and `children`.
`sessions.tree` returns the same records in local tree order with a `depth`
field. These RPC payloads read only local `sessions_dir` JSONL and
`sessions.metadata.json`; they do not create hosted continuation or share
records.

See [examples/rust/session_replay.rs](../examples/rust/session_replay.rs) for a
JSONL replay snippet.
