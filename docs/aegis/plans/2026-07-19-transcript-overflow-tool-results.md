# Automatic Transcript Overflow and Tool Result Presentation Implementation Plan

> **For agentic workers:** Execute with `aegis:executing-plans` or
> `aegis:subagent-driven-development`. Preserve unrelated dirty work, use
> `apply_patch` for edits, and run only the focused verification below.

**Goal:** Preserve the existing Delegate and DelegateSwarm Card UI while
removing presentation-level transcript omission, automatically viewporting
overflow, and giving `ListDelegates` and `Sleep` structured semantic results.

**Architecture:** Start only after Terminal Live Viewport Isolation is complete.
`TranscriptPresentation` emits the complete canonical mutable suffix plus
overflow/frontier signals. `NeoTui` selects either the primary inline path or
one source-preserving alternate-screen viewport with fixed chrome by using the
existing `TerminalFrame` surface flag. The post-isolation `InlineTerminal`
remains untouched as the sole absolute-geometry, protected-history, and physical
surface owner. Existing `ToolCallComponent` and `tool_renderers` consume the
current `ListDelegates` details and `Sleep` arguments without changing core
tools.

**Tech Stack:** Rust 2024, existing `neo-tui` transcript,
`TranscriptViewport`, `NeoTui`, `neo-agent` input routing, vt100 test harness,
Serde JSON, existing animation scheduler.

**Baseline/Authority Refs:**

- `docs/aegis/specs/2026-07-19-terminal-live-viewport-isolation-design.md`
- `docs/aegis/plans/2026-07-19-terminal-live-viewport-isolation.md`
- `docs/aegis/specs/2026-07-19-transcript-overflow-tool-results-design.md`
- `docs/aegis/specs/2026-07-13-immutable-terminal-scrollback-design.md`
- `docs/aegis/specs/2026-07-17-ctrl-o-review-chrome-design.md`
- `docs/aegis/specs/2026-07-13-transcript-boundary-semantics-design.md`
- `docs/aegis/BASELINE-GOVERNANCE.md`
- `AGENTS.md`

**Compatibility Boundary:** Preserve the completed isolation baseline's absolute
geometry, protected history-only scroll region, CPR/resize generation,
transactional acknowledgement, terminal cleanup, and cross-platform behavior.
Also preserve canonical history ordering, Ctrl+O review, Card output/expansion,
and frame width/height bounds. Add no physical renderer change, feature flag,
fallback, new tool field, core tool behavior, dependency, compact mode, or
summary mode.

**TDD Route:**

- Mode: off
- Decision: skipped
- Strict authority: not applicable
- Test posture: post-change focused regressions
- Reason: strict TDD was not requested; the approved design requires deletion
  of an existing fitting path and focused behavior proofs after each slice.
- Verification: every Cargo command below names one package, one target, and one
  exact test.

**Verification:** Use the exact tests in each task, then the final Card Output
Lock, formatting, diff, and terminal-clear checks. Do not run workspace-wide
Cargo tests.

---

## Scope Check

**Requirement Ready Check:** Ready after the higher-priority Terminal Live
Viewport Isolation plan is implemented and committed. The approved overflow
design fixes the trigger, latch-release rule, scroll inputs, manual-review
precedence, Card Output Lock, `ListDelegates` fields, `Sleep` fields, and
acceptance evidence. There are no remaining product questions.

**Change Necessity:** Code change. The existing presentation owner deletes rows
to satisfy `LiveRenderer` height bounds; documentation or a larger numeric
budget cannot retain complete cards. Minimum boundary: presentation output,
`NeoTui` source/surface selection, overflow input routing, and existing
tool-card renderers. No physical terminal source change is necessary.

**Existence Check:** Reuse existing owners. `TranscriptViewport` already owns
row scrolling, the Ctrl+O path already owns alternate-screen rendering with
chrome, and `ToolCallComponent` already owns tool timing/arguments. Add one
small automatic-overflow state inside `NeoTui`; use the current `TerminalFrame`
surface flag and post-isolation backend unchanged. Do not add a second terminal
backend, geometry owner, card mode, timer, parser, or tool contract.

**Architecture Integrity Lens:**

- Invariant: presentation never loses canonical rows; terminal frames never
  exceed physical bounds.
- Canonical owners: `TranscriptPresentation` partitions history/live,
  `NeoTui` chooses a bounded view, post-isolation `InlineTerminal` owns absolute
  geometry/protected insertion/transitions, card components render cards, and
  the interactive controller routes input.
- Responsibility overlap: none. Manual review and automatic overflow are
  logical users of the existing surface flag; neither owns physical geometry.
- Higher-level simplification: delete live-row fitting instead of adding a new
  Card variant or per-card height rules.
- Retirement: delete only fitting types/functions/tests. Retain the
  post-isolation physical API exactly as implemented.
- Verdict: proceed with existing owners.

**Plan-Time Complexity Check:** `presentation.rs` is already pressure-heavy;
Task 1 is net deletion. `app.rs` gains only the automatic viewport state and
selection logic. Physical terminal files gain zero lines. `tool_renderers.rs`
is large but remains the established single owner for special tool
presentation; two narrow helpers are lower entropy than a new module. Do not
extract or refactor unrelated code.

**Plan Pressure Test:** Proceed only from the committed isolation baseline. If
complete source rendering requires changing any output-locked Card,
`screen_output`, raw input, `terminal_io`, physical geometry, scroll margins, or
CPR/resize handling, stop and return to the design rather than adding a fallback.

## Execution Readiness View

- Intent Lock: preserve original Card UI and remove only outer transcript
  omission; add structured `ListDelegates` and countdown `Sleep` presentation.
- Scope Fence: the files listed below; no Card implementation or core tool
  contract changes.
- Baseline Lock: Terminal Live Viewport Isolation is higher priority and must be
  complete; immutable scrollback and Ctrl+O chrome remain active except for the
  explicitly superseded bounded-tail policy.
- Approved Behavior: automatic, complete, bounded, scrollable overflow with
  fixed composer/footer and one physical alternate surface.
- Owner / Contract Constraints: partition in presentation, selection in
  `NeoTui`, unchanged physical geometry/transition in post-isolation
  `InlineTerminal`, input in controller.
- Compatibility Boundary: no omitted rows, no history replay/loss, no nested
  alternate transition, no Card output change.
- Retirement Boundary: remove fitting logic only; retain all post-isolation
  physical names and owners.
- Task Batches: prerequisite check; presentation; automatic viewport; input;
  `ListDelegates`; `Sleep`; final audit.
- Test Obligations: exact presentation, frame, virtual-terminal, controller,
  and tool-card regressions.
- Review Gates: Card Output Lock hashes and no-clear scan before completion.
- Drift / Rewind Rules: if a task needs an output-locked file, core tool,
  `screen_output`, raw input, or `terminal_io` change, stop and revise the plan;
  never revert dirty user files.
- Evidence Required Before Completion: exact passing commands, unchanged Card
  hashes, `rustfmt --check`, targeted `git diff --check`, and one scoped commit.
- Advisory Boundary: method-pack execution guidance only; not completion
  authority.

---

## File Map

**Modify**

- `crates/neo-tui/src/transcript/presentation.rs`
- `crates/neo-tui/src/transcript/pane.rs`
- `crates/neo-tui/src/transcript/tool_call.rs`
- `crates/neo-tui/src/transcript/tool_renderers.rs`
- `crates/neo-tui/src/app.rs`
- `crates/neo-agent/src/modes/interactive/input.rs`
- Focused tests in `crates/neo-tui/tests/terminal_frame.rs`,
  `crates/neo-tui/tests/terminal_scrollback.rs`,
  `crates/neo-tui/tests/transcript_pane.rs`, and
  `crates/neo-agent/src/modes/interactive/tests.rs`

**Do not modify**

- `crates/neo-tui/src/transcript/delegate_card.rs`
- `crates/neo-tui/src/transcript/delegate_group.rs`
- `crates/neo-tui/src/transcript/swarm_card.rs`
- `crates/neo-tui/src/transcript/child_activity.rs`
- `crates/neo-agent-core/src/tools/delegate_controls.rs`
- `crates/neo-agent-core/src/tools/sleep.rs`
- `crates/neo-tui/src/input/raw_input.rs`
- `crates/neo-tui/src/input/mod.rs`
- `crates/neo-tui/src/screen_output/inline_terminal.rs`
- `crates/neo-tui/src/screen_output/live_renderer.rs`
- `crates/neo-tui/src/screen_output/terminal_modes.rs`
- `crates/neo-agent/src/modes/interactive/terminal_io.rs`
- DelegateSwarm estimator/runtime code, provider code, persistence, or docs
  unrelated to the superseded overflow paragraph

Before implementation, record the Card Output Lock hashes:

```bash
git hash-object crates/neo-tui/src/transcript/delegate_card.rs crates/neo-tui/src/transcript/delegate_group.rs crates/neo-tui/src/transcript/swarm_card.rs crates/neo-tui/src/transcript/child_activity.rs
```

Keep that four-line output and require the same command to return identical
hashes in Task 6. This protects current user changes as well as committed code.

---

### Task 0: Confirm The Higher-Priority Isolation Baseline

**Files:** No edits.

**Why:** This plan intentionally depends on the completed absolute-geometry and
protected-history backend. Implementing against the pre-isolation backend would
create avoidable conflicts and invalidate scrollback evidence.

**Change Necessity:** Verification only. Do not cherry-pick, merge, or repair the
isolation work from this plan.

- [ ] **Step 1: Confirm the isolation implementation commit exists**

Inspect recent history and require a committed implementation of
`docs/aegis/plans/2026-07-19-terminal-live-viewport-isolation.md`, not only its
docs commit `12fba5a2`:

```bash
git log -12 --oneline --decorate
git status --short -- crates/neo-tui/src/input/raw_input.rs crates/neo-tui/src/input/mod.rs crates/neo-tui/src/screen_output/inline_terminal.rs crates/neo-tui/src/screen_output/live_renderer.rs crates/neo-tui/src/screen_output/terminal_modes.rs crates/neo-agent/src/modes/interactive/terminal_io.rs crates/neo-tui/tests/terminal_scrollback.rs
```

Expected: the implementation commit is present and the scoped status is empty.
If either condition is false, stop. Do not begin Task 1.

- [ ] **Step 2: Re-run the isolation root regression**

```bash
cargo test --package neo-tui --test terminal_scrollback -- history_commit_never_moves_live_chrome_into_native_scrollback --exact --nocapture
```

Expected: one passing test. A failure belongs to the higher-priority isolation
work and blocks this plan.

---

### Task 1: Emit Complete Live Rows And Overflow Signals

**Files:** Modify `crates/neo-tui/src/transcript/presentation.rs` and
`crates/neo-tui/src/transcript/pane.rs`; update focused tests in
`crates/neo-tui/src/transcript/presentation.rs` and
`crates/neo-tui/tests/transcript_pane.rs`.

**Why:** Stop deleting Card bodies at the root cause while retaining the
canonical history/live frontier.

**Change Necessity:** `fit_live_blocks` is the only owner producing omitted and
header-only output. Removing it is the minimum stable repair.

**Impact/Compatibility:** History partitioning, spacing, atomic block rendering,
animation detection, and acknowledgement remain unchanged. The returned live
vector may exceed the physical body budget; only `NeoTui` may viewport it.

**Repair Track:** Return every `LiveBlock.lines` row in order, calculate
`live_overflow`, and expose whether `commit_blocked` established a live
frontier.

**Retirement Track:** Delete `FittedLine`, `FittedLiveBlock`,
`fit_live_blocks`, `omission_line`, their imports, and fitting/truncation tests.
Keep no renamed wrapper or compatibility branch.

- [ ] **Step 1: Extend the terminal update contract**

Add these defaulted fields to `TranscriptTerminalUpdate`:

```rust
pub live_overflow: bool,
pub has_live_frontier: bool,
```

At the end of `TranscriptPresentation::render`, flatten complete `LiveBlock`
rows with the current semantic spacing, set
`live_overflow = live.len() > live_budget`, set `has_live_frontier` from the
canonical commit-blocking state, and keep `has_visible_animation` true only
when an animated row is actually part of the complete live source.

- [ ] **Step 2: Replace truncation tests with one complete-source regression**

Add
`transcript::presentation::tests::live_overflow_preserves_complete_rows`.
Construct a living block followed by enough deferred rows to exceed the budget;
assert every sentinel remains in order, `live_overflow` and
`has_live_frontier` are true, and no row contains `earlier rows omitted`.

Update the existing integration case
`long_unstable_assistant_tail_is_truncated_inside_live_budget` to
`long_unstable_assistant_tail_reports_overflow_without_omission`; assert the
full source is returned and overflow is signaled rather than asserting a
bounded `update.live`.

- [ ] **Step 3: Run exact presentation verification**

```bash
cargo test --package neo-tui --lib -- transcript::presentation::tests::live_overflow_preserves_complete_rows --exact --nocapture --include-ignored
cargo test --package neo-tui --test transcript_pane -- long_unstable_assistant_tail_reports_overflow_without_omission --exact --nocapture
```

Expected: each command reports one passing test.

- [ ] **Step 4: Commit the root-cause repair**

```bash
git add crates/neo-tui/src/transcript/presentation.rs crates/neo-tui/src/transcript/pane.rs crates/neo-tui/tests/transcript_pane.rs
git commit -m "fix(tui): preserve complete live transcript rows"
```

---

### Task 2: Add The Latched Automatic Overflow Viewport

**Files:** Modify `crates/neo-tui/src/app.rs` and
`crates/neo-tui/src/transcript/pane.rs`; test
`crates/neo-tui/tests/terminal_frame.rs` and
`crates/neo-tui/tests/terminal_scrollback.rs`.

**Why:** Bound the physical frame without changing or omitting the canonical
Card source.

**Change Necessity:** `LiveRenderer` cannot accept over-height frames. The
existing alternate-screen plus `TranscriptViewport` is the minimum mechanism
that can show all rows over time while keeping chrome visible.

**Impact/Compatibility:** Normal fitting sessions retain the primary path.
Overflow frames contain no history acknowledgement. History accumulated while
overflow is active is retried after return and appended once through the
post-isolation protected history region. Absolute geometry, scroll margins,
CPR/resize generations, and renderer transactions remain untouched.

**Repair Track:** Add an optional automatic `TranscriptViewport` to `NeoTui`.
`None` means no latch; `Some` means automatic overflow is latched. Reuse the
existing viewport type and chrome composition.

**Retirement Track:** No secondary full-screen renderer, viewport type, or
height configuration is introduced.

- [ ] **Step 1: Add source-preserving viewport rendering**

Add this method beside `render_browser_rows`:

```rust
pub fn render_viewport_rows(
    &mut self,
    viewport: &mut TranscriptViewport,
    width: usize,
    height: usize,
) -> Vec<String>
```

Render from a clone, preserve the clone's current expansion state, synchronize
the supplied viewport, and return its visible range. Do not call
`set_tool_output_expanded`, mutate the real pane, consume `dirty`, or
acknowledge history.

- [ ] **Step 2: Select and latch automatic overflow in `NeoTui`**

After fitting chrome, always obtain the normal `TranscriptTerminalUpdate`.
Enter the latch when `update.live_overflow` is true. Release it only when
`update.has_live_frontier` is false. While latched, render
`render_viewport_rows` into the body budget, append normal chrome, return an
alternate-screen frame through the existing `TerminalFrame::with_surface`
boolean, and keep `update.history` unacknowledged. Reuse the current
`NeoTui::acknowledge_history` surface guard; do not change physical output code.

When manual `TranscriptBrowserState` exists, render it instead of the automatic
viewport but retain the automatic latch. A logical switch between these owners
must keep the existing surface flag true. The unchanged post-isolation
`InlineTerminal` emits only the first/last physical transition.

Expose narrow `NeoTui` methods for `automatic_overflow_active`, scroll up, and
scroll down. Do not expose the viewport or add state to `NeoChromeState`.

- [ ] **Step 3: Add bounded-frame and precedence regressions**

Add
`automatic_transcript_overflow_is_bounded_and_preserves_source_and_chrome` to
`terminal_frame.rs`. Use a short terminal and a long live suffix; assert the
latch is active, the existing surface flag is true, frame height/cursor are
bounded, chrome remains visible, source sentinels become reachable through
viewport scrolling, and no omission marker exists.

Add `manual_review_reuses_latched_automatic_alternate_surface` and assert the
logical owner can change without changing the physical surface flag.

- [ ] **Step 4: Add virtual-terminal history regression**

Add
`automatic_overflow_preserves_primary_scrollback_and_appends_deferred_history_once`.
Build on the completed isolation harness and its current constructor/geometry
API. Prove pre-existing shell and finalized sentinels remain once and ordered,
obsolete live sentinels occur zero times across complete scrollback, one
`?1049h` and one `?1049l` are emitted, history finalized during overflow appears
exactly once after release, and neither `CSI 2 J` nor `CSI 3 J` occurs. Do not
edit `InlineTerminal`, `LiveRenderer`, raw input, terminal modes, or
`terminal_io` to make this test pass.

- [ ] **Step 5: Run exact viewport verification**

```bash
cargo test --package neo-tui --test terminal_frame -- automatic_transcript_overflow_is_bounded_and_preserves_source_and_chrome --exact --nocapture
cargo test --package neo-tui --test terminal_frame -- manual_review_reuses_latched_automatic_alternate_surface --exact --nocapture
cargo test --package neo-tui --test terminal_scrollback -- automatic_overflow_preserves_primary_scrollback_and_appends_deferred_history_once --exact --nocapture
```

Expected: each command reports one passing test.

- [ ] **Step 6: Commit the automatic viewport**

```bash
git add crates/neo-tui/src/app.rs crates/neo-tui/src/transcript/pane.rs crates/neo-tui/tests/terminal_frame.rs crates/neo-tui/tests/terminal_scrollback.rs
git commit -m "feat(tui): viewport overflowing live transcripts"
```

---

### Task 3: Route Overflow Scrolling Without Blocking The Composer

**Files:** Modify `crates/neo-agent/src/modes/interactive/input.rs` and
`crates/neo-agent/src/modes/interactive/tests.rs`.

**Why:** Overflow must remain navigable without turning the composer into a
modal or changing ordinary editing/submission.

**Change Necessity:** Only the interactive controller can decide whether a
wheel/page event belongs to the automatic viewport or the existing prompt and
global handlers.

**Impact/Compatibility:** Manual transcript-browser routing keeps logical
precedence. Automatic mode consumes only wheel, PageUp, and PageDown. Insert,
paste, cursor movement, submit, interrupt, suspend, exit, and other global
actions follow current paths.

**Repair Track:** Add `handle_automatic_overflow_event` after manual browser
handling and before prompt edit handling. Translate wheel and page actions to
the narrow `NeoTui` scroll methods and mark transcript dirty.

**Retirement Track:** Do not add an overlay, dialog priority, duplicated
keybinding table, or alternate composer path.

- [ ] **Step 1: Implement the narrow router**

Return `true` only for `ScrollUp`, `ScrollDown`, `EditorPageUp`,
`EditorPageDown`, `SelectPageUp`, `SelectPageDown`, and their configured key
matches while automatic overflow is active and manual review is absent. Use
eight rows for page actions, matching the existing transcript browser.

- [ ] **Step 2: Add one controller workflow regression**

Add `automatic_transcript_overflow_scrolls_without_blocking_prompt`. Trigger
overflow through the real TUI state, assert PageUp changes the automatic
viewport, insert text into the prompt, submit it, and assert the turn is
dispatched. Also assert Ctrl+O manual review temporarily takes precedence and
closing it restores automatic overflow while latched.

- [ ] **Step 3: Run exact controller verification**

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::automatic_transcript_overflow_scrolls_without_blocking_prompt --exact --nocapture --include-ignored
```

Expected: one passing test.

- [ ] **Step 4: Commit the input routing**

```bash
git add crates/neo-agent/src/modes/interactive/input.rs crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "feat(tui): route automatic transcript overflow input"
```

---

### Task 4: Render Structured `ListDelegates` Results

**Files:** Modify `crates/neo-tui/src/transcript/tool_renderers.rs`; test
`crates/neo-tui/tests/transcript_pane.rs`.

**Why:** Replace opaque generic output with the structured agent/swarm snapshot
the tool already supplies.

**Change Necessity:** The core result already contains canonical fields. Only
the existing TUI tool renderer lacks a `delegate_list` presentation branch.

**Impact/Compatibility:** The model still receives unchanged content/details,
including pagination data. The user sees count/total and returned rows, never
the opaque cursor. Malformed or unknown details use the existing generic body.

**Repair Track:** Add narrow helpers that recognize
`details.kind == "delegate_list"`, append an `N of M` header chip, and render
the `delegates` array in returned order. Agent rows use `display_name`, `status`,
and `title`; swarm rows use `description`, `status`, and an aggregate child row.

**Retirement Track:** Do not parse `ToolResult.content`, duplicate core structs,
or expose `next_cursor` / `cursor_query`.

- [ ] **Step 1: Add header and body helpers**

Call the header helper from `tool_header_spans`. Call the body helper before
`render_result_body` in `render_tool_body_with_palette`. Reuse `ToolBodyPalette`,
existing tree glyph conventions, `one_line`, and width truncation. For a valid
empty list, render `No delegates found`; render structured `next_steps` only
when present.

- [ ] **Step 2: Add one mixed agent/swarm regression**

Add `list_delegates_renders_structured_rows_without_opaque_cursor`. Feed a
succeeded `ListDelegates` event with `kind`, `count`, `total`, `next_cursor`, one
agent, and one swarm aggregate. Assert count/total, both tree rows, agent title,
swarm aggregate, and absence of the cursor and raw `next_cursor:` text.

- [ ] **Step 3: Run exact result verification**

```bash
cargo test --package neo-tui --test transcript_pane -- list_delegates_renders_structured_rows_without_opaque_cursor --exact --nocapture
```

Expected: one passing test.

- [ ] **Step 4: Commit the structured result renderer**

```bash
git add crates/neo-tui/src/transcript/tool_renderers.rs crates/neo-tui/tests/transcript_pane.rs
git commit -m "feat(tui): render structured delegate lists"
```

---

### Task 5: Render `Sleep` Duration, Countdown, And Reason

**Files:** Modify `crates/neo-tui/src/transcript/tool_call.rs` and
`crates/neo-tui/src/transcript/tool_renderers.rs`; test
`crates/neo-tui/tests/transcript_pane.rs` and
`crates/neo-tui/tests/terminal_frame.rs`.

**Why:** Make a running wait truthful and inspectable without repeating its
generic completion sentence.

**Change Necessity:** `ToolCallComponent` already owns the start instant and
arguments. Only its render/animation eligibility lacks Sleep semantics.

**Impact/Compatibility:** Core sleeping, cancellation, validation, result text,
and shell independence remain unchanged. Failed/cancelled result content is not
suppressed.

**Repair Track:** Parse `duration_seconds` and `reason` from the existing
arguments with `serde_json::Value`. Format total and saturating remaining time
through existing duration formatting. Append the running countdown from
`streaming_started_at`, render reason as the semantic body, and treat running
Sleep as visibly animated.

**Retirement Track:** Suppress only the successful generic `Waited ...` body.
Do not add another `Instant`, interval, async task, or core result field.

- [ ] **Step 1: Add semantic Sleep rendering**

Add a narrow argument parser/helper in `tool_renderers.rs`. In
`ToolCallComponent::render_with_theme`, use the parsed total and existing
`streaming_started_at` to append `<total> total` and, while running,
`<remaining> remaining`. Route valid Sleep bodies to the reason renderer before
the generic result path. On successful Sleep, return only the reason body; on
failure/cancellation retain existing generic error content after the reason.

- [ ] **Step 2: Make the countdown animation-eligible**

Extend `ToolCallComponent::has_visible_animation` so a pending/running `Sleep`
with `streaming_started_at` requests the existing animation deadline. Do not
change `ANIMATION_INTERVAL` or add a Sleep-specific scheduler.

- [ ] **Step 3: Add semantic and deadline regressions**

Add `sleep_renders_total_remaining_and_reason_without_duplicate_result` to
`transcript_pane.rs`. Assert a running card contains total, remaining, and
reason; after success it retains total/reason and excludes `Waited`.

Add `running_sleep_requests_animation_deadline` to `terminal_frame.rs`; assert
the running tool requests a deadline and the completed tool does not.

- [ ] **Step 4: Run exact Sleep verification**

```bash
cargo test --package neo-tui --test transcript_pane -- sleep_renders_total_remaining_and_reason_without_duplicate_result --exact --nocapture
cargo test --package neo-tui --test terminal_frame -- running_sleep_requests_animation_deadline --exact --nocapture
```

Expected: each command reports one passing test.

- [ ] **Step 5: Commit the Sleep renderer**

```bash
git add crates/neo-tui/src/transcript/tool_call.rs crates/neo-tui/src/transcript/tool_renderers.rs crates/neo-tui/tests/transcript_pane.rs crates/neo-tui/tests/terminal_frame.rs
git commit -m "feat(tui): show sleep countdown and reason"
```

---

### Task 6: Verify Card Output Lock And Terminal Invariants

**Files:** No source edits unless a focused regression exposes an in-scope bug.

**Why:** Prove the outer overflow fix did not redesign Cards, clear native
history, or broaden the workstream.

**Change Necessity:** Verification only. Any source edit discovered here must
return to its owning task and repeat that task's exact test.

**Repair Track:** None.

**Retirement Track:** Confirm no fitted-live type, omission helper,
compatibility path, second renderer, or second geometry owner remains. Existing
post-isolation physical names are explicitly retained.

- [ ] **Step 1: Compare Card Output Lock hashes**

Run the same command captured before Task 1 and require identical four-line
output:

```bash
git hash-object crates/neo-tui/src/transcript/delegate_card.rs crates/neo-tui/src/transcript/delegate_group.rs crates/neo-tui/src/transcript/swarm_card.rs crates/neo-tui/src/transcript/child_activity.rs
```

- [ ] **Step 2: Run representative unchanged Card tests**

```bash
cargo test --package neo-tui --test multi_agent_transcript -- delegate_group_styles_header_names_muted_tree_and_role_badges --exact --nocapture
cargo test --package neo-tui --test multi_agent_transcript -- expanded_swarm_child_uses_delegate_activity_rules --exact --nocapture
```

Expected: each command reports one passing test without updating snapshots or
expected Card output.

- [ ] **Step 3: Check retirement and terminal safety**

```bash
rg -n "FittedLine|FittedLiveBlock|fit_live_blocks|omission_line|earlier rows omitted" crates/neo-tui/src/transcript crates/neo-tui/tests
rg -n "\\x1b\\[2J|\\x1b\\[3J" crates/neo-tui/src/screen_output
```

Expected: both searches return no matches. Do not require or perform any
physical surface rename.

- [ ] **Step 4: Check formatting and scoped diff hygiene**

```bash
rustfmt --check --edition 2024 crates/neo-tui/src/transcript/presentation.rs crates/neo-tui/src/transcript/pane.rs crates/neo-tui/src/transcript/tool_call.rs crates/neo-tui/src/transcript/tool_renderers.rs crates/neo-tui/src/app.rs crates/neo-agent/src/modes/interactive/input.rs
git diff --check -- crates/neo-tui/src/transcript/presentation.rs crates/neo-tui/src/transcript/pane.rs crates/neo-tui/src/transcript/tool_call.rs crates/neo-tui/src/transcript/tool_renderers.rs crates/neo-tui/src/app.rs crates/neo-tui/tests/terminal_frame.rs crates/neo-tui/tests/terminal_scrollback.rs crates/neo-tui/tests/transcript_pane.rs crates/neo-agent/src/modes/interactive/input.rs crates/neo-agent/src/modes/interactive/tests.rs
```

Expected: both commands exit zero. If unrelated dirty files prevent formatting,
report that separately and retain the exact focused test evidence.

- [ ] **Step 5: Review architecture and commit state**

Confirm normal inline mode is still the default, the post-isolation physical
owner is unchanged, history acknowledgement is skipped on alternate-screen
frames, and no output-locked file or core tool changed. If all task commits are
already present, do not add an empty final commit.

## Risks And Rollback Surface

- **Latch never releases:** the frontier signal must come from canonical
  commit-blocking state, not merely `live.is_empty()` or current height.
- **History loss/duplication:** alternate frames must never acknowledge pending
  history; returning to primary must use the existing two-phase ledger.
- **Nested alternate transitions:** the existing physical surface flag remains
  true while logical manual/automatic owners switch; no physical owner is added.
- **Prompt regression:** automatic routing consumes only wheel/page events and
  remains below manual-review/global priority.
- **Card drift:** hash lock and unchanged representative tests guard the four
  output owners.
- **Malformed structured data:** `ListDelegates` and `Sleep` helpers fall back to
  existing generic rendering; they do not invent parsed values.

The rollback surface is the automatic viewport selection and the two special
tool render branches. Do not restore bounded fitting or omitted-row output as a
fallback; if viewport behavior fails, fix `NeoTui` source/surface selection and
do not patch the post-isolation terminal backend.

## Spec Coverage

- Complete Card output and no omitted rows: Tasks 1, 2, and 6.
- Automatic bounded viewport, latch, fixed chrome, and source preservation:
  Task 2.
- Manual Ctrl+O physical sharing: Task 2.
- Scrollable overflow with editable/submittable composer: Task 3.
- Preserved scrollback and history exactly once: Tasks 0, 2, and 6.
- Structured `ListDelegates`: Task 4.
- `Sleep` total, countdown, reason, and duplicate suppression: Task 5.
- Card UI unchanged: preflight hash plus Task 6.
