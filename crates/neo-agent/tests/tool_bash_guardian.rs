use std::{path::PathBuf, time::Duration};

use neo_agent_core::{
    ShellAdmissionClass, ShellAdmissionRequest, ShellCommandOrigin, ShellCommandOutcome,
    ShellExecutionRequest, ShellLimits, ShellRuntime, ToolAccess, ToolContext,
    execute_model_bash_for_runtime, execute_shell_command,
};
use tokio_util::sync::CancellationToken;

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

fn user_admission() -> ShellAdmissionRequest {
    ShellAdmissionRequest {
        owner: "user".to_owned(),
        class: ShellAdmissionClass::User,
    }
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

#[tokio::test]
async fn explicit_timeout_starts_after_guardian_start_and_kills_tree() {
    let workspace = tempfile::tempdir().expect("workspace");
    let ctx = guarded_context(&workspace, ShellLimits::default());
    let result = execute_shell_command(ShellExecutionRequest {
        id: "shell-timeout".to_owned(),
        command: "sleep 30".to_owned(),
        cwd: ctx.cwd.clone(),
        origin: ShellCommandOrigin::UserShellMode,
        timeout: Some(Duration::from_secs(1)),
        max_output_bytes: 1_024,
        cancel_token: CancellationToken::new(),
        stream_update: None,
        background_tasks: None,
        shell_runtime: ctx.shell_runtime.clone(),
        admission: user_admission(),
        admission_callback: None,
        tool_output_capture: None,
    })
    .await
    .expect("run timed shell");

    assert_eq!(result.outcome, ShellCommandOutcome::TimedOut);
}

#[tokio::test]
async fn bash_foreground_collects_output_through_guardian() {
    let workspace = tempfile::tempdir().expect("workspace");
    let ctx = guarded_context(&workspace, ShellLimits::default());
    let result = execute_shell_command(ShellExecutionRequest {
        id: "shell-output".to_owned(),
        command: "printf ok".to_owned(),
        cwd: ctx.cwd.clone(),
        origin: ShellCommandOrigin::UserShellMode,
        timeout: Some(Duration::from_secs(5)),
        max_output_bytes: 1_024,
        cancel_token: CancellationToken::new(),
        stream_update: None,
        background_tasks: None,
        shell_runtime: ctx.shell_runtime.clone(),
        admission: user_admission(),
        admission_callback: None,
        tool_output_capture: None,
    })
    .await
    .expect("run guarded shell");

    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.stdout, "ok");
    assert_eq!(result.outcome, ShellCommandOutcome::Completed);
}

#[tokio::test]
async fn bash_foreground_cancellation_kills_descendant_process_group() {
    let workspace = tempfile::tempdir().expect("workspace");
    let ctx = guarded_context(&workspace, ShellLimits::default());
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let task = tokio::spawn(execute_shell_command(ShellExecutionRequest {
        id: "shell-cancel".to_owned(),
        command: "sleep 30 & wait".to_owned(),
        cwd: ctx.cwd.clone(),
        origin: ShellCommandOrigin::UserShellMode,
        timeout: None,
        max_output_bytes: 1_024,
        cancel_token: task_cancel,
        stream_update: None,
        background_tasks: None,
        shell_runtime: ctx.shell_runtime.clone(),
        admission: user_admission(),
        admission_callback: None,
        tool_output_capture: None,
    }));
    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel.cancel();

    let result = task.await.expect("join shell").expect("cancel shell");
    assert_eq!(result.outcome, ShellCommandOutcome::Cancelled);
}

#[cfg(unix)]
#[tokio::test]
async fn bash_foreground_cancellation_allows_term_cleanup_before_force_kill() {
    let workspace = tempfile::tempdir().expect("workspace");
    let ctx = guarded_context(&workspace, ShellLimits::default());
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let task = tokio::spawn(execute_shell_command(ShellExecutionRequest {
        id: "shell-term-cleanup".to_owned(),
        command: "trap 'sleep 0.1; printf handled > term.marker; exit 0' TERM; printf ready > ready.marker; while :; do sleep 1; done".to_owned(),
        cwd: ctx.cwd.clone(),
        origin: ShellCommandOrigin::UserShellMode,
        timeout: None,
        max_output_bytes: 1_024,
        cancel_token: task_cancel,
        stream_update: None,
        background_tasks: None,
        shell_runtime: ctx.shell_runtime.clone(),
        admission: user_admission(),
        admission_callback: None,
        tool_output_capture: None,
    }));
    for _ in 0..500 {
        if workspace.path().join("ready.marker").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(workspace.path().join("ready.marker").exists());
    cancel.cancel();

    let result = task.await.expect("join shell").expect("cancel shell");
    assert_eq!(result.outcome, ShellCommandOutcome::Cancelled);
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("term.marker")).expect("TERM cleanup marker"),
        "handled"
    );
}

#[tokio::test]
async fn user_shell_runner_registers_foreground_task_for_detach() {
    let workspace = tempfile::tempdir().expect("workspace");
    let ctx = guarded_context(&workspace, ShellLimits::default());
    let manager = ctx.background_tasks.clone();
    let task_manager = manager.clone();
    let runtime = ctx.shell_runtime.clone();
    let cwd = ctx.cwd.clone();
    let task = tokio::spawn(execute_shell_command(ShellExecutionRequest {
        id: "shell-detach".to_owned(),
        command: "sleep 30".to_owned(),
        cwd,
        origin: ShellCommandOrigin::UserShellMode,
        timeout: None,
        max_output_bytes: 1_024,
        cancel_token: CancellationToken::new(),
        stream_update: None,
        background_tasks: Some(task_manager),
        shell_runtime: runtime,
        admission: user_admission(),
        admission_callback: None,
        tool_output_capture: None,
    }));

    let mut task_id = None;
    for _ in 0..500 {
        assert!(
            !task.is_finished(),
            "foreground shell exited before registering"
        );
        task_id = manager
            .list(true, 10)
            .await
            .into_iter()
            .next()
            .map(|task| task.task_id);
        if task_id.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let task_id = task_id.expect("registered foreground shell");
    manager.detach(&task_id).await.expect("detach shell");
    let result = task.await.expect("join shell").expect("detached shell");
    assert!(matches!(
        result.outcome,
        ShellCommandOutcome::Backgrounded { .. }
    ));
    manager
        .stop(&task_id, "test cleanup", 1_024)
        .await
        .expect("stop detached shell");
}

#[tokio::test]
async fn queued_bash_does_not_spawn_guardian_before_permit() {
    let workspace = tempfile::tempdir().expect("workspace");
    let limits = ShellLimits {
        max_active_commands: 1,
        ..ShellLimits::default()
    };
    let runtime = ShellRuntime::new(
        limits,
        PathBuf::from(env!("CARGO_BIN_EXE_neo")),
        workspace.path().join("runtime"),
    );
    let runtime_root = runtime.runtime_root().to_path_buf();
    let hold_cancel = CancellationToken::new();
    let hold_task = tokio::spawn(execute_shell_command(ShellExecutionRequest {
        id: "shell-hold".to_owned(),
        command: "sleep 30".to_owned(),
        cwd: workspace.path().to_path_buf(),
        origin: ShellCommandOrigin::UserShellMode,
        timeout: None,
        max_output_bytes: 1_024,
        cancel_token: hold_cancel.clone(),
        stream_update: None,
        background_tasks: None,
        shell_runtime: runtime.clone(),
        admission: user_admission(),
        admission_callback: None,
        tool_output_capture: None,
    }));

    let mut holders = 0;
    for _ in 0..500 {
        assert!(
            !hold_task.is_finished(),
            "hold command exited before occupying capacity"
        );
        holders = count_running_markers(&runtime_root);
        if holders >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        holders, 1,
        "first command must occupy the only capacity slot"
    );

    let second = tokio::spawn(execute_shell_command(ShellExecutionRequest {
        id: "shell-queued".to_owned(),
        command: "printf second".to_owned(),
        cwd: workspace.path().to_path_buf(),
        origin: ShellCommandOrigin::UserShellMode,
        timeout: Some(Duration::from_secs(5)),
        max_output_bytes: 1_024,
        cancel_token: CancellationToken::new(),
        stream_update: None,
        background_tasks: None,
        shell_runtime: runtime.clone(),
        admission: ShellAdmissionRequest {
            owner: "user-2".to_owned(),
            class: ShellAdmissionClass::User,
        },
        admission_callback: None,
        tool_output_capture: None,
    }));

    // Give the second request time to reach the scheduler queue, then prove it
    // does not spawn another guardian while the first still holds capacity.
    for _ in 0..50 {
        assert!(
            !hold_task.is_finished(),
            "hold command must remain running while second is queued"
        );
        assert_eq!(
            count_running_markers(&runtime_root),
            1,
            "queued command must not spawn a second guardian"
        );
        assert!(!second.is_finished());
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    hold_cancel.cancel();
    let hold_result = hold_task.await.expect("join hold").expect("cancel hold");
    assert_eq!(hold_result.outcome, ShellCommandOutcome::Cancelled);

    let second_result = second.await.expect("join queued").expect("run queued");
    assert_eq!(second_result.exit_code, Some(0));
    assert_eq!(second_result.stdout, "second");
    assert_eq!(second_result.outcome, ShellCommandOutcome::Completed);
}

#[tokio::test]
async fn background_output_is_persisted_by_guardian_in_agent_task_log() {
    let workspace = tempfile::tempdir().expect("workspace");
    let session = tempfile::tempdir().expect("session");
    let ctx = guarded_context(&workspace, ShellLimits::default())
        .with_agent_session_context(session.path(), "agent-test");
    let started = execute_model_bash_for_runtime(
        &ctx,
        serde_json::json!({
            "command": "printf persisted-output",
            "run_in_background": true,
            "description": "persist output"
        }),
    )
    .await
    .expect("start background bash");
    let task_id = started
        .details
        .as_ref()
        .and_then(|details| details["task_id"].as_str())
        .expect("task id")
        .to_owned();

    for _ in 0..100 {
        if ctx
            .background_tasks
            .snapshot(&task_id)
            .await
            .is_ok_and(|snapshot| !snapshot.status.is_active())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let log = session
        .path()
        .join("agents")
        .join("agent-test")
        .join("tasks")
        .join(format!("{task_id}.log"));
    assert_eq!(
        std::fs::read_to_string(log).expect("read guardian task log"),
        "persisted-output"
    );
}

#[tokio::test]
async fn background_bash_clamps_unsupported_timeout() {
    let workspace = tempfile::tempdir().expect("workspace");
    let ctx = guarded_context(&workspace, ShellLimits::default());
    let started = execute_model_bash_for_runtime(
        &ctx,
        serde_json::json!({
            "command": "printf completed",
            "run_in_background": true,
            "description": "clamped background timeout",
            "timeout_secs": 299
        }),
    )
    .await
    .expect("start background bash with clamped timeout");
    assert!(started.content.contains("clamped to 300 seconds"));
    let task_id = started
        .details
        .as_ref()
        .and_then(|details| details["task_id"].as_str())
        .expect("task id")
        .to_owned();

    let result = ctx
        .background_tasks
        .output(&task_id, true, Duration::from_secs(5), 1_024)
        .await
        .expect("background output");
    assert_eq!(
        result.details.as_ref().expect("details")["status"],
        "completed"
    );
    assert!(result.content.contains("completed"));
}

#[cfg(unix)]
#[tokio::test]
async fn queued_model_bash_revalidates_cwd_after_admission() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().expect("workspace");
    let outside = tempfile::tempdir().expect("outside");
    let queued_cwd = workspace.path().join("queued-cwd");
    std::fs::create_dir(&queued_cwd).expect("queued cwd");
    let limits = ShellLimits {
        max_active_commands: 1,
        ..ShellLimits::default()
    };
    let ctx = guarded_context(&workspace, limits);
    let hold_cancel = CancellationToken::new();
    let hold = tokio::spawn(execute_shell_command(ShellExecutionRequest {
        id: "cwd-hold".to_owned(),
        command: "sleep 30".to_owned(),
        cwd: workspace.path().to_path_buf(),
        origin: ShellCommandOrigin::UserShellMode,
        timeout: None,
        max_output_bytes: 1_024,
        cancel_token: hold_cancel.clone(),
        stream_update: None,
        background_tasks: None,
        shell_runtime: ctx.shell_runtime.clone(),
        admission: user_admission(),
        admission_callback: None,
        tool_output_capture: None,
    }));
    for _ in 0..500 {
        if count_running_markers(ctx.shell_runtime.runtime_root()) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(count_running_markers(ctx.shell_runtime.runtime_root()), 1);

    let queued_ctx = ctx.clone();
    let queued = tokio::spawn(async move {
        execute_model_bash_for_runtime(
            &queued_ctx,
            serde_json::json!({
                "command": "printf escaped > escaped.marker",
                "cwd": "queued-cwd"
            }),
        )
        .await
    });
    for _ in 0..50 {
        assert!(!queued.is_finished(), "model Bash should still be queued");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    std::fs::remove_dir(&queued_cwd).expect("remove queued cwd");
    symlink(outside.path(), &queued_cwd).expect("retarget queued cwd");
    hold_cancel.cancel();
    let _ = hold.await.expect("join hold").expect("cancel hold");

    let error = queued
        .await
        .expect("join queued Bash")
        .expect_err("post-admission cwd drift must be rejected");
    assert!(
        error.to_string().contains("outside workspace"),
        "unexpected error: {error}"
    );
    assert!(!outside.path().join("escaped.marker").exists());
}

#[tokio::test]
async fn explicit_timeout_excludes_time_spent_in_admission_queue() {
    let workspace = tempfile::tempdir().expect("workspace");
    let limits = ShellLimits {
        max_active_commands: 1,
        ..ShellLimits::default()
    };
    let runtime = ShellRuntime::new(
        limits,
        PathBuf::from(env!("CARGO_BIN_EXE_neo")),
        workspace.path().join("runtime"),
    );
    let runtime_root = runtime.runtime_root().to_path_buf();
    let started_marker = workspace.path().join("timeout-started.marker");

    let hold_cancel = CancellationToken::new();
    let hold_task = tokio::spawn(execute_shell_command(ShellExecutionRequest {
        id: "shell-hold-timeout".to_owned(),
        command: "sleep 30".to_owned(),
        cwd: workspace.path().to_path_buf(),
        origin: ShellCommandOrigin::UserShellMode,
        timeout: None,
        max_output_bytes: 1_024,
        cancel_token: hold_cancel.clone(),
        stream_update: None,
        background_tasks: None,
        shell_runtime: runtime.clone(),
        admission: user_admission(),
        admission_callback: None,
        tool_output_capture: None,
    }));

    let mut holders = 0;
    for _ in 0..500 {
        assert!(
            !hold_task.is_finished(),
            "hold command exited before occupying capacity"
        );
        holders = count_running_markers(&runtime_root);
        if holders >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(holders, 1, "hold must occupy the only capacity slot");

    let command = if cfg!(windows) {
        format!(
            "echo started> \"{}\" & ping -n 31 127.0.0.1 >nul",
            started_marker.display()
        )
    } else {
        format!("printf started > '{}'; sleep 30", started_marker.display())
    };
    let queued = tokio::spawn(execute_shell_command(ShellExecutionRequest {
        id: "shell-timeout-queued".to_owned(),
        command,
        cwd: workspace.path().to_path_buf(),
        origin: ShellCommandOrigin::UserShellMode,
        timeout: Some(Duration::from_secs(1)),
        max_output_bytes: 1_024,
        cancel_token: CancellationToken::new(),
        stream_update: None,
        background_tasks: None,
        shell_runtime: runtime.clone(),
        admission: ShellAdmissionRequest {
            owner: "timeout-owner".to_owned(),
            class: ShellAdmissionClass::User,
        },
        admission_callback: None,
        tool_output_capture: None,
    }));

    // Queue longer than the explicit one-second deadline so a leak would
    // expire the command before it ever starts.
    for _ in 0..20 {
        assert!(!queued.is_finished(), "queued command finished while held");
        assert_eq!(
            count_running_markers(&runtime_root),
            1,
            "queued command must not spawn before grant"
        );
        assert!(
            !started_marker.exists(),
            "command body must not run while queued"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    hold_cancel.cancel();
    let hold_result = hold_task.await.expect("join hold").expect("cancel hold");
    assert_eq!(hold_result.outcome, ShellCommandOutcome::Cancelled);

    for _ in 0..500 {
        if started_marker.exists() {
            break;
        }
        assert!(
            !queued.is_finished(),
            "timeout command finished before start"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        started_marker.exists(),
        "command must begin after admission grant"
    );
    let granted_at = std::time::Instant::now();

    let result = queued
        .await
        .expect("join timeout command")
        .expect("run timeout command");
    assert_eq!(result.outcome, ShellCommandOutcome::TimedOut);
    let after_grant = granted_at.elapsed();
    assert!(
        after_grant >= Duration::from_millis(700),
        "explicit timeout must run for about one second after start, got {after_grant:?}"
    );
    assert!(
        after_grant <= Duration::from_secs(4),
        "timeout should not include multi-second queue wait, got {after_grant:?}"
    );
}

#[tokio::test]
async fn complete_agent_output_survives_preview_queue_pressure() {
    const FLOOD_BYTES: usize = 12_582_912; // 12 MiB: beyond the 64 KiB result cap AND the 10 MiB log cap
    const TAIL_MARKER: &str = "CAPTURE_TAIL_MARKER_7f3a";
    let workspace = tempfile::tempdir().expect("workspace");
    let session = tempfile::tempdir().expect("session");
    let ctx = guarded_context(&workspace, ShellLimits::default())
        .with_agent_session_context(session.path(), "agent-test");
    let result = execute_model_bash_for_runtime(
        &ctx,
        serde_json::json!({
            "command": format!("yes preview-flood | head -c {FLOOD_BYTES}; printf '{TAIL_MARKER}'"),
        }),
    )
    .await
    .expect("flooding agent bash should complete");

    // Model-visible output stays bounded at the 64 KiB result cap while the
    // preview queue drops and the head-only result buffer omits.
    assert!(
        result.content.contains("[output truncated]"),
        "flood must be truncated in the model-visible result"
    );
    assert!(
        !result.content.contains(TAIL_MARKER),
        "model-visible result must not contain the tail beyond the cap"
    );

    let tasks = session
        .path()
        .join("agents")
        .join("agent-test")
        .join("tasks");
    let captures = std::fs::read_dir(&tasks)
        .expect("agent tasks dir")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "log"))
        .collect::<Vec<_>>();
    assert_eq!(captures.len(), 1, "one capture file expected");
    let log = std::fs::read_to_string(&captures[0]).expect("read capture log");
    assert!(
        log.contains(TAIL_MARKER),
        "complete capture must contain the tail sentinel beyond old caps"
    );
    assert!(
        log.len() >= FLOOD_BYTES + TAIL_MARKER.len(),
        "capture must hold the full flood, got {} bytes",
        log.len()
    );
}

#[tokio::test]
async fn agent_bash_capture_open_failure_prevents_launch() {
    let workspace = tempfile::tempdir().expect("workspace");
    let session = tempfile::tempdir().expect("session");
    // An agent id with a path separator is rejected by the output store, so
    // the guardian cannot open the capture and must refuse to launch.
    let ctx = guarded_context(&workspace, ShellLimits::default())
        .with_agent_session_context(session.path(), "agent/blocked");
    let error = execute_model_bash_for_runtime(
        &ctx,
        serde_json::json!({
            "command": "printf ran > ran.marker",
        }),
    )
    .await
    .expect_err("capture open failure must prevent launch");
    assert!(
        error.to_string().contains("guard"),
        "unexpected error: {error}"
    );
    assert!(
        !workspace.path().join("ran.marker").exists(),
        "command must not run when the capture cannot be opened"
    );
    let tasks = session
        .path()
        .join("agents")
        .join("agent")
        .join("blocked")
        .join("tasks");
    let captures = std::fs::read_dir(&tasks)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "log"))
                .count()
        })
        .unwrap_or(0);
    assert_eq!(
        captures, 0,
        "no capture artifact may exist after open failure"
    );
}

#[tokio::test]
async fn agent_bash_capture_append_failure_stops_process_with_diagnostic() {
    let workspace = tempfile::tempdir().expect("workspace");
    let session = tempfile::tempdir().expect("session");
    let ctx = guarded_context(&workspace, ShellLimits::default())
        .with_agent_session_context(session.path(), "agent-test");
    let started = execute_model_bash_for_runtime(
        &ctx,
        serde_json::json!({
            "command": "sleep 1; printf 'flood\\n'; sleep 30",
            "run_in_background": true,
            "description": "capture failure"
        }),
    )
    .await
    .expect("start background bash");
    let task_id = started
        .details
        .as_ref()
        .and_then(|details| details["task_id"].as_str())
        .expect("task id")
        .to_owned();
    let tasks = session
        .path()
        .join("agents")
        .join("agent-test")
        .join("tasks");
    let log_path = tasks.join(format!("{task_id}.log"));
    for _ in 0..500 {
        if log_path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(log_path.exists(), "capture must open before launch");
    // Squat on the capture path so the next append fails after the command
    // already started (its side effects cannot be rolled back).
    std::fs::remove_file(&log_path).expect("remove capture log");
    std::fs::create_dir(&log_path).expect("block capture log");

    let started_at = std::time::Instant::now();
    let mut terminal = false;
    for _ in 0..500 {
        if let Ok(snapshot) = ctx.background_tasks.snapshot(&task_id).await
            && !snapshot.status.is_active()
        {
            terminal = true;
            break;
        }
        assert!(
            started_at.elapsed() < Duration::from_secs(10),
            "capture failure must stop the command instead of letting it run"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        terminal,
        "task must reach a terminal state after capture failure"
    );
    assert!(
        started_at.elapsed() < Duration::from_secs(10),
        "capture failure must stop the process promptly, took {:?}",
        started_at.elapsed()
    );

    let status: serde_json::Value = serde_json::from_slice(
        &std::fs::read(tasks.join(format!("{task_id}.status.json"))).expect("read final status"),
    )
    .expect("parse final status");
    assert_eq!(status["exit"]["status"], "failed");
    let capture_error = status["exit"]["capture_error"]
        .as_str()
        .expect("capture_error in final status");
    assert!(
        capture_error.contains("not a regular file") || capture_error.contains("directory"),
        "unexpected capture error: {capture_error}"
    );
}

#[tokio::test]
async fn agent_bash_capture_preserves_output_emitted_before_cancellation() {
    let workspace = tempfile::tempdir().expect("workspace");
    let session = tempfile::tempdir().expect("session");
    let ctx = guarded_context(&workspace, ShellLimits::default())
        .with_agent_session_context(session.path(), "agent-test");
    let started = execute_model_bash_for_runtime(
        &ctx,
        serde_json::json!({
            "command": "printf 'BEFORE_CANCEL_MARKER_9f2a'; sleep 30",
            "run_in_background": true,
            "description": "cancel capture"
        }),
    )
    .await
    .expect("start background bash");
    let task_id = started
        .details
        .as_ref()
        .and_then(|details| details["task_id"].as_str())
        .expect("task id")
        .to_owned();
    let tasks = session
        .path()
        .join("agents")
        .join("agent-test")
        .join("tasks");
    let log_path = tasks.join(format!("{task_id}.log"));

    // Wait until the capture holds the output the command emitted before it
    // can be cancelled, then stop it mid-flight.
    let mut captured = false;
    for _ in 0..500 {
        if let Ok(text) = std::fs::read_to_string(&log_path)
            && text.contains("BEFORE_CANCEL_MARKER_9f2a")
        {
            captured = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        captured,
        "capture must contain output emitted before cancellation"
    );
    ctx.background_tasks
        .stop(&task_id, "test cancel", 1_024)
        .await
        .expect("stop background bash");

    let status: serde_json::Value = serde_json::from_slice(
        &std::fs::read(tasks.join(format!("{task_id}.status.json"))).expect("read final status"),
    )
    .expect("parse final status");
    assert_eq!(status["exit"]["status"], "cancelled");
    let log = std::fs::read_to_string(&log_path).expect("read capture log");
    assert!(
        log.contains("BEFORE_CANCEL_MARKER_9f2a"),
        "capture must keep output emitted before cancellation"
    );
}
