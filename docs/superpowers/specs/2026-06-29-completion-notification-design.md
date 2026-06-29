# Spec B: Completion Notification

**Date:** 2026-06-29
**Status:** Approved (design phase)
**Crates affected:** `neo-tui`, `neo-agent`

## Motivation

Neo has no notification when a long-running task finishes. When the user switches to another window while the agent works, there is no bell, no desktop notification, no indication that the agent is done. The status bar spinner simply disappears.

kimi-code has two notification systems:
- `useSoundNotification.ts` — WebAudio-synthesized "ding-dong" chime, default off
- `useNotification.ts` — browser Notification API desktop notification, default on for completion, default off for questions

Neo is a TUI application, so the mechanisms differ (terminal bell + OS notification commands instead of browser APIs), but the user-facing design is the same: configurable notification on task completion and on agent questions.

## Design

### Configuration

```toml
# ~/.neo/config.toml
[tui]
# Completion notification mode: "none" | "bell" | "system" | "all"
# none   = no notification
# bell   = terminal bell (\x07)
# system = desktop notification (macOS osascript / Linux notify-send)
# all    = both bell and desktop notification
completion_notification = "bell"

# Notification when agent asks a question: "none" | "bell" | "system" | "all"
question_notification = "none"
```

Default values (following kimi-code's privacy-conscious choices):
- `completion_notification`: `"bell"` — lightweight, non-intrusive
- `question_notification`: `"none"` — avoid noise

### Notification Module

**New file:** `crates/neo-tui/src/notify.rs`

```rust
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::process::{Stdio, Command};

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

/// Entry point — called from the TUI event loop when a run finishes
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

/// Write terminal bell byte (\x07) to stderr.
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
            "osascript -e 'display notification \"{}\" with title \"{}\" subtitle \"{}\"'",
            body, title, sub
        )
    } else {
        format!("notify-send \"{}\" \"{}\"", title, body)
    };
    let _ = Command::new("sh")
        .args(["-c", &cmd])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
```

### Design Decisions

1. **No `isActiveAndVisible` suppression.** kimi-code's `useNotification.ts` suppresses desktop notifications when the browser tab is active and visible. Neo cannot reliably detect terminal focus — terminal emulators don't expose this via standard APIs. The bell is quiet enough that suppression isn't needed; desktop notifications can be disabled via config if undesired.

2. **Fire-and-forget `spawn()`.** Desktop notifications use `std::process::Command::spawn()` (not `output()` or `status()`), so they never block the TUI event loop. The child process is detached.

3. **Bell goes to stderr.** stdout is used for TUI rendering; writing the bell to stderr avoids corrupting the ratatui frame buffer.

4. **No sound synthesis.** kimi-code uses WebAudio to synthesize a two-note chime. Neo uses the terminal bell — it's universal (every terminal emulator supports `\x07`), requires no audio library, and respects the user's terminal bell settings (visual bell, mute, etc.).

### Trigger Points

**1. Run completion** — in `drain_active_turn()` when `RunFinished` arrives:

```rust
// crates/neo-agent/src/modes/interactive/turn.rs
AgentEvent::RunFinished { stop_reason, .. } => {
    // Only notify on "successful" stops — not on Error or Cancelled
    if matches!(stop_reason, StopReason::EndTurn | StopReason::ToolUse | StopReason::MaxTokens) {
        notify_event(self.notification_mode, EventKind::Completion);
    }
}
```

Error and Cancelled stops are not notified — a notification for failure is misleading (the user needs to read the error, not just know "it's done").

**2. Agent question** — when `QuestionRequested` arrives:

```rust
AgentEvent::QuestionRequested { .. } => {
    notify_event(self.notification_mode, EventKind::Question);
}
```

### Config Integration

```rust
// crates/neo-agent/src/config/mod.rs
pub struct TuiConfig {
    // ... existing fields (image_protocol, fetch_remote_images, keybindings) ...
    pub completion_notification: NotificationMode,
    pub question_notification: NotificationMode,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            // ... existing defaults ...
            completion_notification: NotificationMode::Bell,
            question_notification: NotificationMode::None,
        }
    }
}
```

```rust
// crates/neo-agent/src/config/types.rs
pub(crate) struct FileTuiConfig {
    // ... existing fields ...
    pub(crate) completion_notification: Option<NotificationMode>,
    pub(crate) question_notification: Option<NotificationMode>,
}
```

The `NotificationMode` type lives in `neo-tui::notify` and is re-exported so `neo-agent` config can use it. Alternatively, it can live in `neo-agent::config::types` and be imported by `neo-tui`. The former is preferred since it keeps notification logic together.

### Notification Mode Propagation

`InteractiveMode` reads `notification_mode` from config at initialization and passes it to the turn driver. The notification mode is a static config value — it does not change at runtime and does not need to flow through the agent runtime layer. It is purely a TUI concern.

## Migration Impact

| File | Change |
|---|---|
| `crates/neo-tui/src/notify.rs` | **New file**: `NotificationMode`, `EventKind`, `notify_event()`, `ring_bell()`, `spawn_desktop_notification()` |
| `crates/neo-tui/src/lib.rs` | Export `notify` module |
| `crates/neo-agent/src/config/mod.rs` | `TuiConfig` add `completion_notification`, `question_notification` |
| `crates/neo-agent/src/config/types.rs` | `FileTuiConfig` add corresponding fields |
| `crates/neo-agent/src/modes/interactive/turn.rs` | `drain_active_turn()`: call `notify_event()` on `RunFinished` and `QuestionRequested` |
| `crates/neo-agent/src/modes/interactive/mod.rs` | Read `notification_mode` from config at init, store in `InteractiveMode` state |

## Testing Strategy

### Serialization tests

- `NotificationMode` serializes as lowercase strings: `"none"`, `"bell"`, `"system"`, `"all"`
- `NotificationMode::default()` == `Bell`
- Deserialization round-trip for all 4 variants
- Unknown string → fallback to default (or error, depending on serde behavior)

### Behavior tests

- `NotificationMode::None` → `notify_event()` is a no-op (no bell, no spawn)
- `EventKind::Completion` with `Bell` mode → `ring_bell()` called (verify stderr output contains `\x07`)
- `EventKind::Completion` with `System` mode → `spawn_desktop_notification()` called (verify `Command::spawn` invoked — mock or integration test)
- `EventKind::Question` distinguishes from `EventKind::Completion` in notification body text
- `spawn_desktop_notification()` does not block (spawn is non-blocking by design)
- Config parsing: `[tui] completion_notification = "all"` correctly deserializes

### What is NOT tested

- Actual desktop notification display (requires OS GUI, not unit-testable)
- Actual terminal bell audibility (depends on terminal emulator settings)
- Platform-specific `osascript`/`notify-send` availability (runtime best-effort)
