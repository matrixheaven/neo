# Delegate Edit/Write File Activity Spec Brief

Status: Approved
Date: 2026-07-24
ArchitectureReviewRequired: yes

## Goal

Make every Edit and Write performed by a Delegate, DelegateGroup, or
DelegateSwarm visibly identify the affected files. Keep the existing aggregate
file and line statistics, but add an ordered per-file outcome below the shared
child tool row so users do not need `Ctrl+O` to discover which paths changed.

## Confirmed Product Decisions

1. Keep the existing Delegate, DelegateGroup, and DelegateSwarm card headers,
   hierarchy, ordering, progress, final summaries, and expansion semantics.
2. Reuse their shared child-activity renderer. Do not add card-specific Edit or
   Write renderers.
3. Keep the existing Edit/Write aggregate summary as the first tool row and add
   one visible row per affected file immediately below it.
4. Render the complete ordered file list. Do not replace paths with an omitted
   count or require expansion to reveal them.
5. Derive terminal rows from canonical structured Edit/Write `changes[]`
   results. Never parse summary prose or inspect the final filesystem to infer
   outcomes.
6. Preserve the existing child tool marker, verb, color, elapsed-time,
   child-tool count budget, and output-expansion behavior. Per-file subrows are
   part of one tool activity and are not counted as additional tools.
7. `Ctrl+O` may continue to reveal full diffs, contents, and diagnostics, but
   it must not be the first place where file identity becomes visible.

## Presentation

The diagrams show only the affected child-activity region. The surrounding
cards remain unchanged.

### Completed Delegate Edit

```text
  • Edited 3 files · 4 replacements · +42 -18
      M crates/neo-tui/src/transcript/child_activity.rs  +20 -4
      M crates/neo-tui/src/transcript/tool_renderers.rs  +18 -9
      M crates/neo-agent-core/src/tools/edit.rs           +4 -5
```

### Completed Delegate Write

```text
  • Wrote 2 files · 1 created · 1 overwritten · +96 -12
      C docs/aegis/specs/tool-file-display.md              64 lines
      M docs/en/tools.md                                   32 lines
```

### DelegateSwarm

```text
  ├─ coder-1 · completed
  │    • Edited 2 files · 3 replacements · +31 -12
  │        M crates/neo-tui/src/transcript/child_activity.rs  +24 -8
  │        M crates/neo-tui/src/transcript/tool_renderers.rs   +7 -4
  │
  └─ coder-2 · completed
       • Wrote 1 file · 1 created · +48 -0
           C docs/en/tool-display.md                         48 lines
```

DelegateGroup uses the same file rows under its existing per-agent indentation.

### Running

Before a structured prepared/progress result exists, paths come from the
canonical arguments in declaration order and have no invented operation or
statistics:

```text
  • Using Edit (3 files) · 1s
      … crates/neo-tui/src/transcript/child_activity.rs
      … crates/neo-tui/src/transcript/tool_renderers.rs
      … crates/neo-agent-core/src/tools/edit.rs
```

Once a prepared or progress result arrives, its structured file projection
replaces the argument-only rows.

### Partial Failure

```text
  ✗ Write partial 2/3 · +99 -0
      ✓ C docs/en/tools.md                         48 lines
      ✓ C docs/zh/tools.md                         51 lines
      ✗ M README.md                                permission denied
```

`not_attempted` files remain visible with a neutral marker. A failed prepare or
stale check without `changes[]` shows the path identified by structured error
details; remaining argument paths stay visible without invented outcomes.

## File Row Contract

| Tool/outcome | File marker | Required fields |
|---|---|---|
| Edit planned/running | `…` | path |
| Edit committed | `M` | path, added, removed |
| Write planned/running | `…` | path |
| Write created | `C` | path, final line count |
| Write overwritten | `M` | path, final line count |
| Failed | `✗` | operation when known, path, compact diagnostic when present |
| Not attempted | `–` | operation when known, path |

Additional rules:

- preserve tool declaration/result order;
- display workspace-contained targets relative to the active workspace and
  external authorized targets as absolute paths using `Path`/`PathBuf` logic;
- wrap long paths onto continuation rows instead of silently truncating them;
- keep aggregate statistics on the existing tool summary row;
- do not display unified diffs or created file contents in normal file rows;
- do not add a semantic file-count limit;
- normal transcript viewport clipping is scrolling, not omission: every row
  remains in the rendered transcript and can be reached without switching the
  tool-output expansion mode.

## Data And Ownership

Current Edit and Write terminal results already own the required ordered
`changes[]` data, including path, status, operation where applicable, and
per-file statistics. Those tool contracts remain authoritative and unchanged.

The multi-agent runtime currently collapses these details into one aggregate
`summary` before constructing `AgentActivityKind::Tool`. It must additionally
project the presentation-safe file fields into an optional typed child-activity
file list. The projection excludes full diffs and created contents.

```text
Edit/Write ToolResult.details.changes[]
                  |
                  v
multi_agent::runtime typed file projection
                  |
                  v
AgentActivityKind::Tool
                  |
                  v
child_activity::render_child_tool_row
                  |
          +-------+--------+
          |       |        |
      Delegate  Group    Swarm
```

The TUI consumes the typed projection; it does not parse `ToolResult.content`,
aggregate summary strings, diffs, or arbitrary JSON. Older replayed snapshots
without the optional projection retain the existing summary-only rendering.
This is replay compatibility, not a second live owner or fallback parser.

## Boundaries

- Do not change Edit or Write model-visible schemas, prepare/commit behavior,
  result `details` schemas, approval presentation, or atomicity semantics.
- Do not change Delegate-family headers, progress estimates, scheduling,
  lifecycle state, card identity, final summaries, or transcript placement.
- Do not add a new runtime event, card component, generic file-change system,
  or JSON parser in `neo-tui`.
- Do not expose full file contents or diffs through `AgentSnapshot` activity.
- Do not redesign Bash, Terminal, MCP, or other child tool rows.

## Acceptance

1. One completed multi-file Edit in a Delegate shows the aggregate summary and
   every changed path with its per-file `+/-` statistics.
2. One completed mixed Write shows every path, distinguishes created from
   overwritten files, and shows final line counts.
3. DelegateGroup and expanded DelegateSwarm render the same rows through the
   shared child-activity owner with their existing indentation.
4. Running tools show argument paths before prepared results exist, then update
   from structured prepared/progress details without reordering them.
5. Partial commit, durability-uncertain, failed, and not-attempted outcomes are
   truthful per file and never collapse to a success-looking aggregate row.
6. Long paths wrap without loss; narrow-width rendering does not panic or
   overlap adjacent content.
7. A normal card exposes all file identities; `Ctrl+O` only adds existing full
   result detail.
8. Non-Edit/Write child tools and all Delegate-family outer-card snapshots
   remain unchanged.
9. Old serialized snapshots without file projections replay with their current
   summary-only presentation.

## Design Trace

TaskIntentDraft:

- Outcome: make Delegate-family Edit/Write activity inspectable per file.
- Success evidence: shared renderer tests cover completed, running, partial,
  narrow-width, and old-snapshot cases across the three card families.
- Stop condition: every available canonical file outcome is visible once,
  while outer cards and tool contracts remain unchanged.
- Non-goals: tool schema changes, full diff rendering, new card types, or a
  generic activity redesign.

BaselineUsageDraft:

- Required baseline refs: `AGENTS.md`, Batch Edit/Write designs, current
  Delegate-family renderers, and current multi-agent activity projection.
- Cited in design refs:
  `docs/aegis/specs/2026-07-20-batch-edit-design.md`,
  `docs/aegis/specs/2026-07-21-batch-write-design.md`.
- Missing refs: the historical multi-agent living-transcript spec is no longer
  present at its prior path; current code and surviving approved specs define
  the active boundary.
- Decision: continue.

ImpactStatementDraft:

- Affected layers: multi-agent activity projection, shared child-activity
  presentation, and focused transcript tests.
- Canonical owners: Edit/Write own execution truth; multi-agent runtime owns the
  bounded typed projection; `child_activity.rs` owns rendering.
- Invariants: one execution owner, one projection owner, one shared renderer;
  no card-specific copies or summary-string parsing.
- Compatibility: optional activity fields default absent for old snapshots.

Existence Check:

- Proposed new surface: optional typed per-file data on existing tool activity.
- Existing owner / reuse candidate: `AgentActivityKind::Tool` and
  `render_child_tool_row`.
- Decision: reuse existing owners; add only the minimum data needed to preserve
  information that canonical tool results already provide.

ADR signal: no new ADR. This is a localized presentation projection within the
existing Edit/Write and multi-agent transcript ownership boundaries.
