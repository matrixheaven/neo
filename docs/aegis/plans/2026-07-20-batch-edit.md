# Neo Batch Edit Prepared Execution Implementation Plan

Status: Ready for execution
Date: 2026-07-20

## Goal

Replace Neo's current single-file `Edit` wire contract with the approved
ordered `files[]` batch contract, prepare the complete verified change before
approval, commit existing UTF-8 regular files atomically in declaration order,
report partial commits truthfully, and render the same structured state in the
top-level transcript without changing Delegate-family card structure.

## Architecture

`Edit` remains the only exact-replacement owner. `Write` remains the only file
creation and full-overwrite owner. `edit.rs` owns parsing, validation, exact
matching, staging, fingerprints, diffs, stale checks, atomic per-file commit,
and structured result details. Runtime owns instruction preflight, approval,
scheduling, event emission, and dispatch. TUI code projects structured Edit
arguments, approval data, progress, and results; it never recomputes a verified
diff from raw arguments.

The implementation adds only the narrow runtime transport
`PreparedExecution::Edit(Arc<PreparedEdit>)`. It must not introduce a generic
prepare/commit trait, a second patch tool, a cross-file transaction, rollback,
or a persistent Edit journal.

## Tech Stack

- Rust 2024, minimum Rust 1.96.1
- `serde` and `schemars` for the canonical tool schema
- existing `similar`-based unified diff helper
- existing `sha2` workspace dependency for content fingerprints
- existing `AtomicWriteStatus` and `write_file_atomic_status`
- existing `AgentEvent::ToolExecutionUpdate` and `ToolResult.details`
- Ratatui-compatible Neo transcript primitives and global expansion state

## Baseline And Authority Refs

- `AGENTS.md`
- `docs/aegis/specs/2026-07-20-batch-edit-design.md`
- `docs/aegis/specs/2026-07-17-canonical-approval-protocol-design.md`
- `docs/aegis/specs/2026-07-17-path-scoped-agents-instructions-design.md`
- `docs/aegis/specs/2026-07-20-bash-terminal-tool-card-brief.md`
- current source owners named in the file map below

The approved design is the requirement authority. The implementation agent
must not reopen product choices unless current source evidence makes an
approved invariant impossible to implement.

## Compatibility Boundary

- Keep the model-visible tool name `Edit`.
- Hard-delete the legacy `{ path, old, new, replace_all }` Edit contract.
- Do not decode, alias, translate, or render the old contract.
- Keep `Write`, `Tool`, `ToolResult`, `AgentEvent`, permission modes,
  development modes, queue semantics, and scheduling semantics intact.
- Keep `Edit` classified as `FileWrite` and `Exclusive`.
- Keep Delegate, DelegateGroup, and DelegateSwarm card structure, row budgets,
  ordering, output previews, and expansion behavior exactly as they are.
- Keep persisted session events provider-neutral and do not persist
  `PreparedEdit`.
- Do not claim transactional atomicity across files.

## TDD Route

- Mode: off
- Decision: skipped
- Strict authority: not applicable
- Test posture: post-change regression
- Reason: neither the user nor project rules requested strict test-first TDD;
  the approved design instead requires proportional high-signal regressions.
- Verification: each task names one package, one target selector, and one exact
  test before its root-owned commit checkpoint.

## Verification

Use exact target-level tests only. Do not run workspace-wide `cargo test`,
package-wide `cargo nextest run`, or vague substring filters as completion
evidence. Finish with scoped formatting, lingering-reference, documentation,
and `git diff --check` checks.

When executing these commands in this repository, prefix each shell command
with `rtk` as required by `RTK.md`. Command blocks below intentionally retain
the repository's canonical Cargo/Git spelling so target selectors stay easy to
audit.

## Aegis Visibility

Planning is required because this slice changes a durable tool schema, moves
approval from raw intent to verified prepared data, touches instruction and
Plan-mode path ownership, introduces a narrow runtime-only execution payload,
and retires every old Edit-schema consumer without compatibility fallback.

## Plan Basis

Facts:

- `EditTool::execute` currently parses one `EditInput`, reads one file, matches
  `old`, optionally uses `replace_all`, writes directly with
  `tokio::fs::write`, and returns one diff.
- `PreparedToolCall` currently stores parsed arguments, warning, and Plan/Goal
  approval context only.
- `InstructionScopeProbe::from_prepared_tool` currently returns one optional
  parent directory and reads top-level `path` for Edit.
- `BatchProbes.per_call` currently stores one optional directory per call.
- `check_plan_mode_guard` currently checks one top-level Edit path.
- ordinary Edit approval currently identifies Edit by legacy argument fields
  and renders raw `path` and `replace_all`.
- `SessionApprovalScope` already supports multiple keys.
- `write_file_atomic_status` already returns `Durable` or
  `CommittedUnsynced` and rejects symlink/reparse targets.
- `update_tool_execution` currently appends only `partial_result.content` and
  discards `partial_result.details`.
- `mark_unfinished_tools_for_turn` currently replaces the terminal status and
  loses structured live details.
- `DiffModel::from_tool_details` currently reads one top-level `diff`.
- the global transcript expansion state already reaches normal tool cards.

Assumptions that the executor must verify before editing:

- the existing diff helper remains adequate for original-to-final per-file
  unified diffs;
- moving base `ToolContext` construction earlier in dispatch is side-effect
  free and does not grant tool execution permission;
- Edit's existing `Exclusive` classification guarantees that prepared Edit
  calls reach the sequential execution path;
- existing approval request persistence can serialize a typed Edit
  presentation after the enum gains that variant.

Unknowns are implementation-local, not product decisions:

- the smallest private Edit-only commit-writer seam needed for deterministic
  cross-platform partial-failure tests;
- whether approval expansion is best routed through the existing entry-level
  expansion flag or a small shared presentation helper. It must use the global
  expansion state either way.

## Baseline Usage Draft

- Required baseline refs: approved Batch Edit design, canonical approval
  protocol, path-scoped instructions, Bash/Terminal card boundary, `AGENTS.md`
- Delivered context refs: the same documents plus the current source owners
  listed in this plan
- Acknowledged before plan refs: all required refs
- Cited in plan refs: all required refs
- Missing refs: none
- Decision: continue

## Requirement Ready Check

- Requirement source refs: approved Batch Edit design
- Goals and scope refs: design Goal, Task Intent, Acceptance Criteria, and
  Non-Goals
- User / scenario refs: one coherent model-issued multi-file exact edit,
  inspectable before approval and truthful after partial commit
- Requirement item refs: schema, staging, prepare, approval, stale checks,
  commit, events, TUI, Delegate projection, replay, docs, retirement
- Acceptance / verification criteria refs: design Verification Strategy and
  Acceptance Criteria
- Open blocker questions: none
- Decision: ready

## Ripple Signal Triage

Ripple signal: fired.

Affected consumers that must migrate together:

- tool schema and description;
- file-write permission subject, approval presentation, and session scope;
- instruction-scope typed probes and blocked-scope matching;
- Plan-mode target validation;
- runtime parse/preflight/prepare/authorize/recheck/execute sequence;
- top-level transcript streaming, approval, running, success, failure,
  cancellation, interruption, replay, and resize projection;
- multi-file diff browser/model;
- Delegate child-activity summaries;
- English and Chinese tool/instruction documentation;
- integration tests and fixtures that construct the old schema.

## Change Necessity

- User-visible need: one Edit call must safely express and preview a coherent
  ordered multi-file exact change.
- No-change / non-code option: documentation or prompt guidance cannot make the
  current single-path executor prepare multiple files, approve a verified diff,
  perform stale checks, or report partial commits.
- Why code change is necessary: the current wire schema, runtime lifecycle,
  permission projection, path probes, renderer, and result model encode one
  file and direct write semantics.
- Minimum change boundary: the existing Edit owner plus its direct runtime,
  permission, instruction, TUI, replay, summary, tests, and paired docs
  consumers named in this plan.
- Decision: code-change

## Existence Check

- Proposed new surface: `PreparedExecution::Edit(Arc<PreparedEdit>)` and one
  pure `edit_tool_presentation.rs` renderer
- Existing owner / reuse candidate: `PreparedToolCall`, `edit.rs`,
  `ToolCallComponent`, `ApprovalPresentation`, `ToolExecutionUpdate`, and
  `DiffModel`
- Why existing surface is insufficient: raw arguments cannot carry verified
  staged contents through approval, and the generic renderer currently guesses
  a one-file diff from legacy arguments
- Creation proof: the runtime-only prepared payload binds approved bytes to
  committed bytes; the pure renderer isolates substantial Edit-specific row
  selection without creating state ownership
- Entropy / retirement impact: old Edit schema/render branches are deleted;
  no generic Tool abstraction or second expansion state is added
- Decision: add-with-proof

## Architecture Integrity Lens

- Invariant: the approved diff, stale checks, committed bytes, result details,
  and displayed terminal state all derive from one `PreparedEdit`.
- Canonical owner / contract: `edit.rs` owns Edit semantics; runtime owns
  authorization and event ordering; TUI owns presentation only.
- Responsibility overlap: raw argument parsing may show unverified intent, but
  must never manufacture verified stats or diffs.
- Higher-level simplification: reuse multi-key session scopes, existing atomic
  writer, existing events, and global expansion instead of adding parallel
  mechanisms.
- Retirement / falsifier: any remaining legacy Edit field consumer, fallback
  decoder, second diff source, generic prepare/commit trait, or Delegate card
  layout change falsifies architecture alignment.
- Verdict: proceed with existing owners and the narrow prepared payload.

## Plan Pressure Test

- Owner / contract / retirement: frozen and mapped to explicit files.
- Architecture integrity / higher-level path: existing approval, event,
  atomic-write, expansion, and session-scope owners are reused.
- Verification scope: each boundary has an exact test and the final scan proves
  old schema retirement.
- Task executability: tasks are dependency ordered and include signatures,
  result shapes, commands, and stop conditions.
- Pressure result: proceed

## Complexity Budget

- Artifact class: shared core tool/runtime plus transcript presentation
- Target files / artifacts: `edit.rs`, `tool_dispatch.rs`, `permission.rs`,
  approval data, Plan/instruction probes, TUI event/state/render paths,
  multi-agent summary, paired docs
- Current pressure: `tool_dispatch.rs`, `permission.rs`, and transcript entry
  modules are already large shared owners
- Projected post-change pressure: at-risk if Edit-specific matching or row
  selection is implemented inline in those shared modules
- Budget result: within-budget only if semantics remain in `edit.rs` and
  presentation row construction moves to the dedicated pure renderer
- Planned governance: routing-only edits in shared modules; no unrelated
  refactor; private Edit-only helpers for complex algorithms.

## Plan-Time Complexity Check

- Target files: core and TUI owners in the file map
- Existing size / shape signals: large runtime/permission/entry modules with
  many unrelated branches; compact current `edit.rs`; established pure helper
  patterns in transcript code
- Owner fit: staged edit state belongs in `edit.rs`; approval data belongs in
  `approval.rs`; event sequencing belongs in `tool_dispatch.rs`; rows belong in
  `edit_tool_presentation.rs`
- Add-in-place risk: high for inline Edit algorithms in permission, dispatch,
  or generic renderer code
- Better file boundary: add only the pure Edit presentation file; keep private
  core helpers inside `edit.rs` until evidence shows a second consumer
- Recommendation: extract the renderer, edit other owners in place, and reject
  any new generic abstraction.

## Execution Readiness View

- Intent Lock: implement the approved Batch Edit design exactly; do not
  compare or borrow from `.references/` implementations.
- Scope Fence: source, tests, and paired docs listed in this plan; no unrelated
  cleanup, provider changes, or generic tool redesign.
- Baseline Lock: the approved design and listed authority refs are canonical.
- Approved Behavior: ordered exact replacements, whole-call preparation,
  verified approval, stale rechecks, per-file atomic commits, truthful partial
  errors, structured TUI/replay summaries.
- Owner / Contract Constraints: `Edit` and `Write` ownership is fixed;
  `PreparedExecution` is narrow; runtime owns approval; `edit.rs` owns bytes.
- Compatibility Boundary: no legacy Edit compatibility; stable non-Edit tool,
  event, permission-mode, and Delegate-family behavior.
- Retirement Boundary: delete all old schema fields and consumers in the same
  implementation workstream.
- Task Batches: core engine; path guards; runtime/approval; TUI; Delegate/replay;
  docs/retirement; final verification/ADR assessment.
- Test Obligations: exact regressions named per task; no broad Cargo run.
- Review Gates: contract review after Task 1; runtime event/approval review
  after Task 3; visual/Delegate preservation review after Task 5; retirement
  and docs review before final commit.
- Drift / Rewind Rules: stop and return to the design owner if implementation
  appears to require a new event, generic prepare/commit Tool abstraction,
  cross-file rollback, persistent journal, fuzzy matching, file creation, or a
  compatibility branch.
- Evidence Required Before Completion: exact tests, scoped rustfmt, old-field
  scan, paired-doc parity, Aegis workspace check, and `git diff --check`.
- Advisory Boundary: method-pack execution guidance only; not GateDecision,
  PolicySnapshot, or completion authority.

## Frozen Contract

The executor must preserve this input shape exactly:

```json
{
  "files": [
    {
      "path": "src/model.rs",
      "replacements": [
        {
          "old": "OldName",
          "new": "NewName",
          "expected_matches": 1
        }
      ]
    }
  ]
}
```

Required semantics:

1. `files` and every `replacements` array are non-empty and ordered.
2. Unknown fields are rejected at every object level.
3. `old` is non-empty; `new` may be empty; `old == new` is rejected.
4. `expected_matches` defaults to `1` and must be at least `1`.
5. Matching is exact, non-overlapping UTF-8 substring matching.
6. Each replacement runs against the previous staged result for that file.
7. Duplicate effective targets, missing files, non-regular files, symlinks,
   Windows reparse points, and non-UTF-8 bytes are rejected before writes.
8. Any prepare failure produces zero writes.
9. Every target is rechecked after approval and again immediately before its
   own commit.
10. Files commit in declaration order and stop at the first failed, stale,
    cancelled-before-next-file, or committed-unsynced boundary.
11. Already committed files are never rolled back automatically.
12. Interrupted Edit execution is not resumed after restart.

## Shared Implementation Contracts

### Core Input And Prepared Types

Define the canonical schema in `edit.rs` with object-level unknown-field
rejection:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditInput {
    files: Vec<EditFileInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditFileInput {
    path: PathBuf,
    replacements: Vec<EditReplacementInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditReplacementInput {
    old: String,
    new: String,
    #[serde(default = "default_expected_matches")]
    expected_matches: usize,
}

const fn default_expected_matches() -> usize {
    1
}
```

Keep prepared state runtime-only and non-serializable. The exact private field
layout may follow implementation needs, but it must expose only narrow getters
needed by runtime, permission, and TUI projection:

```rust
pub(crate) struct PreparedEdit {
    files: Vec<PreparedEditFile>,
    replacements: usize,
    added: usize,
    removed: usize,
}

pub(crate) struct PreparedEditFile {
    requested_path: PathBuf,
    resolved_path: PathBuf,
    fingerprint: EditFingerprint,
    original: String,
    staged: String,
    replacements: usize,
    added: usize,
    removed: usize,
    diff: String,
}

struct EditFingerprint {
    resolved_path: PathBuf,
    file_kind: EditFileKind,
    sha256: [u8; 32],
}
```

Use methods rather than making prepared fields public. Required method-level
capabilities are:

```rust
impl PreparedEdit {
    pub(crate) async fn prepare(
        context: &ToolContext,
        arguments: &serde_json::Value,
    ) -> Result<Arc<Self>, ToolResult>;

    pub(crate) fn approval_presentation(&self) -> EditApprovalPresentation;
    pub(crate) fn session_approval_scope(&self, workspace: &str)
        -> Option<SessionApprovalScope>;
    pub(crate) fn prepared_update(&self) -> ToolResult;
    pub(crate) async fn recheck_all(&self) -> Result<(), ToolResult>;
    pub(crate) async fn commit(
        &self,
        cancel_token: &CancellationToken,
        on_progress: &mut dyn FnMut(ToolResult),
    ) -> ToolResult;
}
```

Names may be adjusted to local style, but these responsibilities must remain
in `edit.rs` and must not move into runtime or TUI code.

### Prepared Execution Transport

Add the narrow transport in `runtime/tool_arguments.rs`:

```rust
#[derive(Clone)]
pub(super) enum PreparedExecution {
    Direct,
    Edit(Arc<PreparedEdit>),
}

pub struct PreparedToolCall {
    pub id: String,
    pub name: String,
    pub raw_arguments: String,
    pub arguments: serde_json::Value,
    pub warning: Option<String>,
    pub(super) approval: Option<ApprovalExecutionContext>,
    pub(super) execution: PreparedExecution,
}
```

Clean and guarded-repair argument parsing initially set `Direct`. After
instruction preflight, runtime replaces it with `Edit(prepared)` only for
successfully parsed Edit calls. No provider-visible or persisted type changes.

### Approval Projection

Add serializable presentation-only types in `approval.rs`. They contain paths,
counts, stats, and diffs, but never complete original/staged file bodies:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EditApprovalPresentation {
    pub files: usize,
    pub replacements: usize,
    pub added: usize,
    pub removed: usize,
    pub changes: Vec<EditApprovalChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EditApprovalChange {
    pub path: PathBuf,
    pub replacements: usize,
    pub added: usize,
    pub removed: usize,
    pub diff: String,
}

pub enum ApprovalPresentation {
    // existing variants remain unchanged
    Edit {
        title: String,
        edit: EditApprovalPresentation,
    },
}
```

The title is `Edit N files?`. The session option is present only when every
prepared target is workspace-contained and contributes one existing
`SessionApprovalKey::FileWrite { operation: Edit, ... }`. The exact label is:

```text
Approve edits to these N files for this session
```

### Structured Details

Use the exact top-level terminal statuses from the design:

```text
committed
prepare_failed
stale
partial_commit
durability_uncertain
```

Use the exact per-file statuses:

```text
committed
committed_unsynced
failed
not_attempted
```

Before first commit, emit a `ToolExecutionUpdate` whose
`partial_result.details.kind` is `edit_prepared`. After every durable file
commit, emit one whose kind is `edit_progress`. Do not emit per-replacement
updates and do not add an `AgentEvent` variant.

## File Map

| Path | Planned responsibility |
|---|---|
| `crates/neo-agent-core/src/tools/edit.rs` | Canonical schema, prepare, fingerprints, ordered staging, stale checks, commit, details, focused tests/helpers. |
| `crates/neo-agent-core/src/tools/mod.rs` | Export only the narrow core types/functions runtime needs; do not change `Tool` trait. |
| `crates/neo-agent-core/src/session/atomic_file.rs` | Reuse writer; generalize session-specific wording/default temp name if user-visible. |
| `crates/neo-agent-core/src/runtime/tool_arguments.rs` | `PreparedExecution`, prepared transport, multi-path instruction probes. |
| `crates/neo-agent-core/src/runtime/tool_dispatch.rs` | New phase order, prepared Edit dispatch, stale recheck, prepared/progress event emission. |
| `crates/neo-agent-core/src/runtime/permission.rs` | Prepared-aware approval, multi-key Edit session scope, Plan guard call. |
| `crates/neo-agent-core/src/approval.rs` | Serializable typed Edit approval projection. |
| `crates/neo-agent-core/src/mode/plan_mode_guard.rs` | Whole-batch active-plan target validation. |
| `crates/neo-agent-core/src/multi_agent/runtime.rs` | Bounded Edit argument/progress/result summaries only. |
| `crates/neo-agent-core/tests/tool_files.rs` | Core batch, zero-write prepare failure, partial commit behavior. |
| `crates/neo-agent-core/tests/runtime_turn.rs` | Approval, stale-after-approval, events, Plan mode, instruction integration. |
| `crates/neo-agent-core/tests/multi_agent_runtime.rs` | Child Edit summary and replay/interruption evidence. |
| `crates/neo-agent-core/tests/session_jsonl.rs` | Typed approval/event serialization only if existing coverage requires update. |
| `crates/neo-tui/src/transcript/edit_tool_presentation.rs` | New pure structured Edit renderer. |
| `crates/neo-tui/src/transcript/mod.rs` | Module registration only. |
| `crates/neo-tui/src/transcript/tool_renderers.rs` | Route Edit states; delete legacy raw old/new preview logic. |
| `crates/neo-tui/src/transcript/tool_call.rs` | Retain structured live details and pass state to pure renderer. |
| `crates/neo-tui/src/transcript/event_handler.rs` | Preserve update details and terminal details across interruption. |
| `crates/neo-tui/src/transcript/pane.rs` | Preserve last structured details when marking unfinished Edit calls. |
| `crates/neo-tui/src/transcript/entry/mod.rs` | Typed Edit approval route and global expansion. |
| `crates/neo-tui/src/diff_model.rs` | Parse ordered multi-file `changes[].diff`. |
| `crates/neo-tui/tests/tool_cards.rs` | Wide/collapsed/expanded/narrow/failure/approval table tests. |
| `docs/en/reference/tools.md` | Canonical English Edit contract. |
| `docs/zh/reference/tools.md` | Canonical Chinese Edit contract. |
| `docs/en/customization/agents.md` | Multi-target instruction preflight behavior. |
| `docs/zh/customization/agents.md` | Paired Chinese instruction behavior. |

Only touch an additional file when a direct compiler error or verified
consumer requires it. Record that evidence in the implementation handoff.

## Task Dependencies

```text
Task 1 core Edit engine
  |
  +--> Task 2 instruction probes and Plan guard
  |
  +--> Task 3 runtime and approval prepared execution
            |
            +--> Task 4 top-level TUI and multi-file diff model
            |
            +--> Task 5 Delegate summaries and replay/interruption
                         |
                         +--> Task 6 docs and legacy retirement
                                      |
                                      +--> Task 7 final verification and ADR check
```

Tasks 2 and the non-runtime portion of Task 4 can be delegated in parallel
after Task 1 freezes shared types. Task 3 must land before live TUI and replay
tests can be finalized. The root executor resolves shared-file edits and is the
only agent allowed to stage or commit.

## Task 1: Implement The Core Batch Edit Engine

Files:

- Modify `crates/neo-agent-core/src/tools/edit.rs`
- Modify `crates/neo-agent-core/src/tools/mod.rs` only for narrow visibility
- Modify `crates/neo-agent-core/src/session/atomic_file.rs` only if wording is
  currently session-specific in an Edit-visible path
- Modify `crates/neo-agent-core/tests/tool_files.rs`

Why:

This task establishes the single source of truth for the new schema, exact
staging, fingerprints, atomic file commits, and structured results. Every later
task must consume this owner rather than reproduce its logic.

Change Necessity:

The current direct single-file executor cannot validate a whole batch before
writes or carry staged bytes through approval. The minimum change is a
runtime-only prepared engine inside `edit.rs` plus reuse of the existing atomic
writer.

Impact And Compatibility:

- Delete `EditInput.path`, `old`, `new`, and `replace_all`.
- Keep `EditTool::name()` as `Edit` and keep registry ownership unchanged.
- Keep `EditTool::execute` available for direct registry/tool tests by calling
  `PreparedEdit::prepare`, `PreparedEdit::recheck_all`, and
  `PreparedEdit::commit` in that order. The authorized runtime path must call
  `PreparedEdit::commit` on the already prepared payload and must never invoke
  `EditTool::execute` after approval. This preserves one semantic owner without
  letting approved execution reread or restage the call.
- Do not allow create, delete, move, links, reparse points, or non-UTF-8 data.

Steps:

1. Replace the old input structs with the frozen schema types and
   `deny_unknown_fields` at all levels.
2. Rewrite the tool description to instruct the model to read files first,
   supply observed exact counts, group changes by file, use `Write` for file
   creation/full overwrite, and issue a fresh Edit after any failure.
3. Add strict input validation before filesystem work: non-empty arrays,
   non-empty paths and `old`, positive `expected_matches`, `old != new`.
4. Resolve every path with `ToolContext::resolve_parent_for_write`, canonicalize
   only as allowed by existing workspace policy, and reject duplicate effective
   targets without changing declaration order.
5. Use symlink/reparse-safe metadata checks before reading. Read bytes once,
   reject non-UTF-8, and fingerprint the resolved path, file kind, and SHA-256
   bytes.
6. For each file, apply replacements sequentially to a staged `String`. Count
   non-overlapping exact matches in the current staged value and require exact
   equality with `expected_matches` before replacing all counted matches.
7. Reject a file whose final staged content equals its original content and
   reject an aggregate no-op batch.
8. Compute one original-to-final unified diff and final added/removed line
   stats per file. Aggregate stats from final per-file diffs, not intermediate
   replacement steps.
9. Implement whole-batch and just-in-time fingerprint rechecks using the same
   safe metadata/read path as preparation.
10. Implement declaration-order commit with `write_file_atomic_status`. Map
    `Durable`, `CommittedUnsynced`, pre-replacement error, stale, and
    cancellation to the frozen details states.
11. Add one private Edit-only writer seam for deterministic tests. Production
    must call `write_file_atomic_status`; tests may inject a failure at a named
    file index. Do not expose the seam outside `edit.rs`.
12. Build provider text that explicitly says zero writes, partial writes, or
    complete-but-unsynced contents and tells the model to reread before a fresh
    call. Never recommend blind replay.
13. Add focused regressions:
    - `edit_batch_applies_ordered_replacements_across_files`
    - `edit_batch_prepare_mismatch_writes_nothing`
    - `edit_batch_commit_failure_reports_partial_without_rollback`
    - `edit_batch_rejects_legacy_schema_and_link_like_targets`

Verification:

```bash
cargo test --package neo-agent-core --test tool_files -- edit_batch_applies_ordered_replacements_across_files --exact --nocapture
cargo test --package neo-agent-core --test tool_files -- edit_batch_prepare_mismatch_writes_nothing --exact --nocapture
cargo test --package neo-agent-core --test tool_files -- edit_batch_commit_failure_reports_partial_without_rollback --exact --nocapture
cargo test --package neo-agent-core --test tool_files -- edit_batch_rejects_legacy_schema_and_link_like_targets --exact --nocapture
```

Expected outcomes:

- each command exits 0;
- the mismatch test proves both/all target bytes remain unchanged;
- the partial test proves committed/failed/not-attempted states and no rollback;
- the legacy test proves the old top-level fields are rejected.

Repair Track:

- Root cause: direct one-file execution conflates matching, write, and result.
- Canonical owner: `PreparedEdit` in `edit.rs`.
- Stable repair: one side-effect-free prepare plus one per-file atomic commit.
- Compatibility boundary: no legacy decoder.

Retirement Track:

- Old owner: `EditInput` and `edit_details` legacy shape.
- Active status after task: deleted.
- Lingering references are removed in Task 6 after downstream migration.

Commit checkpoint: the root executor reviews and commits this logical slice;
delegated agents must not run any Git mutation.

## Task 2: Expand Instruction Probes And Plan-Mode Guard To Every Edit Path

Files:

- Modify `crates/neo-agent-core/src/runtime/tool_arguments.rs`
- Modify `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
- Modify `crates/neo-agent-core/src/mode/plan_mode_guard.rs`
- Modify focused tests in those modules and/or
  `crates/neo-agent-core/tests/runtime_turn.rs`

Why:

Every target directory can introduce a different trusted instruction scope,
and Plan mode must reject the whole Edit when any target is not the active plan
file.

Change Necessity:

The current one-optional-probe model and top-level `path` guard cannot represent
the canonical batch schema. The minimum change is a per-call probe collection
and Edit-specific all-target validation inside existing owners.

Impact And Compatibility:

- Non-Edit file, search, Bash, and Terminal probe behavior remains unchanged.
- The instruction batch still defers or blocks as one unit.
- `Write` keeps its existing single-path Plan-mode behavior.

Steps:

1. Change `InstructionScopeProbe::from_prepared_tool` to return a collection,
   for example `Vec<InstructionScopeProbe>`, with zero entries for tools that do
   not produce typed paths.
2. For Edit, parse every `files[].path`, resolve its parent through the existing
   probe policy, preserve declaration order, and deduplicate canonical target
   directories without losing any distinct scope.
3. Change `BatchProbes.per_call` from `Vec<Option<PathBuf>>` to
   `Vec<Vec<PathBuf>>` or the equivalent typed collection.
4. Update `collect_batch_probes`, blocked-scope matching, synthesized results,
   and governed-directory checks so a mutation call is covered when any of its
   probe directories intersects a blocked scope.
5. Update `check_plan_mode_guard` so Edit requires a non-empty `files` array and
   every path must satisfy `is_active_plan_file_path`. One mismatch denies the
   whole call. Do not partially filter files.
6. Add exact tests:
   - `typed_scope_probes_cover_every_edit_parent`
   - `instruction_preflight_defers_whole_edit_for_one_new_scope`
   - `active_plan_mode_allows_edit_only_when_every_target_is_plan_file`
   - `active_plan_mode_rejects_edit_with_any_non_plan_target`

Verification:

```bash
cargo test --package neo-agent-core --lib -- runtime::tool_arguments::tests::typed_scope_probes_cover_every_edit_parent --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn -- instruction_preflight_defers_whole_edit_for_one_new_scope --exact --nocapture
cargo test --package neo-agent-core --lib -- mode::plan_mode_guard::tests::active_plan_mode_allows_edit_only_when_every_target_is_plan_file --exact --nocapture
cargo test --package neo-agent-core --lib -- mode::plan_mode_guard::tests::active_plan_mode_rejects_edit_with_any_non_plan_target --exact --nocapture
```

Expected outcomes:

- every command exits 0;
- the instruction test emits defer/block results for the entire call and writes
  nothing;
- the Plan-mode negative test proves no target is applied.

Repair Track:

- Root cause: one-path probe/guard types encode the retired schema.
- Canonical owner: typed path extraction in `tool_arguments.rs` and Plan-mode
  policy in `plan_mode_guard.rs`.
- Stable repair: collections with whole-call decisions.

Retirement Track:

- Old owner: one `Option<PathBuf>` per call and legacy top-level Edit path.
- Active status after task: deleted.

Commit checkpoint: root only after focused tests pass.

## Task 3: Add Prepared Execution, Verified Approval, Stale Rechecks, And Events

Files:

- Modify `crates/neo-agent-core/src/runtime/tool_arguments.rs`
- Modify `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
- Modify `crates/neo-agent-core/src/runtime/permission.rs`
- Modify `crates/neo-agent-core/src/approval.rs`
- Modify `crates/neo-agent-core/tests/runtime_turn.rs`
- Modify `crates/neo-agent-core/tests/session_jsonl.rs` only when enum
  serialization fixtures require it

Why:

Ask mode must approve the exact prepared bytes and runtime must execute that
same payload after instruction and content stale checks.

Change Necessity:

Raw arguments cannot prove the diff or bind approval to committed content. The
minimum change is the narrow `PreparedExecution` payload plus prepared-aware
permission projection and dispatch.

Impact And Compatibility:

- Non-Edit calls stay `PreparedExecution::Direct` and retain their current
  parse, approval, and execution path.
- No new `AgentEvent`, global partial status, or generic Tool lifecycle.
- Approval remains runtime-owned and serializable.
- Edit remains `Exclusive`, so the authorized batch uses sequential dispatch.

Steps:

1. Add `PreparedExecution` and initialize it as `Direct` in every argument parse
   outcome, including guarded repair.
2. In `execute_tool_calls`, preserve this exact phase order:
   - parse all arguments;
   - instruction preflight over every typed target;
   - construct the base `ToolContext` without running a tool;
   - prepare every valid Edit call side-effect free;
   - authorize the full batch;
   - recheck instruction fingerprint;
   - recheck every prepared Edit target;
   - schedule and execute authorized calls.
3. If Edit preparation fails, convert only that call to a terminal
   `prepare_failed` result before permission. It must never show an approval
   dialog and must not write.
4. Change permission preparation/builders to accept the prepared call or its
   `PreparedExecution`, not just raw arguments, for Edit-specific branches.
5. Delete legacy `is_edit` detection based on `old`, `new`, or `replace_all`.
6. Build `ApprovalPresentation::Edit` from
   `PreparedEdit::approval_presentation`. Do not call the diff helper in
   permission code.
7. Build one multi-key `SessionApprovalScope` from every prepared
   workspace-contained target. Omit the session option if any target cannot be
   represented narrowly.
8. Ensure cached session approval matches only when the complete key set is
   already approved; reuse existing key-set semantics rather than adding a
   wildcard.
9. After approval and instruction recheck, call `PreparedEdit::recheck_all`.
   A mismatch returns top-level `stale` with zero writes.
10. In sequential execution, emit `ToolExecutionStarted`, then immediately emit
    the prepared `ToolExecutionUpdate` before the first commit.
11. Execute the prepared payload directly through an Edit-specific branch in
    dispatch. Do not invoke `EditTool::execute` in a way that rereads/restages
    the call.
12. Convert each durable file callback to one existing
    `ToolExecutionUpdate { partial_result }` with `kind: edit_progress`.
13. Preserve cancellation semantics: cancellation before the first commit is
    zero writes; during commit it is observed only between files; an in-flight
    atomic replacement is allowed to finish.
14. Add exact regressions:
    - `runtime_edit_approval_uses_verified_projection_and_multi_key_scope`
    - `runtime_edit_stale_after_approval_writes_nothing`
    - `runtime_edit_emits_prepared_then_per_file_progress_updates`
    - `runtime_edit_partial_commit_is_failed_tool_result`
    - `approval_edit_presentation_round_trips_in_session_jsonl` when needed

Verification:

```bash
cargo test --package neo-agent-core --test runtime_turn -- runtime_edit_approval_uses_verified_projection_and_multi_key_scope --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn -- runtime_edit_stale_after_approval_writes_nothing --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn -- runtime_edit_emits_prepared_then_per_file_progress_updates --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn -- runtime_edit_partial_commit_is_failed_tool_result --exact --nocapture
```

If `session_jsonl.rs` changes, also run:

```bash
cargo test --package neo-agent-core --test session_jsonl -- approval_edit_presentation_round_trips_in_session_jsonl --exact --nocapture
```

Expected outcomes:

- approval diff and committed success diff are byte-identical;
- stale-after-approval produces zero writes and no started commit;
- event order is Started, `edit_prepared`, zero or more `edit_progress`, Finished;
- partial commit has `is_error = true` and exact per-file states.

Repair Track:

- Root cause: authorization currently sees only unverified raw arguments.
- Canonical owner: runtime transports `PreparedEdit`; permission projects it;
  `edit.rs` commits it.
- Stable repair: one prepared payload from approval through result.

Retirement Track:

- Old owner: raw Edit approval and direct registry execution.
- Active status after task: deleted for Edit; unchanged for Direct tools.

Review gate: inspect event order, absence of generic abstractions, and complete
session-key matching before the root commit checkpoint.

Commit checkpoint: root only after exact runtime tests pass.

## Task 4: Implement The Structured Top-Level Edit Card And Multi-File Diff Model

Files:

- Create `crates/neo-tui/src/transcript/edit_tool_presentation.rs`
- Modify `crates/neo-tui/src/transcript/mod.rs`
- Modify `crates/neo-tui/src/transcript/tool_renderers.rs`
- Modify `crates/neo-tui/src/transcript/tool_call.rs`
- Modify `crates/neo-tui/src/transcript/event_handler.rs`
- Modify `crates/neo-tui/src/transcript/pane.rs`
- Modify `crates/neo-tui/src/transcript/entry/mod.rs`
- Modify `crates/neo-tui/src/diff_model.rs`
- Modify `crates/neo-tui/tests/tool_cards.rs`

Why:

Users must see verified planned changes before approval, live file-boundary
progress, truthful terminal states, explicit collapsed omission, full global
expansion, and width-safe rows.

Change Necessity:

The current renderer extracts legacy `old`/`new`, `DiffModel` reads one diff,
and live updates lose details. The minimum change is one pure Edit renderer,
structured detail retention, and multi-file parsing in existing state owners.

Impact And Compatibility:

- `ToolCallComponent` remains the stateful owner.
- Global `Ctrl+O` remains the only expansion state.
- Approval entries gain expansion through that same state; no Edit-only toggle.
- Other tool cards and Delegate-family cards are not redesigned.
- Styled rows are regenerated on resize/replay and are not persisted.

Steps:

1. Add `edit_tool_presentation.rs` as a pure module that accepts width, theme,
   global expanded state, tool status, raw argument text, structured live
   details, terminal details, and optional workspace root.
2. Define internal parsed presentation types for proposed, prepared, progress,
   committed, prepare failure, stale, partial commit, durability uncertain,
   cancellation, and interrupted states. These types are TUI-local and never
   become runtime owners.
3. Streaming raw arguments:
   - incomplete JSON renders `Preparing Edit` and receiving text only;
   - safely parsed counts/paths render `?` intent rows;
   - never show added/removed stats or a diff before `edit_prepared` or typed
     approval data arrives.
4. Preserve `partial_result.details` in `update_tool_execution`. Store the last
   structured Edit details alongside live output rather than converting them
   to human text.
5. When terminal result arrives, replace planned/live projection with terminal
   `ToolResult.details` while keeping ordinary result text available for the
   model and fallback display.
6. Change `mark_unfinished_tools_for_turn` so an unfinished Edit retains the
   last structured progress details and becomes an interrupted terminal card.
   Do not pretend the last progress update is a final result.
7. Route `ApprovalPresentation::Edit` through the same pure renderer or a
   shared Edit projection function. The runtime-supplied options, labels,
   payloads, and order remain authoritative.
8. Extend `DiffModel::from_tool_details` to parse ordered `changes[].diff` into
   one multi-file model. Keep current top-level one-diff parsing only for
   non-Edit tools that still use it; do not use it as legacy Edit compatibility.
9. Implement final-row-aware collapsed selection:
   - preserve aggregate summary;
   - preserve first file and first change cluster;
   - include another leading file only when it fits;
   - emit an explicit omitted files/replacements/changed-lines row;
   - preserve final file and final change cluster;
   - expanded mode renders every file and cluster.
10. Hard-wrap paths, stats, code, and diagnostics so every rendered row width is
    at most the available terminal width. Do not silently truncate semantic
    content.
11. Render exact distinctions from the design:
    - prepare and stale state say zero writes;
    - partial state styles only committed diffs as applied;
    - not-attempted changes do not use applied addition/removal styling;
    - committed-unsynced says contents were installed but durability is
      uncertain;
    - interruption says final commit state is unknown and shows last progress.
12. Delete legacy Edit raw diff construction and the `pane.rs` fallback that
    treats top-level `new` as a path-like key.
13. Add table-driven regressions:
    - `edit_batch_card_renders_collapsed_expanded_and_narrow`
    - `edit_batch_card_distinguishes_prepare_stale_partial_and_durability`
    - `edit_batch_approval_uses_global_expansion`
    - `edit_batch_progress_details_survive_interruption`
    - `diff_model_reads_ordered_edit_changes`

Verification:

```bash
cargo test --package neo-tui --test tool_cards -- edit_batch_card_renders_collapsed_expanded_and_narrow --exact --nocapture
cargo test --package neo-tui --test tool_cards -- edit_batch_card_distinguishes_prepare_stale_partial_and_durability --exact --nocapture
cargo test --package neo-tui --test tool_cards -- edit_batch_approval_uses_global_expansion --exact --nocapture
cargo test --package neo-tui --test tool_cards -- edit_batch_progress_details_survive_interruption --exact --nocapture
cargo test --package neo-tui --lib -- diff_model::tests::diff_model_reads_ordered_edit_changes --exact --nocapture
```

Expected outcomes:

- every command exits 0;
- wide collapsed output contains an explicit omission row;
- expanded output contains every path and diff cluster;
- narrow fixtures assert every row width is within the requested width;
- interruption retains progress without claiming resume or success.

Repair Track:

- Root cause: the generic legacy renderer guesses one diff from raw fields and
  event handling discards structured progress.
- Canonical owner: pure Edit renderer plus existing stateful tool component.
- Stable repair: structured projections regenerated for every display state.

Retirement Track:

- Old owner: legacy `old`/`new` preview and single-diff Edit assumptions.
- Active status after task: deleted.

Review gate: compare rendered fixtures to every character-art state in the
design and confirm global expansion is the only expansion owner.

Commit checkpoint: root only after exact TUI tests pass.

## Task 5: Add Bounded Delegate Summaries And Non-Resumable Replay Semantics

Files:

- Modify `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify `crates/neo-agent-core/tests/multi_agent_runtime.rs`
- Modify replay-related TUI/core tests only when the direct consumer requires it

Why:

Parent agents need bounded, truthful child Edit activity while Delegate-family
cards remain visually and structurally unchanged. Restarted sessions must show
uncertainty without resuming an unfinished commit.

Change Necessity:

Current generic argument summarization falls back to the first object field and
does not understand Edit progress/result details. The minimum change is
Edit-specific bounded strings inside existing child activity projection.

Impact And Compatibility:

- No changes to Delegate, DelegateGroup, or DelegateSwarm renderer/layout files
  unless a compile-time signature migration is unavoidable.
- No full child Edit arguments or diffs are persisted in parent progress.
- Existing activity capacity, order, and output preview limits remain fixed.

Steps:

1. Extend `summarize_tool_arguments` for Edit to extract bounded file count,
   replacement count, and head/tail paths from `files[]`.
2. In `apply_tool_activity_event`, prefer structured `edit_prepared`,
   `edit_progress`, and terminal Edit details over raw arguments once present.
3. Produce only the approved bounded forms for ongoing, success, prepare
   failure, and partial failure. Omit missing fields instead of parsing human
   result text.
4. Keep Swarm's existing two-line projection and Delegate-family row budgets.
5. Ensure child persistence stores existing events/results only. Never persist
   `PreparedEdit` or complete staged content.
6. On replay, an unfinished Edit with progress is marked interrupted and is not
   submitted to runtime again. Display the last recorded committed count and
   final-state uncertainty.
7. Add exact regressions:
   - `edit_tool_summary_preserves_counts_and_head_tail_within_budget`
   - `edit_tool_summary_prefers_structured_partial_progress`
   - `replayed_unfinished_edit_is_interrupted_and_not_resumed`

Verification:

```bash
cargo test --package neo-agent-core --test multi_agent_runtime -- edit_tool_summary_preserves_counts_and_head_tail_within_budget --exact --nocapture
cargo test --package neo-agent-core --test multi_agent_runtime -- edit_tool_summary_prefers_structured_partial_progress --exact --nocapture
cargo test --package neo-agent-core --test multi_agent_runtime -- replayed_unfinished_edit_is_interrupted_and_not_resumed --exact --nocapture
```

Expected outcomes:

- summaries remain within the existing byte/row budget;
- structured partial progress overrides raw argument intent;
- replay records interruption and makes no tool execution attempt.

Repair Track:

- Root cause: generic summary extraction has no batch Edit semantics.
- Canonical owner: existing multi-agent activity summary functions.
- Stable repair: bounded structured summary, no renderer redesign.

Retirement Track:

- Old owner: first-field fallback for Edit and any stale one-path fixtures.
- Active status after task: deleted for Edit only.

Review gate: diff Delegate-family renderer files before and after this task. If
their structure/layout changed, stop and correct the implementation.

Commit checkpoint: root only after exact summary/replay tests pass.

## Task 6: Update Paired Documentation And Delete Every Legacy Consumer

Files:

- Modify `docs/en/reference/tools.md`
- Modify `docs/zh/reference/tools.md`
- Modify `docs/en/customization/agents.md`
- Modify `docs/zh/customization/agents.md`
- Modify any remaining source/test fixture proven by the scoped legacy scan

Why:

The new contract must be the only documented and executable Edit behavior in
both languages. Retaining old fields anywhere invites model calls and internal
fixtures to recreate the retired path.

Change Necessity:

The source contract change is incomplete while prompts, docs, tests, approval,
Plan guard, probes, or renderers still teach or consume the old shape.

Impact And Compatibility:

- English and Chinese docs must describe identical semantics.
- Do not retain a migration note that suggests the old schema is accepted.
- Unrelated uses of words such as `old`, `new`, or `replace_all` in non-Edit
  domains are not retirement targets.

Steps:

1. Replace the Edit examples with the canonical `files[]` schema.
2. Document ordered staged matching, exact `expected_matches`, zero-write
   prepare failure, post-approval stale checks, per-file atomic commit, partial
   commit truthfulness, and fresh-call retry guidance.
3. State that Edit supports existing UTF-8 regular files only and that `Write`
   owns creation and full-file replacement.
4. State explicitly that the legacy Edit fields are not accepted, without
   presenting them as an alternative invocation.
5. Update instruction docs so every Edit target parent participates in scope
   discovery and one changed/blocked scope defers or blocks the whole call.
6. Run a scoped legacy-consumer scan. Classify each hit before editing:
   - Edit source/test/doc consumer: migrate or delete;
   - unrelated domain usage: retain and note why in the execution summary.
7. Confirm no source, test, tool description, approval branch, Plan guard,
   instruction probe, TUI path, or child summary still reads legacy Edit fields.

Verification:

```bash
rg -n 'EditInput|replace_all|arguments\.get\("old"\)|arguments\.get\("new"\)|string_field\(&value, "old"\)|string_field\(&value, "new"\)' crates/neo-agent-core crates/neo-tui docs/en docs/zh
rg -n 'expected_matches|"files"|partial commit|zero writes' docs/en/reference/tools.md docs/zh/reference/tools.md
rg -n 'every.*target|whole.*Edit|所有.*目标|整个.*Edit' docs/en/customization/agents.md docs/zh/customization/agents.md
```

Expected outcomes:

- the first command has no Edit-schema consumer hits; unrelated
  instruction-registry or regex API hits are explicitly classified and left
  untouched;
- both tool docs contain the canonical contract and failure semantics;
- both instruction docs state all-target whole-call behavior.

Repair Track:

- Root cause: contract consumers are distributed across code, fixtures, and
  paired docs.
- Canonical owner: the new schema and approved design.
- Stable repair: migrate all consumers in one workstream.

Retirement Track:

- Deletion class: contract-carrying internal code.
- Old path: single-file Edit schema and every consumer.
- New canonical owner: prepared `files[]` batch.
- External boundary evidence: none.
- Retirement path: delete-first.

Commit checkpoint: root only after paired-doc and lingering-reference checks.

## Task 7: Run Final Scoped Verification, Architecture Review, And Root Commit

Files:

- No new implementation surface by default
- Add or amend an ADR only if the completion-time ADR assessment says the
  durable architecture decision is not already represented by an authoritative
  record
- Update `docs/aegis/INDEX.md` only if an ADR is created

Why:

The final gate must prove the approved contract, retirement, UI boundary,
cross-platform behavior, and documentation parity without hiding unrelated
worktree state.

Steps:

1. Inspect `git status --short` and record all unrelated pre-existing changes.
   Never revert, restore, stash, or clean them.
2. Review the complete scoped diff against all sixteen design acceptance
   criteria and the Frozen Contract section in this plan.
3. Re-run the smallest exact test from each prior task after all integration
   edits. Add another exact test only when fresh evidence identifies a boundary
   not covered by the named regressions.
4. Run formatting only for touched Rust files with edition 2024, then run the
   check form on the same files. Do not format unrelated files.
5. Run `git diff --check` against the exact touched path list.
6. Re-run the legacy-consumer scan and classify unrelated lexical hits.
7. Run the configured Aegis workspace structural check.
8. Perform the ADR backfill assessment using the design's architecture signal:
   - create/amend only when no current architecture record owns prepared Edit
     execution, approval binding, and partial commit semantics;
   - otherwise record `no ADR change` with the existing authority reference;
   - do not invent implementation evidence before tests pass.
9. Verify Delegate-family layout modules have no structural redesign.
10. Stage only implementation, focused tests, paired docs, and any justified
    ADR/index file. The root executor alone runs `git add` and `git commit`.
11. Use a conventional commit message such as:

```text
feat: prepare and commit batch Edit calls
```

Required final commands:

```bash
cargo test --package neo-agent-core --test tool_files -- edit_batch_applies_ordered_replacements_across_files --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn -- runtime_edit_approval_uses_verified_projection_and_multi_key_scope --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn -- instruction_preflight_defers_whole_edit_for_one_new_scope --exact --nocapture
cargo test --package neo-agent-core --test multi_agent_runtime -- replayed_unfinished_edit_is_interrupted_and_not_resumed --exact --nocapture
cargo test --package neo-tui --test tool_cards -- edit_batch_card_renders_collapsed_expanded_and_narrow --exact --nocapture
cargo test --package neo-tui --test tool_cards -- edit_batch_card_distinguishes_prepare_stale_partial_and_durability --exact --nocapture
git diff --check -- crates/neo-agent-core crates/neo-tui docs/en docs/zh docs/aegis
python /Users/chenyuanhao/.codex/aegis/scripts/aegis-workspace.py check --root /Users/chenyuanhao/Workspace/neo
```

Expected outcomes:

- every exact test exits 0;
- formatting/check commands report no touched-file drift;
- legacy Edit consumers are absent;
- `git diff --check` exits 0;
- Aegis workspace check exits 0;
- staged paths contain no unrelated user changes.

## Anti-Entropy Declaration

- Deletion Class: contract-carrying internal code
- Old Path/Object: single-file Edit schema, `replace_all`, and every old-field
  consumer
- New Canonical Owner: ordered `files[]` prepared execution in `edit.rs`
- Expected Preserved Behavior: exact replacement in existing UTF-8 files and
  unified diff reporting
- Expected Retired Behavior: one-file invocation, `replace_all`, raw guessed
  approval diff, and direct unprepared write
- External Boundary Touched: no proven active dependency
- Source-of-Truth Data Risk: none; implementation changes code behavior but
  performs no live data deletion during development
- User Confirmation Required: no

## Retirement Decision

- Path: delete-first
- Why: this is an internal tool contract with no proven active external
  dependency and the user explicitly approved canonical-only replacement.
- Non-edits: do not remove unrelated `replace_all` APIs or generic uses of
  words `old` and `new`; do not delete user files or persistent state.

## Verification Plan

- Main-path check: exact core/runtime/TUI tests prove prepared batch success.
- Lingering-reference check: scoped source/test/doc scan proves old consumers
  are gone.
- Negative check: legacy schema rejection and no-write mismatch/stale tests.
- Boundary check: Delegate structure, non-Edit Direct execution, Plan mode,
  instruction scopes, atomic writer, replay, and paired docs remain aligned.

## Risks And Mitigations

1. Race after the final just-in-time fingerprint check.
   Mitigation: document the filesystem limitation; never claim compare-and-swap
   or cross-file transaction semantics.
2. Partial commit after an I/O or durability error.
   Mitigation: exact per-file states, failed tool result, no rollback, clear
   provider text, and structured TUI.
3. Prepared contents increasing memory use for large batches.
   Mitigation: no speculative product cap; retain only data required by the
   approved diff/commit contract and avoid duplicate TUI persistence.
4. Shared runtime/permission modules growing Edit-specific logic.
   Mitigation: narrow routing in shared owners and semantic methods in
   `PreparedEdit`.
5. Approval and running cards diverging.
   Mitigation: both consume projection derived from the same prepared payload;
   success diff equality is tested.
6. Delegate card regression.
   Mitigation: summary-only core change and explicit structural diff review.
7. Dirty worktree contamination.
   Mitigation: root-only Git mutation, exact path staging, no revert/stash/clean.

## Stop And Escalate Conditions

Stop implementation and return to the design owner if any of these appears
necessary:

- adding an `apply_patch` or differently named patch tool;
- accepting the legacy Edit schema in any compatibility path;
- adding a generic `Tool::prepare`/`Tool::commit` abstraction;
- adding a new `AgentEvent` solely for Edit;
- adding cross-file rollback, a transaction claim, or write-ahead journal;
- adding fuzzy, normalized, occurrence-based, or line-hint matching;
- allowing Edit to create, delete, move, or follow link-like targets;
- changing Delegate-family card layout or expansion semantics;
- persisting or resuming `PreparedEdit` after restart;
- discovering a proven external dependency on the old schema;
- discovering persistent-state deletion or another irreversible operation.

Do not work around a stop condition with a fallback. Capture the exact source
evidence and ask for a design decision.

## Completion Handoff Contract

The implementing AI must return:

- commit hash(es) created by the root executor;
- exact changed-path list;
- exact test commands and exit results;
- classified lingering-reference scan results;
- confirmation that Delegate-family structure was preserved;
- ADR assessment result and any ADR path;
- uncovered platform/runtime risk;
- explicit statement that no legacy compatibility, second tool, rollback,
  journal, fuzzy matching, or generic Tool prepare/commit abstraction was added.
