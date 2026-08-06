use super::*;

#[test]
fn task_list_result_shows_empty_notice() {
    let result = task_list_result(&[], true);
    assert!(result.content.contains("active_background_tasks: 0"));
    assert!(result.content.contains("No background tasks found."));
}

#[test]
fn task_list_result_lists_tasks() {
    let snapshot = BackgroundTaskSnapshot {
        task_id: "bash-abc".to_owned(),
        kind: BackgroundTaskKind::Bash,
        status: BackgroundTaskStatus::Running,
        description: "long command".to_owned(),
        elapsed: Duration::from_secs(5),
        output: None,
        answers: None,
        delegate: None,
        swarm: None,
        workflow: None,
    };
    let result = task_list_result(&[snapshot], true);
    assert!(result.content.contains("active_background_tasks: 1"));
    assert!(result.content.contains("task_id: bash-abc"));
    assert!(result.content.contains("status: running"));
}

#[tokio::test]
async fn task_list_tool_lists_active_tasks() {
    let manager = BackgroundTaskManager::new();
    manager
        .start_question("q-1".to_owned(), "Pick one".to_owned())
        .await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_background_tasks(manager);
    let tool = TaskListTool;
    let result = tool.execute(&ctx, json!({})).await.expect("execute");
    assert!(!result.is_error);
    assert!(result.content.contains("active_background_tasks: 1"));
    assert!(result.content.contains("task_id: q-1"));
}

#[tokio::test]
async fn task_list_uses_metadata_only_enumeration_and_excludes_delegates() {
    let workspace = tempfile::tempdir().expect("workspace");
    let tasks = tempfile::tempdir().expect("tasks");
    let secret_log = "SECRET_TASK_LOG_BODY_MUST_NOT_APPEAR_IN_TASK_LIST";
    tokio::fs::write(
        tasks.path().join("bash-persisted.status.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "task_id": "bash-persisted",
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
    tokio::fs::write(
        tasks.path().join("bash-persisted.log"),
        secret_log.as_bytes(),
    )
    .await
    .expect("write log");

    let manager = BackgroundTaskManager::new().with_persistence_dir(tasks.path().to_path_buf());
    manager
        .start_question("q-list".to_owned(), "Pick one".to_owned())
        .await;

    let ctx = ToolContext::new(workspace.path())
        .expect("tool context")
        .with_background_tasks(manager.clone());
    let agent = ctx
        .multi_agent
        .start_foreground_delegate_for_test("should not appear in TaskList");
    manager.start_delegate(agent.clone()).await;
    let swarm_id = ctx.multi_agent.create_swarm_for_test(vec![(
        "runtime swarm must not appear",
        crate::multi_agent::AgentLifecycleState::Running,
    )]);

    let result = TaskListTool
        .execute(&ctx, json!({ "active_only": false }))
        .await
        .expect("execute");

    assert!(!result.is_error);
    assert!(result.content.contains("task_id: q-list"));
    assert!(result.content.contains("kind: question"));
    assert!(result.content.contains("task_id: bash-persisted"));
    assert!(result.content.contains("kind: bash"));
    assert!(
        !result.content.contains(secret_log),
        "TaskList must not hydrate persisted logs: {}",
        result.content
    );
    assert!(
        !result.content.contains(agent.id.as_str()),
        "TaskList must exclude manager delegate records"
    );
    assert!(
        !result.content.contains(&swarm_id),
        "TaskList must not synthesize runtime swarms"
    );
    assert!(!result.content.contains("kind: delegate"));
    assert!(!result.content.contains("kind: delegate-swarm"));

    let listed = result.details.as_ref().unwrap()["tasks"]
        .as_array()
        .expect("tasks array");
    assert_eq!(listed.len(), 2);
    for task in listed {
        let kind = task["kind"].as_str().expect("kind");
        assert!(
            matches!(kind, "bash" | "question" | "workflow"),
            "unexpected kind {kind}"
        );
        assert!(task.get("stdout").is_none());
        assert!(task.get("output").is_none());
    }

    let metadata = manager.list_metadata(false).await;
    assert!(
        metadata.iter().all(|snap| snap.output.is_none()),
        "list_metadata must never hydrate output bodies"
    );
    assert!(
        metadata.iter().all(|snap| {
            matches!(
                snap.kind,
                BackgroundTaskKind::Bash
                    | BackgroundTaskKind::Question
                    | BackgroundTaskKind::Workflow
            )
        }),
        "list_metadata must exclude delegate/swarm kinds"
    );
    assert!(
        !metadata
            .iter()
            .any(|snap| snap.task_id == agent.id.as_str()),
        "list_metadata must not surface manager delegate projections"
    );
    // Manager still tracks delegate projections for TaskOutput/TaskStop adapters.
    assert_eq!(
        manager.task_kind(agent.id.as_str()).await,
        Some(BackgroundTaskKind::Delegate)
    );
}

#[tokio::test]
async fn list_metadata_excludes_delegate_and_swarm_kinds() {
    let workspace = tempfile::tempdir().expect("workspace");
    let manager = BackgroundTaskManager::new();
    manager
        .start_question("q-meta".to_owned(), "Pick one".to_owned())
        .await;

    let ctx = ToolContext::new(workspace.path())
        .expect("tool context")
        .with_background_tasks(manager.clone());
    let agent = ctx
        .multi_agent
        .start_foreground_delegate_for_test("delegate projection must not list");
    manager.start_delegate(agent.clone()).await;

    let swarm_id = ctx.multi_agent.create_swarm_for_test(vec![(
        "swarm child",
        crate::multi_agent::AgentLifecycleState::Running,
    )]);
    let swarm = ctx
        .multi_agent
        .swarm_snapshot(&swarm_id)
        .expect("swarm snapshot");
    manager.start_delegate_swarm(swarm).await;

    let metadata = manager.list_metadata(false).await;
    assert!(
        metadata.iter().all(|snap| {
            matches!(
                snap.kind,
                BackgroundTaskKind::Bash
                    | BackgroundTaskKind::Question
                    | BackgroundTaskKind::Workflow
            )
        }),
        "list_metadata must only return Bash|Question|Workflow"
    );
    assert_eq!(metadata.len(), 1);
    assert_eq!(metadata[0].task_id, "q-meta");
    assert!(
        !metadata
            .iter()
            .any(|snap| snap.task_id == agent.id.as_str()),
        "delegate records must not appear in list_metadata"
    );
    assert!(
        !metadata.iter().any(|snap| snap.task_id == swarm_id),
        "swarm records must not appear in list_metadata"
    );
    // Manager may still track projections for other adapters.
    assert_eq!(
        manager.task_kind(agent.id.as_str()).await,
        Some(BackgroundTaskKind::Delegate)
    );
    assert_eq!(
        manager.task_kind(&swarm_id).await,
        Some(BackgroundTaskKind::DelegateSwarm)
    );
}
