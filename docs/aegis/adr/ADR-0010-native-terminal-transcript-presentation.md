# ADR-0010 - Native Terminal Transcript Presentation

Status: `amended`
Date: `2026-08-01`

## Source Evidence

- Approved design: `docs/aegis/specs/2026-07-30-native-scrollback-progressive-transcript-design.md`
  (commit `be17c322`).
- Implementation plan: `docs/aegis/plans/2026-07-31-native-scrollback-progressive-transcript.md`
  (commit `32467767`).
- Approved workflow amendment:
  `docs/aegis/specs/2026-08-01-workflow-dynamic-transcript-design.md`.
- Workflow amendment plan:
  `docs/aegis/plans/2026-08-01-workflow-dynamic-transcript.md`.
- Landed workflow amendment evidence available at this decision update:
  `29fa1cfc` (`fix(core): preserve live workflow child origin`) and
  `39d6b40d` (`fix(tui): group typed workflow activity`).
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
swarm items; workflow run identity plus terminal lifecycle state for a workflow
group; typed dialog id plus resolved/abandoned/answered/cancelled state for
blocking dialogs). Rendered ANSI, human-readable text, row position, and vector
indexes are never used to infer identity or finality. Workflow
`projection_sequence` remains only a stale-update watermark and is not a
history identity. Sources without an append-only proof (assistant attempts,
thinking, ordinary tool and shell live output, compaction, retry status,
connecting MCP startup) stay bounded live and commit their canonical finalized
form exactly once.

Workflow presentation is one logical group in fixed sibling order: one main
card, one workflow Delegate summary when present, and one workflow
DelegateSwarm summary when present. Direct workflow-origin tool activity stays
inside the main card. Each visible child occupies one summary row; the existing
full Delegate-family cards are not nested. Typed workflow origin is carried
only through live runtime and transcript state and is not added to persisted
session JSON.

Queued, running, phase, log, report, paused, and waiting workflow snapshots
update that same live group in place and never enter native history. Completion,
failure, cancellation, or interruption submits the logical group once at the
terminal event position. The main card and its optional sibling summaries are
acknowledged together, so no non-terminal transition or duplicate final group
can be replayed.

Normal-frame composition reserves the real bottom-region cost before fitting
live transcript blocks. The cost includes wrapped content rows and each
`separator_before`; Todo, pending input, composer, footer, borders, gutters,
and cursor ownership are measured with the same effective width used to render
them. The assembled frame must fit before it reaches `LiveRenderer`.
Post-`append_chrome` tail truncation is not a valid repair path.

Every approval and every question has exactly one visible owner: the
transcript card. The runtime approval modal and `QuestionStateMachine` remain
the input and selection owners; the question card borrows the state machine's
display state and is synced after each input. Keys route to the earliest
unresolved blocking entry by transcript order.

## Alternatives Considered

- Keep workflow-origin tools as ordinary top-level cards and add an origin
  label: rejected because it preserves one card per call and leaves the
  workflow state fragmented.
- Nest the existing full Delegate-family cards inside the workflow card:
  rejected because it duplicates headers, expansion state, and activity bodies
  while increasing frame pressure.
- Add a compatibility renderer or a second workflow presentation owner:
  rejected because it creates two state paths with no persistence or user-data
  requirement.
- Use one main card plus two optional sibling summaries: selected because it
  keeps one typed state owner, bounds the normal screen, and leaves existing
  non-workflow cards unchanged.

## Compatibility Boundary

- Workflow execution, scheduling, journal, recovery, result, model-visible
  output, and persisted session JSON remain unchanged.
- Non-workflow Delegate, DelegateGroup, and DelegateSwarm card layout,
  expansion, ordering, activity rows, progressive history, and transcript
  placement remain unchanged.
- Approval and question entries remain independent blocking input owners.
- Ordinary normal-screen rendering, native scrollback, explicit `Ctrl+O`
  review, Task Browser, terminal writes, and acknowledgement ordering remain
  unchanged.
- Existing persisted sessions are read as recorded; no migration or historical
  transcript rewrite is introduced.

## Consequences

- Tall yolo and ask sessions never emit an automatic alternate-screen enter
  sequence and never capture the mouse automatically.
- Stable facts enter native scrollback exactly once; completion appends
  remaining facts plus one terminal status and never repeats a complete card.
- A workflow remains one bounded live logical group until terminalization, then
  that group enters history exactly once. Mutable workflow transitions never
  become history facts.
- Workflow-origin direct tools and Delegate-family children remain visibly
  associated with the workflow without creating isolated top-level cards.
- Ordinary progress preserves Todo, composer, footer, and cursor because final
  frame cost is calculated before composition. A blocking approval or question
  keeps its existing independent input ownership and intentional composer
  hiding.
- A pending approval or question stays visible and operable while later
  Delegate, workflow, and model events arrive.
- `Ctrl+O` review still enters one balanced alternate-screen transition and
  returns to unchanged primary scrollback.
- The dead `QueuedMessage` transcript path and the queued-approval badge are
  deleted; composer input preview remains the queued-message owner.
- The superseded automatic-overflow requirements in the 2026-07-19 overflow
  design are no longer authoritative.
- Workflow execution, scheduling, journal, recovery, result, and persistence
  remain unchanged. Non-workflow Delegate, DelegateGroup, and DelegateSwarm
  card layout, expansion, ordering, activity rows, and transcript placement
  remain unchanged. Ordinary normal-screen and native-scrollback ownership
  also remain unchanged.

## Retirement Impact

Deleted in this work: `NeoTui::automatic_overflow` and its latch/release,
automatic viewport rendering, automatic fixed chrome, automatic mouse capture,
`handle_automatic_overflow_event`, `TranscriptTerminalUpdate::live_overflow`
and `has_live_frontier`, `render_viewport_rows`/`viewport_splits_terminal_image`
(no explicit-review caller remained), the pane-level queued-approval queue and
badge, `TranscriptEntry::QueuedMessage`, and question rendering in fixed
chrome. No feature flag, fallback, alias, or permission-mode branch preserves
any of them.

The workflow amendment additionally retires `WorkflowTransition` history,
`WorkflowTransitionFact`, `ProgressiveFactId::WorkflowTransition`,
`capture_workflow_transition`, isolated top-level cards for workflow-origin
tools and Delegate-family children, and normal-path tail truncation after
`append_chrome`. No compatibility renderer, second workflow state owner,
second transcript store, origin side map, or persistence migration replaces
them.

## Baseline Sync

`docs/aegis/baseline/2026-07-31-native-terminal-transcript.md` is amended with
the same current-state rule: accepted non-terminal workflow transitions are no
longer history; one dynamic workflow group enters history once at terminal
state. The baseline also records the preserved runtime, persistence,
non-workflow card, blocking-input, normal-screen, and native-scrollback
boundaries.

## Evidence References

- Approved workflow design:
  `docs/aegis/specs/2026-08-01-workflow-dynamic-transcript-design.md`.
- Implementation plan:
  `docs/aegis/plans/2026-08-01-workflow-dynamic-transcript.md`.
- Current workflow implementation evidence: commits `29fa1cfc` and
  `39d6b40d`.
- Current-state baseline:
  `docs/aegis/baseline/2026-07-31-native-terminal-transcript.md`.

This is an advisory Aegis Method Pack record. It documents the architecture
decision and does not grant completion authority; implementation and platform
claims require their own fresh evidence.
