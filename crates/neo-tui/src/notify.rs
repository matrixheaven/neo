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
