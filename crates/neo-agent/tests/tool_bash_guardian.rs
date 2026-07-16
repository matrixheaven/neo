use std::{path::PathBuf, time::Duration};

use neo_agent_core::{
    ResourceLimitCause, ShellCommandOrigin, ShellCommandOutcome, ShellExecutionRequest,
    ShellLimits, ShellRuntime, ToolAccess, ToolContext, ToolError, execute_model_bash_for_runtime,
    execute_shell_command,
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

#[tokio::test]
async fn background_timeout_finishes_without_task_output_polling() {
    let workspace = tempfile::tempdir().expect("workspace");
    let limits = ShellLimits {
        background_timeout_secs: 1,
        ..ShellLimits::default()
    };
    let ctx = guarded_context(&workspace, limits);
    let result = execute_model_bash_for_runtime(
        &ctx,
        serde_json::json!({
            "command": "sleep 30",
            "run_in_background": true,
            "description": "deadline regression"
        }),
    )
    .await
    .expect("start background command");
    let task_id = result
        .details
        .as_ref()
        .and_then(|details| details["task_id"].as_str())
        .expect("task id")
        .to_owned();

    tokio::time::sleep(Duration::from_millis(1_800)).await;
    let stopped = ctx
        .background_tasks
        .stop(&task_id, "test cleanup", limits.max_output_bytes)
        .await
        .expect("stop timed-out task");

    assert_eq!(stopped.details.as_ref().unwrap()["status"], "timed_out");
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
        foreground_timeout: Duration::from_secs(5),
        background_timeout: Duration::from_secs(30),
        max_output_bytes: 1_024,
        cancel_token: CancellationToken::new(),
        stream_update: None,
        background_tasks: None,
        shell_runtime: ctx.shell_runtime.clone(),
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
        foreground_timeout: Duration::from_secs(30),
        background_timeout: Duration::from_secs(30),
        max_output_bytes: 1_024,
        cancel_token: task_cancel,
        stream_update: None,
        background_tasks: None,
        shell_runtime: ctx.shell_runtime.clone(),
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
        foreground_timeout: Duration::from_secs(10),
        background_timeout: Duration::from_secs(30),
        max_output_bytes: 1_024,
        cancel_token: CancellationToken::new(),
        stream_update: None,
        background_tasks: Some(task_manager),
        shell_runtime: runtime,
    }));

    let mut task_id = None;
    for _ in 0..100 {
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
async fn second_bash_is_rejected_by_shared_active_command_limit() {
    let workspace = tempfile::tempdir().expect("workspace");
    let limits = ShellLimits {
        max_active_commands: 1,
        ..ShellLimits::default()
    };
    let ctx = guarded_context(&workspace, limits);
    let started = execute_model_bash_for_runtime(
        &ctx,
        serde_json::json!({
            "command": "sleep 30",
            "run_in_background": true,
            "description": "hold admission permit"
        }),
    )
    .await
    .expect("start first bash");
    let task_id = started
        .details
        .as_ref()
        .and_then(|details| details["task_id"].as_str())
        .expect("task id")
        .to_owned();

    let error =
        execute_model_bash_for_runtime(&ctx, serde_json::json!({ "command": "printf second" }))
            .await
            .expect_err("second bash must not queue or spawn");
    ctx.background_tasks
        .stop(&task_id, "test cleanup", limits.max_output_bytes)
        .await
        .expect("stop first bash");

    assert!(matches!(
        error,
        ToolError::ResourceLimited {
            cause: ResourceLimitCause::ActiveCommands
        }
    ));
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
