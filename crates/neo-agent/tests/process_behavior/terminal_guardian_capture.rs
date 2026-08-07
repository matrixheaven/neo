//! Terminal guardian capture/output behavior (split from `terminal_guardian.rs`).

use super::terminal_guardian::{guarded_context, serial_guard, start_terminal_command};

#[cfg(windows)]
use super::terminal_guardian::{interactive_shell_command, windows_powershell_command};
use std::time::Duration;

#[cfg(unix)]
use std::process::Stdio;

#[cfg(windows)]
use base64::Engine as _;
use neo_agent_core::{ShellLimits, ToolContext, ToolError, ToolRegistry};
use serde_json::json;

pub(crate) async fn run_one_attempt(
    registry: &ToolRegistry,
    context: &ToolContext,
) -> Result<(), String> {
    #[cfg(windows)]
    let command = interactive_shell_command();
    #[cfg(not(windows))]
    let command =
        "while :; do set -- $(stty size); printf 'size:%s %s\\n' \"$1\" \"$2\"; sleep 0.1; done"
            .to_owned();
    let details = start_terminal_command(registry, context, command, 40, 8, 500).await?;
    let handle = details["handle"]
        .as_str()
        .expect("terminal handle")
        .to_owned();
    assert_ne!(details["guardian_pid"], details["command_pid"]);

    let size_query = if cfg!(windows) {
        "CMD:PTY\n"
    } else {
        "write-probe\n"
    };
    let written = match registry
        .run(
            "Terminal",
            context,
            json!({
                "mode": "write",
                "handle": handle,
                "input": [{ "text": size_query }],
                "yield_time_ms": 1500
            }),
        )
        .await
    {
        Ok(result) => result,
        Err(e) => {
            try_stop(registry, context, &handle).await;
            return Err(format!("terminal write: {e}"));
        }
    };
    // write now yields bounded PTY output and advances read_offset.
    let mut output = details["output"].as_str().unwrap_or_default().to_owned();
    output.push_str(
        written
            .details
            .as_ref()
            .and_then(|details| details["output"].as_str())
            .unwrap_or_default(),
    );
    let initial_size = if cfg!(windows) {
        "pty:40:8"
    } else {
        "size:8 40"
    };
    if !output.contains(initial_size) {
        output.push_str(&read_until(registry, context, &handle, initial_size).await);
    }
    if !output.contains(initial_size) || (!cfg!(windows) && !output.contains("write-probe")) {
        try_stop(registry, context, &handle).await;
        return Err(format!("terminal output: {output:?}"));
    }

    if let Err(e) = registry
        .run(
            "Terminal",
            context,
            json!({ "mode": "resize", "handle": handle, "cols": 72, "rows": 18 }),
        )
        .await
    {
        try_stop(registry, context, &handle).await;
        return Err(format!("terminal resize: {e}"));
    }
    #[cfg(windows)]
    let resized_write = match registry
        .run(
            "Terminal",
            context,
            json!({
                "mode": "write",
                "handle": handle,
                "input": [{ "text": "CMD:SIZE:72:18\n" }],
                "yield_time_ms": 1500
            }),
        )
        .await
    {
        Ok(result) => result,
        Err(e) => {
            try_stop(registry, context, &handle).await;
            return Err(format!("write after resize: {e}"));
        }
    };
    #[cfg(not(windows))]
    let resized_write = match registry
        .run(
            "Terminal",
            context,
            json!({
                "mode": "read",
                "handle": handle,
                "yield_time_ms": 1500
            }),
        )
        .await
    {
        Ok(result) => result,
        Err(e) => {
            try_stop(registry, context, &handle).await;
            return Err(format!("read after resize: {e}"));
        }
    };
    let mut output = resized_write
        .details
        .as_ref()
        .and_then(|details| details["output"].as_str())
        .unwrap_or_default()
        .to_owned();
    if !output.contains("size:18 72") {
        output.push_str(&read_until(registry, context, &handle, "size:18 72").await);
    }
    if !output.contains("size:18 72") {
        try_stop(registry, context, &handle).await;
        return Err(format!("resized output: {output:?}"));
    }

    let stopped = registry
        .run(
            "Terminal",
            context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .map_err(|e| format!("terminal stop: {e}"))?;
    assert!(
        matches!(
            stopped.details.as_ref().unwrap()["status"].as_str(),
            Some("cancelled" | "completed" | "failed")
        ),
        "unexpected stop status: {:?}",
        stopped.details.as_ref().unwrap()["status"]
    );
    assert!(matches!(
        registry
            .run(
                "Terminal",
                context,
                json!({ "mode": "read", "handle": handle }),
            )
            .await,
        Err(ToolError::InvalidInput { .. })
    ));
    Ok(())
}

pub(crate) async fn try_stop(registry: &ToolRegistry, context: &ToolContext, handle: &str) {
    let _ = registry
        .run(
            "Terminal",
            context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await;
}

pub(crate) async fn run_max_output_cap_attempt(
    registry: &ToolRegistry,
    context: &ToolContext,
    _workspace: &tempfile::TempDir,
) -> Result<(), String> {
    #[cfg(windows)]
    let command = windows_powershell_command(
        "Start-Sleep -Milliseconds 200; [Console]::Out.Write([IO.File]::ReadAllText((Resolve-Path -LiteralPath 'payload.txt'))); [Console]::Out.Flush(); Start-Sleep -Seconds 300",
    );
    #[cfg(not(windows))]
    let command = "sleep 0.2; cat payload.txt; sleep 300".to_owned();
    let details = start_terminal_command(registry, context, command, 80, 24, 0).await?;
    let handle = details["handle"]
        .as_str()
        .ok_or_else(|| "missing handle".to_owned())?
        .to_owned();

    let mut read = None;
    for _ in 0..30 {
        let result = match registry
            .run(
                "Terminal",
                context,
                json!({
                    "mode": "read",
                    "handle": handle,
                    "max_output_bytes": 4,
                    "yield_time_ms": 100
                }),
            )
            .await
        {
            Ok(result) => result,
            Err(e) => {
                try_stop(registry, context, &handle).await;
                return Err(format!("read: {e}"));
            }
        };
        let truncated = result.content.contains("truncated: true")
            || result
                .details
                .as_ref()
                .and_then(|details| details["truncated"].as_bool())
                .unwrap_or(false)
            || result
                .details
                .as_ref()
                .and_then(|details| details["output_truncated"].as_bool())
                .unwrap_or(false);
        let has_output = result
            .details
            .as_ref()
            .and_then(|details| details["output"].as_str())
            .is_some_and(|output| !output.is_empty());
        if truncated || has_output {
            read = Some(result);
            break;
        }
    }
    let read = if let Some(read) = read {
        read
    } else {
        try_stop(registry, context, &handle).await;
        return Err("expected capped terminal read".to_owned());
    };
    let serialized = serde_json::to_string(&read).map_err(|e| format!("serialize: {e}"))?;
    let truncated = read.content.contains("truncated: true")
        || read
            .details
            .as_ref()
            .and_then(|details| details["output_truncated"].as_bool())
            .unwrap_or(false);
    if !truncated {
        try_stop(registry, context, &handle).await;
        return Err(format!("missing truncation markers: {}", read.content));
    }
    if serialized.contains("terminal-leak-tail") {
        try_stop(registry, context, &handle).await;
        return Err(format!("capped payload leaked full tail: {serialized}"));
    }
    let output = read
        .details
        .as_ref()
        .and_then(|details| details["output"].as_str())
        .unwrap_or_default();
    if output.len() > 4 {
        try_stop(registry, context, &handle).await;
        return Err(format!("output longer than cap: {output:?}"));
    }

    registry
        .run(
            "Terminal",
            context,
            json!({ "mode": "stop", "handle": handle, "max_output_bytes": 4 }),
        )
        .await
        .map_err(|e| format!("stop: {e}"))?;
    Ok(())
}

pub(crate) async fn read_until(
    registry: &ToolRegistry,
    context: &ToolContext,
    handle: &str,
    needle: &str,
) -> String {
    let mut output = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now()) {
        let result = match tokio::time::timeout(
            remaining,
            registry.run(
                "Terminal",
                context,
                json!({ "mode": "read", "handle": handle, "yield_time_ms": 100 }),
            ),
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(_)) | Err(_) => break,
        };
        output.push_str(result.details.as_ref().unwrap()["output"].as_str().unwrap());
        if output.contains(needle) {
            return output;
        }
        if result.details.as_ref().unwrap()["status"].as_str() != Some("running") {
            break;
        }
    }
    output
}

pub(crate) async fn run_incremental_bounded_attempt(
    registry: &ToolRegistry,
    context: &ToolContext,
    workspace: &tempfile::TempDir,
) -> Result<(), String> {
    let subdir = workspace.path().join(format!(
        "subdir-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    std::fs::create_dir_all(&subdir).map_err(|e| format!("subdir: {e}"))?;
    std::fs::write(subdir.join("marker"), b"ok").map_err(|e| format!("marker: {e}"))?;
    let cwd = subdir
        .strip_prefix(workspace.path())
        .map_err(|e| format!("cwd strip: {e}"))?
        .to_string_lossy()
        .into_owned();
    let command =
        "test -f marker && printf initial-output; read line; printf 'reply:%s' \"$line\"; sleep 300"
            .to_owned();
    let started = registry
        .run(
            "Terminal",
            context,
            json!({
                "mode": "start",
                "cwd": cwd,
                "yield_time_ms": 2500,
                "command": command
            }),
        )
        .await
        .map_err(|e| format!("terminal start: {e}"))?;
    let handle = started
        .details
        .as_ref()
        .and_then(|details| details["handle"].as_str())
        .ok_or_else(|| "missing handle".to_owned())?
        .to_owned();
    let observed_output = started
        .details
        .as_ref()
        .and_then(|d| d["output"].as_str())
        .unwrap_or_default()
        .to_owned();
    #[cfg(windows)]
    if observed_output.contains("\u{1b}[6n") || started.content.contains("\u{1b}[6n") {
        try_stop(registry, context, &handle).await;
        return Err("ConPTY requested inherited cursor state".to_owned());
    }
    if !observed_output.contains("initial-output") && !started.content.contains("initial-output") {
        try_stop(registry, context, &handle).await;
        return Err(format!(
            "start/handshake did not collect cwd-gated initial output: content={:?} output={observed_output:?}",
            started.content
        ));
    }
    let status = started
        .details
        .as_ref()
        .and_then(|d| d["status"].as_str())
        .unwrap_or("missing");
    if status != "running" {
        try_stop(registry, context, &handle).await;
        return Err(format!(
            "start not running: {status}; content={:?}; details={:?}",
            started.content, started.details
        ));
    }

    // Shared offset: a zero-yield read after consuming initial-output must not
    // re-emit it.
    let immediate = match registry
        .run(
            "Terminal",
            context,
            json!({
                "mode": "read",
                "handle": handle,
                "yield_time_ms": 0
            }),
        )
        .await
    {
        Ok(result) => result,
        Err(error) => {
            try_stop(registry, context, &handle).await;
            return Err(format!("immediate read: {error}"));
        }
    };
    let immediate_output = immediate
        .details
        .as_ref()
        .and_then(|details| details["output"].as_str())
        .unwrap_or_default();
    if immediate_output.contains("initial-output") {
        try_stop(registry, context, &handle).await;
        return Err(format!("offset not advanced: {immediate_output:?}"));
    }
    if !immediate_output.is_empty() {
        try_stop(registry, context, &handle).await;
        return Err(format!("immediate read not empty: {immediate_output:?}"));
    }

    let written = match registry
        .run(
            "Terminal",
            context,
            json!({
                "mode": "write",
                "handle": handle,
                "input": [{ "text": "hello\n" }],
                "yield_time_ms": 2500
            }),
        )
        .await
    {
        Ok(result) => result,
        Err(error) => {
            try_stop(registry, context, &handle).await;
            return Err(format!("write: {error}"));
        }
    };
    let write_output = written
        .details
        .as_ref()
        .and_then(|details| details["output"].as_str())
        .unwrap_or_default();
    if !write_output.contains("reply:hello") && !written.content.contains("reply:hello") {
        try_stop(registry, context, &handle).await;
        return Err(format!(
            "missing reply:hello content={:?} details={write_output:?}",
            written.content
        ));
    }
    if written
        .details
        .as_ref()
        .and_then(|d| d["written"].as_bool())
        != Some(true)
    {
        try_stop(registry, context, &handle).await;
        return Err("written flag missing".to_owned());
    }

    let after_write = match registry
        .run(
            "Terminal",
            context,
            json!({
                "mode": "read",
                "handle": handle,
                "yield_time_ms": 0
            }),
        )
        .await
    {
        Ok(result) => result,
        Err(error) => {
            try_stop(registry, context, &handle).await;
            return Err(format!("read after write: {error}"));
        }
    };
    let after_write_output = after_write
        .details
        .as_ref()
        .and_then(|details| details["output"].as_str())
        .unwrap_or_default();
    if after_write_output.contains("reply:hello") {
        try_stop(registry, context, &handle).await;
        return Err(format!(
            "write did not advance offset: {after_write_output:?}"
        ));
    }
    if !after_write_output.is_empty() {
        try_stop(registry, context, &handle).await;
        return Err(format!(
            "immediate read after write not empty: {after_write_output:?}"
        ));
    }

    registry
        .run(
            "Terminal",
            context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .map_err(|e| format!("stop: {e}"))?;
    Ok(())
}

#[cfg(unix)]
pub(crate) async fn wait_for_pid(path: &std::path::Path) -> u32 {
    for _ in 0..100 {
        if let Ok(pid) = std::fs::read_to_string(path)
            && let Ok(pid) = pid.trim().parse()
        {
            return pid;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("missing descendant PID at {}", path.display());
}

#[cfg(unix)]
pub(crate) async fn wait_for_process_exit(pid: u32) -> bool {
    for _ in 0..100 {
        if !std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

#[tokio::test]
async fn terminal_read_details_do_not_leak_output_past_max_output_bytes() {
    let _guard = serial_guard().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let context = guarded_context(&workspace, ShellLimits::default());
    let registry = ToolRegistry::with_builtin_tools();
    // Keep the secret in a file so the command itself cannot leak the needle.
    std::fs::write(
        workspace.path().join("payload.txt"),
        "keep-terminal-leak-tail",
    )
    .expect("payload file");
    run_max_output_cap_attempt(&registry, &context, &workspace)
        .await
        .expect("max_output_bytes cap");
}

#[tokio::test]
async fn terminal_read_reports_natural_guard_exit_status() {
    let _guard = serial_guard().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let context = guarded_context(&workspace, ShellLimits::default());
    let registry = ToolRegistry::with_builtin_tools();
    let started = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "start", "command": "true" }),
        )
        .await
        .expect("terminal start");
    let handle = started.details.as_ref().unwrap()["handle"]
        .as_str()
        .unwrap();

    let mut status = String::new();
    for _ in 0..100 {
        let read = registry
            .run(
                "Terminal",
                &context,
                json!({ "mode": "read", "handle": handle }),
            )
            .await
            .expect("terminal read");
        status = read.details.as_ref().unwrap()["status"]
            .as_str()
            .unwrap()
            .to_owned();
        if status != "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert_eq!(status, "completed");
    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .expect("terminal cleanup");
}

#[tokio::test]
async fn terminal_start_write_and_read_share_incremental_bounded_output() {
    let _guard = serial_guard().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let context = guarded_context(&workspace, ShellLimits::default());
    let registry = ToolRegistry::with_builtin_tools();
    run_incremental_bounded_attempt(&registry, &context, &workspace)
        .await
        .expect("incremental bounded output");
}

#[tokio::test]
async fn terminal_capture_survives_ring_overflow() {
    let _guard = serial_guard().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let session = tempfile::tempdir().expect("session");
    let context = guarded_context(&workspace, ShellLimits::default())
        .with_agent_session_context(session.path(), "agent-test");
    let registry = ToolRegistry::with_builtin_tools();
    #[cfg(windows)]
    let command = windows_powershell_command(
        "Start-Sleep -Milliseconds 200; [Console]::Out.Write(('f' * 550000)); [Console]::Out.Write('TERMINAL_CAPTURE_TAIL_7f3a'); [Console]::Out.Flush(); Start-Sleep -Seconds 300",
    );
    #[cfg(not(windows))]
    let command =
        "sleep 0.2; yes terminal-ring-flood | head -c 524288; printf 'TERMINAL_CAPTURE_TAIL_7f3a'; sleep 300"
            .to_owned();
    let details = start_terminal_command(&registry, &context, command, 80, 24, 500)
        .await
        .expect("start flooding terminal");
    let handle = details["handle"].as_str().expect("handle").to_owned();
    let task_id = format!("terminal-{handle}");

    // The model-visible snapshot is bounded by the 64 KiB ring no matter how
    // much floods in.
    let visible = details["output"].as_str().unwrap_or_default();
    assert!(
        visible.len() <= 65_536,
        "model-visible terminal output must stay bounded, got {} bytes",
        visible.len()
    );

    // The complete capture holds the whole flood including the tail sentinel
    // beyond the ring capacity. The capture is appended before the ring and
    // fsync-backed, so the flood arrives here at the capture's own pace.
    let capture_path = session
        .path()
        .join("agents")
        .join("agent-test")
        .join("tasks")
        .join(format!("{task_id}.log"));
    let mut complete = None;
    for _ in 0..3_000 {
        if let Ok(text) = std::fs::read_to_string(&capture_path)
            && text.contains("TERMINAL_CAPTURE_TAIL_7f3a")
        {
            complete = Some(text);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let complete = complete.expect("capture must contain the tail sentinel");
    assert!(
        complete.len() >= 524_288 + "TERMINAL_CAPTURE_TAIL_7f3a".len(),
        "capture must hold the full flood, got {} bytes",
        complete.len()
    );

    // Once the capture saw the tail, the ring counter has seen the whole
    // flood too: the overflow is proven by the ring's unbounded total.
    let read = registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "read",
                "handle": handle,
                "yield_time_ms": 0
            }),
        )
        .await
        .expect("read after flood");
    assert!(
        read.details
            .as_ref()
            .and_then(|details| details["total_output_bytes"].as_u64())
            .unwrap_or(0)
            >= 524_288,
        "ring total must show the full flood: {:?}",
        read.details
    );

    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .expect("stop flooding terminal");
}
