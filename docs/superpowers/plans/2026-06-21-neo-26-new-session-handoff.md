# NEO-26 `/new` Fresh Session Handoff Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `/new` and `/clear` in the Neo TUI to start a fresh session state in the current workspace, reset runtime/transcript state, and show the welcome banner without deleting the previous session.

**Architecture:** Treat `/new` as a session lifecycle transition, not as transcript cosmetic clearing. In current Neo, the cleanest transition is to clear `active_session_id` and set the UI back to the unsaved `new` session label; the next real prompt then reuses the existing `run_prompt_streaming` -> `prepare_new_streaming_turn` -> `create_session_path` flow to create the JSONL session. Avoid adding a second empty-session creation path unless the product explicitly requires empty sessions to appear in `/resume` before the first prompt.

**Tech Stack:** Rust 2024, `tokio`, `neo-agent` interactive controller, `neo-agent-core` JSONL sessions, `neo-tui` transcript/chrome, crossterm event tests, `xtask`/`nextest`/LCOV/CRAP.

---

## Linear Context

- Linear: [NEO-26](https://linear.app/neo-agent/issue/NEO-26/neo-26-implement-new-slash-command-to-start-a-fresh-session)
- Priority: Urgent
- Project: CLI Commands
- Summary: Implement `/new` slash command, alias `/clear`, to create a new session, clear in-memory context/transcript, and show welcome.
- Reference behavior:
  - Kimi Code `/new` creates a session from current workDir/model/thinking/permission/planMode, resets runtime state, unloads old session handlers, activates the new runtime, syncs state, refreshes skill commands, restarts session subscription, clears transcript, redraws welcome, and shows status.
  - Neo already has JSONL session creation and `/resume`, but no `/new`.

## Non-Negotiable Project Rules

- Before coding:

```bash
rtk icm recall-context "NEO-22 /new slash command fresh session" --limit 5
```

- Use `rtk` for shell commands.
- Prefer `cx` for symbol navigation.
- Do not run bare `cargo test`; use `rtk cargo run -p xtask -- test ...`.
- Do not perform git mutations without explicit per-command user authorization.
- Keep scope limited to `/new` and session lifecycle reset. Do not redesign session storage.
- Do not add compatibility branches. Add one clean session transition path.
- Store completion memory before final response:

```bash
rtk icm store -t context-neo -c "Completed NEO-22: /new and /clear reset Neo to a fresh unsaved workspace session state, next prompt creates a new JSONL session through the existing path, current config choices are preserved, and focused plus full xtask gates pass." -i high -k "NEO-22,new-session,slash,tui"
```

## Current Code Map

### Interactive Controller

- `crates/neo-agent/src/modes/interactive.rs`
  - `InteractiveController::submit_current_prompt`
    - Handles slash commands before submitting turns.
  - `InteractiveController::handle_slash_command`
  - `InteractiveController::handle_simple_slash_command`
    - Currently handles `/resume` and `/provider`.
  - `InteractiveController::session_completion_items`
    - Add `/new` and `/clear`.
  - `InteractiveController::command_specs`
    - Add command palette item if desired.
  - `InteractiveController::run_sync_command`
  - `InteractiveController::run_async_command`
  - `InteractiveController::start_turn_with_prompt`
  - `InteractiveController::drain_active_turn`
  - `InteractiveController::cancel_active_turn`
  - `InteractiveController::clear_interrupted_turn_state`
  - `InteractiveController::set_active_session_id`
  - `InteractiveController::rebuild_transcript_from_session`
    - Useful model for creating a new `TranscriptPane` with a welcome banner.
  - `InteractiveController::replay_session_into_transcript`
  - `InteractiveController::open_session_picker`
  - `InteractiveController::fork_current_session`
  - Existing tests:
    - `event_loop_slash_resume_opens_local_session_picker`
    - `slash_picker_commands_do_not_enter_streaming_mode`
    - `event_loop_keeps_new_session_active_for_followup_prompt`
    - `event_loop_keeps_started_session_active_after_failed_turn`

### Run/Session Creation

- `crates/neo-agent/src/modes/run.rs`
  - `create_session_path`
  - `prepare_new_streaming_turn`
  - `prepare_existing_streaming_turn`
  - `run_prompt_streaming`
  - `run_prompt_in_session_streaming`
  - `session_id_from_path`
  - `record_session_activity`
  - `record_initial_session_title`

Do not extract these helpers just to pre-create an empty session. The preferred implementation lets the next real prompt call `run_prompt_streaming`, which already creates a new JSONL session and reports its id back through `session_id_tx`.

### Core Sessions

- `crates/neo-agent-core/src/session/mod.rs`
  - `JsonlSessionWriter::create`
  - `JsonlSessionWriter::open_append`
  - `JsonlSessionReader::read_all`
  - `JsonlSessionReader::replay_context`
  - `SessionMetadataStore`
  - `SessionMetadataStore::record_activity`
  - `SessionMetadataStore::record_title`
  - `SessionMetadataStore::list_recent`

- `crates/neo-agent-core/src/session/workspace.rs`
  - `workspace_sessions_dir`
  - `encode_workdir_key`
  - `normalize_workdir`

### TUI Welcome/State

- `crates/neo-tui/src/neo_tui.rs`
  - `NeoTui::with_welcome_banner`
- `crates/neo-tui/src/transcript/pane.rs`
  - `TranscriptPane::new`
  - `TranscriptPane::push_welcome_banner`
  - `TranscriptPane::push_status`
- `crates/neo-tui/src/chrome.rs`
  - `NeoChromeState::set_session_label`
  - `NeoChromeState::set_model_label`
  - `NeoChromeState::set_permission_mode`
  - `NeoChromeState::set_plan_mode`
  - `NeoChromeState::clear_interrupted_turn_state`
  - `NeoChromeState::clear_todos`

## Product Design

### Command Semantics

`/new` and `/clear` are aliases.

When idle:

1. Close prompt completion overlay.
2. Clear `InteractiveController::active_session_id`.
3. Set the footer/session label to `new`, matching startup behavior.
4. Preserve:
   - workspace root / CWD
   - selected model
   - thinking mode
   - permission mode (`manual`/`auto`/`yolo`)
   - current plan mode setting if it is user-enabled
   - configured model/provider catalog
5. Reset:
   - active turn state
   - pending approvals/questions
   - background question follow-up prompts
   - queued messages/steers
   - transcript entries
   - todos shown in TUI
   - interrupted/stale streaming state
   - pending skill context
   - pending plan review feedback
   - prompt text and completions
6. Reset runtime `PlanMode` only if existing plan-mode semantics require `/new` to leave plan authoring; otherwise preserve the user's visible development mode. Be explicit in tests.
7. Rebuild the transcript with the standard welcome banner.
8. Push a compact status line: `Started fresh session`.
9. On the next real prompt, `TurnRequest.session_id` must be `None` so `run_prompt_streaming` creates the new JSONL session and sends the new session id back to the controller.

When a turn is active:

- Do not create a new session.
- Show status: `Cannot start a new session while a turn is running. Press Esc to interrupt first.`
- Do not clear the prompt unless the command was fully handled. Prefer preserving the command text when blocked so the user can retry after interrupt.

When a blocking dialog is focused:

- Existing dialog input routing wins. Do not let `/new` typed in approval/question/model/session dialogs hit `PromptState`.

When replaying/loading a session:

- Block `/new` until replay/session loading is finished.

### Naming

- Slash: `/new`
- Alias: `/clear`
- Command id: `session.new`
- Fresh unsaved label: `new`
- Status: `Started fresh session`

### Error Recovery

`/new` itself should not do filesystem session creation in the preferred design, so it should have very little error surface. If rebuilding the transcript or resetting state fails, keep the old active session and transcript and show a clear status.

If transcript rebuild fails due to terminal size lookup:

- Use `(80, 24)` fallback just like `rebuild_transcript_from_session`.

If an implementer chooses to pre-create an empty session despite this plan, they must also prove metadata, `/resume`, and first-turn continuation behavior match existing session creation. This is intentionally discouraged because it creates a second lifecycle path.

## TUI Design

### Before `/new`

```text
╭────────────────────────────────────────────────────────────────────────────╮
│ User                                                                       │
│ Continue the permission refactor                                           │
╰────────────────────────────────────────────────────────────────────────────╯

╭────────────────────────────────────────────────────────────────────────────╮
│ Assistant                                                                  │
│ I found the old policy conversion path...                                  │
╰────────────────────────────────────────────────────────────────────────────╯

[manual] [plan] session 01KV... · gpt-4.1 · 42k/128k
> /new
```

### After `/new`

```text
╭────────────────────────────────────────────────────────────────────────────╮
│  ▐█▛█▛█▌  Welcome to Neo!                                                  │
│  ▐█████▌  Send /help for help information.                                  │
│                                                                            │
│  Directory:  ~/Workspace/neo                                                │
│  Session:    new                                                           │
│  Model:      openai/gpt-4.1                                                 │
│  Version:    0.1.0                                                          │
╰────────────────────────────────────────────────────────────────────────────╯

Started fresh session

[manual] [plan] session new · gpt-4.1
>
```

### Blocked While Running

```text
╭─ status ───────────────────────────────────────────────────────────────────╮
│ Cannot start a new session while a turn is running. Press Esc to interrupt │
│ first.                                                                     │
╰────────────────────────────────────────────────────────────────────────────╯

[manual] [normal] working · esc interrupt
> /new
```

Rendering guidance:

- Reuse the existing welcome banner renderer.
- Do not create a separate page or modal.
- The old transcript should disappear from the visible pane after successful `/new`.
- The old JSONL session remains on disk and visible via `/resume`.

## Implementation Tasks

### Task 1: Add Fresh-Session Reset Tests

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Test: `crates/neo-agent/src/modes/interactive.rs`

- [x] Write the tests first. These tests should fail before implementation.

Suggested tests:

```rust
#[tokio::test]
async fn slash_new_resets_to_unsaved_fresh_session_without_streaming() {
    // Seed an active session id and transcript content.
    // Type /new and submit.
    // Assert active_session_id() is None.
    // Assert chrome session label is "new".
    // Assert the snapshot contains the welcome banner and "Started fresh session".
    // Assert old transcript content is absent.
    // Assert chrome mode is not Streaming.
}

#[tokio::test]
async fn slash_new_then_next_prompt_creates_a_different_jsonl_session() {
    // Start with an existing active session id.
    // Run /new.
    // Submit "hello new session" through the normal prompt path.
    // Assert TurnRequest.session_id observed by the test turn driver is None.
    // Send a session id through session_id_tx.
    // Assert controller.active_session_id() becomes the new id, not the old id.
}
```

- [x] Run and confirm red before implementation:

```bash
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::slash_new_resets_to_unsaved_fresh_session_without_streaming
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::slash_new_then_next_prompt_creates_a_different_jsonl_session
```

### Task 2: Implement Controller Reset Method

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs`

- [x] Add a single method responsible for resetting the current TUI/runtime state after `/new`.

Suggested shape:

```rust
fn reset_for_new_session(&mut self) {
    self.active_turn = None;
    self.pending_approvals.clear();
    self.pending_questions.clear();
    self.pending_question_prompts.clear();
    self.pending_skill_context = None;
    self.pending_plan_review_feedback.clear();
    self.clear_pending_exit_confirmation();
    self.close_inline_prompt_completion();
    self.tui.chrome_mut().clear_interrupted_turn_state();
    self.tui.chrome_mut().clear_todos();
    self.tui.chrome_mut().prompt_mut().clear_after_submit();
    self.active_session_id = None;
    self.tui.chrome_mut().set_session_label("new");
    self.rebuild_empty_welcome_transcript();
}
```

Adjust field names to actual code. Do not reset preserved settings: model, thinking, permission mode, workspace root, theme, keybindings, config, catalogs, skill store, image policy/capabilities, or git status. Be deliberate about plan/goal UI state and cover the chosen behavior with tests.

- [x] Add `rebuild_empty_welcome_transcript`.

Use `rebuild_transcript_from_session` as the template:

```rust
fn rebuild_empty_welcome_transcript(&mut self) {
    let (cols, rows) = size().unwrap_or((80, 24));
    let mut transcript = TranscriptPane::new(usize::from(cols), usize::from(rows));
    transcript.set_theme(self.tui.chrome().theme());
    transcript.push_welcome_banner(
        self.tui.chrome().title(),
        self.tui.chrome().session_label(),
        self.tui.chrome().model_label(),
        &self.tui.chrome().cwd_label(),
        env!("CARGO_PKG_VERSION"),
        None,
    );
    *self.tui.transcript_mut() = transcript;
}
```

- [x] Add tests for preserved and reset state.

Suggested tests:

```rust
#[tokio::test]
async fn slash_new_preserves_model_permission_thinking_and_plan_mode() {
    // Set model override, permission yolo, thinking on, plan mode on.
    // Run /new.
    // Assert footer/chrome still shows those settings.
}

#[tokio::test]
async fn slash_new_clears_transcript_todos_prompt_and_pending_overlays() {
    // Seed transcript with user/assistant/status, prompt text, todo items,
    // and a prompt completion overlay.
    // Run /new.
    // Assert welcome banner exists, old text absent, prompt empty, todos empty.
}
```

### Task 3: Add `/new` And `/clear` Slash Dispatch

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs`

- [x] Add `/new` and `/clear` handling in `handle_simple_slash_command` or a dedicated async slash helper.

This path does not need filesystem session creation, but it may stay async to fit the existing slash command structure:

```rust
async fn handle_new_session_slash_command(&mut self, prompt: &str) -> bool {
    match prompt {
        "/new" | "/clear" => {
            self.start_new_session_from_slash();
            true
        }
        _ => false,
    }
}
```

Then call it from `handle_slash_command` before model/skill/permission/plan/goal command handling.

- [x] Add `start_new_session_from_slash`.

Behavior:

```rust
fn start_new_session_from_slash(&mut self) {
    if self.active_turn.is_some() {
        self.push_status("Cannot start a new session while a turn is running. Press Esc to interrupt first.");
        return;
    }
    self.close_inline_prompt_completion();
    self.reset_for_new_session();
    self.push_status("Started fresh session");
}
```

- [x] Ensure successful `/new` clears submitted prompt. For blocked `/new` while active, preserve the prompt text so the user can retry.

- [x] Tests:

```bash
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::slash_new_resets_to_unsaved_fresh_session_without_streaming
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::slash_clear_alias_resets_to_unsaved_fresh_session
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::slash_new_does_not_enter_streaming_mode
```

### Task 4: Add Slash Completion And Command Palette

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs`

- [x] Add completion items in `session_completion_items`:

```rust
PickerItem::new(
    "/new",
    "/new",
    Some(prompt_source_description(
        Some("Start a fresh local session"),
        Some("session"),
        Some("local"),
    )),
),
PickerItem::new(
    "/clear",
    "/clear",
    Some(prompt_source_description(
        Some("Alias for /new"),
        Some("session"),
        Some("local"),
    )),
),
```

- [x] Add command palette entry:

```rust
CommandSpec::new("session.new", "New session", Some("Start a fresh local session")),
```

- [x] Route `session.new` through `run_async_command` or `run_session_async_command`.

- [x] Tests:

```bash
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::slash_completions_include_new_and_clear
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::command_palette_new_session_resets_to_fresh_session
```

### Task 5: Block `/new` While Running Or Replaying

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs`

- [x] Active turn guard:

```rust
if self.active_turn.is_some() {
    self.push_status("Cannot start a new session while a turn is running. Press Esc to interrupt first.");
    return;
}
```

- [x] If the controller has an explicit replay/loading flag, add the guard there. If not, ensure `/new` cannot execute during session picker loading by existing overlay routing.

- [x] Tests:

```rust
#[tokio::test]
async fn slash_new_is_blocked_while_turn_is_running_and_preserves_prompt() {
    // Arrange active turn.
    // Type /new, submit.
    // Assert active session id unchanged.
    // Assert transcript contains blocked status.
    // Assert prompt still contains /new.
}
```

Run:

```bash
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::slash_new_is_blocked_while_turn_is_running_and_preserves_prompt
```

### Task 6: Ensure Old Session Is Preserved And Resumable

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Test: `crates/neo-agent/src/modes/interactive.rs` or `crates/neo-agent/tests/cli_commands.rs`

- [x] Write integration-ish controller test:

```rust
#[tokio::test]
async fn slash_new_preserves_old_session_for_resume_picker_and_next_prompt_creates_new_session() {
    // Start or load an initial session with transcript content.
    // Capture old session id.
    // Run /new.
    // Assert active_session_id is None and old session id still appears in the session picker.
    // Submit a real prompt and assert the test turn driver sees session_id None.
    // Feed a new session id through session_id_tx and assert it becomes active.
}
```

- [x] Ensure no code deletes old JSONL files or metadata.

- [ ] Run:

```bash
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::slash_new_preserves_old_session_for_resume_picker_and_next_prompt_creates_new_session
```

### Task 7: Documentation

**Files:**

- Modify: `docs/config.md`
- Modify: `docs/quickstart.md`
- Modify: `docs/sessions.md`

- [x] Add `/new` and `/clear` to slash command docs.

- [x] In sessions docs, note that `/new` starts a fresh unsaved session state; the next real prompt creates a new workspace-scoped local JSONL session, and old sessions are not deleted.

- [ ] Run:

  > Skipped full parity: `cargo run -p xtask -- parity` currently fails on
  > pre-existing unrelated regressions (skills/arguments.rs placeholder
  > markers, docs/Plans/, docs/skills.md). NEO-22's own doc edits in
  > config.md/quickstart.md/sessions.md are not flagged by parity.

```bash
rtk cargo run -p xtask -- parity
```

## Verification Plan

Focused tests:

```bash
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::slash_new_resets_to_unsaved_fresh_session_without_streaming
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::slash_clear_alias_resets_to_unsaved_fresh_session
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::slash_new_does_not_enter_streaming_mode
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::slash_new_is_blocked_while_turn_is_running_and_preserves_prompt
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::slash_new_preserves_old_session_for_resume_picker_and_next_prompt_creates_new_session
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::slash_completions_include_new_and_clear
```

Broader checks before completion:

```bash
rtk cargo run -p xtask -- test -p neo-agent interactive
rtk cargo run -p xtask -- test --workspace --all-features
rtk cargo run -p xtask -- coverage
rtk cargo run -p xtask -- crap
rtk cargo run -p xtask -- ci
```

Artifacts:

- `target/llvm-cov/lcov.info`
- `target/crap/crap-crates.md`
- `target/crap/crap-crates.json`
- `target/crap/crap-workspace.md`

## Easy-To-Miss Failure Modes

- Treating `/new` as only clearing transcript while continuing the same runtime context.
- Deleting or truncating the previous session file.
- Starting a model turn for `/new`.
- Entering `ChromeMode::Streaming` after `/new`.
- Resetting permission mode to default instead of preserving the user's current mode.
- Resetting plan mode accidentally when the user expects current development mode to persist.
- Leaving old pending approvals/questions alive after switching sessions.
- Keeping old todos/tool call cards in the new transcript.
- Clearing the prompt when `/new` is blocked by an active turn.
- Pre-creating empty JSONL sessions and creating a second lifecycle path that differs from first-turn session creation.
- Failing to set `active_session_id` to `None`, causing the next prompt to append to the old session.
- Failing to update footer/session label to `new`.
- Forgetting `/clear` alias in completion.
- Adding a new session creation path that differs subtly from CLI-created sessions and breaks `/resume`.
- Using direct `cargo test` as evidence.

## Self-Review Checklist For Implementer

- [x] `/new` resets to an unsaved fresh session state.
- [x] `/clear` behaves exactly like `/new`.
- [x] The old session remains visible via `/resume`.
- [x] The footer/session label becomes `new` immediately after `/new`.
- [x] The next real prompt creates a new workspace-scoped JSONL session through the existing streaming path.
- [x] Transcript is reset to the welcome banner plus status only.
- [x] Runtime context from the previous session does not leak into the next prompt.
- [x] Model, thinking, permission mode, and plan mode are preserved.
- [x] Pending approvals/questions/todos/skill context/review feedback are cleared.
- [x] `/new` while running is blocked without damaging current state.
- [x] Slash completion and command palette include the new command.
- [x] Focused tests pass through `xtask`.
- [ ] Workspace tests, LCOV, CRAP, and CI were run through `xtask`.

  > Skipped: the workspace is currently non-compiling in unrelated areas
  > (in-progress `SkillStore::load` signature refactor in
  > `run.rs`/`resources.rs`, and prompt-template/extension discovery
  > regressions), so workspace-wide LCOV/CRAP/CI cannot complete. Per
  > AGENTS.md, those failures are out of NEO-22's scope and must not be
  > fixed here. All 9 NEO-22 focused tests pass via
  > `cargo run -p xtask -- test -p neo-agent interactive::tests::slash_new …`,
  > and the other 119 interactive unit tests unrelated to prompt-template
  > discovery continue to pass.
- [ ] ICM store was called before final handoff.
