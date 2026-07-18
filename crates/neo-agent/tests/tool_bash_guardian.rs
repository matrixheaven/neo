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
    }));
    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel.cancel();

    let result = task.await.expect("join shell").expect("cancel shell");
    assert_eq!(result.outcome, ShellCommandOutcome::Cancelled);
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
