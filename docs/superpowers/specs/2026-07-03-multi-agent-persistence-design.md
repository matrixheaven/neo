# Neo Multi-Agent Persistence Design

## Summary

Neo will move from a single-session transcript file to a session-scoped agent
layout:

```text
<sessions>/<bucket>/<session_id>/
  state.json
  agents/
    main/
      wire.jsonl
      tasks/
    agent-0/
      wire.jsonl
      tasks/
        bash-xxxxxxxx.json
        bash-xxxxxxxx/
          output.log
```

The main agent and every subagent use the same `AgentEvent` JSONL wire format.
`state.json` is the durable registry that binds agent ids to record
directories, parent agent ids, roles, and swarm membership. Runtime code will
read only this new layout after migration. The old `<session>/transcript.jsonl`
path is migrated away and then deleted; Neo will not keep long-term dual-path
compatibility.

This follows the Kimi Code model where `Session` owns the agent registry and
each `Agent` owns its own `wire.jsonl` and `tasks/`, with one Neo-specific
adjustment: Neo stores agent record directories as session-relative paths rather
than absolute homedirs so fork, copy, and cross-platform migration stay simple.

## Goals

- Persist the full main-agent and subagent event histories across process exit.
- Allow `neo resume` to restore Delegate and DelegateSwarm cards in the chat
  transcript.
- Allow `Delegate(resume = "...")`, `WaitDelegate`, task browser views, and
  related tools to find and resume subagents after restarting Neo.
- Keep each agent's background task metadata and large output logs scoped under
  that agent's record directory.
- Provide a one-shot migration tool for existing sessions.
- Remove the obsolete runtime dependency on `transcript.jsonl` after migration.

## Non-Goals

- Do not preserve `transcript.jsonl` as a supported runtime read path.
- Do not create a second subagent-specific wire schema.
- Do not embed full subagent transcripts inside the main agent wire.
- Do not store large command output in agent wire records when an `output.log`
  file is the authoritative full output.
- Do not add hosted or cross-device session behavior.

## Current State

Neo currently stores the main session at:

```text
~/.neo/sessions/<bucket>/<session_id>/transcript.jsonl
```

The current IO centers on `JsonlSessionWriter` and `JsonlSessionReader` in
`crates/neo-agent-core/src/session/mod.rs`. Major call sites include:

- run-mode creation and append in `crates/neo-agent/src/modes/run/mod.rs`
- session path creation in `crates/neo-agent/src/modes/run/session_mgmt.rs`
- CLI session commands in `crates/neo-agent/src/modes/sessions.rs`
- interactive resume in `crates/neo-agent/src/modes/interactive/mod.rs`
- interactive session creation in
  `crates/neo-agent/src/modes/interactive/controller_factory.rs`
- shell event persistence in
  `crates/neo-agent/src/modes/interactive/shell_command.rs`
- RPC session handlers in `crates/neo-agent/src/rpc/server.rs`
- `SummarizeSessionsTool` in
  `crates/neo-agent-core/src/tools/sessions.rs`

Neo already emits parent-level multi-agent events in `AgentEvent`:

- `DelegateStarted`
- `DelegateUpdated`
- `DelegateFinished`
- `DelegateSwarmStarted`
- `DelegateSwarmUpdated`
- `DelegateSwarmFinished`

Those events contain `AgentSnapshot` or `SwarmSnapshot` data and are enough for
TUI cards to render during the current process. The missing piece is durable
runtime restoration. Today subagent child turns run in isolated `AgentContext`
values, but their child `AgentEvent` streams are only accumulated locally and
compressed into snapshot fields such as `activity`, `latest_text`,
`token_count`, and `prior_messages`. On `neo resume`, the main transcript is
replayed into `AgentContext`, but delegate/swarm events are not replayed into
`MultiAgentRuntime`, and subagent child wires do not exist.

## Target Session State

Add a session state file:

```json
{
  "schema_version": 1,
  "created_at": "2026-07-03T00:00:00Z",
  "updated_at": "2026-07-03T00:00:00Z",
  "agents": {
    "main": {
      "kind": "main",
      "record_dir": "agents/main",
      "parent_agent_id": null
    },
    "agent-0": {
      "kind": "sub",
      "record_dir": "agents/agent-0",
      "parent_agent_id": "main",
      "role": "coder",
      "swarm_id": null,
      "swarm_item": null
    }
  }
}
```

Rules:

- `record_dir` is relative to the session directory.
- `main` must exist and must have `kind = "main"`.
- Subagent ids are stable across restarts.
- A subagent must name its `parent_agent_id`.
- Swarm children record `swarm_id` and, when available, `swarm_item`.
- The existing workspace bucket and `session_<uuid>` id format remain unchanged.

The state file is not a transcript. It is an index and ownership registry. The
wire files remain the source of replayable agent history.

## Path API

Introduce a single path API in `neo-agent-core` session code and make all
callers use it:

- `session_state_path(session_dir) -> PathBuf`
- `agents_dir(session_dir) -> PathBuf`
- `agent_record_dir(session_dir, agent_id) -> PathBuf`
- `agent_wire_path(session_dir, agent_id) -> PathBuf`
- `main_agent_wire_path(session_dir) -> PathBuf`
- `agent_tasks_dir(session_dir, agent_id) -> PathBuf`

Existing helpers that return a transcript file path should be renamed or
replaced so the name reflects `wire.jsonl`. Tests and fixtures should stop
using the string `transcript.jsonl` for active sessions.

## Main Agent Wire

The main agent's wire moves from:

```text
<session>/transcript.jsonl
```

to:

```text
<session>/agents/main/wire.jsonl
```

The JSONL records stay as `AgentEvent` records. `JsonlSessionWriter` and
`JsonlSessionReader` can continue to be used, but should be renamed only if that
clarifies ownership. The first implementation can keep the type names and
change the paths they receive.

Main wire must continue to support:

- user and assistant message replay
- context compaction replay
- plan mode and todo replay
- approval/question/tool event replay
- parent-level delegate and swarm card replay
- CLI transcript/export/compact/session-summary commands
- RPC message/session handlers

## Subagent Wire

Each subagent gets:

```text
<session>/agents/<agent_id>/wire.jsonl
```

The child wire records the child runtime's full `AgentEvent` stream:

- `RunStarted`, `TurnStarted`, and completion events
- `MessageStarted`, deltas, and `MessageAppended`
- `ToolCall*` and `ToolExecution*`
- `TokenUsage`
- `Error`
- compaction or other state events if a future child runtime emits them

When a subagent is spawned:

1. Allocate a stable agent id.
2. Create or update `state.json`.
3. Create `agents/<agent_id>/wire.jsonl`.
4. Run the child turn using that wire writer.
5. Continue to emit parent-level `Delegate*` or `DelegateSwarm*` events into
   the main wire for transcript cards.

When a subagent is resumed:

1. Load metadata from `state.json`.
2. Validate that the target is a subagent and belongs to the calling parent.
3. Replay the subagent wire into a fresh child `AgentContext`.
4. Start the new child turn from that context.
5. Append new child events to the same subagent wire.
6. Emit parent-level lifecycle updates to main wire.

`AgentSnapshot.prior_messages` may remain as a UI/runtime convenience, but it
must not be the durable source of subagent conversation history after this
change. The durable source is the subagent wire.

## MultiAgentRuntime Restore

Add a replay API to `MultiAgentRuntime`, for example:

```text
restore_agent_snapshot(snapshot)
restore_swarm_snapshot(snapshot)
restore_from_main_events(events)
```

Behavior:

- Replaying `DelegateStarted` registers or refreshes a non-terminal agent.
- Replaying `DelegateUpdated` merges newer snapshot fields without state
  regression.
- Replaying `DelegateFinished` registers the terminal snapshot.
- Replaying `DelegateSwarmStarted/Updated/Finished` restores swarm snapshots and
  child references.
- Restored terminal agents can be resumed by `Delegate(resume = "...")`.
- Restored running/background agents that have no live task after process
  restart are surfaced as `lost`, with a resume hint.

This runtime restore should be used by interactive resume and by any path that
constructs an `AgentRuntime` for a continued session.

## Transcript Replay

`load_session_transcript` should keep both:

- replayed chat messages for `AgentContext`
- replayable UI events for the transcript pane

`replay_session_into_transcript` should replay delegate/swarm events through
the same TUI event path used for live events, not through a separate renderer.
That keeps Delegate and DelegateSwarm cards consistent with live rendering,
including existing grouped delegate cards and swarm progress summaries.

The normal `Using Delegate` / `Used Delegate` tool card absorption behavior
should remain unchanged: transcript replay should still show the richer
Delegate/DelegateSwarm cards rather than duplicate generic tool cards.

## Background Tasks and Output Logs

Task persistence becomes agent-scoped:

```text
<session>/agents/<agent_id>/tasks/<task_id>.json
<session>/agents/<agent_id>/tasks/<task_id>/output.log
```

Rules:

- `tasks/<task_id>.json` stores task metadata, status, kind, timestamps, and
  output path information.
- `tasks/<task_id>/output.log` stores complete stdout/stderr or equivalent
  output for large/background shell tasks.
- Wire records store bounded previews and paths, not full large output.
- On resume, task records that were running in a previous process become
  terminal `lost` records unless Neo has an explicit live process handle.
- `TaskOutput` reads from the agent-scoped task store.

This mirrors Kimi's separation of task metadata from full output logs while
keeping Neo's local-only model.

## Session Fork, Export, Compact, and Summary

Fork:

- Copy the entire session directory.
- Rewrite `state.json` only if needed for metadata fields.
- Relative `record_dir` entries require no path rewrite.
- Append fork markers to every agent wire if Neo keeps that behavior.

Export:

- Include `state.json`.
- Include all `agents/*/wire.jsonl`.
- Include task metadata and output logs when exporting a full session archive.
- Existing human-readable transcript export may default to main chat plus
  parent-level delegate/swarm summaries.

Compact:

- Main compaction reads and appends to `agents/main/wire.jsonl`.
- Subagent compaction is out of scope for the first pass unless a subagent wire
  is resumed and hits context pressure.

SummarizeSessions:

- Resolve sessions through `session_index.jsonl` as before.
- Summarize from `agents/main/wire.jsonl`.
- Optionally include delegate summaries from main wire snapshots.

## Migration Tool

Add a migration script under `scripts/`, for example:

```text
scripts/migrate_sessions_to_agent_layout.py
```

CLI:

```text
python scripts/migrate_sessions_to_agent_layout.py --neo-home ~/.neo --dry-run
python scripts/migrate_sessions_to_agent_layout.py --neo-home ~/.neo --apply
```

Default behavior:

- `--dry-run` is the default.
- `--backup` is enabled by default when `--apply` is used.
- The script is idempotent: sessions already using `agents/main/wire.jsonl` are
  reported as skipped.

For each old session:

1. Detect `<session>/transcript.jsonl`.
2. Verify `<session>/agents/main/wire.jsonl` does not already exist.
3. Create `<session>/agents/main/`.
4. Copy `transcript.jsonl` to `agents/main/wire.jsonl`.
5. Write `state.json` with a `main` agent entry.
6. Preserve file modified times where practical so session recency remains
   stable.
7. Remove `<session>/transcript.jsonl` after successful copy and state write.

For sessions that contain only main history, migration does not synthesize
subagent wires. Existing delegate snapshots in the main wire remain available
for transcript card replay. New runs after migration will create real subagent
wires.

Failure behavior:

- Never delete `transcript.jsonl` until copy and `state.json` write both
  succeed.
- If backup is enabled, copy the full session directory to a sibling backup
  before mutation.
- Print a per-session status table: `migrated`, `skipped`, or `failed`.
- Exit non-zero if any session fails under `--apply`.

## Runtime Cutover

After the migration script exists, runtime code should be cut over to the new
layout directly:

- New sessions create `state.json` and `agents/main/wire.jsonl`.
- Continued sessions require `state.json` and `agents/main/wire.jsonl`.
- Old sessions without migration fail with a clear error telling the user to run
  the migration script.
- No fallback reads from `transcript.jsonl` remain in runtime code.

This is intentional. Keeping both paths would create a permanent maintenance
trap and make future agents continue patching the obsolete layout.

## Error Handling

- Missing `state.json`: report that the session has not been migrated.
- Missing `agents/main/wire.jsonl`: report corrupted or incomplete session.
- Missing subagent wire for a registered subagent: surface the subagent as
  `lost` and prevent resume with a clear error.
- Unknown subagent id: preserve current `unknown delegate target` behavior.
- Parent mismatch on resume: reject the resume, matching Kimi's guard that a
  subagent must belong to the requesting parent.
- Running background tasks after process restart: mark as `lost`, not
  `running`.

## Testing

Use narrow tests only.

Core session path and replay:

```text
cargo nextest run -p neo-agent-core --test session_jsonl jsonl_session_appends_reads_and_replays_events
cargo nextest run -p neo-agent-core --test session_jsonl jsonl_session_compaction_appends_algorithmic_summary_and_replays_kept_context
cargo nextest run -p neo-agent-core --test session_tree session_metadata_lists_existing_jsonl_sessions_with_names_and_children
```

Run and interactive resume:

```text
cargo nextest run -p neo-agent --bin neo create_session_path_uses_named_uuid_session_ids
cargo nextest run -p neo-agent --bin neo run_prompt_with_runtime_appends_continuation_to_existing_session_context
cargo nextest run -p neo-agent --bin neo load_session_transcript_estimates_context_usage_for_replayed_session
cargo nextest run -p neo-agent --bin neo fork_session_transcript_copies_jsonl_metadata_and_loads_child
```

CLI/RPC boundaries:

```text
cargo nextest run -p neo-agent --test cli_commands sessions_show_and_resume_read_jsonl_transcripts
cargo nextest run -p neo-agent --test cli_commands sessions_compact_stores_algorithmic_summary_and_resume_replays_kept_context
cargo nextest run -p neo-agent --test rpc_mode rpc_get_messages_replays_session_jsonl_messages
```

Multi-agent persistence:

```text
cargo nextest run -p neo-agent-core --test multi_agent_runtime <exact restore/resume test>
cargo nextest run -p neo-tui --test multi_agent_transcript <exact replay card test>
```

Migration:

```text
cargo nextest run -p neo-agent --test session_migration <exact migration test>
```

The exact test names should be chosen during implementation. Avoid broad
package-wide tests as evidence.

## Open Decisions

None for the design direction. The selected approach is the full Kimi-like
agent registry with Neo-specific relative paths, one-shot migration, and no
long-term old-path compatibility.
