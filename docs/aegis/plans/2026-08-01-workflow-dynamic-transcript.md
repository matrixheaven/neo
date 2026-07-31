# Neo Workflow Dynamic Transcript Implementation Plan

Date: `2026-08-01`

Status: `approved design; ready for implementation`

## Goal

Replace repeated workflow progress rows and orphaned workflow-origin tool cards
with one bounded workflow group: one main card, one optional Delegate summary,
and one optional DelegateSwarm summary. Keep Todo, composer, footer, approvals,
and questions operable throughout progress, then commit the terminal workflow
group to native scrollback exactly once.

The permission-picker width fix in commit `253e711f` is a prerequisite and a
regression obligation, not part of the implementation work below.

## Architecture

Keep the existing runtime and transcript path:

```text
WorkflowRuntime / tool runtime
  -> typed AgentEvent with WorkflowExecutionOrigin
  -> TranscriptPane event routing
  -> one Workflow transcript entry with typed direct-tool and child snapshots
  -> workflow main / Delegate summary / DelegateSwarm summary projection
  -> TranscriptPresentation history/live decision and one terminal commit
  -> NeoTui reserves fitted chrome before composing the final live frame
  -> InlineTerminal transactional normal-screen write and acknowledgement
```

`WorkflowRuntime` remains the only workflow lifecycle and persistence owner.
`TranscriptStore` remains the typed transcript and ordering owner.
`TranscriptPresentation` remains the only history-versus-live owner. The new
summary renderers are pure projections over data already held by the workflow
entry; they do not own state or persistence.

Workflow-origin activity is stored under its existing workflow entry. It is not
kept as an independently rendered top-level tool or Delegate-family entry. The
existing non-workflow paths remain unchanged.

## Tech Stack

- Rust 2024, minimum Rust `1.96.1`;
- existing `AgentEvent`, `WorkflowExecutionOrigin`, `WorkflowCardComponent`,
  `ToolCallComponent`, `AgentSnapshot`, `SwarmSnapshot`,
  `TranscriptPresentation`, and `NeoChromeState`;
- existing `apply_agent_progress` and `apply_swarm_child_progress` helpers;
- standard-library collections only;
- no new dependency, feature flag, persistence format, compatibility renderer,
  second transcript store, or configurable card-height system.

## Baseline And Authority Refs

- approved design:
  `docs/aegis/specs/2026-08-01-workflow-dynamic-transcript-design.md`;
- reviewed implementation plan:
  `docs/aegis/plans/2026-07-31-native-scrollback-progressive-transcript.md`;
- reviewed handoff:
  `docs/aegis/handoffs/2026-07-31-native-scrollback-progressive-transcript.md`;
- current landed baseline:
  `docs/aegis/baseline/2026-07-31-native-terminal-transcript.md`;
- architecture decision:
  `docs/aegis/adr/ADR-0010-native-terminal-transcript-presentation.md`;
- user approval on 2026-08-01.

## Compatibility Boundary

- preserve workflow scheduling, journal, recovery, result, task projection, and
  model-visible tool behavior;
- preserve session JSONL shape; new Delegate-family origin fields are live-only
  metadata and are skipped by serialization and schema generation;
- preserve all non-workflow Delegate, DelegateGroup, and DelegateSwarm layouts,
  expansion behavior, ordering, progressive facts, and transcript placement;
- preserve ordinary non-workflow tool grouping and output expansion;
- preserve approval and question entries as independent blocking input owners;
- preserve explicit `Ctrl+O` review and Task Browser alternate-screen behavior;
- preserve normal-screen terminal write, flush, and acknowledgement ordering;
- do not rewrite historical transcripts or migrate persisted sessions;
- remove the retired workflow presentation paths without aliases or fallbacks.

## TDD Route

- Mode: `off`
- Decision: `skipped`
- Strict authority: `not applicable`
- Test posture: post-change focused regression
- Reason: strict test-first work was not requested; the approved design and the
  captured runtime/session evidence already identify the three root causes.
- Verification: every Rust test command names one package, one target, and one
  exact test.

## Verification

Focused evidence must prove:

1. live workflow provenance reaches direct-tool, Delegate, Delegate progress,
   DelegateSwarm, swarm progress, and question events without changing
   serialized JSON;
2. queued, running, phase, log, and report updates create zero workflow history
   rows and update one live workflow group in place;
3. workflow-origin direct tools appear only in the main card;
4. workflow-origin Delegate and DelegateSwarm activity appears only in its
   dedicated sibling summary, one row per visible child;
5. non-workflow Delegate-family cards render exactly through their existing
   path;
6. a terminal workflow group enters history once at terminalization, never as
   both a live and final group;
7. live row budgeting includes separators and the already-fitted bottom region;
8. Todo, composer, footer, and cursor survive every ordinary progress frame;
9. the earliest unresolved approval or question remains the sole input owner;
10. every final row fits the effective width and the assembled live frame fits
    the terminal height;
11. the permission picker still fits a 120-column terminal;
12. the retired workflow transition and tail-truncation paths have no remaining
    normal-path references.

## Scope Check

### Aegis Visibility

Planning is required because the repair crosses the event provenance boundary,
transcript ownership, retirement of an already-landed history path, and final
terminal geometry.

### Fact, Assumption, And Unknown

- Fact: `WorkflowRuntime` emits each observed projection once and assigns a
  monotonic `projection_sequence`.
- Fact: `TranscriptStore::upsert_workflow` currently captures accepted
  non-terminal snapshots as `WorkflowTransition` facts.
- Fact: `TranscriptPane::apply_tool_event` currently discards the existing
  `workflow_origin` on all ordinary tool execution mutations.
- Fact: Delegate-family and question events currently lack workflow provenance
  even though they can be emitted inside the workflow-hosted tool dispatch.
- Fact: `bound_live_blocks` counts content rows but not every separator inserted
  by `compose_live_blocks`.
- Fact: `NeoTui::render_terminal_frame_at` appends fitted chrome and then
  truncates from the tail, which removes the bottom interaction region first.
- Assumption: `WorkflowStarted` is delivered before tool activity for that run;
  add a focused assertion and a visible terminal routing error if this invariant
  is violated instead of silently creating a generic card.
- Unknown: none that blocks implementation. If runtime event order disproves the
  assumption, stop and repair that typed ordering boundary before continuing;
  do not add a pending-origin queue.

### BaselineUsageDraft

- Required baseline refs: approved design, native terminal baseline, ADR-0010.
- Delivered context refs: previous plan, handoff, implementation commits from
  `28efaf4f` through `3e9fb6fb`, current source, and captured session journal.
- Acknowledged before plan refs: all required refs.
- Cited in plan refs: all required refs.
- Missing refs: none.
- Decision: `continue`.

### Requirement Ready Check

- Requirement source refs: approved design and current user approval.
- Goals and scope refs: design Goals, Non-Goals, Existing Owners, Height Rules,
  Event Routing And Projection, Retirement Boundary, and Acceptance Criteria.
- User and scenario refs: `/permissions` width failure, repeated workflow rows,
  orphaned tool cards, and temporary loss of Todo/composer/footer during a live
  workflow.
- Acceptance and verification refs: the twelve focused obligations above.
- Open blocker questions: none.
- Decision: `ready`.

### Ripple Signal Triage

- `AgentEvent` constructors, session compaction match arms, and the question
  stream adapter must accept the new live-only provenance fields.
- TUI replay must continue to read historical events whose Delegate-family
  variants contain no provenance.
- Tool queue and shell updates that do not carry provenance must resolve their
  existing typed tool-call identity rather than infer a workflow from text.
- Existing non-workflow Delegate-family regressions are required because the
  shared events gain fields even though their renderer path does not change.
- Decision: contain changes to `neo-agent-core` event forwarding and `neo-tui`
  transcript presentation; runtime execution and persisted workflow files stay
  untouched.

### Change Necessity

- User-visible need: one stable workflow group and a bottom interaction region
  that cannot disappear during progress.
- No-change option: documentation or configuration cannot stop accepted
  snapshots becoming history, restore discarded typed provenance, or correct
  final row accounting.
- Why code change is necessary: all three failures are deterministic source
  behavior in the current transcript and frame path.
- Minimum change boundary: live event stamping, workflow transcript entry,
  workflow-only render helpers, presentation budgeting, and final frame
  composition.
- Decision: `code-change`.

### Existence Check

- Proposed new surfaces: two workflow-only summary renderers and one pure group
  projection helper.
- Existing reuse candidates: full Delegate/DelegateSwarm cards and generic tool
  groups.
- Why existing surfaces are insufficient: nesting or reusing full cards would
  duplicate headers, expansion state, activity bodies, and height pressure;
  generic tool groups cannot provide one-row child summaries.
- Creation proof: the approved UI requires exactly two sibling summary shapes,
  while state remains in the existing workflow transcript entry.
- Entropy impact: three private projection modules, no new state owner; delete
  the workflow transition renderer and top-level workflow child path.
- Decision: `add-with-proof`.

### Architecture Integrity Lens

- Invariant: mutable workflow state has one typed transcript entry and never
  becomes immutable history before terminalization.
- Canonical owner: workflow lifecycle stays in `WorkflowRuntime`; transcript
  state and ordering stay in `TranscriptStore`; history/live decisions stay in
  `TranscriptPresentation`.
- Responsibility overlap: workflow-only tool and child data lives inside the
  workflow entry, so no side map or duplicate top-level entry owns it.
- Higher-level simplification: stamp origin once in the workflow-hosted dispatch
  forwarding path and route all consumers by the same `run_id` and
  `invocation_id`.
- Retirement falsifier: any remaining `WorkflowTransition`, generic top-level
  workflow child card, or post-composition tail truncation means the old path is
  still active.
- Verdict: proceed.

### Complexity Budget

- Artifact class: shared event enum, large transcript store/event handler, and
  terminal presentation owner.
- Current pressure: `presentation.rs`, `store.rs`, `event_handler.rs`, and
  `tool_dispatch.rs` already exceed one thousand lines or carry several shared
  responsibilities.
- Projected pressure: adding all card rendering in place would make the shared
  owners harder to review and would couple child layout to lifecycle code.
- Budget result: `at-risk`.
- Planned governance: keep routing and storage edits local; put only pure
  workflow projections in `workflow_group.rs`, `workflow_delegate_card.rs`, and
  `workflow_swarm_card.rs`; do not extract generic frameworks.

### Plan-Time Complexity Check

- Target files: event forwarding, transcript store/event handler, workflow card,
  presentation, chrome composition, and focused tests.
- Owner fit: event provenance belongs in `AgentEvent`; workflow activity belongs
  in the workflow entry; row budgeting belongs in presentation.
- Add-in-place risk: high for new rendering logic in `presentation.rs` and
  `store.rs`; low for small routing branches and removal of old facts.
- Better file boundary: three private pure projection modules only.
- Recommendation: edit owners in place and add the three renderer helpers; no
  general abstraction or compatibility layer.

### Plan Pressure Test

- Owner and retirement: one runtime, one transcript entry, one history decision;
  old transition facts and orphaned card paths are deleted.
- Architecture integrity: typed provenance replaces inference and independent
  lifecycle maps.
- Verification scope: event JSON, routing, card shapes, terminal order, width,
  height, input focus, and lingering-reference checks are covered.
- Task executability: tasks below name exact files, methods, regressions, and
  commands.
- Pressure result: `proceed`.

## Anti-Entropy Decision

### Anti-Entropy Declaration

- Deletion class: `code-retirement`.
- Old paths: `WorkflowTransition`, `capture_workflow_transition`, standalone
  workflow-origin tool cards, full workflow-origin Delegate-family cards, and
  normal-path `update.live.truncate(height)`.
- New canonical owner: one workflow transcript entry plus the existing
  presentation and frame owners.
- Preserved behavior: runtime execution, persistence, explicit review,
  blocking input, non-workflow cards, and terminal acknowledgement.
- Retired behavior: non-terminal workflow history rows, orphaned workflow cards,
  and tail deletion of bottom chrome.
- External boundary touched: no.
- Source-of-truth data risk: none.
- User confirmation required: no.

### Retirement Decision

- Path: `delete-first`.
- Why: all retired paths are internal presentation logic with no published data
  dependency.
- Non-edits: no session migration, runtime rewrite, journal rewrite, provider
  change, tool schema change, or non-workflow card redesign.

### Verification Plan

- Main-path check: origin-bearing workflow activity renders in one live group
  and one terminal history block.
- Lingering-reference check: removed symbols and tail truncation are absent.
- Negative check: no queued/running/phase history line or workflow-origin
  top-level child card can be produced.
- Boundary check: live-only origin fields do not appear in serialized events and
  non-workflow cards retain their current snapshots and layout.

## File Map

### `neo-agent-core`

- `crates/neo-agent-core/src/events.rs`: add live-only workflow provenance to
  Delegate-family and question events.
- `crates/neo-agent-core/src/runtime/tool_dispatch.rs`: stamp all tool,
  Delegate-family, approval, and supported question events emitted by one
  workflow-hosted invocation.
- `crates/neo-agent-core/src/session/event_persistence.rs`: preserve existing
  persisted event shape while accepting the new fields.

### `neo-tui`

- `crates/neo-tui/src/transcript/tool_call.rs`: retain typed workflow origin on
  the existing tool entry and reject conflicting reparenting.
- `crates/neo-tui/src/transcript/workflow_card.rs`: own one workflow snapshot,
  direct tools, Delegate snapshots, and swarm snapshots; render the main card.
- `crates/neo-tui/src/transcript/workflow_group.rs` (new): pure main/sibling
  composition and height-priority degradation.
- `crates/neo-tui/src/transcript/workflow_delegate_card.rs` (new): one-row
  Delegate summaries for workflow children.
- `crates/neo-tui/src/transcript/workflow_swarm_card.rs` (new): one-row swarm
  child summaries with swarm identity when needed.
- `crates/neo-tui/src/transcript/mod.rs`: private module wiring.
- `crates/neo-tui/src/transcript/progressive.rs`: delete workflow transition
  identities, payloads, and rendering.
- `crates/neo-tui/src/transcript/store.rs`: route typed activity into the
  workflow entry, retain projection ordering, and remove transition capture.
- `crates/neo-tui/src/transcript/event_handler.rs`: preserve origin for every
  tool mutation and route Delegate-family events by typed run identity.
- `crates/neo-tui/src/shell/stream.rs` and
  `crates/neo-tui/src/shell/event_router.rs`: retain question provenance across
  the existing stream adapter.
- `crates/neo-tui/src/transcript/entry/mod.rs` and
  `crates/neo-tui/src/transcript/pane.rs`: retain live-only provenance on the
  independent question entry without changing its rendering.
- `crates/neo-tui/src/transcript/presentation.rs`: emit one logical workflow
  group, include separators in live cost, and commit one terminal group.
- `crates/neo-tui/src/app.rs`: remove post-composition tail truncation and assert
  the already-budgeted frame invariant.
- `crates/neo-tui/tests/workflow_transcript.rs`: workflow state, routing, card,
  ordering, and retirement regressions.
- `crates/neo-tui/tests/terminal_scrollback.rs`: combined normal-screen width,
  height, chrome, cursor, blocking input, and terminal commit regression.
- existing Delegate-family tests: non-workflow compatibility proof only; do not
  rewrite their card expectations.

### Completion Records

- `docs/aegis/adr/ADR-0010-native-terminal-transcript-presentation.md`: amend the
  workflow projection decision after implementation evidence exists.
- `docs/aegis/baseline/2026-07-31-native-terminal-transcript.md`: replace the
  superseded workflow transition rule with the landed group rule.
- `docs/aegis/INDEX.md`: add any new completion record created by the final
  implementation task.

## Execution Readiness View

- Intent Lock: implement the approved one-main-plus-two-sibling workflow group.
- Scope Fence: event provenance, transcript workflow grouping, workflow-only
  renderers, final row budgeting, focused tests, and landed decision records.
- Baseline Lock: approved design, native terminal baseline, and ADR-0010.
- Approved Behavior: one live group, no non-terminal history, one terminal
  commit, separate blocking input, and bottom chrome preservation.
- Owner Constraints: no second runtime, store, origin map, lifecycle map,
  renderer path, or persistence format.
- Compatibility Boundary: non-workflow cards, runtime, model results, sessions,
  explicit review, and terminal acknowledgement remain unchanged.
- Retirement Boundary: delete the four old presentation paths; do not retain a
  feature flag or fallback.
- Task Batches: Tasks 1-2 establish typed data; Tasks 3-5 project and finalize;
  Task 6 proves frame safety; Task 7 records the landed decision.
- Test Obligations: exact core event test, exact TUI workflow tests, exact
  terminal regression, existing permission-width regression, formatting, and
  lingering-reference search.
- Review Gates: after Task 2 verify no duplicate data owner; after Task 5 verify
  non-workflow card source files did not change layout; after Task 6 verify every
  progress frame retains the active input owner.
- Drift Rule: if provenance requires inference, persistence migration, or a
  second pending queue, stop and return to the approved design.
- Rewind Rule: repair the typed event/store owner that violates an invariant;
  never restore an old rendering path as a fallback.
- Evidence Required Before Completion: exact passing commands, clean
  `git diff --check`, retired-symbol search, implementation commits, and updated
  decision records.
- Advisory Boundary: planning guidance only; passing implementation evidence is
  still required before completion.

## Task 1: Carry Workflow Provenance Through Delegate-Family Events

### Files

- modify `crates/neo-agent-core/src/events.rs`;
- modify `crates/neo-agent-core/src/runtime/tool_dispatch.rs`;
- modify `crates/neo-agent-core/src/session/event_persistence.rs`.

### Why And Change Necessity

Ordinary tool execution already carries `WorkflowExecutionOrigin`, but
Delegate-family snapshots lose it at the event boundary. Event order, turn,
name, and title are ambiguous when workflows run concurrently. The minimum
repair is live-only typed provenance on the existing events.

### Repair Track

1. Add `workflow_origin: Option<WorkflowExecutionOrigin>` to
   `DelegateStarted`, `DelegateUpdated`, `DelegateProgressUpdated`,
   `DelegateFinished`, `DelegateSwarmStarted`, `DelegateSwarmUpdated`,
   `DelegateSwarmProgressUpdated`, `DelegateSwarmFinished`, and
   `QuestionRequested`.
2. Mark each field `#[serde(skip)]` and `#[schemars(skip)]`. Deserialization must
   produce `None`; serialized JSON must remain byte-for-byte free of the field.
3. Update every constructor and compact-progress reconstruction to initialize
   the field with `None` outside workflow forwarding.
4. Extend `stamp_workflow_origin` to fill these variants only when their field
   is empty. Preserve any already-stamped value.
5. Keep the existing `run_id`, `phase_id`, and `invocation_id`; do not add a
   second lineage type or correlate through a queue.
6. Add unit coverage for all stamped variants and for live-only JSON behavior.

### Retirement Track

- Retire turn/name/order inference before it is introduced.
- Do not persist the new fields and do not add a session migration.

### Impact And Compatibility

- Rust constructors and pattern matches change; wire JSON does not.
- Non-workflow event construction remains `workflow_origin: None`.

### Verification

```bash
cargo test --package neo-agent-core --lib events::tests::delegate_workflow_origin_is_live_only -- --exact --nocapture
cargo test --package neo-agent-core --lib runtime::tool_dispatch::tests::stamp_workflow_origin_covers_delegate_families -- --exact --nocapture
```

### Commit

```bash
git add crates/neo-agent-core/src/events.rs crates/neo-agent-core/src/runtime/tool_dispatch.rs crates/neo-agent-core/src/session/event_persistence.rs
git commit -m "fix(core): preserve live workflow child origin"
```

## Task 2: Make One Workflow Entry Own Its Typed Activity

### Files

- modify `crates/neo-tui/src/transcript/tool_call.rs`;
- modify `crates/neo-tui/src/transcript/workflow_card.rs`;
- modify `crates/neo-tui/src/transcript/progressive.rs`;
- modify `crates/neo-tui/src/transcript/store.rs`;
- modify `crates/neo-tui/src/transcript/event_handler.rs`;
- modify `crates/neo-tui/tests/workflow_transcript.rs`.

### Why And Change Necessity

The current store already updates one workflow entry by run ID, but it also
captures mutable transitions as history and creates workflow tools as unrelated
entries. The minimum stable repair is to keep typed direct tools and child
snapshots on the existing workflow entry.

### Repair Track

1. Add `workflow_origin: Option<WorkflowExecutionOrigin>` to
   `ToolCallComponent`, set it at queue/start, expose a read-only accessor, and
   reject a later different `run_id` or `invocation_id` without reparenting.
2. Extend `WorkflowCardComponent` with first-seen-order vectors for direct
   `ToolCallComponent`, `AgentSnapshot`, and `SwarmSnapshot`. Upsert tools by
   tool-call ID, delegates by agent ID, and swarms by swarm ID.
3. Apply compact progress with the existing `apply_agent_progress` and
   `apply_swarm_child_progress` functions; do not duplicate progress merging.
4. Route origin-bearing queue/start/update/finish events by `run_id` into the
   workflow entry. `ToolExecutionQueueUpdated` and shell events resolve the
   already-stored tool-call ID, so they do not need another origin map.
5. Route origin-bearing Delegate-family events by `run_id`. Use
   `invocation_id` to absorb the corresponding `Delegate` or `DelegateSwarm`
   placeholder. If the child never starts, retain the failed placeholder as a
   direct failure row in the main card.
6. Keep origin-free events on their current top-level path unchanged.
7. If an origin-bearing event arrives before its workflow entry, show one
   bounded workflow-presentation error, log the typed run and invocation IDs,
   retain no live orphan, and cover the invariant with a focused test. Do not
   expose internal IDs, create a pending queue, or silently reinterpret it as a
   normal tool.
8. Delete `ProgressiveFactId::WorkflowTransition`,
   `ProgressiveFactPayload::WorkflowTransition`, `WorkflowTransitionFact`, its
   renderer branch, `capture_workflow_transition`, and both calls from
   `upsert_workflow`.
9. Keep `WorkflowCardComponent::accepts_projection` and
   `projection_sequence` rejection unchanged; sequence is a stale-update
   watermark, not a history identity.

### Retirement Track

- Delete mutable workflow history facts completely.
- Delete the workflow-origin top-level tool/child normal path rather than hide a
  duplicate card after rendering.
- Preserve Delegate-family progressive facts only for non-workflow entries.

### Impact And Compatibility

- Presentation-only memory changes; no runtime or persistence migration.
- Explicit review reads full workflow activity from the same workflow entry.

### Verification

```bash
cargo test --package neo-tui --test workflow_transcript workflow_updates_stay_in_one_live_entry_without_transition_history -- --exact --nocapture
cargo test --package neo-tui --test workflow_transcript workflow_origin_routes_tools_and_children_into_one_entry -- --exact --nocapture
```

### Commit

```bash
git add crates/neo-tui/src/transcript/tool_call.rs crates/neo-tui/src/transcript/workflow_card.rs crates/neo-tui/src/transcript/progressive.rs crates/neo-tui/src/transcript/store.rs crates/neo-tui/src/transcript/event_handler.rs crates/neo-tui/tests/workflow_transcript.rs
git commit -m "fix(tui): group typed workflow activity"
```

## Task 3: Render The Bounded Workflow Main Card

### Files

- modify `crates/neo-tui/src/transcript/workflow_card.rs`;
- create `crates/neo-tui/src/transcript/workflow_group.rs`;
- modify `crates/neo-tui/src/transcript/mod.rs`;
- modify `crates/neo-tui/tests/workflow_transcript.rs`.

### Why And Change Necessity

The main card must show current workflow truth and direct activity without
becoming an audit log. Existing full tool cards are too tall and would recreate
the reported frame pressure.

### Repair Track

1. Keep the current header state, phase, elapsed, counts, report, terminal
   reason, and actionable wait/pause text.
2. Project direct activity in this priority: active or queued tools, failed
   tools, newest relevant completed tools, latest report, then a real omitted
   count. Use existing tool header/summary helpers; never include complete tool
   output in the normal card.
3. Truncate every rendered line with the effective content width after ANSI
   styling. Long commands, paths, reports, and wide characters must fit.
4. Implement `workflow_group.rs` as a pure row-budget composer. It receives the
   current workflow entry and available rows, returns main and optional sibling
   blocks, and owns no retained state.
5. Degrade the main card by approved priority until it reaches one header plus
   one actionable row; at one available row, keep only the header with observed
   counts.
6. Keep elapsed animation only while the workflow or visible child activity is
   non-terminal.
7. Keep the existing full-entry render path for explicit `Ctrl+O` review; the
   normal terminal path alone receives a row budget. Both views read the same
   workflow entry.

### Retirement Track

- Do not call the generic top-level tool-group renderer for workflow-origin
  tools.
- Do not add an internal scroll area, expansion state, or configurable height.

### Impact And Compatibility

- Existing workflow lifecycle wording remains typed and current.
- Full activity remains available through explicit review from the same entry.

### Verification

```bash
cargo test --package neo-tui --test workflow_transcript workflow_main_card_bounds_direct_tools_and_long_content -- --exact --nocapture
```

### Commit

```bash
git add crates/neo-tui/src/transcript/workflow_card.rs crates/neo-tui/src/transcript/workflow_group.rs crates/neo-tui/src/transcript/mod.rs crates/neo-tui/tests/workflow_transcript.rs
git commit -m "feat(tui): render bounded workflow main card"
```

## Task 4: Add Dedicated Workflow Delegate And Swarm Summaries

### Files

- create `crates/neo-tui/src/transcript/workflow_delegate_card.rs`;
- create `crates/neo-tui/src/transcript/workflow_swarm_card.rs`;
- modify `crates/neo-tui/src/transcript/workflow_group.rs`;
- modify `crates/neo-tui/src/transcript/mod.rs`;
- modify `crates/neo-tui/tests/workflow_transcript.rs`;
- verify without layout edits in `delegate_card.rs`, `delegate_group.rs`, and
  `swarm_card.rs`.

### Why And Change Necessity

The approved UI needs one row per workflow child without nesting full existing
cards. Two dedicated pure renderers are the smallest implementation that keeps
Delegate and swarm identity and counts unambiguous.

### Repair Track

1. Render one Delegate sibling header per workflow run with observed running,
   queued, failed, and completed counts.
2. Render one row per visible agent using typed agent ID, display name, role,
   lifecycle text, current tool or terminal outcome, and elapsed time when it
   fits. Display name is never the identity key.
3. Render one swarm sibling header per workflow run. Flatten each swarm child to
   one row keyed by swarm ID plus item index and agent ID; prefix the swarm label
   only when more than one swarm would otherwise be ambiguous.
4. Preserve first-seen child order at normal height. Under row pressure retain
   failed, running, queued, then completed rows, followed by exact omitted
   counts. Do not impose a fixed product child limit.
5. At minimum height, collapse each sibling card to one header/count row; if the
   whole group has one row, fold both counts into the main header.
6. Use the shared child status, role badge, elapsed, and visible-width helpers;
   do not copy full Delegate-family render code.
7. Assert that origin-free Delegate, DelegateGroup, and DelegateSwarm events
   still produce their existing card types and never enter a workflow summary.

### Retirement Track

- Workflow-origin full Delegate-family cards are removed from the normal path.
- Existing non-workflow card files and behavior are retained, not forked.

### Impact And Compatibility

- No nested cards, expansion state, or duplicate child transcript.
- Every child remains available in explicit review from the workflow entry.

### Verification

```bash
cargo test --package neo-tui --test workflow_transcript workflow_child_summaries_use_two_sibling_cards_and_one_row_per_agent -- --exact --nocapture
cargo test --package neo-tui --test workflow_transcript non_workflow_delegate_family_cards_remain_unchanged -- --exact --nocapture
```

### Commit

```bash
git add crates/neo-tui/src/transcript/workflow_delegate_card.rs crates/neo-tui/src/transcript/workflow_swarm_card.rs crates/neo-tui/src/transcript/workflow_group.rs crates/neo-tui/src/transcript/mod.rs crates/neo-tui/tests/workflow_transcript.rs
git commit -m "feat(tui): add workflow child summaries"
```

## Task 5: Keep One Logical Group Live And Commit It Once

### Files

- modify `crates/neo-tui/src/transcript/presentation.rs`;
- modify `crates/neo-tui/src/transcript/workflow_group.rs`;
- modify `crates/neo-tui/src/transcript/workflow_card.rs`;
- modify `crates/neo-tui/src/shell/stream.rs`;
- modify `crates/neo-tui/src/shell/event_router.rs`;
- modify `crates/neo-tui/src/transcript/entry/mod.rs`;
- modify `crates/neo-tui/src/transcript/pane.rs`;
- modify `crates/neo-tui/tests/workflow_transcript.rs`.

### Why And Change Necessity

Correct card rendering is insufficient if presentation still commits mutable
updates or emits the launch-position entry as a second terminal card. The
existing acknowledgement owner must treat the main and sibling blocks as one
logical workflow group.

### Repair Track

1. While workflow state is non-terminal, emit only the bounded live group and
   no `FinalizedBlock` or progressive workflow fact.
2. Allow unrelated later finalized entries to enter native history while the
   workflow group remains live; the workflow launch position must not become a
   history barrier.
3. On the first accepted terminal snapshot, project the same workflow entry as
   one final group containing the main card and both optional summaries. Use one
   `FinalizedBlockProof` tied to the workflow entry ID and terminal revision.
4. Queue that block at terminalization time after already pending earlier
   finalized blocks. Do not append a second workflow entry or preserve a start
   row. A successful terminal write acknowledges the group once; a failed write
   leaves it retryable.
5. Remove the live group in the same presentation update that offers the final
   block, so live and final forms are never simultaneously visible.
6. Carry the question event's live-only origin through `StreamUpdate` into
   `QuestionPromptData`. Keep approval origin in its existing `ApprovalRequest`.
   Neither entry is absorbed into the workflow group or changes visible wording.
7. Keep approvals and questions as independent barrier entries. The earliest
   unresolved one remains visible and owns input even when it originated from
   the workflow.
8. Add a sequence regression: start workflow, finalize an unrelated row, commit
   it, update workflow several times, finish workflow, and assert history order
   is unrelated row then one terminal workflow group.

### Retirement Track

- Delete any presentation branch that renders a workflow-origin ToolRun,
  Delegate, or DelegateSwarm as an independent block.
- Do not create a second terminal workflow card or terminal-order queue.

### Impact And Compatibility

- Reuses the current terminal write/flush/acknowledge transaction.
- Explicit review can still render the complete stored workflow entry.

### Verification

```bash
cargo test --package neo-tui --test workflow_transcript workflow_group_commits_once_at_terminal_event_position -- --exact --nocapture
cargo test --package neo-tui --test workflow_transcript workflow_group_keeps_earliest_blocking_input_owner -- --exact --nocapture
```

### Commit

```bash
git add crates/neo-tui/src/transcript/presentation.rs crates/neo-tui/src/transcript/workflow_group.rs crates/neo-tui/src/transcript/workflow_card.rs crates/neo-tui/src/shell/stream.rs crates/neo-tui/src/shell/event_router.rs crates/neo-tui/src/transcript/entry/mod.rs crates/neo-tui/src/transcript/pane.rs crates/neo-tui/tests/workflow_transcript.rs
git commit -m "fix(tui): commit workflow group once"
```

## Task 6: Make Final Frame Geometry Exact

### Files

- modify `crates/neo-tui/src/transcript/presentation.rs`;
- modify `crates/neo-tui/src/app.rs`;
- modify `crates/neo-tui/tests/terminal_scrollback.rs`.

### Why And Change Necessity

The current live budget omits separators that composition later inserts. The
application then truncates from the tail after appending Todo, composer, and
footer. Geometry must be correct before the frame reaches the terminal writer.

### Repair Track

1. Define live block cost as visible content rows plus a separator only when the
   composed block has a preceding visible block and `separator_before` is true.
2. Use that same cost in total-fit checks, reverse selection, partial newest
   block selection, and omitted-row summary insertion.
3. Keep `fit_chrome_to_height(render_chrome(...), height)` as the bottom-region
   owner. Pass its actual fitted row count to transcript presentation before
   composing live rows.
4. Require `update.live.len() + chrome_render.lines.len() <= height` before
   `append_chrome`. Preserve the returned cursor within the same bound.
5. Delete the normal-path `update.live.truncate(height)` after chrome append.
   Replace it with a debug assertion that catches violated budgeting during
   development but never deletes the input region.
6. Extend the terminal regression to apply repeated workflow projections,
   origin-bearing direct tools, Delegate and swarm updates, Todo, composer,
   footer, and an approval/question barrier across multiple heights and widths.
7. At every progress frame assert: no alternate screen, no mouse capture,
   visible width at most terminal width, live height at most terminal height,
   cursor in bounds, and the active bottom/input sentinels present exactly once.
8. After terminalization assert one final workflow group enters native
   scrollback and the bottom region does not jump or reappear from a previously
   truncated state.

### Retirement Track

- Tail truncation is deleted, not retained as a defensive fallback.
- No second viewport, overflow latch, or minimum-terminal mode is added.

### Impact And Compatibility

- Full-screen overlays retain their existing independent height fitting.
- Ordinary progress remains on the normal screen.

### Verification

```bash
cargo test --package neo-tui --test terminal_scrollback workflow_group_progress_preserves_bottom_region_and_native_history -- --exact --nocapture
cargo test --package neo-tui --lib dialogs::choice_picker::tests::rendered_lines_fit_terminal_width -- --exact --nocapture
```

### Commit

```bash
git add crates/neo-tui/src/transcript/presentation.rs crates/neo-tui/src/app.rs crates/neo-tui/tests/terminal_scrollback.rs
git commit -m "fix(tui): preserve workflow frame geometry"
```

## Task 7: Verify Retirement And Record The Landed Decision

### Files

- modify `docs/aegis/adr/ADR-0010-native-terminal-transcript-presentation.md`;
- modify `docs/aegis/baseline/2026-07-31-native-terminal-transcript.md`;
- modify `docs/aegis/INDEX.md` only if a new completion record is added.

### Why And Change Necessity

The earlier landed baseline explicitly treated accepted workflow transitions as
history. Completion must retire that rule and record the new single-group
behavior so future work does not restore the defect.

### Repair Track

1. Run the exact tests from Tasks 1-6 and record only fresh results.
2. Run a lingering-reference search. Expected result: no source references to
   `WorkflowTransitionFact`, `ProgressiveFactId::WorkflowTransition`, or
   `capture_workflow_transition`; no ordinary frame tail truncation after
   `append_chrome`.
3. Confirm `delegate_card.rs`, `delegate_group.rs`, and `swarm_card.rs` have no
   layout changes unless a focused failure proved a shared helper correction.
4. Amend ADR-0010 with the one-main-plus-two-sibling projection, live-only
   workflow provenance, terminal-only history rule, and exact final-frame cost.
5. Amend the baseline to supersede accepted-transition history with one terminal
   workflow group and reference the implementation commits and focused evidence.
6. Do not claim Windows/Linux native verification unless it was actually run;
   local deterministic Rust evidence is distinct from platform terminal proof.

### Retirement Track

- Delete stale tests and baseline text that require non-terminal workflow
  history rows.
- Classify any gap before repair: missing owner logic goes to the existing
  workflow entry or presentation owner; no compatibility branch is allowed.

### Verification

```bash
rg -n "WorkflowTransitionFact|ProgressiveFactId::WorkflowTransition|capture_workflow_transition" crates/neo-tui/src crates/neo-tui/tests
rg -n -U "append_chrome[\\s\\S]{0,240}truncate\\(height\\)" crates/neo-tui/src/app.rs
rustfmt --edition 2024 --check crates/neo-agent-core/src/events.rs crates/neo-agent-core/src/runtime/tool_dispatch.rs crates/neo-agent-core/src/session/event_persistence.rs crates/neo-tui/src/transcript/tool_call.rs crates/neo-tui/src/transcript/workflow_card.rs crates/neo-tui/src/transcript/workflow_group.rs crates/neo-tui/src/transcript/workflow_delegate_card.rs crates/neo-tui/src/transcript/workflow_swarm_card.rs crates/neo-tui/src/transcript/progressive.rs crates/neo-tui/src/transcript/store.rs crates/neo-tui/src/transcript/event_handler.rs crates/neo-tui/src/transcript/presentation.rs crates/neo-tui/src/app.rs crates/neo-tui/tests/workflow_transcript.rs crates/neo-tui/tests/terminal_scrollback.rs
git diff --check
```

The two `rg` commands are negative checks and must return no matches. Run each
exact Rust test command from Tasks 1-6 separately; do not replace them with a
workspace-wide test run.

### Commit

```bash
git add docs/aegis/adr/ADR-0010-native-terminal-transcript-presentation.md docs/aegis/baseline/2026-07-31-native-terminal-transcript.md docs/aegis/INDEX.md
git commit -m "docs(tui): record workflow transcript presentation"
```

## Risks And Stop Conditions

- If Delegate-family provenance cannot remain live-only without changing
  historical JSON, stop before changing persistence.
- If a workflow child must be correlated by event order, title, turn, or name,
  stop and repair the typed origin at the producer.
- If direct tool activity must exist in both a workflow entry and a top-level
  entry to render correctly, stop and remove the duplicate owner.
- If terminal ordering requires a second workflow card or independent terminal
  queue, stop and repair the existing presentation acknowledgement path.
- If preserving bottom chrome requires post-composition truncation, stop and
  correct block cost or fitted chrome measurement.
- If implementation changes non-workflow card layout, split and justify that
  change separately; it is outside this plan.
- If deterministic local tests pass but native Windows/Linux terminal behavior
  remains untested, report that residual risk explicitly.

## Completion Criteria

Implementation is complete only when all exact tests pass, both negative
searches return no matches, formatting and diff checks pass, old workflow paths
are deleted, decision records match the landed source, and no required work is
left behind a feature flag, fallback, or follow-up placeholder.
