# ADR-0012 - Fullscreen Transcript Document

Status: `accepted`
Date: `2026-08-04`

## Source Evidence

- Approved design: `docs/aegis/specs/2026-08-04-fullscreen-transcript-document-design.md`
  (commit `43cdfd3e`).
- Implementation plan: `docs/aegis/plans/2026-08-04-fullscreen-transcript-document.md`
  (commit `b62859d8`).
- Implementation authorization handoff:
  `docs/aegis/handoffs/2026-08-04-fullscreen-transcript-document.md`
  (commit `c641c6fd`).
- Landed implementation commits, in task order:
  - `87c53612` — `feat(core): add complete tool output store`;
  - `99ac82ba` — `feat(core): capture complete shell output`;
  - `dc7b10c4` — `feat(core): persist tool output references`;
  - `8506ae40` — `feat(tui): add fullscreen transcript document`;
  - `73d70a5c` — `refactor(tui): use one fullscreen transcript`;
  - `350b3a42` — `feat(tui): add transcript text selection`;
  - `2962e90f` — `feat(tui): show complete workflow tools`;
  - `f42cfd7d` — `feat(agent): integrate fullscreen transcript lifecycle`;
  - `47f447a6` — `refactor(tui): remove legacy transcript viewport`;
  - `aeda4030` — `fix(agent): resume suspends through the single stdin reader`
    (native-validation defect: the resume cursor probe raced the background
    stdin reader; the probe now uses the app's single-reader channel).
- Landed baseline: `docs/aegis/baseline/2026-08-04-fullscreen-transcript-document.md`.

## Context

The previous presentation direction (ADR-0010, native normal-screen scrollback
with a history/live split and an explicit `Ctrl+O` review browser) could not
keep every presentation input reachable: the live suffix was bounded by
terminal height, `bound_live_blocks` and `ToolCallComponent` discarded older
live rows, and Bash/Terminal capped output before the TUI ever saw it. The
normal terminal screen cannot rewrite arbitrarily tall mutable rows in place.

## Decision

Interactive Neo uses exactly one application-owned fullscreen transcript
document from startup until exit. `TranscriptStore` remains the sole typed
content owner; `DocumentLayout` (transcript/document.rs) owns per-entry
rendered height, virtual start rows, and one logical `TranscriptAnchor`;
`FullscreenTerminal` (renamed from `InlineTerminal`) owns the single
alternate-screen + mouse lifecycle entered once at startup and restored on
normal exit, error, panic, and supported suspend paths; `LiveRenderer` remains
the only differential writer with frames bounded to terminal height.

Complete display output is captured source-side before every bounded sink:
each agent-originated Bash/Terminal execution appends decoded text to
`agents/<agent_id>/tasks/<task_id>.log` before the preview queue, result caps,
and ring buffers, through `ToolOutputStore` (session/tool_output.rs) with a
rebuildable sparse `.log.idx` index and bounded range reads. A typed optional
`ToolOutputRef` travels through events, child wire.jsonl, `DelegateToolProgress`,
and Workflow grouping, and is rehydrated into the original card without
entering `ToolResult`, provider requests, compaction input, or cache-prefix
bytes. Missing legacy output is explicitly incomplete, never relabeled
complete.

Workflow renders its fixed main/Delegate/DelegateSwarm sibling order with every
structural row and expands a direct tool inline beneath its one-line summary
via `ToolCallComponent`, reading only the visible complete-output range.
Ordinary Delegate-family card components are unchanged.

Selection uses document coordinates (`entry_id`, `row_in_entry`, `display_cell`)
with typed SGR mouse parsing, cross-entry drag, edge auto-scroll on the
existing frame cadence, double-click Unicode word selection, Shift-drag bypass,
and materialized plain-text clipboard copy that survives clipboard failure.
`Ctrl+O` toggles the selected tool inside the primary document; Task Browser
remains an overlay inside the already-fullscreen session.

Animation keeps the existing 100 ms `FrameScheduler` cadence and schedules
frames only for visible active entries. Exit restores the terminal first and
then prints a bounded static projection (final assistant answer, terminal
task/workflow status, session reopen hint). Print, pipe, export, `neo run`, and
non-TTY resume never construct the fullscreen terminal.

## Alternatives Considered

- Keep inline mode and enter fullscreen only on overflow: rejected because it
  preserves two physical modes, two scroll owners, migration at the worst
  possible time, and cannot recover output discarded upstream.
- Reuse the existing `Ctrl+O` browser unchanged: rejected because it clones the
  pane per frame, uses absolute row offsets that drift under growth, has entry
  selection rather than cross-card text selection, and does not solve
  source-side output loss.
- Store complete output only in session JSONL: rejected because existing
  command updates are not lossless and lossless events would force every resume
  to parse unbounded output.
- Port the grok-build rendering stack: rejected because Neo already has the
  typed entries, layout caches, lifecycle, scheduler, and differential
  renderer; the reference's private dependencies and extra modes add code
  without changing the essential document model.

## Compatibility Boundary

- Existing session JSONL remains append-only and readable; new output-reference
  metadata is optional (`serde(default)`) and old records deserialize with
  `None`.
- Model-visible `ToolResult`, canonical messages, provider requests, compaction
  input, and cache prefixes never contain display-output files, paths, or
  references (proven by `display_output_never_enters_model_context` and
  `unchanged_session_keeps_cache_prefix_and_new_context_appends`).
- Micro compaction and snip/dedup remain independently opt-in and default off.
- Workflow execution, scheduling, journal, recovery, and model-visible output
  are unchanged.
- Non-workflow Delegate-family card output is byte-identical for identical
  snapshots and expansion state.
- Print, pipe, export, `neo run`, and non-TTY resume remain static.

## Retirement Impact

Deleted without aliases or fallback branches: `TranscriptPresentation`,
history/live partition, `bound_live_blocks`, acknowledgement ledger, live
budget, `TranscriptTerminalUpdate`, `FinalizedBlock`, `stable_prefix_len`,
protected history insertion, flattened history, `TranscriptBrowserState`,
review-screen state, `streaming_prefix.rs`, `browser.rs`, the old
`Ctrl+O` review surface, viewport omission markers, Workflow
`available_rows`/`max_rows`/`folded_child_counts` and the omission strings
`direct tools omitted`, `agents omitted`, `child rows omitted`, `more rows`,
and the legacy `TranscriptViewport`. No existing session file or output
artifact was deleted or rewritten.

## Native Evidence

- macOS (host, real PTY, terminal emulation): one balanced
  `\x1b[?1049h`/`\x1b[?1049l` pair and balanced mouse enable/disable at
  startup/exit; wheel/press/drag/release SGR input accepted; resize without
  crash and no-write-on-unchanged; Ctrl+C double-press (500 ms window) exits;
  Ctrl+Z suspend restores the terminal before the process stops and re-enters
  fullscreen on SIGCONT; exit projection printed after restoration; abnormal
  resume (DSR timeout) emits the bounded recovery line. Automated matrix 11/11.
- Fedora 43 aarch64 (Parallels VM, real PTY): same automated matrix 11/11,
  including suspend/resume after the `aeda4030` single-reader probe fix; the
  six focused `cargo nextest` commands pass as a non-root user.
- Windows 11 (Parallels VM): the six focused `cargo nextest` commands pass
  natively plus `fullscreen_terminal` (3), `terminal_frame` (11), `tool_cards`
  (65), `fullscreen_transcript` (5), `transcript_selection` (5), and `cargo
  fmt --all --check`; the interactive binary correctly stays in static mode
  when its std handles are not console handles (ConPTY pipe handles fail
  `GetConsoleMode`, so `is_terminal()` is false and no fullscreen sequences are
  emitted). Suspend is not supported on Windows by design.
- Real-terminal mouse hardware events, system clipboard side effects, and
  Shift-drag bypass in a graphical terminal emulator remain residual risk;
  deterministic tests cover the selection/word/materialization logic. A
  graphical Windows Terminal interactive smoke (mouse, `clip.exe`, resize,
  error restore) requires a logged-in desktop session and was not runnable
  over the VM's ssh-only access; it remains residual risk.

## Follow-up Repair (2026-08-06)

Six follow-up fixes landed in the same architecture (plan
`docs/aegis/plans/2026-08-06-fullscreen-transcript-follow-up-repair.md`,
commits `b89c80ec`, `bac11068`, `f563df9a`, `706d9021`, `53726f62`,
`74a5093b`): consecutive `ToolRun` groups re-measure on any span change; mouse
drag gestures survive the input queue and SGR no-button motion (35) is never a
release; the document-coordinate selection is painted on visible rows and
stays routable while approvals/questions own the keyboard, with a visible copy
hint; the earliest unresolved blocking entry owns the visible window (action
area shown, later parallel `Preparing` deferred until it resolves, card-internal
scrolling for tall cards); locked views render a `new activity · end to follow`
notice that disappears on returning to the tail; and rich dialogs slice
themselves to the actual terminal height instead of losing their top rows.

Verified 2026-08-06: all 12 exact per-task regressions plus `cargo fmt
--all --check` on macOS host, Fedora 43 aarch64 VM, and Windows 11 aarch64 VM;
real-PTY lifecycle smokes on macOS and Fedora (balanced alternate-screen and
mouse capture enter/restore, SGR press/drag/release/wheel accepted, exit
projection after restore, no native-history writes) and the copy-path
behavior of Ctrl+C over a live selection on Fedora. Graphical-terminal mouse
hardware, system clipboard side effects, and Shift-drag bypass still require a
human in a graphical terminal (macOS Terminal/iTerm2 and a Windows Terminal
logged-in desktop session); they remain residual risk as in the original
landing. Full evidence record in the landed baseline.

## Consequences

- Neo owns one fullscreen transcript document and one terminal lifecycle.
- Complete Bash and Terminal display output remains outside model context while
  staying recoverable for transcript rendering.
- Old sessions remain readable through optional output references.
- Graphical-terminal mouse and clipboard side effects remain explicit residual
  native risk rather than being inferred from non-interactive automation.

## Evidence References

- `docs/aegis/baseline/2026-08-04-fullscreen-transcript-document.md`
- `docs/aegis/plans/2026-08-06-fullscreen-transcript-follow-up-repair.md`
- `docs/aegis/handoffs/2026-08-06-fullscreen-transcript-follow-up-repair.md`
- Landed implementation and repair commits listed in Source Evidence and Follow-up Repair

## Baseline Sync

- New landed baseline: `docs/aegis/baseline/2026-08-04-fullscreen-transcript-document.md`.
- ADR-0010 is marked superseded with its historical content preserved.
- The `2026-07-31-native-terminal-transcript.md` baseline remains unchanged as
  historical evidence.

This is an advisory Aegis Method Pack record. It documents the architecture
decision and does not grant completion authority; implementation and platform
claims require their own fresh evidence.
