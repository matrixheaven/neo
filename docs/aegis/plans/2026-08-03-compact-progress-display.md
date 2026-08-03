# Neo Compact Progress Display Implementation Plan

Date: `2026-08-03`
Status: approved for execution

## Goal

Implement the approved compact progress display redesign from
`docs/aegis/specs/2026-08-03-compact-progress-display-design.md`.

The interactive TUI must retain numeric progress while presenting it as an
estimate, smooth the runtime's `15% -> 85%` anchor transition into a monotonic
rate-limited display, keep the summarizing estimate bounded below completion,
show `100%` only after `CompactionApplied`, and remove the per-frame filled-bar
shimmer. The runtime event stream, JSONL representation, context/cache-prefix
semantics, and historical transcript contracts remain unchanged.

## Architecture

`neo-agent-core` remains the canonical owner of compaction phases and terminal
completion. `TranscriptPane` remains the canonical owner of the ephemeral
visual projection. A private `CompactionDisplayState` in `pane.rs` will hold the
current phase, confirmed runtime floor, display value, phase timing, and update
cadence. It will never be serialized, placed in `TranscriptStore`, or added to
`TranscriptEntry`'s persisted/canonical shape.

`TranscriptEntry::Compaction.percent` remains presentation data. Pane methods
will write the derived display value to that field only when the visible value
changes. The existing event handler continues routing `CompactionProgress`
through `update_compaction_progress` and `CompactionApplied` through
`upsert_compaction`; no new runtime owner or cross-crate display type is added.

## Tech Stack

- Rust workspace, edition 2024, minimum Rust `1.96.1`.
- `neo-agent-core` normalized `AgentEvent` and `CompactionPhase` contracts.
- `neo-tui` `TranscriptPane`, `TranscriptStore`, `TranscriptEntry`, and
  `render_status::render_compaction`.
- Existing Rust unit-test infrastructure and `cargo test`/`cargo nextest`.
- `rtk` command prefix required by the repository workflow.

## Baseline/Authority Refs

- Approved design: `docs/aegis/specs/2026-08-03-compact-progress-display-design.md`,
  especially Sections 6, 9, 11, and 12.
- Runtime event producer: `crates/neo-agent-core/src/compaction/summary.rs`,
  `run_full_compaction`.
- Event contract: `crates/neo-agent-core/src/events.rs`,
  `CompactionPhase` and `CompactionProgress`.
- TUI event routing: `crates/neo-tui/src/transcript/event_handler.rs`,
  `apply_compaction_event`.
- TUI projection and tick owner: `crates/neo-tui/src/transcript/pane.rs`,
  `upsert_compaction`, `update_compaction_progress`, and
  `advance_animation_at_ms`.
- Renderer: `crates/neo-tui/src/transcript/entry/render_status.rs`,
  `render_compaction` and `compaction_pulse_char`.
- Entry lifecycle and animation contract:
  `crates/neo-tui/src/transcript/entry/mod.rs`.
- Append-only presentation/cache boundaries:
  `crates/neo-tui/src/transcript/store.rs` and
  `crates/neo-tui/src/transcript/presentation.rs`.
- Project operating rules: `AGENTS.md`, `CX.md`, `RTK.md`, and
  `docs/aegis/BASELINE-GOVERNANCE.md`.

### BaselineUsageDraft

- Required baseline refs: runtime compaction events, TUI event projection,
  pane animation ticks, compaction renderer, entry finalization, and append-only
  transcript/cache behavior.
- Delivered context refs: current source reads plus CodeGraph and
  codebase-memory exploration of the compaction symbols and their callers.
- Acknowledged before plan refs: `AGENTS.md`, `CX.md`, `RTK.md`, and
  `docs/aegis/BASELINE-GOVERNANCE.md`.
- Cited in this plan: all required baseline refs above.
- Missing refs: no additional authoritative product wording beyond the approved
  design; the user-confirmed design spec is the requirement authority.
- Decision: `continue`.

## Compatibility Boundary

The following must remain byte/schema/behavior compatible:

- Do not modify `AgentEvent::CompactionProgress`, `CompactionPhase`, or
  `CompactionApplied`.
- Do not modify compaction event emission in `neo-agent-core`.
- Do not modify JSONL serialization or replay records.
- Do not modify `AgentContext`, provider request projections, context cache-prefix
  bytes, message ordering, or canonical historical records.
- Keep old replay anchors such as `15%` and `85%` renderable through the same
  phase-aware TUI projection.
- Keep completed-card wording, token reduction values, and append-only behavior.
- Keep micro compaction and snip+dedup default-off.
- Keep Delegate, DelegateGroup, DelegateSwarm, and workflow card layout/content/
  expansion behavior unchanged.
- Do not add an elapsed-time field to `TranscriptEntry` or `TranscriptStore`.
  The approved design permits elapsed metadata, but the first implementation
  uses elapsed time only inside the private estimator so the render pipeline
  and canonical entry shape stay unchanged.

## TDD Route

- Mode: off
- Decision: skipped
- Strict authority: not applicable; the user approved focused post-change
  regression tests rather than strict test-first TDD.
- Test posture: post-change regression with deterministic pane time inputs and
  renderer snapshots expressed as assertions.
- Reason: this is a localized TUI behavior change with an existing rendering
  test surface; the minimum proportional proof is a focused set of pane and
  entry tests after the implementation.
- Verification: exact `neo-tui` library test selectors, formatting, and the
  crate library lint command listed below.

## Aegis Visibility

Planning is necessary because this change introduces a private visual-state
owner, changes live-animation scheduling, retires a per-frame renderer path,
and needs explicit proof that runtime/session/context contracts remain outside
the change.

## Plan Basis

The approved design identifies the runtime's `15%` and `85%` values as phase
anchors rather than a known summary completion percentage. It requires phase
ranges of `0-10`, `10-20`, `20-82` until confirmed `85`, and `85-99`, with
`100` reserved for terminal application. It also requires monotonic display,
rate-limited event jumps, stale-event protection, duplicate-event no-ops, a
stable `█`/`░` bar, an estimated `~NN%` label, and narrow-width fallbacks.

## Requirement Ready Check

- Requirement source refs: user-confirmed hybrid progress semantics and
  `docs/aegis/specs/2026-08-03-compact-progress-display-design.md`.
- Goals and scope refs: design Sections 1, 2, 6, and 7.
- User/scenario refs: an operator watching a long manual or automatic compact in
  the interactive TUI.
- Requirement item refs: design Sections 9.1-9.5.
- Acceptance/verification refs: design Section 12.
- Open blocker questions: none.
- Decision: `ready`.

## Change Necessity

- User-visible need: the current compact card visibly jumps from a low anchor to
  `85%`, pulses its leading glyph every frame, and presents a phase boundary as
  exact completion progress.
- No-change/non-code option: documentation or event replay changes alone cannot
  change the already-rendered TUI behavior.
- Why code change is necessary: the current entry stores only the latest event
  value and the renderer derives a frame-dependent pulse; smoothing and bounded
  phase estimation require ephemeral timing and mutation logic in the TUI.
- Minimum change boundary: `TranscriptPane` for derived state and ticks,
  `render_status.rs` for stable output, and existing focused tests in `pane.rs`
  and `entry/mod.rs`.
- Decision: `code-change`.

## Existence Check

- Proposed new surface: private `CompactionDisplayState`.
- Existing owner/reuse candidate: `TranscriptPane` already owns compaction
  upsert/update routing, dirty state, and render ticks.
- Why the existing entry alone is insufficient: it has no phase-start timing,
  confirmed event floor, or rate-limit state, and adding timing to the entry
  would widen the canonical/render data shape unnecessarily.
- Creation proof: the private pane field is the smallest non-persisted state
  that can derive a monotonic time-based estimate and be reset on completion.
- Entropy/retirement impact: delete `compaction_pulse_char` and stop treating a
  completed compaction as live animation; no compatibility branch is added.
- Decision: `add-with-proof`.

## Architecture Integrity Lens

- Invariant: runtime events remain source-of-truth facts; the TUI estimate is a
  derived presentation only.
- Canonical owner/contract: core owns event phases and terminal application;
  `TranscriptPane` owns visual smoothing; `render_status` owns glyph/color
  presentation.
- Responsibility overlap: do not add a core estimator, persisted percentage,
  second pane model, or reuse the swarm progress estimator.
- Higher-level simplification: make live scheduling update the compact card only
  when the derived percentage changes, while retaining the existing scheduler
  signal for active compaction and other live cards.
- Retirement/falsifier: the pulse helper and unconditional completed-card
  animation path are retired. If replay evidence shows the bounded estimate is
  materially misleading, the next change is a phase-only design review, not
  more animation or another progress owner.
- Verdict: proceed with the existing TUI owner.

## Plan Pressure Test

- Owner/contract/retirement: explicit; only a private pane view state is added,
  and the old pulse path is removed.
- Architecture integrity/higher-level path: explicit; core events and append-only
  context remain untouched.
- Verification scope: covers rate limiting, bounded summary estimate, stale and
  duplicate events, terminal completion, static glyphs, and narrow rendering.
- Task executability: each task names exact files, methods, state transitions,
  and exact focused commands.
- Pressure result: `proceed`.

## Plan-Time Complexity Check

- Target files: `crates/neo-tui/src/transcript/pane.rs`,
  `crates/neo-tui/src/transcript/entry/render_status.rs`, and
  `crates/neo-tui/src/transcript/entry/mod.rs` tests/animation behavior.
- Existing size/shape signals: `pane.rs` already owns compaction mutation and
  the animation tick; `render_status.rs` has one compact renderer and one pulse
  helper; `entry/mod.rs` has existing compact lifecycle tests.
- Owner fit: all changes fit the current pane/renderer/entry boundaries.
- Add-in-place risk: adding timing to `TranscriptEntry` or introducing a new
  module would expand persisted/render ownership without need.
- Better file boundary: private estimator helpers in `pane.rs`, stable output in
  `render_status.rs`, and focused assertions in the existing test modules.
- Recommendation: `edit-in-place` with one private pane state and small pure
  helpers; do not split a new subsystem.
- Complexity budget: `within-budget`.

## Files

### Modify

- `crates/neo-tui/src/transcript/pane.rs`
  - Add private `CompactionDisplayState` and phase range/target helpers.
  - Initialize/reset/update the state in compaction upsert/progress paths.
  - Add a deterministic `*_at_ms` internal path used by the public/super event
    method and tests.
  - Advance the estimate on the existing pane animation tick at a cadence of
    about 200-250 ms, with a maximum visible step of one percentage point per
    tick.
  - Avoid dirtying the pane for duplicate events or ticks that do not change the
    displayed percentage.
  - Ignore lower-rank stale phases and progress after a completed card until a
    new `CompactionStarted` creates a new live card.
  - Preserve the scheduler's active-live signal for unfinished compaction while
    preventing compact-only frames from forcing redraws due to `activity_frame`.
  - Update the existing completed-card append test for derived display values and
    add deterministic transition tests.

- `crates/neo-tui/src/transcript/entry/render_status.rs`
  - Remove `compaction_pulse_char`.
  - Keep the shared `activity_frame` parameter for call-site compatibility, but
    make compact output independent of it.
  - Render filled cells only as `█` and empty cells only as `░`.
  - Use a stable active color for the current phase and a muted empty-cell color;
    remove the old 30%/70% threshold color changes.
  - Prefix active progress with `~` and preserve the completed success line.
  - Add stable compact and very-narrow single-line fallbacks that prioritize
    phase and estimate over optional header metadata.

- `crates/neo-tui/src/transcript/entry/mod.rs`
  - Make a completed compaction no longer report visible animation.
  - Keep other entries' animation and cache behavior unchanged.
  - Update the active renderer test for estimated progress and stable glyphs.
  - Add a same-state/different-frame renderer assertion and a narrow-width
    single-line assertion.

- `docs/aegis/plans/2026-08-03-compact-progress-display.md`
  - This implementation plan.

- `docs/aegis/INDEX.md`
  - Append the plan record without changing existing entries.

### Read-only verification boundary

- `crates/neo-tui/src/transcript/event_handler.rs`: existing compaction event
  routing must continue to call the pane methods.
- `crates/neo-agent-core/src/compaction/summary.rs` and
  `crates/neo-agent-core/src/events.rs`: runtime event producer/schema remain
  unchanged.
- `crates/neo-tui/src/transcript/store.rs` and
  `crates/neo-tui/src/transcript/presentation.rs`: existing append-only/cache and
  live-block behavior must remain intact.

## Execution Readiness View

- Intent Lock: deliver a stable, honest compact progress estimate in the
  interactive TUI while retaining useful numeric feedback.
- Scope Fence: modify only the three TUI source files and the Aegis plan/index;
  do not modify core events, context, JSONL, provider requests, historical data,
  or unrelated dirty files.
- Baseline Lock: use the approved design spec and the listed runtime/TUI source
  refs as the authority; re-read any target function immediately before editing.
- Approved Behavior: `~NN%`, monotonic rate-limited phase ranges, summary cap at
  `82%` until the `85%` runtime anchor, terminal `100%` only after apply, static
  bar, and stable narrow fallbacks.
- Owner/Contract Constraints: private ephemeral state belongs to
  `TranscriptPane`; runtime events and canonical transcript records remain
  unchanged; other card designs remain unchanged.
- Compatibility Boundary: preserve event schema, replay anchors, completed-card
  wording, append-only transcript/cache-prefix semantics, and default-off
  compaction options.
- Retirement Boundary: remove the pulse helper and completed-compaction live
  animation; do not retain a duplicate shimmer path or compatibility renderer.
- Task Batches: pane state and scheduling; renderer and entry animation; focused
  regression tests and verification.
- Test Obligations: deterministic time-step tests, stale/duplicate/terminal event
  tests, static glyph comparison, narrow fallback, and token-reduction regression.
- Review Gates: self-review the diff against the scope fence; run exact focused
  tests; run formatting and `neo-tui` library clippy; inspect scoped diff before
  any commit.
- Drift/Rewind Rules: if current source has changed in a target file, re-read it
  and adapt the patch; never revert unrelated worktree changes. If the event
  schema or owner boundary must change, stop and return to design/ADR review.
- Evidence Required Before Completion: focused test output, formatting/lint
  output, and a scoped diff showing only the intended plan/index/TUI changes.
- Advisory Boundary: this is execution guidance only, not a gate decision,
  policy snapshot, or completion authority.

## Tasks

### Task 1: Add private phase-aware pane estimation

**Files:**

- Modify `crates/neo-tui/src/transcript/pane.rs`.
- Exercise/add tests in the `#[cfg(test)] mod tests` at the bottom of the same
  file.
- Read-only boundary: `event_handler.rs`, `events.rs`, and `summary.rs`.

**Why:** The current pane copies raw event percentages into the live entry, so
an anchor jump is immediately visible and no time-based estimate exists.

**Change Necessity:** A renderer-only change cannot smooth event data or protect
against stale events. The minimum source boundary is the existing pane owner
plus one private state field and pure helpers.

**Implementation:**

1. Add this exact private copyable state:
   `#[derive(Debug, Clone, Copy)] struct CompactionDisplayState {
   phase: neo_agent_core::CompactionPhase, confirmed_percent: u8,
   display_percent: u8, phase_started_at_ms: u64, last_update_at_ms: u64,
   }`. Store it as an `Option<CompactionDisplayState>` on `TranscriptPane`; initialize it to `None` in `new`.
2. Add these exact private constants: `COMPACTION_PROGRESS_TICK_MS: u64 = 250`,
   `COMPACTION_MAX_STEP_PER_TICK: u8 = 1`,
   `COMPACTION_TAU_ESTIMATING_MS: u64 = 1_000`,
   `COMPACTION_TAU_SELECTING_MS: u64 = 1_500`,
   `COMPACTION_TAU_SUMMARIZING_MS: u64 = 30_000`, and
   `COMPACTION_TAU_APPLYING_MS: u64 = 1_000`. Add a pure phase bounds helper
   returning the approved range and matching time constant:
   `Estimating: 0-10`, `SelectingBoundary: 10-20`,
   `Summarizing: 20-82` until confirmed `85`, and `Applying: 85-99`.
3. Implement the approved target calculation with a bounded asymptotic time
   fraction: `1 - exp(-elapsed / tau)`, then take the maximum of the confirmed
   event floor and time target, clamp to the current phase cap, and finally
   advance by at most one point from the previous display. Once a summarizing
   event confirms at least `85`, permit the display to advance to `85`; never
   produce `100` in a non-terminal state.
4. Compare phase rank before accepting an event. Ignore a lower-rank event than
   the active phase. For an accepted higher-rank phase, retain the current
   display value, reset the phase timer, and let the target rise from the new
   phase range. For a same-phase event, raise the confirmed floor without ever
   lowering it.
5. Split the existing progress update into the exact private method
   `update_compaction_progress_at_ms(phase, percent, now_ms)` and keep
   `update_compaction_progress` as the existing event-facing wrapper that uses
   `monotonic_time_ms()`. This gives pane tests deterministic timestamps without
   changing the event handler contract.
6. When a live card is first created, initialize the state at the event time and
   write the initial derived value rather than the raw jump. When
   `CompactionApplied` reaches `upsert_compaction` with applying/100, write
   authoritative `100`, clear the state, and leave the completed card static.
   Preserve the existing rule that a new compaction after a completed card
   appends a new entry.
7. When no live entry exists, reject progress updates if the latest compaction
   entry is already complete. This prevents a delayed earlier event from
   reopening a completed card; a later start event is still allowed to append a
   fresh live card.
8. Change `advance_animation_at_ms` to call the deterministic compaction
   estimator. Retain the existing frame counter for other animated entries, but
   mark the pane dirty for compact only when the derived percentage changes.
   Keep unfinished compaction visible to the outer scheduler and stop reporting
   it as visible animation after completion.
9. Make duplicate events return without mutation and without calling
   `mark_dirty` when phase, confirmed floor, presentation percent, and metadata
   are unchanged. Ensure entry mutation still invalidates the existing
   transcript/body caches through the established `TranscriptStore` path.

**Impact/Compatibility:** The only changed value is the ephemeral TUI
presentation percent. Core event values, event routing, JSONL, and transcript
records remain untouched. Existing append-only completed-card behavior remains
an invariant.

**Verification:** Run the exact pane tests after implementation:

```text
rtk cargo test -p neo-tui --lib transcript::pane::tests::compaction_progress_is_rate_limited_and_monotonic --exact --nocapture
rtk cargo test -p neo-tui --lib transcript::pane::tests::compaction_summary_estimate_stays_below_completion --exact --nocapture
rtk cargo test -p neo-tui --lib transcript::pane::tests::stale_compaction_progress_does_not_regress_or_reopen_completed_card --exact --nocapture
rtk cargo test -p neo-tui --lib transcript::pane::tests::duplicate_compaction_progress_is_a_noop --exact --nocapture
```

**Repair Track:** Root cause is raw event percentages being used as direct
presentation values. The canonical repair is the private pane estimator with
phase rank, cap, cadence, and monotonic step enforcement. No runtime repair is
needed.

**Retirement Track:** Retire direct raw-percent assignment for live compaction
and the unconditional compact redraw assumption. The old behavior is deleted
from the pane path; no fallback is retained. A future retirement trigger is a
later design decision to remove numeric estimates entirely, which is outside
this task.

### Task 2: Stabilize compact rendering and lifecycle animation

**Files:**

- Modify `crates/neo-tui/src/transcript/entry/render_status.rs`.
- Modify `crates/neo-tui/src/transcript/entry/mod.rs`.
- Exercise the existing entry test module in `entry/mod.rs`.

**Why:** The current renderer changes the leading bar glyph on every activity
frame and uses abrupt percentage color thresholds. Completed compaction also
reports visible animation because the match arm is unconditional.

**Change Necessity:** The visual defect is created directly by the renderer and
entry animation contract, so the existing renderer and entry owner are the
minimum code boundary.

**Implementation:**

1. Delete `compaction_pulse_char` and remove its use. Keep the
   `render_compaction` parameter position only if needed by the shared render
   path; name the unused parameter accordingly so compact output cannot depend
   on `activity_frame`.
2. Render the normal fixed-width bar with filled `█` cells and empty `░` cells.
   Use one active color selected from the current phase and muted text for empty
   cells. Remove the 0-29/30-69/70-100 color thresholds.
3. Render active numeric values as `~NN%` and leave the completed success line
   unchanged, including message count and token reduction formatting.
4. Keep the normal two-line compact presentation where the width permits. For a
   narrower but usable width, render one stable line containing the compact icon,
   phase, a reduced fixed-width bar, and `~NN%`. At very narrow widths render
   one stable line in the form `◈ compacting ~NN%`; do not let wrapping split the
   bar into changing geometry. Phase and estimate take priority over optional
   metadata.
5. Change `TranscriptEntry::has_visible_animation` so a compaction is animated
   only while it is not applying/100. Preserve every other entry's animation
   condition. Do not change Delegate, DelegateGroup, DelegateSwarm, workflow,
   retry, or MCP startup behavior.
6. Keep completed compaction output static and preserve the existing token
   reduction test. Leave canonical entry fields and copy/finalization semantics
   unchanged.

**Impact/Compatibility:** The shared `activity_frame` render API remains
available to other live entries. Compact text becomes stable between actual
pane state updates, completed cards stop participating in live animation, and
all unrelated card renderers keep their current layout/content semantics.

**Verification:** Run these exact entry tests:

```text
rtk cargo test -p neo-tui --lib transcript::entry::tests::compaction_in_progress_renders_estimated_static_progress_bar --exact --nocapture
rtk cargo test -p neo-tui --lib transcript::entry::tests::compaction_render_is_independent_of_activity_frame --exact --nocapture
rtk cargo test -p neo-tui --lib transcript::entry::tests::compaction_narrow_width_keeps_one_stable_line --exact --nocapture
rtk cargo test -p neo-tui --lib transcript::entry::tests::compaction_complete_renders_token_reduction --exact --nocapture
```

**Repair Track:** Root cause is the renderer's pulse helper and the entry's
unconditional compaction animation match arm. The repair is to remove the
visual pulse, mark active estimates explicitly, and make completion terminal
for scheduling.

**Retirement Track:** `compaction_pulse_char` and its frame-driven output are
fully retired. No alternate shimmer or color-threshold compatibility path is
kept.

### Task 3: Run cross-boundary focused verification and inspect scope

**Files:**

- Read-only review of `crates/neo-agent-core/src/events.rs`,
  `crates/neo-agent-core/src/compaction/summary.rs`,
  `crates/neo-tui/src/transcript/event_handler.rs`,
  `crates/neo-tui/src/transcript/store.rs`, and
  `crates/neo-tui/src/transcript/presentation.rs`.
- Inspect the scoped diff for the files listed under Modify.

**Why:** The behavior change is TUI-local, but the success criterion includes
preserving core event, replay, context, cache, and historical transcript
contracts in the shared dirty worktree.

**Change Necessity:** No additional production source change is needed for this
verification task. It proves that the implementation stayed within the approved
boundary.

**Implementation:** No source edits. Re-read the event handler to confirm it
still routes start/progress/applied events to the existing pane methods. Compare
the final diff against the task-start dirty snapshot and ignore unrelated
changes in core/runtime/provider files.

**Impact/Compatibility:** If the diff contains changes to event schema, JSONL,
context, cache-prefix construction, provider requests, or unrelated card
renderers, stop the task and repair the scope before verification is claimed.

**Verification:** Run the exact focused regression set and checks:

```text
rtk cargo test -p neo-tui --lib transcript::pane::tests::compaction_progress_is_rate_limited_and_monotonic --exact --nocapture
rtk cargo test -p neo-tui --lib transcript::pane::tests::compaction_summary_estimate_stays_below_completion --exact --nocapture
rtk cargo test -p neo-tui --lib transcript::pane::tests::stale_compaction_progress_does_not_regress_or_reopen_completed_card --exact --nocapture
rtk cargo test -p neo-tui --lib transcript::pane::tests::duplicate_compaction_progress_is_a_noop --exact --nocapture
rtk cargo test -p neo-tui --lib transcript::entry::tests::compaction_in_progress_renders_estimated_static_progress_bar --exact --nocapture
rtk cargo test -p neo-tui --lib transcript::entry::tests::compaction_render_is_independent_of_activity_frame --exact --nocapture
rtk cargo test -p neo-tui --lib transcript::entry::tests::compaction_narrow_width_keeps_one_stable_line --exact --nocapture
rtk cargo test -p neo-tui --lib transcript::entry::tests::compaction_complete_renders_token_reduction --exact --nocapture
rtk cargo fmt --all --check
rtk cargo clippy -p neo-tui --lib -- -D clippy::all
rtk git diff --check
```

No `neo-agent-core` test is required because the runtime producer and event
schema remain unchanged. If implementation unexpectedly changes event routing
or core source, add one exact core compaction test selector only after
reassessing that boundary.

**Repair Track:** If a focused test fails, diagnose the smallest in-scope cause
and repair it in the owning TUI file. Do not loosen assertions, alter unrelated
work, or widen to a package-wide test run.

**Retirement Track:** Keep no temporary compatibility test path or duplicate
renderer. Remove any diagnostic-only helper before the scoped diff review.

### Task 4: Commit only the verified coherent change

**Files:**

- `docs/aegis/plans/2026-08-03-compact-progress-display.md`
- `docs/aegis/INDEX.md`
- `crates/neo-tui/src/transcript/pane.rs`
- `crates/neo-tui/src/transcript/entry/render_status.rs`
- `crates/neo-tui/src/transcript/entry/mod.rs`

**Why:** The repository workflow requires one coherent verified commit when the
work is complete, while the shared worktree contains unrelated user/agent
changes.

**Change Necessity:** A scoped commit is the minimum history operation needed to
record this coherent task; it must not include unrelated dirty files.

**Implementation:** After all checks pass, inspect
`rtk git diff -- docs/aegis/plans/2026-08-03-compact-progress-display.md docs/aegis/INDEX.md crates/neo-tui/src/transcript/pane.rs crates/neo-tui/src/transcript/entry/render_status.rs crates/neo-tui/src/transcript/entry/mod.rs`
and stage only the listed paths. If `.git/index.lock` is present, do not
delete it or retry destructively; inspect for an active owner and report the
blocker if the lock remains. Never reset, restore, stash, clean, or overwrite
unrelated worktree changes.

**Impact/Compatibility:** The commit must contain only the plan/index and the
three TUI source files for this task. Existing unrelated modifications remain
untouched and unstaged.

**Verification:** Use non-interactive commands:

```text
rtk git diff --check
rtk git status --short
rtk git add docs/aegis/plans/2026-08-03-compact-progress-display.md docs/aegis/INDEX.md crates/neo-tui/src/transcript/pane.rs crates/neo-tui/src/transcript/entry/render_status.rs crates/neo-tui/src/transcript/entry/mod.rs
rtk git diff --cached --check
rtk git commit -m "fix: stabilize compact progress display"
```

If the shared index lock blocks staging, preserve the verified worktree and
report that commit verification could not be completed because of the external
lock; do not claim a commit.

**Repair Track:** If staging includes an unrelated path, unstage only that path
with a targeted non-destructive index operation and re-check the staged path
list. Do not alter worktree contents.

**Retirement Track:** No temporary staging path or duplicate commit is retained.

## Risks and Mitigations

- Estimate stalls near `82%`: this is an intentional honesty bound because no
  summary total exists. Tests assert it never reaches `100%` without apply.
- Phase event arrives out of order: phase rank and terminal-card guards reject
  regressions and delayed replay events.
- Ticks cause excessive redraws: compact-only dirtying occurs only when the
  derived percentage changes; other live-card animation remains unchanged.
- Narrow terminal wraps the bar: explicit compact and very-narrow one-line
  fallbacks keep geometry stable.
- Existing completed-card behavior regresses: preserve the completion renderer,
  token reduction assertion, and append-after-completion test.
- Shared dirty worktree hides scope mistakes: capture the task-start status,
  inspect only the listed diff, and never revert unrelated files.
- Existing `.git/index.lock` blocks commit: do not remove it; leave verified
  worktree changes intact and report the exact commit limitation.

## Retirement and Follow-up Boundary

The obsolete pulse helper, frame-dependent compact glyph changes, and completed
compact live-animation path are removed in this task. No compatibility renderer,
second estimator, persisted display field, or core fallback is retained.

Elapsed time is used internally for the estimate but is intentionally not added
to the first implementation's visible text because the current entry-only render
pipeline has no private pane snapshot channel. Adding elapsed metadata later
requires a separate design/verification decision so it does not widen canonical
transcript state. Likewise, moving estimation into core or changing the event
schema requires a new ADR and replay compatibility review.

## Self-Review Record

- Spec coverage: phase ranges, bounded summary estimate, monotonic/rate-limited
  updates, stale/duplicate handling, authoritative completion, stable glyphs,
  narrow fallback, and focused verification each map to Tasks 1-3.
- Placeholder scan: no unresolved work markers or deferred implementation
  instructions are present.
- Type consistency: the existing `CompactionPhase`, `AgentEvent`, pane methods,
  entry fields, and shared render signatures remain the named interfaces.
- Compatibility: runtime, JSONL, context/cache-prefix, historical transcript,
  completed wording, and unrelated card contracts are explicit non-goals for
  modification.
- Change necessity: the plan identifies why smoothing requires pane code and
  why renderer stabilization requires entry/render code.
- Existence/architecture: the only new surface is a private pane field with a
  proof and a retirement path.
- Complexity/minimality: no new module, public API, event field, persisted field,
  or cross-crate owner is introduced.
- Verification: every major behavior has an exact library test selector plus
  formatting, clippy, and diff checks.
- Dual track: repair and retirement tracks are included for each implementation
  task.
- ADR signal: no ADR is required unless runtime ownership or persisted event
  schema changes during execution.

## Execution Route

- Decision: `inline`.
- Evidence: pane state, renderer, entry lifecycle, and tests are coupled in the
  same `neo-tui` owner; the shared dirty worktree also requires one coordinator
  to preserve scoped edits.
- Fallback: if a genuinely independent read-only review is needed, use an
  explorer subagent without delegating overlapping edits; implementation remains
  inline.
- User confirmation required: `no`; the user already approved the design and
  explicitly requested plan followed by implementation.

After this plan is saved and indexed, invoke the `executing-plans` skill and
execute Tasks 1-4 with checkpoints. Before the first source edit, capture the
current scoped worktree snapshot and re-read the exact target functions.
