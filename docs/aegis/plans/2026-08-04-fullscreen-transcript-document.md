# Fullscreen Transcript Document Implementation Plan

Date: `2026-08-04`

Status: `ready for implementation after explicit runtime-code authorization`

Spec: [`docs/aegis/specs/2026-08-04-fullscreen-transcript-document-design.md`](../specs/2026-08-04-fullscreen-transcript-document-design.md)

## Implementer Directive

Execute the approved design without reopening the product decision. Interactive
Neo has exactly one application-owned fullscreen transcript document from
startup until exit. Do not preserve an inline conversation renderer, a
history/live split, an automatic-overflow switch, the old `Ctrl+O` transcript
browser, or any assistant-prefix write into native terminal history.

The current Delegate, DelegateGroup, and DelegateSwarm component output is a
frozen boundary. Preserve their layout, hierarchy, ordering, progress,
collapsed/expanded behavior, and card-local activity limits. Remove only outer
viewport-pressure loss. Workflow keeps the fixed sibling order of main card,
Delegate summary, and DelegateSwarm summary and adds only the approved inline
expansion for a direct Workflow tool.

This document plans runtime work but does not authorize it. It also does not
authorize branch switching, pushing, deleting user session data, or modifying
the unrelated theme-manager work already present in the shared worktree.

## TaskStartSnapshot

- Branch: `main`.
- Starting commit: `43cdfd3e` (`docs: design fullscreen transcript document`).
- `docs/aegis/` was clean and the plan file did not exist before planning.
- Pre-existing user-owned changes, excluded from this plan and its commit:
  `crates/neo-agent/src/config/{loader,mod,mutations,types}.rs`,
  `crates/neo-agent/src/mcp_ops.rs`,
  `crates/neo-agent/src/modes/btw.rs`,
  `crates/neo-agent/src/modes/interactive/{controller_factory,custom_endpoint_provider,tests}.rs`,
  `crates/neo-agent/src/modes/run/mod.rs`,
  `crates/neo-agent/src/modes/sessions.rs`,
  `crates/neo-agent/src/themes.rs`, and
  `crates/neo-agent/tests/cli_commands.rs`.
- Implementation must take a fresh snapshot before each task. If an authorized
  task overlaps a user-owned file above, preserve the current contents and make
  only a narrow additive or targeted edit; never restore the file to this
  snapshot.

## Goal

Replace the interactive history/live transcript split with one scrollable,
selectable fullscreen document whose mutable entries can grow in place without
losing presentation data. Capture complete Bash and Terminal display output for
the main agent, Delegate-family children, and Workflow children before existing
result, preview, queue, and ring-buffer limits.

Stop only when focused evidence proves all of the following:

- a streaming assistant can grow beyond many terminal heights and every row
  remains reachable before completion;
- bottom following and locked logical scrolling survive entry growth, retry
  removal, wrapping, resize, and resume;
- no viewport-height budget reaches a card renderer;
- Workflow renders all structural rows and expands a direct tool in place;
- complete captured Bash and Terminal text remains reachable beyond the old
  64 KiB, six-line, 50,000-character, ring-buffer, and 10 MiB limits;
- selection crosses transcript entries and cards, supports drag auto-scroll and
  double-click word selection, and copies materialized plain text;
- interactive startup enters one fullscreen/mouse lifecycle and every normal,
  error, panic, and supported suspend path restores it;
- exit prints only the approved static projection after terminal restoration;
- print, pipe, and non-TTY resume paths never enter fullscreen;
- model-visible results, provider requests, context history, cache prefixes,
  and the default-off compaction options remain unchanged;
- the retired history/live, stable-prefix, overflow, and review-browser symbols
  have no main-path references.

## Architecture

Keep one owner for each concern:

```text
process output bytes
  -> source-side safe text capture
  -> agents/<agent-id>/tasks/<task-id>.log
  -> rebuildable sparse <task-id>.log.idx
  -> optional typed output reference on presentation events
  -> TranscriptStore entry
  -> DocumentLayout + logical TranscriptAnchor
  -> visible document slice + bottom chrome
  -> FullscreenTerminal
  -> LiveRenderer row diff at origin zero
```

- `TranscriptStore` remains the sole typed transcript content and ordering
  owner. Entries update by stable identity and revision and never move between
  history and live collections.
- `crates/neo-tui/src/transcript/document.rs` becomes the only layout and scroll
  owner. It stores per-entry rendered height, virtual start row, and one logical
  top anchor. It replaces the old absolute-row viewport and browser viewport.
- `TranscriptPane` coordinates typed entry rendering with `DocumentLayout`. It
  does not keep a second flattened-document source or clone itself for review.
- `crates/neo-agent-core/src/session/tool_output.rs` owns path-safe output
  references, complete append, sparse index format, bounded range reads, and
  index rebuild. The `.log` file is the source; `.idx` is derived.
- Bash and Terminal source readers feed that owner before bounded model result,
  live preview, response sampling, or terminal ring updates.
- `ToolResult` keeps its existing model-facing shape. Optional output metadata
  belongs on presentation/session events and child activity projections, not in
  provider-visible messages or result details.
- `FullscreenTerminal` is the renamed and simplified physical terminal owner.
  It enters alternate screen and mouse reporting once, reuses synchronized
  differential writes, and restores once.
- `LiveRenderer` and `FrameScheduler` remain the only differential renderer and
  frame scheduler. No terminal fork, cell renderer, or second scheduler is
  added.
- Task Browser remains an explicit task-control surface inside the already
  active fullscreen session. It is not a second transcript view.

## Tech Stack

- Rust 2024 workspace, minimum Rust `1.96.1`.
- `neo-agent-core` for session paths, output capture, typed events, child
  activity, and JSONL persistence.
- `neo-tui` for typed transcript entries, incremental document layout, cards,
  selection coordinates, and differential frames.
- `neo-agent` for the interactive loop, fullscreen lifecycle, clipboard side
  effects, resume wiring, and exit projection.
- Existing `serde`, `serde_json`, `uuid`, `unicode-segmentation`, terminal input,
  and clipboard dependencies only.
- Standard-library `Path`/`PathBuf`, bounded channels, and file I/O. No new
  dependency, feature flag, output-size setting, renderer, or cleanup UI.

## Baseline And Authority Refs

- Approved authority:
  [`2026-08-04-fullscreen-transcript-document-design.md`](../specs/2026-08-04-fullscreen-transcript-document-design.md).
- Existing decision to supersede only after implementation and focused proof:
  [`ADR-0010-native-terminal-transcript-presentation.md`](../adr/ADR-0010-native-terminal-transcript-presentation.md).
- Existing landed baseline to supersede only after implementation and focused
  proof:
  [`2026-07-31-native-terminal-transcript.md`](../baseline/2026-07-31-native-terminal-transcript.md).
- Workflow hierarchy and provenance baseline:
  [`2026-08-01-workflow-dynamic-transcript-design.md`](../specs/2026-08-01-workflow-dynamic-transcript-design.md).
- Shell admission and Terminal yield behavior remain governed by
  `AGENTS.md`: admission waits remain pending; an absent explicit timeout or
  cancel may wait indefinitely; Terminal yield applies only after process
  start.
- Context integrity and default-off micro compaction and snip/dedup behavior
  remain governed by `AGENTS.md`.

### BaselineUsageDraft

- Required baseline refs: approved fullscreen spec, ADR-0010, native terminal
  baseline, Workflow dynamic transcript design, current source owners, and
  `AGENTS.md`.
- Acknowledged before planning: all required refs above.
- Cited in plan: all required refs above.
- Missing refs: none.
- Decision: continue with the fullscreen spec as current authority; retain the
  old ADR and landed baseline unchanged until implementation evidence exists.

## Compatibility Boundary

- Existing JSONL remains append-only and readable. New output metadata uses
  `serde(default)` and `skip_serializing_if`; old records need no migration.
- Missing old output is shown as incomplete and never relabeled complete.
- Canonical `AgentMessage`, model-visible `ToolResult`, provider projection,
  compaction input, and cache-prefix bytes do not read display-output files.
- Retry and compaction may discard a provisional assistant entry from the
  document, while only the winning assistant message enters session history.
- Print, pipe, non-TTY resume, export, and `neo run` remain static.
- Workflow scheduling, journal, recovery, result, model-visible output, and
  typed origin remain unchanged.
- Non-workflow Delegate-family component files remain byte-for-byte unchanged
  unless an implementation compile repair proves a signature-only edit is
  unavoidable. Their rendered output must remain identical.
- The card-local `MAX_CHILD_TOOL_ROWS = 4` semantic limit remains. It is not a
  viewport omission and must not be removed.
- Task Browser behavior remains. Only its physical screen-transition dependency
  is removed.
- Paths use `Path`/`PathBuf`; terminal and clipboard behavior remains native on
  macOS, Linux, and Windows.

## Scope Check

Included:

- complete display-output capture, metadata, range reads, and recovery;
- one incremental transcript document and logical scroll anchor;
- bottom follow, locked-scroll indicator, resize/reflow stability, and visible
  animation scheduling;
- one fullscreen terminal lifecycle and removal of the old interactive paths;
- mouse coordinates, cross-entry selection, auto-scroll, word selection, and
  clipboard routing;
- complete Workflow structural rendering and direct-tool inline expansion;
- exit projection, static-mode preservation, focused tests, native platform
  proof, and post-implementation architecture records.

Excluded:

- card-body redesign for Delegate, DelegateGroup, or DelegateSwarm;
- search, bookmarks, editing, detachable panes, or another detail view;
- binary-perfect terminal recording or preservation of arbitrary escape
  programs;
- lifting deliberate source-tool pagination such as Read or Grep;
- writing complete display output into JSONL event bodies, model context, or
  provider requests;
- a new hosted service, cleanup manager, migration command, feature flag, or
  fallback inline renderer;
- porting the grok-build renderer or its terminal fork.

## Requirement Ready Check

- Requirement source refs: approved fullscreen design and explicit user
  approval on 2026-08-04.
- Goals and scope refs: design Goals, Non-goals, Selected Architecture,
  Compatibility Boundary, and Acceptance Criteria.
- User scenarios: bottom following, locked upward scroll, oversized assistant
  and cards, complete Workflow, cross-card copy, and terminal restoration.
- Acceptance refs: design sections Document And Scrolling, Cards And Workflow,
  Output Retention, Selection And Fullscreen Lifecycle, Rendering And
  Performance, and Context Integrity.
- Open blocker questions: none for planning.
- Decision: ready.

## Facts, Assumptions, And Constrained Unknowns

### Facts

- `TranscriptStore` already has stable entry IDs, revisions, event ordering,
  dirty-entry tracking, and typed mutation methods.
- `TranscriptPane` already caches per-entry rendering information and entry row
  starts, but also owns a flattened row cache and two render paths.
- `TranscriptPresentation` currently owns the history/live split,
  acknowledgement ledger, live budget, and viewport omission markers.
- `render_terminal_frame_at` currently branches between ordinary history/live,
  the `Ctrl+O` transcript browser, and full-screen overlays.
- `InlineTerminal` currently owns native history insertion, dynamic-region
  geometry, saved normal-screen state, review-screen transitions, synchronized
  writes, and rollback after write failure.
- `LiveRenderer` already rejects oversized physical frames, emits row-level
  diffs, and writes nothing for an unchanged frame.
- Current animation cadence is 100 ms, already below the approved 30 frames per
  second ceiling.
- Bash drops log chunks when a bounded queue fills and caps its log; Terminal
  keeps only a bounded ring. `ToolCallComponent` keeps a six-line and
  50,000-character live preview and discards it at finalization.
- Current SGR parsing keeps only wheel direction. It discards coordinates,
  release, drag, and modifier information.
- Current selection stores entry indexes and copies whole entries, not rendered
  row/cell endpoints.

### Assumptions

- Existing per-agent task directories are the correct access and lifecycle
  boundary for output files.
- Existing task IDs are path-safe UUIDs; the output owner still validates both
  agent and task IDs and never accepts caller-supplied path separators.
- A fixed sparse index stride of 256 logical lines is sufficient. It is an
  internal constant, not configuration.
- Width-specific wrapped-row indexes are derived TUI cache. They may stream-scan
  a complete output file outside the physical terminal write when a user first
  expands it or the width changes, while retaining bounded memory.
- Synchronous bounded range reads from a local session file are acceptable on
  the render owner when cached and limited to the visible range plus look-ahead.

### Constrained Unknowns

- Exact helper extraction inside `store.rs` and `pane.rs` may change after the
  first compile, but the new responsibility stays in `document.rs`; it must not
  return to `presentation.rs` or create another store.
- Exact child activity type names may require narrow signature repair, but the
  output reference must travel through typed fields, never be inferred from
  rendered strings or result JSON.
- Real terminal Shift-drag bypass and clipboard commands vary by emulator and
  require native smoke evidence in addition to deterministic tests.

## Change Necessity

- User-visible need: mutable content must remain complete, scrollable,
  selectable, and dynamically refreshed at arbitrary document height.
- No-change option: documentation or a larger live-row tail cannot recover rows
  already dropped by the presentation budget, Bash log queue, Terminal ring, or
  tool preview.
- Why code is necessary: the normal terminal screen cannot rewrite mutable rows
  outside the physical viewport. A single application document and earlier
  output capture are the minimum mechanisms that satisfy the approved behavior.
- Minimum change boundary: transcript layout/presentation, screen lifecycle,
  input/selection, shell output capture/event metadata, Workflow outer
  rendering, resume, and exit projection.
- Decision: code-change after explicit runtime authorization.

## Existence Check

### Document layout

- Proposed surface: `transcript/document.rs`.
- Existing reuse candidates: `TranscriptStore`, `TranscriptBodyCache`, entry
  revisions, cached row starts, and old `TranscriptViewport` behavior.
- Why existing placement is insufficient: `store.rs` is 2,503 lines and
  `pane.rs` is 2,148 lines; neither should absorb a new layout/anchor state
  machine. The new file replaces `browser.rs` and the layout portions of the
  1,658-line `presentation.rs` rather than adding another path.
- Decision: add with proof; delete the old viewport/browser/presentation owners.

### Complete output owner

- Proposed surface: `session/tool_output.rs`.
- Existing reuse candidates: `agent_tasks_dir`, task IDs, Bash `.log` naming,
  bounded channels, and existing session cleanup.
- Why existing logs are insufficient: they omit foreground/model paths, drop on
  queue pressure, stop at 10 MiB, and do not cover Terminal.
- Creation proof: one shared format and reader is required by Bash, Terminal,
  resume, child projection, and the TUI. Reusing the old logger unchanged would
  preserve the data-loss bug.
- Entropy impact: the session output owner replaces agent-path log truncation as
  the complete source; the old bounded result and preview remain only for their
  existing model/UI summary roles.
- Decision: add with proof; no second complete-output store.

### Selection state

- Proposed surface: `transcript/selection.rs`.
- Existing reuse candidates: entry selection in `store.rs`, row starts, visible
  width helpers, `unicode-segmentation`, and the clipboard writer.
- Why extraction is justified: document-coordinate selection, drag lifecycle,
  materialization, and word boundaries are independent of typed entry storage;
  keeping them in the already oversized store would add a second reason to
  change.
- Decision: add with proof; remove the old index-based selection type.

## Architecture Integrity Lens

- Invariant: one typed transcript, one layout owner, one scroll owner, one
  physical fullscreen writer, and one complete output source per tool process.
- Canonical owners: `TranscriptStore`, `DocumentLayout`, `ToolOutputStore`,
  `FullscreenTerminal`, and the existing clipboard writer.
- Responsibility overlap to remove: `TranscriptPresentation` versus
  `TranscriptPane`, browser viewport versus primary viewport, normal versus
  review terminal state, live preview versus complete output, and entry-index
  versus document-coordinate selection.
- Higher-level simplification: delete the entire interactive history/live and
  review-browser paths before adding card-specific overflow fixes.
- Falsifier: any implementation that passes terminal-height budgets to card
  renderers, writes assistant prefixes to native history, retains two viewport
  states, or lets preview data claim completeness fails this plan.
- Verdict: proceed on the single-document path only.

## Anti-Entropy Declaration

- Deletion class: internal code retirement and optional event metadata change.
- Old paths: history/live projection, acknowledgement ledger, live budget,
  stable assistant prefix, automatic overflow, protected native history,
  review browser, review screen state, viewport omission markers, and
  index-based selection.
- New canonical owners: `TranscriptStore` plus `DocumentLayout`,
  `ToolOutputStore`, and `FullscreenTerminal`.
- Preserved behavior: typed transcript ordering, retry-safe persistence,
  Delegate-family cards, Workflow runtime, Task Browser, static modes,
  terminal restoration, frame diffing, and clipboard writing.
- Retired behavior: inline mutable conversation rendering, native-scrollback
  conversation ownership, `Ctrl+O` review, and viewport-driven card omission.
- External boundary touched: optional JSONL metadata and terminal protocol
  behavior, both backward-readable.
- Source-of-truth data risk: none; no existing session or output file is
  deleted or rewritten.
- User confirmation required for code retirement: no after runtime
  implementation is explicitly authorized.

## Retirement Decision

- Path: delete-first.
- Why: all retired paths are internal, the approved fullscreen path replaces
  their behavior, and no active external dependency requires them.
- Non-edits: do not delete user sessions, old JSONL, existing output files, or
  Task Browser state.
- Verification: prove the main path works, scan for lingering symbols and
  omission strings, prove retired shortcuts no longer activate, and prove old
  JSONL and static modes remain readable.

## TDD Route

- Mode: `off`.
- Decision: `skipped`.
- Strict authority: `not applicable`.
- Test posture: minimum implementation followed by focused post-change
  regression and native behavior smoke checks.
- Reason: strict test-first work was not requested. The approved design and
  current source already identify the failure mechanisms.
- Verification: every automated command names one package, one target selector,
  and one test-name filter; native terminal claims remain separate.

## Plan-Time Complexity Check

### Complexity Budget

- Artifact class: source, test, decision, and persistent event metadata.
- Target pressure: `store.rs` 2,503 lines, `pane.rs` 2,148 lines,
  `presentation.rs` 1,658 lines, `inline_terminal.rs` 956 lines,
  `interactive/mod.rs` 2,933 lines, `interactive/input.rs` 1,707 lines, and
  `interactive/tests.rs` 19,475 lines.
- Projected result: at risk if new state is embedded into those mixed owners;
  within budget if focused owners replace deleted files and large files receive
  wiring-only edits.
- Planned governance: add `document.rs`, `selection.rs`, and `tool_output.rs`;
  replace `browser.rs`, `presentation.rs`, and `streaming_prefix.rs`; rename and
  simplify `inline_terminal.rs`; add focused test targets instead of expanding
  `interactive/tests.rs`.

### File Boundary Decisions

- `store.rs`: typed content and revision wiring only.
- `pane.rs`: document coordination and visible-slice rendering only.
- `document.rs`: layout, logical anchor, follow/lock, new-activity state, and
  visible range resolution.
- `selection.rs`: document endpoints, drag state, auto-scroll intent, word
  selection, and materialized text.
- `tool_output.rs`: path validation, append metadata, sparse indexes, bounded
  reads, and rebuild.
- `app.rs`, `interactive/mod.rs`, and `input.rs`: wiring-only after helper
  extraction.
- `interactive/tests.rs`: do not add new test blocks. Put private interactive
  tests in `modes/interactive/fullscreen_tests.rs` and CLI/static tests in a new
  integration target.
- Recommendation: extract focused owners and make delete-first wiring changes;
  do not add new responsibilities to oversized files.

## File Map

### Create

- `crates/neo-agent-core/src/session/tool_output.rs`
- `crates/neo-agent-core/tests/tool_output_capture.rs`
- `crates/neo-tui/src/transcript/document.rs`
- `crates/neo-tui/src/transcript/selection.rs`
- `crates/neo-tui/tests/fullscreen_transcript.rs`
- `crates/neo-tui/tests/transcript_selection.rs`
- `crates/neo-agent/src/modes/interactive/fullscreen_tests.rs`
- `crates/neo-agent/tests/fullscreen_output.rs`
- `docs/aegis/adr/ADR-0012-fullscreen-transcript-document.md`
- `docs/aegis/baseline/2026-08-04-fullscreen-transcript-document.md`

### Modify

- `crates/neo-agent-core/src/session/{mod,layout,event_persistence}.rs`
- `crates/neo-agent-core/src/events.rs`
- `crates/neo-agent-core/src/tools/{mod,bash,terminal,background_tasks}.rs`
- `crates/neo-agent-core/src/tools/shell_guard/{client,guardian,terminal_guard,status}.rs`
- `crates/neo-agent-core/src/multi_agent/{runtime,state}.rs`
- `crates/neo-agent-core/src/runtime/{tool_dispatch,workflow_dispatch}.rs`
- `crates/neo-tui/src/input/{mod,raw_input}.rs`
- `crates/neo-tui/src/transcript/{mod,store,pane,event_handler,entry/mod,tool_call,live_output,progressive}.rs`
- `crates/neo-tui/src/transcript/{workflow_group,workflow_card,workflow_delegate_card,workflow_swarm_card}.rs`
- `crates/neo-tui/src/screen_output/{mod,terminal_modes,live_renderer}.rs`
- `crates/neo-tui/src/app.rs`
- `crates/neo-tui/src/shell/{mod,overlay,dialog_factory}.rs`
- `crates/neo-tui/tests/{transcript_store,transcript_pane,terminal_frame,live_renderer,workflow_transcript,tool_cards,multi_agent_transcript}.rs`
- `crates/neo-agent/src/modes/interactive/{mod,input,terminal_io,prompt_edit,frame_scheduler}.rs`
- `crates/neo-agent/src/main.rs`
- `docs/aegis/adr/ADR-0010-native-terminal-transcript-presentation.md`
- `docs/aegis/work/2026-08-04-fullscreen-transcript-document/*`
- `docs/aegis/INDEX.md`

### Rename Or Delete Without Compatibility Aliases

- rename `crates/neo-tui/src/screen_output/inline_terminal.rs` to
  `fullscreen_terminal.rs` and `InlineTerminal` to `FullscreenTerminal`;
- rename `crates/neo-tui/tests/inline_terminal.rs` to
  `fullscreen_terminal.rs`;
- delete `crates/neo-tui/src/transcript/presentation.rs`;
- delete `crates/neo-tui/src/transcript/browser.rs`;
- delete `crates/neo-tui/src/transcript/streaming_prefix.rs`;
- delete `crates/neo-tui/tests/terminal_scrollback.rs`;
- delete obsolete history/live, browser, and stable-prefix tests from their
  remaining targets rather than preserving old expectations.

## Dependency Order

```text
Task 1 complete output store
  -> Task 2 Bash and Terminal source capture
  -> Task 3 typed metadata, child projection, and resume
                                      |
Task 4 document layout and anchor -----+
  -> Task 5 single fullscreen lifecycle and old-path retirement
  -> Task 6 mouse selection and clipboard
                                      |
Task 3 + Task 4 -----------------------+
  -> Task 7 complete cards and Workflow expansion
  -> Task 8 exit/static/resume integration
  -> Task 9 native validation and architecture closeout
```

Do not begin Task 5 by passing `usize::MAX` through the old inline path. The
single document must exist before history/live and Workflow budgets are
deleted. Do not begin Task 7 complete expansion before Tasks 1-3 provide a
typed, readable output reference.

## Tasks

### Task 1: Establish The Complete Session Output Store

**Files:**

- Create `crates/neo-agent-core/src/session/tool_output.rs`.
- Modify `crates/neo-agent-core/src/session/{mod,layout}.rs`.
- Create `crates/neo-agent-core/tests/tool_output_capture.rs`.

**Why:** Current logs, result strings, preview callbacks, and terminal rings are
not complete sources. A reusable session owner must exist before either process
reader can promise lossless display output.

**Change Necessity:** Reusing the existing logger unchanged preserves queue and
size loss. The minimum new responsibility is one session module that resolves
safe task paths and owns the raw text plus rebuildable index.

**Impact / Compatibility:** This task adds only session-scoped display files and
range-reading APIs. It does not change JSONL, tool results, model context, or
existing session cleanup; old sessions simply have no complete-output file.

**Required types and behavior:**

```rust
pub struct ToolOutputRef {
    pub agent_id: String,
    pub task_id: String,
    pub byte_len: u64,
    pub line_count: u64,
    pub complete: bool,
}

pub struct ToolOutputRange {
    pub text: String,
    pub start_line: u64,
    pub next_line: u64,
    pub reached_end: bool,
}

pub struct ToolOutputStore {
    session_dir: PathBuf,
}
```

- Resolve only `agents/<agent_id>/tasks/<task_id>.log` and adjacent `.log.idx`
  through `agent_tasks_dir`; reject empty IDs, separators, `.` and `..`.
- Append normalized UTF-8 display text in source order. Preserve newlines and
  printable content; do not interpret it as terminal control or model context.
- Record a byte offset every 256 logical lines and final byte/line counts.
- Treat `.log` as source and `.idx` as derived. Rebuild a missing/corrupt index
  by streaming `.log` with bounded memory and atomic index replacement.
- Read only the requested line range plus bounded look-ahead; never load the
  complete file into memory.
- Opening must precede process launch. Append failure is returned to the caller.
- No configured byte or line ceiling and no unbounded in-memory queue.

**Steps:**

1. Add path validation, file naming, metadata structs, and a pure index parser.
2. Add append and finish operations with monotonic byte/line metadata.
3. Add bounded range reads from the nearest sparse offset.
4. Add missing/corrupt-index rebuild without modifying the source log.
5. Cover invalid IDs, chunk-split UTF-8/newlines, data beyond 10 MiB, range
   boundaries, rebuild, missing source, and injected write failure.

**Verification:**

```bash
rtk cargo nextest run -p neo-agent-core --test tool_output_capture tool_output_store
rtk cargo nextest run -p neo-agent-core --test session_jsonl session_layout_paths_are_agent_scoped
```

Expected: the new target passes; the existing layout test still proves
agent-scoped paths; no test reads the user's real session directory.

**Commit:** `feat(core): add complete tool output store`

### Task 2: Capture Bash And Terminal Before Existing Limits

**Files:**

- Modify `crates/neo-agent-core/src/tools/{mod,bash,terminal,background_tasks}.rs`.
- Modify `crates/neo-agent-core/src/tools/shell_guard/{client,guardian,terminal_guard,status}.rs`.
- Modify focused tests in `crates/neo-agent/tests/{tool_bash_guardian,tool_terminal_guardian}.rs`.

**Why:** A complete store is useless if bytes first pass through the dropping
Bash queue or Terminal ring.

**Change Necessity:** The minimum correct capture points are
`spawn_output_drain` and `spawn_terminal_reader`; later callbacks cannot recover
dropped chunks.

**Impact / Compatibility:** Bash and Terminal process supervision gain one
fallible display-output sink. Existing bounded results, previews, admission,
timeout, cancellation, background, and Terminal-yield behavior stay unchanged.

**Repair Track:**

- Root cause: Bash uses `try_send` and a 10 MiB writer; Terminal stores only a
  bounded `TerminalOutputBuffer`.
- Canonical repair: open the session capture before launch, append each decoded
  chunk before bounded preview/result updates, and apply bounded backpressure.
- Failure behavior: an open failure prevents launch; an append failure reaches
  supervision, stops the process, and reports possible partial side effects.
- Existing admission waits, explicit timeout/cancel, background execution, and
  Terminal yield timing remain unchanged.

**Retirement Track:**

- Agent tool executions no longer use the dropping/capped log writer as a
  completeness source.
- `TaggedHeadTailBuffer`, model result caps, Terminal ring, and live callback
  sampling remain bounded for their existing purposes and cannot claim
  completeness.
- Do not add an unbounded `Vec`, channel, or result field.

**Steps:**

1. Pass the agent task output target from `ToolContext` through `BashStart` and
   `TerminalStart`; internal non-agent shell uses remain explicitly uncaptured.
2. Make Bash append to the capture before `TaggedHeadTailBuffer`, response
   sampling, and bounded preview delivery.
3. Make Terminal append before `TerminalOutputBuffer::push`.
4. Propagate writer/read-thread failure into guardian termination and final
   status instead of cleanup-only diagnostics or silent thread exit.
5. Keep foreground/background result and TaskOutput bytes bounded exactly as
   today; read their bounded views from existing buffers, not the complete file.
6. Cover queue saturation, output over 10 MiB, Terminal output over ring
   capacity, open failure before spawn, midstream failure after side effects,
   cancellation, and no explicit timeout.

**Verification:**

```bash
rtk cargo nextest run -p neo-agent --test tool_bash_guardian complete_agent_output_survives_preview_queue_pressure
rtk cargo nextest run -p neo-agent --test tool_terminal_guardian terminal_capture_survives_ring_overflow
rtk cargo nextest run -p neo-agent-core --test tool_bash bash_foreground_details_do_not_leak_output_past_max_output_bytes
```

Expected: complete files contain sentinels beyond old caps; model-visible
results remain bounded; injected capture failure prevents or stops execution
with an explicit partial-side-effect diagnostic.

**Commit:** `feat(core): capture complete shell output`

### Task 3: Persist Typed Output References Through Every Agent Path

**Files:**

- Modify `crates/neo-agent-core/src/events.rs`.
- Modify `crates/neo-agent-core/src/session/event_persistence.rs`.
- Modify `crates/neo-agent-core/src/runtime/{tool_dispatch,workflow_dispatch}.rs`.
- Modify `crates/neo-agent-core/src/multi_agent/{runtime,state}.rs`.
- Modify `crates/neo-agent-core/src/tools/terminal.rs`.
- Modify `crates/neo-tui/src/transcript/{event_handler,store,tool_call}.rs`.
- Modify focused session, runtime, multi-agent, and Workflow tests.

**Why:** Main-agent live capture alone does not make child output reachable after
aggregation or resume.

**Change Necessity:** Typed optional metadata is the minimum association that
survives JSONL and child projection. Text, result JSON, timing, and ID
correlation are forbidden inference sources.

**Impact / Compatibility:** New event fields are optional and old JSONL remains
readable. The reference is presentation metadata only; it must not enter
`ToolResult`, canonical messages, provider requests, or cache-prefix input.

**Required behavior:**

- Add `Option<ToolOutputRef>` with serde defaults to relevant
  `ToolExecutionStarted/Update/Finished`, `ShellCommandFinished`, and Terminal
  session events. Do not add it to `ToolResult`, `AgentMessage`, or provider
  request data.
- A Terminal process keeps one output reference across start/read/write/resize/
  stop tool calls.
- Child `wire.jsonl`, `AgentActivityKind::Tool`, `DelegateToolProgress`, and
  swarm child progress retain the same typed reference and final metadata.
- Workflow direct tools use the main agent reference; Workflow Delegate-family
  tools use their child agent reference while remaining grouped under the
  Workflow entry.
- `SessionEventPersistence` may remove bounded live preview text but must retain
  the output reference.
- Old records deserialize with `None`. New records with missing source files are
  explicitly incomplete.

**Steps:**

1. Add optional fields and constructor plumbing to `AgentEvent` and child
   activity structs.
2. Make runtime dispatch allocate/attach the reference without changing
   `ToolResult`.
3. Carry Terminal session identity across subsequent Terminal calls.
4. Preserve references in child wire persistence and root activity projection.
5. Rehydrate them into the original `ToolCallComponent` or Workflow direct tool
   by typed tool/agent/run identity.
6. Add a request-projection regression proving full output text, paths, and
   references do not enter model context or cache-prefix input.

**Verification:**

```bash
rtk cargo nextest run -p neo-agent-core --test session_jsonl tool_output_reference_is_optional_and_round_trips
rtk cargo nextest run -p neo-agent-core --test multi_agent_runtime child_tool_output_reference_survives_wire_replay
rtk cargo nextest run -p neo-agent-core --test workflow_dispatch bash_lifecycle_events_use_invocation_id
rtk cargo nextest run -p neo-agent-core --test runtime_turn display_output_never_enters_model_context
rtk cargo nextest run -p neo-tui --test workflow_transcript jsonl_replay_preserves_workflow_question_tool_and_child_grouping
```

Expected: old fixtures remain readable, new references survive every origin,
and request-visible context is byte-for-byte unchanged except for ordinary new
canonical messages.

**Commit:** `feat(core): persist tool output references`

### Task 4: Build One Incremental Document And Logical Scroll Anchor

**Files:**

- Create `crates/neo-tui/src/transcript/document.rs`.
- Modify `crates/neo-tui/src/transcript/{mod,store,pane,progressive}.rs`.
- Create `crates/neo-tui/tests/fullscreen_transcript.rs`.
- Modify focused transcript cache tests.

**Why:** Dynamic height is safe only when the complete document owns layout and
the physical terminal receives a bounded visible slice.

**Change Necessity:** The old absolute row offset drifts when entries above grow.
The minimum replacement is one anchor keyed by entry identity and logical
position.

**Impact / Compatibility:** This replaces viewport geometry without changing
entry identity, ordering, card rendering, or retry ownership. Resume may rebuild
derived layout, but no scroll or selection state is persisted.

**Required types and behavior:**

```rust
pub struct TranscriptAnchor {
    pub entry_id: TranscriptEntryId,
    pub row_in_entry: usize,
    pub cell_offset: usize,
}

pub struct EntryLayout {
    pub entry_id: TranscriptEntryId,
    pub revision: u64,
    pub start_row: usize,
    pub height: usize,
}

pub struct DocumentViewport {
    pub anchor: Option<TranscriptAnchor>,
    pub following_tail: bool,
    pub new_activity: bool,
}
```

- Invalidate only a changed entry by ID/revision, recompute its height, and
  shift later virtual starts by the height delta.
- On tail follow, resolve directly to the new document bottom.
- On upward scroll, store the current top logical point. Later revisions set one
  Boolean new-activity indicator without changing the anchor.
- On resize/theme change, rebuild widths outside physical write and resolve the
  same anchor against new wrapping.
- If a retry removes the anchored provisional entry, fall back to the nearest
  preceding surviving entry while remaining locked.
- Replace the full flattened `rows` cache with per-entry render caches and
  visible-range composition.
- Dynamic Delegate-family facts remain attached to their entry and are not
  acknowledged away after physical output.
- Expanded huge output computes width-specific wrap rows by streaming the
  output file with bounded memory and caches the derived mapping for that width.

**Steps:**

1. Move viewport state out of `store.rs` and add the document types above.
2. Change `TranscriptBodyCache` to per-entry revision/height/start metadata.
3. Implement tail follow, explicit lock, Boolean activity, fallback, and range
   resolution.
4. Render only intersecting entries while preserving exact virtual geometry.
5. Add resize/reflow and expanded-output derived layout invalidation.
6. Delete code that clears progressive facts after history acknowledgement.

**Verification:**

```bash
rtk cargo nextest run -p neo-tui --test fullscreen_transcript logical_anchor_survives_growth_removal_resize_and_wrap
rtk cargo nextest run -p neo-tui --test fullscreen_transcript tail_follow_and_locked_scroll_have_one_activity_indicator
rtk cargo nextest run -p neo-tui --test transcript_pane streaming_assistant_grows_past_ten_viewports_without_omission
rtk cargo nextest run -p neo-tui --lib append_only_render_reuses_cached_body_prefix
```

Expected: document height grows, the physical slice remains bounded, locked
content stays stable, and unchanged entry render caches are reused.

**Commit:** `feat(tui): add fullscreen transcript document`

### Task 5: Retire History/Live And Enter One Fullscreen Lifecycle

**Files:**

- Delete `transcript/{presentation,browser,streaming_prefix}.rs`.
- Modify `transcript/{mod,pane,store}.rs`, `app.rs`, and shell overlay files.
- Rename and simplify `screen_output/inline_terminal.rs` to
  `screen_output/fullscreen_terminal.rs`.
- Modify `screen_output/{mod,terminal_modes,live_renderer}.rs`.
- Modify `neo-agent` interactive `terminal_io.rs`, `input.rs`, and `mod.rs`.
- Replace old terminal-scrollback/review tests with focused fullscreen tests.

**Why:** Leaving the old renderer or review state active preserves two layout
owners and reintroduces the exact overflow failure.

**Change Necessity:** The single document cannot be authoritative while native
history insertion, history/live projection, and the review viewport can still
render the same transcript. The minimum safe change is to delete those paths
and simplify the existing physical writer into one fullscreen owner.

**Impact / Compatibility:** Interactive sessions enter alternate screen and
mouse reporting once. Task Browser, terminal restoration, suspend/resume,
differential writes, and static modes remain; `Ctrl+O` changes only from opening
a review page to toggling the selected tool inside the primary document.

**Repair Track:**

- Root cause: `TranscriptPresentation` creates a mutable suffix bounded by
  terminal height; `InlineTerminal` then coordinates native history and review
  screen transitions.
- Canonical repair: one document slice fills the transcript region; one
  fullscreen renderer writes a frame bounded to terminal height.
- Preserve synchronized writes, image deletion, write-failure rollback, cursor
  handling, suspend/resume, and no-write-on-unchanged behavior.

**Retirement Track:**

- Delete `TranscriptTerminalUpdate`, `FinalizedBlock`, proof/block IDs,
  acknowledgement methods, `bound_live_blocks`, omission markers,
  `stable_prefix_len`, protected history insertion, flattened history,
  `TranscriptBrowserState`, review-surface flags, saved-normal renderer state,
  and duplicate expansion/scroll state.
- Keep `KeybindingAction::ToolOutputToggle` and route `Ctrl+O` to the selected
  tool in the primary document. It must not open another surface.
- Keep Task Browser; render it as an overlay in the already active fullscreen
  session without a physical transition.

**Steps:**

1. Make `TerminalFrame` contain only bounded frame lines, cursor, and next
   animation deadline.
2. Compose the visible document slice and fitted chrome once in
   `render_terminal_frame_at`.
3. Enter alternate screen and mouse reporting in `TerminalModeGuard::enter`,
   and restore them in leave, error, panic guard, and suspend preparation.
4. Rename the terminal owner with no alias and keep `LiveRenderer` at origin
   zero.
5. Remove history acknowledgement from `NeoTerminal::draw_tui`.
6. Remove every `Ctrl+O` browser factory, overlay, renderer, and input branch.
7. Remove old tests rather than changing their expected output to accept both
   paths.

**Verification:**

```bash
rtk cargo nextest run -p neo-tui --test fullscreen_transcript fullscreen_lifecycle_enters_and_restores_once
rtk cargo nextest run -p neo-tui --test terminal_frame interactive_frame_is_one_bounded_fullscreen_document
rtk cargo nextest run -p neo-tui --test live_renderer unchanged_live_frame_emits_no_bytes
rtk cargo nextest run -p neo-agent --bin neo ctrl_o_toggles_primary_document_without_review_surface
rtk cargo nextest run -p neo-agent --bin neo task_browser_stays_inside_existing_fullscreen_surface
```

Expected: one enter and one leave sequence, no native history write, no review
transition, Task Browser remains operable, and repeated frame content emits no
bytes.

**Commit:** `refactor(tui): use one fullscreen transcript`

### Task 6: Add Document-Coordinate Selection And Clipboard Routing

**Files:**

- Create `crates/neo-tui/src/transcript/selection.rs`.
- Modify `neo-tui` input, document, pane, store, app, and frame files.
- Modify `neo-agent` interactive `terminal_io.rs`, `input.rs`, and
  `prompt_edit.rs`.
- Create `crates/neo-tui/tests/transcript_selection.rs`.
- Add focused private interactive tests outside `interactive/tests.rs`.

**Why:** Alternate screen has no native scrollback selection. The application
must supply ordinary cross-card text selection and reuse the system clipboard.

**Change Necessity:** Existing entry-index selection cannot express a word,
partial row, display cell, or cross-entry drag.

**Impact / Compatibility:** Mouse events become coordinate-bearing across the
interactive input path. Blocking dialogs and Task Browser keep priority,
Shift-drag remains available to the terminal, and clipboard failure never
destroys the current selection.

**Required types and behavior:**

```rust
pub struct DocumentPoint {
    pub entry_id: TranscriptEntryId,
    pub row_in_entry: usize,
    pub display_cell: usize,
}

pub enum MouseKind {
    Press,
    Drag,
    Release,
    ScrollUp,
    ScrollDown,
}

pub struct MouseEvent {
    pub kind: MouseKind,
    pub button: MouseButton,
    pub column: u16,
    pub row: u16,
    pub modifiers: KeyModifiers,
}
```

- Replace `ScrollUp`/`ScrollDown` and partial mouse parsing with one typed mouse
  event. Migrate every existing scroll consumer; do not keep duplicate wheel
  events.
- Convert one-based SGR coordinates to zero-based exactly once.
- `app.rs` maps screen coordinates into transcript-local coordinates;
  `DocumentLayout` maps those into `DocumentPoint`.
- Drag may cross entries/cards. Crossing the top/bottom edge requests scrolling
  through the existing frame cadence and stops on release.
- Double-click selects one Unicode word using the installed dependency.
- Movement threshold separates clicks from drags so existing card controls keep
  their actions.
- Shift-modified drag is not consumed by Neo.
- Mouse release materializes plain text at the observed document revision.
  Later updates cannot change copied text.
- If an endpoint row disappears, clamp inside the same entry, then use normal
  anchor fallback.
- A visible copy action and the existing configured copy action both call the
  existing clipboard writer. Clipboard failure preserves selection.
- Coalesce only consecutive drag-motion events in the input queue; never drop
  press or release.

**Steps:**

1. Parse SGR button, coordinates, motion, release, wheel, and modifiers.
2. Replace old selection and wheel variants with the typed event and endpoint
   state.
3. Add screen-to-document mapping and materialization across entries.
4. Add auto-scroll and double-click state without a second timer/scheduler.
5. Preserve Task Browser and blocking-dialog input priority ahead of document
   selection.
6. Route copy through `copy_transcript_selection_to_clipboard` and keep errors
   visible without clearing selection.

**Verification:**

```bash
rtk cargo nextest run -p neo-tui --lib sgr_mouse_parses_coordinates_buttons_modifiers_motion_and_release
rtk cargo nextest run -p neo-tui --test transcript_selection selection_crosses_entries_autoscrolls_and_materializes_text
rtk cargo nextest run -p neo-tui --test transcript_selection double_click_selects_one_unicode_word
rtk cargo nextest run -p neo-agent --bin neo selection_and_task_browser_preserve_input_priority
```

Expected: exact document text is copied across card boundaries, wide characters
map correctly, Shift drag is ignored, and Task Browser/dialogs retain input.

**Commit:** `feat(tui): add transcript text selection`

### Task 7: Remove Card Budgets And Add Complete Workflow Tool Expansion

**Files:**

- Modify `workflow_group.rs`, `workflow_card.rs`,
  `workflow_delegate_card.rs`, `workflow_swarm_card.rs`, `entry/mod.rs`,
  `tool_call.rs`, `live_output.rs`, `store.rs`, and `event_handler.rs`.
- Modify focused Workflow, tool-card, and Delegate-family tests.
- Do not edit `delegate_card.rs`, `delegate_group.rs`, `swarm_card.rs`, or
  `child_activity.rs` unless compilation proves a signature-only change is
  unavoidable.

**Why:** The document fixes outer overflow only if card renderers return their
complete approved structure and expanded tools can read the complete source.

**Change Necessity:** Workflow currently accepts `available_rows`/`max_rows` and
emits four omission forms. Direct tools have no per-row expansion path.

**Impact / Compatibility:** Only Workflow outer budgeting and direct-tool
expansion change. Workflow execution, result, journal, recovery, sibling order,
and every non-Workflow Delegate-family component remain unchanged.

**Repair Track:**

- Remove terminal-height inputs from the Workflow group, main card, Delegate
  summary, and swarm summary.
- Always render main, optional Delegate summary, and optional DelegateSwarm
  summary in that order.
- Keep direct tools one-line by default. Toggle one tool by typed tool ID and
  render its command/arguments, details, and visible complete-output range
  immediately beneath that row using `ToolCallComponent`.
- Keep the six-line/50,000-character buffer only as bounded live preview; it is
  never the complete source and completion must not erase access to the output
  reference.
- Ordinary Delegate-family cards render exactly through their existing
  components and keep `MAX_CHILD_TOOL_ROWS = 4`.

**Retirement Track:**

- Delete `available_rows`, Workflow `max_rows`, budget allocation,
  `folded_child_counts`, and the strings `direct tools omitted`, `agents
  omitted`, `child rows omitted`, and viewport `more rows`.
- Delete old bounded-history Workflow tests; do not invert them into dual-mode
  acceptance.

**Steps:**

1. Record frozen output fixtures for ordinary Delegate, DelegateGroup, collapsed
   DelegateSwarm, and expanded DelegateSwarm.
2. Remove Workflow height parameters and render all structural rows.
3. Add typed direct-tool selection and local expansion.
4. Read only the visible complete-output range through `ToolOutputStore` and
   document layout cache.
5. Cover live, completed, resumed, child-origin, missing legacy output, and
   absent/corrupt index states.
6. Scan rendered output and source for every retired omission marker.

**Verification:**

```bash
rtk cargo nextest run -p neo-tui --test workflow_transcript workflow_document_renders_every_row_without_viewport_omissions
rtk cargo nextest run -p neo-tui --test workflow_transcript workflow_direct_tool_expands_inline_and_collapses_to_one_row
rtk cargo nextest run -p neo-tui --test workflow_transcript non_workflow_delegate_family_cards_remain_unchanged
rtk cargo nextest run -p neo-tui --test multi_agent_transcript option_b_expanded_swarm_preserves_full_child_transcripts
rtk cargo nextest run -p neo-tui --test tool_cards completed_tool_expansion_reads_output_beyond_live_preview_limits
```

Expected: all Workflow structural rows are reachable, expansion reads beyond
preview limits, and ordinary Delegate-family output is unchanged.

**Commit:** `feat(tui): show complete workflow tools`

### Task 8: Complete Resume, Animation, Exit, And Static-Mode Integration

**Files:**

- Modify `neo-agent` interactive `mod.rs`, `terminal_io.rs`,
  `frame_scheduler.rs`, and `main.rs`.
- Add `modes/interactive/fullscreen_tests.rs` and
  `tests/fullscreen_output.rs`.
- Modify focused resume and transcript tests only where needed.

**Why:** The feature is incomplete if resume loses associations, off-screen
animations consume work, exit prints inside alternate screen, or non-TTY modes
emit control sequences.

**Change Necessity:** These behaviors are owned outside the TUI document and
cannot be proven by component tests alone.

**Impact / Compatibility:** Resume and exit consume the new presentation
metadata lazily, while non-interactive modes remain byte-oriented static output.
Animation keeps the existing cadence and scheduler and is limited to visible
active entries; no new user setting or scheduler is introduced.

**Required behavior:**

- Resume rebuilds `TranscriptStore` in event order and resolves optional output
  references without reading complete output files up front.
- A fresh resume starts at tail with no selection. Expansion/selection/scroll
  are UI state and are not appended to session JSONL.
- Only visible active entries request animation deadlines. Off-screen entries
  pause presentation ticks. Existing 100 ms cadence remains; no scheduler or
  decorative animation is added.
- Build the exit projection from canonical final state, then restore terminal,
  then return it to `main` for printing. Include final assistant answer, terminal
  task/Workflow status, and session reopen command only.
- On abnormal termination, prioritize restoration and emit a bounded recovery
  line with session ID when a safe projection cannot be built.
- Non-TTY root mode, `neo run`, pipe output, export, and non-TTY resume never
  construct `FullscreenTerminal` and never emit alternate-screen or mouse
  sequences.

**Steps:**

1. Rehydrate output refs and group Workflow child activity without eager output
   reads or duplicate top-level cards.
2. Base animation scheduling on the visible entry range using the existing
   scheduler and cadence.
3. Extract a pure exit projection builder and move printing after `leave`.
4. Cover interrupted assistant/tool/Workflow terminal states and safe recovery.
5. Cover static root, run, pipe, export, and resume paths for absence of terminal
   control sequences.
6. Prove unchanged provider request/cache prefix for an unchanged session and
   append-only addition for new canonical messages.

**Verification:**

```bash
rtk cargo nextest run -p neo-agent --bin neo terminal_exit_projection_prints_after_restore
rtk cargo nextest run -p neo-agent --bin neo resume_restores_output_references_without_eager_full_reads
rtk cargo nextest run -p neo-agent --bin neo offscreen_entries_do_not_schedule_animation_frames
rtk cargo nextest run -p neo-agent --test fullscreen_output static_modes_never_emit_fullscreen_sequences
rtk cargo nextest run -p neo-agent-core --test runtime_turn unchanged_session_keeps_cache_prefix_and_new_context_appends
```

Expected: final output appears after restoration, static modes contain no
fullscreen sequences, resume remains lazy, and context integrity holds.

**Commit:** `feat(agent): integrate fullscreen transcript lifecycle`

### Task 9: Run Native Acceptance And Record The Landed Decision

**Files:**

- Create `docs/aegis/adr/ADR-0012-fullscreen-transcript-document.md`.
- Create `docs/aegis/baseline/2026-08-04-fullscreen-transcript-document.md`.
- Mark `docs/aegis/adr/ADR-0010-native-terminal-transcript-presentation.md`
  superseded without rewriting its historical decision or evidence.
- Update this workstream checkpoint, evidence, proof bundle, and index.

**Why:** Deterministic tests cannot prove real terminal mouse behavior,
selection bypass, clipboard integration, or platform restoration. Architecture
records must describe only verified landed behavior.

**Change Necessity:** Documentation sync is required only after Tasks 1-8 pass;
updating it earlier would make an unimplemented design appear landed.

**Impact / Compatibility:** The new ADR and baseline become current only after
focused and native evidence exists. ADR-0010 and the 2026-07-31 baseline remain
historical records; only ADR-0010 receives a supersession marker.

**Verification:**

**Automated preflight:**

```bash
rtk cargo nextest run -p neo-agent-core --test tool_output_capture tool_output_store
rtk cargo nextest run -p neo-tui --test fullscreen_transcript logical_anchor_survives_growth_removal_resize_and_wrap
rtk cargo nextest run -p neo-tui --test transcript_selection selection_crosses_entries_autoscrolls_and_materializes_text
rtk cargo nextest run -p neo-tui --test workflow_transcript workflow_document_renders_every_row_without_viewport_omissions
rtk cargo nextest run -p neo-agent --bin neo terminal_exit_projection_prints_after_restore
rtk cargo nextest run -p neo-agent --test fullscreen_output static_modes_never_emit_fullscreen_sequences
cargo fmt --all --check
git diff --check
```

Each `nextest` command still names one package, one target selector, and one
test-name filter. Do not replace them with a workspace-wide test run.

**Native matrix:**

1. Run `vm_stat` and `prlctl list`; ensure only one VM is active before starting
   native validation.
2. On macOS, use a real terminal to verify bottom follow, locked scroll,
   streaming growth, card expansion, drag/double-click/copy, Shift-drag bypass,
   resize, Task Browser, suspend/resume, normal exit, and forced error restore.
3. In Fedora, run the six focused automated commands above from the Parallels
   shared checkout, then repeat the real-terminal mouse/clipboard/restore smoke
   in a Linux terminal. Record terminal emulator and Wayland/X11 clipboard path.
4. Stop Fedora before starting Windows. In Windows 11, run the same focused
   targets from the shared checkout and repeat Windows Terminal mouse,
   `clip.exe`, resize, error restore, and normal exit smoke.
5. Shut down the VM after use. Do not claim `prlctl exec` alone proves real
   terminal selection or alternate-screen behavior.

**Architecture closeout:**

- Mark ADR-0010 superseded by the landed single fullscreen document decision,
  preserving its original history.
- Create ADR-0012 with the landed decision, rejected alternatives, compatibility
  boundary, and retirement evidence.
- Create a new landed baseline with verified owner/file/test evidence. Keep the
  old baseline unchanged as historical evidence, and do not carry its native
  scrollback or `Ctrl+O` review behavior into the new baseline.
- Record exact local, Fedora, and Windows evidence separately. If one native
  platform is unavailable, leave closeout incomplete and state the residual
  risk rather than claiming cross-platform completion.
- Run lingering-reference and negative scans:

```bash
rg -n 'TranscriptPresentation|bound_live_blocks|TranscriptBrowserState|review_surface|stable_prefix_len|append_protected_history|automatic_overflow' crates/neo-tui crates/neo-agent
rg -n 'direct tools omitted|agents omitted|child rows omitted|more rows' crates/neo-tui/src/transcript
```

Expected: both scans return no main-path references; any fixture-only match is
reviewed and documented, not silently ignored.

**Commit:** `docs(tui): record fullscreen transcript architecture`

## Verification Matrix

| Approved behavior | Primary proof |
|---|---|
| Assistant grows without disappearing | Task 4 document test plus native streaming smoke |
| Bottom follow and upward lock | Task 4 anchor/activity tests |
| Dynamic height and resize stability | Task 4 reflow test |
| Bash/Terminal complete output | Tasks 1-2 cap/saturation tests |
| Main/child/Workflow persistence | Task 3 JSONL and replay tests |
| Delegate-family card freeze | Task 7 ordinary card parity tests |
| Workflow fixed sibling order | Task 7 Workflow document test |
| Workflow direct-tool expansion | Task 7 expansion test |
| Cross-card selection and copy | Task 6 coordinate/materialization tests plus native smoke |
| One fullscreen lifecycle | Task 5 control-sequence tests plus native restore smoke |
| Visible-only balanced animation | Task 8 scheduler test |
| Exit projection after restore | Task 8 terminal ordering test |
| Static modes unchanged | Task 8 integration test |
| Context/cache prefix unchanged | Task 3 and Task 8 request-projection tests |
| Old path actually removed | Task 9 lingering/negative scans |
| macOS/Linux/Windows | Task 9 separated native evidence |

## Risks And Stop Conditions

- Stop if complete output cannot be opened before launch or a midstream append
  error cannot reliably stop the corresponding process. Do not silently fall
  back to preview output.
- Stop if any proposed event field would enter provider-visible messages,
  compaction input, or cache prefix. Move metadata to the presentation event
  path instead.
- Stop if child output ownership would require inferring from text, result JSON,
  timing, or unrelated IDs.
- Stop if implementation requires editing Delegate-family component rendering;
  return to the approved design boundary with exact evidence.
- Stop if the document frame itself exceeds terminal height. Large blocks belong
  to virtual document geometry, never the physical frame.
- Stop if selection adds a second row map outside `DocumentLayout`.
- Stop if terminal failure advances the differential baseline before a
  successful write or prevents guaranteed restoration.
- Stop if any implementation task overlaps unresolved theme-manager edits in a
  way that cannot be handled with a narrow targeted change; never restore user
  work.
- Native clipboard, Shift bypass, and alternate-screen behavior remain residual
  risk until tested in a real terminal on the named platform.

## Plan Pressure Test

- Owner and retirement: one new layout owner and one output owner replace three
  obsolete interactive owners; no compatibility renderer remains.
- Higher-level path: the plan fixes the document and source capture before card
  symptoms, so sibling cards and assistant streaming share one repair.
- Verification: focused tests cover every boundary; native claims are separated
  from deterministic tests.
- Executability: each task names files, required types/behavior, dependency
  order, exact filtered commands, expected result, and one scoped commit.
- Pressure result: proceed after explicit runtime-code authorization.

## Execution Readiness View

- Intent Lock: one application-owned fullscreen transcript with no information
  loss from viewport pressure.
- Scope Fence: output capture, document layout, fullscreen lifecycle,
  selection, Workflow outer rendering, resume/exit/static integration, and
  architecture closeout only.
- Baseline Lock: approved fullscreen spec is current; ADR-0010 and the old
  baseline remain historical inputs until verified supersession.
- Approved Behavior: tail follow; logical lock plus one activity indicator;
  complete reachable assistant/cards/output; cross-card copy; balanced visible
  animation; restored terminal plus bounded exit projection.
- Owner Constraints: frozen Delegate-family components; Workflow runtime and
  model context unchanged; one output/document/scroll/renderer owner.
- Compatibility Boundary: optional old-session metadata, static modes, Task
  Browser, retry safety, context/cache prefix, and three platforms.
- Retirement Boundary: delete history/live, stable prefix, automatic overflow,
  protected history, review browser, duplicate scroll state, and card budgets.
- Task Batches: Tasks 1-3 output/persistence; Tasks 4-6 document/lifecycle/input;
  Tasks 7-8 cards/integration; Task 9 native closeout.
- Test Obligations: exact filtered package/target tests per task, negative scans,
  and separate real-terminal evidence.
- Review Gates: two-stage code review after Tasks 3, 6, and 8; architecture
  review before Task 9 closeout.
- Drift Rules: any second renderer/store/fallback, card-body change,
  model-context change, or silent capture loss returns to the approved spec.
- Evidence Required Before Completion: scoped commits, focused command output,
  lingering-reference scans, write-failure proof, request-prefix proof, and
  macOS/Fedora/Windows evidence.
- Advisory Boundary: this plan guides implementation; it does not grant runtime
  authorization or completion authority.

## Execution Route

- Decision: subagent-driven after runtime authorization.
- Evidence: Tasks 1-3, 4-6, and 7-8 contain bounded domains, while review gates
  protect the shared event/document boundaries.
- Fallback: execute inline in dependency order if isolated subagent ownership is
  unavailable or overlaps unresolved user changes.
- User confirmation required: yes; the current request authorizes planning and
  plan documentation only, not runtime source implementation.
