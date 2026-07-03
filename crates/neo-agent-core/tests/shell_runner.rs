use std::time::Duration;

use neo_agent_core::{
    BackgroundTaskStatus, ShellCommandOrigin, ShellCommandOutcome, ShellExecutionRequest,
    ToolAccess, ToolContext, execute_shell_command,
};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

fn test_context(temp: &TempDir) -> ToolContext {
    ToolContext::new(temp.path())
        .expect("context")
        .with_access(ToolAccess::all())
}

#[tokio::test]
async fn shell_runner_collects_stdout_and_exit_code() {
    let temp = TempDir::new().expect("tempdir");
    let ctx = test_context(&temp);
    let result = execute_shell_command(ShellExecutionRequest {
        id: "shell-1".to_owned(),
        command: "printf ok".to_owned(),
        cwd: ctx.cwd.clone(),
        origin: ShellCommandOrigin::UserShellMode,
        foreground_timeout: Duration::from_secs(5),
        background_timeout: Duration::from_secs(600),
        max_output_bytes: 1024,
        cancel_token: CancellationToken::new(),
        stream_update: None,
        background_tasks: None,
    })
    .await
    .expect("runner succeeds");
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.stdout, "ok");
    assert_eq!(result.outcome, ShellCommandOutcome::Completed);
}

#[tokio::test]
async fn shell_runner_caps_final_output_and_marks_truncated() {
    let temp = TempDir::new().expect("tempdir");
    let ctx = test_context(&temp);
    let result = execute_shell_command(ShellExecutionRequest {
        id: "shell-capped".to_owned(),
        command: "printf abcdef".to_owned(),
        cwd: ctx.cwd.clone(),
        origin: ShellCommandOrigin::UserShellMode,
        foreground_timeout: Duration::from_secs(5),
        background_timeout: Duration::from_secs(600),
        max_output_bytes: 3,
        cancel_token: CancellationToken::new(),
        stream_update: None,
        background_tasks: None,
    })
    .await
    .expect("runner succeeds");

    assert_eq!(result.stdout, "abc");
    assert!(result.stderr.is_empty());
    assert!(result.truncated);
}

#[tokio::test]
async fn shell_runner_cancel_kills_process_group() {
    let temp = TempDir::new().expect("tempdir");
    let ctx = test_context(&temp);
    let token = CancellationToken::new();
    let cloned = token.clone();
    let task = tokio::spawn(async move {
        execute_shell_command(ShellExecutionRequest {
            id: "shell-2".to_owned(),
            command: "sleep 30".to_owned(),
            cwd: ctx.cwd.clone(),
            origin: ShellCommandOrigin::UserShellMode,
            foreground_timeout: Duration::from_secs(60),
            background_timeout: Duration::from_secs(600),
            max_output_bytes: 1024,
            cancel_token: cloned,
            stream_update: None,
            background_tasks: None,
        })
        .await
    });
    token.cancel();
    let result = task.await.expect("join").expect("runner returns result");
    assert_eq!(result.outcome, ShellCommandOutcome::Cancelled);
}

#[tokio::test]
async fn user_shell_runner_registers_foreground_task_for_detach() {
    let temp = TempDir::new().expect("tempdir");
    let ctx = test_context(&temp);
    let token = CancellationToken::new();
    let manager = ctx.background_tasks.clone();
    let runner_manager = manager.clone();
    let task = tokio::spawn(async move {
        execute_shell_command(ShellExecutionRequest {
            id: "shell-3".to_owned(),
            command: "sleep 30".to_owned(),
            cwd: ctx.cwd.clone(),
            origin: ShellCommandOrigin::UserShellMode,
            foreground_timeout: Duration::from_secs(60),
            background_timeout: Duration::from_secs(600),
            max_output_bytes: 1024,
            cancel_token: token,
            stream_update: None,
            background_tasks: Some(runner_manager),
        })
        .await
    });

    tokio::time::sleep(Duration::from_millis(20)).await;
    let tasks = manager.list(true, 10).await;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, BackgroundTaskStatus::Running);
    let task_id = tasks[0].task_id.clone();
    manager.detach(&task_id).await.expect("detach");
    let result = task.await.expect("join").expect("runner returns result");
    assert!(matches!(
        result.outcome,
        ShellCommandOutcome::Backgrounded { .. }
    ));
    let _ = manager
        .stop(&task_id, "test cleanup", 1024)
        .await
        .expect("detached task should stop");
}

#[tokio::test]
async fn user_shell_runner_reports_current_foreground_task_id() {
    let temp = TempDir::new().expect("tempdir");
    let ctx = test_context(&temp);
    let token = CancellationToken::new();
    let manager = ctx.background_tasks.clone();
    let runner_manager = manager.clone();
    let task = tokio::spawn(async move {
        execute_shell_command(ShellExecutionRequest {
            id: "shell-task-id".to_owned(),
            command: "sleep 30".to_owned(),
            cwd: ctx.cwd.clone(),
            origin: ShellCommandOrigin::UserShellMode,
            foreground_timeout: Duration::from_secs(60),
            background_timeout: Duration::from_secs(600),
            max_output_bytes: 1024,
            cancel_token: token,
            stream_update: None,
            background_tasks: Some(runner_manager),
        })
        .await
    });

    let task_id = loop {
        let tasks = manager.list(true, 10).await;
        if let Some(task) = tasks.first() {
            break task.task_id.clone();
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    manager.detach(&task_id).await.expect("detach");
    let result = task.await.expect("join").expect("runner returns result");

    assert_eq!(result.foreground_task_id.as_deref(), Some(task_id.as_str()));
    assert!(matches!(
        result.outcome,
        ShellCommandOutcome::Backgrounded { task_id: outcome_task_id }
            if outcome_task_id.as_ref() == task_id.as_str()
    ));
    let _ = manager
        .stop(&task_id, "test cleanup", 1024)
        .await
        .expect("detached task should stop");
}
