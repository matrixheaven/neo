use std::{path::PathBuf, process::Stdio, time::Duration};

use neo_agent_core::{
    ShellLimits, ShellRuntime, ToolAccess, ToolContext, ToolError, ToolRegistry,
    execute_model_bash_for_runtime,
};
use serde_json::json;

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

#[tokio::test]
async fn terminal_start_accepts_no_execution_deadline() {
    let workspace = tempfile::tempdir().expect("workspace");
    let context = guarded_context(&workspace, ShellLimits::default());
    let registry = ToolRegistry::with_builtin_tools();
    let started = registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "start",
                "command": "bash --noprofile --norc",
                "cols": 40,
                "rows": 8
            }),
        )
        .await
        .expect("terminal start without timeout_secs");
    let handle = started
        .details
        .as_ref()
        .and_then(|details| details["handle"].as_str())
        .expect("terminal handle")
        .to_owned();
    assert_eq!(
        started.details.as_ref().unwrap()["status"],
        "running",
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
    let workspace = tempfile::tempdir().expect("workspace");
    let context = guarded_context(&workspace, ShellLimits::default());
    let registry = ToolRegistry::with_builtin_tools();
    let started = registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "start",
                "command": "bash --noprofile --norc",
                "cols": 40,
                "rows": 8
            }),
        )
        .await
        .expect("terminal start");
    let details = started.details.as_ref().expect("start details");
    let handle = details["handle"].as_str().expect("terminal handle");
    assert_ne!(details["guardian_pid"], details["command_pid"]);

    registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "write",
                "handle": handle,
                "input": "printf 'pty:%s\\n' \"$COLUMNS:$LINES\"\n"
            }),
        )
        .await
        .expect("terminal write");
    let output = read_until(&registry, &context, handle, "pty:40:8").await;
    assert!(output.contains("pty:40:8"), "terminal output: {output:?}");

    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "resize", "handle": handle, "cols": 72, "rows": 18 }),
        )
        .await
        .expect("terminal resize");
    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "write", "handle": handle, "input": "stty size\n" }),
        )
        .await
        .expect("write after resize");
    let output = read_until(&registry, &context, handle, "18 72").await;
    assert!(output.contains("18 72"), "resized output: {output:?}");

    let stopped = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .expect("terminal stop");
    assert_eq!(stopped.details.as_ref().unwrap()["status"], "cancelled");
    assert!(matches!(
        registry
            .run(
                "Terminal",
                &context,
                json!({ "mode": "read", "handle": handle }),
            )
            .await,
        Err(ToolError::InvalidInput { .. })
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn terminal_stop_terminates_descendant_processes() {
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
    let workspace = tempfile::tempdir().expect("workspace");
    let context = guarded_context(&workspace, ShellLimits::default());
    let registry = ToolRegistry::with_builtin_tools();
    let started = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "start", "command": "printf keep-terminal-leak-tail; sleep 1" }),
        )
        .await
        .expect("terminal start");
    let handle = started.details.as_ref().unwrap()["handle"]
        .as_str()
        .unwrap();
    let read = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "read", "handle": handle, "max_output_bytes": 4 }),
        )
        .await
        .expect("terminal read");
    let serialized = serde_json::to_string(&read).expect("serialize result");
    assert!(read.content.contains("truncated: true"));
    assert!(!serialized.contains("terminal-leak-tail"));
    let output = read.details.as_ref().unwrap()["output"].as_str().unwrap();
    assert!(output.len() <= 4);

    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle, "max_output_bytes": 4 }),
        )
        .await
        .expect("terminal stop");
}

#[tokio::test]
async fn terminal_read_reports_natural_guard_exit_status() {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocked_write_in_one_terminal_does_not_block_other_handles() {
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
                    "input": format!("x\n{}", "x".repeat(2 * 1024 * 1024))
                }),
            )
            .await
    });
    let started = tokio::time::timeout(
        Duration::from_secs(5),
        read_until(&registry, &context, &blocked_handle, "writer-started"),
    )
    .await
    .expect("blocked terminal must emit writer-started");
    assert!(started.contains("writer-started"));
    let healthy = tokio::time::timeout(
        Duration::from_millis(500),
        registry.run(
            "Terminal",
            &context,
            json!({ "mode": "read", "handle": healthy_handle }),
        ),
    )
    .await
    .expect("another guardian must remain responsive")
    .expect("healthy terminal read");
    assert!(healthy.content.contains("healthy-terminal"));

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
    for _ in 0..50 {
        let result = registry
            .run(
                "Terminal",
                context,
                json!({ "mode": "read", "handle": handle }),
            )
            .await
            .expect("terminal read");
        output.push_str(result.details.as_ref().unwrap()["output"].as_str().unwrap());
        if output.contains(needle) {
            return output;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    output
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
