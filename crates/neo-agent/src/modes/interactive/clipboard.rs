//! System clipboard write helpers with a private bounded helper lifetime.
//!
//! Uses `tokio::process::Command` with `kill_on_drop(true)`. One fixed short
//! deadline covers stdin write and child exit. This deadline is private to
//! clipboard helpers and does not affect Bash/Terminal/ShellRuntime.

use std::fmt::Display;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::time::timeout;

/// Private fixed deadline covering stdin write + child exit.
pub(super) const CLIPBOARD_HELPER_DEADLINE: Duration = Duration::from_secs(2);

/// Platform clipboard helper command specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ClipboardCommandSpec {
    pub(super) program: &'static str,
    pub(super) args: &'static [&'static str],
}

/// Native helper selection: macOS `pbcopy`, Windows `clip.exe`, Linux
/// `wl-copy` then `xclip`. Never shell-wrapped.
pub(super) fn clipboard_command_specs() -> &'static [ClipboardCommandSpec] {
    if cfg!(target_os = "macos") {
        &[ClipboardCommandSpec {
            program: "pbcopy",
            args: &[],
        }]
    } else if cfg!(target_os = "windows") {
        &[ClipboardCommandSpec {
            program: "clip.exe",
            args: &[],
        }]
    } else {
        &[
            ClipboardCommandSpec {
                program: "wl-copy",
                args: &[],
            },
            ClipboardCommandSpec {
                program: "xclip",
                args: &["-selection", "clipboard"],
            },
        ]
    }
}

/// Try platform helpers in order until one succeeds.
pub(super) async fn write_system_clipboard(text: String) -> Result<()> {
    let mut errors = Vec::new();
    for spec in clipboard_command_specs() {
        match write_clipboard_command(spec.program, spec.args, &text).await {
            Ok(()) => return Ok(()),
            Err(error) => errors.push(format!("{}: {error}", spec.program)),
        }
    }
    anyhow::bail!(
        "no system clipboard writer succeeded ({})",
        errors.join("; ")
    )
}

/// Spawn one clipboard helper, write stdin, wait for exit — all under the
/// private deadline. Drop/timeout kills the child via `kill_on_drop(true)`.
pub(super) async fn write_clipboard_command(
    program: &str,
    args: &[&str],
    text: &str,
) -> Result<()> {
    match timeout(
        CLIPBOARD_HELPER_DEADLINE,
        write_clipboard_command_inner(program, args, text),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => anyhow::bail!("clipboard helper timed out"),
    }
}

async fn write_clipboard_command_inner(program: &str, args: &[&str], text: &str) -> Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(super::clipboard_stdio_piped())
        .stdout(super::clipboard_stdio_null())
        .stderr(super::clipboard_stdio_piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("failed to start {program}"))?;

    {
        let mut stdin = child
            .stdin
            .take()
            .context("clipboard command stdin was unavailable")?;
        stdin
            .write_all(text.as_bytes())
            .await
            .with_context(|| format!("failed to write to {program}"))?;
        stdin
            .shutdown()
            .await
            .with_context(|| format!("failed to close stdin for {program}"))?;
    }

    let mut stderr_pipe = child
        .stderr
        .take()
        .context("clipboard command stderr was unavailable")?;
    let mut stderr_buf = Vec::new();
    let status = {
        let wait = child.wait();
        let read_stderr = stderr_pipe.read_to_end(&mut stderr_buf);
        let (status, read_result) = tokio::join!(wait, read_stderr);
        let _ = read_result;
        status.with_context(|| format!("failed to wait for {program}"))?
    };

    if status.success() {
        return Ok(());
    }
    Err(clipboard_exit_error(status, &stderr_buf))
}

fn clipboard_exit_error(status: impl Display, stderr: &[u8]) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(stderr).trim().to_owned();
    let suffix = if stderr.is_empty() {
        String::new()
    } else {
        format!(": {stderr}")
    };
    anyhow::anyhow!("exited with {status}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn clipboard_command_spec_uses_native_helper_without_shell() {
        let specs = clipboard_command_specs();
        assert!(!specs.is_empty());
        for spec in specs {
            assert_ne!(spec.program, "sh");
            assert_ne!(spec.program, "bash");
            assert_ne!(spec.program, "cmd");
            assert_ne!(spec.program, "cmd.exe");
            assert_ne!(spec.program, "powershell");
            assert_ne!(spec.program, "powershell.exe");
            assert!(
                !spec.program.contains('/') && !spec.program.contains('\\'),
                "helper should be a bare program name, got {}",
                spec.program
            );
        }

        if cfg!(target_os = "macos") {
            assert_eq!(specs.len(), 1);
            assert_eq!(specs[0].program, "pbcopy");
            assert!(specs[0].args.is_empty());
        } else if cfg!(target_os = "windows") {
            assert_eq!(specs.len(), 1);
            assert_eq!(specs[0].program, "clip.exe");
            assert!(specs[0].args.is_empty());
        } else {
            assert_eq!(specs.len(), 2);
            assert_eq!(specs[0].program, "wl-copy");
            assert!(specs[0].args.is_empty());
            assert_eq!(specs[1].program, "xclip");
            assert_eq!(specs[1].args, &["-selection", "clipboard"]);
        }
    }

    #[tokio::test]
    async fn clipboard_command_timeout_kills_child() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("clipboard-child.pid");
        let pid_path = pid_file.to_string_lossy().replace('\'', "");

        #[cfg(windows)]
        let (program, arg_storage): (&str, Vec<String>) = {
            let script = format!(
                "$pid | Out-File -Encoding ascii -FilePath '{pid_path}'; Start-Sleep -Seconds 60"
            );
            (
                "powershell.exe",
                vec!["-NoProfile".to_owned(), "-Command".to_owned(), script],
            )
        };
        #[cfg(not(windows))]
        let (program, arg_storage): (&str, Vec<String>) = {
            let script = format!("printf '%s\\n' $$ > '{pid_path}'; exec sleep 60");
            ("sh", vec!["-c".to_owned(), script])
        };
        let args: Vec<&str> = arg_storage.iter().map(String::as_str).collect();

        let started = Instant::now();
        let error = write_clipboard_command(program, &args, "clipboard-payload")
            .await
            .expect_err("blocking clipboard helper must time out");
        assert!(
            error.to_string().contains("timed out"),
            "unexpected error: {error}"
        );
        assert!(
            started.elapsed() < CLIPBOARD_HELPER_DEADLINE + Duration::from_secs(2),
            "timeout took too long: {:?}",
            started.elapsed()
        );

        // Child must write its pid before sleeping; wait briefly for the file.
        let pid = wait_for_pid_file(&pid_file).await;
        assert!(
            wait_until_process_gone(&pid).await,
            "clipboard helper child {pid} still alive after timeout/drop"
        );
    }

    async fn wait_for_pid_file(path: &std::path::Path) -> String {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(contents) = std::fs::read_to_string(path) {
                let pid = contents.trim().to_owned();
                if !pid.is_empty() {
                    return pid;
                }
            }
            if Instant::now() >= deadline {
                panic!("pid file was not written by clipboard helper child");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn wait_until_process_gone(pid: &str) -> bool {
        for _ in 0..100 {
            if !super::super::clipboard_test_process_exists(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        !super::super::clipboard_test_process_exists(pid)
    }
}
