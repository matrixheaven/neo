# ADR-0010 - Native Terminal Transcript Presentation

Status: `recorded-from-work`
Date: `2026-08-01`

## Source Evidence

- Approved design: `docs/aegis/specs/2026-07-30-native-scrollback-progressive-transcript-design.md`
  (commit `be17c322`).
- Implementation plan: `docs/aegis/plans/2026-07-31-native-scrollback-progressive-transcript.md`
  (commit `32467767`).
- Retained geometry baseline: `docs/aegis/specs/2026-07-19-terminal-live-viewport-isolation-design.md`.
- Superseded automatic-overflow design:
  `docs/aegis/specs/2026-07-19-transcript-overflow-tool-results-design.md`.
- Landed baseline: `docs/aegis/baseline/2026-07-31-native-terminal-transcript.md`.

## Context

The previous transcript presentation treated the earliest mutable entry as a
global commit barrier, so a long-running Delegate, DelegateGroup, DelegateSwarm,
workflow, tool, shell, or model attempt retained every later row in one mutable
suffix. When that suffix exceeded the rows above chrome, `NeoTui` automatically
entered the alternate screen, captured the mouse, rendered an application
viewport, and appended Todo, composer, and footer as fixed chrome. That hid the
shell launch line and native history, disabled terminal scrolling and text
selection, made chrome look like a persistent dock, and let later background
updates displace a pending approval that still owned input.

## Decision

Ordinary Neo conversation always stays on the terminal's normal screen and
native scrollback. The terminal owns wheel scrolling, text selection, and the
shell launch line; Todo, composer, and footer scroll away with the rest of the
viewport. The only ordinary-conversation path into an application-owned
alternate screen is explicit `Ctrl+O` review; Task Browser keeps its existing
explicit surface. Automatic alternate-screen entry, the overflow latch, fixed
chrome, automatic mouse capture, and wheel/page routing into an application
viewport are deleted without a compatibility branch.

The transcript presentation model is:

- `TranscriptStore` remains the typed source and canonical entry-order owner,
  and now captures immutable progressive facts at update time (before source
  snapshots trim or replace them).
- `TranscriptPresentation` remains the only history-versus-live owner, with a
  typed fact acknowledgement ledger, a bounded live area actually capped by
  `live_budget`, and two commit rules: ordinary mutable entries never block
  unrelated stable facts, and the earliest unresolved approval or question
  defers later history until it resolves.
- `InlineTerminal` remains the sole physical normal-screen owner with
  transactional writes, failure rollback, and post-success acknowledgement.
- `progressive.rs` holds only typed fact identities and pure projection
  helpers; it is not a second store or renderer.

Stability is proven exclusively by typed identity and typed finality
(`(entry, agent id, run_count, tool id)` plus `Done`/`Failed` for child tools;
entry plus agent/run identity plus terminal lifecycle state for agents and
swarm items; entry plus accepted `projection_sequence` for workflow
transitions; typed dialog id plus resolved/abandoned/answered/cancelled state
for blocking dialogs). Rendered ANSI, human-readable text, row position, and
vector indexes are never used to infer identity or finality. Sources without an
append-only proof (assistant attempts, thinking, ordinary tool and shell live
output, compaction, retry status, connecting MCP startup) stay bounded live and
commit their canonical finalized form exactly once.

Every approval and every question has exactly one visible owner: the
transcript card. The runtime approval modal and `QuestionStateMachine` remain
the input and selection owners; the question card borrows the state machine's
display state and is synced after each input. Keys route to the earliest
unresolved blocking entry by transcript order.

## Consequences

- Tall yolo and ask sessions never emit an automatic alternate-screen enter
  sequence and never capture the mouse automatically.
- Stable facts enter native scrollback exactly once; completion appends
  remaining facts plus one terminal status and never repeats a complete card.
- A pending approval or question stays visible and operable while later
  Delegate, workflow, and model events arrive.
- `Ctrl+O` review still enters one balanced alternate-screen transition and
  returns to unchanged primary scrollback.
- The dead `QueuedMessage` transcript path and the queued-approval badge are
  deleted; composer input preview remains the queued-message owner.
- The superseded automatic-overflow requirements in the 2026-07-19 overflow
  design are no longer authoritative.

## Retirement

Deleted in this work: `NeoTui::automatic_overflow` and its latch/release,
automatic viewport rendering, automatic fixed chrome, automatic mouse capture,
`handle_automatic_overflow_event`, `TranscriptTerminalUpdate::live_overflow`
and `has_live_frontier`, `render_viewport_rows`/`viewport_splits_terminal_image`
(no explicit-review caller remained), the pane-level queued-approval queue and
badge, `TranscriptEntry::QueuedMessage`, and question rendering in fixed
chrome. No feature flag, fallback, alias, or permission-mode branch preserves
any of them.
