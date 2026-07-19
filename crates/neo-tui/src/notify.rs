//! Terminal bell and desktop notification for task completion.

use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(any(windows, test))]
use base64::Engine as _;

static NOTIFICATION_ERROR_REPORTED: AtomicBool = AtomicBool::new(false);

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
fn spawn_desktop_notification(title: &str, body: &str, subtitle: Option<&str>) {
    let Some(command) = desktop_notification_command(title, body, subtitle) else {
        report_notification_error_once("desktop notifications are unsupported on this platform");
        return;
    };
    match Command::new(command.program)
        .args(command.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => {
            let child = Arc::new(Mutex::new(Some(child)));
            let waiter = Arc::clone(&child);
            if let Err(error) = std::thread::Builder::new()
                .name("neo-notification".to_owned())
                .spawn(move || report_notification_exit(wait_for_notification_child(&waiter)))
            {
                stop_notification_child(&child);
                report_notification_error_once(&format!(
                    "failed to start desktop notification waiter: {error}"
                ));
            }
        }
        Err(error) => report_notification_error_once(&format!(
            "failed to start desktop notification command: {error}"
        )),
    }
}

fn wait_for_notification_child(child: &Mutex<Option<Child>>) -> io::Result<ExitStatus> {
    let mut child = child
        .lock()
        .map_err(|_| io::Error::other("desktop notification child lock poisoned"))?
        .take()
        .ok_or_else(|| io::Error::other("desktop notification child already taken"))?;
    child.wait()
}

fn stop_notification_child(child: &Mutex<Option<Child>>) {
    let Ok(mut child) = child.lock() else {
        return;
    };
    if let Some(mut child) = child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn report_notification_exit(result: io::Result<ExitStatus>) {
    if let Some(message) = notification_exit_diagnostic(result) {
        report_notification_error_once(&message);
    }
}

fn notification_exit_diagnostic(result: io::Result<ExitStatus>) -> Option<String> {
    match result {
        Ok(status) if status.success() => None,
        Ok(status) => Some(format!("desktop notification command exited with {status}")),
        Err(error) => Some(format!(
            "failed to wait for desktop notification command: {error}"
        )),
    }
}

fn report_notification_error_once(message: &str) {
    if !NOTIFICATION_ERROR_REPORTED.swap(true, Ordering::Relaxed) {
        eprintln!("Neo notification: {message}");
    }
}

fn desktop_notification_command(
    title: &str,
    body: &str,
    subtitle: Option<&str>,
) -> Option<NotificationCommand> {
    #[cfg(target_os = "macos")]
    {
        let script = if let Some(subtitle) = subtitle.filter(|value| !value.is_empty()) {
            format!("display notification {body:?} with title {title:?} subtitle {subtitle:?}")
        } else {
            format!("display notification {body:?} with title {title:?}")
        };
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

    #[cfg(windows)]
    {
        return Some(windows_notification_command(title, body, subtitle));
    }

    #[allow(unreachable_code)]
    {
        let _ = (title, body, subtitle);
        None
    }
}

#[cfg(any(windows, test))]
fn windows_notification_command(
    title: &str,
    body: &str,
    subtitle: Option<&str>,
) -> NotificationCommand {
    let body = subtitle
        .filter(|value| !value.is_empty())
        .map_or_else(|| body.to_owned(), |subtitle| format!("{body}\n{subtitle}"));
    let encode = |value: &str| base64::engine::general_purpose::STANDARD.encode(value);
    let script = format!(
        "$ErrorActionPreference='Stop';\
         $title=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{}'));\
         $body=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{}'));\
         [Windows.UI.Notifications.ToastNotificationManager,Windows.UI.Notifications,ContentType=WindowsRuntime]>$null;\
         $xml=[Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02);\
         $text=$xml.GetElementsByTagName('text');\
         $text.Item(0).AppendChild($xml.CreateTextNode($title))>$null;\
         $text.Item(1).AppendChild($xml.CreateTextNode($body))>$null;\
         $toast=[Windows.UI.Notifications.ToastNotification]::new($xml);\
         [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Neo').Show($toast)",
        encode(title),
        encode(&body)
    );
    NotificationCommand {
        program: "powershell.exe",
        args: vec![
            "-NoProfile".to_owned(),
            "-NonInteractive".to_owned(),
            "-Command".to_owned(),
            script,
        ],
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

        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
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

        #[cfg(windows)]
        {
            assert_eq!(command.program, "powershell.exe");
            assert_eq!(
                &command.args[..3],
                ["-NoProfile", "-NonInteractive", "-Command"]
            );
        }
    }

    #[test]
    fn windows_notification_command_encodes_untrusted_text() {
        let title = "Neo'; Write-Error injected; '";
        let body = "Task completed $(Write-Error injected)";
        let subtitle = "line two\n'quoted'";
        let command = windows_notification_command(title, body, Some(subtitle));
        let script = command.args.last().unwrap();

        assert_eq!(command.program, "powershell.exe");
        assert!(!script.contains(title));
        assert!(!script.contains(body));
        assert!(!script.contains(subtitle));
        assert!(script.contains(&base64::engine::general_purpose::STANDARD.encode(title)));
        assert!(script.contains(
            &base64::engine::general_purpose::STANDARD.encode(format!("{body}\n{subtitle}"))
        ));
        assert!(script.starts_with("$ErrorActionPreference='Stop';"));
    }

    #[test]
    fn notification_exit_diagnostic_formats_nonzero_status() {
        #[cfg(unix)]
        let status = {
            use std::os::unix::process::ExitStatusExt as _;
            ExitStatus::from_raw(7 << 8)
        };
        #[cfg(windows)]
        let status = {
            use std::os::windows::process::ExitStatusExt as _;
            ExitStatus::from_raw(7)
        };
        #[cfg(not(any(unix, windows)))]
        return;

        let diagnostic = notification_exit_diagnostic(Ok(status)).unwrap();
        assert!(diagnostic.contains("exited with"));
        assert!(diagnostic.contains('7'));
    }
}
