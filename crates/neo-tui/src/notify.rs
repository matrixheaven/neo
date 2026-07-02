//! Terminal bell and desktop notification for task completion.

use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
struct NotificationCommand {
    program: &'static str,
    args: Vec<String>,
}

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
    /// A model run finished (`EndTurn`, `ToolUse`, or `MaxTokens`).
    Completion,
    /// The agent is requesting user input (`AskUserQuestion`).
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
/// Errors are silently ignored — notification is best-effort.
fn spawn_desktop_notification(title: &str, body: &str, subtitle: Option<&str>) {
    let Some(command) = desktop_notification_command(title, body, subtitle) else {
        return;
    };
    let _ = Command::new(command.program)
        .args(command.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn desktop_notification_command(
    title: &str,
    body: &str,
    subtitle: Option<&str>,
) -> Option<NotificationCommand> {
    #[cfg(target_os = "macos")]
    {
        let mut script = format!("display notification {body:?} with title {title:?}");
        if let Some(subtitle) = subtitle.filter(|value| !value.is_empty()) {
            script.push_str(&format!(" subtitle {subtitle:?}"));
        }
        return Some(NotificationCommand {
            program: "osascript",
            args: vec!["-e".to_owned(), script],
        });
    }

    #[cfg(target_os = "linux")]
    {
        let _ = subtitle;
        return Some(NotificationCommand {
            program: "notify-send",
            args: vec![title.to_owned(), body.to_owned()],
        });
    }

    #[allow(unreachable_code)]
    {
        let _ = (title, body, subtitle);
        None
    }
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

    #[test]
    fn desktop_notification_command_uses_platform_binary_without_shell() {
        let command = desktop_notification_command("Neo", "Task completed", Some("Done"));

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            assert!(command.is_none());
            return;
        }

        let command = command.expect("notification command on supported platform");

        assert_ne!(command.program, "sh");
        assert!(!command.args.iter().any(|arg| arg == "-c"));

        #[cfg(target_os = "macos")]
        {
            assert_eq!(command.program, "osascript");
            assert_eq!(command.args[0], "-e");
            assert!(command.args[1].contains("display notification"));
        }

        #[cfg(target_os = "linux")]
        {
            assert_eq!(command.program, "notify-send");
            assert_eq!(command.args, vec!["Neo", "Task completed"]);
        }
    }
}
