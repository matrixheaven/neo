# Sessions

Sessions are Neo's durable local event record. They make agent work inspectable,
exportable, and resumable from local JSONL history.

## Storage

Sessions are stored in a **centralized, workspace-scoped** layout under
`~/.neo/sessions/` (or `$NEO_HOME/sessions/`). Each workspace (project
directory) gets a deterministic bucket directory:

```
~/.neo/sessions/
├── wd_neo_eb208ec56c5c/          ← bucket for /path/to/neo
│   ├── 1718370000000.jsonl       ← session transcript
│   ├── sessions.metadata.json    ← per-bucket metadata index
│   └── ...
├── wd_myproject_a1b2c3d4e5f6/    ← bucket for /path/to/myproject
│   └── ...
└── session_index.jsonl           ← global index (session ID → location)
```

The bucket name is `wd_<slug>_<hash12>` where `<slug>` is derived from the
directory basename and `<hash12>` is the first 12 hex chars of SHA-256 of the
canonicalized absolute path. This ensures:
- `/resume` only shows sessions from the **current workspace**
- Different projects with the same basename get different buckets
- The `NEO_HOME` env var overrides the home directory (`~/.neo` by default)

The global `session_index.jsonl` enables `neo resume <session_id>` to locate
sessions across workspaces.

## Prompt History

Neo persists prompt input history per workspace bucket so the Up/Down keys can
recall prompts submitted in earlier TUI sessions for the same project.

```
~/.neo/sessions/
├── wd_neo_eb208ec56c5c/
│   ├── prompt-history.jsonl     ← append-only prompt input history
│   ├── 1718370000000.jsonl
│   └── ...
```

Each workspace bucket holds a single `prompt-history.jsonl` file. The format is
one JSON record per line:

```json
{"created_at":"2026-06-21T12:34:56.789Z","session_id":"01KV...","text":"implement /new"}
```

Storage rules:

- **Append only.** The file is never rewritten on each prompt; new entries are
  appended with `O_APPEND`.
- **Workspace scoped.** History lives under the session bucket, so different
  projects never share prompts.
- **Real prompts only.** Slash commands (for example `/model`, `/resume`,
  `/new`) never become user turns and are never persisted.
- **Trimmed and de-duplicated.** Blank prompts are skipped, and consecutive
  repeats are collapsed. Non-consecutive repeats are kept so recurring prompts
  stay recallable.
- **Bounded in memory.** Only the most recent 500 entries are loaded into the
  composer; the file itself is unbounded.
- **File order is authoritative.** `created_at` is informational; recall order
  follows the order entries were appended, not session id.

Navigation semantics:

- Up from an **empty** composer recalls the most recent prompt.
- Up while already navigating moves older; Down moves newer.
- Down past the newest restores the original draft (usually empty).
- Up from a **non-empty** composer that is not already navigating does **not**
  overwrite the draft — type or clear it first.
- Edits during navigation stop history navigation.
- Blocking dialogs (approval, question, model/session picker) consume Up/Down
  before the composer, so they never leak into prompt history.

History is non-critical: load and append failures never block prompt submission
or turn execution.

`neo-agent-core` provides JSONL helpers under `neo_agent_core::session`:

- `JsonlSessionWriter::create(path)`
- `JsonlSessionWriter::open_append(path)`
- `append_event(&AgentEvent)`
- `JsonlSessionReader::read_all(path)`
- `JsonlSessionReader::replay_messages(path)`
- `JsonlSessionReader::replay_context(path)`
- `compact_jsonl_session(path, options)`
- `SessionMetadataStore::list()`
- `SessionMetadataStore::list_recent()`
- `SessionMetadataStore::fork(parent_id, name)`
- `SessionMetadataStore::rename(session_id, name)`
- `SessionMetadataStore::summarize(session_id, summary)`
- `SessionMetadataStore::record_summary(session_id, summary, source)`
- `SessionMetadataStore::record_activity(session_id, prompt)`
- `SessionMetadataStore::record_title(session_id, title, model)`

New JSONL files start with a `session_metadata` record containing the
`neo.session.jsonl` format name, schema version, and creation timestamp.
Subsequent lines are serialized `AgentEvent` records. Readers skip the metadata
record during replay, and existing event-only JSONL files remain readable.

Session tree metadata is stored next to JSONL records in
`sessions.metadata.json`. Fork, rename, and local branch summary entries
decorate real `.jsonl` session files. `sessions export-json` replays the same
local JSONL events and combines them with local metadata into a portable JSON
artifact that omits absolute session file paths.

`compact_jsonl_session` appends an `AgentEvent::CompactionApplied` record using
a deterministic extractive transcript summary. It does not call a model and
does not synthesize AI prose.

## Resume Flow

The current replay flow is local:

1. `neo-agent resume <session-ref>` resolves the session file from
   `sessions_dir`.
2. `JsonlSessionReader` loads event history.
3. `replay_context` reconstructs `AgentContext`, including any stored
   `CompactionApplied` event.
4. The CLI prints the replayed transcript, compaction summary, and stored local
   branch summary.

CLI session references can be an exact session id, a unique id prefix, or a
`.jsonl` path inside the configured `sessions_dir`. Ambiguous prefixes are
rejected with matching candidates, and paths outside `sessions_dir` remain
invalid.

In live interactive TTY mode, `ctrl+r` opens a local session picker backed by
`SessionMetadataStore::list()` and the configured `sessions_dir`. Selecting a
session replays its JSONL context into the TUI, updates the session label, and
uses that same replayed context for the next prompt. With the session picker
focused, `ctrl+n` forks the selected session through
`SessionMetadataStore::fork()`, loads the forked JSONL transcript, and appends
subsequent prompts to the child session.

The `/new` slash command (alias `/clear`) starts a fresh unsaved session state
in the current workspace. It clears the active session id, transcript, todos,
pending approvals/questions, and prompt text, then redraws the welcome banner
and shows a `Started fresh session` status. Workspace root, selected model,
thinking, permission mode, and the current development mode are preserved. The
previous JSONL session is **not** deleted and remains visible in `/resume`. The
next real prompt carries `session_id = None`, so the existing streaming path
creates a brand-new workspace-scoped JSONL session. `/new` is blocked while a
turn is running (interrupt the turn first) and never pre-creates an empty
session file.

## CLI Surface

```bash
neo sessions list
neo sessions show <session-ref>
neo sessions rename <session-ref> <name>
neo sessions fork <session-ref> --name <name>
neo sessions summarize <session-ref>
neo sessions compact <session-ref> --keep-recent 20
neo sessions export-html <session-ref>
neo sessions export-json <session-ref>
neo resume <session-ref>
```

Session directory defaults to `~/.neo/sessions/` with workspace-scoped bucket
subdirectories. Can be overridden with `sessions_dir`.

`export-html` replays `MessageAppended` events and renders a standalone HTML
conversation with `neo-agent-core`'s safe Markdown renderer. `export-json` replays the
same events and emits a stable local-only artifact:

```json
{
  "format": "neo.session.export_json",
  "schema_version": 1,
  "metadata": {
    "id": "alpha",
    "parent_id": null,
    "children": [],
    "message_count": 2
  },
  "messages": []
}
```

`sessions summarize` stores a deterministic local branch summary in
`sessions.metadata.json` and surfaces it in `sessions list` and `resume`.
`sessions compact` appends a `CompactionApplied` event to the session JSONL.

## RPC Surface

`neo rpc` accepts JSONL request frames for local session clients:

```json
{"type":"request","id":"commands","method":"get_commands","params":{}}
{"type":"request","id":"sessions","method":"sessions.list","params":{}}
{"type":"request","id":"messages","method":"get_messages","params":{"session_id":"alpha"}}
{"type":"request","id":"export","method":"sessions.export_json","params":{"session_id":"alpha"}}
```

`sessions.list`, `sessions.get`, `sessions.export_html`, and
`sessions.export_json` read local `sessions_dir` JSONL plus
`sessions.metadata.json`. They do not create remote continuation, share, or
import records.

## Non-Goals

Neo's local-only session docs do not present profile sync, hosted share/import,
remote resume, managed collaboration, or hosted continuation services as
supported features.

See [examples/rust/session_replay.rs](../examples/rust/session_replay.rs) for a
JSONL replay snippet.
