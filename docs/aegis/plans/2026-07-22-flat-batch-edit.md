# Neo Flat Batch Edit Contract Implementation Plan

> Executor note: implement the approved flat contract exactly. Do not restart
> product discovery, compare reference projects, restore either prior Edit
> schema, add argument-repair fallbacks, add a generic batch abstraction, or
> redesign prepared execution, approval, Edit cards, or Delegate-family cards.

## Goal

Replace the nested model-visible
`Edit { files: [{ path, replacements: [...] }] }` contract with the approved
flat `Edit { edits: [{ path, old, new, expected_matches? }] }` contract while
preserving multi-file prepared execution, ordered staging, verified approval,
stale rechecks, per-file atomic commits, truthful partial results, finalized
Edit presentation, and non-resumable replay.

Delete every active nested Edit consumer and current reference in the same
workstream. Do not retain compatibility decoding or a second schema owner.

## Architecture

```text
model-visible edits[]
      |
      v
edit.rs validation + stable-path grouping
      |
      v
existing PreparedEdit { files: Vec<PreparedEditFile> }
      |
      +-> instruction probes / Plan guard / session approval
      +-> verified Ask approval
      +-> stale recheck / atomic per-file commit
      +-> existing structured details
             |
             +-> finalized Edit cards
             +-> replay
             +-> Delegate terminal summaries
```

Only the wire input and raw-argument projections change. `edit.rs` remains the
canonical owner. Runtime authorization consumes `PreparedEdit`; finalized UI
consumes structured result details. No new adapter, owner, event, or card state
is added.

## Tech Stack

- Rust 2024, minimum Rust 1.96.1
- `serde`, `serde_json`, `schemars`
- standard-library `HashMap` and existing `HashSet`
- existing `PreparedEdit`, approval, atomic-file, diff, instruction-scope,
  transcript, and multi-agent summary owners
- `cargo nextest`, `cargo clippy`, `cargo fmt`, `rg`, and `git diff --check`

## Baseline And Authority Refs

- `AGENTS.md`
- `docs/aegis/specs/2026-07-22-flat-batch-edit-design.md` (approved canonical
  requirement and architecture contract)
- `docs/aegis/specs/2026-07-20-batch-edit-design.md` (superseded historical
  prepared-execution evidence only)
- `docs/aegis/specs/2026-07-17-canonical-approval-protocol-design.md`
- `docs/aegis/specs/2026-07-17-path-scoped-agents-instructions-design.md`
- current source and tests listed in the file map below

## Compatibility Boundary

Hard break, canonical-only:

```json
{"edits":[{"path":"src/a.rs","old":"a","new":"A"}]}
```

The nested `files[].replacements[]` input is invalid. Do not add a decoder,
alias, adapter, fallback, feature flag, deprecation period, or dual renderer.
Migrate all active internal callers, tests, raw projections, and current docs.

Stable boundaries:

- `Write` keeps its independent `files[]` contract;
- `PreparedEdit` remains runtime-only and non-persistent;
- permission modes and the canonical approval protocol remain unchanged;
- result detail keys `files`, `replacements`, and `changes[]` remain unchanged;
- finalized Edit card content and expansion behavior remain unchanged;
- Delegate, DelegateGroup, and DelegateSwarm layouts, ordering, row budgets,
  output previews, and expansion behavior remain unchanged;
- historical releases, superseded specs, and completed plans remain historical
  evidence rather than being rewritten as current documentation.

## TDD Route

- Mode: off.
- Decision: skipped.
- Strict authority: not applicable; neither the user nor project requested
  strict test-first TDD.
- Test posture: minimum contract migration followed by focused regression tests
  at each affected owner boundary.
- Reason: this is an approved contract correction with existing safety
  behavior, not an explicit strict-TDD task.
- Verification: every test command names one package, one target selector, and
  a precise test filter.

## Verification

Use only focused commands as evidence:

```bash
cargo nextest run -p neo-agent-core --test tool_files edit_flat_batch_applies_ordered_edits_across_files
cargo nextest run -p neo-agent-core --test tool_files edit_flat_contract_is_model_visible_and_strict
cargo nextest run -p neo-agent-core --test tool_files edit_flat_match_mismatch_reports_global_index_and_writes_nothing
cargo nextest run -p neo-agent-core --lib typed_scope_probes_cover_every_edit_parent
cargo nextest run -p neo-agent-core --lib edit_tool_summary_preserves_counts_and_head_tail_within_budget
cargo nextest run -p neo-tui --test tool_cards edit_streaming_preview_shows_flat_intent
cargo fmt --all --check
cargo clippy -p neo-agent-core --lib -- -D clippy::all
cargo clippy -p neo-agent-core --test tool_files -- -D clippy::all
cargo clippy -p neo-tui --test tool_cards -- -D clippy::all
git diff --check
```

Do not use broad `cargo test` or package-wide unfiltered `cargo nextest run` as
completion evidence.

## Aegis Visibility

Planning protects the canonical Edit owner and prepared safety invariants while
retiring the defective nested wire contract. It prevents a local prompt tweak,
compatibility fallback, or consumer-side repair from leaving the model with two
argument shapes.

## Plan Basis

### Facts

- The current model-visible schema is nested through `files[]` and
  `replacements[]`.
- `expected_matches` belongs to the innermost item and defaults to `1`.
- Observed model output has placed `expected_matches` on the file object.
- `PreparedEdit` already owns grouped per-file staging, verified diffs,
  fingerprints, rechecks, commits, and structured details.
- Typed instruction probes, live TUI intent, and multi-agent activity summaries
  parse raw Edit arguments and therefore require direct migration.
- Finalized cards and approval presentations consume prepared or structured
  details and do not require redesign.

### Assumptions

- No published external client depends on the unreleased/internal nested Edit
  schema. Existing project policy already permits hard retirement when no
  active dependency is proven.
- The same requested `PathBuf` spelling is sufficient to group multiple edits
  for one target; alternate spellings that canonicalize to one target remain an
  error.

### Unknowns

- Provider-specific first-call conformance cannot be proven by deterministic
  unit tests. The model-visible schema and real-session retry rate remain the
  observation boundary after implementation.

## Baseline Usage Draft

- Required baseline refs: approved flat design, superseded Batch Edit design,
  approval protocol, path-scoped instructions, and `AGENTS.md`.
- Acknowledged before plan refs: all required refs above.
- Cited in plan refs: all required refs above.
- Missing refs: none.
- Decision: continue.

## Requirement Ready Check

- Requirement source refs: explicit user confirmation to retain multi-file
  batch plus the approved flat design.
- Goals and scope refs: design Goal, Approved Product Decisions, Compatibility
  And Retirement, and Acceptance Criteria.
- User/scenario refs: AI models repeatedly issue Edit calls during coding and
  currently waste calls and tokens on nested-field mistakes.
- Requirement item refs: flat `edits[]`, optional item-local match count,
  ordered same-path staging, strict rejection, and preserved runtime safety.
- Acceptance refs: design Verification Strategy and Acceptance Criteria.
- Open blocker questions: none.
- Decision: ready.

## Ripple Signal Triage

- Shared/core contract: yes, built-in tool schema.
- Producer and consumers: `edit.rs` produces the schema and prepared details;
  instruction probes, live intent, Delegate summaries, tests, and docs consume
  raw arguments.
- Source-of-truth change: model-visible input only; `PreparedEdit` remains the
  runtime source of truth after preparation.
- Compatibility: hard retirement, no external dependency evidence.
- Required verification expansion: core tool behavior plus every raw-argument
  consumer; finalized detail consumers need regression confirmation, not
  redesign.
- Decision: proceed as one bounded contract migration.

## Change Necessity

- User-visible need: reduce malformed AI Edit calls and retry/token waste while
  retaining one-call multi-file editing.
- No-change / non-code option: description-only changes leave the three-level
  schema and cannot prevent field-placement errors.
- Why code change is necessary: the generated tool schema comes from Rust input
  types, so only replacing the canonical input type removes the bad nesting.
- Minimum change boundary: `edit.rs`, active raw-argument consumers, focused
  tests, and current English/Chinese references.
- Decision: code-change.

## Existence Check

- Proposed new surface: none.
- Existing owner / reuse candidate: `EditTool`, `PreparedEdit`, instruction
  probes, Edit renderer, and multi-agent summary.
- Why existing surface is sufficient: only their input parsing changes; their
  ownership remains correct.
- Creation proof: not applicable.
- Entropy / retirement impact: one input struct and nested consumer shape are
  deleted; no fallback or adapter remains.
- Decision: reuse-existing.

## Architecture Integrity Lens

- Invariant: all semantic validation and staged bytes are owned by `edit.rs`
  before authorization or presentation.
- Canonical owner / contract: flat `EditInput` in `edit.rs`, then
  `PreparedEdit`.
- Responsibility overlap: none; raw projections may summarize but never
  authorize or stage edits.
- Higher-level simplification: replace nested wire types in place and retain
  the already-correct prepared representation.
- Retirement / falsifier: any active `files[].replacements[]` Edit parser,
  decoder, current doc, or fallback falsifies completion.
- Verdict: aligned after delete-first migration.

## Plan Pressure Test

- Owner / contract / retirement: one canonical flat schema; nested schema
  deleted.
- Architecture integrity / higher-level path: prepared execution remains the
  stable higher-level owner; no generic tool abstraction is needed.
- Verification scope: core schema/behavior, instruction probes, raw summaries,
  live intent, docs, and lingering references.
- Task executability: each task names exact files, behavior, tests, and commands.
- Pressure result: proceed.

## Complexity Budget

- Artifact class: core tool owner plus two bounded raw-projection consumers.
- Target files / artifacts: `edit.rs`, `tool_arguments.rs`,
  `multi_agent/runtime.rs`, `edit_tool_presentation.rs`, focused tests, docs.
- Current pressure: `edit.rs` and multi-agent runtime are large maintained
  files, but the touched functions already own this behavior.
- Projected post-change pressure: line-neutral or reduced in `edit.rs`; small
  local replacements elsewhere.
- Budget result: within-budget if no helper module, generic adapter, or fallback
  is added.
- Planned governance: delete nested wire types, keep grouping private and
  direct, reuse prepared/file detail types.

## Plan-Time Complexity Check

- Target files: current Edit owner and raw argument projections only.
- Existing size / shape signals: large files, but cohesive target functions
  with no new responsibility.
- Owner fit: direct.
- Add-in-place risk: accidental duplication if flat parsing is added beside the
  nested path.
- Better file boundary: none; extraction would create a one-use abstraction.
- Recommendation: edit in place and delete the old path in the same task.

## Execution Readiness View

- Intent Lock: make AI Edit input flat while preserving multi-file batch and
  current safety behavior.
- Scope Fence: input types, private grouping, diagnostics, raw-argument
  consumers, focused tests, and current docs.
- Baseline Lock: approved flat design and existing prepared-execution
  invariants.
- Approved Behavior: flat `edits[]`, optional `expected_matches`, ordered
  same-path staging, hard rejection of nested input.
- Owner / Contract Constraints: `edit.rs` owns correctness; prepared details
  remain authoritative downstream.
- Compatibility Boundary: no nested decoder, alias, fallback, or dual docs.
- Retirement Boundary: remove active `files[].replacements[]` Edit references;
  preserve historical artifacts.
- Task Batches: core owner, runtime probes, raw projections, docs/retirement,
  final verification.
- Test Obligations: exact schema, multi-file/same-path behavior, mismatch,
  probes, summaries, live intent, lingering reference search.
- Review Gates: stop if prepared safety semantics or card layouts would need
  redesign.
- Drift / Rewind Rules: return to the spec rather than adding compatibility or
  provider-specific repair.
- Evidence Required Before Completion: focused tests, clippy/fmt, reference
  search, diff review, and architecture/ADR backfill check.
- Advisory Boundary: method-pack execution guidance only; not a
  `GateDecision`, `PolicySnapshot`, or completion authority.

## Frozen Contract

### Input Types

Replace the nested deserializable types with:

```rust
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditInput {
    #[schemars(
        description = "Non-empty ordered exact-text edits. Declaration order is meaningful."
    )]
    edits: Vec<EditOperationInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditOperationInput {
    #[schemars(
        description = "Path to an existing file. Relative paths resolve against the working directory."
    )]
    path: PathBuf,
    #[schemars(
        description = "Exact non-empty current UTF-8 text to replace. Include enough context to make it unique."
    )]
    old: String,
    #[schemars(description = "Replacement text. Empty removes the matched text.")]
    new: String,
    #[serde(default = "default_expected_matches")]
    #[schemars(
        description = "Optional exact non-overlapping match count for this edit. Defaults to 1."
    )]
    expected_matches: usize,
}
```

Use one private grouping type, not another wire contract:

```rust
#[derive(Debug)]
struct EditFilePlan {
    path: PathBuf,
    edits: Vec<(usize, EditOperationInput)>,
}

fn group_edits(input: EditInput) -> Vec<EditFilePlan> {
    let mut file_indices = HashMap::<PathBuf, usize>::new();
    let mut files = Vec::<EditFilePlan>::new();
    for (edit_index, edit) in input.edits.into_iter().enumerate() {
        if let Some(&file_index) = file_indices.get(&edit.path) {
            files[file_index].edits.push((edit_index, edit));
        } else {
            let path = edit.path.clone();
            file_indices.insert(path.clone(), files.len());
            files.push(EditFilePlan {
                path,
                edits: vec![(edit_index, edit)],
            });
        }
    }
    files
}
```

This helper groups only identical requested `PathBuf` values. Keep the existing
resolved-target `HashSet` check so alternate spellings of one effective target
remain invalid.

### Result Contract

Preserve terminal and progress detail meanings:

```text
files        = distinct prepared paths
replacements = edits.len()
changes[]    = one entry per prepared path
```

For item-specific prepare failures, replace `replacement_index` with the global
flat `edit_index`. Do not change commit-time `file_index` semantics.

### Tool Description

Use the exact example-first text from the approved spec's
`Canonical Tool Description` section. Do not append the retired nested shape or
implementation-only partial-commit details.

## File Map

| File | Change |
| --- | --- |
| `crates/neo-agent-core/src/tools/edit.rs` | Replace nested input types, validate/group flat edits, use global edit indexes, update schema description and errors. |
| `crates/neo-agent-core/tests/tool_files.rs` | Replace nested fixtures and cover schema shape, same-path ordering, strict retirement, and mismatch diagnostics. |
| `crates/neo-agent-core/src/runtime/tool_arguments.rs` | Extract every `edits[].path` for typed instruction probes and update partial-argument fixtures. |
| `crates/neo-agent-core/src/multi_agent/runtime.rs` | Summarize distinct paths and edit count from flat raw arguments; update focused tests only. |
| `crates/neo-tui/src/transcript/edit_tool_presentation.rs` | Render live unverified intent from flat edits grouped by path. |
| `crates/neo-tui/tests/tool_cards.rs` | Update the live raw-argument fixture and assertion; keep finalized detail/card tests unchanged. |
| `docs/en/reference/tools.md` | Publish the flat canonical Edit contract and semantics. |
| `docs/zh/reference/tools.md` | Publish the same contract and semantics in Chinese. |

No change is planned for `approval.rs`, prepared permission routing, final Edit
detail rendering, diff models, session replay, or Delegate-family layout.

## Task Dependencies

```text
Task 1 core flat owner
   |
   +-> Task 2 instruction probes
   +-> Task 3 raw UI and Delegate projections
   +-> Task 4 docs and retirement search
              |
              v
          Task 5 final verification
```

Tasks 2 and 3 depend on the frozen contract but are otherwise independent.
Do not execute them against a dual-schema intermediate commit.

## Task 1: Replace The Canonical Edit Input And Preserve Prepared Semantics

**Files**

- Modify: `crates/neo-agent-core/src/tools/edit.rs`
- Modify: `crates/neo-agent-core/tests/tool_files.rs`

**Why**

This is the root repair. Description-only changes cannot remove the nested
schema generated from Rust types.

**Change Necessity**

Delete `EditFileInput` and `EditReplacementInput`; add the frozen flat types and
private grouping helper. Reuse `PreparedEditFile` and every post-staging path.

**Impact / Compatibility**

- Accept only `edits[]`.
- Allow repeated identical paths and preserve global edit order per path.
- Keep alternate-path aliases to one resolved target invalid.
- Keep result `files` and `replacements` counters stable.
- Change item failure details from `replacement_index` to `edit_index`.

**Steps**

1. Replace imports with `use std::collections::{HashMap, HashSet};`.
2. Replace the nested input structs with the exact frozen types.
3. Rewrite `validate_edit_input` to require a non-empty `edits` array and
   validate each item by global index: non-empty path/old, positive count, and
   `old != new`.
4. Add `EditFilePlan` and `group_edits` exactly as frozen above. Capture
   `let total_edits = input.edits.len();` before moving `input` into grouping.
5. In `PreparedEdit::prepare`, parse and validate before grouping, then run the
   existing file metadata, workspace, UTF-8, fingerprint, diff, and duplicate
   effective-target checks once per grouped path.
6. Apply each grouped `(edit_index, edit)` in declaration order against the
   file's staged content. On mismatch, report the global index and guidance:

```rust
format!(
    "expected {} exact matches · found {actual}; matches at lines {line_list}",
    edit.expected_matches
)
```

```text
Use a more specific edits[N].old, or set edits[N].expected_matches to ACTUAL only if every match is intended.
```

7. Set each prepared file's replacement count to `file.edits.len()` and the
   batch replacement count to `total_edits`.
8. Update `prepare_failed` and structured details to emit `edit_index`; retain
   `file_index` only for file-level failures.
9. Replace `EditTool::description` with the approved exact text and change
   generic parse guidance to the canonical one-line JSON example.
10. Update existing in-file and integration fixtures to `edits[]`.
11. Add or reshape these focused tests:

```rust
async fn edit_flat_batch_applies_ordered_edits_across_files()
async fn edit_flat_contract_is_model_visible_and_strict()
async fn edit_flat_match_mismatch_reports_global_index_and_writes_nothing()
```

The first must repeat one identical path non-contiguously and prove the later
edit sees prior staged content. The schema test must inspect `Edit`'s emitted
tool spec, assert root `edits`, assert flat item properties, and prove the old
nested shape plus root-level `expected_matches` are rejected with zero writes.
The mismatch test must assert `edit_index`, actual/expected count, line evidence,
actionable `edits[N]` guidance, and unchanged files.

**Verification**

```bash
cargo nextest run -p neo-agent-core --test tool_files edit_flat_batch_applies_ordered_edits_across_files
cargo nextest run -p neo-agent-core --test tool_files edit_flat_contract_is_model_visible_and_strict
cargo nextest run -p neo-agent-core --test tool_files edit_flat_match_mismatch_reports_global_index_and_writes_nothing
```

Expected: each focused test passes; no old nested input is accepted.

**Repair Track**

- Root cause: model-visible three-level schema permits item-local fields to be
  generated at the file level.
- Canonical owner: `edit.rs` input types and preparation.
- Stable repair: one flat item type plus private same-path grouping.
- Verification: emitted schema and prepared behavior tests above.

**Retirement Track**

- Old owner: deserializable `EditFileInput` and `EditReplacementInput`.
- Active status after task: deleted.
- Keep reason: none.
- Deletion proof: strict rejection test and later lingering-reference search.

## Task 2: Migrate Typed Instruction Probes And Argument Repair Fixtures

**Files**

- Modify: `crates/neo-agent-core/src/runtime/tool_arguments.rs`

**Why**

Instruction preflight must still discover every target parent before prepared
execution. This consumer reads raw arguments and cannot keep the nested path.

**Change Necessity**

Change only the existing Edit branch and its tests; do not alter Write or other
tool probes.

**Impact / Compatibility**

- Extract each `edits[].path`.
- Preserve directory de-duplication and declaration order.
- Repeated identical paths produce one parent probe.
- Keep partial JSON repair generic and schema-driven.

**Steps**

1. In `InstructionScopeProbe::from_prepared_tool`, replace the Edit-specific
   `files[]` traversal with `edits[]` traversal and reuse the existing path
   resolution/de-duplication helper.
2. Update the partial-argument repair fixture whose required top-level Edit
   field is currently `files` so it uses a complete `edits` array.
3. Rewrite `typed_scope_probes_cover_every_edit_parent` with flat items for two
   directories and one repeated path; keep the expected de-duplicated order.
4. Do not parse shell strings or add a fallback for `files[]`.

**Verification**

```bash
cargo nextest run -p neo-agent-core --lib typed_scope_probes_cover_every_edit_parent
```

Expected: the exact probe test passes and reports one probe per distinct parent.

**Repair Track**

- Canonical owner: typed raw-argument probe extraction.
- Stable repair: switch the existing Edit traversal to the one canonical root.

**Retirement Track**

- Old path: Edit-only `files[].path` extraction and fixture.
- Active status after task: deleted.
- Keep reason: none.

## Task 3: Migrate Raw Intent And Delegate Summaries Without Card Redesign

**Files**

- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-tui/src/transcript/edit_tool_presentation.rs`
- Modify: `crates/neo-tui/tests/tool_cards.rs`

**Why**

These are the only active raw-argument presentation consumers. They must count
flat items without changing any finalized prepared/detail presentation.

**Change Necessity**

Replace local argument traversal only. Reuse existing bounded summary and code
frame rendering functions.

**Impact / Compatibility**

- Distinct-file count is the number of first-seen unique requested paths.
- Replacement count is `edits.len()`.
- Head/tail paths are first/last distinct requested paths.
- Live unverified intent renders one row per distinct path with that path's edit
  count.
- Final cards, progress, approval, partial status, and Delegate layout remain
  unchanged.

**Steps**

1. Rewrite `summarize_edit_arguments` to read `edits[]`, collect distinct paths
   in first-seen order, use `edits.len()` for replacements, and pass the same
   formatted string through `bounded_edit_summary`.
2. Update `edit_tool_summary_preserves_counts_and_head_tail_within_budget` to
   use four flat edits across three paths, including one repeated path.
3. Leave `summarize_edit_details`, progress, terminal partial, interruption,
   and replay functions unchanged.
4. Rewrite `render_streaming_or_intent` to parse `edits[]`, build a local
   first-seen `Vec<(path, count)>`, and render the existing summary plus one
   existing code frame per distinct path.
5. Rename the focused TUI test to
   `edit_streaming_preview_shows_flat_intent`, provide flat raw JSON, and assert
   the path plus `unverified intent`. Do not modify finalized Edit card tests.

Use simple local vectors for first-seen grouping; do not add a shared summary
abstraction for two small consumers.

**Verification**

```bash
cargo nextest run -p neo-agent-core --lib edit_tool_summary_preserves_counts_and_head_tail_within_budget
cargo nextest run -p neo-tui --test tool_cards edit_streaming_preview_shows_flat_intent
```

Expected: both exact tests pass; existing row budgets and frames are unchanged.

**Repair Track**

- Canonical owners: existing raw summary functions.
- Stable repair: direct traversal of flat items.
- Compatibility boundary: prepared/detail renderers unchanged.

**Retirement Track**

- Old path: raw `files[]/replacements[]` summary traversal.
- Active status after task: deleted.
- Keep reason: none.

## Task 4: Publish The Canonical Contract And Remove Active Nested References

**Files**

- Modify: `docs/en/reference/tools.md`
- Modify: `docs/zh/reference/tools.md`

**Why**

Current reference docs are active model/user guidance. Leaving the nested shape
there would preserve a second owner after the code migration.

**Change Necessity**

Replace only Edit contract text. Do not change Write's independent `files[]`
contract or rewrite historical release/spec/plan records.

**Impact / Compatibility**

- Both languages show the same normal flat example with omitted
  `expected_matches`.
- Both explain item-local intentional counts, repeated same-path ordering, zero
  writes on prepare failure, and hard rejection of the nested shape.

**Steps**

1. Replace Edit's reference table row and detailed section in English.
2. Apply the equivalent content in Chinese.
3. Search active sources and current references:

```bash
rg -n 'files\[\]\.replacements|using the files\[\] contract|arguments\.get\("files"\)' crates/neo-agent-core/src/tools/edit.rs crates/neo-agent-core/src/runtime/tool_arguments.rs crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-tui/src/transcript/edit_tool_presentation.rs docs/en/reference/tools.md docs/zh/reference/tools.md
```

Expected: no active nested Edit contract match. Investigate any match; do not
silence the search by broad exclusions inside these target files.

4. Search the new canonical root:

```bash
rg -n 'edits\[\]|"edits"|expected_matches' crates/neo-agent-core/src/tools/edit.rs crates/neo-agent-core/src/runtime/tool_arguments.rs crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-tui/src/transcript/edit_tool_presentation.rs docs/en/reference/tools.md docs/zh/reference/tools.md
```

Expected: every active owner and current reference uses the flat contract.

**Repair Track**

- Canonical owner: code-generated schema, mirrored by paired current docs.
- Verification: paired text plus active-reference search.

**Retirement Track**

- Old path: current English/Chinese nested Edit documentation.
- Active status after task: deleted.
- Historical evidence: retained outside current reference docs.

## Task 5: Run Final Scoped Verification And Architecture Review

**Files**

- Review only: every file in the file map
- Modify only if a focused failure proves an in-scope defect

**Why**

Completion requires evidence that the new owner works, the old owner died, and
the preserved safety/presentation boundaries did not drift.

**Change Necessity**

No planned source additions. Any repair must remain inside the established
owner and receive the narrowest corresponding regression.

**Steps**

1. Run the focused core behavior and schema tests:

```bash
cargo nextest run -p neo-agent-core --test tool_files edit_flat_batch_applies_ordered_edits_across_files
cargo nextest run -p neo-agent-core --test tool_files edit_flat_contract_is_model_visible_and_strict
cargo nextest run -p neo-agent-core --test tool_files edit_flat_match_mismatch_reports_global_index_and_writes_nothing
```

2. Run the exact raw-consumer tests:

```bash
cargo nextest run -p neo-agent-core --lib typed_scope_probes_cover_every_edit_parent
cargo nextest run -p neo-agent-core --lib edit_tool_summary_preserves_counts_and_head_tail_within_budget
cargo nextest run -p neo-tui --test tool_cards edit_streaming_preview_shows_flat_intent
```

3. Run focused lint and formatting checks:

```bash
cargo fmt --all --check
cargo clippy -p neo-agent-core --lib -- -D clippy::all
cargo clippy -p neo-agent-core --test tool_files -- -D clippy::all
cargo clippy -p neo-tui --test tool_cards -- -D clippy::all
git diff --check
```

4. Run both retirement searches from Task 4 and inspect the diff for a hidden
   decoder, alias, provider branch, changed finalized detail contract, or card
   layout drift.
5. Confirm `git status --short` contains only the intended task files plus
   unrelated pre-existing user changes. Never revert or include unrelated work.
6. Perform the completion-time ADR backfill check. If implementation evidence
   shows the superseding spec is insufficient as durable architecture history,
   amend or add the smallest ADR through the dedicated ADR workflow; otherwise
   record that no ADR is needed because ownership and runtime architecture did
   not change.
7. Store the significant completion and any resolved error in ICM before final
   handoff.

Expected: every command exits `0`; active nested references are absent; no
unrelated files are staged.

**Repair Track**

- Root cause class: model-facing schema design defect.
- Canonical repair: flat schema at `edit.rs` with every raw consumer migrated.
- Verification: focused target tests plus schema/reference checks.

**Retirement Track**

- Old owner located: nested input types and active consumers.
- Deleted status: required for completion.
- Retention reason: none.
- Lingering-reference check: required and explicit.

**Single Logical Commit**

After every focused verification and retirement check passes, stage only the
implementation files named in this plan and create the repository-required one
logical-task commit:

```bash
git add crates/neo-agent-core/src/tools/edit.rs crates/neo-agent-core/tests/tool_files.rs crates/neo-agent-core/src/runtime/tool_arguments.rs crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-tui/src/transcript/edit_tool_presentation.rs crates/neo-tui/tests/tool_cards.rs docs/en/reference/tools.md docs/zh/reference/tools.md
git commit -m "fix(edit): flatten batch input contract"
git status --short
git log -5 --oneline
```

Do not stage unrelated shared-worktree changes and do not create intermediate
commits for the execution slices above.

## Anti-Entropy Declaration

```text
Deletion Class: contract-carrying code
Old Path/Object: nested Edit files[].replacements[] wire types and consumers
New Canonical Owner: flat Edit edits[] input in edit.rs
Expected Preserved Behavior: prepared multi-file exact editing and presentation
Expected Retired Behavior: nested arguments and replacement_index failures
External Boundary Touched: no proven active dependency
Source-of-Truth Data Risk: none
User Confirmation Required: no
```

## Retirement Decision

- Path: delete-first.
- Why: the old contract is internal, recently introduced, and has no proven
  active external dependency; retaining it recreates the reliability problem.
- Non-edits: historical artifacts remain; prepared execution, details, and card
  architecture are not retired.

## Verification Plan

- Main-path check: flat multi-file and repeated-path focused tests.
- Lingering-reference check: active owner/current-doc `rg` search.
- Negative check: old nested and misplaced-field payloads fail with zero writes.
- Boundary check: instruction probes, raw summaries, live intent, approval,
  finalized details, replay, and card invariants remain aligned.

## Risks And Mitigations

| Risk | Mitigation |
| --- | --- |
| Same-path edits commit twice | Group identical requested paths before existing preparation. |
| Alternate spellings silently alias | Preserve resolved-target duplicate rejection. |
| Global edit order is lost | Carry original global index in each grouped item and test non-contiguous same-path edits. |
| Instruction scopes miss a target | Migrate and verify typed probes over every flat path. |
| UI or Delegate summaries show wrong counts | Count distinct paths and `edits.len()` in the two raw consumers only. |
| Compatibility fallback survives | Hard negative test plus active lingering-reference search. |
| Final cards drift | Leave structured detail contract/renderers unchanged and inspect diff. |
| Deterministic tests overclaim model reliability | Report schema/behavior evidence separately from real-session first-call observation. |

## Stop And Escalate Conditions

Stop and return to the approved design if implementation appears to require:

- accepting both flat and nested input;
- provider-specific schema or argument repair;
- changing `PreparedExecution`, approval authority, or result detail shape;
- redesigning finalized Edit or Delegate-family cards;
- fuzzy matching, line-authoritative replacement, or automatic observed-count
  adoption;
- a new generic batch abstraction or tool trait;
- a persistent-state migration or external compatibility exception.

Do not add a fallback to make progress around any of these conditions.

## Completion Handoff Contract

Implementation is ready for review only when:

1. `edits[]` is the sole active Edit input;
2. all focused verification commands pass with exit code `0`;
3. old nested payloads fail with zero writes;
4. no active nested consumer or current reference remains;
5. prepared safety, result details, and finalized card behavior remain stable;
6. unrelated shared-worktree changes are neither staged nor modified;
7. the ADR/backfill and ICM completion checks are recorded.

The remaining non-automated risk is provider/model first-call behavior in real
sessions. The next most valuable post-implementation observation is whether the
same workloads stop producing schema-placement retries; this is observed actual
usage, not a predicted task-size or cost threshold.
