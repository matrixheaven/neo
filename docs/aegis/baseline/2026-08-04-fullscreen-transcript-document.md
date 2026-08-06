# Fullscreen Transcript Document — Landed Baseline

Status: `recorded-from-work`
Date: `2026-08-04`
ADR: `docs/aegis/adr/ADR-0012-fullscreen-transcript-document.md`
Supersedes: `docs/aegis/baseline/2026-07-31-native-terminal-transcript.md`
(kept unchanged as historical evidence)

This baseline records the landed fullscreen transcript document
implementation, in task order:

- `87c53612` — `feat(core): add complete tool output store`
  (`ToolOutputStore` owns `agents/<agent_id>/tasks/<task_id>.log` plus a
  rebuildable sparse `.log.idx`; strict ID/path validation; append-only,
  no configured truncation; monotonic byte/line metadata; bounded range reads
  with one-line look-ahead; streaming index rebuild with atomic replacement;
  failures returned to the caller);
- `99ac82ba` — `feat(core): capture complete shell output`
  (Bash `spawn_output_drain` and Terminal `spawn_terminal_reader` append every
  decoded chunk before the bounded preview queue, result caps, and ring
  buffers; capture opens before launch; append failure stops the process and
  reports possible partial side effects; stdout/stderr appends serialized per
  task; non-agent shell uses uncaptured);
- `dc7b10c4` — `feat(core): persist tool output references`
  (optional typed `ToolOutputRef` on `ToolExecutionStarted/Update/Finished`,
  `ShellCommandFinished`, and Terminal session events; child wire.jsonl,
  `AgentActivityKind::Tool`, `DelegateToolProgress`, swarm progress; one
  Terminal reference across start/read/write/resize/stop; Workflow direct tools
  use the main-agent reference, Delegate-family children their own; TUI
  rehydration by typed identity; old JSONL deserializes with `None`;
  `display_output_never_enters_model_context` proves no leak into
  request-visible context);
- `8506ae40` — `feat(tui): add fullscreen transcript document`
  (`DocumentLayout`/`TranscriptAnchor`/`DocumentViewport` in
  `transcript/document.rs`: per-entry revision/height/start invalidation, tail
  follow, locked scroll with one Boolean new-activity indicator, retry-removal
  fallback staying locked, resize/reflow anchor resolution, bounded visible
  composition, per-entry render cache reuse, background DelegateGroup
  off-screen updates stay reachable);
- `73d70a5c` — `refactor(tui): use one fullscreen transcript`
  (deleted `presentation.rs`, `browser.rs`, `streaming_prefix.rs`;
  `InlineTerminal` → `FullscreenTerminal` with no alias; `TerminalFrame`
  bounded to frame lines + cursor + animation deadline; one enter/leave
  alternate-screen + mouse lifecycle with restoration on leave/error/panic/
  suspend; `Ctrl+O` routes to the primary-document tool toggle; Task Browser
  overlay without a physical transition);
- `350b3a42` — `feat(tui): add transcript text selection`
  (typed `MouseEvent`/`MouseKind`/`DocumentPoint` SGR parsing with
  one-based→zero-based conversion once; cross-entry drag with edge auto-scroll
  on the existing frame cadence; double-click Unicode word selection; Shift
  bypass; materialization at release; endpoint clamp + anchor fallback;
  clipboard failure preserves selection; dialog/Task Browser input priority);
- `2962e90f` — `feat(tui): show complete workflow tools`
  (Workflow height budgets and omission strings deleted; fixed
  main/Delegate/DelegateSwarm sibling order with every structural row;
  direct-tool one-line default with typed inline expansion reading the visible
  complete-output range via `ToolOutputStore`; honest incomplete states for
  missing legacy output and absent/corrupt indexes; frozen Delegate-family
  fixture tests);
- `f42cfd7d` — `feat(agent): integrate fullscreen transcript lifecycle`
  (lazy resume with metadata-only output-ref resolution and typed Workflow
  regrouping; visible-only animation scheduling on the existing 100 ms
  cadence; pure exit-projection builder printed after terminal restoration;
  bounded abnormal-termination recovery line; static modes never emit
  fullscreen sequences; unchanged-session cache-prefix proof);
- `47f447a6` — `refactor(tui): remove legacy transcript viewport`
  (dead absolute-row viewport and its tests removed after the browser
  retirement);
- `aeda4030` — `fix(agent): resume suspends through the single stdin reader`
  (native-validation fix: the resume cursor probe raced the background stdin
  reader; the probe now reads the CPR through the app's single-reader channel,
  with parked-origin fallback on probe failure).

## Compatibility Boundary

- Existing session JSONL remains append-only and readable; new output
  metadata is optional and old records deserialize with `None`.
- Missing legacy output is explicitly incomplete, never relabeled complete.
- `ToolResult`, canonical messages, provider requests, compaction input, and
  cache-prefix bytes never contain display-output files, paths, or references.
- Micro compaction and snip/dedup remain independently opt-in and default off.
- Workflow execution, scheduling, journal, recovery, results, and model-visible
  output are unchanged.
- Non-workflow Delegate-family card component output is byte-identical for
  identical snapshots and expansion state (`MAX_CHILD_TOOL_ROWS = 4` kept).
- Task Browser remains an overlay inside the fullscreen session.
- Print, pipe, export, `neo run`, and non-TTY resume remain static.
- Shell admission waits, explicit timeout/cancel, background execution, and
  Terminal-yield timing are unchanged.
- Paths use `Path`/`PathBuf`; suspend is `cfg(unix)` with a portable default.

## Current Owners

- `TranscriptStore` — sole typed transcript content and ordering owner.
- `DocumentLayout` (transcript/document.rs) — sole layout and scroll owner.
- `ToolOutputStore` (neo-agent-core session/tool_output.rs) — sole complete
  display-output owner per task.
- `FullscreenTerminal` (screen_output/fullscreen_terminal.rs) — sole physical
  fullscreen owner.
- `LiveRenderer` + `FrameScheduler` — sole differential renderer and frame
  scheduler.

## Retired Owners

`TranscriptPresentation`, history/live partition, `bound_live_blocks`,
acknowledgement ledger, live budget, `TranscriptTerminalUpdate`,
`FinalizedBlock`, `stable_prefix_len`, protected history insertion, flattened
history, `TranscriptBrowserState`, review-screen state, `streaming_prefix.rs`,
`browser.rs`, the `Ctrl+O` review surface, viewport omission markers, Workflow
`available_rows`/`max_rows`/`folded_child_counts`, omission strings, and the
legacy `TranscriptViewport`. Zero production references remain.

## Native Evidence Summary

- macOS host: fullscreen enter/leave balanced once, mouse enable/disable
  balanced, wheel/drag/resize accepted, Ctrl+C double-press exit, Ctrl+Z
  suspend restores then re-enters on SIGCONT, exit projection after
  restoration, bounded recovery line on abnormal resume. Automated matrix
  11/11.
- Fedora 43 aarch64 VM: automated matrix 11/11 after `aeda4030`; the six
  focused test commands pass as a non-root user.
- Windows 11 VM: the six focused test commands plus `fullscreen_terminal`
  (3), `terminal_frame` (11), `tool_cards` (65), `fullscreen_transcript` (5),
  and `transcript_selection` (5) all pass natively; the interactive binary
  correctly stays static when std handles are not console handles; suspend
  skipped by design.
- Residual: real-terminal mouse hardware, clipboard side effects, and Shift
  bypass need graphical-terminal confirmation (Windows interactive smoke
  requires a logged-in desktop session).

This is an advisory Aegis Method Pack record. It records the landed
architecture state and does not grant completion authority.
