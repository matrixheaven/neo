# Neo Compact Progress Display Design

Date: `2026-08-03`
Status: `design direction approved`

## 1. Aegis Visibility

This design separates compact's runtime progress events from the TUI's derived
visual estimate. That boundary prevents a cosmetic animation from pretending to
be runtime evidence, avoids changing context/session records, and keeps the
implementation small enough to validate with focused tests.

## 2. TaskIntentDraft

- Outcome: make manual and automatic compact progress feel continuous, honest,
  and visually stable.
- Goal: remove the visible `15% -> 85%` jump and the filled-bar shimmer that
  looks like forward/backward motion.
- Success evidence:
  - visible progress never decreases;
  - the long summarizing phase advances slowly toward a bounded estimate;
  - no per-frame glyph or color flicker;
  - completion remains authoritative at `CompactionApplied`.
- Stop condition: the TUI displays a stable monotonic phase estimate and the
  focused tests cover event jumps, stale events, animation frames, and terminal
  completion.
- Non-goals:
  - changing compaction content, selection boundaries, or context semantics;
  - changing provider/model summary generation;
  - inventing a token-level summary completion metric without a known total;
  - changing the JSONL event schema in the first implementation;
  - reusing the swarm progress estimator.
- Risks:
  - a time-based estimate can look stalled near its cap during a very long
    summary;
  - a terminal that is too narrow cannot show the full two-line presentation;
  - changing persisted progress semantics would create unnecessary replay and
    compatibility work.

## 3. BaselineReadSetHint

Required baseline surfaces already inspected:

- `crates/neo-agent-core/src/compaction/summary.rs`:
  `run_full_compaction` emits the current phase/percent anchors.
- `crates/neo-agent-core/src/events.rs`:
  `AgentEvent::CompactionProgress` and `CompactionPhase`.
- `crates/neo-tui/src/transcript/event_handler.rs`:
  `apply_compaction_event` maps runtime events into the transcript.
- `crates/neo-tui/src/transcript/pane.rs`:
  `upsert_compaction`, `update_compaction_progress`, live animation ticks.
- `crates/neo-tui/src/transcript/entry/render_status.rs`:
  `render_compaction` and `compaction_pulse_char`.
- `crates/neo-tui/src/transcript/entry/mod.rs`:
  compaction finalization, animation, and render-cache behavior.
- `docs/aegis/BASELINE-GOVERNANCE.md` and `docs/aegis/INDEX.md`.

The existing compact recovery brief is unrelated to progress rendering and does
not change this design's runtime boundary.

## 4. BaselineUsageDraft

- Required baseline refs: runtime compaction event flow, TUI compaction
  projection/rendering, project context and append-only rules.
- Delivered context refs: current source from CodeGraph and codebase-memory
  queries for `run_full_compaction`, `apply_compaction_event`,
  `upsert_compaction`, `update_compaction_progress`, and `render_compaction`.
- Acknowledged before plan refs: `AGENTS.md`, `CX.md`, `RTK.md`,
  `docs/aegis/BASELINE-GOVERNANCE.md`.
- Cited in design refs: the source files listed in Section 3.
- Missing refs: no authoritative product spec for compact progress wording;
  this brief records the user-confirmed target behavior.
- Decision: `continue`.

## 5. Requirement Ready Check

- Requirement source refs: user report and follow-up choice of the hybrid
  progress semantic.
- Goals and scope refs: Sections 1-2 and Section 7.
- User/scenario refs: an operator watching a long compact in the interactive TUI.
- Requirement item refs: Section 7.
- Acceptance/verification refs: Section 12.
- Open blocker questions: none for the design direction.
- Decision: `ready`.

## 6. ImpactStatementDraft

- Affected layers:
  - `neo-agent-core` remains the source of runtime phase events;
  - `neo-tui` owns the derived display state and rendering.
- Canonical owner:
  - runtime truth: `run_full_compaction` and `AgentEvent`;
  - visual estimate: a private display state owned by `TranscriptPane`.
- Invariants:
  - canonical events and context remain append-only;
  - display percent never regresses;
  - `100%` requires the applying/completed event;
  - completed cards remain static.
- Compatibility:
  - retain `AgentEvent::CompactionProgress { phase, percent }` in the first
    implementation;
  - old sessions with `15%` and `85%` anchors still render through the same
    phase-aware view logic.
- Non-goals: no new runtime owner, no context rewrite, no persisted migration.

## 7. Current Baseline and Problem

The current runtime emits these effective anchors:

```text
Estimating          0%
Selecting boundary 15%
Summarizing         15%
First summary text  85%
Applying            100%
```

The summary stream has no known final character/token count. Therefore the
first non-empty summary callback cannot be interpreted as `85%` completion. It
only proves that summary generation has started producing output.

The TUI currently renders a 12-cell bar and changes the leading filled cell
through `▓ / ▒ / ▓ / █` on every activity frame. Compaction is marked as a visible
animation, so this pulse is refreshed even when the underlying progress did not
change. The result reads as flicker rather than progress.

## 8. Options and Decision

### Option A: UI-only smoothing

Keep the current anchors and animate every incoming jump at a fixed rate. This
is low risk but leaves `85%` looking like a precise value even though it is only
a phase boundary.

### Option B: Phase-aware hybrid estimate

Keep runtime anchors as canonical facts. Add a private, non-persisted TUI display
state that maps the current phase to a bounded range, advances monotonically,
and labels the result with `~`. The selected phase ranges are:

```text
Estimating          0   - 10
Selecting boundary  10  - 20
Summarizing         20  - 85   (estimated; cap display at 82 until confirmed)
Applying            85  - 99
Completed           100       (authoritative)
```

This is the approved direction. It preserves useful numeric feedback without
claiming that summary output has a measurable total.

### Option C: Phase-only stepper

Remove the numeric percent and show only phase states. This is honest and
stable, but it gives up the numeric signal the user wants to retain.

## 9. Detailed Design

### 9.1 Derived display state

Reuse the existing `TranscriptPane`/compaction projection owner. Add only a
private view state; do not create a second runtime progress model or public
cross-crate type.

The state needs these values:

- current `CompactionPhase`;
- latest runtime event percent as a confirmed floor;
- current display percent;
- phase start time or elapsed tick origin;
- the phase's display cap;
- whether the compaction is terminal.

`TranscriptEntry::Compaction.percent` remains presentation data. Runtime event
values stay canonical in the event stream; the TUI may update the entry's
presentation percent as the derived display advances.

### 9.2 Target and rate limiting

For a non-terminal phase, calculate a slow target inside the phase range:

```text
elapsed_fraction = 1 - exp(-elapsed / tau_phase)
time_target      = phase_start + phase_span * elapsed_fraction
target           = max(event_percent, time_target)
target           = min(target, phase_display_cap)
display_percent  = min(target, previous_display + max_step_per_tick)
```

The exact `tau_phase`, tick interval, and step size should be calibrated against
local replay samples, but the initial behavior should satisfy these constraints:

- the summary estimate never exceeds `82%` before the runtime reports `85%`;
- an event jump is visually rate-limited rather than applied as one large bar
  mutation;
- the display never moves backward;
- duplicate events are no-ops;
- stale phase events cannot reopen or regress a later phase;
- `CompactionApplied` immediately transitions to the completed presentation.

The elapsed text may refresh at approximately one-second cadence. The filled
bar should update at a lower cadence such as 200-250ms, and only when the
visible percent actually changes.

### 9.3 Event transitions

| Runtime event | Display behavior |
|---|---|
| `CompactionStarted` | initialize at `0%`, phase `Estimating`, start the view timer |
| `CompactionProgress(Estimating, p)` | clamp into the estimating range and advance monotonically |
| `CompactionProgress(SelectingBoundary, p)` | enter the selecting range; do not regress below the previous display |
| `CompactionProgress(Summarizing, p)` | enter the summary range; time estimate may advance toward `82%` |
| summary output callback / `85%` event | move toward the confirmed `85%` anchor at the display rate |
| `CompactionProgress(Applying, p)` | show the applying range, capped below `100%` |
| `CompactionApplied` | set `100%`, remove live animation, render the completion summary |
| interrupt | replace the live entry with the existing interrupted status |

### 9.4 Stable renderer

Remove `compaction_pulse_char` from the compact presentation. The bar has a
fixed width and fixed glyphs:

```text
filled: █
empty:  ░
```

Use one stable active color for the current phase and a muted color for empty
cells. Only the completed line uses the success color. Do not switch colors at
30% and 70%, because those thresholds add another abrupt visual transition.

The renderer should not depend on `activity_frame` for compact. The existing
activity-frame argument may remain in the shared render path for other live
entries, but compact itself must not change glyphs merely because a frame tick
arrived.

### 9.5 UI presentation

Normal width:

```text
◈ Compacting context · 852 messages · 192k tokens
  Estimating       [█░░░░░░░░░░░░░░░░]  ~04%   0s
```

During the long phase:

```text
◈ Compacting context · 852 messages · 192k tokens
  Summarizing      [████████░░░░░░░░]  ~56%   8s
```

Near the estimate cap:

```text
◈ Compacting context · 852 messages · 192k tokens
  Summarizing      [███████████░░░░░]  ~78%   31s
```

Applying:

```text
◈ Compacting context · 852 messages · 192k tokens
  Applying         [███████████████░]  ~92%   1s
```

Completed:

```text
✔ Compaction complete · 852 messages · 192k → 24k tokens
```

Narrow-width fallback should preserve the phase and estimate before optional
metadata:

```text
◈ Summarizing [██████░░░░] ~56% · 8s
```

At very narrow widths, collapse to a single stable status line rather than
wrapping the bar into a second inconsistent geometry:

```text
◈ compacting ~56%
```

## 10. Architecture and Boundary Review

### Existence Check

- Proposed new surface: private `CompactionDisplayState` in the existing TUI
  compaction owner.
- Existing owner/reuse candidate: `TranscriptPane`, `TranscriptStore`, and
  `render_status::render_compaction`.
- Why existing surface is insufficient: the current entry only carries the
  latest phase/percent and has no elapsed-time or rate-limit state.
- Creation proof: the state is required to derive a bounded monotonic estimate;
  it is private, non-persisted, and does not create a second runtime owner.
- Entropy/retirement impact: remove the pulse helper and its per-frame visual
  behavior; no compatibility branch is added.
- Decision: `add-with-proof`, as a private view-state field only.

### Product Risk Lens

- Value: users can tell that compact is working without being misled by a
  dramatic jump or distracted by flicker.
- Non-goals: the bar is not an exact summary-token completion meter.
- Trade-off: the estimate may sit near `82%` while a slow model finishes; that
  is preferable to claiming false completion.
- Decision needed: approved hybrid semantics; phase ranges can be tuned from
  replay evidence during implementation.

### Architecture Integrity Lens

- Invariant: runtime events remain the source of truth; visual state is derived.
- Canonical owner/contract: core owns phases and terminal completion; TUI owns
  only presentation smoothing.
- Responsibility overlap: do not add a second core estimator or reuse swarm
  progress logic.
- Higher-level simplification: remove `compaction_pulse_char` and let phase
  changes, elapsed time, and a static bar communicate activity.
- Retirement/falsifier: if replay evidence shows the time estimate is more
  misleading than a stepper, fall back to the approved phase-only presentation
  rather than adding more animation.
- Verdict: coherent with existing runtime/TUI boundaries.

### Baseline Role Alignment

- Product/Requirement Baseline: user-confirmed hybrid progress semantics and
  stable non-flickering UI.
- Architecture/Runtime Boundary Baseline: core event source plus TUI-derived
  presentation state, with append-only context/session records.
- Result: `aligned`.
- Scope: `both`.
- Next action: create a focused implementation plan after this brief is
  reviewed.

### Plan-Time Complexity Check

- Artifact class: localized cross-crate behavior change with a TUI-derived state.
- Target files: compaction summary event producer only if calibration requires
  it; TUI pane/event handler/entry renderer; focused unit tests.
- Current pressure: renderer has a single compact helper, while pane owns live
  entry mutation and animation ticks.
- Projected pressure: moderate; no new public crate API or event schema.
- Budget result: `within-budget` if the first implementation stays UI-derived.
- Better file boundary: keep state in `TranscriptPane` and rendering in the
  existing `render_status` module; do not create a new progress subsystem.
- Recommendation: `edit-in-place` with a private helper/state, then revisit
  core event granularity only if replay evidence shows it is necessary.

## 11. Compatibility and Context Integrity

- Do not mutate, delete, reorder, or summarize canonical context messages.
- Do not change cache-prefix bytes or provider request projections.
- Do not rewrite historical JSONL events. The display state is ephemeral.
- Keep micro compaction and snip+dedup default-off.
- Keep the existing completed-card token reduction wording and semantics.
- Keep old `15%`/`85%` progress events renderable during replay.

## 12. Acceptance and Verification

Focused TUI tests should prove:

1. A `15%` summarizing anchor followed by `85%` never renders as a single
   visible backward/forward jump; display values are monotonic and rate-limited.
2. A summary phase with no new runtime event advances only inside its bounded
   estimate range and never reaches `100%`.
3. A stale event for an earlier phase cannot reduce the displayed percentage or
   reopen a completed entry.
4. Rendering the same compaction state with different activity frames produces
   the same bar glyph sequence.
5. `CompactionApplied` renders `100%` and the existing token reduction summary.
6. Duplicate progress events do not mark the transcript dirty unnecessarily.
7. Narrow widths retain phase and estimate without wrapping the progress bar
   into unstable geometry.

Verification should use the narrowest exact `neo-tui` library test target and,
if the event handling changes, one exact `neo-agent-core` compaction test. No
broad workspace test is required as evidence for this localized design.

## 13. ADR Signal

This is a user-visible behavior change, but the approved first implementation
keeps the normalized event schema and runtime ownership unchanged. No new ADR is
required before implementation. An ADR becomes necessary only if later work
changes `AgentEvent::CompactionProgress` into a new persisted contract or moves
progress estimation into a new core owner.

## 14. Decision

Approved design direction:

- retain numeric progress as a clearly marked phase estimate;
- use bounded phase ranges and monotonic rate-limited display updates;
- remove shimmer and all per-frame compact bar glyph changes;
- keep core events canonical and TUI estimation ephemeral;
- verify with focused deterministic renderer and event-transition tests.
