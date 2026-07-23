use super::*;

#[test]
fn approval_presentation_counts_trailing_newline() {
    let presentation = approval_presentation(&serde_json::json!({
        "name": "line count",
        "description": "line count test",
        "phases": [{"id": "work", "description": "work"}],
        "script": "first\nsecond\n",
        "args": {}
    }))
    .unwrap();

    assert_eq!(presentation.line_count, 3);
}

#[tokio::test]
async fn rollback_failure_terminalizes_run_and_consumes_capability() {
    let session = tempfile::tempdir().unwrap();
    let runtime = crate::workflow::WorkflowRuntime::default();
    let capability = crate::workflow::WorkflowCapability::default();
    capability.grant().await;
    let reservation = capability.reserve().expect("reservation");
    let input = validated_input(&serde_json::json!({
        "name": "rollback",
        "description": "rollback test",
        "phases": [{"id": "work", "description": "work"}],
        "script": "neo.phase('work')",
        "args": {}
    }))
    .unwrap();
    let handle = runtime
        .create_run(
            session.path(),
            input.launch_request(crate::PermissionMode::Auto),
        )
        .await
        .unwrap();
    runtime.inject_rollback_remove_failure();

    let result = rollback_registration_failure(
        &runtime,
        reservation,
        &handle,
        "injected registration failure",
    )
    .await;

    assert!(result.is_error);
    assert_eq!(
        result.details.as_ref().unwrap()["capability_consumed"],
        true
    );
    assert!(!capability.inspect());
    assert_eq!(
        handle.snapshot().await.state,
        crate::workflow::WorkflowState::Failed
    );
    assert!(crate::workflow::run_dir(session.path(), &handle.run_id).exists());
    assert!(
        crate::workflow::read_journal(&crate::workflow::journal_path(
            session.path(),
            &handle.run_id,
        ))
        .unwrap()
        .iter()
        .any(|record| matches!(
            record,
            crate::workflow::JournalRecord::StateChanged {
                new: crate::workflow::WorkflowState::Failed,
                ..
            }
        ))
    );
}

#[tokio::test]
async fn capability_generation_change_rollback_failure_terminalizes_old_run() {
    let session = tempfile::tempdir().unwrap();
    let runtime = crate::workflow::WorkflowRuntime::default();
    let background_tasks = crate::tools::BackgroundTaskManager::new();
    let capability = crate::workflow::WorkflowCapability::default();
    capability.grant().await;
    let reservation = capability.reserve().expect("reservation");
    let input = validated_input(&serde_json::json!({
        "name": "generation race",
        "description": "generation race test",
        "phases": [{"id": "work", "description": "work"}],
        "script": "neo.phase('work')",
        "args": {}
    }))
    .unwrap();
    let handle = runtime
        .create_run(
            session.path(),
            input.launch_request(crate::PermissionMode::Auto),
        )
        .await
        .unwrap();
    let task_id = handle.run_id.0.clone();
    background_tasks
        .start_workflow(task_id.clone(), input.description, handle.clone())
        .await
        .unwrap();

    capability.revoke().await;
    capability.grant().await;
    assert!(!reservation.commit());
    runtime.inject_rollback_remove_failure();

    let result = rollback_capability_change(&runtime, &background_tasks, &handle).await;

    assert!(result.is_error);
    assert_eq!(
        result.details.as_ref().unwrap()["reservation_consumed"],
        true
    );
    assert!(
        capability.inspect(),
        "the newer capability must remain available"
    );
    assert!(background_tasks.workflow_handle(&task_id).await.is_none());
    assert_eq!(handle.snapshot().await.state, WorkflowState::Failed);
}
