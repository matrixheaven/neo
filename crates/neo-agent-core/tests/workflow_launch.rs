use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, ApprovalAction,
    ApprovalPresentation, ApprovalResponse, PermissionMode, ToolRegistry, ToolResult,
    harness::FakeHarness,
};
use neo_ai::AiStreamEvent;
use serde_json::{Value, json};
use tokio::sync::Notify;

fn valid_input(name: &str) -> Value {
    json!({
        "name": name,
        "description": "Run a reviewed workflow",
        "phases": [{"id": "work", "description": "Do the work"}],
        "script": "neo.phase('work')",
        "args": {"target": "core"}
    })
}

fn harness_for_calls(calls: &[(&str, Value)]) -> FakeHarness {
    let mut first = vec![AiStreamEvent::MessageStart {
        id: "msg_1".to_owned(),
    }];
    for (id, arguments) in calls {
        first.push(AiStreamEvent::ToolCallStart {
            id: (*id).to_owned(),
            name: "RunWorkflow".to_owned(),
        });
        first.push(AiStreamEvent::ToolCallEnd {
            id: (*id).to_owned(),
            raw_arguments: arguments.to_string(),
        });
    }
    first.push(AiStreamEvent::MessageEnd {
        stop_reason: neo_ai::StopReason::ToolUse,
        usage: None,
    });
    FakeHarness::from_turns([
        first,
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ])
}

fn config_for(
    harness: &FakeHarness,
    session_dir: &std::path::Path,
    mode: PermissionMode,
) -> AgentConfig {
    AgentConfig::for_model(harness.model())
        .with_permission_mode(mode)
        .with_session_directory(session_dir)
        .with_agent_id("main")
}

async fn run(harness: &FakeHarness, config: AgentConfig) -> (Vec<AgentEvent>, AgentConfig) {
    let runtime = AgentRuntime::with_tools(
        config.clone(),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("launch"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn succeeds");
    (events, config)
}

fn workflow_results(events: &[AgentEvent]) -> Vec<ToolResult> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolExecutionFinished { name, result, .. } if name == "RunWorkflow" => {
                Some(result.clone())
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn missing_capability_and_invalid_input_never_open_workflow_approval() {
    let session = tempfile::tempdir().unwrap();
    let approval_calls = Arc::new(AtomicUsize::new(0));
    let harness = harness_for_calls(&[("missing", valid_input("missing"))]);
    let calls = Arc::clone(&approval_calls);
    let config = config_for(&harness, session.path(), PermissionMode::Ask).with_approval_handler(
        move |_| {
            calls.fetch_add(1, Ordering::AcqRel);
            panic!("missing capability must not prompt")
        },
    );
    let (events, _) = run(&harness, config).await;
    assert_eq!(approval_calls.load(Ordering::Acquire), 0);
    assert!(
        workflow_results(&events)[0]
            .content
            .contains("requires a launch capability")
    );

    let harness = harness_for_calls(&[("invalid", json!({"title": "legacy"}))]);
    let calls = Arc::clone(&approval_calls);
    let config = config_for(&harness, session.path(), PermissionMode::Ask).with_approval_handler(
        move |_| {
            calls.fetch_add(1, Ordering::AcqRel);
            panic!("invalid input must not prompt")
        },
    );
    config.workflow_capability.grant();
    let (events, config) = run(&harness, config).await;
    assert_eq!(approval_calls.load(Ordering::Acquire), 0);
    assert_eq!(
        workflow_results(&events)[0].details.as_ref().unwrap()["kind"],
        "invalid_workflow_input"
    );
    assert!(config.workflow_capability.inspect());
}

#[tokio::test]
async fn source_and_run_metadata_limits_return_typed_invalid_input() {
    for (input, limits) in [
        {
            let mut input = valid_input("source-limit");
            input["script"] = Value::String("neo.phase('work')".to_owned());
            let limits = neo_agent_core::workflow::WorkflowLimits {
                lua_source_bytes: 8,
                ..Default::default()
            };
            (input, limits)
        },
        {
            let mut input = valid_input("metadata-limit");
            input["args"] = json!({"payload": "x".repeat(1024)});
            let limits = neo_agent_core::workflow::WorkflowLimits {
                journal_record_bytes: 256,
                ..Default::default()
            };
            (input, limits)
        },
    ] {
        let session = tempfile::tempdir().unwrap();
        let harness = harness_for_calls(&[("invalid", input)]);
        let mut config = config_for(&harness, session.path(), PermissionMode::Auto);
        config.workflow_runtime = neo_agent_core::workflow::WorkflowRuntime::new(limits);
        config.workflow_capability.grant();

        let (events, config) = run(&harness, config).await;
        let result = &workflow_results(&events)[0];
        assert_eq!(
            result.details.as_ref().unwrap()["kind"],
            "invalid_workflow_input"
        );
        assert!(config.workflow_capability.inspect());
        assert!(config.background_tasks.list(false, 10).await.is_empty());
    }
}

#[tokio::test]
async fn ask_launch_uses_typed_full_review_and_returns_registered_running_task() {
    let session = tempfile::tempdir().unwrap();
    let worker_started = Arc::new(Notify::new());
    let worker_release = Arc::new(Notify::new());
    let harness = harness_for_calls(&[("launch", valid_input("reviewed"))]);
    let config = config_for(&harness, session.path(), PermissionMode::Ask).with_approval_handler(
        |request| {
            assert_eq!(
                request.operation,
                neo_agent_core::PermissionOperation::WorkflowLaunch
            );
            let ApprovalPresentation::Workflow { workflow, .. } = &request.presentation else {
                panic!("typed workflow presentation")
            };
            assert_eq!(workflow.name, "reviewed");
            assert_eq!(workflow.source, "neo.phase('work')");
            assert!(workflow.warning.contains("orchestration only"));
            assert_eq!(workflow.phases, ["work: Do the work"]);
            ApprovalResponse::Selected {
                request_id: request.id.clone(),
                action: ApprovalAction::LaunchWorkflow,
                feedback: None,
            }
        },
    );
    config.workflow_capability.grant();
    config
        .workflow_runtime
        .bind_runner({
            let worker_started = Arc::clone(&worker_started);
            let worker_release = Arc::clone(&worker_release);
            move |_handle, _metadata, _session_dir| {
                let worker_started = Arc::clone(&worker_started);
                let worker_release = Arc::clone(&worker_release);
                async move {
                    worker_started.notify_one();
                    worker_release.notified().await;
                    Ok(())
                }
            }
        })
        .unwrap();

    let (events, config) = run(&harness, config).await;
    worker_started.notified().await;
    let result = &workflow_results(&events)[0];
    assert!(!result.is_error);
    let task_id = result.details.as_ref().unwrap()["task_id"]
        .as_str()
        .unwrap();
    assert_eq!(result.details.as_ref().unwrap()["status"], "running");
    let handle = config
        .background_tasks
        .workflow_handle(task_id)
        .await
        .expect("registered workflow handle");
    assert_eq!(handle.run_id.0, task_id);
    assert_eq!(
        handle.snapshot().await.state,
        neo_agent_core::workflow::WorkflowState::Running
    );
    assert!(!config.workflow_capability.inspect());
    worker_release.notify_one();
}

#[tokio::test]
async fn workflow_projection_emits_started_updated_and_finished_after_durable_transitions() {
    let session = tempfile::tempdir().unwrap();
    let mut input = valid_input("projected");
    input["script"] = Value::String(
        "neo.phase('work')\nneo.log('verification running')\nneo.report('scoped checks passed')"
            .to_owned(),
    );
    let harness = harness_for_calls(&[("launch", input)]);
    let config = config_for(&harness, session.path(), PermissionMode::Auto);
    config.workflow_capability.grant();
    let idle_events = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&idle_events);
    let _idle_lease = config
        .workflow_dispatch_resolver
        .lease_idle_event_route(
            Some(session.path()),
            Arc::new(move |event| captured.lock().expect("idle events").push(event)),
        )
        .expect("idle workflow event route");

    let (mut events, config) = run(&harness, config).await;
    let task_id = workflow_results(&events)[0].details.as_ref().unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let handle = config
        .background_tasks
        .workflow_handle(&task_id)
        .await
        .expect("registered workflow");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if handle.snapshot().await.state.is_terminal() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("workflow reaches terminal state");
    tokio::task::yield_now().await;
    events.extend(idle_events.lock().expect("idle events").clone());

    let projections = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::WorkflowStarted { workflow, .. } => Some(("started", workflow)),
            AgentEvent::WorkflowUpdated { workflow, .. } => Some(("updated", workflow)),
            AgentEvent::WorkflowFinished { workflow, .. } => Some(("finished", workflow)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let started = projections
        .iter()
        .find(|(stage, _)| *stage == "started")
        .expect("durable started projection")
        .1;
    let finished = projections
        .iter()
        .rev()
        .find(|(stage, _)| *stage == "finished")
        .expect("durable finished projection")
        .1;

    assert!(
        projections
            .iter()
            .any(|(stage, workflow)| { *stage == "updated" && workflow.invocation_count > 0 }),
        "durable invocations emit updated projections"
    );
    assert_eq!(started.projection_sequence, Some(0));
    assert!(
        finished.projection_sequence.unwrap() > started.projection_sequence.unwrap(),
        "finished projection follows durable journal order"
    );
    assert_eq!(
        finished.state,
        neo_agent_core::workflow::WorkflowState::Completed
    );
    assert_eq!(finished.current_phase.as_deref(), Some("work"));
    assert_eq!(
        finished.latest_log_summary.as_deref(),
        Some("verification running")
    );
    assert_eq!(
        finished.latest_report_summary.as_deref(),
        Some("scoped checks passed")
    );
    assert!(
        projections
            .iter()
            .all(|(_, workflow)| workflow.steps.is_empty())
    );
}

#[tokio::test]
async fn ask_revise_preserves_capability_and_cancel_revokes_without_run() {
    for (action, remains) in [
        (
            ApprovalAction::ReviseWorkflow {
                preset_feedback: None,
            },
            true,
        ),
        (ApprovalAction::CancelWorkflow, false),
    ] {
        let session = tempfile::tempdir().unwrap();
        let harness = harness_for_calls(&[("review", valid_input("review"))]);
        let selected = action.clone();
        let config = config_for(&harness, session.path(), PermissionMode::Ask)
            .with_approval_handler(move |request| ApprovalResponse::Selected {
                request_id: request.id.clone(),
                action: selected.clone(),
                feedback: matches!(selected, ApprovalAction::ReviseWorkflow { .. })
                    .then(|| "split the phases".to_owned()),
            });
        config.workflow_capability.grant();
        let (events, config) = run(&harness, config).await;
        assert_eq!(config.workflow_capability.inspect(), remains);
        assert!(config.background_tasks.list(false, 10).await.is_empty());
        assert!(workflow_results(&events)[0].is_error != remains);
    }
}

#[tokio::test]
async fn auto_and_yolo_cannot_bypass_capability_and_one_grant_launches_once() {
    for mode in [PermissionMode::Auto, PermissionMode::Yolo] {
        let session = tempfile::tempdir().unwrap();
        let harness = harness_for_calls(&[("missing", valid_input("missing"))]);
        let (events, _) = run(&harness, config_for(&harness, session.path(), mode)).await;
        assert!(
            workflow_results(&events)[0]
                .content
                .contains("requires a launch capability")
        );
    }

    let session = tempfile::tempdir().unwrap();
    let harness = harness_for_calls(&[
        ("first", valid_input("first")),
        ("second", valid_input("second")),
    ]);
    let config = config_for(&harness, session.path(), PermissionMode::Auto);
    config.workflow_capability.grant();
    let (events, config) = run(&harness, config).await;
    let results = workflow_results(&events);
    assert_eq!(results.iter().filter(|result| !result.is_error).count(), 1);
    assert_eq!(results.iter().filter(|result| result.is_error).count(), 1);
    assert_eq!(config.background_tasks.list(false, 10).await.len(), 1);
}

#[tokio::test]
async fn durable_create_failure_rolls_reservation_back() {
    let root = tempfile::tempdir().unwrap();
    let session_file = root.path().join("not-a-directory");
    std::fs::write(&session_file, b"x").unwrap();
    let harness = harness_for_calls(&[("create", valid_input("create"))]);
    let config = config_for(&harness, &session_file, PermissionMode::Auto);
    config.workflow_capability.grant();
    let (events, config) = run(&harness, config).await;
    assert!(
        workflow_results(&events)[0]
            .content
            .contains("workflow launch failed")
    );
    assert!(config.workflow_capability.inspect());
    assert!(config.background_tasks.list(false, 10).await.is_empty());
}
