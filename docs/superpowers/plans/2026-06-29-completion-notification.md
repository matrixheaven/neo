# Completion Notification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add terminal bell and desktop notifications when a task finishes or the agent asks a question.

**Architecture:** New `notify.rs` module in `neo-tui` provides `NotificationMode` enum + `notify_event()` function. Config adds two fields to `TuiConfig`. `InteractiveController` calls `notify_event()` from `drain_active_turn` — via a helper method `notify_for_event()` — on both draining blocks, for `RunFinished` (completion) and `PendingQuestion` (question).

**Tech Stack:** Rust, serde, std::process::Command, std::io

**Spec:** `docs/superpowers/specs/2026-06-29-completion-notification-design.md`

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/neo-tui/src/notify.rs` | **NEW** — `NotificationMode` enum, `EventKind`, `notify_event()`, `ring_bell()`, `spawn_desktop_notification()` |
| `crates/neo-tui/src/lib.rs` | Export `notify` module |
| `crates/neo-agent/src/config/mod.rs` | `TuiConfig` add `completion_notification`, `question_notification` |
| `crates/neo-agent/src/config/types.rs` | `FileTuiConfig` add corresponding fields |
| `crates/neo-agent/src/config/loader.rs` | Map new fields in `tui_from_file`, add `NotificationMode` import |
| `crates/neo-agent/src/modes/interactive/mod.rs` | Store `completion_notification` / `question_notification` in `InteractiveController`; add `notify_for_event()` helper |
| `crates/neo-agent/src/modes/interactive/turn.rs` | Call `notify_for_event()` inside both draining blocks in `drain_active_turn` |

> **Note:** `neo-tui` is **already** a dependency of `neo-agent` (`crates/neo-agent/Cargo.toml` line 22). No Cargo.toml change is needed.

---

## Task 1: Create `notify.rs` module with `NotificationMode` + serialization

**Files:**
- Create: `crates/neo-tui/src/notify.rs`
- Modify: `crates/neo-tui/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/neo-tui/src/notify.rs` with tests first:

```rust
//! Terminal bell and desktop notification for task completion.

use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::process::{Command, Stdio};

/// Notification mode, serialized as lowercase string in config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum NotificationMode {
    None,
    #[default]
    Bell,
    System,
    All,
}

/// What kind of event triggered the notification.
#[derive(Debug, Clone, Copy)]
pub enum EventKind {
    /// A model run finished (EndTurn, ToolUse, or MaxTokens).
    Completion,
    /// The agent is requesting user input (AskUserQuestion).
    Question,
}

/// Entry point — called from `InteractiveController` when a run finishes
/// or a question is requested.
pub fn notify_event(mode: NotificationMode, kind: EventKind) {
    if matches!(mode, NotificationMode::None) {
        return;
    }
    match kind {
        EventKind::Completion => {
            if matches!(mode, NotificationMode::Bell | NotificationMode::All) {
                ring_bell();
            }
            if matches!(mode, NotificationMode::System | NotificationMode::All) {
                spawn_desktop_notification("Neo", "Task completed", None);
            }
        }
        EventKind::Question => {
            if matches!(mode, NotificationMode::Bell | NotificationMode::All) {
                ring_bell();
            }
            if matches!(mode, NotificationMode::System | NotificationMode::All) {
                spawn_desktop_notification(
                    "Neo",
                    "Question waiting",
                    Some("The agent needs your input"),
                );
            }
        }
    }
}

/// Write terminal bell byte (`\x07`) to stderr.
fn ring_bell() {
    let _ = write!(io::stderr(), "\x07");
    let _ = io::stderr().flush();
}

/// Spawn a fire-and-forget desktop notification.
///
/// macOS: `osascript -e 'display notification ...'`
/// Linux: `notify-send`
/// Errors are silently ignored — notification is best-effort.
fn spawn_desktop_notification(title: &str, body: &str, subtitle: Option<&str>) {
    let cmd = if cfg!(target_os = "macos") {
        let sub = subtitle.unwrap_or("");
        format!(
            "osascript -e 'display notification \"{body}\" with title \"{title}\" subtitle \"{sub}\"'"
        )
    } else {
        format!("notify-send \"{title}\" \"{body}\"")
    };
    let _ = Command::new("sh")
        .args(["-c", &cmd])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_mode_serializes_lowercase() {
        let json = serde_json::to_string(&NotificationMode::Bell).unwrap();
        assert_eq!(json, "\"bell\"");

        let json = serde_json::to_string(&NotificationMode::None).unwrap();
        assert_eq!(json, "\"none\"");

        let json = serde_json::to_string(&NotificationMode::System).unwrap();
        assert_eq!(json, "\"system\"");

        let json = serde_json::to_string(&NotificationMode::All).unwrap();
        assert_eq!(json, "\"all\"");
    }

    #[test]
    fn notification_mode_deserializes() {
        let mode: NotificationMode = serde_json::from_str("\"system\"").unwrap();
        assert_eq!(mode, NotificationMode::System);
    }

    #[test]
    fn notification_mode_default_is_bell() {
        assert_eq!(NotificationMode::default(), NotificationMode::Bell);
    }

    #[test]
    fn none_mode_is_noop() {
        // This just verifies the function doesn't panic
        notify_event(NotificationMode::None, EventKind::Completion);
        notify_event(NotificationMode::None, EventKind::Question);
    }
}
```

- [ ] **Step 2: Register the module in `lib.rs`**

In `crates/neo-tui/src/lib.rs`, add:

```rust
pub mod notify;
```

- [ ] **Step 3: Run tests to verify they pass**

Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/neo-tui/src/notify.rs crates/neo-tui/src/lib.rs
git commit -m "feat(neo-tui): add NotificationMode and notify_event module"
```

---

## Task 2: Add notification config to `TuiConfig`

**Files:**
- Modify: `crates/neo-agent/src/config/mod.rs`
- Modify: `crates/neo-agent/src/config/types.rs`
- Modify: `crates/neo-agent/src/config/loader.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/neo-agent/src/config/mod.rs` test module. This test deserializes real TOML into `TuiConfig` to verify the fields round-trip correctly through serde:

```rust
    #[test]
    fn tui_config_parses_notification_fields() {
        use neo_tui::notify::NotificationMode;

        let toml = r#"
            completion_notification = "all"
            question_notification = "bell"
        "#;
        let tui: TuiConfig = toml::from_str(toml).unwrap();
        assert_eq!(tui.completion_notification, NotificationMode::All);
        assert_eq!(tui.question_notification, NotificationMode::Bell);
    }

    #[test]
    fn tui_config_defaults_notification_fields() {
        use neo_tui::notify::NotificationMode;

        let tui = TuiConfig::default();
        assert_eq!(tui.completion_notification, NotificationMode::Bell);
        assert_eq!(tui.question_notification, NotificationMode::None);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Expected: FAIL — `TuiConfig` does not yet have `completion_notification` / `question_notification` fields.

- [ ] **Step 3: Add fields to `TuiConfig`**

In `crates/neo-agent/src/config/mod.rs`, add the import near the top of the file:

```rust
use neo_tui::notify::NotificationMode;
```

Currently `TuiConfig` (around line 187) derives `Default`. Add the two new fields, remove `#[derive(Default)]`, and add an explicit `Default` impl so `question_notification` defaults to `None` (not `Bell`):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
    #[serde(default)]
    pub image_protocol: ImageProtocolPreference,
    #[serde(default)]
    pub fetch_remote_images: bool,
    #[serde(default)]
    pub keybindings: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub completion_notification: NotificationMode,
    #[serde(default)]
    pub question_notification: NotificationMode,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            image_protocol: ImageProtocolPreference::default(),
            fetch_remote_images: false,
            keybindings: BTreeMap::new(),
            completion_notification: NotificationMode::Bell,
            question_notification: NotificationMode::None,
        }
    }
}
```

- [ ] **Step 4: Add fields to `FileTuiConfig`**

In `crates/neo-agent/src/config/types.rs`, add the import at the top:

```rust
use neo_tui::notify::NotificationMode;
```

Then add the two fields to `FileTuiConfig`:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct FileTuiConfig {
    pub(crate) image_protocol: Option<ImageProtocolPreference>,
    pub(crate) fetch_remote_images: Option<bool>,
    pub(crate) keybindings: Option<BTreeMap<String, Vec<String>>>,
    pub(crate) completion_notification: Option<NotificationMode>,
    pub(crate) question_notification: Option<NotificationMode>,
}
```

- [ ] **Step 5: Update `tui_from_file` in `loader.rs`**

In `crates/neo-agent/src/config/loader.rs`, add the import at the top:

```rust
use neo_tui::notify::NotificationMode;
```

Then update the `tui_from_file` function (around line 201) to map the new fields:

```rust
fn tui_from_file(tui: Option<FileTuiConfig>) -> TuiConfig {
    let Some(tui) = tui else { return TuiConfig::default(); };
    TuiConfig {
        image_protocol: tui.image_protocol.unwrap_or_default(),
        fetch_remote_images: tui.fetch_remote_images.unwrap_or(false),
        keybindings: tui.keybindings.unwrap_or_default(),
        completion_notification: tui.completion_notification.unwrap_or_default(),
        question_notification: tui
            .question_notification
            .unwrap_or(NotificationMode::None),
    }
}
```

- [ ] **Step 6: Run build and focused tests**

Run: `cargo build -p neo-agent`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add crates/neo-agent/src/config/
git commit -m "feat(neo-agent): add completion_notification and question_notification to TuiConfig"
```

---

## Task 3: Wire notification into `InteractiveController`

**Key facts about the code structure:**

- The struct is **`InteractiveController`** (NOT `InteractiveMode`), defined at `crates/neo-agent/src/modes/interactive/mod.rs:289`. It has ~70 fields.
- Constructor `new()` is at line 592 and initializes all fields in a big struct literal.
- Test constructors: `new_with_event_driver` (line 704) and `new_with_event_driver_and_forker` (line 733).
- `drain_active_turn` lives in `crates/neo-agent/src/modes/interactive/turn.rs:121-188`. It has **two** draining blocks (pre-completion and post-completion), each draining `session_ids`, `approvals`, `questions`, and `events`.
- Events arrive as `Result<AgentEvent, anyhow::Error>`; the inner `AgentEvent` is available only inside the `Ok(event) =>` arm.
- Questions arrive as `PendingQuestion` via `turn.questions.try_recv()`.
- We insert notification calls **into** the existing draining loops — we do NOT rewrite the loops or remove the `session_ids`/`approvals` draining.
- We add a helper method `notify_for_event(&self, event: &AgentEvent)` on `InteractiveController` and call it from the `Ok(event)` arm in **both** draining blocks. This avoids duplication.
- For questions, we add the notification call **only** in the `turn.questions` draining loop (both blocks), NOT in the events loop — because questions arrive solely via the `turn.questions` channel; the `QuestionRequested` event stream path is internal to `register_pending_question`.

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive/turn.rs`

- [ ] **Step 1: Add notification fields to `InteractiveController`**

In `crates/neo-agent/src/modes/interactive/mod.rs`, add two fields to the `InteractiveController` struct (line 289):

```rust
pub struct InteractiveController {
    // ... existing ~70 fields ...
    completion_notification: neo_tui::notify::NotificationMode,
    question_notification: neo_tui::notify::NotificationMode,
}
```

- [ ] **Step 2: Add `notify_for_event` helper method**

In `crates/neo-agent/src/modes/interactive/mod.rs` (in an `impl InteractiveController` block, alongside other helper methods), add:

```rust
/// Fire a notification for the given event based on the controller's
/// configured notification modes. Called from `drain_active_turn`.
fn notify_for_event(&self, event: &AgentEvent) {
    use neo_agent_core::StopReason;
    if let AgentEvent::RunFinished { stop_reason, .. } = event {
        if matches!(
            stop_reason,
            StopReason::EndTurn | StopReason::ToolUse | StopReason::MaxTokens
        ) {
            neo_tui::notify::notify_event(
                self.completion_notification,
                neo_tui::notify::EventKind::Completion,
            );
        }
    }
}
```

> **Why not handle `QuestionRequested` here?** Questions arrive **only** via the `turn.questions` channel (as `PendingQuestion`), and the `QuestionRequested` event in the event stream is an internal detail of `register_pending_question`. Adding it in the events loop would double-fire. So question notification goes in the `turn.questions` loop only (Step 4 below).
>
> If `StopReason` is not already in scope at this call site, the `use neo_agent_core::StopReason;` inside the method body keeps the import local. Verify the exact path by checking existing imports in `mod.rs` / `turn.rs`; `StopReason` is re-exported from `neo_agent_core`.

- [ ] **Step 3: Initialize the new fields in `new()`**

In the main `new()` constructor (line 592, struct literal runs through ~643–700), add the two fields. Read `completion_notification` / `question_notification` from the `TuiConfig` passed via `app_config` (or wherever the existing fields like `fetch_remote_images` are sourced). Add inside the struct literal:

```rust
completion_notification: app_config.tui.completion_notification,
question_notification: app_config.tui.question_notification,
```

> Match the existing pattern in `new()` for how `TuiConfig` fields are accessed. If `new()` takes individual parameters rather than the whole `AppConfig`, follow whatever the other TUI-derived fields do.

Also update the test constructors that build a full `InteractiveController` struct literal — `new_with_event_driver` (line 704) and `new_with_event_driver_and_forker` (line 733) — to include:

```rust
completion_notification: neo_tui::notify::NotificationMode::None,
question_notification: neo_tui::notify::NotificationMode::None,
```

(Use `None` for tests so tests don't ring bells or spawn notifications.)

- [ ] **Step 4: Insert notification calls into `drain_active_turn`**

In `crates/neo-agent/src/modes/interactive/turn.rs:121-188`, the method has **two** draining blocks:

1. **Pre-completion block** (lines 126–142)
2. **Post-completion block** (lines 149–165)

Each block contains four draining loops: `session_ids`, `approvals`, `questions`, `events`. **Do not rewrite or remove any of these loops.** Insert notification calls *inside* the existing loops as follows.

**In the `events` loop** of **both** blocks — events arrive as `Result<AgentEvent, anyhow::Error>`, so the call goes inside the `Ok(event) =>` arm. Current code:

```rust
while let Ok(event) = turn.events.try_recv() {
    match event {
        Ok(event) => self.apply_turn_event(event),
        Err(error) => {
            self.push_status(format!("Error: {error}"));
        }
    }
}
```

Change the `Ok(event) =>` arm to call the helper before `apply_turn_event`:

```rust
while let Ok(event) = turn.events.try_recv() {
    match event {
        Ok(event) => {
            self.notify_for_event(&event);
            self.apply_turn_event(event);
        }
        Err(error) => {
            self.push_status(format!("Error: {error}"));
        }
    }
}
```

Apply this exact change in **both** the pre-completion block and the post-completion block.

**In the `questions` loop** of **both** blocks — questions arrive as `PendingQuestion`. Current code:

```rust
while let Ok(pending) = turn.questions.try_recv() {
    self.register_pending_question(pending);
}
```

Add the notification call before `register_pending_question`:

```rust
while let Ok(pending) = turn.questions.try_recv() {
    neo_tui::notify::notify_event(
        self.question_notification,
        neo_tui::notify::EventKind::Question,
    );
    self.register_pending_question(pending);
}
```

Apply this exact change in **both** the pre-completion block and the post-completion block.

> **Do NOT add a `QuestionRequested` case to `notify_for_event` or to the events loop.** Questions are notified solely via the `turn.questions` channel path above; handling them in the events loop too would double-fire.

- [ ] **Step 5: Verify imports**

- `AgentEvent` is already in scope in `turn.rs` (used by `apply_turn_event`). The helper `notify_for_event` takes `&AgentEvent`, so it must be in scope in `mod.rs` where the helper is defined — check that `AgentEvent` is imported there (it's a common import; if missing, add `use neo_agent_core::AgentEvent;` or whatever path the codebase already uses).
- `StopReason` is imported locally inside `notify_for_event` via `use neo_agent_core::StopReason;`. If the codebase already imports `StopReason` at module level in `mod.rs`, you can drop the local `use` and rely on the existing import instead.
- `neo_tui::notify::*` is referenced by full path — no new top-level imports needed.

- [ ] **Step 6: Run build and fix compilation**

Run: `cargo build -p neo-agent`
Expected: PASS

- [ ] **Step 7: Run focused tests**

Expected: PASS — existing interactive-mode tests must still pass; test constructors initialize notification modes to `None` so they won't fire.

- [ ] **Step 8: Commit**

```bash
git add crates/neo-agent/src/modes/interactive/
git commit -m "feat(neo-agent): trigger bell/notification on run completion and questions"
```

---

## Task 4: Manual smoke test in TUI mode

Notifications fire from `drain_active_turn`, which runs only in the interactive TUI path — **not** in `--print` mode. So the smoke test must use the TUI.

- [ ] **Step 1: Build the binary**

Run: `cargo build -p neo-agent`
Expected: PASS

- [ ] **Step 2: Configure notification**

Create or edit `~/.neo/config.toml`:

```toml
[tui]
completion_notification = "bell"
```

- [ ] **Step 3: Run in TUI mode and verify the bell**

Run: `cargo run -p neo-agent`
In the TUI, type a short prompt (e.g. "say hello") and press Enter.
Expected: Terminal bell sounds when the response finishes.

- [ ] **Step 4: Test system notification**

Change config to:

```toml
[tui]
completion_notification = "system"
```

Restart `cargo run -p neo-agent` and submit another short prompt.
Expected: macOS notification appears ("Neo — Task completed").

- [ ] **Step 5: Test question notification**

Set:

```toml
[tui]
question_notification = "bell"
```

Trigger a question (e.g. a prompt that causes `ask_user`), or confirm the bell fires when the agent requests input via the `turn.questions` channel.
Expected: Terminal bell sounds when the question dialog appears.

- [ ] **Step 6: Commit any final fixes**

```bash
git add -u
git commit -m "feat: completion notification system"
```
