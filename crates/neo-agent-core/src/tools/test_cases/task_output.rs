use super::*;
use crate::ShellLimits;
use crate::ShellRuntime;
use crate::ToolAccess;

#[tokio::test]
async fn task_output_clamps_persisted_log_to_context_and_runtime_limits() {
    let workspace = tempfile::tempdir().expect("workspace");
    let tasks = tempfile::tempdir().expect("tasks");
    tokio::fs::write(
        tasks.path().join("bash-test.status.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "task_id": "bash-test",
            "started_at_ms": 1,
            "finished_at_ms": 2,
            "exit": {
                "status": "completed",
                "exit_code": 0,
                "signal": null,
                "resource_limit": null,
                "omitted_output_bytes": 0,
                "omitted_log_bytes": 0
            },
            "cleanup_errors": []
        }))
        .unwrap(),
    )
    .await
    .expect("write status");
    tokio::fs::write(tasks.path().join("bash-test.log"), b"0123456789")
        .await
        .expect("write log");
    let limits = ShellLimits {
        max_output_bytes: 4,
        ..ShellLimits::default()
    };
    let manager = BackgroundTaskManager::new().with_persistence_dir(tasks.path().to_path_buf());
    let mut context = ToolContext::new(workspace.path())
        .expect("tool context")
        .with_access(ToolAccess::all())
        .with_background_tasks(manager)
        .with_shell_runtime(ShellRuntime::new(
            limits,
            PathBuf::from("unused-guardian"),
            workspace.path().join("runtime"),
        ));
    context.max_output_bytes = 8;

    let result = TaskOutputTool
        .execute(
            &context,
            json!({ "task_id": "bash-test", "max_output_bytes": 100 }),
        )
        .await
        .expect("task output");

    assert_eq!(result.details.as_ref().unwrap()["stdout"], "0123");
    assert_eq!(result.details.as_ref().unwrap()["truncated"], true);
}

#[tokio::test]
async fn task_output_tool_reads_runtime_delegate_without_background_record() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path()).unwrap();
    let agent = ctx
        .multi_agent
        .start_foreground_delegate_for_test("calculate a small sum");

    let result = TaskOutputTool
        .execute(&ctx, json!({ "task_id": agent.id.as_str() }))
        .await
        .expect("execute");

    assert!(!result.is_error);
    let content: serde_json::Value = serde_json::from_str(&result.content).expect("JSON result");
    assert_eq!(content["kind"], "delegate_result");
    assert_eq!(content["target"]["id"], agent.id.as_str());
    assert_eq!(result.details.as_ref().unwrap()["kind"], "delegate");
    assert_eq!(
        result.details.as_ref().unwrap()["agent_id"],
        agent.id.as_str()
    );
}

#[tokio::test]
async fn task_output_tool_preserves_runtime_delegate_context_mode() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path()).unwrap();
    let agent = ctx.multi_agent.start_delegate(
        "calculate a small sum",
        None,
        crate::multi_agent::AgentRole::Coder,
        crate::multi_agent::AgentRunMode::Foreground,
        crate::multi_agent::DelegateContext::Summary,
        crate::multi_agent::AgentPathKind::Root,
    );

    let result = TaskOutputTool
        .execute(&ctx, json!({ "task_id": agent.id.as_str() }))
        .await
        .expect("execute");

    assert!(!result.is_error);
    let content: serde_json::Value = serde_json::from_str(&result.content).expect("JSON result");
    assert_eq!(content["context_mode"], "summary");
    assert_eq!(result.details.as_ref().unwrap()["context_mode"], "summary");
}
