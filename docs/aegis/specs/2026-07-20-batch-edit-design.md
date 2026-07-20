# Neo Batch Edit Prepared Execution Design

Status: Approved
Date: 2026-07-20

## Goal

Upgrade Neo's existing `Edit` tool from one exact replacement in one file to
one ordered, structured batch of exact replacements across existing UTF-8
files. A batch must be inspectable before approval, reject all semantic errors
before writing, commit each file atomically, and report partial commits
truthfully without adding a second file-editing owner.

## Approved Product Decisions

1. `Edit` remains the canonical exact-replacement tool. No `apply_patch` tool
   is added.
2. `Write` remains the only owner for file creation and full-file overwrite.
3. `Edit` accepts `files[]`, with ordered `replacements[]` inside each file.
4. Replacements in one file run in declaration order against staged content.
5. `expected_matches` replaces `replace_all`; the default is exactly one.
6. All paths, permissions, file types, contents, matches, and diffs are
   prepared before approval. Any prepare failure writes nothing.
7. Ask mode approves a verified planned diff, not a projection guessed from
   raw tool arguments.
8. Approval is followed by path/content fingerprint rechecks. A stale target
   is never silently overwritten.
9. Files commit in declaration order through per-file atomic replacement.
   Cross-file transactional atomicity is not claimed.
10. A commit failure does not trigger automatic rollback. The result reports
    committed, failed, and not-attempted files exactly.
11. A partial commit is a failed tool result, never a successful result with a
    warning and never a new ambiguous global `Partial` state.
12. The ordinary transcript uses one Edit card with explicit head/tail
    elision; global `Ctrl+O` reveals the complete diff.
13. Delegate, DelegateGroup, and DelegateSwarm keep their current layout,
    ordering, row budgets, output previews, and expansion behavior. They only
    gain a bounded Edit summary projection.
14. The old `{ path, old, new, replace_all }` schema is removed outright. No
    compatibility decoder, alias, fallback, or dual-schema renderer remains.

## Baseline And Authority

This design is constrained by:

- `AGENTS.md`, especially scoped verification, cross-platform behavior,
  canonical-owner deletion, and exact Delegate-card preservation;
- `docs/aegis/specs/2026-07-17-canonical-approval-protocol-design.md`;
- `docs/aegis/specs/2026-07-17-path-scoped-agents-instructions-design.md`;
- `docs/aegis/specs/2026-07-20-bash-terminal-tool-card-brief.md`;
- the current `EditTool`, runtime permission pipeline, `PreparedToolCall`,
  `ToolCallComponent`, diff renderer, and multi-agent activity summary.

The current implementation facts that shape the change are:

- `EditTool` reads one UTF-8 file, performs an exact unique replacement or
  `replace_all`, writes directly, and returns one unified diff;
- `PreparedToolCall` currently carries parsed arguments and approval context,
  but no prepared execution payload;
- instruction preflight currently derives at most one typed path probe per
  tool call;
- ordinary Edit approval currently renders raw `path` and `replace_all`
  arguments;
- Write and Edit already have `Exclusive` scheduling class;
- the TUI currently renders Edit's proposed diff directly from raw arguments;
- child activity stores a bounded string summary instead of full child tool
  arguments.

## Task Intent

- Outcome: one model-issued Edit call can express a coherent multi-file exact
  modification and show the real change before a human approves it.
- Success evidence: contract, runtime, permission, instruction-scope, plan
  guard, TUI, Delegate-summary, replay, and documentation checks listed below.
- Stop condition: the new canonical Edit schema and prepared execution path
  are the only active Edit behavior; all approved visuals and failure states
  are implemented without redesigning generic tool or Delegate cards.
- Non-goals: freeform patch parsing, fuzzy matching, file creation/deletion/
  movement, cross-file rollback, write-ahead journaling, or tool-system-wide
  prepare/commit abstraction.

## Canonical Ownership

```text
Read    -> read files
Write   -> create files or replace complete file contents
Edit    -> exact replacements in existing UTF-8 regular files
```

`Edit` does not create, delete, or move files. It rejects directories,
symlinks, Windows reparse points, missing files, and non-UTF-8 contents.
Rejecting link-like targets avoids changing the link itself into a regular file
during atomic replacement.

The stateful card owner remains `ToolCallComponent`. Edit-specific TUI logic is
pure presentation. The runtime remains the only approval owner. `edit.rs`
remains the only owner of argument validation, matching, staging,
fingerprinting, diff calculation, recheck, and commit.

## Tool Input Contract

The canonical model-visible schema is:

```json
{
  "files": [
    {
      "path": "crates/neo-agent-core/src/tools/edit.rs",
      "replacements": [
        {
          "old": "old text",
          "new": "new text",
          "expected_matches": 1
        }
      ]
    }
  ]
}
```

Required constraints:

| Field | Contract |
|---|---|
| `files` | Non-empty array; declaration order is meaningful. |
| `files[].path` | Non-empty file path resolved by the existing workspace policy. |
| `files[].replacements` | Non-empty array; declaration order is meaningful. |
| `old` | Non-empty UTF-8 string. |
| `new` | Any UTF-8 string; empty means remove the matched text. |
| `expected_matches` | Integer, default `1`, minimum `1`. |
| Unknown fields | Rejected at every object level. |

Additional validation:

- two declared paths that resolve to the same effective target are rejected;
- `old == new` is rejected as a no-op;
- there is no product-level semantic maximum for file or replacement count;
  existing provider payload and process safety limits remain authoritative;
- the tool description tells the model to read target files first, use
  observed exact counts, group replacements by file, use `Write` for creation
  or full replacement, and issue a fresh call after any failure.

The legacy top-level `path`, `old`, `new`, and `replace_all` fields are not
accepted.

## Exact Match Semantics

Each replacement counts non-overlapping exact UTF-8 substring matches in the
current staged content. It succeeds only when:

```text
actual_matches == expected_matches
```

On success every counted match is replaced. On mismatch the whole Edit prepare
fails and no file is written.

Matching never:

- ignores leading, trailing, or internal whitespace;
- normalizes LF and CRLF;
- changes or manufactures a final newline;
- normalizes Unicode punctuation or spaces;
- chooses an occurrence by line hint;
- applies a closest match.

Diagnostics may report actual match counts and line numbers. Diagnostics are
read-only evidence and never loosen the matching rule.

## Ordered Staging

Replacements in one file apply in declaration order:

```text
original content
      |
      | replacement[0]
      v
staged content 1
      |
      | replacement[1]
      v
staged content 2
      |
      | ...
      v
final staged content
```

Every step validates against the previous step's staged result. A file is
written at most once, after all replacements in the entire Edit call have
prepared successfully. Per-file and aggregate statistics compare original
content with final staged content; they are not the sum of intermediate diffs.

## Prepared Execution Architecture

The task does not generalize the `Tool` trait into a speculative two-phase
interface. `PreparedToolCall` gains a narrow runtime-only payload:

```rust
enum PreparedExecution {
    Direct,
    Edit(Arc<PreparedEdit>),
}
```

`PreparedEdit` contains the resolved targets, original fingerprints, original
and staged contents, replacement counts, unified diffs, and aggregate
statistics needed for approval and commit. It is never serialized, persisted,
or sent to a provider.

The runtime phases become:

```text
1. parse all tool arguments
2. instruction preflight over every typed target directory
3. prepare Edit calls without side effects
4. authorize the full batch
5. recheck instruction fingerprint
6. recheck prepared Edit targets
7. schedule and execute authorized calls
```

Non-Edit tools use `PreparedExecution::Direct` and retain their current path.
Edit execution consumes the exact prepared payload that produced the approval
projection. Edit does not open or resolve its own approval dialog.

## Prepare Phase

For every declared file, prepare must:

1. resolve the effective write target through `WorkspaceAccessPolicy`;
2. reject duplicate resolved targets;
3. verify the target is an existing UTF-8 regular file and not a symlink or
   Windows reparse point;
4. read the complete current contents;
5. record a fingerprint containing the resolved target identity/path, file
   type, and SHA-256 of the bytes;
6. run ordered staged replacements with exact `expected_matches` checks;
7. reject no-op files and no-op batches;
8. compute one original-to-final unified diff per file;
9. compute aggregate file, replacement, added-line, and removed-line counts;
10. construct the runtime-only `PreparedEdit` and the bounded approval
    projection.

Any failure produces `status: prepare_failed`, identifies the file and
replacement index when applicable, and guarantees zero writes.

## Approval Contract

`ApprovalPresentation` gains a typed Edit variant that carries only display
projection data, not complete original or staged file contents:

```text
Edit title
file count
replacement count
aggregate added/removed lines
ordered per-file path/stat/diff projections
```

Ask mode presents the verified planned diff. Auto and Yolo modes still run the
same prepare and stale checks, but do not open a dialog.

Session approval derives one existing `SessionApprovalKey::FileWrite` with
`operation: Edit` for every prepared workspace-contained target. The option is
offered only when all targets can participate in a narrow session scope. The
label is:

```text
Approve edits to these N files for this session
```

A cached scope matches only when its complete key set is approved. No generic
"approve Edit everywhere" wildcard is introduced.

## Stale Rechecks

After authorization and instruction-fingerprint recheck, Neo re-resolves and
re-fingerprints every target before the first write. Any mismatch returns
`status: stale` with zero writes.

Immediately before committing each file, Neo performs a just-in-time recheck.
This narrows the race between global recheck and later files. If this later
check fails, already committed files remain committed, the current file is
failed as stale, later files are not attempted, and the tool returns a partial
commit error.

Normal local filesystems do not provide cross-file compare-and-swap. The
design therefore does not claim that an external process cannot change a file
in the final interval between the last recheck and atomic rename.

## Commit Contract

Files commit in `files[]` order. Each file uses the existing atomic write
semantics: write and sync a temporary sibling, atomically replace the target,
then sync the parent directory where supported.

`AtomicWriteStatus` maps as follows:

| Atomic result | Edit state |
|---|---|
| `Durable` | File is `committed`; continue. |
| `CommittedUnsynced(error)` | File is `committed_unsynced`; stop and return an error. |
| Error before replacement | File is `failed`; stop and return an error. |

The existing atomic helper's session-specific error wording should be made
generic if it becomes user-visible for workspace files; the atomic algorithm
must be reused rather than copied.

No automatic rollback is attempted. Rollback could fail and could overwrite a
concurrent user or agent edit made after approval.

## Structured Tool Results

Success details use this stable shape:

```json
{
  "kind": "edit",
  "status": "committed",
  "files": 3,
  "replacements": 7,
  "added": 18,
  "removed": 11,
  "changes": [
    {
      "path": "src/model.rs",
      "status": "committed",
      "replacements": 3,
      "added": 5,
      "removed": 3,
      "diff": "..."
    }
  ]
}
```

Top-level failure statuses are:

```text
prepare_failed
stale
cancelled
commit_failed
partial_commit
durability_uncertain
```

Per-file statuses are:

```text
committed
committed_unsynced
failed
not_attempted
```

Errors include the exact path, file index, replacement index, expected/actual
match count, stale reason, or I/O cause that made the call fail. Provider text
must tell the model whether writes were zero, partial, or complete-but-unsynced
and must not recommend blindly replaying the same Edit call.

`cancelled` means cancellation was observed before the first file commit and
therefore guarantees zero writes. Cancellation after one or more commits uses
`partial_commit` plus `cause: "cancelled"`. `commit_failed` means the first
atomic replacement failed before any durable write. Both statuses include the
same ordered per-file `changes` projection used by the other terminal states.

## Progress Events

No new `AgentEvent` variant is introduced. Immediately after
`ToolExecutionStarted` and before the first commit, runtime emits one existing
`ToolExecutionUpdate` whose details carry the verified planned projection:

```json
{
  "kind": "edit_prepared",
  "files": 5,
  "replacements": 9,
  "added": 28,
  "removed": 17,
  "changes": [
    {
      "path": "src/model.rs",
      "replacements": 3,
      "added": 5,
      "removed": 3,
      "diff": "..."
    }
  ]
}
```

This update makes the running tool card use the same verified projection that
was approved; it must not recompute a diff from raw arguments.

After each durable file commit, runtime emits another existing
`ToolExecutionUpdate` with structured progress details:

```json
{
  "kind": "edit_progress",
  "committed": 2,
  "total": 5,
  "latest_path": "src/lib.rs",
  "added": 9,
  "removed": 4
}
```

Commit updates occur only at file boundaries, never per replacement. The TUI
must retain `partial_result.details` for Edit updates instead of discarding
them as generic live text. These updates support live progress, child
summaries, and bounded replay evidence without a new event or write-ahead
journal.

## Instruction And Plan-Mode Integration

Instruction scope discovery must return every workspace-contained parent
directory represented by `files[].path`. `BatchProbes.per_call` therefore
becomes a per-call collection rather than one optional directory. The batch
still defers or blocks as one unit when a new or blocked scope is encountered.

In Plan mode, every Edit target must resolve to the active plan file. If any
target differs, the whole Edit call is denied. `Write` keeps its existing
single-path behavior.

Edit remains a `FileWrite` permission operation, remains `Exclusive`, and
retains existing child-role allow/deny policy.

## Top-Level TUI Presentation

The stateful owner remains `ToolCallComponent`. A focused pure renderer,
`transcript/edit_tool_presentation.rs`, owns structured extraction, batch
summary, diff rendering, head/tail selection, failure rows, and width-safe
wrapping. `tool_renderers.rs` only routes Edit states to the helper.

### Shared Edit/Write file frame

Every Edit file projection is enclosed in its own full-width rounded code
frame. Write uses the same frame for its single-file preview so both mutation
tools share one visual grammar:

```text
╭────────────────────────────────────────────────────╮
│ ✓ src/model.rs                            +2 -2    │
│ 12 - pub struct OldName {                           │
│ 12 + pub struct NewName {                           │
│ ... 6 changed lines hidden · ctrl+o to expand      │
╰────────────────────────────────────────────────────╯
```

The status/path/stats row, every numbered diff row, diagnostics, and the
collapsed omission row remain inside the frame. The omission row is
left-aligned with the body; no detached right-side border is allowed. Global
`Ctrl+O` is the only expansion owner: expanding reveals the complete clustered
diff and moves the bottom border down, while collapsing restores the bounded
frame. Each frame is independently height-derived from its visible rows.

Diff line numbers are always present when source line numbers are known. Code
tokens retain the existing Markdown-code-block syntax highlighting; removal
and addition prefixes/line-number gutters use the existing red and green diff
styles. A successful `✓` and `+N` use success/addition green, while `-N` uses
removal red. Color is supplementary to the textual markers. Paths, code,
diagnostics, and omission text hard-wrap inside the frame without truncation.

Typed Edit approval renders the runtime-supplied verified projection through
this same framed diff projection. It participates in global `Ctrl+O`; approval
must never reconstruct or guess a diff from raw Edit arguments.

### Streaming arguments

Incomplete JSON does not produce a guessed diff:

```text
● Preparing Edit
  receiving structured changes... ▌
```

Once file and replacement counts are safely extractable but not prepared, `?`
marks unverified intent and no added/removed statistics appear:

```text
● Preparing Edit · 3 files · 7 replacements
  ? crates/neo-agent-core/src/tools/edit.rs       3 replacements
  ? crates/neo-tui/src/transcript/tool_renderers.rs
                                                    2 replacements
  ? crates/neo-agent-core/tests/tool_files.rs     2 replacements
```

### Verified approval

The canonical approval entry is inserted after the Edit card:

```text
● Preparing Edit · verified · 3 files · 7 replacements · +18 -11
  M crates/neo-agent-core/src/tools/edit.rs              +8 -5
  M crates/neo-tui/src/transcript/tool_renderers.rs      +6 -4
  M crates/neo-agent-core/tests/tool_files.rs            +4 -2

────────────────────────────────────────────────────────────────────
▶ Edit 3 files?
  verified against current workspace · 7 replacements · +18 -11

  M crates/neo-agent-core/src/tools/edit.rs              +8 -5
    14 │- struct EditInput {
    14 │+ struct EditBatchInput {
       │  ...
    27 │-     replace_all: bool,
    27 │+     expected_matches: usize,

  ▶ 1. Approve once
    2. Approve edits to these 3 files for this session
    3. Reject

  ctrl+o expand · ↑/↓ select · number keys choose · ↵ confirm
────────────────────────────────────────────────────────────────────
```

The runtime supplies the title, projection, options, labels, payloads, and
ordering. TUI consumers do not reconstruct them.

### Collapsed running preview

```text
● Using Edit · 5 files · 9 replacements · +28 -17
  M src/model.rs                                      +5 -3
    12 │- pub struct OldName {
    12 │+ pub struct NewName {

  M src/lib.rs                                        +4 -2
     4 │- pub use model::OldName;
     4 │+ pub use model::NewName;

  ... 2 files · 4 replacements · 11 changed lines hidden
      ctrl+o to expand

  M tests/model.rs                                    +9 -6
    88 │- let value = OldName::new();
    88 │+ let value = NewName::new();
```

Collapsed selection uses final visual rows and preserves the aggregate
summary, the first file and first change cluster, another leading file when it
fits, an explicit omission row, and the final file and final change cluster.
No omitted content is silent.

Global `Ctrl+O` renders every file and complete clustered diff. No Edit-only
expansion state is added. Replay and resize regenerate styled rows from stored
structured data; styled spans are not persisted.

### Queue, commit, and success

```text
● Queued Edit · #2 · waiting 4s
  3 files · 7 replacements · +18 -11
  src/tools/edit.rs … tests/tool_files.rs

● Using Edit · committing 2/5 files
  ✓ src/model.rs
  ✓ src/lib.rs
  ● src/runtime.rs
  · tests/runtime.rs
  · tests/model.rs

● Used Edit · 3 files · 7 replacements · +18 -11
  ✓ crates/neo-agent-core/src/tools/edit.rs
  ✓ crates/neo-tui/src/transcript/tool_renderers.rs
  ✓ crates/neo-agent-core/tests/tool_files.rs

  M crates/neo-agent-core/src/tools/edit.rs              +8 -5
    14 │- struct EditInput {
    14 │+ struct EditBatchInput {
       │  ... 6 changed lines hidden · ctrl+o to expand

  M crates/neo-agent-core/tests/tool_files.rs            +4 -2
    42 │- json!({ "path": path, "old": old, "new": new })
    42 │+ json!({ "files": [{ "path": path, ... }] })
```

On completion, committed result details replace the planned projection. On an
ordinary success the planned and committed diffs must be byte-identical; a
mismatch is an implementation defect.

### Width behavior

Paths and code hard-wrap within the terminal width. They are never silently
truncated or allowed to cross the width invariant:

```text
● Used Edit · 2 files
  3 replacements · +7 -4

  M crates/neo-agent-core/src/
    tools/edit.rs
    +5 -3
    14 │- struct EditInput {
    14 │+ struct EditBatchInput {
```

Color enhances but does not carry meaning: removal, addition, path/stat, and
status styles reuse the existing diff and tool palettes; `?`, `M`, `✓`, `!`,
`✗`, `·`, `+`, and `-` preserve meaning without color.

## Failure And Cancellation Presentation

### Prepare failure

```text
✗ Failed Edit · prepare · zero writes
  src/model.rs · replacement 2/3
  expected 4 exact matches · found 5
  matches at lines 18, 42, 77, 103, 128
  Re-read the file and submit a new Edit call.
```

### Approval denial

```text
✗ Failed Edit · approval denied · zero writes
  3 files · 7 replacements were not applied

approval: Rejected
```

### Stale before writes

```text
✗ Failed Edit · stale · zero writes
  src/model.rs changed after approval
  planned content no longer matches the current workspace
  Re-read affected files and submit a new Edit call.
```

### Partial commit

```text
✗ Failed Edit · partial commit · 2/5 files · +9 -4
  ✓ src/model.rs                 committed · +5 -3
  ✓ src/lib.rs                   committed · +4 -1
  ✗ src/runtime.rs               atomic replace failed
  · tests/runtime.rs             not attempted
  · tests/model.rs               not attempted

  Files already committed were not rolled back.
  permission denied while replacing src/runtime.rs
```

Expanded partial cards render only real committed diffs. Not-attempted planned
changes do not use added/removed styling that could imply they happened.

### Durability uncertain

```text
✗ Failed Edit · durability uncertain · 3/3 files committed
  ! src/model.rs                 committed · sync failed
  ✓ src/lib.rs                   committed
  ✓ tests/model.rs               committed

  All requested contents were installed.
  Durability could not be confirmed before returning.
```

### Cancellation

Before commit:

```text
⊘ Cancelled Edit · prepare · zero writes
```

During commit, cancellation prevents the next file from starting but never
interrupts an atomic replacement in the middle:

```text
⊘ Cancelled Edit · partial commit · 2/5 files
  ✓ src/model.rs                 committed
  ✓ src/lib.rs                   committed
  · src/runtime.rs               not attempted
  · tests/runtime.rs             not attempted
  · tests/model.rs               not attempted
```

## Delegate-Family Projection

Delegate, DelegateGroup, and DelegateSwarm keep their current card structure,
row budgets, ordering, progress, output previews, and expansion semantics.
They never embed an Edit card or diff and never persist complete child Edit
arguments in parent progress state.

Within the existing bounded summary:

```text
  • Using Edit (5 files · 9 replacements · src/model.rs … tests/model.rs)
  • Used Edit (5 files · 9 replacements · +28 -17)
  • Failed Edit (prepare · zero writes · src/model.rs: expected 4, found 5)
  • Failed Edit (partial · 2/5 committed · src/runtime.rs)
```

Swarm retains its existing two-line projection:

```text
nova · running · waiting on Edit
  5 files · 9 replacements · src/model.rs … tests/model.rs

nova · failed · Edit partial commit
  2/5 committed · failed at src/runtime.rs
```

Before preparation, summaries derive bounded counts and head/tail paths from
arguments. After progress or completion, structured details become
authoritative. Missing structured fields are omitted rather than reconstructed
from human-readable result text.

## Replay And Crash Contract

- Raw tool arguments reconstruct only proposed/Preparing presentation.
- `ApprovalRequested` reconstructs the planned approval projection.
- `ToolExecutionUpdate` reconstructs the last recorded per-file progress.
- `ToolResult.details` reconstructs committed, stale, partial, and durability
  terminal cards.
- Runtime-only `PreparedEdit` is never persisted and is never resumed.

A restarted process never continues an unfinished Edit. If execution ended
without final details, replay renders:

```text
✗ Interrupted Edit · final commit state unknown
  Last recorded progress: 2/5 files committed
  Inspect the workspace before retrying.
```

Per-file updates reduce but cannot eliminate the interval between atomic
replacement and event persistence. This design does not add a write-ahead
journal or infer success from current disk contents.

## Documentation

Update both languages:

- `docs/en/reference/tools.md`
- `docs/zh/reference/tools.md`
- `docs/en/customization/agents.md`
- `docs/zh/customization/agents.md`

Tool reference documentation must show the new `files[]` schema, exact staged
semantics, `expected_matches`, zero-write prepare failures, partial commit
truthfulness, `Write` ownership, and the absence of legacy schema support.
Instruction documentation must state that Edit probes every target file's
parent directory and defers the whole call when any scope changes.

## Ownership And File Boundaries

Expected implementation owners:

- `crates/neo-agent-core/src/tools/edit.rs`: input types, preparation,
  fingerprinting, staging, diff, recheck, commit, and result details;
- `crates/neo-agent-core/src/runtime/tool_arguments.rs`: prepared execution
  transport and multi-target instruction probes;
- `crates/neo-agent-core/src/runtime/tool_dispatch.rs`: prepare phase,
  authorization transport, rechecks, and prepared commit dispatch;
- `crates/neo-agent-core/src/runtime/permission.rs`: typed approval projection,
  multi-key session scope, and plan-mode handling;
- `crates/neo-agent-core/src/approval.rs`: serializable Edit presentation data;
- `crates/neo-agent-core/src/mode/plan_mode_guard.rs`: every-target plan guard;
- `crates/neo-agent-core/src/session/atomic_file.rs`: reused atomic writer with
  generic error wording if needed;
- `crates/neo-agent-core/src/multi_agent/runtime.rs`: bounded Edit summaries;
- `crates/neo-tui/src/transcript/edit_tool_presentation.rs`: pure Edit display;
- `crates/neo-tui/src/transcript/tool_renderers.rs`: routing only;
- `crates/neo-tui/src/transcript/entry/mod.rs`: typed approval rendering route;
- `crates/neo-tui/src/diff_model.rs`: multi-file structured result parsing.

The generic `Tool` trait, `ToolResult`, `AgentEvent`, and Delegate card modules
are not new owners and should not gain speculative Edit-specific state.

## Verification Strategy

Tests must be high-signal and non-redundant:

1. one core contract test for ordered staged replacements across multiple
   files, expected counts, one write per file, and aggregate details;
2. one prepare-failure test proving a later mismatch leaves every file intact;
3. one injected commit-failure test proving committed/failed/not-attempted
   details and no rollback;
4. one runtime approval test proving verified diff projection and multi-key
   session scope;
5. one stale-during-approval test proving zero writes;
6. one instruction test proving all file-parent probes participate and defer
   the whole batch;
7. one Plan-mode test proving any non-plan target rejects the whole Edit call;
8. one TUI table test covering wide collapsed, full expansion, narrow wrapping,
   and line-width invariants;
9. one TUI failure test covering prepare, stale, partial, and durability
   distinctions;
10. one Delegate-summary test covering running, success, and partial failure
    within the existing character budget;
11. one replay test proving final details reconstruct and incomplete progress
    never resumes execution.

Verification evidence must use one package, one target selector, and at least
one precise test filter per command. Workspace-wide test runs are not required
for this task.

## Acceptance Criteria

1. One Edit call can modify multiple existing UTF-8 files.
2. The old single-edit schema is rejected and no compatibility path remains.
3. `expected_matches` mismatch anywhere produces zero writes.
4. Replacements in one file apply in declaration order.
5. Ask mode displays a verified planned diff before approval.
6. Stale targets are not silently overwritten.
7. Every committed file uses the existing atomic-write semantics.
8. Partial and durability failures are provider-visible errors.
9. Every result distinguishes committed, failed, and not-attempted targets.
10. Collapsed display never silently omits files or changes; `Ctrl+O` shows the
    complete diff.
11. No rendered row exceeds terminal width.
12. Delegate-family layout and expansion behavior remain unchanged.
13. Replay never depends on persisted `PreparedEdit` and never resumes commit.
14. Plan-mode and path-scoped instruction behavior covers every target path.
15. Write remains the only file creation and full-overwrite owner.
16. English and Chinese docs describe the same canonical behavior.

## Non-Goals

- a new `apply_patch` tool;
- Codex freeform patch or unified-diff input parsing;
- create, delete, or move actions in Edit;
- fuzzy whitespace, Unicode, closest-match, occurrence, or line-hint matching;
- `replace_all` or an unbounded match mode;
- a cross-file transaction guarantee or automatic rollback;
- a write-ahead Edit journal or crash-time commit recovery;
- per-replacement progress events;
- a new global Partial status or Edit-only card state machine;
- nested Edit cards or full diffs inside Delegate-family cards;
- a legacy schema decoder, alias, fallback, or dual renderer;
- arbitrary product-level limits based on predicted task scale or cost.

## Anti-Entropy Decision

```text
Deletion Class: contract-carrying internal code
Old Path: single-file EditInput and every path/old/new/replace_all consumer
New Canonical Owner: files[] plus ordered replacements[] prepared execution
Expected Preserved Behavior: exact existing-file replacement and unified diff
Expected Retired Behavior: single-file wire schema and replace_all
External Boundary Touched: no proven active dependency
Source-of-Truth Data Risk: none
User Confirmation Required: no
Retirement Path: delete-first
```

Lingering-reference verification must search source, tests, docs, tool
descriptions, argument probes, plan guards, approval rendering, and child
summaries for old Edit fields after migration.

## Architecture Signal

Architecture review is required because this changes a durable tool contract,
runtime prepared-execution transport, approval presentation, instruction path
discovery, and transcript projection. The ADR signal is `yes`; completion
should evaluate whether to create or amend an ADR or synchronize an existing
architecture baseline after implementation evidence exists.

## Planning Notes

- TDD Route is not strict unless separately authorized. Use proportional
  post-change regressions for each touched boundary.
- Keep the implementation in one worktree and preserve unrelated dirty files.
- Do not redesign the already-approved canonical approval protocol,
  path-scoped instruction system, generic tool cards, or Delegate-family cards.
- If implementation evidence contradicts this spec, stop and return to design;
  do not introduce a fallback or compatibility branch to make progress.
