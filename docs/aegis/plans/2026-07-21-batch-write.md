# Neo Batch Write Prepared Execution Implementation Plan

> Executor note: implement the approved design exactly. Do not restart product
> discovery, compare `.references/` projects, add a `BatchWrite` tool, generalize
> the `Tool` trait, retain the old schema, or redesign Delegate-family cards.

## Goal

Replace Neo's direct single-file `Write { path, content }` behavior with the
approved ordered `Write { files: [{ path, content }] }` prepared-execution
contract. Support mixed create/overwrite batches, verified approval, stale
rechecks, per-file atomic installation, truthful partial results, structured
progress/replay, and the approved content-adaptive TUI. Retire every active old
single-file schema and renderer path, and remove the first-path chip from
successful Edit headers.

## Architecture

Add the smallest established parallel to Batch Edit:

```text
write.rs PreparedWrite
      ↓
PreparedExecution::Write(Arc<PreparedWrite>)
      ↓
runtime instruction preflight -> prepare -> approval -> recheck -> commit
      ↓
ToolExecutionUpdate(write_prepared/write_progress) + terminal ToolResult
      ↓
write_tool_presentation.rs -> ToolCallComponent / inline approval
      ↓
bounded existing Delegate activity projection
```

`write.rs` owns all Write bytes and status semantics. Runtime modules only
route prepared state and authorization. `ToolCallComponent` remains the only
stateful TUI card owner. The new pure Write renderer reuses current atomic file,
diff, syntax-highlighting, wrapping, and code-frame primitives.

## Tech Stack

- Rust 2024, minimum Rust 1.96.1
- `serde`, `serde_json`, `schemars`
- `tokio`, `tokio_util::sync::CancellationToken`
- existing `sha2`, `similar`, `uuid`, and filesystem helpers already used by
  Batch Edit and atomic writes
- `neo-agent-core` runtime/tool/approval/session owners
- `neo-tui` transcript and primitive rendering owners
- `cargo test`/`cargo nextest` focused tests, `rustfmt`, `rg`, and
  `git diff --check`

## Baseline And Authority Refs

- `AGENTS.md`
- `docs/aegis/specs/2026-07-21-batch-write-design.md` (approved requirement
  and architecture authority)
- `docs/aegis/specs/2026-07-20-batch-edit-design.md` (implementation precedent,
  not a license to copy unrelated Edit semantics)
- `docs/aegis/specs/2026-07-17-canonical-approval-protocol-design.md`
- `docs/aegis/specs/2026-07-17-path-scoped-agents-instructions-design.md`
- current source named in the file map below

## Baseline Usage Draft

- Required baseline refs: the approved Batch Write design, AGENTS rules,
  canonical approval protocol, path-scoped instructions, Batch Edit precedent.
- Acknowledged before plan refs: all required refs above.
- Cited in plan refs: all required refs above.
- Missing refs: none.
- Decision: continue.

## Requirement Ready Check

- Requirement source refs: approved Batch Write design plus explicit user
  choices recorded in that design.
- Goals and scope refs: design Goal, Task Intent, Canonical Ownership, and
  Compatibility And Retirement sections.
- User/scenario refs: one coherent Write call creates and overwrites multiple
  files with inspectable approval and truthful partial failure.
- Requirement item refs: eighteen Approved Product Decisions.
- Acceptance refs: design Verification And Acceptance Criteria.
- Open blocker questions: none.
- Decision: ready.

## Compatibility Boundary

Hard break, canonical-only:

```json
{"files":[{"path":"src/a.rs","content":"fn main() {}\n"}]}
```

The old top-level `path`/`content` schema is invalid. Do not add a decoder,
alias, adapter, fallback, feature flag, deprecation period, or dual renderer.
Migrate all internal callers and current docs in the same workstream. Historical
`docs/aegis/specs/` and `docs/aegis/plans/` remain historical evidence and are
not rewritten merely because they quote the former contract.

Stable boundaries:

- `Read` and `Edit` tool responsibilities remain unchanged;
- permission modes Ask/Auto/Yolo retain their meanings;
- existing `AgentEvent` variants remain unchanged;
- non-Write scheduling behavior remains unchanged;
- Delegate/DelegateGroup/DelegateSwarm layouts and expansion semantics remain
  byte-for-byte behaviorally equivalent outside bounded summary text;
- unfinished prepared execution is never resumed from replay.

## TDD Route

- Mode: off.
- Decision: skipped.
- Strict authority: not applicable; neither user nor project requires strict
  test-first TDD.
- Test posture: minimum implementation followed by focused regression tests at
  each owner boundary.
- Reason: this is an approved feature/contract migration, not an explicit strict
  TDD task; AGENTS.md requires proportional exact verification.
- Verification: every task names exact package, target, and test filter; no
  broad workspace test run is evidence.

## Change Necessity

- User-visible need: one Write call must safely create/overwrite multiple files
  with real approval and truthful partial state.
- No-change/docs-only option: cannot change the model schema, direct write
  behavior, approval bytes, atomicity, or TUI state.
- Minimum code boundary: canonical Write engine, existing prepared runtime and
  permission seams, pure Write presentation, bounded child projection, paired
  current docs.
- Decision: code-change.

## Existence Check

### `PreparedWrite`

- Existing reuse candidate: `PreparedEdit` pattern and `PreparedExecution`.
- Why insufficient directly: Write has distinct absent-target, parent creation,
  create-new atomicity, content projection, and operation semantics.
- Creation proof: runtime must carry the exact bytes/state approved by the user
  through stale recheck and commit.
- Decision: add-with-proof as a private `write.rs` owner and one narrow enum
  variant; do not create a generic prepared-tool framework.

### `WriteApprovalPresentation`

- Existing reuse candidate: typed `EditApprovalPresentation` and generic Tool
  presentation.
- Why insufficient directly: generic raw argument details cannot express
  verified create/overwrite previews; Edit replacement fields are wrong.
- Creation proof: Ask mode must approve prepared bytes and operation identity.
- Decision: add-with-proof as a typed approval projection.

### `write_tool_presentation.rs`

- Existing reuse candidate: generic single-file Write functions in
  `tool_renderers.rs` and pure `edit_tool_presentation.rs`.
- Why insufficient directly: batch created-content and overwrite-diff state
  would overload the already shared generic renderer.
- Creation proof: one pure owner is needed for streaming, prepared, success,
  partial, interrupted, and approval states.
- Decision: add-with-proof, then delete the old Write renderer functions.

### Recorded parent-directory creation

- Existing reuse candidate: `session/atomic_file.rs` safe directory and atomic
  create/replace helpers.
- Why insufficient directly: current helper does not return directories created
  before a later error, so results could not report exact remaining side effects.
- Creation proof: the approved contract requires explicit remaining directory
  data and forbids ambiguous zero-mutation wording.
- Decision: extend the existing atomic owner narrowly; do not create a
  transaction/journal subsystem.

## Architecture Integrity Lens

- Invariant: the bytes approved from `PreparedWrite` are the bytes committed.
- Canonical owner: `write.rs` for Write semantics; runtime for authorization;
  transcript component for state; pure Write module for rows.
- Responsibility overlap: old raw-argument Write approval and renderer paths
  must be removed as their replacements land.
- Higher-level simplification: reuse the prepared enum/events/helpers; no new
  trait, event family, tool, or global file-mutation abstraction.
- Retirement/falsifier: any implementation requiring old schema acceptance,
  generic Tool trait changes, automatic rollback, a persistent journal, or a
  second stateful UI owner falsifies this plan and must stop for design review.
- Verdict: aligned.

## Anti-Entropy Declaration

- Deletion class: internal contract-carrying code.
- Old path: single-file Write schema, raw approval/scope derivation,
  first-path-only probes/guards, single-file renderer, and stale fixtures/docs.
- New canonical owner: `files[]` plus `PreparedWrite` and typed Write
  presentation.
- Preserved behavior: create/complete-overwrite UTF-8 files, workspace policy,
  permission modes, plan-file special case, syntax-highlighted preview.
- Retired behavior: top-level `path`/`content`, overwrite of non-UTF-8 bytes,
  direct non-atomic writes, guessed approval, and path-bearing success headers.
- External boundary touched: model-visible tool schema, intentionally hard
  replaced by explicit user decision.
- Source-of-truth data risk: none; this plan deletes code paths, not user data.
- User confirmation required: no.
- Retirement path: delete-first.

## Ripple Signal Triage

The schema and prepared payload affect all of these consumers and therefore all
must be migrated in the same workstream:

- tool description/schema and direct registry execution;
- atomic file/parent handling;
- instruction scope probes;
- plan-mode guard;
- runtime preparation/recheck/execution;
- permission subject, typed approval, session scope, cancellation;
- ordinary transcript streaming/update/terminal/replay;
- inline approval rendering;
- Delegate/Swarm activity summaries;
- current integration fixtures and bilingual tool docs.

No provider adapter, session schema migration, goal model, shell scheduler, or
reference implementation is in scope.

## Complexity Budget

- Artifact class: shared core runtime plus transcript presentation.
- Current pressure: `tool_dispatch.rs`, `permission.rs`,
  `multi_agent/runtime.rs`, `transcript/entry/mod.rs`, and
  `tool_renderers.rs` are already large mixed owners.
- Projected pressure: over-budget if Write validation/rows are implemented
  inline in shared routers.
- Budget result: within-budget only with semantics in `write.rs`, rows in one
  new pure presentation file, and routing-only edits in shared modules.
- Governance: no unrelated refactor; no generic trait; keep tests focused and
  avoid duplicate cosmetic cases.

## Plan-Time Complexity Check

- `write.rs`: grows substantially but remains the correct semantic owner, just
  as `edit.rs` owns Batch Edit.
- `atomic_file.rs`: add only recorded safe-directory creation needed by Write;
  retain existing atomic algorithms.
- runtime/permission/entry/multi-agent shared files: wiring and small projections
  only; do not add byte-level Write logic.
- `tool_renderers.rs`: route headers/body to the new module and delete old Write
  functions, producing a net responsibility reduction.
- Recommendation: edit owners in place; add only
  `write_tool_presentation.rs`; split tasks by owner boundary.

## File Map

### Create

- `crates/neo-tui/src/transcript/write_tool_presentation.rs` — pure structured
  Write intent/prepared/approval/terminal renderer.

### Core Modify

- `crates/neo-agent-core/src/tools/write.rs`
- `crates/neo-agent-core/src/tools/mod.rs`
- `crates/neo-agent-core/src/session/atomic_file.rs`
- `crates/neo-agent-core/src/approval.rs`
- `crates/neo-agent-core/src/lib.rs` — re-export the typed Write approval
  projection used by `neo-tui`
- `crates/neo-agent-core/src/runtime/tool_arguments.rs`
- `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
- `crates/neo-agent-core/src/runtime/permission.rs`
- `crates/neo-agent-core/src/mode/plan_mode_guard.rs`
- `crates/neo-agent-core/src/multi_agent/runtime.rs`

### TUI Modify

- `crates/neo-tui/src/transcript/mod.rs`
- `crates/neo-tui/src/transcript/tool_renderers.rs`
- `crates/neo-tui/src/transcript/edit_tool_presentation.rs`
- `crates/neo-tui/src/transcript/event_handler.rs`
- `crates/neo-tui/src/transcript/entry/mod.rs`

### Tests Modify

- `crates/neo-agent-core/tests/tool_files.rs`
- `crates/neo-agent-core/tests/tool_permissions.rs`
- `crates/neo-agent-core/tests/runtime_turn.rs`
- unit tests colocated in `write.rs`, `atomic_file.rs`, `tool_arguments.rs`,
  `permission.rs`, and `multi_agent/runtime.rs`
- `crates/neo-tui/tests/tool_cards.rs`
- `crates/neo-tui/tests/transcript_pane.rs`
- `crates/neo-tui/tests/transcript_store.rs`
- `crates/neo-tui/tests/terminal_frame.rs`
- `crates/neo-tui/tests/multi_agent_transcript.rs`

### Current Docs Modify

- `docs/en/reference/tools.md`
- `docs/zh/reference/tools.md`

## Task 1: Implement The Canonical Batch Write Engine

**Files**

- Modify `crates/neo-agent-core/src/tools/write.rs`
- Modify `crates/neo-agent-core/src/tools/mod.rs`
- Modify `crates/neo-agent-core/src/session/atomic_file.rs`
- Modify `crates/neo-agent-core/src/approval.rs`
- Modify `crates/neo-agent-core/src/lib.rs`
- Modify `crates/neo-agent-core/tests/tool_files.rs`

**Why**

The current direct single-file write cannot validate a complete batch, approve
the installed bytes, prevent stale overwrite, or report partial state.

**Change Necessity**

Source changes are required at the canonical Write and atomic owners. Runtime
or TUI work must not compensate for missing byte semantics.

**Implementation steps**

1. Replace `WriteInput { path, content }` with unknown-field-rejecting
   `WriteInput { files: Vec<WriteFileInput> }` and
   `WriteFileInput { path: PathBuf, content: String }`.
2. Validate non-empty `files`, non-empty paths, duplicate effective targets,
   existing target type/UTF-8, and no-op overwrites. Do not impose semantic
   batch/content limits.
3. Add private `PreparedWrite`, ordered `PreparedWriteFile`, explicit
   `WriteOperation::{Created, Overwritten}`, and fingerprint types. Store only
   data required by the approved spec.
4. Implement side-effect-free `PreparedWrite::prepare`,
   `approval_presentation`, `session_approval_scope`,
   `all_resolved_targets_match`, `prepared_update`, `recheck_all`,
   `cancelled_before_commit_result`, and ordered `commit` methods following
   the established `PreparedEdit` method boundaries.
5. Keep `WriteTool::execute` valid for direct registry execution by performing
   prepare -> recheck -> commit itself, as canonical `EditTool::execute` does.
6. Extend `atomic_file.rs` with the narrow safe parent-creation operation needed
   to return paths Neo actually created, including paths created before an
   error. Reuse `write_file_atomic_create_new` and
   `replace_existing_file_atomic_status`; do not copy their algorithms.
7. For created files call the create-new helper only after the JIT absent check.
   For overwritten files call the strict existing replacement helper only after
   the JIT fingerprint check.
8. Emit stable `kind: write` result details with separate `operation` and
   `status`, applied-only aggregate stats on failure, ordered changes, and
   `created_directories`.
9. Add injectable writer/parent-creator seams only inside tests or private
   methods where needed to deterministically prove failure after a prior
   commit. Do not expose them as product abstractions.
10. Migrate current file-tool fixtures to `files[]`; reverse the old
    `write_overwrites_non_utf8_without_diff_preview` expectation so non-UTF-8
    overwrite is rejected without mutation.

**Focused tests**

Add high-signal tests with these exact names:

- `write_batch_mixed_create_overwrite_commits_in_order`
- `write_batch_prepare_rejections_leave_files_and_directories_untouched`
- `write_batch_failure_after_first_commit_reports_partial_without_rollback`
- `write_batch_reports_directories_created_before_install_failure`
- `write_batch_cancellation_before_and_after_first_commit_is_truthful`
- `write_schema_rejects_legacy_and_unknown_fields`

Use table-driven assertions inside the prepare-rejection test for duplicate,
unsafe, non-UTF-8, and no-op cases instead of near-duplicate tests.

Run:

```bash
cargo test --package neo-agent-core --test tool_files -- write_batch_mixed_create_overwrite_commits_in_order --exact --nocapture
cargo test --package neo-agent-core --lib -- tools::write::tests::write_batch_prepare_rejections_leave_files_and_directories_untouched --exact --nocapture
cargo test --package neo-agent-core --lib -- tools::write::tests::write_batch_failure_after_first_commit_reports_partial_without_rollback --exact --nocapture
cargo test --package neo-agent-core --lib -- tools::write::tests::write_batch_reports_directories_created_before_install_failure --exact --nocapture
cargo test --package neo-agent-core --lib -- tools::write::tests::write_batch_cancellation_before_and_after_first_commit_is_truthful --exact --nocapture
cargo test --package neo-agent-core --lib -- tools::write::tests::write_schema_rejects_legacy_and_unknown_fields --exact --nocapture
```

Expected: every command reports one passed test and zero failures.

**Review gate**

Verify prepare performs no writes/directories, created and overwritten are
immutable classifications, applied stats exclude not-attempted content, and no
generic prepared abstraction appeared.

**Commit**

```bash
git add crates/neo-agent-core/src/tools/write.rs crates/neo-agent-core/src/tools/mod.rs crates/neo-agent-core/src/session/atomic_file.rs crates/neo-agent-core/src/approval.rs crates/neo-agent-core/src/lib.rs crates/neo-agent-core/tests/tool_files.rs
git commit -m "feat(tools): prepare batch Write executions"
```

## Task 2: Route Prepared Write Through Runtime, Approval, And Guards

**Files**

- Modify `crates/neo-agent-core/src/runtime/tool_arguments.rs`
- Modify `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
- Modify `crates/neo-agent-core/src/runtime/permission.rs`
- Modify `crates/neo-agent-core/src/mode/plan_mode_guard.rs`
- Modify `crates/neo-agent-core/tests/runtime_turn.rs`
- Modify `crates/neo-agent-core/tests/tool_permissions.rs`

**Why**

The approved projection must be the committed payload, every target must pass
instruction/permission guards, and plan mode must not accidentally approve a
multi-target batch.

**Change Necessity**

Raw first-path consumers cannot represent a prepared target set. The minimum
runtime change is one enum variant plus routing in established preparation,
recheck, authorization, and execution functions.

**Implementation steps**

1. Import `PreparedWrite` and add
   `PreparedExecution::Write(Arc<PreparedWrite>)`; update pointer equality and
   comments without changing public serialization.
2. Extend `InstructionScopeProbe::from_prepared_tool` with a typed Write helper
   that collects every `files[].path` parent. Do not fall back to top-level
   `path` or parse shell strings.
3. Add `ToolRegistry::has_canonical_prepared_write` so custom tools named
   `Write` stay on direct registry execution, matching the existing canonical
   Edit guard.
4. In `tool_dispatch.rs`, prepare canonical Write calls after instruction
   preflight and before authorization, recheck authorized prepared Write calls,
   and execute `PreparedWrite::commit` with existing update emission. Keep
   Write scheduling `Exclusive`.
5. Prefer small paired functions (`prepare_file_mutation_calls` only if it
   reduces branching without moving semantics) or explicit Edit/Write routing;
   do not introduce a general trait.
6. Extend `ApprovalPresentation` use in permission building so
   `PreparedExecution::Write` produces the typed Write presentation and
   cancellation returns the prepared zero-install result.
7. Replace raw Write session scope derivation with
   `PreparedWrite::session_approval_scope`. Offer the session option only for
   the complete workspace-contained key set.
8. Update permission subject/title to batch wording; do not expose a first path
   as the batch identity.
9. Update `check_plan_file_write` and `mode/plan_mode_guard.rs`: only a prepared
   one-target Write matching the active plan path gets the special plan grant;
   any additional target is denied.
10. Ensure Ask opens only after successful prepare; Auto/Yolo skip the dialog
    but retain prepare/recheck/commit.
11. Migrate all active runtime and permission fixtures from top-level fields to
    one-element `files[]` unless the test intentionally asserts legacy rejection.

**Focused tests**

Add or migrate exact tests:

- `typed_scope_probes_cover_every_write_parent`
- `runtime_write_approval_uses_verified_batch_projection`
- `runtime_write_stale_existing_and_appeared_target_install_nothing`
- `runtime_write_emits_prepared_and_ordered_progress_updates`
- `runtime_plan_mode_allows_only_single_active_plan_write_target`
- `runtime_write_session_scope_requires_complete_prepared_target_set`
- `noncanonical_write_calls_stay_on_direct_registry_execution`

Run:

```bash
cargo test --package neo-agent-core --lib -- runtime::tool_arguments::tests::typed_scope_probes_cover_every_write_parent --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn -- runtime_write_approval_uses_verified_batch_projection --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn -- runtime_write_stale_existing_and_appeared_target_install_nothing --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn -- runtime_write_emits_prepared_and_ordered_progress_updates --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn -- runtime_plan_mode_allows_only_single_active_plan_write_target --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn -- runtime_write_session_scope_requires_complete_prepared_target_set --exact --nocapture
cargo test --package neo-agent-core --lib -- runtime::tool_dispatch::tests::noncanonical_write_calls_stay_on_direct_registry_execution --exact --nocapture
```

Expected: one passed test per command, no approval before successful prepare,
and no write before approval/recheck.

**Review gate**

Trace one Ask-mode mixed batch end to end. Confirm the same `Arc<PreparedWrite>`
produces approval and commit; no raw-argument Write permission owner remains.

**Commit**

```bash
git add crates/neo-agent-core/src/runtime/tool_arguments.rs crates/neo-agent-core/src/runtime/tool_dispatch.rs crates/neo-agent-core/src/runtime/permission.rs crates/neo-agent-core/src/mode/plan_mode_guard.rs crates/neo-agent-core/tests/runtime_turn.rs crates/neo-agent-core/tests/tool_permissions.rs
git commit -m "feat(runtime): authorize prepared batch Write"
```

## Task 3: Implement The Structured Write And Approval TUI

**Files**

- Create `crates/neo-tui/src/transcript/write_tool_presentation.rs`
- Modify `crates/neo-tui/src/transcript/mod.rs`
- Modify `crates/neo-tui/src/transcript/tool_renderers.rs`
- Modify `crates/neo-tui/src/transcript/edit_tool_presentation.rs`
- Modify `crates/neo-tui/src/transcript/event_handler.rs`
- Modify `crates/neo-tui/src/transcript/entry/mod.rs`
- Modify `crates/neo-tui/tests/tool_cards.rs`
- Modify `crates/neo-tui/tests/transcript_pane.rs`
- Modify `crates/neo-tui/tests/terminal_frame.rs`

**Why**

Users must inspect verified created content and overwrite diffs, see exact
terminal state, and use one coherent visual language across Write and Edit.

**Change Necessity**

The current single-file parser cannot render a batch or prepared operations.
The minimum stable boundary is one pure Write presentation module plus routing
and typed approval integration.

**Implementation steps**

1. Add `write_tool_presentation.rs` with one input struct parallel to
   `EditRenderInput` and pure functions for streaming intent, prepared,
   approval, success, partial/failure, and interrupted states.
2. Created projections render complete final content using current syntax
   highlighting, line numbers, tab expansion, wrapping, and
   `render_code_frame`.
3. Overwritten projections parse real diffs through the established diff model
   and reuse Edit's line-number/color/cluster behavior without duplicating a
   second diff parser.
4. Upgrade the existing shared `render_code_frame` chrome so its semantic
   `header` is embedded in the top border (`╭─ header ─╮`) rather than emitted as
   the first body row. This is the sole Edit/Write frame-chrome owner.
5. Update `edit_tool_presentation::render_change_frame` to use that titled
   border directly. Prepared Edit renders
   `M path · N replacements · +N -N`; committed Edit renders
   `✓ path · committed · N replacements · +N -N`. Remove the old duplicated
   header body row for approval, committed, and failure states.
6. Fit narrow top-border titles on one row by eliding the path middle while
   preserving marker, filename/deepest tail, terminal status, and `+N/-N`.
   Keep the existing unframed fallback below the minimum valid frame width.
7. Share only small frame/wrap/selection helpers that already have two real
   consumers. Do not move state or build a generic mutation card hierarchy.
8. Implement file-level collapse: at most first two plus last file when more
   than three. Created content retains head/tail; overwrite retains first/last
   clusters. The omission summary is left-aligned and unframed.
9. Route `render_tool_body_with_palette`, streaming preview, and typed inline
   approval to this module. `ToolExecutionUpdate` for Write must store structured
   live details just as Edit does; replace the `is_edit` special case with the
   narrow `matches!(name, "Edit" | "Write")` structured-mutation behavior.
10. Add semantic Write header spans. Success is exactly
   `Used Write · N files · N created · N overwritten · +N -N`; partial/error
   totals are applied-only.
11. Suppress argument-derived `(path)` for structured Write and Edit. Change
   successful Edit header to
   `Used Edit · N files · N replacements · +N -N` for one- and multi-file
   calls.
12. Render created/overwritten operation and committed/failed/not-attempted
   status separately. Do not render planned bodies for not-attempted files.
13. Render remaining `created_directories` from structured details on relevant
    errors.
14. Add `ApprovalPresentation::Write` rendering before generic detail lines.
    Reuse the global expansion flag; do not add a second approval component.
15. Preserve active approval input semantics: composer hidden, digits/arrows
    select, Enter resolves, mouse wheel scrolls transcript, `Ctrl+O` expands.
16. Delete old `parse_write_arguments`, `render_write_body`, and
    `render_write_preview` after all phases route to the new owner. Retain only
    shared highlight/frame helpers with real consumers.
17. Migrate current TUI fixtures to `files[]`; keep one explicit negative test
    that raw legacy arguments do not activate the canonical renderer.

**Focused tests**

Add/migrate exact tests:

- `batch_write_card_renders_created_content_and_overwrite_diff`
- `batch_write_partial_header_uses_committed_totals_only`
- `batch_write_frames_preserve_highlight_line_numbers_clusters_and_narrow_width`
- `batch_write_collapse_keeps_first_two_and_last_file`
- `batch_write_approval_follows_global_expansion`
- `write_and_edit_success_headers_omit_paths_and_color_stats`
- `edit_and_write_file_frames_embed_semantic_headers_in_top_border`
- `streaming_batch_write_uses_unverified_content_preview_without_raw_json`
- retain `approval_mouse_wheel_scrolls_transcript_without_moving_selection`
  in `neo-agent` as a regression boundary; do not duplicate it in TUI.

Run:

```bash
cargo test --package neo-tui --test tool_cards -- batch_write_card_renders_created_content_and_overwrite_diff --exact --nocapture
cargo test --package neo-tui --test tool_cards -- batch_write_partial_header_uses_committed_totals_only --exact --nocapture
cargo test --package neo-tui --test tool_cards -- batch_write_frames_preserve_highlight_line_numbers_clusters_and_narrow_width --exact --nocapture
cargo test --package neo-tui --test tool_cards -- batch_write_collapse_keeps_first_two_and_last_file --exact --nocapture
cargo test --package neo-tui --test transcript_pane -- batch_write_approval_follows_global_expansion --exact --nocapture
cargo test --package neo-tui --test tool_cards -- write_and_edit_success_headers_omit_paths_and_color_stats --exact --nocapture
cargo test --package neo-tui --test tool_cards -- edit_and_write_file_frames_embed_semantic_headers_in_top_border --exact --nocapture
cargo test --package neo-tui --test tool_cards -- streaming_batch_write_uses_unverified_content_preview_without_raw_json --exact --nocapture
cargo test --package neo-agent --bin neo -- modes::interactive::tests::approval_mouse_wheel_scrolls_transcript_without_moving_selection --exact --nocapture
```

Expected: each exact test passes; all rendered lines fit their requested width.

**Manual visual gate**

Render or run deterministic Edit and Write fixtures at wide and narrow widths
and compare to the spec character art. Check that both semantic file headers
live in `╭─ ... ─╮`, no duplicate header body row remains, and path tails,
color spans, line numbers, head/tail omission, global expansion, and selection
stability are preserved. This is a targeted TUI check, not a redesign exercise.

**Commit**

```bash
git add crates/neo-tui/src/transcript/write_tool_presentation.rs crates/neo-tui/src/transcript/mod.rs crates/neo-tui/src/transcript/tool_renderers.rs crates/neo-tui/src/transcript/edit_tool_presentation.rs crates/neo-tui/src/transcript/event_handler.rs crates/neo-tui/src/transcript/entry/mod.rs crates/neo-tui/tests/tool_cards.rs crates/neo-tui/tests/transcript_pane.rs crates/neo-tui/tests/terminal_frame.rs
git commit -m "feat(tui): render prepared batch Write cards"
```

## Task 4: Add Bounded Delegate Summaries And Replay Safety

**Files**

- Modify `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify `crates/neo-tui/tests/transcript_store.rs`
- Modify `crates/neo-tui/tests/multi_agent_transcript.rs`

**Why**

Child activity needs useful batch progress without changing Delegate cards, and
persisted updates must never reconstruct executable prepared state.

**Change Necessity**

Current child argument fallback sees no top-level Write path and current detail
summaries know only Edit structured kinds. A bounded Write projection at the
existing summary owner is the minimum change.

**Implementation steps**

1. Add `summarize_write_arguments` for unverified `files[]` intent. Include
   file count and meaningful first/last path tails under the existing bound; do
   not include contents.
2. Add `summarize_write_details` for `write_prepared`, `write_progress`, and
   terminal `kind: write`. Success includes aggregate operations/stats; partial
   uses committed count and applied stats; errors distinguish zero install,
   partial, and durability uncertainty.
3. Route Write through the existing `summarize_tool_arguments` and
   detail-preference path. Structured updates override raw-argument summaries.
4. Preserve the existing summary length bound and every Delegate-family row,
   layout, ordering, expansion, and output-preview rule.
5. Ensure replayed unfinished Write updates remain visible as interrupted or
   ongoing historical state according to the existing transcript finalization
   contract, but never create `PreparedWrite` or execute a commit.
6. Migrate active replay fixtures to `files[]`; do not rewrite historical Aegis
   documents.

**Focused tests**

Add exact tests:

- `write_tool_summary_prefers_structured_progress_and_terminal_partial`
- `live_write_summary_is_bounded_without_content`
- `replayed_unfinished_write_is_interrupted_and_not_resumed`
- `delegate_card_layout_is_unchanged_by_batch_write_summary`

Run:

```bash
cargo test --package neo-agent-core --lib -- multi_agent::runtime::tests::write_tool_summary_prefers_structured_progress_and_terminal_partial --exact --nocapture
cargo test --package neo-agent-core --lib -- multi_agent::runtime::tests::live_write_summary_is_bounded_without_content --exact --nocapture
cargo test --package neo-agent-core --lib -- multi_agent::runtime::tests::replayed_unfinished_write_is_interrupted_and_not_resumed --exact --nocapture
cargo test --package neo-tui --test multi_agent_transcript -- delegate_card_layout_is_unchanged_by_batch_write_summary --exact --nocapture
```

**Review gate**

Diff rendered Delegate/Swarm fixtures before and after. Only bounded summary
text may change. Verify no content/diff bytes enter child activity summaries.

**Commit**

```bash
git add crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-tui/tests/transcript_store.rs crates/neo-tui/tests/multi_agent_transcript.rs
git commit -m "feat(agent): summarize batch Write progress"
```

## Task 5: Retire Old Contract, Update Current Docs, And Verify

**Files**

- Modify `docs/en/reference/tools.md`
- Modify `docs/zh/reference/tools.md`
- Modify any active source/test fixture identified by the scoped retirement
  scan; do not touch unrelated or historical Aegis documents

**Why**

The new contract is not complete while current docs, fixtures, or runtime owners
still accept or present old single-file Write behavior.

**Repair Track**

- Canonical repair: `files[]`, prepared approval/commit, structured renderer.
- Verification: new main path schema/runtime/TUI tests and paired docs.

**Retirement Track**

- Old owner: top-level Write fields, raw approval/scope, single-path probes,
  direct write, single-file renderer, path-bearing headers.
- Active status after task: deleted.
- Retention reason: none.
- Reintroduction trigger: none; any proven new requirement returns to design,
  not a compatibility patch.

**Implementation steps**

1. Update English and Chinese Write summaries and add a Batch Write
   prepare/approval/stale/commit section parallel in precision to current Edit
   documentation.
2. Document mixed create/overwrite, non-UTF-8 rejection, no-op rejection,
   missing parents, per-file atomicity, no rollback, terminal/per-file statuses,
   created directory reporting, and fresh-call guidance.
3. Update active docs/examples to show only `files[]` when they explicitly
   teach Write arguments. Do not churn unrelated historical plans/specs.
4. Run a scoped active-source scan and remove remaining old owner symbols and
   fixtures. Intentional negative tests may contain old JSON but must assert
   rejection.
5. Review the complete implementation diff against every acceptance criterion
   in the design.
6. Run rustfmt only on touched Rust files, then run focused checks below.

**Retirement scans**

```bash
rg -n "parse_write_arguments|render_write_preview" crates
rg -n 'get\("path"\).*Write|Write.*get\("path"\)' crates/neo-agent-core/src crates/neo-tui/src
rg -n '"Write".*\{"path"|name: "Write".*path' crates/neo-agent-core/tests crates/neo-tui/tests
rg -n 'WriteInput.*path|struct WriteInput' crates/neo-agent-core/src/tools/write.rs
```

Expected:

- removed owner symbols have zero hits;
- raw first-path Write consumers have zero hits;
- old positive fixtures have zero hits;
- one explicit legacy-rejection fixture may remain and is reviewed manually;
- `WriteInput` contains only `files`.

**Fresh verification**

Repeat the highest-signal exact tests from Tasks 1-4, then run:

```bash
cargo check --package neo-agent-core --lib
cargo check --package neo-tui --lib
rustfmt --check --edition 2024 \
  crates/neo-agent-core/src/tools/write.rs \
  crates/neo-agent-core/src/tools/mod.rs \
  crates/neo-agent-core/src/session/atomic_file.rs \
  crates/neo-agent-core/src/approval.rs \
  crates/neo-agent-core/src/lib.rs \
  crates/neo-agent-core/src/runtime/tool_arguments.rs \
  crates/neo-agent-core/src/runtime/tool_dispatch.rs \
  crates/neo-agent-core/src/runtime/permission.rs \
  crates/neo-agent-core/src/mode/plan_mode_guard.rs \
  crates/neo-agent-core/src/multi_agent/runtime.rs \
  crates/neo-agent-core/tests/tool_files.rs \
  crates/neo-agent-core/tests/tool_permissions.rs \
  crates/neo-agent-core/tests/runtime_turn.rs \
  crates/neo-tui/src/transcript/write_tool_presentation.rs \
  crates/neo-tui/src/transcript/mod.rs \
  crates/neo-tui/src/transcript/tool_renderers.rs \
  crates/neo-tui/src/transcript/edit_tool_presentation.rs \
  crates/neo-tui/src/transcript/event_handler.rs \
  crates/neo-tui/src/transcript/entry/mod.rs \
  crates/neo-tui/tests/tool_cards.rs \
  crates/neo-tui/tests/transcript_pane.rs \
  crates/neo-tui/tests/transcript_store.rs \
  crates/neo-tui/tests/terminal_frame.rs \
  crates/neo-tui/tests/multi_agent_transcript.rs
git diff --check
```

Expected:

- exact tests pass;
- both library checks exit zero without new warnings;
- rustfmt exits zero;
- `git diff --check` emits no errors;
- English/Chinese contracts agree;
- the only unrelated worktree change remains untouched.

**ADR backfill assessment**

At implementation completion, compare the landed architecture to this approved
design and the Batch Edit precedent. If it uses only the specified narrow
variant/typed projection/pure renderer, record `no new ADR: approved design is
sufficient authority`. If execution changes owner, public contract, fallback,
or source-of-truth beyond this design, stop and route that divergence through
architecture review before claiming completion.

**Commit**

```bash
git add docs/en/reference/tools.md docs/zh/reference/tools.md
git commit -m "docs(tools): document batch Write contract"
```

If retirement fixes change active source/tests after prior commits, make one
focused `refactor(tools): retire single-file Write paths` commit before the docs
commit. Never amend or bundle unrelated `.gitignore` work.

## Plan Pressure Test

- Owner/contract/retirement: one canonical Write owner and hard schema cut are
  explicit; old paths have a delete-first task and scan.
- Architecture integrity: reuses prepared runtime and event primitives; no
  duplicate tool, trait, or stateful UI owner.
- Verification scope: core, runtime, permission, preflight, TUI, child/replay,
  docs, and retirement each have exact evidence.
- Task executability: file paths, symbols, expected behaviors, commands, review
  gates, and commit boundaries are explicit.
- Pressure result: proceed.

## Execution Readiness View

- Intent Lock: implement the approved Batch Write design exactly and include
  the Edit success-header path removal.
- Scope Fence: named core/TUI/tests/current docs only; no providers, shell,
  session migration, generic tool redesign, or reference-project research.
- Baseline Lock: the approved 2026-07-21 design is canonical; Batch Edit is the
  implementation precedent.
- Approved Behavior: hard `files[]` schema, mixed create/overwrite, UTF-8-only
  existing targets, no-op rejection, missing parents during commit, verified
  approval, stale rechecks, ordered per-file atomic install, no rollback,
  truthful partial state, approved frames/headers.
- Owner/Contract Constraints: `write.rs` owns bytes; runtime owns approval;
  `ToolCallComponent` owns state; one pure Write renderer owns rows.
- Compatibility Boundary: no legacy Write compatibility; non-Write behavior and
  Delegate-family design remain stable.
- Retirement Boundary: delete all active old schema/consumer/renderer paths;
  do not rewrite historical Aegis records.
- Task Batches: core engine; runtime/approval; TUI; Delegate/replay; docs and
  retirement.
- Test Obligations: exact tests named per task plus scoped checks; no broad
  workspace test as completion evidence.
- Review Gates: core semantics after Task 1; approved-payload trace after Task
  2; visual/interaction gate after Task 3; Delegate/replay preservation after
  Task 4; retirement/docs/ADR assessment before completion.
- Drift/Rewind Rules: stop and return to design if a generic prepared trait,
  cross-file rollback, persistent journal, new event, compatibility decoder,
  binary overwrite, or Delegate redesign appears necessary.
- Evidence Required Before Completion: focused pass outputs, visual check,
  old-owner scans, doc parity, rustfmt, library checks, workspace Aegis check,
  cached diff review, and `git diff --check`.
- Advisory Boundary: execution guidance only; not a `GateDecision`,
  `PolicySnapshot`, or completion authority.

## Risks And Controls

| Risk | Control |
|---|---|
| A created target appears after approval | Whole-batch and JIT absent rechecks plus atomic create-new. |
| Existing bytes change after approval | SHA-256/type/path recheck plus strict atomic replacement. |
| Parent creation leaves side effects | Record exact created directories; never claim zero mutation; no unsafe cleanup. |
| Partial commit is shown as success | Failed ToolResult with ordered per-file states and applied-only stats. |
| Approval differs from commit | Same runtime-only `Arc<PreparedWrite>` owns both. |
| Shared routers gain semantics | Routing-only edits; bytes stay in `write.rs`, rows in pure renderer. |
| Old schema survives for tests | Delete-first retirement scans; negative rejection test only. |
| TUI duplicates summary/path | Semantic header owner plus removal of generic argument chip and body summary. |
| Delegate UI drifts | Bounded summary-only tests and visual diff gate. |
| Cross-platform link/reparse gap | Existing atomic/workspace helpers, portable paths, targeted Unix/Windows guards. |

## Completion Boundary

Implementation is ready for user acceptance only after all five tasks are
committed, focused evidence is fresh, current docs agree, active old paths are
gone, no unrelated worktree change was staged, and the final report names any
manual or cross-platform evidence not collected. Passing this plan proves the
implementation task boundary; it does not authorize push, release, merge, or
tagging.
