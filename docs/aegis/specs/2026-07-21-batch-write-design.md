# Neo Batch Write Prepared Execution Design

Status: Approved
Date: 2026-07-21

## Goal

Upgrade Neo's canonical `Write` tool from one direct single-file write to one
ordered batch of complete UTF-8 file contents. One call may create new files and
fully overwrite existing files together. The whole batch must be validated and
inspectable before approval, each file must be installed atomically, stale
targets must not be silently overwritten, and partial commits must be reported
truthfully.

The design deliberately follows the proven Batch Edit prepared-execution path.
It does not introduce a second write tool, a generic two-phase `Tool` trait, or
a cross-file transaction system.

## Approved Product Decisions

1. `Write` remains the only owner for file creation and complete-content
   replacement. No `BatchWrite` tool is added.
2. The canonical input becomes `files[]`; a single-file call uses a one-element
   array.
3. Legacy top-level `path` and `content` are removed outright. There is no
   compatibility decoder, alias, fallback, or dual-schema renderer.
4. One batch may mix new-file creation and existing-file overwrite.
5. Existing targets must be UTF-8 regular files. Directories, symlinks, Windows
   reparse points, and non-UTF-8 files are rejected before approval.
6. An overwrite whose final bytes equal the current bytes is a no-op and fails
   the whole prepare phase with zero file installations. Creating an empty new
   file remains valid.
7. Missing parent directories are allowed. Prepare is side-effect-free; parent
   directories are created only during authorized commit.
8. All paths, types, contents, operations, fingerprints, diffs, and statistics
   are prepared before approval. Any prepare error installs zero files.
9. Ask mode approves the verified prepared projection, not a preview inferred
   from raw tool arguments.
10. Approval is followed by whole-batch and just-in-time path/content rechecks.
    A new target that appears after approval is stale and is never overwritten.
11. Files commit in declaration order. Each file installation is atomic, but
    the batch is not a cross-file transaction.
12. A commit failure never triggers automatic rollback. Already installed
    files remain installed and are reported exactly.
13. A partial commit is a failed tool result, not a warning-bearing success and
    not a new ambiguous global tool state.
14. Created files render as line-numbered syntax-highlighted final content.
    Overwritten files render as real unified diffs.
15. Every Write and Edit file owns the same content-width-adaptive titled code
    frame: its semantic file header is embedded in the top border, never emitted
    as a separate first body row. `Ctrl+O` expands all files and hidden content.
16. Successful Edit and Write headers are batch summaries and never include a
    first-path projection.
17. Delegate, DelegateGroup, and DelegateSwarm retain their current card layout,
    ordering, row budgets, output previews, and expansion behavior. Only their
    bounded Write activity summary changes.
18. There is no product-level semantic maximum for file count or content size.
    Existing provider payload and process safety limits remain authoritative.

## Baseline And Authority

This design is constrained by:

- `AGENTS.md`, especially canonical-owner deletion, cross-platform behavior,
  scoped verification, and exact Delegate-family preservation;
- `docs/aegis/specs/2026-07-20-batch-edit-design.md`, which is the proven
  prepared execution, stale checking, partial commit, approval, and TUI model;
- `docs/aegis/specs/2026-07-17-canonical-approval-protocol-design.md`, which
  keeps the runtime as the sole approval owner;
- `docs/aegis/specs/2026-07-17-path-scoped-agents-instructions-design.md`, which
  requires typed target paths to participate in instruction preflight;
- the current `WriteTool`, `PreparedExecution`, permission pipeline,
  `ToolCallComponent`, diff/code-frame renderer, and child activity projection.

Current implementation facts that require the change:

- `WriteInput` is currently `{ path, content }`;
- `WriteTool::execute` reads one path, creates parents, and calls
  `tokio::fs::write` directly;
- current Write approval is a generic `ApprovalPresentation::Tool` built from
  raw arguments;
- instruction and plan-mode guards extract only one top-level Write path;
- current Write TUI parses one raw `path`/`content` pair and owns no structured
  terminal state model;
- `PreparedExecution` already carries runtime-only `PreparedEdit`, making a
  narrow `PreparedWrite` variant the smallest established extension;
- shared atomic helpers already distinguish durable installation from
  committed-but-unsynced results and support create-new versus replace-existing
  files.

## Task Intent

- Outcome: one model-issued Write call can create and overwrite a coherent set
  of UTF-8 files while showing the real batch before a human approves it.
- Success evidence: schema, prepare, path safety, permission, instruction
  preflight, plan guard, atomic commit, progress, TUI, approval, replay,
  Delegate summary, retirement scans, and paired documentation checks pass.
- Stop condition: `files[]` and prepared execution are the only active Write
  behavior; all visible states match this design; old single-file paths are
  deleted.
- Non-goals: file deletion or movement, binary content, targeted replacement,
  freeform patch parsing, cross-file rollback, persistent journals, restart
  resume, generalized prepared-tool infrastructure, or Delegate-card redesign.

## Canonical Ownership

```text
Read    -> read files
Write   -> create files or replace complete UTF-8 file contents
Edit    -> exact replacements in existing UTF-8 regular files
```

`write.rs` remains the only owner of Write validation, operation
classification, fingerprints, staging, diff/stat calculation, stale rechecks,
parent creation bookkeeping, commit order, and terminal result construction.
The runtime owns authorization and event sequencing. `ToolCallComponent` remains
the stateful transcript owner. Write-specific TUI code is a pure presentation
module.

## Tool Input Contract

The only model-visible schema is:

```json
{
  "files": [
    {
      "path": "src/a.rs",
      "content": "fn main() {}\n"
    },
    {
      "path": "src/generated/empty.txt",
      "content": ""
    }
  ]
}
```

Required constraints:

| Field | Contract |
|---|---|
| `files` | Non-empty array; declaration order is meaningful. |
| `files[].path` | Non-empty path resolved through the existing workspace policy. |
| `files[].content` | Complete UTF-8 text content; empty is valid. |
| Unknown fields | Rejected at every object level. |

Additional validation:

- two declared paths that resolve to the same effective target are rejected;
- an existing target with identical bytes is rejected as a no-op;
- new and existing targets may appear in any declaration order;
- path separators and resolution use `Path`/`PathBuf`; no Unix-only path logic
  is introduced;
- provider truncation repair may only recover the complete required top-level
  `files` value under the existing guarded parser contract; it never converts
  legacy `path`/`content` into the new schema;
- the tool description tells the model to use `Read` before overwriting,
  provide complete contents, group coherent writes, use `Edit` for targeted
  changes, and submit a fresh call after any stale or partial failure.

## Operation Classification

Every prepared target is classified exactly once:

```text
target absent  -> created
target exists  -> overwritten
```

For `overwritten`, prepare reads the existing bytes without following the
target, requires an ordinary UTF-8 regular file, rejects identical final bytes,
and computes an original-to-final unified diff.

For `created`, prepare requires the target to be absent, validates the nearest
existing ancestor through the workspace policy, treats the original content as
empty for diff statistics, and retains the complete final content for approval
and syntax-highlighted transcript presentation.

The operation is immutable after approval. An absent target becoming present
is stale; it is never silently reclassified as `overwritten`.

## Prepared Execution Architecture

The task does not generalize the `Tool` trait. The existing runtime-only enum
gains one narrow variant:

```rust
enum PreparedExecution {
    Direct,
    Edit(Arc<PreparedEdit>),
    Write(Arc<PreparedWrite>),
}
```

`PreparedWrite` contains the ordered files, requested and resolved targets,
immutable operation, existing/absent fingerprints, final UTF-8 content,
line counts, diffs or created-content projections, aggregate statistics, and
data required for approval/session scope/commit. It is never serialized,
persisted, or sent to a provider.

The runtime phases are:

```text
1. parse every tool call
2. instruction preflight over every typed Write target directory
3. prepare canonical Write calls without side effects
4. authorize the verified complete batch
5. recheck instruction fingerprint
6. recheck every prepared Write target
7. schedule the authorized Write call as Exclusive
8. emit prepared update, then commit files in order with progress updates
9. emit one truthful terminal ToolResult
```

Non-Write and non-Edit tools remain `PreparedExecution::Direct`.

## Prepare Phase

For every declared file, prepare must:

1. resolve the effective target through `WorkspaceAccessPolicy`;
2. reject duplicate effective targets;
3. inspect the model-supplied target without following a symlink or reparse
   point;
4. classify it as absent or an existing regular file;
5. for an existing file, read bytes without following the target, require
   UTF-8, reject a no-op, and record SHA-256 plus resolved path/type;
6. for an absent file, record the resolved target and the nearest safe existing
   ancestor needed to repeat resolution;
7. reject directories and other non-regular existing objects;
8. compute final line count and original-to-final added/removed statistics;
9. retain complete created content or overwrite diff for the verified display
   projection;
10. construct the runtime-only `PreparedWrite` and bounded approval data.

Any failure returns `kind: "write"`, `status: "prepare_failed"`, identifies the
file index and path when available, and guarantees that no file was installed
and no parent directory was created.

## Fingerprints And Stale Rechecks

An existing target fingerprint contains:

- the resolved target path;
- regular-file kind;
- SHA-256 of the original bytes.

An absent target fingerprint contains:

- the resolved target path;
- the fact that the target did not exist;
- the canonical nearest existing ancestor used by workspace resolution.

After authorization and instruction recheck, Neo re-resolves and rechecks every
target before the first file installation. Immediately before each file, it
performs the same just-in-time recheck.

Stale conditions include:

- an overwritten file's bytes, type, or resolved target changed;
- a created target appeared;
- a relevant path component became a symlink or reparse point;
- repeated workspace resolution produces a different effective target;
- an expected existing ancestor is no longer a safe directory.

Normal cross-platform filesystems do not expose a portable directory
compare-and-swap or eliminate the final interval between recheck and atomic
installation. The implementation must narrow this interval and never claim a
stronger guarantee.

## Parent Directory Contract

Prepare never creates directories. During authorized commit of a created file,
Neo creates missing parents from the nearest existing ancestor outward, checks
every component for link-like/reparse behavior, and records each directory for
which Neo's own creation operation succeeded.

If directory creation or the later file installation fails:

- already created directories are not automatically removed, because another
  process may have placed content inside them;
- the structured result lists `created_directories` exactly as observed by the
  commit path;
- user/provider text distinguishes `zero file installs` from remaining created
  directories;
- a failure while processing a later file may combine prior committed files and
  newly created directories under `partial_commit`.

This bookkeeping may be implemented by a narrow helper in the existing atomic
file module. It is safety behavior, not a new generic transaction abstraction.

## Approval Contract

`ApprovalPresentation` gains a typed Write variant. The projection represents
the prepared state and uses an explicit created-versus-overwritten preview
variant rather than ambiguous optional fields:

```text
Write title
aggregate file/created/overwritten/+/- counts
ordered per-file path, operation, line count, stats
created -> complete final content
overwritten -> unified diff
```

Ask mode displays this projection. Auto and Yolo still run prepare and stale
checks but do not open the dialog.

Session approval derives one `SessionApprovalKey::FileWrite` with operation
`Write` for every workspace-contained prepared target. The option is offered
only when every target can participate in the narrow scope. Its label is:

```text
Approve writes to these N files for this session
```

The exact complete key set must be approved; no workspace-wide Write wildcard
is introduced.

Approval remains an inline transcript entry. While it is active:

- the composer is hidden;
- digits and arrow keys choose an option and Enter confirms;
- mouse wheel input scrolls transcript history and never moves the selection;
- `Ctrl+O` expands or collapses the verified projection;
- no second chrome/dialog renderer presents the same approval.

## Plan Mode And Instruction Preflight

Instruction scope probes collect the parent directory of every `files[].path`.
No shell-string parsing or first-path-only fallback is allowed.

Plan mode may authorize Write without a general prompt only when the prepared
batch contains exactly one target and that resolved target is the active plan
file. Any additional target or different path is denied by the plan guard.

## Commit Contract

Files commit in declaration order and stop at the first stale, cancellation,
I/O, or durability boundary.

- `created` uses the shared atomic create-new helper and must never overwrite a
  target that appeared after approval.
- `overwritten` uses the shared strict existing-file replacement helper,
  preserving existing permissions where supported.
- both variants sync the temporary file and parent directory according to the
  existing cross-platform atomic helper contract.

Atomic results map to Write state as follows:

| Atomic result | Write state |
|---|---|
| `Durable` | File `status: committed`; continue. |
| `CommittedUnsynced(error)` | File `status: committed_unsynced`; stop with installed content. |
| Error before installation | File `status: failed`; stop. |

No automatic rollback is attempted. Rollback could fail or overwrite a human,
agent, or process change made after approval.

## Cancellation Contract

- cancellation observed before the first file commit returns `cancelled` and
  installs zero files;
- cancellation is checked before each file and does not interrupt an atomic
  installation already in progress;
- cancellation after one or more installed files returns `partial_commit` with
  `cause: "cancelled"`;
- unfinished prepared Write execution is not resumed after process restart.

## Structured Tool Results

Success details have this stable shape:

```json
{
  "kind": "write",
  "status": "committed",
  "files": 3,
  "created": 2,
  "overwritten": 1,
  "added": 84,
  "removed": 17,
  "changes": [
    {
      "path": "src/a.rs",
      "operation": "created",
      "status": "committed",
      "line_count": 42,
      "added": 42,
      "removed": 0,
      "content": "..."
    },
    {
      "path": "src/config.rs",
      "operation": "overwritten",
      "status": "committed",
      "line_count": 35,
      "added": 2,
      "removed": 2,
      "diff": "..."
    }
  ],
  "created_directories": ["src/generated"]
}
```

Top-level terminal statuses are:

```text
committed
prepare_failed
stale
cancelled
commit_failed
partial_commit
durability_uncertain
```

Per-file fields are deliberately separate:

```text
operation: created | overwritten
status: committed | committed_unsynced | failed | not_attempted
```

Failure results include ordered `changes`, the exact failure path/index/cause,
applied aggregate statistics, and `created_directories`. Provider text must say
whether file installations were zero, partial, or installed-but-unsynced and
must not recommend blindly replaying the same Write call.

For any failed or partial terminal header, `+N/-N` counts only installed files,
never the planned totals.

## Progress Events

No new `AgentEvent` variant is introduced. Existing `ToolExecutionUpdate`
carries two structured detail kinds.

Prepared projection before the first commit:

```json
{
  "kind": "write_prepared",
  "files": 3,
  "created": 2,
  "overwritten": 1,
  "added": 84,
  "removed": 17,
  "changes": []
}
```

Progress after each file boundary:

```json
{
  "kind": "write_progress",
  "committed": 2,
  "total": 3,
  "latest_path": "src/config.rs",
  "latest_operation": "overwritten",
  "added": 44,
  "removed": 2
}
```

The real emitted prepared projection includes the ordered display data needed
by the transcript. The abbreviated example above only highlights aggregate
fields.

## TUI Ownership And Rendering

`write_tool_presentation.rs` becomes the pure Write presentation owner. It
consumes status, arguments, structured details, expansion state, width, and
theme. It reuses existing diff parsing, syntax highlighting, wrapping, and
content-adaptive code frames.

`ToolCallComponent` remains the only stateful card owner. Generic
`tool_renderers.rs` only routes Write state and renders the semantic header.
The old single-file `parse_write_arguments`/`render_write_preview` path is
deleted after the new renderer owns streaming, prepared, and terminal states.

The shared `render_code_frame` contract changes once for both Edit and Write:
the supplied semantic header is rendered inside the top border. Callers no
longer prepend that header to the body. This shared chrome owner prevents Edit
and Write from drifting into different frame layouts.

### Header Contract

Successful headers never show a path:

```text
● Used Write · 3 files · 2 created · 1 overwritten · +84 -17
● Used Edit · 3 files · 7 replacements · +18 -8
```

The batch summary appears only in the header. `+N` uses `theme.diff_added` and
`-N` uses `theme.diff_removed`; separators and counts use muted metadata color.
The Edit first-path chip is removed for both single- and multi-file calls.

### Shared Edit And Write File Frame Contract

The old Edit shape is retired:

```text
╭──────────────────────────────────────────────────╮
│ ✓ src/model.rs  +8 -3 · committed               │
│ ... diff body ...                                │
╰──────────────────────────────────────────────────╯
```

Edit uses the same titled top border as Write:

```text
╭─ M src/model.rs · 3 replacements · +8 -3 ───────╮
│ ... verified diff body ...                       │
╰──────────────────────────────────────────────────╯

╭─ ✓ src/model.rs · committed · 3 replacements · +8 -3 ─╮
│ ... committed diff body ...                            │
╰────────────────────────────────────────────────────────╯
```

Prepared Edit uses `M`; committed Edit uses a green `✓`; failed and
not-attempted states use the same red `✗` and muted `·` semantics as Write.
Paths are primary bold text, replacement/status metadata is muted or
status-colored, and `+N/-N` preserves added/removed colors.

The top-border title is always one visual row. At narrow widths the caller fits
the semantic title by eliding the path middle while preserving the marker,
filename/deepest tail, terminal status, and `+N/-N`. It never wraps the title
back into a separate body header. Widths too narrow for a valid frame keep the
existing unframed fallback.

### Approval And Prepared Rendering

```text
● Using Write · 3 files · 2 created · 1 overwritten · +84 -17

╭─ A src/new.rs · create · 42 lines · +42 -0 ─────╮
│   1  use crate::App;                             │
│   2                                              │
│   3  pub fn start() {                            │
│   4      App::run();                             │
│   5  }                                           │
╰──────────────────────────────────────────────────╯

╭─ M src/config.rs · overwrite · +2 -2 ────────────╮
│  18   timeout = 30                               │
│  19 - retries = 2                                │
│  19 + retries = 4                                │
│  20   cache = true                               │
╰──────────────────────────────────────────────────╯

... 1 file · 1 created · +40 -15 hidden · ctrl+o to expand

  Write these 3 files?
▶ 1. Yes
  2. Yes, and approve writes to these 3 files for this session
  3. No
```

`A` is added/created and uses the added color. `M` uses the diff hunk color.
Paths are primary bold text. Added and removed diff lines retain their normal
colors and old/new line numbers.

### Successful Terminal Rendering

```text
● Used Write · 3 files · 2 created · 1 overwritten · +84 -17

╭─ ✓ src/new.rs · created · 42 lines · +42 -0 ─────╮
│   1  use crate::App;                              │
│   2                                               │
│   3  pub fn start() {                             │
│   4      App::run();                              │
│   5  }                                            │
╰───────────────────────────────────────────────────╯

╭─ ✓ src/config.rs · overwritten · +2 -2 ───────────╮
│  18   timeout = 30                                │
│  19 - retries = 2                                 │
│  19 + retries = 4                                 │
│  20   cache = true                                │
╰───────────────────────────────────────────────────╯
```

The check mark is green. Created content retains line numbers and syntax
highlighting. Overwritten content uses the same real-line diff presentation as
Edit.

### Collapse And Width Rules

- three or fewer files render completely at the file-selection level;
- more than three render the first two and last file, with one left-aligned
  unframed omission summary between them;
- a collapsed created file preserves head and tail content with an omission row
  inside its frame;
- a collapsed overwritten file preserves first and last change clusters;
- global `Ctrl+O` reveals every file and all hidden content;
- frames shrink to visible content width and never exceed the viewport;
- frame titles remain in the top border; narrow views elide their paths while
  preserving marker, filename tail, status, and `+N/-N`;
- narrow code/diff bodies wrap without border overflow;
- continuation rows use blank line-number gutters;
- long paths preserve the filename/deepest tail;
- hidden summaries never acquire a leading frame `│` that pushes text to the
  right edge.

### Failure And Partial Rendering

```text
✗ Failed Write · partial commit · 1/3 committed · +42 -0

partial commit · already written files remain

╭─ ✓ src/new.rs · created · committed · +42 -0 ────╮
│   1  use crate::App;                              │
│   ...                                             │
╰───────────────────────────────────────────────────╯

╭─ ✗ src/config.rs · overwrite · failed ────────────╮
│ stale: content changed after approval             │
╰───────────────────────────────────────────────────╯

╭─ · src/routes.rs · create · not attempted ────────╮
╰───────────────────────────────────────────────────╯
```

Committed files show their installed projection. Failed files show exact
diagnostics. Not-attempted files show only their path/operation/status header so
planned content cannot be mistaken for installed content.

If directories remain without a file installation:

```text
✗ Failed Write · commit failed · zero file installs

created directories remain:
  src/generated/
  src/generated/api/

file install failed: permission denied
```

The directory list is structured result data, not guessed prose.

## Streaming Arguments

Before prepare completes, the card may render a bounded `unverified intent`
projection from partial `files[]` arguments. It may show declared paths and
complete content fragments already received, but it must not label a target as
created/overwritten, claim verified diffs, or leak raw JSON.

Once `write_prepared` arrives, it replaces the unverified projection with the
verified operation/diff/content model. There is only one Write renderer across
streaming, prepared, and terminal phases.

## Replay And Interrupted Execution

Prepared execution objects are runtime-only and are never reconstructed from
session data. Structured update/result details are sufficient to replay the
same visible card.

An unfinished replayed `write_prepared`/`write_progress` sequence becomes an
interrupted terminal projection stating that final commit state is unknown. It
never re-submits the Write call or resumes commit.

## Delegate And Swarm Projection

Delegate-family cards preserve their current presentation and only receive
bounded activity strings:

```text
Write · prepared · 3 files · 2 created · 1 overwritten · +84 -17
Write · committing 2/3 · latest src/config.rs
Write · partial commit · 1/3 committed · +42 -0
```

Child cards do not embed full file bodies or diffs. The ordinary transcript
Write card remains the detailed presentation owner. Summary strings remain
bounded by the existing activity limit and preserve meaningful path tails.

## Error Handling

The result must distinguish:

- invalid schema or semantic prepare failure;
- unsafe target/ancestor type;
- non-UTF-8 overwrite target;
- no-op overwrite;
- duplicate effective target;
- stale existing content;
- new target appeared;
- parent directory creation failure;
- atomic file installation failure;
- cancellation before/after prior commits;
- committed-but-unsynced durability uncertainty.

Every error tells the model whether file installations were zero or partial,
lists remaining created directories when relevant, and directs it to re-read
affected paths before a fresh call.

## Cross-Platform Requirements

- use `Path`/`PathBuf`, never hardcoded separators;
- reject symlinks on Unix and reparse points on Windows;
- create and validate parent components through portable filesystem APIs;
- isolate any platform-specific atomic replacement behavior behind existing
  `cfg(unix)`/`cfg(windows)` helpers;
- preserve existing permissions for overwritten files where supported;
- no shell command is used for file operations;
- unsupported durability operations return the established explicit status,
  never `panic!`, `todo!`, or false success.

## Compatibility And Retirement

Deletion class: internal contract-carrying code. Path: delete-first. No active
external compatibility boundary or persistent-state deletion is involved.

The implementation must delete in the same workstream:

- top-level Write `path`/`content` schema and all fixtures;
- raw-argument single-path permission/session-scope derivation;
- first-path-only instruction and plan-mode handling;
- generic Write approval details;
- old single-file `parse_write_arguments` and `render_write_preview` owners;
- the old Edit/Write empty-top-border plus separate-header-body-row chrome;
- terminal header path extraction for structured Write and Edit;
- documentation examples that present the old schema as valid.

No compatibility branch may be reintroduced to make stale tests pass. Internal
callers and tests migrate to `files[]`.

## Verification And Acceptance Criteria

The implementation is accepted only when focused evidence proves:

1. schema exposes only non-empty ordered `files[]` and rejects legacy/unknown
   fields;
2. mixed create/overwrite prepares without side effects and approval receives
   verified typed projection;
3. duplicate targets, unsafe types, non-UTF-8 overwrites, and no-op overwrites
   fail the whole call before mutations;
4. all Write target parents participate in instruction preflight;
5. plan mode allows exactly one active plan-file target and rejects any batch
   containing another target;
6. cached/session approval uses the exact full prepared target set;
7. existing and absent stale cases install zero files before the first commit;
8. per-file atomic create/replace and declaration order are preserved;
9. writer/parent/cancellation failures after prior commits report exact partial
   state without rollback;
10. created directories are reported when they remain after a failure;
11. progress uses existing events and replay never resumes execution;
12. created previews retain line numbers/highlighting/head-tail expansion;
13. overwrite/Edit diffs retain old/new line numbers, colors, change clusters,
    and content-width frames whose semantic file headers live in the top border;
14. successful Write/Edit tool headers omit paths, show one aggregate summary,
    and color `+N/-N` correctly; their per-file frames do not contain a duplicate
    header body row;
15. approval mouse wheel scrolls transcript without moving selection;
16. Delegate/Swarm layouts remain unchanged while bounded Write summaries are
    updated;
17. paired English/Chinese tool docs match the final contract;
18. repository scans find no active legacy single-file Write schema/renderer.

## ADR Signal

This design changes a durable tool schema, runtime-only prepared execution
boundary, approval presentation contract, and TUI ownership. Completion must
run the project's ADR backfill assessment. An ADR is warranted only if the
implemented architecture materially diverges from or establishes authority
beyond this approved design and the existing Batch Edit precedent; the design
spec itself remains the requirement authority for this workstream.
