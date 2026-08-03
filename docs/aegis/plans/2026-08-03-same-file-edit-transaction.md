# Same-File Edit Transaction Coalescing Implementation Plan

Date: `2026-08-03`

Status: `implemented; verification candidate`

## Goal

Prevent a single assistant tool-call batch from producing one committed Edit
followed by stale rejections when multiple canonical Edit calls target the same
file. Same-file calls must be composed in original call order and committed as
one atomic transaction, while external changes must continue to fail closed.

## Architecture

- Canonical mutation owner: `crates/neo-agent-core/src/tools/edit.rs`.
- Runtime orchestration owner: `crates/neo-agent-core/src/runtime/tool_dispatch.rs`.
- `PreparedEdit` remains the runtime-only carrier for a fingerprinted staged
  mutation. It will gain an internal same-file coalescing operation; no public
  tool schema or persisted representation changes.
- The runtime will coalesce only consecutive authorized canonical Edit calls
  with the same resolved target. A coalesced group executes once through the
  existing `PreparedEdit::commit` path. Follower tool calls receive synthetic
  results derived from the one transaction result.

## Tech Stack

- Rust edition 2024.
- `neo-agent-core` library crate.
- Existing `serde_json`, SHA-256 fingerprinting, atomic file installation,
  `tokio` cancellation, and `ToolResult`/`AgentEvent` contracts.
- Existing `cargo-nextest` and unit-test harnesses.

## Baseline / Authority Refs

- `docs/aegis/BASELINE-GOVERNANCE.md`.
- Approved design discussion in the preceding conversation: retain external
  compare-and-swap protection, coalesce same-file edits, reject conflicts, and
  do not blindly retry stale staged content.
- `crates/neo-agent-core/src/tools/edit.rs:311-733`: prepared Edit lifecycle,
  fingerprint recheck, ordered atomic commit, and stale result contract.
- `crates/neo-agent-core/src/runtime/tool_dispatch.rs:624-753`: batch Edit
  preparation and mutation recheck.
- `crates/neo-agent-core/src/runtime/tool_dispatch.rs:1083-1153`: batch
  finalization and sequential/parallel selection.
- `crates/neo-agent-core/src/runtime/tool_dispatch.rs:1400-1680`: scheduling and
  authorized execution.

## Compatibility Boundary

- A single Edit call keeps its current input schema, approval presentation,
  fingerprint validation, atomic write behavior, progress events, and result
  shape.
- External changes between preparation and final commit remain stale failures
  with zero writes. Disabling or weakening the fingerprint check is out of
  scope.
- Noncanonical custom tools named `Edit` continue to use direct registry
  execution and are never coalesced.
- Same-file calls separated by another tool call, a denied/terminal call, a
  Write call, or a different target are not coalesced.
- Different files retain the existing mutation scheduling and commit behavior;
  per-path parallel scheduling is deferred.
- Canonical transcript/history records remain append-only. The change only
  changes runtime tool execution and result projections.
- Existing unrelated worktree changes are outside this task and must remain
  untouched.

## TDD Route

- Mode: `off`.
- Decision: `skipped`.
- Strict authority: `not applicable`; the user requested implementation but did
  not request strict test-first TDD.
- Test posture: focused post-change regression plus unit coverage of the
  coalescing invariant.
- Reason: the repository requires proportional targeted verification, not a
  strict RED/GREEN protocol absent explicit authority.
- Verification: exact `neo-agent-core --lib` nextest filters, formatting, and
  library clippy.

## Requirement Ready Check

- Requirement source refs: approved recommendation and user instruction to
  implement it.
- Goal and scope refs: same-file canonical Edit batches only.
- Scenario: two or more Edit calls in one model batch target the same file and
  each `old` fragment is valid in the intended call order.
- Acceptance criteria:
  1. Disjoint same-file edits in one batch produce one final file containing all
     changes.
  2. The batch returns one provider-valid result per original tool call in the
     original order.
  3. The transaction performs one final fingerprint recheck and one atomic
     install for the target file.
  4. A same-batch operation that cannot apply to the staged result fails the
     whole same-file group before any write.
  5. An external file change still causes a stale failure and does not get
     overwritten.
  6. Single Edit calls and noncanonical custom Edit tools remain compatible.
- Decision: `ready`.

## BaselineUsageDraft

- Required baseline refs: `docs/aegis/BASELINE-GOVERNANCE.md`; current Edit and
  dispatch owners listed above.
- Delivered context refs: prior codegraph exploration and ICM recall of the
  fingerprint/recheck flow.
- Acknowledged before plan refs: baseline governance and current worktree
  status.
- Cited in plan refs: Architecture, Compatibility Boundary, Repair Track, and
  Verification.
- Missing refs: no existing approved spec artifact for this narrow repair;
  approved conversation requirements are sufficient.
- Decision: `continue`.

## Change Necessity

- User-visible need: same-file edits in one batch currently look like one
  success plus several failed edits even when the requested replacements are
  independent and ordered.
- No-change option: keep stale failures and ask the model to reissue each edit;
  this preserves safety but fails the approved usability goal.
- Why code change is necessary: only the runtime and prepared mutation owner can
  compose staged content while preserving authorization, fingerprint, atomic
  commit, and one result per tool call.
- Minimum change boundary: `edit.rs` and `tool_dispatch.rs`, with focused unit
  and runtime tests in those existing test modules.
- Decision: `code-change`.

## Existence And Ownership Check

- Proposed new surface: an internal same-file coalescing operation and runtime
  group marker.
- Existing owner / reuse candidate: `PreparedEdit` owns staged content,
  fingerprint, diff, and atomic commit; `AuthorizedToolCall` owns per-batch
  execution state.
- Why existing surface is insufficient: the current one-call `PreparedEdit`
  cannot compose multiple operations, and the runtime has no way to return one
  committed result to follower calls without duplicate writes.
- Creation proof: the user-visible behavior requires a single final atomic
  write and one result for each original call; reusing the existing owners
  avoids a second mutation subsystem or public tool.
- Entropy / retirement impact: no public API, schema, persistence, fallback, or
  duplicate owner is introduced. The old independent same-file execution path
  is retired for groups of two or more canonical Edit calls; the singleton path
  remains canonical.
- Decision: `reuse-existing` owners with a minimal internal carrier field.

## Architecture Integrity Lens

- Invariant: an approved staged mutation may only replace the exact file version
  it was derived from, unless the intervening changes are explicitly known
  members of the same transaction.
- Canonical owner / contract: `PreparedEdit` owns composition and final
  compare-and-swap; `tool_dispatch` only groups calls and propagates results.
- Responsibility overlap: do not reimplement file reads, hashing, or atomic
  installation in the runtime.
- Higher-level simplification: a same-file group becomes one `PreparedEdit`
  commit rather than several competing prepared whole-file replacements.
- Retirement / falsifier: if a group can commit two physical writes or if an
  external edit is silently merged, the design is invalid and must revert to
  fail-closed behavior.
- Verdict: architecture aligned; edit in existing owners.

## Plan-Time Complexity Check

- Artifact class: maintained core runtime and mutation owner.
- Target files / artifacts: `edit.rs`, `tool_dispatch.rs`, and their existing
  unit-test modules.
- Current pressure: both files are already large, but the relevant owners and
  test seams are established. Adding a new subsystem would increase coupling.
- Projected pressure: moderate local branch and helper growth; no new public
  contract or persistence shape.
- Budget result: `within-budget` if implemented in-place with one coalescing
  helper and one runtime grouping helper.
- Recommendation: `edit-in-place`; do not extract a new mutation crate/file.

## Repair Track

- Root cause: all Edit calls in a provider batch are prepared from the same
  filesystem snapshot. Execution is already sequential for `Edit`/`Write`, but
  later whole-file staged payloads are rechecked against a snapshot invalidated
  by the first commit.
- Minimal sufficient repair: compose same-file operations in memory in original
  order, retain the original fingerprint as the transaction base, commit the
  final staged content once, and synthesize follower results.
- Safety rule: only known same-file group members may account for the in-memory
  version transition. Any external version change or operation mismatch fails
  closed.
- Verification: unit composition tests, runtime batch test, external stale
  regression, formatting, clippy, and diff checks.

## Retirement Track

- Old path: independent execution of two or more consecutive canonical Edit
  calls against one target within one authorized batch.
- Active status: retire for same-file groups; retain unchanged for singleton
  Edit calls and noncanonical tools.
- Keep reason: singleton and custom-tool paths are distinct contracts and are
  not redundant with canonical grouped execution.
- Deletion trigger: remove the old path only after grouped execution covers
  success, conflict, stale, cancellation, and result-order regressions.

## Execution Readiness View

- Intent Lock: make same-file Edit batches behave as one ordered transaction
  without weakening external stale protection.
- Scope Fence: `edit.rs`, `tool_dispatch.rs`, and focused tests only; no TUI,
  provider, persistence, or global scheduler changes.
- Baseline Lock: preserve `PreparedEdit` atomic commit and current `ToolResult`
  contracts for singleton calls.
- Approved Behavior: group consecutive same-target authorized canonical Edits;
  compose in call order; commit once; synthesize follower results; reject
  staged conflicts and external changes.
- Owner / Contract Constraints: `PreparedEdit` owns filesystem safety;
  `tool_dispatch` owns grouping and result ordering.
- Compatibility Boundary: listed above; no schema or serialized event rewrite.
- Retirement Boundary: only the duplicate same-file prepared-write path is
  retired.
- Task Batches: mutation-owner composition, runtime grouping/result handling,
  focused verification.
- Test Obligations: unit composition success/conflict, runtime two-call success,
  external stale protection, singleton compatibility.
- Review Gates: inspect diff for one physical install, no weakened recheck, and
  no unrelated worktree changes.
- Drift / Rewind Rules: if approval semantics or event ordering cannot be
  preserved, stop and return to design rather than add a fallback retry path.
- Evidence Required Before Completion: targeted nextest output, clippy, format
  check, and clean scoped diff review.
- Advisory Boundary: this readiness view is execution guidance, not completion
  authority.

## Implementation Tasks

### Task 1: Add same-file staged composition to `PreparedEdit`

Files:

- Modify `crates/neo-agent-core/src/tools/edit.rs`.
- Add focused tests to the existing test module in that file.

Why: the mutation owner must compose approved replacements without making the
runtime read or write files itself.

Change Necessity: the existing one-operation `PreparedEdit` cannot express a
single base fingerprint with multiple ordered replacements. The minimum change
is to retain each prepared input/base and add one internal coalescing constructor;
no public schema or new owner is needed.

Impact / Compatibility:

- Keep `PreparedEdit::prepare`, `recheck_all`, `commit`, progress, cancellation,
  and singleton result behavior unchanged.
- Add `EditInput: Clone` and retain the original base text plus the parsed input
  in the runtime-only prepared file state so composition can be deterministic.
- Add an internal `PreparedEdit::coalesce_same_file` operation that accepts
  prepared edits in call order, requires one file per edit and equal resolved
  targets/base fingerprints, applies each input to the previous staged string,
  and returns one final prepared edit.
- Recompute the final unified diff from the original base to final staged text;
  retain the original fingerprint so the existing commit recheck remains the
  external compare-and-swap guard.
- Return a structured `same_batch_conflict` prepare failure when an operation
  no longer matches the staged text. This failure must state that no side effect
  occurred and must not retry against live disk content.

Verification:

- Add `prepared_edit_coalesces_disjoint_same_file_operations`: two edits based
  on the same file compose to the expected final staged content and commit
  through one writer invocation.
- Add `prepared_edit_coalescing_rejects_conflicting_sequence`: a later old
  fragment that is invalid after the earlier replacement returns the structured
  conflict and performs no writer call.
- Keep the existing stale, cancellation, and writer-failure tests passing.
- Run:

  `rtk cargo nextest run -p neo-agent-core --lib prepared_edit_coalesces_disjoint_same_file_operations`

  `rtk cargo nextest run -p neo-agent-core --lib prepared_edit_coalescing_rejects_conflicting_sequence`

### Task 2: Coalesce consecutive authorized same-file Edit calls in dispatch

Files:

- Modify `crates/neo-agent-core/src/runtime/tool_dispatch.rs`.
- Add focused tests to its existing `#[cfg(test)]` module.

Why: the runtime must map multiple provider calls to one prepared transaction
while retaining permission results and original call ordering.

Change Necessity: `authorize_tool_batch` currently owns one prepared execution
per call and `recheck_prepared_mutations` rechecks every one before execution.
The runtime needs one group marker and one grouping pass; moving filesystem
logic into dispatch would violate the existing owner boundary.

Impact / Compatibility:

- Add a private follower marker to `AuthorizedToolCall` identifying the primary
  index of a coalesced group. Initialize it as `None` for every call.
- After authorization and before mutation recheck, scan only consecutive
  authorized `Edit` entries whose prepared executions are canonical `Edit`s
  targeting the same resolved path. Stop a group at any non-Edit, terminal,
  Write, or different-target entry.
- Compose each group using `PreparedEdit::coalesce_same_file`; replace the
  primary prepared execution with the combined payload and mark followers.
- If composition fails, convert every member of that group to a terminal
  `same_batch_conflict` result. No member may reach commit.
- Make `recheck_prepared_mutations` skip followers. The primary combined edit
  retains the original fingerprint and is rechecked once.
- Do not change `ToolSchedulingClass`: canonical Edit remains `Exclusive`, so
  the group executes in deterministic provider call order.

Verification:

- Add `same_file_edit_batch_commits_once_and_returns_results_for_each_call`:
  run two disjoint canonical Edit calls against one file with the existing
  builtin registry and parallel tool mode, assert the final content includes
  both changes, assert two results are returned in call order, and assert the
  event stream contains one actual Edit commit/progress sequence.
- Add `same_file_edit_batch_conflict_writes_nothing`: use a conflicting second
  replacement, assert both group results are terminal conflict results and the
  file remains unchanged.
- Add `same_file_edit_batch_preserves_external_stale_rejection`: arrange an
  external change after preparation and before commit using the existing test
  hook/seam if available; otherwise retain the direct `PreparedEdit` stale test
  as the proof and document that the runtime path delegates to the same check.
- Run:

  `rtk cargo nextest run -p neo-agent-core --lib same_file_edit_batch_commits_once_and_returns_results_for_each_call`

  `rtk cargo nextest run -p neo-agent-core --lib same_file_edit_batch_conflict_writes_nothing`

### Task 3: Propagate one transaction result to follower calls

Files:

- Modify `crates/neo-agent-core/src/runtime/tool_dispatch.rs`.
- Extend the same focused dispatch test module if result/event assertions need a
  helper.

Why: providers require one valid tool result per emitted tool call, but only the
primary group member may execute the physical mutation.

Change Necessity: the current sequential executor invokes every authorized
entry independently. Without a follower branch it would either duplicate the
write or return fewer results than the provider expects.

Impact / Compatibility:

- In `execute_authorized_sequential`, execute the primary normally and retain
  its result by index. For each follower, create a result with `kind: "edit"`,
  `coalesced: true`, and `primary_call_id`; report success when the primary
  committed and a corresponding coalesced failure when it did not.
- Run existing `after_tool_result` processing and authorized-result event
  projection for each follower, but do not emit a second started event, commit,
  atomic write, or progress stream.
- Preserve result vector order and permission-decision alignment.
- Keep cancellation semantics unchanged for non-grouped calls; do not add a
  retry or background task path.
- Ensure `executed_any` reflects the primary physical execution only.

Verification:

- Extend the success test to assert follower details identify the primary and
  that no follower result reports a second physical commit.
- Extend the conflict/failure test to assert all group members receive valid
  results and no partial file write occurs.
- Run the two focused dispatch filters from Task 2 again after this task.

## Final Verification And Commit

Run the smallest complete verification set for the touched library:

1. `rtk cargo nextest run -p neo-agent-core --lib prepared_edit_coalesces_disjoint_same_file_operations`
2. `rtk cargo nextest run -p neo-agent-core --lib same_file_edit_batch_commits_once_and_returns_results_for_each_call`
3. `rtk cargo nextest run -p neo-agent-core --lib same_file_edit_batch_conflict_writes_nothing`
4. `rtk cargo nextest run -p neo-agent-core --lib cancellation_before_first_commit_writes_nothing`
5. `rtk cargo fmt --all --check`
6. `rtk cargo clippy -p neo-agent-core --lib -- -D clippy::all`
7. `rtk git diff --check`
8. `rtk git diff -- crates/neo-agent-core/src/tools/edit.rs crates/neo-agent-core/src/runtime/tool_dispatch.rs`

Review the diff specifically for:

- exactly one physical atomic install for a successful same-file group;
- no weakened external fingerprint or path validation;
- no synthetic success after a failed primary transaction;
- no changes to noncanonical custom Edit behavior;
- no edits to unrelated dirty files.

After all focused checks pass, create one conventional commit for this logical
repair: `fix: coalesce same-file edit batches`.

## Risks

- Existing approval dialogs show each original Edit separately while the commit
  is physically grouped. The implementation must keep each requested operation
  unchanged and only combine their known staged effects; if composition changes
  an operation's meaning, fail the group rather than silently merge it.
- A same-file group is intentionally all-or-nothing. This changes the prior
  partial-commit behavior for this narrow batch shape and is required to avoid
  reporting follower success after a primary failure.
- The runtime test must prove result alignment because `finalize_authorized_batch`
  assumes execution returns provider-order results.
- No automatic rebase against live disk is permitted; that would invalidate the
  approved diff and reintroduce lost-update risk.

## Retirement / Baseline Sync

- No ADR or persisted schema change is required because the change reuses the
  existing `PreparedEdit` owner and runtime-only state.
- The old same-file independent prepared execution path is retired only for
  consecutive canonical Edit groups. Singleton and noncanonical paths remain
  explicit compatibility boundaries.
- If implementation requires a new public tool schema, persistent transaction
  identity, or cross-module mutation owner, stop and return to design review;
  that would exceed this plan's approved boundary.
