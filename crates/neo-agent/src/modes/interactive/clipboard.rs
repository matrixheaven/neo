//! Extracted: system clipboard write helpers.

use std::io::Write as _;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

pub(super) fn write_system_clipboard(text: &str) -> Result<()> {
    let mut errors = Vec::new();
    for (program, args) in clipboard_commands() {
        match write_clipboard_command(program, args, text) {
            Ok(()) => return Ok(()),
            Err(error) => errors.push(format!("{program}: {error}")),
        }
    }
    anyhow::bail!(
        "no system clipboard writer succeeded ({})",
        errors.join("; ")
    )
}

fn clipboard_commands() -> &'static [(&'static str, &'static [&'static str])] {
    if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else if cfg!(target_os = "windows") {
        &[("clip.exe", &[])]
    } else {
        &[("wl-copy", &[]), ("xclip", &["-selection", "clipboard"])]
    }
}

fn write_clipboard_command(program: &str, args: &[&str], text: &str) -> Result<()> {
    let mut child = spawn_clipboard_command(program, args)?;
    write_clipboard_stdin(&mut child, program, text)?;
    let output = wait_clipboard_command(child, program)?;
    if output.status.success() {
        return Ok(());
    }
    Err(clipboard_exit_error(&output))
}

fn spawn_clipboard_command(program: &str, args: &[&str]) -> Result<std::process::Child> {
    Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start {program}"))
}

fn write_clipboard_stdin(child: &mut std::process::Child, program: &str, text: &str) -> Result<()> {
    child
        .stdin
        .as_mut()
        .context("clipboard command stdin was unavailable")?
        .write_all(text.as_bytes())
        .with_context(|| format!("failed to write to {program}"))
}

fn wait_clipboard_command(
    child: std::process::Child,
    program: &str,
) -> Result<std::process::Output> {
    child
        .wait_with_output()
        .with_context(|| format!("failed to wait for {program}"))
}

fn clipboard_exit_error(output: &std::process::Output) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let suffix = if stderr.is_empty() {
        String::new()
    } else {
        format!(": {stderr}")
    };
    anyhow::anyhow!("exited with {}{}", output.status, suffix)
}
