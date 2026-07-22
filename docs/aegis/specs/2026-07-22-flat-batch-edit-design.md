# Neo Flat Batch Edit Contract Design

Status: Approved
Date: 2026-07-22
Supersedes: `docs/aegis/specs/2026-07-20-batch-edit-design.md`

## Goal

Make model-issued `Edit` calls structurally easy to generate without removing
the multi-file batch, exact-match, prepared approval, stale-recheck, atomic
per-file commit, or truthful partial-failure guarantees already implemented by
Batch Edit.

The model-visible contract becomes one flat `edits[]` array. Each item owns its
`path`, `old`, `new`, and optional `expected_matches`. The nested
`files[].replacements[]` contract is deleted rather than retained as a decoder,
alias, fallback, or second owner.

## Problem And Evidence

The current contract requires the model to construct three semantic levels:

```text
files[]
  -> replacements[]
       -> expected_matches
```

Observed production failures include:

```text
expected 1 exact matches · found 2; matches at lines 93, 241
invalid Edit arguments: unknown field `expected_matches`, expected `path` or `replacements`
```

The first result is an intentional safety refusal: the supplied `old` text is
not unique in the current staged content. The second result proves the model
placed `expected_matches` on a `files[]` item instead of a
`files[].replacements[]` item. `EditTool` is stateless and the tool schema is
stable, so prior successful calls do not mutate this behavior. The nested wire
shape is the avoidable reliability cost.

## Approved Product Decisions

1. One `Edit` call continues to support ordered edits across multiple existing
   UTF-8 files.
2. The canonical model-visible input is `{"edits":[...]}`.
3. Each `edits[]` item directly owns `path`, `old`, `new`, and optional
   `expected_matches`.
4. `expected_matches` defaults to `1`. The ordinary single-match example omits
   it so the model generates less optional structure.
5. Edits execute in declaration order against staged content. Later edits to
   the same path see earlier staged edits to that path.
6. Repeated identical requested paths are valid and are grouped into one
   prepared file. The file commit order is the order of first path appearance.
7. Distinct requested path spellings that resolve to the same effective target
   remain invalid. The model must use one consistent path spelling per target.
8. Every target is prepared before any write. A schema, path, file-type,
   content, or match failure writes nothing.
9. Ask mode continues to approve verified prepared diffs rather than raw
   argument projections.
10. Approval continues to be followed by path and content fingerprint
    rechecks. Stale content is never silently overwritten.
11. Files continue to commit atomically one-by-one in first-appearance order.
    Cross-file transactional atomicity and automatic rollback are not claimed.
12. Partial and durability failures continue to report committed, failed, and
    not-attempted files truthfully.
13. Unknown fields remain rejected at every object level. Neo does not silently
    move a misplaced field or adopt the observed match count.
14. Structured result details remain file-oriented so approval, final Edit
    cards, replay, and Delegate-family terminal summaries keep their current
    contracts.
15. The current `files[].replacements[]` input is removed outright. No
    compatibility period, feature flag, decoder, alias, fallback, or dual
    documentation remains.

## Baseline And Authority

This design is constrained by:

- `AGENTS.md`, especially canonical-owner retirement, scoped verification,
  cross-platform behavior, and exact Delegate-card preservation;
- `docs/aegis/specs/2026-07-20-batch-edit-design.md`, as historical evidence
  for the prepared execution and presentation invariants retained here;
- `docs/aegis/specs/2026-07-17-canonical-approval-protocol-design.md`;
- `docs/aegis/specs/2026-07-17-path-scoped-agents-instructions-design.md`;
- the current `EditTool`, `PreparedEdit`, permission pipeline, transcript Edit
  renderer, and multi-agent activity summary.

The previous Batch Edit spec is superseded because its approved wire contract
conflicts with this one. Its implementation history remains evidence, not a
second active contract.

## Task Intent

- Outcome: models can issue one multi-file Edit call through the shallowest
  provider-compatible JSON object shape.
- User-visible value: fewer invalid tool calls, fewer retries, and less token
  expenditure caused by schema-placement mistakes.
- Success evidence: the model-visible schema exposes only `edits[]`; flat
  calls preserve multi-file ordered staging and current safety behavior; active
  nested-contract consumers and docs are gone.
- Stop condition: `edits[]` is the only accepted Edit input and every active
  source, test, prompt description, and current reference document agrees.
- Non-goals: fuzzy matching, automatic field relocation, automatic match-count
  adoption, freeform patch parsing, a second editing tool, or a redesign of
  prepared execution and Edit presentation.

## Canonical Ownership

```text
Read    -> inspect files
Write   -> create files or replace complete file contents
Edit    -> exact ordered replacements in existing UTF-8 regular files
```

`crates/neo-agent-core/src/tools/edit.rs` remains the only owner of Edit input
validation, path grouping, matching, staging, fingerprinting, diff calculation,
recheck, and commit. Runtime modules route prepared state and authorization.
`ToolCallComponent` remains the only stateful transcript card owner.

No new adapter or translation layer is introduced. Flat input is parsed
directly into the canonical Edit input type and grouped into the existing
runtime-only per-file prepared representation.

## Canonical Model-Visible Contract

Normal unique replacements omit `expected_matches`:

```json
{
  "edits": [
    {
      "path": "crates/neo-agent-core/src/tools/edit.rs",
      "old": "exact unique existing text",
      "new": "replacement text"
    },
    {
      "path": "crates/neo-agent-core/src/tools/mod.rs",
      "old": "old export",
      "new": "new export"
    }
  ]
}
```

Intentional multiple replacement supplies an observed exact count on the same
item:

```json
{
  "edits": [
    {
      "path": "src/lib.rs",
      "old": "OldType",
      "new": "NewType",
      "expected_matches": 3
    }
  ]
}
```

Required constraints:

| Field | Contract |
| --- | --- |
| `edits` | Non-empty array; declaration order is meaningful. |
| `edits[].path` | Non-empty existing-file path. |
| `edits[].old` | Non-empty exact UTF-8 substring. |
| `edits[].new` | Any UTF-8 string; empty removes the matched text. |
| `edits[].expected_matches` | Optional integer, default `1`, minimum `1`. |
| Unknown fields | Rejected at the root and item levels. |

Additional validation:

- `old == new` is rejected as a no-op;
- one requested path spelling may appear any number of times;
- two different requested path spellings resolving to the same effective
  target are rejected;
- no product-level file or edit count limit is introduced;
- provider payload and process memory limits remain machine-safety boundaries,
  not task-scale or cost governance.

## Ordered Staging And Grouping

The flat declaration order is authoritative:

```text
edits[0] path A -> staged A1
edits[1] path B -> staged B1
edits[2] path A -> staged A2, using staged A1
```

Preparation groups repeated identical requested paths for the existing
per-file prepared representation while preserving each edit's global index.
The first appearance of a path establishes file presentation and commit order.
Edits to different paths are independent during staging.

The prepared payload continues to contain one entry per distinct requested
path, with one final staged byte sequence, fingerprint, unified diff, and
replacement count. Structured result fields retain their existing meanings:

- `files`: distinct prepared-file count;
- `replacements`: number of `edits[]` items;
- `changes[]`: one entry per prepared file.

## Exact Match Semantics

For each item, Neo counts non-overlapping exact matches in that path's current
staged content and proceeds only when:

```text
actual_matches == expected_matches
```

Every counted match is replaced on success. Matching never normalizes
whitespace, line endings, Unicode, or punctuation, and never selects an
occurrence by line hint or proximity.

An expected count of `1` with multiple actual matches remains an error. The
tool cannot infer whether the model intended one location or every location.
The model must either provide a more specific `old` value or explicitly set the
observed count on that `edits[]` item.

## Canonical Tool Description

The model-visible description is intentionally example-first and omits runtime
internals that do not help argument construction:

```text
Apply ordered exact-text edits to existing UTF-8 files.

Use exactly this input shape:
{"edits":[{"path":"src/file.rs","old":"exact existing text","new":"replacement text"}]}

Each edits[] item is one replacement and contains:
- path: existing file path
- old: exact current text to replace
- new: replacement text; empty deletes old
- expected_matches: optional exact match count, default 1

Read each target before editing. For the normal single-match case, omit
expected_matches and include enough surrounding text in old to make it unique.
Set expected_matches only when intentionally replacing an observed exact count
greater than 1.

Items run in declaration order. Later edits to the same path see the staged
result of earlier edits. The entire call is prepared before any write.

If a match-count or stale-content error occurs, use the returned evidence to
construct a fresh Edit call. Do not replay the failed arguments.

Use Write to create files or replace complete file contents.
```

## Error Contract

Schema failures remain failed tool results with zero writes. Guidance must name
the canonical root and item shape, for example:

```text
invalid Edit arguments: ...
Submit exactly {"edits":[{"path":"...","old":"...","new":"..."}]}.
```

Semantic failures identify the global flat index, not a nested replacement
index:

```text
Edit prepare failed · zero writes · src/lib.rs · edit 2
expected 1 exact matches · found 2; matches at lines 93, 241
Use a more specific edits[2].old, or set edits[2].expected_matches to 2 only if both matches are intended.
```

Structured failure details use `edit_index` for item-specific failures.
`file_index` may remain for file-level preparation or commit failures.
`replacement_index` is retired from active Edit result details.

Neo does not:

- move `expected_matches` from the root or another object;
- accept `files[]` as an alias;
- replace every observed match after a count mismatch;
- reuse a failed prepared payload.

## Runtime And Presentation Boundaries

The following behavior remains unchanged after flat parsing and grouping:

1. typed instruction probes cover every distinct `edits[].path` parent;
2. instruction preflight completes before prepared execution is authorized;
3. Plan-mode guards check every prepared target;
4. Ask approval uses `EditApprovalPresentation` from verified diffs;
5. session approval keys remain one per prepared file;
6. stale rechecks cover every prepared file before commit;
7. progress and terminal details remain file-oriented;
8. replay never persists or resumes `PreparedEdit`;
9. finalized Edit cards render structured details exactly as before;
10. Delegate, DelegateGroup, and DelegateSwarm retain current card layout,
    ordering, row budgets, previews, and expansion behavior.

Only two raw-argument projections change shape:

- the live unverified Edit intent preview groups `edits[]` by path;
- the bounded Delegate activity summary counts distinct paths and edit items
  from `edits[]`.

## Compatibility And Retirement

This is a hard internal contract migration:

```json
{"files":[{"path":"src/a.rs","replacements":[{"old":"a","new":"A"}]}]}
```

is invalid after implementation. Historical release notes, superseded specs,
and completed plans remain historical evidence. Active source, tests, current
English and Chinese tool references, and the model-visible description must
use only `edits[]`.

There is no persistence migration. Tool-call arguments are session transcript
history, not resumable prepared execution. Replaying historical completed calls
continues to render from structured result details; unfinished Edit execution
continues to become interrupted and is never resumed.

## Verification Strategy

Focused evidence must cover:

1. the emitted model-visible schema has root `edits` and flat item properties;
2. one call stages ordered edits across multiple files;
3. repeated identical paths see prior staged content and commit once;
4. `expected_matches` defaults to `1` and intentional counts greater than one
   still work;
5. the old nested contract and misplaced fields are rejected with zero writes;
6. mismatch diagnostics use global `edit_index` and actionable flat paths;
7. typed instruction probes de-duplicate all flat edit parents;
8. live Edit intent and Delegate summaries derive correct distinct-file and
   edit counts from flat arguments;
9. existing prepared approval, stale, partial commit, final card, and replay
   tests remain valid without visual redesign;
10. a lingering-reference search finds no active nested Edit contract.

Verification commands must follow `AGENTS.md`: one package, one target selector,
and at least one precise test filter. A broad workspace test run is not required.

## Acceptance Criteria

1. One `Edit` call still modifies multiple existing UTF-8 files.
2. `edits[]` is the only accepted model-visible root.
3. Every edit item carries its own path and match contract.
4. Repeated identical paths preserve ordered staged semantics.
5. The ordinary single-match call does not require `expected_matches`.
6. Match-count mismatch anywhere produces zero writes.
7. Old nested input and misplaced fields are rejected, never repaired.
8. Ask approval, stale recheck, per-file atomic commit, partial failure, final
   card, and replay semantics remain unchanged.
9. Raw intent and Delegate summaries report distinct-file and edit counts from
   flat arguments without changing card layouts.
10. Current English and Chinese references describe the same canonical shape.
11. No active source or current documentation teaches
    `files[].replacements[]` for Edit.

## Non-Goals

- restoring the old single-edit `{path, old, new, replace_all}` contract;
- retaining the current nested batch contract;
- adding `apply_patch`, unified-diff parsing, or fuzzy edit behavior;
- accepting a line number as replacement authority;
- provider-specific argument repair or schema forks;
- changing Write's `files[]` contract;
- changing prepared result details or finalized Edit card design;
- changing Delegate-family layout, expansion, or content budgets;
- adding usage prediction, task-size limits, or cost-governance thresholds.

## Baseline Role Alignment

- Product / Requirement Baseline: preserve multi-file batch and all current
  safety semantics while materially reducing AI argument-shape failures.
- Architecture / Runtime Boundary Baseline: `edit.rs` remains the canonical
  owner; prepared execution and presentation consume grouped verified state.
- Result: Design Defect in the approved model-visible nested contract; the
  prepared runtime architecture remains aligned.
- Scope: requirements and architecture contract, not runtime ownership.
- Next action: replace the wire shape in place and retire every active nested
  consumer.

## Anti-Entropy Declaration

```text
Deletion Class: contract-carrying code
Old Path/Object: Edit files[].replacements[] input and active consumers
New Canonical Owner: flat edits[] input parsed by edit.rs
Expected Preserved Behavior: multi-file prepared exact replacement and presentation
Expected Retired Behavior: nested model-visible Edit arguments and replacement_index diagnostics
External Boundary Touched: no proven active dependency
Source-of-Truth Data Risk: none
User Confirmation Required: no
```

Retirement path: `delete-first`. Historical artifacts remain evidence, but no
active decoder, alias, fallback, or current reference retains the old contract.

## Architecture Signal

Architecture review is required because this changes a durable built-in tool
contract and its raw-argument projections. The ADR signal is `yes`; completion
must evaluate whether the existing architecture baseline needs an amendment or
whether the superseding spec is sufficient after implementation evidence exists.

## Planning Notes

- TDD Route is not strict unless separately authorized.
- Reuse `PreparedEdit`, `PreparedEditFile`, approval presentation, structured
  results, and existing card renderers.
- Do not add a generic batch abstraction or modify the `Tool` trait.
- Do not redesign Write, approval, transcript, or Delegate-family cards.
- If flat parsing cannot preserve an approved safety invariant, return to this
  design rather than adding compatibility logic.
