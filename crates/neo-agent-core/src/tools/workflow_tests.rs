use super::*;

#[test]
fn workflow_schema_is_exact_and_rejects_model_limits() {
    let schema = RunWorkflowTool.input_schema();
    let properties = schema["properties"]
        .as_object()
        .expect("workflow schema properties");
    let names = properties
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(
        names,
        std::collections::BTreeSet::from([
            "name",
            "description",
            "phases",
            "script",
            "args",
            "output_schema",
        ])
    );
    assert_eq!(schema["additionalProperties"], false);

    let error = validated_input(&serde_json::json!({
        "name": "bounded by runtime",
        "description": "model cannot set machine limits",
        "phases": [{"id": "work", "description": "work"}],
        "script": "neo.phase('work')",
        "args": {},
        "output_schema": {"type": "object"},
        "limits": {"token_cap": 1}
    }))
    .expect_err("model-supplied limits were accepted");
    assert!(error.contains("unknown field `limits`"), "{error}");
}

#[test]
fn approval_presentation_counts_trailing_newline() {
    let presentation = approval_presentation(&serde_json::json!({
        "name": "line count",
        "description": "line count test",
        "phases": [{"id": "work", "description": "work"}],
        "script": "first\nsecond\n",
        "args": {},
        "output_schema": {"type": "object"}
    }))
    .unwrap();

    assert_eq!(presentation.line_count, 3);
}

#[tokio::test]
async fn dynamic_tool_adapter_launches_only_through_coordinator() {
    let session = tempfile::tempdir().unwrap();
    let runtime = crate::workflow::WorkflowRuntime::default();
    let capability = crate::workflow::WorkflowCapability::default();
    let background_tasks = crate::tools::BackgroundTaskManager::new();
    capability.grant();
    let nonce = capability.launch_nonce().expect("nonce");
    let input = validated_input(&serde_json::json!({
        "name": "coord",
        "description": "coordinator path",
        "phases": [{"id": "work", "description": "work"}],
        "script": "neo.phase('work')",
        "args": {},
        "output_schema": {"type": "object"}
    }))
    .unwrap();
    runtime
        .bind_runner(|_handle, _metadata, _session_dir| async move { Ok(()) })
        .unwrap();

    let intent = WorkflowLaunchIntent::from_parts(
        input.launch_request(crate::PermissionMode::Auto),
        crate::workflow::WorkflowLaunchBinding {
            session_identity: session.path().display().to_string(),
            workspace_identity: session.path().display().to_string(),
            launch_nonce: nonce,
            actor: WorkflowActor::Model,
            permission_mode: crate::PermissionMode::Auto,
            parent_lineage: None,
            compiled_input_schema: None,
            schema_sha256: String::new(),
        },
    );
    let outcome = WorkflowLaunchCoordinator
        .launch(
            &intent,
            WorkflowLaunchHosts {
                runtime: &runtime,
                capability: &capability,
                background_tasks: &background_tasks,
                session_dir: session.path(),
            },
            LaunchAuthorizationMode::SessionCapability,
        )
        .await
        .expect("launch via coordinator");

    assert!(!capability.inspect());
    assert!(
        background_tasks
            .workflow_handle(&outcome.task_id)
            .await
            .is_some()
    );
}
