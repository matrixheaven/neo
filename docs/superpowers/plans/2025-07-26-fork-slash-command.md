# `/fork` Slash Command Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire a `/fork` slash command into Neo's existing fork infrastructure, with two transcript notices ("fork from session …" / "switch to fork session …").

**Architecture:** The fork pipeline already exists end-to-end (`SessionMetadataStore::fork` → `fork_session_transcript` → `fork_current_session`). We add `/fork` to the slash command dispatcher, update the notice text, remove a now-redundant status line, and add `/fork` to completion/help.

**Tech Stack:** Rust, tokio, neo-agent / neo-tui crates.

---

## File Map

| File | Responsibility | Change |
|---|---|---|
| `crates/neo-agent/src/modes/interactive/mod.rs` | `fork_session_transcript` — orchestrates fork + load + notices | Update notice text (2 lines) |
| `crates/neo-agent/src/modes/interactive/slash_commands.rs` | Slash command dispatcher | Add `"/fork"` arm |
| `crates/neo-agent/src/modes/interactive/prompt_completion.rs` | Static slash command catalog | Add `/fork` entry |
| `crates/neo-agent/src/modes/interactive/sessions.rs` | `fork_current_session` lifecycle handler | Remove redundant `push_status` + unused `child_id` |
| `crates/neo-agent/src/modes/interactive/tests.rs` | Integration + unit tests | Update existing test assertions + add `/fork` slash command test |

---

### Task 1: Update fork notices in `fork_session_transcript`

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs:2085-2096`
- Test: `crates/neo-agent/src/modes/interactive/tests.rs:5827-5871`

- [ ] **Step 1: Update the existing unit test to expect new notices**

In `crates/neo-agent/src/modes/interactive/tests.rs`, find the test `fork_session_transcript_copies_jsonl_metadata_and_loads_child` (line 5827). Replace the single-notice assertion:

```rust
    assert_eq!(
        forked.transcript.notices.first().map(String::as_str),
        Some(format!("forked from {SESSION_A}").as_str())
    );
```

with two assertions:

```rust
    assert_eq!(
        forked.transcript.notices.first().map(String::as_str),
        Some(format!("fork from session {SESSION_A}").as_str())
    );
    assert_eq!(
        forked.transcript.notices.get(1).map(String::as_str),
        Some(format!("switch to fork session {}", forked.session_id).as_str())
    );
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --package neo-agent --lib modes::interactive::tests::fork_session_transcript_copies_jsonl_metadata_and_loads_child 2>&1 | tail -20`

Expected: FAIL — the assertion expects `"fork from session …"` but the current code produces `"forked from …"`.

- [ ] **Step 3: Update `fork_session_transcript` to emit two notices**

In `crates/neo-agent/src/modes/interactive/mod.rs`, find `fork_session_transcript` (line 2085). Replace the body:

**Before (lines 2085–2096):**
```rust
async fn fork_session_transcript(
    parent_id: String,
    config: &AppConfig,
) -> Result<ForkedSessionTranscript> {
    let session = SessionMetadataStore::new(workspace_sessions_dir(config))
        .fork(&parent_id, None)
        .with_context(|| format!("failed to create local fork for session {parent_id}"))?;
    let child_id = session.id;
    let mut loaded = load_session_transcript(child_id.clone(), config).await?;
    loaded.notices.insert(0, format!("forked from {parent_id}"));
    Ok(ForkedSessionTranscript::new(child_id, loaded))
}
```

**After:**
```rust
async fn fork_session_transcript(
    parent_id: String,
    config: &AppConfig,
) -> Result<ForkedSessionTranscript> {
    let session = SessionMetadataStore::new(workspace_sessions_dir(config))
        .fork(&parent_id, None)
        .with_context(|| format!("failed to create local fork for session {parent_id}"))?;
    let child_id = session.id;
    let mut loaded = load_session_transcript(child_id.clone(), config).await?;
    loaded.notices.insert(0, format!("fork from session {parent_id}"));
    loaded.notices.insert(1, format!("switch to fork session {child_id}"));
    Ok(ForkedSessionTranscript::new(child_id, loaded))
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --package neo-agent --lib modes::interactive::tests::fork_session_transcript_copies_jsonl_metadata_and_loads_child 2>&1 | tail -20`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/neo-agent/src/modes/interactive/mod.rs crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "feat: update fork notices to 'fork from session' / 'switch to fork session'"
```

---

### Task 2: Add `/fork` to the slash command dispatcher

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/slash_commands.rs:46-71`

- [ ] **Step 1: Add the `"/fork"` match arm**

In `crates/neo-agent/src/modes/interactive/slash_commands.rs`, find the `handle_simple_slash_command` match block (line 46). Add a new arm after the `"/resume"` line:

**Before:**
```rust
            "/resume" => self.open_session_picker(),
            "/provider" => self.open_provider_picker(),
```

**After:**
```rust
            "/resume" => self.open_session_picker(),
            "/fork" => {
                if let Err(error) = self.fork_current_session().await {
                    self.push_status(format!("Failed to fork session: {error}"));
                }
            }
            "/provider" => self.open_provider_picker(),
```

> **Note:** `handle_simple_slash_command` returns `bool`, not `Result`, so we cannot use `?`. Errors are surfaced via `push_status` instead.

- [ ] **Step 2: Verify the project compiles**

Run: `cargo check --package neo-agent 2>&1 | tail -10`

Expected: clean compile with no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/neo-agent/src/modes/interactive/slash_commands.rs
git commit -m "feat: add /fork slash command dispatch"
```

---

### Task 3: Remove redundant `push_status` from `fork_current_session`

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/sessions.rs:88-104`
- Test: `crates/neo-agent/src/modes/interactive/tests.rs:5455-5555`

- [ ] **Step 1: Update the existing integration test that checks for the old status line**

In `crates/neo-agent/src/modes/interactive/tests.rs`, find the test `event_loop_forks_selected_session_and_continues_child_session` (line 5455). The forker closure (around line 5506-5508) currently returns a single notice:

```rust
                    [format!("forked from {SESSION_A}")],
```

Update it to return the two new notices:

```rust
                    [
                        format!("fork from session {SESSION_A}"),
                        format!("switch to fork session {SESSION_CHILD}"),
                    ],
```

Then find the assertion at line 5533-5536:

```rust
    assert!(transcript_has_status(
        &controller,
        &format!("forked from {SESSION_A}")
    ));
```

Replace it with assertions for both new notices:

```rust
    assert!(transcript_has_status(
        &controller,
        &format!("fork from session {SESSION_A}")
    ));
    assert!(transcript_has_status(
        &controller,
        &format!("switch to fork session {SESSION_CHILD}")
    ));
```

- [ ] **Step 2: Run the test to verify it still passes (notices come from mocked forker)**

Run: `cargo test --package neo-agent --lib modes::interactive::tests::event_loop_forks_selected_session_and_continues_child_session 2>&1 | tail -20`

Expected: PASS — the test uses a mocked forker that returns the notices directly, so the `fork_session_transcript` function change doesn't affect it. This confirms the test assertions match the new format.

- [ ] **Step 3: Remove the redundant `push_status` and unused `child_id`**

In `crates/neo-agent/src/modes/interactive/sessions.rs`, find `fork_current_session` (line 88).

**Before (lines 88–104):**
```rust
    pub(super) async fn fork_current_session(&mut self) -> Result<()> {
        let Some(parent_id) = self.active_session_id.clone() else {
            self.push_status("No active session to fork");
            return Ok(());
        };
        let forked = (self.fork_session)(parent_id.clone())
            .await
            .with_context(|| format!("failed to fork session {parent_id}"))?;
        let child_id = forked.session_id.clone();
        self.tui
            .chrome_mut()
            .set_session_label(forked.transcript.label.clone());
        self.rebuild_transcript_from_session(&forked.transcript);
        self.active_session_id = Some(forked.session_id);
        self.push_status(format!("Forked session {parent_id} to {child_id}"));
        Ok(())
    }
```

**After:**
```rust
    pub(super) async fn fork_current_session(&mut self) -> Result<()> {
        let Some(parent_id) = self.active_session_id.clone() else {
            self.push_status("No active session to fork");
            return Ok(());
        };
        let forked = (self.fork_session)(parent_id.clone())
            .await
            .with_context(|| format!("failed to fork session {parent_id}"))?;
        self.tui
            .chrome_mut()
            .set_session_label(forked.transcript.label.clone());
        self.rebuild_transcript_from_session(&forked.transcript);
        self.active_session_id = Some(forked.session_id);
        Ok(())
    }
```

(Removes the `let child_id = …` line and the `push_status` line. The two transcript notices now convey parent/child info.)

- [ ] **Step 4: Run the fork integration test again to verify it passes**

Run: `cargo test --package neo-agent --lib modes::interactive::tests::event_loop_forks_selected_session_and_continues_child_session 2>&1 | tail -20`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/neo-agent/src/modes/interactive/sessions.rs crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "refactor: remove redundant fork push_status, rely on transcript notices"
```

---

### Task 4: Add `/fork` to completion and help catalog

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/prompt_completion.rs:61-77`

- [ ] **Step 1: Add `/fork` to `STATIC_SLASH_COMMANDS`**

In `crates/neo-agent/src/modes/interactive/prompt_completion.rs`, find the `STATIC_SLASH_COMMANDS` array (line 61). Add `/fork` after the `/new` / `/clear` entries:

**Before:**
```rust
static STATIC_SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/resume", "Resume a local session"),
    ("/new", "Start a fresh local session"),
    ("/clear", "Alias for /new"),
```

**After:**
```rust
static STATIC_SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/resume", "Resume a local session"),
    ("/new", "Start a fresh local session"),
    ("/clear", "Alias for /new"),
    ("/fork", "Fork the current session"),
```

- [ ] **Step 2: Verify the project compiles**

Run: `cargo check --package neo-agent 2>&1 | tail -10`

Expected: clean compile.

- [ ] **Step 3: Commit**

```bash
git add crates/neo-agent/src/modes/interactive/prompt_completion.rs
git commit -m "feat: add /fork to slash command completion and help"
```

---

### Task 5: Add `/fork` slash command integration test

**Files:**
- Test: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Write the integration test**

Add a new test function at the end of the `tests` module in `crates/neo-agent/src/modes/interactive/tests.rs` (before the closing `}` of the `mod tests` block, i.e. before the very last `}` at line 10710). Insert:

```rust
#[tokio::test]
async fn slash_fork_forks_current_session_and_enters_child() {
    let mut controller = InteractiveController::new_with_event_driver_and_forker(
        "neo",
        SESSION_A,
        "openai/gpt-4.1",
        test_workspace_root(),
        move |_request| async move {
            Ok(vec![AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            }])
        },
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: Vec::new(),
        },
        |_session_id| async move {
            panic!("fork should not use the load_session callback");
            #[allow(unreachable_code)]
            Ok(LoadedSessionTranscript::new("", Vec::new(), Vec::new()))
        },
        |parent_id| async move {
            assert_eq!(parent_id, SESSION_A);
            Ok(ForkedSessionTranscript::new(
                SESSION_CHILD,
                LoadedSessionTranscript::new(
                    SESSION_CHILD,
                    [
                        format!("fork from session {SESSION_A}"),
                        format!("switch to fork session {SESSION_CHILD}"),
                    ],
                    [AgentMessage::user_text("hello")],
                ),
            ))
        },
    );
    controller.active_session_id = Some(SESSION_A.to_owned());

    let consumed = controller.handle_slash_command("/fork").await;
    assert!(consumed, "/fork should be consumed as a slash command");

    assert_eq!(
        controller.active_session_id(),
        Some(SESSION_CHILD),
        "active session switched to fork child"
    );
    assert_eq!(controller.chrome().session_label(), SESSION_CHILD);
    assert!(
        transcript_has_status(&controller, &format!("fork from session {SESSION_A}")),
        "transcript shows fork-from notice"
    );
    assert!(
        transcript_has_status(
            &controller,
            &format!("switch to fork session {SESSION_CHILD}")
        ),
        "transcript shows switch-to notice"
    );
}
```

- [ ] **Step 2: Run the test to verify it passes**

Run: `cargo test --package neo-agent --lib modes::interactive::tests::slash_fork_forks_current_session_and_enters_child 2>&1 | tail -20`

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "test: add /fork slash command integration test"
```

---

### Task 6: Full verification

- [ ] **Step 1: Run the full interactive test suite**

Run: `cargo test --package neo-agent --lib modes::interactive::tests 2>&1 | tail -20`

Expected: all tests PASS.

- [ ] **Step 2: Run clippy on the workspace**

Run: `cargo clippy --package neo-agent --lib 2>&1 | tail -20`

Expected: no new warnings.

- [ ] **Step 3: Final commit if any fixups needed**

If steps 1–2 reveal issues, fix and commit. Otherwise, no commit needed.
