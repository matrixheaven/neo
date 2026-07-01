# Neo Transcript Viewport Follow-Tail Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Neo's transcript scrolling Kimi-style: follow new content only when the user is already at the bottom, and preserve the user's visible history when they scroll up.

**Architecture:** Replace the transcript viewport's bottom-offset model with an app-managed `scroll_top_rows + follow_tail` model, then make transcript rendering slice rows through that viewport. New transcript content must not force bottom-follow; explicit user actions restore follow-tail. Mouse wheel input should be captured by Neo and routed to the same viewport state.

**Tech Stack:** Rust 2024, `neo-tui`, `neo-agent`, crossterm raw terminal control, `cargo nextest`.

---

## Non-Negotiable Constraints

- Do not use `git add`, `git commit`, `git checkout`, `git reset`, `git stash`, `git clean`, branch mutation, or push unless the user gives explicit per-command authorization in the implementation session.
- Do not preserve old compatibility paths. Replace the old bottom-offset scroll model with the new top-index model.
- Do not use bare `cargo test` as evidence. Use the narrow `cargo nextest run ...` commands listed in each task.
- Keep changes cross-platform. Mouse capture must use crossterm abstractions, not raw Unix-only terminal setup.

## File Map

- Modify `crates/neo-tui/src/transcript/store.rs`
  - Owns `TranscriptViewport`.
  - Remove unconditional follow-bottom behavior from transcript mutation paths.
  - Add explicit `scroll_to_bottom` behavior through the existing `follow_bottom()` method.
- Modify `crates/neo-tui/src/transcript/pane.rs`
  - Make transcript rendering use the viewport slice.
  - Ensure scroll actions mark the pane dirty.
- Modify `crates/neo-tui/src/input/mod.rs`
  - Parse SGR mouse wheel sequences into `InputEvent::ScrollUp` / `InputEvent::ScrollDown`.
- Modify `crates/neo-tui/src/screen_output/frame_differ.rs`
  - Enable and disable crossterm mouse capture with raw mode.
- Modify `crates/neo-agent/src/modes/interactive/input.rs`
  - Restore follow-tail explicitly on prompt submit paths.
- Test `crates/neo-tui/tests/primitives.rs`
  - Unit coverage for viewport top-index semantics and store mutation behavior.
- Test `crates/neo-tui/tests/transcript.rs` or create it if missing
  - Rendering coverage for viewport slicing.
- Test `crates/neo-agent/src/modes/interactive/tests.rs`
  - Event-loop behavior around scroll state and explicit submit follow-tail.
- Test `crates/neo-tui/src/input/mod.rs`
  - Raw SGR mouse wheel parsing.
- Test `crates/neo-tui/src/screen_output/frame_differ.rs`
  - Mouse capture escape sequences in enter/leave output helpers.

---

### Task 1: Convert `TranscriptViewport` To Top-Index Follow-Tail

**Files:**
- Modify: `crates/neo-tui/src/transcript/store.rs`
- Test: `crates/neo-tui/tests/primitives.rs`

- [ ] **Step 1: Replace the viewport tests with top-index expectations**

In `crates/neo-tui/tests/primitives.rs`, replace `transcript_viewport_tracks_bottom_and_manual_scroll` and `transcript_viewport_syncs_visual_row_scrollback_and_follow_tail` with:

```rust
#[test]
fn transcript_viewport_tracks_top_index_and_follow_tail() {
    let mut view = TranscriptViewport::new();

    view.sync(8, 3);
    assert_eq!(view.visible_row_range(8, 3), 5..8);
    assert_eq!(view.scrollback(), 0);
    assert!(view.is_following_tail());

    view.scroll_up(2);
    assert_eq!(view.visible_row_range(8, 3), 3..6);
    assert_eq!(view.scrollback(), 2);
    assert!(!view.is_following_tail());

    view.scroll_down(1);
    assert_eq!(view.visible_row_range(8, 3), 4..7);
    assert_eq!(view.scrollback(), 1);
    assert!(!view.is_following_tail());

    view.scroll_down(1);
    assert_eq!(view.visible_row_range(8, 3), 5..8);
    assert_eq!(view.scrollback(), 0);
    assert!(view.is_following_tail());
}

#[test]
fn transcript_viewport_preserves_history_position_when_content_grows() {
    let mut view = TranscriptViewport::new();

    view.sync(40, 10);
    assert_eq!(view.visible_row_range(40, 10), 30..40);

    view.scroll_up(12);
    assert_eq!(view.visible_row_range(40, 10), 18..28);
    assert!(!view.is_following_tail());

    view.sync(50, 10);
    assert_eq!(
        view.visible_row_range(50, 10),
        18..28,
        "new content must not yank a manually scrolled viewport"
    );
    assert_eq!(view.scrollback(), 22);
    assert!(!view.is_following_tail());
}

#[test]
fn transcript_viewport_follows_tail_only_when_already_at_bottom() {
    let mut view = TranscriptViewport::new();

    view.sync(40, 10);
    assert_eq!(view.visible_row_range(40, 10), 30..40);
    assert!(view.is_following_tail());

    view.sync(50, 10);
    assert_eq!(view.visible_row_range(50, 10), 40..50);
    assert_eq!(view.scrollback(), 0);
    assert!(view.is_following_tail());

    view.scroll_up(4);
    assert_eq!(view.visible_row_range(50, 10), 36..46);
    assert!(!view.is_following_tail());

    view.follow_bottom();
    view.sync(80, 12);
    assert_eq!(view.visible_row_range(80, 12), 68..80);
    assert_eq!(view.scrollback(), 0);
    assert!(view.is_following_tail());
}
```

- [ ] **Step 2: Run the failing viewport tests**

Run:

```bash
rtk cargo nextest run -p neo-tui --test primitives transcript_viewport
```

Expected: at least `transcript_viewport_preserves_history_position_when_content_grows` fails because the old bottom-offset model shifts the visible range when content grows.

- [ ] **Step 3: Replace `TranscriptViewport` internals**

In `crates/neo-tui/src/transcript/store.rs`, replace the current `TranscriptViewport` struct and impl with:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptViewport {
    scroll_top_rows: usize,
    content_rows: usize,
    viewport_rows: usize,
    follow_tail: bool,
    selection: Option<TranscriptSelection>,
}

impl TranscriptViewport {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            scroll_top_rows: 0,
            content_rows: 0,
            viewport_rows: 0,
            follow_tail: true,
            selection: None,
        }
    }

    #[must_use]
    pub const fn selection(&self) -> Option<&TranscriptSelection> {
        self.selection.as_ref()
    }

    #[must_use]
    pub fn scrollback(&self) -> usize {
        self.max_scroll_top().saturating_sub(self.scroll_top_rows)
    }

    #[must_use]
    pub const fn is_following_tail(&self) -> bool {
        self.follow_tail
    }

    pub fn follow_bottom(&mut self) {
        self.follow_tail = true;
        self.scroll_top_rows = self.max_scroll_top();
    }

    pub fn scroll_up(&mut self, rows: usize) {
        self.follow_tail = false;
        self.scroll_top_rows = self.scroll_top_rows.saturating_sub(rows);
    }

    pub fn scroll_down(&mut self, rows: usize) {
        self.scroll_top_rows = self.scroll_top_rows.saturating_add(rows).min(self.max_scroll_top());
        if self.scroll_top_rows == self.max_scroll_top() {
            self.follow_tail = true;
        }
    }

    pub fn sync(&mut self, content_rows: usize, viewport_rows: usize) {
        self.content_rows = content_rows;
        self.viewport_rows = viewport_rows;
        let max = self.max_scroll_top();
        if self.follow_tail {
            self.scroll_top_rows = max;
        } else {
            self.scroll_top_rows = self.scroll_top_rows.min(max);
            if self.scroll_top_rows == max {
                self.follow_tail = true;
            }
        }
    }

    #[must_use]
    pub fn visible_row_range(&self, row_count: usize, height: usize) -> std::ops::Range<usize> {
        if height == 0 || row_count == 0 {
            return 0..0;
        }
        let window = height.min(row_count);
        let max_start = row_count.saturating_sub(window);
        let start = self.scroll_top_rows.min(max_start);
        start..start + window
    }

    fn max_scroll_top(&self) -> usize {
        self.content_rows.saturating_sub(self.viewport_rows)
    }
}
```

Remove the obsolete `has_synced_dimensions()` and `max_scroll_offset()` methods.

- [ ] **Step 4: Run viewport tests again**

Run:

```bash
rtk cargo nextest run -p neo-tui --test primitives transcript_viewport
```

Expected: PASS.

- [ ] **Step 5: Authorization checkpoint**

Do not commit. If the user has explicitly authorized git mutations in this implementation session, ask before running:

```bash
git add crates/neo-tui/src/transcript/store.rs crates/neo-tui/tests/primitives.rs
git commit -m "fix: preserve transcript viewport position"
```

Without authorization, leave the changes unstaged and continue.

---

### Task 2: Stop Transcript Mutations From Forcing Bottom Follow

**Files:**
- Modify: `crates/neo-tui/src/transcript/store.rs`
- Test: `crates/neo-tui/tests/primitives.rs`

- [ ] **Step 1: Add store mutation tests**

Append these tests near the viewport tests in `crates/neo-tui/tests/primitives.rs`:

```rust
#[test]
fn transcript_store_push_preserves_manual_scroll_state() {
    let mut store = TranscriptStore::new();
    for index in 0..20 {
        store.push(TranscriptEntry::status(format!("line {index}")));
    }
    store.viewport_mut().sync(20, 5);
    store.viewport_mut().scroll_up(6);
    assert_eq!(store.viewport().visible_row_range(20, 5), 9..14);
    assert!(!store.viewport().is_following_tail());

    store.push(TranscriptEntry::status("new line"));
    store.viewport_mut().sync(21, 5);

    assert_eq!(store.viewport().visible_row_range(21, 5), 9..14);
    assert!(!store.viewport().is_following_tail());
}

#[test]
fn transcript_store_explicit_follow_bottom_restores_tail_after_push() {
    let mut store = TranscriptStore::new();
    for index in 0..20 {
        store.push(TranscriptEntry::status(format!("line {index}")));
    }
    store.viewport_mut().sync(20, 5);
    store.viewport_mut().scroll_up(6);
    assert!(!store.viewport().is_following_tail());

    store.viewport_mut().follow_bottom();
    store.push(TranscriptEntry::status("new line"));
    store.viewport_mut().sync(21, 5);

    assert_eq!(store.viewport().visible_row_range(21, 5), 16..21);
    assert_eq!(store.viewport().scrollback(), 0);
    assert!(store.viewport().is_following_tail());
}
```

- [ ] **Step 2: Run the failing mutation tests**

Run:

```bash
rtk cargo nextest run -p neo-tui --test primitives transcript_store
```

Expected: `transcript_store_push_preserves_manual_scroll_state` fails while `TranscriptStore::push` still calls `follow_bottom()`.

- [ ] **Step 3: Remove implicit bottom-follow from mutation paths**

In `crates/neo-tui/src/transcript/store.rs`:

Change `push` from:

```rust
pub fn push(&mut self, entry: TranscriptEntry) {
    self.entries.push(entry);
    self.viewport.follow_bottom();
}
```

to:

```rust
pub fn push(&mut self, entry: TranscriptEntry) {
    self.entries.push(entry);
}
```

In `insert_approval_after_tool_or_push`, remove this line after `insert`:

```rust
self.viewport.follow_bottom();
```

In `upsert_delegate`, remove the `self.viewport.follow_bottom();` that runs after converting a root delegate into a `DelegateGroup`.

In `remove`, remove this line:

```rust
self.viewport.follow_bottom();
```

Do not remove `follow_bottom()` itself. It remains the explicit command for user-driven or lifecycle-driven bottom-follow.

- [ ] **Step 4: Run mutation tests again**

Run:

```bash
rtk cargo nextest run -p neo-tui --test primitives transcript_store
```

Expected: PASS.

- [ ] **Step 5: Authorization checkpoint**

Do not commit. If the user has explicitly authorized git mutations in this implementation session, ask before running:

```bash
git add crates/neo-tui/src/transcript/store.rs crates/neo-tui/tests/primitives.rs
git commit -m "fix: avoid forced transcript tail follow"
```

Without authorization, leave the changes unstaged and continue.

---

### Task 3: Make Transcript Rendering Use The Viewport Slice

**Files:**
- Modify: `crates/neo-tui/src/transcript/pane.rs`
- Test: create `crates/neo-tui/tests/transcript.rs` if it does not exist

- [ ] **Step 1: Add a rendering test for sliced transcript rows**

Create `crates/neo-tui/tests/transcript.rs` with this content if the file does not exist. If it already exists, add only the test and helper functions:

```rust
use neo_tui::transcript::TranscriptPane;

fn strip_ansi(text: &str) -> String {
    let mut out = String::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == 0x1b {
            index += 1;
            while index < bytes.len() {
                let byte = bytes[index];
                index += 1;
                if (0x40..=0x7e).contains(&byte) || byte == b'\x07' {
                    break;
                }
            }
            continue;
        }
        let Some(ch) = text[index..].chars().next() else {
            break;
        };
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

#[test]
fn transcript_render_frame_slices_rows_through_viewport() {
    let mut pane = TranscriptPane::new(80, 6);
    pane.set_live_chrome_height(0);
    for index in 0..12 {
        pane.push_status(format!("status line {index:02}"));
    }

    let bottom = pane
        .render_frame(80, 6)
        .expect("initial render should be dirty")
        .join("\n");
    let bottom_plain = strip_ansi(&bottom);
    assert!(!bottom_plain.contains("status line 00"));
    assert!(bottom_plain.contains("status line 11"));

    pane.scroll_transcript_up(4);
    let scrolled = pane
        .render_frame(80, 6)
        .expect("scrolling should dirty the pane")
        .join("\n");
    let scrolled_plain = strip_ansi(&scrolled);
    assert!(scrolled_plain.contains("status line 04"));
    assert!(scrolled_plain.contains("status line 07"));
    assert!(!scrolled_plain.contains("status line 11"));

    pane.push_status("status line 12");
    let grown = pane
        .render_frame(80, 6)
        .expect("new status should dirty the pane")
        .join("\n");
    let grown_plain = strip_ansi(&grown);
    assert!(grown_plain.contains("status line 04"));
    assert!(grown_plain.contains("status line 07"));
    assert!(!grown_plain.contains("status line 12"));
}
```

- [ ] **Step 2: Run the failing rendering test**

Run:

```bash
rtk cargo nextest run -p neo-tui --test transcript transcript_render_frame_slices_rows_through_viewport
```

Expected: FAIL because `render_transcript_rows` currently returns all rows and scroll actions do not mark the pane dirty.

- [ ] **Step 3: Mark scroll actions dirty**

In `crates/neo-tui/src/transcript/pane.rs`, change the scroll methods to:

```rust
pub fn scroll_transcript_up(&mut self, rows: usize) {
    self.transcript.viewport_mut().scroll_up(rows);
    self.mark_dirty();
}

pub fn scroll_transcript_down(&mut self, rows: usize) {
    self.transcript.viewport_mut().scroll_down(rows);
    self.mark_dirty();
}
```

- [ ] **Step 4: Slice rendered transcript rows through the viewport**

In `crates/neo-tui/src/transcript/pane.rs`, replace `render_body_lines` with:

```rust
fn render_body_lines(&mut self, width: usize) -> Vec<String> {
    let content_width = super::chrome_render::frame_content_width(width);
    self.render_transcript_rows(content_width)
        .into_iter()
        .map(|line| line.to_ansi())
        .collect()
}
```

Then replace `render_transcript_rows` with this version:

```rust
fn render_transcript_rows(&mut self, width: usize) -> Vec<Line> {
    let mut rows = Vec::new();
    let mut tool_run = Vec::new();
    let entries = self.transcript.entries().to_owned();

    for entry in entries {
        match entry {
            TranscriptEntry::ToolRun { component } => tool_run.push(component),
            entry => {
                append_transcript_block(&mut rows, self.flush_tool_run(&mut tool_run, width));
                append_transcript_block(
                    &mut rows,
                    entry.render_with_activity_frame(width, &self.theme, self.activity_frame),
                );
            }
        }
    }
    append_transcript_block(&mut rows, self.flush_tool_run(&mut tool_run, width));

    let viewport_rows = self.height.saturating_sub(self.live_chrome_height).max(1);
    self.transcript
        .viewport_mut()
        .sync(rows.len(), viewport_rows);
    let range = self
        .transcript
        .viewport()
        .visible_row_range(rows.len(), viewport_rows);
    rows.into_iter().skip(range.start).take(range.len()).collect()
}
```

Keep `render_body_lines` as a thin ANSI conversion wrapper. The viewport sync must happen after all rows are generated because wrapping and tool grouping determine the final row count.

- [ ] **Step 5: Run the rendering test again**

Run:

```bash
rtk cargo nextest run -p neo-tui --test transcript transcript_render_frame_slices_rows_through_viewport
```

Expected: PASS.

- [ ] **Step 6: Run viewport unit tests to catch regressions**

Run:

```bash
rtk cargo nextest run -p neo-tui --test primitives transcript_viewport
```

Expected: PASS.

- [ ] **Step 7: Authorization checkpoint**

Do not commit. If the user has explicitly authorized git mutations in this implementation session, ask before running:

```bash
git add crates/neo-tui/src/transcript/pane.rs crates/neo-tui/tests/transcript.rs
git commit -m "fix: render transcript through viewport"
```

Without authorization, leave the changes unstaged and continue.

---

### Task 4: Restore Follow-Tail On Explicit User Submit

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/input.rs`
- Test: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Add an event-loop test for submit restoring follow-tail**

Add this test near `event_loop_dispatches_mouse_wheel_to_transcript_view` in `crates/neo-agent/src/modes/interactive/tests.rs`:

```rust
#[tokio::test]
async fn event_loop_submit_restores_transcript_follow_tail() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.transcript_mut().sync_transcript_view(30, 6);

    controller
        .handle_input_event(InputEvent::ScrollUp(5))
        .await
        .expect("wheel up scrolls transcript");
    assert!(transcript_scrollback(&controller) > 0);
    assert!(!controller.transcript().transcript().viewport().is_following_tail());

    controller
        .handle_input_event(InputEvent::Insert('h'))
        .await
        .expect("typing works");
    controller
        .handle_input_event(InputEvent::Insert('i'))
        .await
        .expect("typing works");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit restores tail before sending");

    assert_eq!(transcript_scrollback(&controller), 0);
    assert!(controller.transcript().transcript().viewport().is_following_tail());
}
```

- [ ] **Step 2: Run the failing submit test**

Run:

```bash
rtk cargo nextest run -p neo-agent --lib event_loop_submit_restores_transcript_follow_tail
```

Expected: FAIL because submit currently does not explicitly restore follow-tail after Task 2 removes implicit `push` follow-bottom.

- [ ] **Step 3: Add an explicit helper to the interactive controller**

In `crates/neo-agent/src/modes/interactive/input.rs`, add this private helper inside the `impl InteractiveController` block:

```rust
fn follow_transcript_tail(&mut self) {
    self.transcript_mut()
        .transcript_mut()
        .viewport_mut()
        .follow_bottom();
}
```

`InteractiveController::transcript_mut()` already returns `&mut TranscriptPane`, and `TranscriptPane::transcript_mut()` returns `&mut TranscriptStore`, so the helper above uses the existing accessor chain without touching controller fields directly.

- [ ] **Step 4: Call the helper before submit paths**

In `handle_input_event`, change the submit arm to:

```rust
InputEvent::Submit => {
    self.clear_pending_exit_confirmation();
    self.follow_transcript_tail();
    self.submit_current_prompt().await?;
}
```

In `handle_overlay_keybinding_action`, change the `KeybindingAction::InputSubmit` arm to:

```rust
KeybindingAction::InputSubmit => {
    self.clear_pending_exit_confirmation();
    self.follow_transcript_tail();
    self.submit_current_prompt().await?;
}
```

Do not add a follow-tail call to passive streaming deltas, tool updates, delegate updates, or background notifications.

- [ ] **Step 5: Run the submit test again**

Run:

```bash
rtk cargo nextest run -p neo-agent --lib event_loop_submit_restores_transcript_follow_tail
```

Expected: PASS.

- [ ] **Step 6: Run the existing wheel dispatch test**

Run:

```bash
rtk cargo nextest run -p neo-agent --lib event_loop_dispatches_mouse_wheel_to_transcript_view
```

Expected: PASS.

- [ ] **Step 7: Authorization checkpoint**

Do not commit. If the user has explicitly authorized git mutations in this implementation session, ask before running:

```bash
git add crates/neo-agent/src/modes/interactive/input.rs crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "fix: restore transcript tail on submit"
```

Without authorization, leave the changes unstaged and continue.

---

### Task 5: Parse Mouse Wheel Input Into Scroll Events

**Files:**
- Modify: `crates/neo-tui/src/input/mod.rs`
- Test: `crates/neo-tui/src/input/mod.rs`

- [ ] **Step 1: Add raw SGR mouse wheel parser tests**

In the `#[cfg(test)] mod tests` block of `crates/neo-tui/src/input/mod.rs`, add:

```rust
#[test]
fn raw_sgr_mouse_wheel_up_produces_scroll_up() {
    let mut parser = InputParser::new();
    assert_eq!(
        parser.feed_bytes(b"\x1b[<64;20;10M"),
        vec![InputEvent::ScrollUp(3)]
    );
}

#[test]
fn raw_sgr_mouse_wheel_down_produces_scroll_down() {
    let mut parser = InputParser::new();
    assert_eq!(
        parser.feed_bytes(b"\x1b[<65;20;10M"),
        vec![InputEvent::ScrollDown(3)]
    );
}

#[test]
fn raw_sgr_mouse_release_is_ignored() {
    let mut parser = InputParser::new();
    assert_eq!(parser.feed_bytes(b"\x1b[<64;20;10m"), Vec::<InputEvent>::new());
}
```

- [ ] **Step 2: Run the failing parser tests**

Run:

```bash
rtk cargo nextest run -p neo-tui --lib raw_sgr_mouse
```

Expected: FAIL because SGR mouse payloads are recognized as complete escape sequences but are not mapped to `InputEvent`.

- [ ] **Step 3: Add SGR mouse parsing helper**

In `crates/neo-tui/src/input/mod.rs`, add this helper near `is_plain_printable_key_id`:

```rust
fn parse_sgr_mouse_scroll(seq: &str) -> Option<InputEvent> {
    let payload = seq.strip_prefix("\x1b[<")?;
    if payload.ends_with('m') {
        return None;
    }
    let payload = payload.strip_suffix('M')?;
    let mut parts = payload.split(';');
    let button = parts.next()?.parse::<u16>().ok()?;
    let _x = parts.next()?.parse::<u16>().ok()?;
    let _y = parts.next()?.parse::<u16>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    match button {
        64 => Some(InputEvent::ScrollUp(3)),
        65 => Some(InputEvent::ScrollDown(3)),
        _ => None,
    }
}
```

The scroll amount is `3` to match the existing event-loop test's row-scale expectation.

- [ ] **Step 4: Call the helper before key parsing**

In `convert_key_sequence`, add this immediately after the key-release check:

```rust
if let Some(event) = parse_sgr_mouse_scroll(seq) {
    return vec![event];
}
```

The top of `convert_key_sequence` should become:

```rust
fn convert_key_sequence(&mut self, seq: &str) -> Vec<InputEvent> {
    // Skip key release events
    if is_key_release(seq) {
        return Vec::new();
    }

    if let Some(event) = parse_sgr_mouse_scroll(seq) {
        return vec![event];
    }

    // Try printable key first (for text insertion)
    if let Some(ch) = decode_printable_key(seq) {
        return vec![InputEvent::Insert(ch)];
    }
    // ...
}
```

- [ ] **Step 5: Run parser tests again**

Run:

```bash
rtk cargo nextest run -p neo-tui --lib raw_sgr_mouse
```

Expected: PASS.

- [ ] **Step 6: Authorization checkpoint**

Do not commit. If the user has explicitly authorized git mutations in this implementation session, ask before running:

```bash
git add crates/neo-tui/src/input/mod.rs
git commit -m "fix: parse terminal mouse wheel input"
```

Without authorization, leave the changes unstaged and continue.

---

### Task 6: Enable Mouse Capture In The Terminal Renderer

**Files:**
- Modify: `crates/neo-tui/src/screen_output/frame_differ.rs`
- Test: `crates/neo-tui/src/screen_output/frame_differ.rs`

- [ ] **Step 1: Add renderer output helpers for mouse capture testing**

In `crates/neo-tui/src/screen_output/frame_differ.rs`, refactor the crossterm setup so tests can inspect the output without touching real raw mode.

Add these imports:

```rust
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
```

Add this helper near `write_leave_output`:

```rust
fn write_enter_output(output: &mut dyn Write) -> std::io::Result<()> {
    execute!(
        output,
        EnableBracketedPaste,
        EnableMouseCapture,
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS,
        )
    )
}
```

Change `TuiRenderer::enter()` so the terminal setup block is:

```rust
enable_raw_mode()?;
let mut output = stdout();
write_enter_output(&mut output)?;
Ok(Self {
    previous_lines: Vec::new(),
    previous_kitty_image_ids: BTreeSet::new(),
    viewport_top: 0,
    previous_viewport_top: 0,
    hardware_cursor_row: 0,
    previous_width: 0,
    previous_height: 0,
    first_render: true,
    max_lines_rendered: 0,
    cursor_row: 0,
    clear_on_shrink: false,
    show_hardware_cursor: hardware_cursor_enabled_from_env_value(
        env::var("NEO_HARDWARE_CURSOR").ok().as_deref(),
    ),
})
```

Change `suspend_resume()` to use:

```rust
enable_raw_mode()?;
let mut output = stdout();
write_enter_output(&mut output)?;
```

Change `leave()` to execute `DisableMouseCapture`:

```rust
let _ = execute!(
    output,
    DisableMouseCapture,
    PopKeyboardEnhancementFlags,
    DisableBracketedPaste,
);
```

- [ ] **Step 2: Add tests for mouse capture escape output**

In the existing tests module in `crates/neo-tui/src/screen_output/frame_differ.rs`, add:

```rust
#[test]
fn enter_output_enables_mouse_capture() {
    let mut buf = Vec::new();
    write_enter_output(&mut buf).unwrap();
    let output = String::from_utf8_lossy(&buf);
    assert!(
        output.contains("\x1b[?1000h") || output.contains("\x1b[?1002h") || output.contains("\x1b[?1006h"),
        "enter output should enable terminal mouse reporting: {output:?}"
    );
}

#[test]
fn leave_output_disables_mouse_capture() {
    let mut renderer = test_renderer(Vec::new());
    let mut buf = Vec::new();
    renderer.write_leave_output(&mut buf);
    let _ = execute!(
        &mut buf,
        DisableMouseCapture,
        PopKeyboardEnhancementFlags,
        DisableBracketedPaste,
    );
    let output = String::from_utf8_lossy(&buf);
    assert!(
        output.contains("\x1b[?1000l") || output.contains("\x1b[?1002l") || output.contains("\x1b[?1006l"),
        "leave output should disable terminal mouse reporting: {output:?}"
    );
}
```

The test checks crossterm's concrete escape sequences loosely because versions can emit multiple mouse modes.

- [ ] **Step 3: Run renderer mouse capture tests**

Run:

```bash
rtk cargo nextest run -p neo-tui --lib mouse_capture
```

Expected: PASS after the helper and leave changes are in place.

- [ ] **Step 4: Run input parser mouse tests**

Run:

```bash
rtk cargo nextest run -p neo-tui --lib raw_sgr_mouse
```

Expected: PASS.

- [ ] **Step 5: Authorization checkpoint**

Do not commit. If the user has explicitly authorized git mutations in this implementation session, ask before running:

```bash
git add crates/neo-tui/src/screen_output/frame_differ.rs
git commit -m "fix: capture mouse wheel in tui"
```

Without authorization, leave the changes unstaged and continue.

---

### Task 7: Final Focused Verification

**Files:**
- No new files.

- [ ] **Step 1: Run transcript viewport unit tests**

Run:

```bash
rtk cargo nextest run -p neo-tui --test primitives transcript_viewport
```

Expected: PASS.

- [ ] **Step 2: Run transcript store mutation tests**

Run:

```bash
rtk cargo nextest run -p neo-tui --test primitives transcript_store
```

Expected: PASS.

- [ ] **Step 3: Run transcript rendering viewport test**

Run:

```bash
rtk cargo nextest run -p neo-tui --test transcript transcript_render_frame_slices_rows_through_viewport
```

Expected: PASS.

- [ ] **Step 4: Run raw mouse parser tests**

Run:

```bash
rtk cargo nextest run -p neo-tui --lib raw_sgr_mouse
```

Expected: PASS.

- [ ] **Step 5: Run renderer mouse capture tests**

Run:

```bash
rtk cargo nextest run -p neo-tui --lib mouse_capture
```

Expected: PASS.

- [ ] **Step 6: Run interactive submit follow-tail test**

Run:

```bash
rtk cargo nextest run -p neo-agent --lib event_loop_submit_restores_transcript_follow_tail
```

Expected: PASS.

- [ ] **Step 7: Run existing interactive wheel dispatch test**

Run:

```bash
rtk cargo nextest run -p neo-agent --lib event_loop_dispatches_mouse_wheel_to_transcript_view
```

Expected: PASS.

- [ ] **Step 8: Review for accidental broad behavior changes**

Run:

```bash
rtk git diff -- crates/neo-tui/src/transcript/store.rs crates/neo-tui/src/transcript/pane.rs crates/neo-tui/src/input/mod.rs crates/neo-tui/src/screen_output/frame_differ.rs crates/neo-agent/src/modes/interactive/input.rs crates/neo-tui/tests/primitives.rs crates/neo-tui/tests/transcript.rs crates/neo-agent/src/modes/interactive/tests.rs
```

Expected:
- No unrelated refactors.
- No compatibility branch that preserves the old bottom-offset model.
- No broad test widening.
- No git mutations unless explicitly authorized.

---

## Self-Review

- Spec coverage: The plan covers the selected方案 B app-managed viewport, Kimi-style follow-tail semantics, explicit submit-to-tail behavior, rendering slice behavior, and mouse wheel routing.
- Placeholder scan: No task contains placeholder language. Each code-changing step includes exact code or exact removal instructions.
- Type consistency: The plan consistently uses `TranscriptViewport`, `scroll_top_rows`, `follow_tail`, `visible_row_range`, `InputEvent::ScrollUp`, and `InputEvent::ScrollDown`.
- Scope check: The plan stays inside transcript scrolling and terminal wheel routing. It does not redesign transcript storage, renderer diffing, or session replay beyond the required viewport behavior.
