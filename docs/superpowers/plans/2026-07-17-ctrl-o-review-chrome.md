# Ctrl+O Review Chrome Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep Neo's composer and cursor usable when Ctrl+O reviews committed expanded transcript content.

**Architecture:** Retain the alternate-screen review owner required by immutable terminal scrollback, but compose its bounded transcript body with the existing chrome renderer. Route only browser-specific input to review state and let the existing prompt editor own ordinary input.

**Tech Stack:** Rust 2024, crossterm 0.29, existing `neo-tui` and `neo-agent` test harnesses.

## Global Constraints

- No legacy full-screen modal compatibility path or feature flag.
- Do not mutate or replay committed terminal scrollback.
- Preserve Windows, Linux, and macOS behavior through crossterm ANSI primitives.
- Add no dependency and no new general-purpose layout abstraction.

---

### Task 1: Bound Review Body And Preserve Chrome

**Files:**
- Modify: `crates/neo-tui/src/app.rs`
- Test: `crates/neo-tui/tests/terminal_frame.rs`

**Interfaces:**
- Consumes: existing `render_chrome`, `fit_chrome_to_height`, `append_chrome`, and `TranscriptPane::render_browser_rows`.
- Produces: review `TerminalFrame` values whose `live` rows include chrome and whose cursor points into that frame.

- [ ] **Step 1: Write the failing exact-fill review test**

Add `transcript_browser_expansion_reserves_chrome_rows` with a prompt, enough expandable content to fill a short terminal, and assertions that the frame is bounded, contains the prompt/footer, and returns an in-bounds cursor.

- [ ] **Step 2: Run the exact test and verify RED**

Run:

```bash
rtk cargo test --package neo-tui --test terminal_frame -- transcript_browser_expansion_reserves_chrome_rows --exact --nocapture
```

Expected: failure because the current review frame contains no chrome and has `cursor=None`.

- [ ] **Step 3: Implement the minimal shared composition**

Render and fit chrome before the review early return, pass `height.saturating_sub(chrome.lines.len())` to `render_browser_rows`, then append chrome and return its cursor. Reuse the same composition in `render_frame` and `render_terminal_frame_at` without retaining the old full-screen branch.

- [ ] **Step 4: Run the exact test and verify GREEN**

Run the command from Step 2. Expected: one passed test.

### Task 2: Route Prompt Input While Reviewing

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/input.rs`
- Test: `crates/neo-agent/src/modes/interactive/tests.rs`

**Interfaces:**
- Consumes: existing prompt editing and transcript-browser close helpers.
- Produces: browser handling that returns `false` for ordinary editor input and closes review before prompt submission.

- [ ] **Step 1: Write one failing controller workflow test**

Add `transcript_browser_keeps_prompt_editable_and_closes_on_submit`, proving inserted text reaches the existing prompt while review is open and submit closes review before dispatching that prompt.

- [ ] **Step 2: Run each exact test and verify RED**

```bash
rtk cargo test --package neo-agent --bin neo -- modes::interactive::tests::transcript_browser_keeps_prompt_editable_and_closes_on_submit --exact --nocapture --include-ignored
```

Expected: prompt remains unchanged or browser remains open.

- [ ] **Step 3: Implement minimal routing changes**

Return `false` from browser handling for ordinary input, and close/dirty the review surface before the existing submit path runs.

- [ ] **Step 4: Run the exact test and verify GREEN**

Run the Step 2 command. Expected: one passed test.

### Task 3: Make Cursor Visibility Match Logical Cursor State

**Files:**
- Modify: `crates/neo-tui/src/screen_output/live_renderer.rs`
- Test: `crates/neo-tui/tests/live_renderer.rs`

**Interfaces:**
- Consumes: `LiveRenderer::render_to(lines, cursor)`.
- Produces: ANSI output that hides the hardware cursor for `None` and shows it for `Some(CursorPos)`.

- [ ] **Step 1: Write a failing cursor-transition test**

Add `logical_cursor_state_controls_hardware_cursor_visibility`, render once with no cursor and once with a cursor, and assert `\x1b[?25l` then `\x1b[?25h`.

- [ ] **Step 2: Run the exact test and verify RED**

```bash
rtk cargo test --package neo-tui --test live_renderer -- logical_cursor_state_controls_hardware_cursor_visibility --exact --nocapture
```

Expected: neither cursor visibility sequence is emitted.

- [ ] **Step 3: Emit cursor visibility from the existing render transaction**

Append the crossterm-compatible ANSI hide/show sequence from the logical cursor state. Existing `previous_cursor` comparison already suppresses unchanged frames, so no new visibility state is needed.

- [ ] **Step 4: Run the exact test and verify GREEN**

Run the Step 2 command. Expected: one passed test.

### Task 4: Preserve Real Pane Dirty State During Review

**Files:**
- Modify: `crates/neo-tui/src/transcript/pane.rs`
- Test: `crates/neo-tui/tests/terminal_frame.rs`

**Interfaces:**
- Consumes: `TranscriptPane::render_browser_rows` clone-based snapshot rendering.
- Produces: review rendering that never consumes the real pane's normal-render dirty state.

- [ ] **Step 1: Write a direct dirty-ownership regression**

Add `browser_render_does_not_consume_normal_pane_dirty_state`: create a dirty `TranscriptPane`, call `render_browser_rows`, and assert `is_dirty()` remains true.

- [ ] **Step 2: Run the exact test and verify RED**

```bash
rtk cargo test --package neo-tui --test terminal_frame -- browser_render_does_not_consume_normal_pane_dirty_state --exact --nocapture
```

Expected: failure because `render_browser_rows` sets the real pane dirty flag to false.

- [ ] **Step 3: Delete the incorrect dirty-state mutation**

Remove `self.dirty = false` from `render_browser_rows`; the clone owns review rendering, so it cannot consume the real pane's dirty flag.

- [ ] **Step 4: Run the exact test and verify GREEN**

Run the Step 2 command. Expected: one passed test.

### Task 5: Focused Verification

**Files:**
- Verify only files changed by Tasks 1-4.

**Interfaces:**
- Consumes: all behavior added above.
- Produces: fresh evidence for the bug fix and immutable-scrollback invariants.

- [ ] **Step 1: Run focused neighboring regressions sequentially**

```bash
rtk cargo test --package neo-agent --bin neo -- modes::interactive::tests::ctrl_o_enters_and_leaves_transcript_browser --exact --nocapture --include-ignored
rtk cargo test --package neo-tui --test terminal_frame -- transcript_browser_frame_is_bounded_and_marked_review_surface --exact --nocapture
rtk cargo test --package neo-tui --test terminal_scrollback -- committed_tool_review_does_not_duplicate_native_scrollback --exact --nocapture
```

Expected: one passed test from each command.

- [ ] **Step 2: Check formatting and diff hygiene**

```bash
rtk cargo fmt --all --check
rtk git diff --check
```

Expected: both commands exit zero.

- [ ] **Step 3: Review scope**

Confirm no unrelated files changed and no compatibility branch, feature flag, or dependency was added. Do not commit without explicit authorization.
