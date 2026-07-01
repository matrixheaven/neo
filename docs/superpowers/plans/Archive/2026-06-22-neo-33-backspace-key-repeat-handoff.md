# NEO-33 Backspace Key Repeat Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix Neo TUI input so holding Backspace, Delete, arrow keys, and normal editing keys produces repeated edits/navigation instead of only handling the initial key press.

**Architecture:** Treat crossterm `KeyEventKind::Repeat` as a real input event in the shared `neo-tui` input layer, while continuing to ignore `Release`. Centralize the accepted-key-event-kind predicate so `InputEvent`, `InputParser`, and `KeyId` cannot drift. Add focused tests that reproduce the current Backspace failure and verify prompt/dialog paths consume repeated Backspace events.

**Tech Stack:** Rust 2024, `crossterm` keyboard events, Neo `InputEvent` / `InputParser`, `neo-tui` rich dialogs and prompt state, `nextest` through `cargo nextest run`.

---

## Linear Context

- Linear: [NEO-33](https://linear.app/neo-agent/issue/NEO-33/fix-long-press-backspace-key-repeat-in-tui-inputs)
- Title: Fix long-press Backspace key repeat in TUI inputs
- Priority: High
- Project: TUI & UX Polish
- Team: Neo
- Label: Bug

## User Report

The user reports:

> 目前，我无论是什么地方的输入（输入框，/provider 的填写），我长按 backspace，都只有按下时的第一个字母会被 backspace 掉，它不会像正常的 backspace 一样，能够持续地删内容，这导致我需要连续敲击 backspace 才能把一长串内容删掉，对于交互而言很不友好，可以看一下 docs/kimi-code 是如何处理长按 backspace 的，这估计是个 bug，需要修复。

This is a bug, not a feature request. The expected behavior is normal terminal/editor behavior: holding Backspace continuously deletes text.

## Root Cause Hypothesis

Neo's terminal renderer enables crossterm keyboard event-type reporting:

- `crates/neo-tui/src/terminal/renderer.rs`
  - `KeyboardEnhancementFlags::REPORT_EVENT_TYPES`

With event types enabled, terminals commonly emit:

1. `KeyEventKind::Press` for initial key down.
2. `KeyEventKind::Repeat` for auto-repeat while the key is held.
3. `KeyEventKind::Release` for key up.

Neo currently filters almost every key path to `KeyEventKind::Press` only:

- `InputEvent::from_key_event`
- `InputEvent::from_key_event_with_keybindings`
- `InputParser::feed_crossterm_event`
- `KeyId::from_key_event`
- special cases in `InputParser::feed_key_event`

That means a held Backspace becomes one handled `Press`, followed by many discarded `Repeat` events. The user sees exactly one character deleted.

## Kimi Reference

Read these references before coding:

- `docs/kimi-code/apps/kimi-code/src/tui/components/editor/custom-editor.ts`
- `docs/kimi-code/apps/kimi-code/src/tui/components/dialogs/api-key-input-dialog.ts`
- `docs/kimi-code/apps/kimi-code/src/tui/components/dialogs/custom-registry-import.ts`
- `docs/kimi-code/apps/kimi-code/src/tui/utils/searchable-list.ts`
- `docs/kimi-code/apps/kimi-code/test/tui/printable-key-guard.test.ts`

Important behavior to borrow:

- Kimi routes raw input strings to the focused editor/dialog.
- It explicitly ignores key release with `isKeyRelease(normalized)`.
- It does not drop ordinary repeated key data before it reaches `Input.handleInput(data)`.
- Dialogs delegate editing to shared `Input` components, so Backspace repeat naturally works everywhere.
- Kimi uses `matchesKey(data, Key.backspace)` for Backspace in searchable lists and other custom input handlers.

Neo cannot copy this architecture directly because it uses crossterm typed events, but it should borrow the policy: ignore release, keep press and repeat.

## Mandatory References

Read these before coding:

- `AGENTS.md`
- `~/.codex/RTK.md`
- `~/.codex/CX.md`
- `crates/neo-tui/src/input.rs`
- `crates/neo-tui/tests/primitives.rs`
- `crates/neo-tui/src/terminal/renderer.rs`
- `crates/neo-tui/src/dialogs/api_key_input.rs`
- `crates/neo-tui/src/dialogs/custom_registry_import.rs`
- `crates/neo-tui/src/widgets/question_dialog.rs`
- `crates/neo-tui/src/chrome.rs`
- `crates/neo-agent/src/modes/interactive.rs`

Run project recall first:

```bash
rtk icm recall-context "Neo TUI backspace key repeat KeyEventKind Repeat InputParser dialogs provider" --limit 5
```

## Non-Negotiable Project Rules

- Use `rtk` for shell commands.
- Prefer `cx` for symbol navigation before broad reads.
- Do not run bare `cargo test`; use `rtk cargo nextest run ...`.
- Do not perform git mutations unless the user gives explicit per-command authorization. This includes `git add`, `git commit`, `git push`, `git switch`, `git checkout`, `git reset`, `git stash`, `git clean`, `git rm`, `git merge`, and `git rebase`.
- Preserve unrelated worktree changes.
- Keep the fix in the shared input layer. Do not patch each dialog separately unless tests reveal a dialog-specific bug after the shared fix.

## Product / UX Requirements

- Holding Backspace in the main composer should continuously delete text.
- Holding Backspace in `/provider` API key input should continuously delete text.
- Holding Backspace in `/provider` custom registry URL/token fields should continuously delete text.
- Holding Backspace in question dialog "Other" text should continuously delete text.
- Holding Backspace in approval "Reject with feedback" text should continuously delete text.
- Holding Delete should continuously delete forward.
- Holding arrow keys should continuously move cursor/list selection where the UI supports repeated navigation.
- Release events should remain ignored.
- Repeated Ctrl+C, Esc, and other command keys should not cause surprising repeated destructive/cancel behavior. If a repeat is intentionally allowed, tests should document it; otherwise keep command repeats filtered at the action handling layer.

## File Structure

Modify:

- `crates/neo-tui/src/input.rs`
  - Add a single accepted-key-event-kind helper.
  - Use it in `InputEvent`, `InputParser`, and `KeyId`.
  - Add unit tests for repeat mapping.

- `crates/neo-tui/tests/primitives.rs`
  - Add integration-style tests for crossterm Repeat events.
  - Verify Press + Repeat + Repeat Backspace yields three edit events.

- `crates/neo-agent/src/modes/interactive.rs`
  - Add or extend focused tests proving repeated Backspace edits prompt and dialog text. Implementation changes here should be unnecessary unless command repeats need guarding.

Possibly modify:

- `crates/neo-tui/src/chrome.rs`
  - Only if approval feedback or prompt state has its own repeat filtering.

Do not modify:

- `crates/neo-tui/src/terminal/renderer.rs`
  - Keep `REPORT_EVENT_TYPES`; the fix should handle event types correctly rather than disabling them.

## Task 1: Reproduce Repeat Filtering in `neo-tui` Input Tests

**Files:**

- Modify: `crates/neo-tui/src/input.rs`
- Modify: `crates/neo-tui/tests/primitives.rs`

- [ ] **Step 1: Add a test helper for Repeat events in `input.rs` tests**

Inside `#[cfg(test)] mod tests` in `crates/neo-tui/src/input.rs`, add:

```rust
fn repeat_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new_with_kind(code, modifiers, KeyEventKind::Repeat)
}
```

- [ ] **Step 2: Add failing unit tests for repeated Backspace and keybindings**

Add these tests near the existing `alt_enter_produces_newline` / keybinding tests:

```rust
#[test]
fn repeat_backspace_maps_to_backspace_input_event() {
    assert_eq!(
        InputEvent::from_key_event(repeat_key(KeyCode::Backspace, KeyModifiers::NONE)),
        Some(InputEvent::Backspace)
    );
}

#[test]
fn repeat_keybinding_maps_to_key_event() {
    assert_eq!(
        InputEvent::from_key_event_with_keybindings(
            repeat_key(KeyCode::Backspace, KeyModifiers::NONE),
            &KeybindingsManager::default(),
        ),
        Some(InputEvent::Key(KeyId::new("backspace").expect("valid key")))
    );
}

#[test]
fn repeat_key_id_maps_like_press() {
    assert_eq!(
        KeyId::from_key_event(repeat_key(KeyCode::Backspace, KeyModifiers::NONE)),
        Some(KeyId::new("backspace").expect("valid key"))
    );
}

#[test]
fn release_key_id_is_still_ignored() {
    let release = KeyEvent::new_with_kind(
        KeyCode::Backspace,
        KeyModifiers::NONE,
        KeyEventKind::Release,
    );
    assert_eq!(KeyId::from_key_event(release), None);
}
```

Expected before implementation: FAIL, because Repeat is currently filtered out.

- [ ] **Step 3: Add failing parser integration test**

In `crates/neo-tui/tests/primitives.rs`, add a helper:

```rust
fn repeat_key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new_with_kind(
        code,
        KeyModifiers::NONE,
        KeyEventKind::Repeat,
    ))
}
```

Then add:

```rust
#[test]
fn input_parser_accepts_repeated_backspace_events() {
    let mut parser = InputParser::new();
    let events = [
        press_key(KeyCode::Backspace),
        repeat_key(KeyCode::Backspace),
        repeat_key(KeyCode::Backspace),
    ];

    let produced = events
        .iter()
        .flat_map(|event| parser.feed_crossterm_event(event))
        .collect::<Vec<_>>();

    assert_eq!(
        produced,
        vec![
            InputEvent::Backspace,
            InputEvent::Backspace,
            InputEvent::Backspace,
        ]
    );
}
```

Expected before implementation: FAIL, because `feed_crossterm_event` currently filters to `Press`.

- [ ] **Step 4: Run the failing tests**

Run:

```bash
```

Expected before implementation: both fail.

## Task 2: Centralize Accepted Key Event Kinds

**Files:**

- Modify: `crates/neo-tui/src/input.rs`

- [ ] **Step 1: Add helper functions near the constants**

Add near `ESC_ENTER_NEWLINE_WINDOW`:

```rust
fn is_key_down_event(kind: KeyEventKind) -> bool {
    matches!(kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

fn is_initial_press_event(kind: KeyEventKind) -> bool {
    kind == KeyEventKind::Press
}
```

The distinction matters:

- `is_key_down_event`: normal input/edit/navigation should accept Press and Repeat.
- `is_initial_press_event`: special buffered recognition that should only start on initial press, such as pending Esc behavior.

- [ ] **Step 2: Update `InputEvent::from_key_event`**

Replace:

```rust
if event.kind != KeyEventKind::Press {
    return None;
}
```

with:

```rust
if !is_key_down_event(event.kind) {
    return None;
}
```

- [ ] **Step 3: Update `InputEvent::from_key_event_with_keybindings`**

Replace:

```rust
if event.kind != KeyEventKind::Press {
    return None;
}
```

with:

```rust
if !is_key_down_event(event.kind) {
    return None;
}
```

- [ ] **Step 4: Update `InputParser::feed_crossterm_event`**

Replace:

```rust
Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
    self.feed_key_event(*key_event)
}
```

with:

```rust
Event::Key(key_event) if is_key_down_event(key_event.kind) => {
    self.feed_key_event(*key_event)
}
```

- [ ] **Step 5: Update `KeyId::from_key_event`**

Replace:

```rust
if event.kind != KeyEventKind::Press {
    return None;
}
```

with:

```rust
if !is_key_down_event(event.kind) {
    return None;
}
```

- [ ] **Step 6: Keep pending Esc start Press-only**

In `InputParser::feed_key_event`, keep the Esc buffering branch Press-only:

```rust
if event.code == KeyCode::Esc
    && event.modifiers == KeyModifiers::NONE
    && is_initial_press_event(event.kind)
{
    self.pending_esc = Some((Instant::now(), event));
    return Vec::new();
}
```

Do not allow `Repeat` Esc to keep resetting the pending-esc timer. This avoids turning held Esc into a delayed or repeated cancel storm.

- [ ] **Step 7: Update `flush_pending_escape` synthetic events only if necessary**

`flush_pending_escape` constructs synthetic `KeyEventKind::Press` events. Leave those as `Press`; they represent decoded raw bytes, not physical repeat state.

- [ ] **Step 8: Run unit tests**

Run:

```bash
```

Expected: PASS.

## Task 3: Verify Parser Regression Coverage

**Files:**

- Modify: `crates/neo-tui/tests/primitives.rs`

- [ ] **Step 1: Add repeat Delete and arrow tests**

Add:

```rust
#[test]
fn input_parser_accepts_repeated_delete_events() {
    let mut parser = InputParser::new();
    let events = [
        press_key(KeyCode::Delete),
        repeat_key(KeyCode::Delete),
        repeat_key(KeyCode::Delete),
    ];

    let produced = events
        .iter()
        .flat_map(|event| parser.feed_crossterm_event(event))
        .collect::<Vec<_>>();

    assert_eq!(
        produced,
        vec![InputEvent::Delete, InputEvent::Delete, InputEvent::Delete]
    );
}

#[test]
fn input_parser_accepts_repeated_arrow_events() {
    let mut parser = InputParser::new();
    let events = [
        press_key(KeyCode::Left),
        repeat_key(KeyCode::Left),
        repeat_key(KeyCode::Left),
    ];

    let produced = events
        .iter()
        .flat_map(|event| parser.feed_crossterm_event(event))
        .collect::<Vec<_>>();

    assert_eq!(
        produced,
        vec![InputEvent::MoveLeft, InputEvent::MoveLeft, InputEvent::MoveLeft]
    );
}
```

- [ ] **Step 2: Add release still ignored test**

Add:

```rust
#[test]
fn input_parser_ignores_release_events() {
    let mut parser = InputParser::new();
    let release = Event::Key(KeyEvent::new_with_kind(
        KeyCode::Backspace,
        KeyModifiers::NONE,
        KeyEventKind::Release,
    ));

    assert!(parser.feed_crossterm_event(&release).is_empty());
}
```

- [ ] **Step 3: Run parser tests**

Run:

```bash
```

Expected: PASS.

## Task 4: Add Prompt Editing Regression Test

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs`

- [ ] **Step 1: Locate existing prompt edit tests**

There are existing tests around prompt input and Backspace, including:

- `event_loop_backspace_deletes_slash_while_completion_is_open`
- tests that call `.handle_input_event(InputEvent::Key(KeyId::new("backspace")...))`

Use nearby test helpers rather than building a new harness.

- [ ] **Step 2: Add a test for multiple repeated Backspace events at app-input level**

If the test harness can feed crossterm `Event`s, test the parser. If it only feeds `InputEvent`s, add the higher-level regression like this:

```rust
#[tokio::test]
async fn repeated_backspace_input_events_delete_multiple_prompt_chars() {
    let mut app = test_app().await;

    for ch in "abcdef".chars() {
        app.handle_input_event(InputEvent::Insert(ch))
            .await
            .expect("insert should succeed");
    }
    for _ in 0..3 {
        app.handle_input_event(InputEvent::Backspace)
            .await
            .expect("backspace should succeed");
    }

    assert_eq!(app.tui.chrome().prompt().text(), "abc");
}
```

Adjust `test_app()` and prompt getter names to existing local helpers. Do not invent a new test framework if the file already has one.

This does not test crossterm Repeat directly, but paired with `neo-tui` parser tests it proves repeated `InputEvent::Backspace` events behave correctly in the prompt.

- [ ] **Step 3: Run focused interactive test**

Run:

```bash
```

Expected: PASS after Task 2.

## Task 5: Add Dialog Input Regression Tests

**Files:**

- Modify: `crates/neo-tui/src/dialogs/api_key_input.rs`
- Modify: `crates/neo-tui/src/dialogs/custom_registry_import.rs`

- [ ] **Step 1: Add repeated Backspace test for API key input**

Find existing test `backspace_removes_last` in `api_key_input.rs`. Add:

```rust
#[test]
fn repeated_backspace_removes_multiple_characters() {
    let mut state = ApiKeyInputState::new(
        ApiKeyInputOptions {
            title: "API Key".to_owned(),
            provider_name: "openai".to_owned(),
        },
        TuiTheme::default(),
    );

    state.handle_input(&InputEvent::Paste("abcdef".to_owned()));
    state.handle_input(&InputEvent::Backspace);
    state.handle_input(&InputEvent::Backspace);
    state.handle_input(&InputEvent::Backspace);

    assert_eq!(state.value(), "abc");
}
```

If there is no public `value()` accessor, either use an existing result/render assertion pattern or add a `#[cfg(test)]` helper method. Do not expose secret values in production API just for tests.

- [ ] **Step 2: Add repeated Backspace test for custom registry import**

Find existing test `backspace_works_on_active_field` in `custom_registry_import.rs`. Add:

```rust
#[test]
fn repeated_backspace_edits_active_field_multiple_times() {
    let mut state = CustomRegistryImportState::new(
        CustomRegistryImportOptions {
            title: "Import Custom Registry".to_owned(),
        },
        TuiTheme::default(),
    );

    state.handle_input(InputEvent::Paste("https://example.com/abcdef".to_owned()));
    state.handle_input(InputEvent::Backspace);
    state.handle_input(InputEvent::Backspace);
    state.handle_input(InputEvent::Backspace);

    assert_eq!(state.url(), "https://example.com/abc");
}
```

Again, adapt to existing helper/accessor names.

- [ ] **Step 3: Run dialog tests**

Run:

```bash
```

Expected: PASS.

## Task 6: Check Command Repeat Safety

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs` only if needed.
- Modify: `crates/neo-tui/src/input.rs` only if needed.

After accepting `Repeat`, command-like keys can also repeat. This is usually fine for navigation/editing, but review these paths:

- Ctrl+C -> `InputEvent::Interrupt` or keybinding action.
- Esc -> `InputEvent::Cancel`.
- Enter -> `InputEvent::Submit`.
- Ctrl+D -> app exit when prompt empty.
- Ctrl+S -> future steer behavior.

- [ ] **Step 1: Decide repeat policy**

Recommended policy:

- Accept Repeat for editing and navigation keys.
- For command keys that cause one-shot actions, either:
  - ignore Repeat in the action handler, or
  - rely on existing state guards to make repeat harmless.

Do not block all Repeat globally, because that reintroduces the user's bug.

- [ ] **Step 2: Keep Enter repeat safe**

Holding Enter may repeatedly submit in many terminals. If Neo already receives Enter repeat after this change, decide whether to guard it. Safer first patch:

```rust
if event.kind == KeyEventKind::Repeat && event.code == KeyCode::Enter {
    return None;
}
```

Only add this if tests or manual reasoning show repeated submit is risky. If added, document it with a test:

```rust
#[test]
fn repeat_enter_is_not_mapped_to_submit() {
    let repeat = KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Repeat);
    assert_eq!(InputEvent::from_key_event(repeat), None);
}
```

But prefer not to special-case until necessary; many terminal apps allow repeated Enter.

- [ ] **Step 3: Keep Esc buffering Press-only**

This is already handled in Task 2. Add a test if you change behavior:

```rust
#[test]
fn repeat_escape_does_not_reset_pending_escape_timer() {
    let mut parser = InputParser::new();
    assert!(parser.feed_key_event(key(KeyCode::Esc, KeyModifiers::NONE)).is_empty());
    assert!(parser.feed_key_event(repeat_key(KeyCode::Esc, KeyModifiers::NONE)).is_empty());
    std::thread::sleep(ESC_ENTER_NEWLINE_WINDOW + Duration::from_millis(20));
    assert_eq!(parser.flush_timeout(), vec![InputEvent::Cancel]);
}
```

- [ ] **Step 4: Run keybinding smoke tests**

Run:

```bash
```

Expected: PASS.

## Task 7: Manual Verification Notes

This bug is tactile and worth manual checking after tests.

- [ ] Start Neo TUI locally:

```bash
rtk cargo run -p neo-agent
```

- [ ] In the main composer:
  - Type `abcdefghijklmnopqrstuvwxyz`.
  - Hold Backspace.
  - Expected: characters continue deleting until released.

- [ ] In `/provider`:
  - Open `/provider`.
  - Choose Add -> known/custom flow until an input field appears.
  - Type or paste a long value.
  - Hold Backspace.
  - Expected: characters continue deleting until released.

- [ ] In a question dialog or approval feedback dialog if easy to trigger:
  - Enter freeform text.
  - Hold Backspace.
  - Expected: repeated deletion.

If manual verification is not practical, state that in the final response and rely on the parser/dialog tests.

## Task 8: Focused Verification Commands

Run the minimum focused set:

```bash
rtk cargo fmt --all --check
```

If test name filters do not match due to exact names, use:

```bash
```

Do not run workspace CI for this localized input-layer bug.

## Edge Cases and Pitfalls

- Do not disable `REPORT_EVENT_TYPES`; it is used to distinguish press/release and for modern terminal behavior.
- Do not patch only Backspace in one dialog; the bug is in shared input event filtering.
- Do not accept Release events. That would double-trigger keys on key up.
- Do not let Repeat Esc constantly reset the ESC/Enter detection window.
- Do not expose API key or token values through new production accessors just for tests.
- Do not introduce compatibility branches that keep both "Press-only" and "Press+Repeat" input models. Replace the model cleanly.
- Do not assume Repeat only matters for Backspace. Delete, arrows, PageUp/PageDown, and text insertion can repeat too.
- Do not break bracketed paste. Existing paste tests must keep passing.
- Do not break Shift+Enter / Alt+Enter / Ctrl+J newline insertion.
- Do not make a slow or flaky test based on real key-hold timing. Simulate `KeyEventKind::Repeat` directly.

## Self Review Checklist

Before final handoff/PR, verify:

- [ ] `InputEvent::from_key_event` accepts Press and Repeat.
- [ ] `InputEvent::from_key_event_with_keybindings` accepts Press and Repeat.
- [ ] `InputParser::feed_crossterm_event` forwards Press and Repeat.
- [ ] `KeyId::from_key_event` accepts Press and Repeat.
- [ ] Release remains ignored.
- [ ] Pending Esc recognition is still Press-only.
- [ ] Press + Repeat + Repeat Backspace yields three Backspace events.
- [ ] Press + Repeat + Repeat Delete yields three Delete events.
- [ ] Press + Repeat + Repeat Left yields three MoveLeft events or three keybinding events when keybindings are active.
- [ ] Main prompt can consume repeated Backspace events.
- [ ] API key/custom registry dialog tests cover repeated Backspace at the focused field.
- [ ] Existing bracketed paste tests pass.
- [ ] Existing Shift+Enter, Alt+Enter, Ctrl+J tests pass.
- [ ] No secrets are exposed by test helpers.
- [ ] Focused tests were run with .
- [ ] No unrelated files were edited.

## Suggested Implementation Order

1. Add failing Repeat tests in `crates/neo-tui/src/input.rs`.
2. Add failing parser tests in `crates/neo-tui/tests/primitives.rs`.
3. Implement `is_key_down_event` and update shared filters.
4. Keep pending Esc start Press-only.
5. Add prompt/dialog repeated Backspace tests.
6. Run focused tests.
7. Manually verify long-press Backspace if possible.

## Implementation Notes for the Next AI

This should be a small, surgical fix. The most important thing is not to solve it at the wrong layer. If a long-press key never becomes an `InputEvent`, every focused input surface will feel broken. Once `Repeat` is admitted into `InputParser`, existing prompt and dialog editing code should mostly work without modification.

