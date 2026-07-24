# Delegate Edit/Write File Activity Implementation Plan

## Goal

Implement the approved per-file Edit/Write activity rows inside Delegate,
DelegateGroup, and expanded DelegateSwarm cards while preserving every outer
card layout and keeping Edit/Write `ToolResult.details` as execution truth.

## Architecture

`neo-agent-core` projects the presentation-safe subset of structured Edit/Write
arguments and `details.changes[]` into the existing `AgentActivityKind::Tool`.
`neo-tui::transcript::child_activity` renders that typed projection once for all
three Delegate-family consumers. No card-specific parser or renderer is added.

## Tech Stack

- Rust 2024, `serde`, `serde_json`, `schemars`
- Neo `AgentEvent`, `AgentSnapshot`, and `ToolResult` contracts
- `neo-tui` `Line`/`Span` and Unicode-width wrapping
- `cargo nextest`, standalone `rustfmt`, `git diff --check`

## Baseline/Authority Refs

- `AGENTS.md`
- `docs/aegis/specs/2026-07-24-delegate-edit-write-file-activity-brief.md`
- `docs/aegis/specs/2026-07-20-batch-edit-design.md`
- `docs/aegis/specs/2026-07-21-batch-write-design.md`
- current multi-agent state/runtime and shared child-activity renderer

## Compatibility Boundary

- Do not change Edit/Write model-visible inputs or `ToolResult.details`.
- Do not change Delegate-family headers, hierarchy, progress, identity,
  placement, final summaries, or expansion controls.
- New fields default empty and are omitted when serialized, so old snapshots
  keep summary-only rendering.
- Do not add runtime events, summary parsers, arbitrary JSON in the TUI, or
  card-specific Edit/Write renderers.

## TDD Route

- Mode: off
- Decision: skipped
- Strict authority: not applicable
- Test posture: post-change regression
- Reason: strict TDD was not requested; existing integration targets can prove
  the projection and shared presentation directly.
- Verification: exact filtered tests plus scoped formatting/diff checks.

## Verification

```bash
cargo nextest run -p neo-agent-core --test multi_agent_runtime child_activity_projects_edit_write_file_rows
cargo nextest run -p neo-tui --test multi_agent_transcript delegate_family_renders_edit_write_file_rows
rustfmt --check --edition 2024 crates/neo-agent-core/src/multi_agent/state.rs crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-agent-core/src/multi_agent/mod.rs crates/neo-agent-core/tests/multi_agent_runtime.rs crates/neo-tui/src/transcript/child_activity.rs crates/neo-tui/tests/multi_agent_transcript.rs
git diff --check -- crates/neo-agent-core/src/multi_agent/state.rs crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-agent-core/src/multi_agent/mod.rs crates/neo-agent-core/tests/multi_agent_runtime.rs crates/neo-tui/src/transcript/child_activity.rs crates/neo-tui/tests/multi_agent_transcript.rs
```

Each `nextest` command must report its named test as `PASS`; no workspace-wide
health claim is implied.

## Planning Readback

Requirement Ready Check:

- Source: approved 2026-07-24 Spec Brief.
- Acceptance: all nine Spec acceptance items.
- Open blockers: none.
- Decision: ready.

Change Necessity:

- Need: file identities are discarded before Delegate-family rendering.
- Non-code option: documentation cannot restore absent runtime data.
- Minimum boundary: current multi-agent projection and shared renderer.
- Decision: code-change.

Existence Check:

- Surface: typed per-file fields on existing tool activity.
- Reuse: `AgentActivityKind::Tool`, `DelegateToolProgress`,
  `render_child_tool_row`.
- Proof: aggregate prose cannot safely encode ordered structured outcomes.
- Decision: add-with-proof inside existing owners.

Architecture Integrity Lens:

- Edit/Write own execution truth; multi-agent runtime owns projection; TUI owns
  presentation.
- One shared renderer covers all card families; no overlap or fallback parser.
- Verdict: aligned.

Plan-Time Complexity Check:

- `state.rs` is 554 lines, `runtime.rs` 4201, `child_activity.rs` 563.
- `runtime.rs` is large but already owns all activity projection and batch-tool
  summaries; small adjacent helpers are lower entropy than a new module.
- Recommendation: edit in place with private helpers.
- Budget: within scope.

Plan Pressure Test: owners, compatibility, verification, and retirement are
explicit; result `proceed`.

## Execution Readiness View

- Intent Lock: reveal every Edit/Write path and per-file outcome normally.
- Scope Fence: typed projection, shared renderer, focused tests.
- Baseline Lock: approved Spec plus current Batch Edit/Write contracts.
- Owner Constraints: structured details only; no summary parsing.
- Compatibility: old snapshots default to no typed rows.
- Retirement: no legacy live parser exists or will be added.
- Drift Rule: stop if implementation needs result-schema changes, new events,
  or card-specific rendering.
- Completion Evidence: two named tests, rustfmt, scoped diff check, scoped
  commit.
- Advisory Boundary: guidance only, not completion authority.

## Task 1: Preserve Typed File Rows

Files:

- `crates/neo-agent-core/src/multi_agent/state.rs`
- `crates/neo-agent-core/src/multi_agent/runtime.rs`
- `crates/neo-agent-core/src/multi_agent/mod.rs`
- `crates/neo-agent-core/tests/multi_agent_runtime.rs`

Why: the canonical runtime currently reduces structured Edit/Write results to
aggregate prose and loses file identities.

Steps:

1. Add typed presentation data beside `AgentToolOutputPreview`:

   ```rust
   pub enum AgentToolFileOperation { Edited, Created, Overwritten }
   pub enum AgentToolFileStatus {
       Pending, Committed, CommittedUnsynced, Failed, NotAttempted,
   }
   pub struct AgentToolFileChange {
       pub path: String,
       pub operation: Option<AgentToolFileOperation>,
       pub status: AgentToolFileStatus,
       pub line_count: Option<usize>,
       pub added: Option<usize>,
       pub removed: Option<usize>,
       pub message: Option<String>,
   }
   ```

   Derive adjacent debug/clone/equality/serde/schema traits and serialize enum
   values as `snake_case`.

2. Add `files: Vec<AgentToolFileChange>` with empty serde defaults to
   `AgentActivityKind::Tool` and `DelegateToolProgress`. Propagate through
   progress snapshots, progress application, existing constructors, and public
   exports.

3. Extend existing upsert helpers so a non-empty projection replaces prior
   rows while an empty progress update preserves richer prepared rows.

4. Add private runtime projections next to batch summary helpers:

   - Edit arguments: first-seen `edits[].path`, `Edited/Pending`.
   - Write arguments: `files[].path`, unknown operation, `Pending`.
   - prepared/terminal `changes[]`: ordered typed rows with known operation,
     status, line count, `added`, `removed`, and compact message.
   - terminal `path` without `changes[]`: mark the matching argument row failed
     and remaining rows not attempted.
   - progress details without `changes[]`: preserve prior rows.

   Ignore malformed entries; never parse `ToolResult.content`.

5. Add `child_activity_projects_edit_write_file_rows` covering running Edit
   path de-duplication, committed Edit stats, partial Write outcomes, and
   `progress_snapshot()` preservation.

6. Run the exact core `nextest` command from Verification and require `PASS`.

## Task 2: Render Rows Through The Shared Owner

Files:

- `crates/neo-tui/src/transcript/child_activity.rs`
- `crates/neo-tui/tests/multi_agent_transcript.rs`

Why: all three card families already call `render_child_tool_row`; this is the
only presentation seam that needs behavior.

Steps:

1. Extend `ChildToolRow` and `tool_row()` to borrow the typed file list.

2. Add private formatting helpers with the approved mapping:

   ```text
   Pending -> …
   Edit committed -> M <path> +N -N
   Write created -> C <path> N lines
   Write overwritten -> M <path> N lines
   CommittedUnsynced -> ! plus known C/M operation
   Failed -> ✗ plus known operation and compact message
   NotAttempted -> – plus known operation
   ```

3. Render rows immediately after the aggregate tool row and before existing
   output preview. Use `wrap_width` plus continuation indentation so long paths
   are complete and no visual row exceeds width. Do not cap file rows.

4. Add `delegate_family_renders_edit_write_file_rows` covering completed Edit,
   partial Write, Delegate/Group/expanded Swarm reuse, order, markers, long-path
   wrapping, and unchanged empty-file-list rendering.

5. Run the exact TUI `nextest` command from Verification and require `PASS`.

## Task 3: Scoped Verification And Commit

1. Run both exact tests again.
2. Run the exact standalone `rustfmt --check` command; format only touched Rust
   files if required.
3. Run scoped `git diff --check`, inspect `git diff --stat`, and confirm no
   outer-card, Edit/Write schema, result, or event source changed.
4. Commit only the six planned Rust/test files as:

   ```bash
   git commit -m "feat(tui): show delegate file activity"
   ```

## Risks

- Empty progress updates could erase prepared rows; upsert must preserve them.
- The enum-variant field requires mechanical constructor updates; keep them in
  existing owner/test files.
- Unbounded rows increase transcript height by approved design; viewport
  scrolling must retain them without changing hierarchy.
- Malformed/old snapshots must yield empty typed rows, never summary parsing.

## Retirement

No old file-row implementation exists. Aggregate summaries remain first-row
content by design. Summary-only replay is retained only for old snapshots with
no optional typed field; there is no live fallback parser to retire.

ADR signal: no new ADR or baseline sync unless implementation disproves the
approved ownership boundary.
