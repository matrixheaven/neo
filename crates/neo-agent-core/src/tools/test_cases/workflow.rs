use super::*;
use crate::workflow::WorkflowActor;
use crate::workflow::WorkflowChildKey;
use crate::workflow::WorkflowChildKind;
use crate::workflow::WorkflowChildRow;
use crate::workflow::WorkflowChildState;
use crate::workflow::WorkflowLaunchRequest;
use crate::workflow::WorkflowLimits;
use crate::workflow::WorkflowPhase;
use crate::workflow::WorkflowRuntime;
use crate::workflow::WorkflowState;
use crate::workflow::journal::JournalPayload;
use crate::workflow::journal::collect_journal;

fn workflow_child_for_test(
    key: WorkflowChildKey,
    child_kind: WorkflowChildKind,
    agent_id: Option<String>,
    state: WorkflowChildState,
) -> WorkflowChildRow {
    WorkflowChildRow {
        key,
        child_kind,
        phase_id: None,
        agent_id,
        state,
        title: Some("Review".to_owned()),
        role: None,
        queued_at_ms: Some(1_000),
        started_at_ms: Some(2_000),
        updated_at_ms: 3_000,
        terminal_at_ms: None,
        terminal_summary: None,
        error_summary: None,
        actual_usage: None,
        latest_activity: None,
        generated_files: Vec::new(),
    }
}

fn live_workflow_agent_for_test(
    state: crate::multi_agent::AgentLifecycleState,
) -> crate::multi_agent::AgentProgressSnapshot {
    use crate::multi_agent::{AgentId, AgentRunMode, AgentToolActivityPhase, DelegateToolProgress};

    crate::multi_agent::AgentProgressSnapshot {
        agent_id: AgentId::from_suffix_for_test("workflow-live"),
        state,
        mode: AgentRunMode::Foreground,
        detached_from_foreground: false,
        started_at_ms: Some(1_500),
        updated_at_ms: 2_500,
        terminal_at_ms: None,
        terminal_reason: None,
        run_count: 1,
        live_messages_received: 1,
        tool_count: 1,
        token_count: 10,
        cache_read_token_count: 2,
        cache_write_token_count: 3,
        elapsed_ms: 1_000,
        latest_text: Some("fallback text".to_owned()),
        latest_thinking: None,
        last_tool: Some(DelegateToolProgress {
            id: "tool-1".to_owned(),
            name: "Read".to_owned(),
            summary: Some("src/lib.rs".to_owned()),
            phase: AgentToolActivityPhase::Ongoing,
            output: None,
            files: Vec::new(),
            output_ref: None,
        }),
        outcome: None,
    }
}

#[test]
fn workflow_live_child_merge_uses_real_activity_usage_and_preserves_durable_facts() {
    use crate::multi_agent::AgentLifecycleState;

    let mut live = live_workflow_agent_for_test(AgentLifecycleState::Running);
    let mut child = workflow_child_for_test(
        WorkflowChildKey::DirectDelegate {
            invocation_id: "inv-1".to_owned(),
        },
        WorkflowChildKind::Delegate,
        Some(live.agent_id.as_str().to_owned()),
        WorkflowChildState::Recovering,
    );

    merge_live_workflow_child(&mut child, &live);
    assert_eq!(child.state, WorkflowChildState::Recovering);
    assert_eq!(child.actual_usage.as_ref().unwrap()["total_tokens"], 15);
    assert_eq!(child.latest_activity.as_deref(), Some("Read: src/lib.rs"));

    child.state = WorkflowChildState::Completed;
    child.terminal_summary = Some("durable result".to_owned());
    child.actual_usage = Some(serde_json::json!({"input_tokens": 7}));
    live.token_count = 99;
    merge_live_workflow_child(&mut child, &live);
    assert_eq!(child.state, WorkflowChildState::Completed);
    assert_eq!(child.actual_usage.as_ref().unwrap()["input_tokens"], 7);
    assert_eq!(child.terminal_summary.as_deref(), Some("durable result"));

    child.state = WorkflowChildState::Queued;
    merge_live_workflow_child(&mut child, &live);
    assert_eq!(child.state, WorkflowChildState::Queued);

    child.state = WorkflowChildState::Recovering;
    live.state = AgentLifecycleState::Completed;
    merge_live_workflow_child(&mut child, &live);
    assert_eq!(child.state, WorkflowChildState::Recovering);
}

#[tokio::test]
async fn workflow_task_controls_use_registered_handle() {
    let session = tempfile::tempdir().expect("session");
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = runtime
        .create_run(
            session.path(),
            WorkflowLaunchRequest {
                name: "controls".to_owned(),
                description: "controls test".to_owned(),
                phases: vec![WorkflowPhase {
                    id: "work".to_owned(),
                    description: "work".to_owned(),
                }],
                script: "neo.phase('work')".to_owned(),
                args: json!({}),
                launch_source: "test".to_owned(),
                output_schema: None,
                display_name: None,
                input_schema: None,
                definition_origin: None,
                inline_unsaved: false,
            },
        )
        .await
        .expect("create workflow");
    let task_id = handle.run_id.0.clone();
    let manager = BackgroundTaskManager::new();
    manager
        .start_workflow(task_id.clone(), "controls test".to_owned(), handle)
        .await
        .expect("register workflow");

    let pause = manager
        .pause_workflow(&task_id, WorkflowActor::Model)
        .await
        .expect("pause route");
    assert!(!pause.is_error);
    assert_eq!(
        manager
            .snapshot(&task_id)
            .await
            .expect("paused snapshot")
            .status,
        BackgroundTaskStatus::Paused
    );
    runtime
        .bind_runner(|handle, _metadata, _session_dir| async move {
            handle.stop_token().cancelled().await;
            Ok(())
        })
        .expect("bind test runner");

    let resume = manager
        .resume_workflow(&task_id, WorkflowActor::Human)
        .await
        .expect("resume route");
    assert!(!resume.is_error);
    assert_eq!(
        manager
            .snapshot(&task_id)
            .await
            .expect("running snapshot")
            .status,
        BackgroundTaskStatus::Running
    );

    manager
        .stop_with_actor(&task_id, "test stop", 1024, WorkflowActor::Human)
        .await
        .expect("stop route");
    let output = manager
        .output(&task_id, true, Duration::from_secs(1), 4096)
        .await
        .expect("terminal workflow output");
    assert_eq!(
        output.details.as_ref().expect("details")["state"],
        "cancelled"
    );

    let records = collect_journal(
        &crate::workflow::journal_path(session.path(), &crate::workflow::WorkflowId(task_id)),
        None,
        crate::workflow::WorkflowLimits::default().journal_record_bytes,
        crate::workflow::WorkflowLimits::default().journal_total_bytes,
    )
    .expect("journal");
    assert!(records.iter().any(|record| matches!(
        &record.payload,
        JournalPayload::StateChanged {
            new: WorkflowState::Paused,
            actor: WorkflowActor::Model,
            ..
        }
    )));
    // Resume is Paused -> Queued (Human), then start_worker Queued -> Running (Runtime).
    assert!(records.iter().any(|record| matches!(
        &record.payload,
        JournalPayload::StateChanged {
            new: WorkflowState::Queued,
            actor: WorkflowActor::Human,
            ..
        }
    )));
    assert!(records.iter().any(|record| matches!(
        &record.payload,
        JournalPayload::StateChanged {
            new: WorkflowState::Running,
            actor: WorkflowActor::Runtime,
            ..
        }
    )));
    assert!(matches!(
        records.last().map(|r| &r.payload),
        Some(JournalPayload::StateChanged {
            new: WorkflowState::Cancelled,
            actor: WorkflowActor::Human,
            ..
        })
    ));
}

#[tokio::test]
async fn workflow_registration_collision_is_fail_closed() {
    let session = tempfile::tempdir().expect("session");
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = runtime
        .create_run(
            session.path(),
            WorkflowLaunchRequest {
                name: "collision".to_owned(),
                description: "collision test".to_owned(),
                phases: vec![WorkflowPhase {
                    id: "work".to_owned(),
                    description: "work".to_owned(),
                }],
                script: "neo.phase('work')".to_owned(),
                args: json!({}),
                launch_source: "test".to_owned(),
                output_schema: None,
                display_name: None,
                input_schema: None,
                definition_origin: None,
                inline_unsaved: false,
            },
        )
        .await
        .expect("create workflow");
    let task_id = handle.run_id.0.clone();
    let manager = BackgroundTaskManager::new();
    manager
        .start_workflow(task_id.clone(), "first".to_owned(), handle.clone())
        .await
        .expect("first registration");

    let error = manager
        .start_workflow(task_id.clone(), "replacement".to_owned(), handle)
        .await
        .expect_err("duplicate registration must fail");
    assert!(error.to_string().contains("already exists"));
    assert_eq!(manager.list(false, 10).await.len(), 1);
    assert_eq!(
        manager
            .workflow_handle(&task_id)
            .await
            .expect("original handle")
            .run_id
            .0,
        task_id
    );
}

#[tokio::test]
async fn workflow_pause_resume_reject_non_workflows_with_typed_results() {
    let manager = BackgroundTaskManager::new();
    manager
        .start_question("question-control".to_owned(), "Pick one".to_owned())
        .await;

    for result in [
        manager
            .pause_workflow("question-control", WorkflowActor::Model)
            .await
            .expect("pause result"),
        manager
            .resume_workflow("question-control", WorkflowActor::Human)
            .await
            .expect("resume result"),
    ] {
        assert!(result.is_error);
        let details = result.details.expect("typed error details");
        assert_eq!(details["kind"], "question");
        assert_eq!(details["outcome"], "unsupported");
        assert_eq!(details["supported_kind"], "workflow");
    }

    assert_eq!(
        manager
            .snapshot("question-control")
            .await
            .expect("question snapshot")
            .status,
        BackgroundTaskStatus::WaitingForUser
    );
}
