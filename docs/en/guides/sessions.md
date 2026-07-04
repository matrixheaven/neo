# Session management

Neo stores every conversation as a **local JSONL transcript** — resumable, forkable, compactable, and exportable. This page covers the session persistence model and common operations.

## Session persistence concepts

| Concept | Notes |
| --- | --- |
| **Session** | A JSONL event stream containing system / user / assistant / tool / shell messages |
| **Storage location** | Isolated per workspace: `~/.neo/sessions/wd_<slug>_<hash12>/agents/main/wire.jsonl` |
| **Metadata** | Name, summary, parent/child relationships — stored in a metadata file inside the session directory |
| **Session ID** | Shaped like `session_<uuid>`; can be referenced by full ID or by the JSONL path |

The transcript is provider-agnostic — all events are normalized to `AgentEvent`, and the JSONL never depends on a specific provider protocol.

> Workspace isolation means sessions from different project directories never interfere; switching directories switches the visible session pool.

## Resuming sessions

### Command line

```bash
neo -c                          # continue the most recent session in the current workspace
neo -r                          # open the session picker (TTY only)
neo resume <session-id>         # resume a specific session; in non-TTY mode, prints its transcript
```

| Flag | Behavior |
| --- | --- |
| `-c` / `--continue` | Resume the most recent session directly |
| `-r` / `--resume` | Open the session picker on launch |
| `--no-session` | Don't persist a new session this run (handy for one-off scripts) |

`-c`, `-r`, and `--no-session` are mutually exclusive.

### Inside the TUI

Type `/resume` or press the session-picker shortcut in the interactive UI to open the picker; you can toggle scope between "current workspace" and "all workspaces", then press Enter to load a session.

## Forking sessions /fork

Forking copies an existing session's current state into a new independent branch — the original session is left untouched. Great for "try two paths from this point".

```bash
neo sessions fork <session-id> --name "experiment-A"
```

In the TUI session picker, select the target session and press the fork shortcut.

A forked session shows up in the list with a `parent=<id>` marker, and the original session's `children` field records the new session ID.

## Compacting sessions /compact

Long sessions approach the context window. `/compact` uses an LLM to summarize older messages into a compaction summary, keeping the most recent few raw messages.

```bash
# CLI: compact a specific session, keeping the last 20 messages
neo sessions compact <session-id> --keep-recent 20
```

```text
# Inside the TUI
/compact                                      # compact with default strategy
/compact keep only the parts about the auth module   # with a natural-language instruction
```

After compaction, the session file is rewritten: compacted messages are replaced by a `CompactionSummary`, and new conversation continues to append after it. `neo resume` restores the compaction summary automatically when reading.

## Exporting sessions

Neo currently supports HTML and JSON export (Markdown export can be derived from JSON):

| Command | Output |
| --- | --- |
| `neo sessions export-html <session-id>` | Styled, human-readable HTML |
| `neo sessions export-json <session-id>` | Structured JSON (schema `neo.session.export_json`, v1) |

```bash
neo sessions export-html session_abc123 > talk.html
neo sessions export-json session_abc123 > talk.json
```

The JSON artifact contains session metadata (`id` / `name` / `summary` / `parent_id` / `children` / `message_count`) and the full message list, useful for archival or post-processing.

## Other session commands

| Command | Effect |
| --- | --- |
| `neo sessions list` | List sessions in the current workspace (name, parent/child, summary) |
| `neo sessions show <id>` | Print a session's raw JSONL |
| `neo sessions rename <id> <name>` | Rename a session |

A session's `summary` is generated automatically by Neo as it runs; names and summaries help you identify past sessions quickly in the picker.

## Storage location reference

| Content | Path |
| --- | --- |
| Main config | `~/.neo/config.toml` |
| Sessions root | `~/.neo/sessions/` |
| Workspace session bucket | `~/.neo/sessions/wd_<slug>_<hash12>/` |
| Main agent transcript | `<bucket>/agents/main/wire.jsonl` |
| Goals / plans / tasks | `<bucket>/agents/main/{goals,plans,tasks}/` |

Set the `NEO_HOME` environment variable to relocate all of this data. See the [data locations reference](../configuration/data-locations.md).

## Next steps

- [Interaction mode](interaction.md) — Interactive use of `/resume`, `/fork`, `/compact`
- [Goal mode](goals.md) — Session-level goals and phase artifacts
- [Data locations reference](../configuration/data-locations.md)
