# Terminal Live Viewport Isolation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `aegis:subagent-driven-development` or `aegis:executing-plans` to execute this
> plan task by task. Do not re-open product/card design; the approved spec is the
> authority.

**Goal:** Prevent mutable Neo chrome from entering native terminal scrollback by
making `InlineTerminal` own absolute live geometry and inserting finalized
history only above that viewport.

**Architecture:** Preserve the current transcript presentation and bounded live
frame. Replace the terminal backend's relative cursor anchor and unrestricted
CRLF history append with one absolute normal-screen geometry owner, a bounded
cursor-position observation path, and a protected history scroll region. Retire
the old path in the same change.

**Tech Stack:** Rust 2024, crossterm 0.29, existing raw stdin parser, vt100 0.16
test harness, existing `neo-tui` and `neo-agent` crates.

**Baseline/Authority Refs:**

- `docs/aegis/specs/2026-07-19-terminal-live-viewport-isolation-design.md`
- `docs/aegis/specs/2026-07-13-immutable-terminal-scrollback-design.md`
- `docs/aegis/specs/2026-07-17-ctrl-o-review-chrome-design.md`
- `docs/aegis/specs/2026-07-19-transcript-overflow-tool-results-design.md`
- `AGENTS.md`

**Compatibility Boundary:** Preserve native scrollback, shell history,
presentation acknowledgement, request-driven rendering, Ctrl+O alternate-screen
review, automatic overflow, Kitty image ownership, current card/chrome layout,
and Windows/Linux/macOS support. Add no dependency, setting, feature flag,
Ghostty special case, synthetic scrollback, or compatibility renderer.

**TDD Route:**

- Mode: off
- Decision: skipped
- Strict authority: not applicable
- Test posture: diagnostic reproduction followed by post-change regression
- Reason: strict TDD was not requested; one complete-scrollback reproduction is
  the minimum evidence that distinguishes the bug from visible-screen output.
- Verification: exact commands are listed per task.

**Verification:** Every Cargo command names one package, one target, and one
exact test. Run commands sequentially through `rtk`; do not run workspace-wide
tests.

---

## Scope Check

**Requirement Ready Check:** Ready. The approved design fixes the root-cause
owner, preserved history/card behavior, resize/probe policy, old-path deletion,
and acceptance evidence.

**Change Necessity:** Code change. Documentation or a terminal setting cannot
prevent unrestricted CRLF from scrolling mutable rows. Minimum boundary:
terminal input geometry observation, `InlineTerminal`, `LiveRenderer`, and their
focused vt100 regressions.

**Existence Check:** Add only private terminal geometry/probe state required to
connect the existing raw stdin owner to the existing `InlineTerminal` owner.
Reuse `InputParser`, crossterm, `TerminalFrame`, and vt100. Add no general
viewport abstraction or dependency.

**Architecture Integrity Lens:** `TranscriptPresentation` decides history vs
live; `InlineTerminal` owns physical geometry; `LiveRenderer` diffs rows within
the supplied absolute viewport. The old relative cursor and bottom-anchored
history writer are duplicate physical owners and must be deleted. Verdict:
proceed.

**Anti-Entropy Declaration:**

- Deletion class: internal code retirement
- Old path: relative `hardware_cursor_row`, `fresh_anchor_pending`, inferred
  live clear, unrestricted `append_history_lines`
- New canonical owner: absolute geometry and protected insertion in
  `InlineTerminal`
- Preserved behavior: append-only history, bounded live UI, current card/chrome
  semantics, review, images, lifecycle
- Retired behavior: abandoning or scrolling mutable rows into native history
- External boundary touched: terminal protocol only
- Source-of-truth data risk: none
- User confirmation required: no
- Retirement decision: delete-first

**Plan Pressure Test:** Stop and revise the spec if implementation requires a
TranscriptStore/presentation lifecycle change, card layout change, second
renderer, scrollback purge, terminal-specific compatibility strategy, or new
dependency.

---

## File Map

**Modify**

- `crates/neo-tui/src/input/raw_input.rs`
- `crates/neo-tui/src/input/mod.rs`
- `crates/neo-tui/src/screen_output/inline_terminal.rs`
- `crates/neo-tui/src/screen_output/live_renderer.rs`
- `crates/neo-tui/tests/terminal_scrollback.rs`
- `crates/neo-agent/src/modes/interactive/terminal_io.rs`

**Modify only if an exact existing regression requires it**

- `crates/neo-tui/src/screen_output/terminal_modes.rs` for unconditional scroll
  margin cleanup using the existing RAII owner.
- `crates/neo-agent/src/modes/interactive/tests.rs` for the existing resize
  event-loop regression, without changing controller behavior.

**Do not modify**

- transcript store, presentation, replay, semantic spacing, or frame scheduler;
- Todo/composer/footer/dialog renderers;
- Delegate, DelegateGroup, DelegateSwarm, Workflow, tool, thinking, or assistant
  card components; or
- automatic overflow and Ctrl+O browser layout/interaction owners.

---

### Task 1: Turn The Screenshot Into A Complete-Scrollback Regression

**Files:** Modify `crates/neo-tui/tests/terminal_scrollback.rs`.

**Why:** The current visible-only assertion cannot detect live chrome already
stored above the visible screen.

**Change Necessity:** This test target already owns the vt100 normal-screen and
scrollback lifecycle harness. No new fixture or test target is needed.

**Impact/Compatibility:** Test-only. Preserve existing shell/history retention
assertions and reuse `all_terminal_rows`.

**Repair Track:** Reproduce obsolete live rows across history commit and a
cursor-affecting resize generation.

**Retirement Track:** Replace visible-only ghost assertions; do not retain a
second weaker test that proves only the current screen.

- [ ] **Step 1: Strengthen the existing ghost-row test**

Rename it to
`history_commit_never_moves_live_chrome_into_native_scrollback`. Seed more than
one screen of shell rows, render unique markers
`obsolete-todo-sentinel`/`obsolete-composer-sentinel`, perform a height resize
through `resize_vt100` and `InlineTerminal::resize`, commit enough finalized
rows to force scrolling, then render current live markers.

Use `all_terminal_rows(&mut screen)` for every absence/count/order assertion:

```rust
assert!(
    retained.iter().all(|row| {
        !row.contains("obsolete-todo-sentinel")
            && !row.contains("obsolete-composer-sentinel")
    }),
    "obsolete live chrome entered native scrollback: {retained:#?}"
);
assert_eq!(
    retained
        .iter()
        .filter(|row| row.contains("current-composer-sentinel"))
        .count(),
    1,
    "current composer must appear exactly once: {retained:#?}"
);
```

Also assert the first/last shell sentinels and each newly committed sentinel
remain exactly once.

- [ ] **Step 2: Run the diagnostic reproduction**

```bash
rtk cargo test --package neo-tui --test terminal_scrollback -- history_commit_never_moves_live_chrome_into_native_scrollback --exact --nocapture
```

Expected before repair: failure showing an obsolete Todo/composer marker in
`all_terminal_rows`. If it does not fail, stop; capture the emitted transaction
and adjust only the event sequence to match the observed Ghostty commit/resize
sequence before editing production code.

---

### Task 2: Keep Cursor Reports Out Of Prompt Input

**Files:**

- Modify `crates/neo-tui/src/input/raw_input.rs`
- Modify `crates/neo-tui/src/input/mod.rs`
- Modify `crates/neo-agent/src/modes/interactive/terminal_io.rs`
- Test in the same files

**Why:** Absolute resize geometry requires cursor-position reports, but Neo's
background raw stdin reader currently treats every complete CSI sequence as a
user key.

**Change Necessity:** The existing raw stdin owner must consume CPR replies so
no second stdin reader races with prompt input.

**Impact/Compatibility:** Ordinary key, paste, Kitty keyboard, mouse-wheel, and
resize events retain ordering. Windows continues to use crossterm's console
cursor query.

**Repair Track:** Add one internal CPR event and one size-generation observation
shared with `NeoTerminal`.

**Retirement Track:** No second input reader, crossterm event loop, global
cursor cache, or leaked `InputEvent` variant remains.

- [ ] **Step 1: Parse CPR as terminal protocol state**

Add this raw event:

```rust
CursorPosition { column: u16, row: u16 },
```

In `RawInputParser::emit_data_sequence`, recognize only the complete
one-based `ESC [ <row> ; <column> R` form, convert both coordinates with
`saturating_sub(1)`, and emit `CursorPosition`. Use `strip_prefix`,
`strip_suffix`, `split_once`, and `parse::<u16>()`; add no regex.

Add `cursor_positions: VecDeque<(u16, u16)>` to `InputParser`. Its
`convert_raw_event` stores CPR values and returns no `InputEvent`. Expose:

```rust
pub fn take_cursor_position(&mut self) -> Option<(u16, u16)>;
```

- [ ] **Step 2: Add chunking and input-isolation coverage**

Add `cursor_position_report_is_internal_and_chunk_safe`. Feed the CPR in two
chunks, assert no prompt event is returned, and assert exactly one zero-based
position. Then feed `x` and assert the normal insert event remains.

- [ ] **Step 3: Associate resize with an observed cursor generation**

Keep a private, cloneable geometry observation in `terminal_io.rs`, shared only
between `RawStdinEvents` and `NeoTerminal`. It stores terminal size, absolute
cursor, and a monotonically increasing generation.

On Unix, when `RawStdinEvents` detects a new size, write `CSI 6 n`, keep
unrelated input queued, and defer the `InputEvent::Resize` until the matching CPR
has updated the shared generation. Bound the probe with the same two-second
limit used by crossterm cursor queries. On timeout or malformed reply, return a
typed I/O error. On Windows, read the cursor through `crossterm::cursor::position`
and publish the same observation without writing CPR.

Seed the initial observation in `NeoTerminal::enter` before constructing the
background stdin reader. Pass one clone to the event factory and one to
`NeoTerminal`; add no process-global state.

- [ ] **Step 4: Run exact input verification**

```bash
rtk cargo test --package neo-tui --lib -- input::tests::cursor_position_report_is_internal_and_chunk_safe --exact --nocapture
rtk cargo test --package neo-agent --bin neo -- modes::interactive::terminal_io::tests::terminal_resize_waits_for_matching_cursor_generation --exact --nocapture --include-ignored
```

Expected: one passing test per command.

---

### Task 3: Replace Relative Live Anchoring With Absolute Geometry

**Files:**

- Modify `crates/neo-tui/src/screen_output/inline_terminal.rs`
- Modify `crates/neo-tui/src/screen_output/live_renderer.rs`
- Modify `crates/neo-agent/src/modes/interactive/terminal_io.rs`
- Test `crates/neo-tui/tests/terminal_scrollback.rs`

**Why:** This is the canonical owner of the terminal bytes that currently leak
mutable chrome into scrollback.

**Change Necessity:** Caller-side guards or terminal detection cannot constrain
the default scroll domain. The physical transaction must change here.

**Impact/Compatibility:** `TerminalFrame`, history acknowledgement, card/chrome
rendering, synchronized-output detection, and image IDs remain unchanged.

**Repair Track:** Store absolute geometry in `InlineTerminal`, render live rows
at its supplied origin, and insert history inside a protected scroll region.

**Retirement Track:** Delete `hardware_cursor_row`, `fresh_anchor_pending`, the
relative clear path, and unrestricted `append_history_lines`. Keep no aliases.

- [ ] **Step 1: Make absolute geometry an `InlineTerminal` invariant**

Pass the initial zero-based cursor to `InlineTerminal::enter/new`. Store screen
width/height, absolute live top, absolute hardware cursor, and geometry
generation directly on `InlineTerminal`; do not create a public geometry API.

Change resize input to require the matching observed cursor and generation.
Reject out-of-bounds or stale observations with `InvalidData` rather than
clamping or guessing.

- [ ] **Step 2: Make `LiveRenderer` origin-neutral**

Pass an absolute `origin_row` and absolute hardware cursor into `render_to` and
the clear operation. Use absolute cursor addressing before changed rows and the
logical cursor. Preserve line diffing, width/height validation, cursor
visibility, and Kitty image deletion.

Delete `hardware_cursor_row`, `fresh_anchor_pending`, and relative
`push_vertical_move`. Width/height changes invalidate cached rows but do not
emit CRLF to establish a new anchor.

- [ ] **Step 3: Insert history above the live viewport**

Replace `append_history_lines` with one private protected promotion helper.
After clearing the old live rows, emit:

```text
CSI 1;<height> r
CSI <live_top + 1>;1 H
for each history line: clear row + line + CRLF
CSI r
```

Remember that ANSI coordinates are one-based while stored geometry is
zero-based. Advance `live_top` by the promoted rows and full-screen scroll only
when the promoted history plus the new live suffix crosses the physical bottom.
Clear the prior live viewport before any scroll, restore the absolute live
cursor after resetting margins, and redraw the complete live frame. Never clear
a committed row or scroll unused live capacity into history.

Keep history insertion, live redraw, cursor restoration, synchronized-output
end marker, and flush in one transaction. Commit geometry/cache state only
after the flush succeeds. On write failure, best-effort emit `CSI r` before
returning the original error.

- [ ] **Step 4: Reconcile live height without scrolling populated live rows**

When live height grows, make room above the viewport before drawing; when it
shrinks, clear released rows before changing the viewport. Use absolute
geometry and scroll the full screen only after mutable rows are cleared and
only by the required overflow. Do not add a `fresh anchor` branch.

- [ ] **Step 5: Run the root regression**

```bash
rtk cargo test --package neo-tui --test terminal_scrollback -- history_commit_never_moves_live_chrome_into_native_scrollback --exact --nocapture
```

Expected: one passing test; obsolete live sentinels occur zero times across
visible rows plus full scrollback.

---

### Task 4: Preserve Review, Suspend, Resume, Exit, And Error Cleanup

**Files:**

- Modify `crates/neo-tui/src/screen_output/inline_terminal.rs`
- Modify `crates/neo-tui/src/screen_output/terminal_modes.rs` only if required
- Modify `crates/neo-agent/src/modes/interactive/terminal_io.rs`
- Test `crates/neo-tui/tests/terminal_scrollback.rs`

**Why:** These transitions can replace the physical screen or invalidate its
cursor; all must restore the same canonical absolute owner.

**Change Necessity:** The old lifecycle methods reset only relative renderer
state. They must now preserve or reacquire absolute geometry and reset margins.

**Impact/Compatibility:** Alternate-screen review, Ctrl+Z behavior, final exit
message, raw mode, bracketed paste, Kitty keyboard, and cursor visibility remain
unchanged.

**Repair Track:** Snapshot normal geometry for review, reacquire geometry after
resume/changed review size, and reset margins on every exit path.

**Retirement Track:** Delete lifecycle calls that invoke the old relative clear
or reset owner.

- [ ] **Step 1: Update normal/alternate-screen transitions**

Keep the saved normal `LiveRenderer` together with its absolute geometry.
Leaving review restores that state, applies a newer geometry observation when
the screen changed, inserts only pending normal history, and redraws live.

- [ ] **Step 2: Update suspend/resume and exit**

Suspend clears the absolute live viewport and resets scroll margins before
leaving terminal modes. Resume obtains a new cursor observation before calling
the first redraw. Exit clears only live-owned rows, moves below final output,
resets margins, shows the cursor, and restores modes.

- [ ] **Step 3: Keep cleanup transactional**

On write/flush/probe failure, retain unacknowledged history and previous
renderer state. Best-effort reset margins and terminal modes without replacing
the original error or emitting `CSI 2 J`/`CSI 3 J`.

- [ ] **Step 4: Run exact lifecycle regressions**

```bash
rtk cargo test --package neo-tui --test terminal_scrollback -- suspend_resume_preserves_committed_history --exact --nocapture
rtk cargo test --package neo-tui --test terminal_scrollback -- committed_tool_review_does_not_duplicate_native_scrollback --exact --nocapture
rtk cargo test --package neo-tui --test terminal_scrollback -- leaving_review_appends_history_finalized_while_browser_was_open --exact --nocapture
rtk cargo test --package neo-tui --test terminal_scrollback -- leave_clears_obsolete_live_and_places_cursor_below_final_output --exact --nocapture
```

Expected: one passing test per command.

---

### Task 5: Verify Retirement And Commit One Logical Fix

**Files:** Verify and commit only files listed in this plan that actually
changed.

**Why:** A green visible frame is insufficient; completion requires proof that
the old owner died and complete scrollback stays clean.

**Change Necessity:** Verification and commit are required by `AGENTS.md` for a
completed implementation task.

**Impact/Compatibility:** No broad workspace checks and no unrelated dirty files
enter the commit.

**Repair Track:** Run the root, input, and neighboring lifecycle evidence.

**Retirement Track:** Search for every retired field/helper and inspect staged
diff for compatibility branches.

- [ ] **Step 1: Run the neighboring long lifecycle regression**

```bash
rtk cargo test --package neo-tui --test terminal_scrollback -- shell_and_committed_history_survive_live_updates_resize_and_exit --exact --nocapture
```

Expected: one passing test.

- [ ] **Step 2: Prove the old owner is gone**

```bash
rtk rg -n "hardware_cursor_row|fresh_anchor_pending|append_history_lines" crates/neo-tui/src/screen_output
```

Expected: no matches.

- [ ] **Step 3: Check touched Rust formatting and diff hygiene**

```bash
rtk rustfmt --check --edition 2024 crates/neo-tui/src/input/raw_input.rs crates/neo-tui/src/input/mod.rs crates/neo-tui/src/screen_output/inline_terminal.rs crates/neo-tui/src/screen_output/live_renderer.rs crates/neo-agent/src/modes/interactive/terminal_io.rs crates/neo-tui/tests/terminal_scrollback.rs
rtk git diff --check -- crates/neo-tui/src/input/raw_input.rs crates/neo-tui/src/input/mod.rs crates/neo-tui/src/screen_output/inline_terminal.rs crates/neo-tui/src/screen_output/live_renderer.rs crates/neo-agent/src/modes/interactive/terminal_io.rs crates/neo-tui/tests/terminal_scrollback.rs
```

Expected: both commands exit zero.

- [ ] **Step 4: Review the exact implementation scope**

Confirm the diff contains no changes to card/chrome layout, transcript
presentation/replay, automatic overflow, terminal capability detection,
dependency manifests, or unrelated worktree files.

- [ ] **Step 5: Commit the verified fix**

Stage only the actual implementation files and commit:

```bash
rtk git add crates/neo-tui/src/input/raw_input.rs crates/neo-tui/src/input/mod.rs crates/neo-tui/src/screen_output/inline_terminal.rs crates/neo-tui/src/screen_output/live_renderer.rs crates/neo-tui/tests/terminal_scrollback.rs crates/neo-agent/src/modes/interactive/terminal_io.rs
rtk git diff --cached --check
rtk git commit -m "fix(tui): isolate live viewport from scrollback"
```

Add `terminal_modes.rs` or the focused interactive test file only if Task 4
actually changed it and its exact regression passed. Do not use `git add .`.

---

## Execution Readiness

- Requirements: ready and approved in the linked Design Spec.
- Canonical owner: `InlineTerminal` absolute geometry and protected insertion.
- TDD route: skipped; diagnostic reproduction plus post-change exact tests.
- Compatibility: no fallback renderer, setting, dependency, or card change.
- Retirement: delete-first for every relative-anchor/unrestricted-CRLF owner.
- Stop condition: complete scrollback contains no obsolete live rows; current
  live and committed/shell history each occur exactly once; lifecycle exact
  regressions pass; old owner search is empty.
- Rewind condition: any required transcript lifecycle/card change, inability to
  associate resize with cursor generation, or terminal-specific fallback
  returns execution to the Design Spec before further source edits.

