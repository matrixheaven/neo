# `/fork` Slash Command — Design Spec

## Summary

Implement a `/fork` slash command that creates a lossless copy of the current session (MainAgent, Subagents, blobs, state, tasks, plans, goals) and immediately enters the forked session — mirroring the `/resume` enter-session UX. The transcript shows two notice lines:

```
fork from session <parent_id>
switch to fork session <child_id>
```

## Background

Neo already has a complete fork infrastructure but does not expose it as a slash command:

- `SessionMetadataStore::fork` (`crates/neo-agent-core/src/session/mod.rs:509`) — recursively copies the entire session directory via `copy_dir_all`.
- `fork_session_transcript` (`crates/neo-agent/src/modes/interactive/mod.rs:2085`) — calls fork, loads child transcript, inserts a notice.
- `fork_current_session` / `fork_selected_session` (`crates/neo-agent/src/modes/interactive/sessions.rs:88,73`) — rebuild transcript, set `active_session_id`.
- Command palette `"fork"` and session-picker `SessionFork` keybinding already work.

What is missing: the `/fork` slash command itself, the two-line notice format, and `/fork` in completion/help.

## Data Integrity (already verified — no changes needed)

`SessionMetadataStore::fork` performs `copy_dir_all` of the session directory, which is fully lossless:

| Data | Location | Copied? |
|---|---|---|
| MainAgent transcript | `agents/main/wire.jsonl` | ✅ recursive copy |
| MainAgent tasks/plans/goals | `agents/main/{tasks,plans,goals}/` | ✅ recursive copy |
| Subagents | `agents/<subagent_id>/` (entire subtree) | ✅ recursive copy |
| Blobs | `blobs/<sha256>.bin` | ✅ content-addressed, ride along with copy |
| Agent state | `state.json` | ✅ copied; `record_dir` uses relative paths (`agents/main`, `agents/<id>`) so no remapping needed |
| Session list metadata | `sessions.metadata.json` | ✅ updated by fork with `parent_id` linkage |

## Design

### Architecture

`/fork` wires into the existing fork pipeline with minimal additions:

```
User types /fork
  → handle_simple_slash_command matches "/fork"
    → fork_current_session()
      → (self.fork_session)(parent_id)        ← calls fork_session_transcript
        → SessionMetadataStore::fork()         ← copy_dir_all (lossless)
        → load_session_transcript(child_id)    ← replay child wire.jsonl
        → notices: [
            "fork from session {parent_id}",
            "switch to fork session {child_id}",
          ]
      → rebuild_transcript_from_session()      ← renders notices + replayed messages
      → active_session_id = child_id           ← entered forked session
```

### Changes (4 files, ~20 lines)

#### 1. `crates/neo-agent/src/modes/interactive/slash_commands.rs`

Add `"/fork"` to the `handle_simple_slash_command` match block:

```rust
"/fork" => {
    self.fork_current_session().await?;
}
```

Placed alongside `"/resume" => self.open_session_picker()`.

#### 2. `crates/neo-agent/src/modes/interactive/mod.rs`

Modify `fork_session_transcript` (lines 2085–2096) to emit two notices instead of one:

**Before:**
```rust
loaded.notices.insert(0, format!("forked from {parent_id}"));
```

**After:**
```rust
loaded.notices.insert(0, format!("fork from session {parent_id}"));
loaded.notices.insert(1, format!("switch to fork session {child_id}"));
```

#### 3. `crates/neo-agent/src/modes/interactive/prompt_completion.rs`

Add `/fork` to `STATIC_SLASH_COMMANDS`:

```rust
("/fork", "Fork the current session"),
```

#### 4. `crates/neo-agent/src/modes/interactive/sessions.rs`

Remove the redundant `push_status` at line 102 of `fork_current_session`:

**Before:**
```rust
self.active_session_id = Some(forked.session_id);
self.push_status(format!("Forked session {parent_id} to {child_id}"));
Ok(())
```

**After:**
```rust
self.active_session_id = Some(forked.session_id);
Ok(())
```

The two transcript notices now convey the parent/child information. This also makes the `/fork` slash path, command palette path, and picker path all produce the same UX (notices in transcript, no status line).

## UX Flow

1. User types `/fork` (with or without trailing whitespace).
2. The current session is forked instantly (directory copy on disk).
3. The user is placed directly into the forked session (same as `/resume` entering a session).
4. The transcript shows, right after the welcome banner and before the replayed message history:
   ```
   fork from session session_abc12345-...
   switch to fork session session_def67890-...
   ```
5. The forked session appears in the `/resume` session picker list.

## Edge Cases

- **No active session** (`active_session_id` is `None`): `fork_current_session` already handles this — pushes status `"No active session to fork"` and returns early.
- **Session not saved to disk**: `SessionMetadataStore::fork` calls `ensure_session_exists`, which returns `SessionError::MissingSession` if `agents/main/wire.jsonl` doesn't exist. The error propagates as an `anyhow::Error` context message.
- **Active turn running**: No explicit guard — consistent with the existing command-palette fork behavior. The `fork_current_session` call will proceed; the existing turn continues against the parent session's in-memory state, which is unaffected by the disk copy.

## Testing

### Unit test update

Modify the existing test `fork_session_transcript_copies_jsonl_metadata_and_loads_child` in `crates/neo-agent/src/modes/interactive/tests.rs:5827`:

- Assert that `forked.transcript.notices` contains exactly two entries:
  - `"fork from session <parent_id>"`
  - `"switch to fork session <child_id>"`

### Integration test (new)

Add a test that exercises the `/fork` slash command through the controller:

- Set up a controller with an active session (wire.jsonl on disk).
- Submit `/fork` via the slash command handler.
- Assert that `active_session_id` changed to the child session.
- Assert that the transcript pane contains the two notice lines.
- Assert that the child session directory is a full copy (wire.jsonl exists, state.json exists).

## Out of Scope

- Fork marker events appended to `wire.jsonl` (kimi-code pattern) — Neo uses transcript notices instead.
- Active-turn guard preventing fork during a running turn.
- Global `session_index.jsonl` population (pre-existing gap unrelated to fork).
