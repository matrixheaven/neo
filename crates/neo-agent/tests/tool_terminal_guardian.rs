use std::{path::PathBuf, sync::LazyLock, time::Duration};

#[cfg(unix)]
use std::process::Stdio;

#[cfg(windows)]
use base64::Engine as _;
use neo_agent_core::{
    ShellLimits, ShellRuntime, ToolAccess, ToolContext, ToolError, ToolRegistry,
    execute_model_bash_for_runtime,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

/// Serializes all tests in this file so they do not compete for OS resources
/// (PTY allocations, process spawns) and trigger spurious guardian timeouts.
static GUARDIAN_SERIAL: LazyLock<tokio::sync::Semaphore> =
    LazyLock::new(|| tokio::sync::Semaphore::new(1));

async fn serial_guard() -> tokio::sync::SemaphorePermit<'static> {
    GUARDIAN_SERIAL
        .acquire()
        .await
        .expect("guardian serial semaphore")
}

fn guarded_context(workspace: &tempfile::TempDir, limits: ShellLimits) -> ToolContext {
    ToolContext::new(workspace.path())
        .expect("tool context")
        .with_access(ToolAccess::all())
        .with_shell_runtime(ShellRuntime::new(
            limits,
            PathBuf::from(env!("CARGO_BIN_EXE_neo")),
            workspace.path().join("runtime"),
        ))
}

/// Portable Unix hold for tests that only need a live PTY.
#[cfg(not(windows))]
fn interactive_shell_command() -> String {
    "sleep 300".to_owned()
}

/// Windows ConPTY + nested Git Bash `-c` is unreliable for interactive bash.
/// Use the same PowerShell hold pattern proven in `process_guard_windows`.
#[cfg(windows)]
fn interactive_shell_command() -> String {
    // Mini line protocol over stdin so tests can drive output without bash.
    windows_powershell_command(
        r#"
$ErrorActionPreference = 'Continue'
function Emit([string]$text) {
  [Console]::Out.WriteLine($text)
  [Console]::Out.Flush()
}
function Size() {
  try {
    return @([Console]::WindowWidth, [Console]::WindowHeight)
  } catch {
    Emit ('size-error:{0}' -f $_.Exception.Message)
    return @(-1, -1)
  }
}
if (Test-Path -LiteralPath 'marker') {
  [Console]::Out.Write('initial-output')
  [Console]::Out.Flush()
} else {
  Emit ('cwd-missing:{0}' -f (Get-Location).Path)
}
while ($true) {
  $line = [Console]::In.ReadLine()
  if ($null -eq $line) { break }
  $t = $line.Trim()
  if ($t -eq 'CMD:PTY') {
    $size = Size
    Emit ('pty:{0}:{1}' -f $size[0], $size[1])
  } elseif ($t.StartsWith('CMD:SIZE:')) {
    $parts = $t.Split(':')
    $wantWidth = [int]$parts[2]
    $wantHeight = [int]$parts[3]
    $deadline = [DateTime]::UtcNow.AddSeconds(3)
    do {
      $size = Size
      if ($size[0] -eq $wantWidth -and $size[1] -eq $wantHeight) { break }
      Start-Sleep -Milliseconds 25
    } while ([DateTime]::UtcNow -lt $deadline)
    Emit ('size:{0} {1}' -f $size[1], $size[0])
  } elseif ($t -eq 'CMD:ALIVE') {
    Emit 'control-alive'
  } elseif ($t.StartsWith('CMD:REPLY:')) {
    [Console]::Out.Write(('reply:{0}' -f $t.Substring(10)))
    [Console]::Out.Flush()
  }
}
"#,
    )
}

#[cfg(windows)]
fn windows_powershell_command(script: &str) -> String {
    let bytes = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!("powershell.exe -NoLogo -NoProfile -EncodedCommand {encoded}")
}

async fn start_terminal_command(
    registry: &ToolRegistry,
    context: &ToolContext,
    command: String,
    cols: u16,
    rows: u16,
    yield_time_ms: u64,
) -> Result<serde_json::Value, String> {
    let started = registry
        .run(
            "Terminal",
            context,
            json!({
                "mode": "start",
                "command": command,
                "cols": cols,
                "rows": rows,
                "yield_time_ms": yield_time_ms
            }),
        )
        .await
        .map_err(|e| format!("interactive terminal start: {e}"))?;
    let handle = started
        .details
        .as_ref()
        .and_then(|details| details["handle"].as_str())
        .ok_or_else(|| "missing handle".to_owned())?
        .to_owned();
    let status = match registry
        .run(
            "Terminal",
            context,
            json!({
                "mode": "read",
                "handle": handle,
                "yield_time_ms": 0,
                "max_output_bytes": 0
            }),
        )
        .await
    {
        Ok(status) => status,
        Err(error) => {
            try_stop(registry, context, &handle).await;
            return Err(format!("status probe: {error}"));
        }
    };
    let status_text = status
        .details
        .as_ref()
        .and_then(|details| details["status"].as_str())
        .unwrap_or("missing");
    if status_text != "running" {
        try_stop(registry, context, &handle).await;
        return Err(format!(
            "interactive terminal not running after start: {status_text} details={:?}",
            started.details
        ));
    }
    Ok(started.details.expect("start details"))
}

async fn start_interactive_terminal(
    registry: &ToolRegistry,
    context: &ToolContext,
    cols: u16,
    rows: u16,
    yield_time_ms: u64,
) -> Result<serde_json::Value, String> {
    start_terminal_command(
        registry,
        context,
        interactive_shell_command(),
        cols,
        rows,
        yield_time_ms,
    )
    .await
}

#[tokio::test]
async fn terminal_start_accepts_no_execution_deadline() {
    let _guard = serial_guard().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let context = guarded_context(&workspace, ShellLimits::default());
    let registry = ToolRegistry::with_builtin_tools();
    let details = start_interactive_terminal(&registry, &context, 40, 8, 500)
        .await
        .expect("interactive start");
    let handle = details["handle"].as_str().expect("handle").to_owned();
    assert_eq!(
        details["status"], "running",
        "start without deadline should remain running"
    );
    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .expect("stop no-deadline terminal");
}

#[tokio::test]
async fn terminal_tool_start_write_read_resize_and_stop_uses_real_pty() {
    let _guard = serial_guard().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let context = guarded_context(&workspace, ShellLimits::default());
    let registry = ToolRegistry::with_builtin_tools();

    run_one_attempt(&registry, &context)
        .await
        .expect("terminal lifecycle");
}

async fn run_one_attempt(registry: &ToolRegistry, context: &ToolContext) -> Result<(), String> {
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

async fn try_stop(registry: &ToolRegistry, context: &ToolContext, handle: &str) {
    let _ = registry
        .run(
            "Terminal",
            context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await;
}

#[cfg(unix)]
#[tokio::test]
async fn terminal_stop_terminates_descendant_processes() {
    let _guard = serial_guard().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let context = guarded_context(&workspace, ShellLimits::default());
    let registry = ToolRegistry::with_builtin_tools();
    let pid_file = workspace.path().join("child.pid");
    let started = registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "start",
                "command": format!("sleep 30 & echo $! > '{}'; wait", pid_file.display())
            }),
        )
        .await
        .expect("terminal start");
    let handle = started.details.as_ref().unwrap()["handle"]
        .as_str()
        .unwrap()
        .to_owned();
    let pid = wait_for_pid(&pid_file).await;

    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .expect("terminal stop");
    assert!(wait_for_process_exit(pid).await);
}

#[cfg(unix)]
#[tokio::test]
async fn terminal_guardian_loss_triggers_identity_checked_emergency_cleanup() {
    let _guard = serial_guard().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let context = guarded_context(&workspace, ShellLimits::default());
    let registry = ToolRegistry::with_builtin_tools();
    let pid_file = workspace.path().join("guardian-loss-child.pid");
    let started = registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "start",
                "command": format!("sleep 30 & echo $! > '{}'; wait", pid_file.display())
            }),
        )
        .await
        .expect("terminal start");
    let details = started.details.as_ref().expect("start details");
    let handle = details["handle"].as_str().unwrap().to_owned();
    let guardian_pid = details["guardian_pid"].as_u64().unwrap().to_string();
    let descendant = wait_for_pid(&pid_file).await;

    let status = std::process::Command::new("kill")
        .args(["-9", &guardian_pid])
        .status()
        .expect("kill guardian");
    assert!(status.success());
    assert!(wait_for_process_exit(descendant).await);

    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .expect("remove failed terminal handle");
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

async fn run_max_output_cap_attempt(
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
    let read = match read {
        Some(read) => read,
        None => {
            try_stop(registry, context, &handle).await;
            return Err("expected capped terminal read".to_owned());
        }
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
async fn bash_and_terminal_share_the_active_command_limit() {
    let _guard = serial_guard().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let limits = ShellLimits {
        max_active_commands: 1,
        ..ShellLimits::default()
    };
    let context = guarded_context(&workspace, limits);
    let registry = ToolRegistry::with_builtin_tools();
    let started = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "start", "command": "bash --noprofile --norc" }),
        )
        .await
        .expect("terminal start");
    let handle = started.details.as_ref().unwrap()["handle"]
        .as_str()
        .unwrap()
        .to_owned();

    let queued = tokio::spawn({
        let context = context.clone();
        async move {
            execute_model_bash_for_runtime(&context, json!({ "command": "printf second" })).await
        }
    });
    for _ in 0..20 {
        assert!(
            !queued.is_finished(),
            "bash must wait for terminal capacity"
        );
        tokio::task::yield_now().await;
    }
    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .expect("terminal stop");
    let result = queued
        .await
        .expect("join queued bash")
        .expect("queued bash after terminal release");
    assert!(!result.is_error);
    assert!(result.content.contains("second"));
}

#[tokio::test]
async fn terminal_session_holds_background_permit_until_process_exit() {
    let _guard = serial_guard().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let limits = ShellLimits {
        max_active_commands: 1,
        ..ShellLimits::default()
    };
    let context = guarded_context(&workspace, limits);
    let runtime_root = context.shell_runtime.runtime_root().to_path_buf();
    let registry = ToolRegistry::with_builtin_tools();
    // Finite process: Start returns immediately, but the session keeps its
    // background permit until the process exits (not until Start returns).
    let command = if cfg!(windows) {
        "ping -n 4 127.0.0.1 >nul".to_owned()
    } else {
        "sleep 3".to_owned()
    };

    let started = registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "start",
                "command": command,
                "cols": 40,
                "rows": 8
            }),
        )
        .await
        .expect("terminal start");
    assert_eq!(
        started.details.as_ref().unwrap()["status"],
        "running",
        "terminal start returns while the process still owns the permit"
    );

    for _ in 0..500 {
        if count_running_markers(&runtime_root) >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        count_running_markers(&runtime_root),
        1,
        "terminal process must occupy the only capacity slot after Start returns"
    );

    let queued = tokio::spawn({
        let context = context.clone();
        async move {
            execute_model_bash_for_runtime(&context, json!({ "command": "printf after-exit" }))
                .await
        }
    });

    // While the Terminal process is still running, Bash must wait even though
    // Terminal Start already returned its handle. Sample for ~1s of sustained hold.
    for _ in 0..20 {
        assert_eq!(
            count_running_markers(&runtime_root),
            1,
            "terminal must keep its running marker (and permit) after Start returns"
        );
        assert!(
            !queued.is_finished(),
            "bash must wait until the terminal process exits and drops its permit"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let result = tokio::time::timeout(Duration::from_secs(10), queued)
        .await
        .expect("bash should start after terminal process exit")
        .expect("join queued bash")
        .expect("queued bash after natural terminal exit");
    assert!(!result.is_error);
    assert!(
        result.content.contains("after-exit"),
        "bash should run only after terminal process release: {}",
        result.content
    );
}

fn count_running_markers(runtime_root: &std::path::Path) -> usize {
    let mut count = 0;
    let Ok(entries) = std::fs::read_dir(runtime_root) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            count += count_running_markers(&path);
            continue;
        }
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".running.json"))
        {
            count += 1;
        }
    }
    count
}

/// Live guardians write `.running.json` and only write `.status.json` after
/// finalize. The running file itself is retained, so "marker disappeared"
/// means no running marker without a final status companion remains.
fn count_active_running_markers(runtime_root: &std::path::Path) -> usize {
    let mut count = 0;
    let Ok(entries) = std::fs::read_dir(runtime_root) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            count += count_active_running_markers(&path);
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(task_id) = name.strip_suffix(".running.json") else {
            continue;
        };
        if !path
            .with_file_name(format!("{task_id}.status.json"))
            .is_file()
        {
            count += 1;
        }
    }
    count
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocked_write_in_one_terminal_does_not_block_other_handles() {
    let _guard = serial_guard().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let context = guarded_context(&workspace, ShellLimits::default());
    let registry = std::sync::Arc::new(ToolRegistry::with_builtin_tools());
    let blocked = registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "start",
                "command": "python3 -c 'import sys,time; print(\"writer-started\",flush=True); sys.stdin.read(1); time.sleep(30)'"
            }),
        )
        .await
        .expect("blocked terminal start");
    let blocked_handle = blocked.details.as_ref().unwrap()["handle"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut started = blocked.details.as_ref().unwrap()["output"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if !started.contains("writer-started") {
        started = read_until(&registry, &context, &blocked_handle, "writer-started").await;
    }
    assert!(started.contains("writer-started"));
    let healthy = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "start", "command": "printf healthy-terminal; sleep 30" }),
        )
        .await
        .expect("healthy terminal start");
    let healthy_handle = healthy.details.as_ref().unwrap()["handle"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut healthy_output = healthy.details.as_ref().unwrap()["output"]
        .as_str()
        .unwrap_or_default()
        .to_owned();

    let write_registry = std::sync::Arc::clone(&registry);
    let write_context = context.clone();
    let write_handle = blocked_handle.clone();
    let write = tokio::spawn(async move {
        write_registry
            .run(
                "Terminal",
                &write_context,
                json!({
                    "mode": "write",
                    "handle": write_handle,
                    "input": [{ "text": format!("x\n{}", "x".repeat(2 * 1024 * 1024)) }]
                }),
            )
            .await
    });
    let healthy = tokio::time::timeout(
        Duration::from_millis(500),
        registry.run(
            "Terminal",
            &context,
            json!({ "mode": "read", "handle": healthy_handle, "yield_time_ms": 0 }),
        ),
    )
    .await
    .expect("another guardian must remain responsive")
    .expect("healthy terminal read");
    healthy_output.push_str(
        healthy.details.as_ref().unwrap()["output"]
            .as_str()
            .unwrap_or_default(),
    );
    assert!(healthy_output.contains("healthy-terminal"));

    tokio::time::timeout(
        Duration::from_secs(3),
        registry.run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": blocked_handle }),
        ),
    )
    .await
    .expect("blocked terminal stop must settle")
    .expect("blocked terminal stop");
    tokio::time::timeout(
        Duration::from_secs(3),
        registry.run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": healthy_handle }),
        ),
    )
    .await
    .expect("healthy terminal stop must settle")
    .expect("healthy terminal stop");
    let _ = tokio::time::timeout(Duration::from_secs(2), write)
        .await
        .expect("blocked write must settle after stop")
        .expect("write task join");
}

async fn read_until(
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

async fn run_incremental_bounded_attempt(
    registry: &ToolRegistry,
    context: &ToolContext,
    workspace: &tempfile::TempDir,
) -> Result<(), String> {
    let subdir = workspace.path().join(format!(
        "subdir-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
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

#[cfg(not(windows))]
#[tokio::test]
async fn terminal_ctrl_c_interrupts_command_and_keeps_session_usable() {
    let _guard = serial_guard().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let context = guarded_context(&workspace, ShellLimits::default());
    let registry = ToolRegistry::with_builtin_tools();
    run_session_usability_attempt(&registry, &context)
        .await
        .expect("Ctrl+C session usability");
}

#[cfg(not(windows))]
#[tokio::test]
async fn terminal_write_sends_ordered_text_and_control_in_one_call() {
    let _guard = serial_guard().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let context = guarded_context(&workspace, ShellLimits::default());
    let registry = ToolRegistry::with_builtin_tools();
    let details = start_terminal_command(
        &registry,
        &context,
        "IFS= read -r value; printf 'received:%s' \"$value\"".to_owned(),
        80,
        24,
        100,
    )
    .await
    .expect("start ordered input terminal");
    let handle = details["handle"]
        .as_str()
        .expect("terminal handle")
        .to_owned();

    let written = registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "write",
                "handle": handle,
                "input": [
                    { "text": "ordered-payload" },
                    // The first Ctrl+D submits canonical buffered text; the
                    // second, on an empty buffer, ends the shell `read`.
                    { "control": 4 },
                    { "control": 4 }
                ],
                "yield_time_ms": 2500
            }),
        )
        .await
        .expect("write ordered text and Ctrl+D");
    let mut output = written
        .details
        .as_ref()
        .and_then(|details| details["output"].as_str())
        .unwrap_or_default()
        .to_owned();
    if !output.contains("received:ordered-payload") {
        output
            .push_str(&read_until(&registry, &context, &handle, "received:ordered-payload").await);
    }
    assert!(
        output.contains("received:ordered-payload"),
        "ordered text/control input was not delivered in one write: {output:?}"
    );
    try_stop(&registry, &context, &handle).await;
}

#[cfg(windows)]
#[tokio::test]
async fn terminal_windows_session_remains_usable_without_signal_guarantee() {
    let _guard = serial_guard().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let context = guarded_context(&workspace, ShellLimits::default());
    let registry = ToolRegistry::with_builtin_tools();
    run_session_usability_attempt(&registry, &context)
        .await
        .expect("Windows session usability");
}

async fn run_session_usability_attempt(
    registry: &ToolRegistry,
    context: &ToolContext,
) -> Result<(), String> {
    #[cfg(windows)]
    let details = start_interactive_terminal(registry, context, 80, 24, 800).await?;
    #[cfg(not(windows))]
    let details = start_terminal_command(
        registry,
        context,
        "trap 'printf control-alive\\n' INT; printf control-ready\\n; while :; do sleep 30; done"
            .to_owned(),
        80,
        24,
        100,
    )
    .await?;
    let handle = details["handle"]
        .as_str()
        .ok_or_else(|| "missing handle".to_owned())?
        .to_owned();

    #[cfg(not(windows))]
    {
        let mut ready = details["output"].as_str().unwrap_or_default().to_owned();
        if !ready.contains("control-ready") {
            ready = read_until(registry, context, &handle, "control-ready").await;
        }
        if !ready.contains("control-ready") {
            try_stop(registry, context, &handle).await;
            return Err(format!("terminal control handler not ready: {ready:?}"));
        }
    }

    #[cfg(windows)]
    let alive_input = json!([{ "text": "CMD:ALIVE" }, { "control": 13 }]);
    #[cfg(not(windows))]
    let alive_input = json!([{ "control": 3 }]);
    let alive = match registry
        .run(
            "Terminal",
            context,
            json!({
                "mode": "write",
                "handle": handle,
                "input": alive_input,
                "yield_time_ms": 2500
            }),
        )
        .await
    {
        Ok(result) => result,
        Err(e) => {
            try_stop(registry, context, &handle).await;
            return Err(format!("write session probe: {e}"));
        }
    };
    let mut combined = alive
        .details
        .as_ref()
        .and_then(|details| details["output"].as_str())
        .unwrap_or_default()
        .to_owned();
    if !combined.contains("control-alive") {
        combined = read_until(registry, context, &handle, "control-alive").await;
    }
    if !combined.contains("control-alive") {
        try_stop(registry, context, &handle).await;
        return Err(format!("session unusable: {combined:?}"));
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_start_cancellation_after_registration_cleans_up_process() {
    let _guard = serial_guard().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let cancel = CancellationToken::new();
    let context =
        guarded_context(&workspace, ShellLimits::default()).with_cancel_token(cancel.clone());
    let registry = std::sync::Arc::new(ToolRegistry::with_builtin_tools());
    let runtime_root = context.shell_runtime.runtime_root().to_path_buf();

    let start = {
        let registry = std::sync::Arc::clone(&registry);
        let context = context.clone();
        tokio::spawn(async move {
            registry
                .run(
                    "Terminal",
                    &context,
                    json!({
                        "mode": "start",
                        "command": "sleep 30",
                        "yield_time_ms": 30000
                    }),
                )
                .await
        })
    };

    let mut saw_marker = false;
    for _ in 0..200 {
        if count_active_running_markers(&runtime_root) >= 1 {
            saw_marker = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        saw_marker,
        "start must register a running process before cancellation"
    );

    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(15), start)
        .await
        .expect("start task should finish after cancel")
        .expect("start task join");
    assert!(
        matches!(result, Err(ToolError::Cancelled)),
        "cancelled start must return Cancelled: {result:?}"
    );

    let cleaned = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if count_active_running_markers(&runtime_root) == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(
        cleaned.is_ok(),
        "cancellation after registration must stop the process; active markers left: {}",
        count_active_running_markers(&runtime_root)
    );
}

#[cfg(unix)]
async fn wait_for_pid(path: &std::path::Path) -> u32 {
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
async fn wait_for_process_exit(pid: u32) -> bool {
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
