# New Session Context Reset and Sessions Alias Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Clear prior-session context usage on `/new` and make `/sessions` open the same picker as `/resume`.

**Architecture:** Keep both changes in their existing owners. The new-session lifecycle resets the context window while preserving the selected model limit; slash dispatch and completion reuse the current resume picker and static command catalog.

**Tech Stack:** Rust 2024, Tokio tests, Neo interactive controller and TUI shell state.

## Global Constraints

- No new alias registry, state type, dependency, or compatibility path.
- Preserve unrelated worktree changes in `.gitignore` and `prompt_completion.rs`.
- Verify with exact `neo-agent` binary test filters only.

---

### Task 1: Reset context usage for a new session

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`
- Modify: `crates/neo-agent/src/modes/interactive/sessions.rs`

**Interfaces:**
- Consumes: `NeoChromeState::context_window`, `NeoChromeState::set_context_window`, `ContextWindow::new`.
- Produces: `/new` state with no used or projected tokens and the current maximum context size intact.

- [ ] **Step 1: Write the failing test**

Seed the existing `slash_new_resets_to_unsaved_fresh_session_without_streaming` controller before submission:

```rust
controller.tui.chrome_mut().set_context_window(Some(
    ContextWindow::new(1_000_000)
        .with_used_tokens(57_000)
        .with_projected_tokens(Some(61_000)),
));
```

Then assert after `/new`:

```rust
assert_eq!(
    controller.chrome().context_window(),
    Some(ContextWindow::new(1_000_000))
);
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
rtk cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_new_resets_to_unsaved_fresh_session_without_streaming --exact --nocapture --include-ignored
```

Expected: FAIL because the actual window still contains `used_tokens: Some(57000)` and `projected_tokens: Some(61000)`.

- [ ] **Step 3: Implement the minimal reset**

In `reset_for_new_session`, read the current maximum and rebuild only that context window:

```rust
let max_context_tokens = self
    .tui
    .chrome()
    .context_window()
    .and_then(|context| context.max_tokens);
self.tui
    .chrome_mut()
    .set_context_window(max_context_tokens.map(neo_tui::shell::ContextWindow::new));
```

- [ ] **Step 4: Run the exact test again**

Run the Step 2 command. Expected: PASS.

### Task 2: Add the `/sessions` alias

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`
- Modify: `crates/neo-agent/src/modes/interactive/slash_commands.rs`
- Modify: `crates/neo-agent/src/modes/interactive/prompt_completion.rs`

**Interfaces:**
- Consumes: `InteractiveController::open_session_picker`, `STATIC_SLASH_COMMANDS`.
- Produces: `/sessions` dispatch and a visible completion/help item.

- [ ] **Step 1: Write failing behavior and catalog assertions**

Change the existing resume test to iterate over both commands:

```rust
for command in ["/resume", "/sessions"] {
    controller.type_text(command);
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("session picker command runs locally");
    assert!(matches!(
        controller.chrome().focused_overlay().map(|overlay| &overlay.kind),
        Some(OverlayKind::SessionPicker(_))
    ));
    controller.chrome_mut().close_focused_overlay();
}
```

In `prompt_completions_merges_real_prompt_package_and_session_commands`, add:

```rust
assert_eq!(
    by_value["/sessions"].description.as_deref(),
    Some("Alias for /resume")
);
```

- [ ] **Step 2: Run both exact tests and verify they fail**

Run:

```bash
rtk cargo test --package neo-agent --bin neo -- modes::interactive::tests::event_loop_slash_resume_and_sessions_open_local_session_picker --exact --nocapture --include-ignored
rtk cargo test --package neo-agent --bin neo -- modes::interactive::tests::prompt_completions_merges_real_prompt_package_and_session_commands --exact --nocapture --include-ignored
```

Expected: the picker test FAILS for `/sessions`; the completion test FAILS because the catalog lacks `/sessions`.

- [ ] **Step 3: Reuse existing dispatch and completion paths**

Dispatch both commands in one branch:

```rust
"/resume" | "/sessions" => self.open_session_picker(),
```

Add the catalog entry next to `/resume`:

```rust
("/sessions", "Alias for /resume"),
```

- [ ] **Step 4: Run both exact tests again**

Run the Step 2 commands. Expected: PASS.

### Task 3: Verify and commit

**Files:**
- Modify only the four implementation/test files named above.

**Interfaces:**
- Consumes: completed Tasks 1 and 2.
- Produces: one focused bugfix commit.

- [ ] **Step 1: Format touched Rust files**

```bash
rustfmt --edition 2024 crates/neo-agent/src/modes/interactive/sessions.rs crates/neo-agent/src/modes/interactive/slash_commands.rs crates/neo-agent/src/modes/interactive/prompt_completion.rs crates/neo-agent/src/modes/interactive/tests.rs
```

- [ ] **Step 2: Re-run the three exact tests and check the diff**

Run the three commands from Tasks 1 and 2, then:

```bash
rtk git diff --check -- crates/neo-agent/src/modes/interactive/sessions.rs crates/neo-agent/src/modes/interactive/slash_commands.rs crates/neo-agent/src/modes/interactive/prompt_completion.rs crates/neo-agent/src/modes/interactive/tests.rs
```

Expected: all tests PASS and diff check is empty.

- [ ] **Step 3: Commit only the task files**

```bash
rtk git add crates/neo-agent/src/modes/interactive/sessions.rs crates/neo-agent/src/modes/interactive/slash_commands.rs crates/neo-agent/src/modes/interactive/prompt_completion.rs crates/neo-agent/src/modes/interactive/tests.rs
rtk git commit -m "fix: reset context for new sessions"
```
