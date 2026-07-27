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
        "action": "run_inline",
        "name": name,
        "description": "Run a reviewed workflow",
        "phases": [{"id": "work", "description": "Do the work"}],
        "script": "neo.phase('work')\nreturn {}",
        "input_schema": {"type": "object"},
        "output_schema": {"type": "object"},
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
            name: "Workflow".to_owned(),
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
            AgentEvent::ToolExecutionFinished { name, result, .. } if name == "Workflow" => {
                Some(result.clone())
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn invalid_workflow_input_never_opens_approval() {
    let session = tempfile::tempdir().unwrap();
    let approval_calls = Arc::new(AtomicUsize::new(0));

    let harness = harness_for_calls(&[("unknown", json!({"action": "explode"}))]);
    let calls = Arc::clone(&approval_calls);
    let config = config_for(&harness, session.path(), PermissionMode::Ask).with_approval_handler(
        move |_| {
            calls.fetch_add(1, Ordering::AcqRel);
            panic!("invalid action must not prompt")
        },
    );
    let (events, _) = run(&harness, config).await;
    assert_eq!(approval_calls.load(Ordering::Acquire), 0);
    let details = workflow_results(&events)[0]
        .details
        .clone()
        .expect("structured details");
    assert_eq!(details["ok"], false);
    assert_eq!(details["error"]["code"], "workflow_action_invalid");
    assert_eq!(details["error"]["side_effect_occurred"], false);

    let harness = harness_for_calls(&[("missing", json!({"action": "run_inline"}))]);
    let calls = Arc::clone(&approval_calls);
    let config = config_for(&harness, session.path(), PermissionMode::Ask).with_approval_handler(
        move |_| {
            calls.fetch_add(1, Ordering::AcqRel);
            panic!("missing fields must not prompt")
        },
    );
    let (events, _) = run(&harness, config).await;
    assert_eq!(approval_calls.load(Ordering::Acquire), 0);
    let details = workflow_results(&events)[0]
        .details
        .clone()
        .expect("structured details");
    assert_eq!(details["ok"], false);
    assert_eq!(details["error"]["code"], "workflow_input_invalid");
    assert_eq!(details["error"]["side_effect_occurred"], false);
}

#[tokio::test]
async fn source_and_run_metadata_limits_return_typed_invalid_input() {
    for (input, limits) in [
        {
            let mut input = valid_input("source-limit");
            input["script"] = Value::String("neo.phase('work')\nreturn {}".to_owned());
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

        let (events, config) = run(&harness, config).await;
        let result = &workflow_results(&events)[0];
        assert!(result.is_error);
        let details = result.details.as_ref().expect("structured details");
        assert_eq!(details["ok"], false);
        assert!(
            matches!(
                details["error"]["code"].as_str().unwrap_or_default(),
                "workflow_input_invalid" | "workflow_definition_invalid"
            ),
            "unexpected error code: {details}"
        );
        assert_eq!(details["error"]["side_effect_occurred"], false);
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
            assert_eq!(workflow.source, "neo.phase('work')\nreturn {}");
            assert!(workflow.warning.contains("orchestration only"));
            assert_eq!(workflow.phases, ["work: Do the work"]);
            ApprovalResponse::Selected {
                request_id: request.id.clone(),
                action: ApprovalAction::LaunchWorkflow,
                feedback: None,
            }
        },
    );
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
    let task_id = result.details.as_ref().unwrap()["task"]["task_id"]
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
    worker_release.notify_one();
}

#[tokio::test]
async fn workflow_projection_emits_started_updated_and_finished_after_durable_transitions() {
    let session = tempfile::tempdir().unwrap();
    let mut input = valid_input("projected");
    input["script"] = Value::String(
        "neo.phase('work')\nneo.log('verification running')\nneo.report('scoped checks passed')\nreturn {}"
            .to_owned(),
    );
    let harness = harness_for_calls(&[("launch", input)]);
    let config = config_for(&harness, session.path(), PermissionMode::Auto);
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
    let task_id = workflow_results(&events)[0].details.as_ref().unwrap()["task"]["task_id"]
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
async fn ask_revise_and_cancel_create_no_run() {
    for (action, expect_error) in [
        (
            ApprovalAction::ReviseWorkflow {
                preset_feedback: None,
            },
            false,
        ),
        (ApprovalAction::CancelWorkflow, true),
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
        let (events, config) = run(&harness, config).await;
        assert!(config.background_tasks.list(false, 10).await.is_empty());
        let result = &workflow_results(&events)[0];
        assert_eq!(result.is_error, expect_error);
        if !expect_error {
            assert!(
                result
                    .content
                    .contains("No workflow save or run was created")
            );
        }
    }
}

#[tokio::test]
async fn auto_and_yolo_launch_without_slash_and_independent_launches_both_run() {
    for mode in [PermissionMode::Auto, PermissionMode::Yolo] {
        let session = tempfile::tempdir().unwrap();
        let harness = harness_for_calls(&[("launch", valid_input("direct"))]);
        let (events, config) = run(&harness, config_for(&harness, session.path(), mode)).await;
        let result = &workflow_results(&events)[0];
        assert!(
            !result.is_error,
            "{mode:?} launch failed: {}",
            result.content
        );
        assert_eq!(config.background_tasks.list(false, 10).await.len(), 1);
    }

    let session = tempfile::tempdir().unwrap();
    let harness = harness_for_calls(&[
        ("first", valid_input("first")),
        ("second", valid_input("second")),
    ]);
    let config = config_for(&harness, session.path(), PermissionMode::Auto);
    let (events, config) = run(&harness, config).await;
    let results = workflow_results(&events);
    assert_eq!(results.iter().filter(|result| !result.is_error).count(), 2);
    assert_eq!(config.background_tasks.list(false, 10).await.len(), 2);
}

#[tokio::test]
async fn durable_create_failure_returns_typed_error_without_run() {
    let root = tempfile::tempdir().unwrap();
    let session_file = root.path().join("not-a-directory");
    std::fs::write(&session_file, b"x").unwrap();
    let harness = harness_for_calls(&[("create", valid_input("create"))]);
    let config = config_for(&harness, &session_file, PermissionMode::Auto);
    let (events, config) = run(&harness, config).await;
    let result = &workflow_results(&events)[0];
    assert!(result.is_error);
    let details = result.details.as_ref().expect("structured details");
    assert_eq!(details["ok"], false);
    assert_eq!(details["error"]["side_effect_occurred"], false);
    assert!(config.background_tasks.list(false, 10).await.is_empty());
}

#[tokio::test]
async fn ask_save_uses_typed_save_review_and_persists_pair() {
    let session = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let neo_home = tempfile::tempdir().unwrap();
    let registry = neo_agent_core::workflow::WorkflowDefinitionRegistry::new(
        neo_agent_core::workflow::WorkflowDefinitionRegistryConfig {
            neo_home: neo_home.path().to_path_buf(),
            workspace: workspace.path().to_path_buf(),
            project_trusted: true,
            limits: neo_agent_core::workflow::WorkflowLimits::default(),
            builtins: Vec::new(),
        },
    );
    let save_input = json!({
        "action": "save",
        "name": "save-review",
        "description": "Persist a reviewed workflow",
        "phases": [{"id": "work", "description": "Do the work"}],
        "script": "neo.phase('work')\nreturn {}",
        "input_schema": {"type": "object"},
        "output_schema": {"type": "object"},
        "scope": "user"
    });
    let harness = harness_for_calls(&[("save", save_input)]);
    let config = config_for(&harness, session.path(), PermissionMode::Ask)
        .with_workflow_definitions(registry)
        .with_approval_handler(|request| {
            assert_eq!(
                request.operation,
                neo_agent_core::PermissionOperation::WorkflowSave
            );
            let neo_agent_core::ApprovalPresentation::WorkflowSave { save, .. } =
                &request.presentation
            else {
                panic!("typed workflow save presentation")
            };
            assert_eq!(save.name, "save-review");
            assert_eq!(save.scope, "user");
            assert!(!save.replace);
            assert!(
                save.source_path.ends_with("save-review.lua"),
                "{}",
                save.source_path.display()
            );
            assert!(
                save.manifest_path.ends_with("save-review.workflow.toml"),
                "{}",
                save.manifest_path.display()
            );
            assert_eq!(save.phases, ["work: Do the work"]);
            assert!(save.warning.contains("does not launch"));
            ApprovalResponse::Selected {
                request_id: request.id.clone(),
                action: ApprovalAction::SaveWorkflow,
                feedback: None,
            }
        });
    let (events, config) = run(&harness, config).await;
    let result = &workflow_results(&events)[0];
    assert!(!result.is_error, "save failed: {}", result.content);
    let details = result.details.as_ref().expect("structured details");
    assert_eq!(details["ok"], true);
    assert_eq!(details["status"], "saved");
    let pair_dir = neo_home.path().join("workflows");
    assert!(pair_dir.join("save-review.lua").is_file());
    assert!(pair_dir.join("save-review.workflow.toml").is_file());
    assert!(config.background_tasks.list(false, 10).await.is_empty());
}

#[tokio::test]
async fn invalid_saved_run_args_fail_before_approval_opens() {
    let session = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let neo_home = tempfile::tempdir().unwrap();
    let registry = neo_agent_core::workflow::WorkflowDefinitionRegistry::new(
        neo_agent_core::workflow::WorkflowDefinitionRegistryConfig {
            neo_home: neo_home.path().to_path_buf(),
            workspace: workspace.path().to_path_buf(),
            project_trusted: true,
            limits: neo_agent_core::workflow::WorkflowLimits::default(),
            builtins: Vec::new(),
        },
    );
    registry
        .save(
            neo_agent_core::workflow::WorkflowSaveScope::User,
            &neo_agent_core::workflow::WorkflowSaveRequest {
                display_name: "typed-args".to_owned(),
                name: "typed-args".to_owned(),
                description: "Requires an integer target".to_owned(),
                phases: vec![neo_agent_core::workflow::WorkflowPhase {
                    id: "work".to_owned(),
                    description: "Do the work".to_owned(),
                }],
                lua_source: "neo.phase('work')\nreturn {}".to_owned(),
                input_schema: Some(json!({
                    "type": "object",
                    "properties": { "target": { "type": "integer" } },
                    "required": ["target"]
                })),
                output_schema: json!({"type": "object"})meta
,
            },
            false,
        )
        .expect("seed saved definition");
    let harness = harness_for_calls(&[(
        "run",
        json!({"action": "run_saved", "name": "typed-args", "args": {"target": "nope"}}),
    )]);
    let approval_calls = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&approval_calls);
    let config = config_for(&harness, session.path(), PermissionMode::Ask)
        .with_workflow_definitions(registry)
        .with_approval_handler(move |_| {
            calls.fetch_add(1, Ordering::AcqRel);
            panic!("invalid args must not prompt")
        });
    let (events, config) = run(&harness, config).await;
    assert_eq!(approval_calls.load(Ordering::Acquire), 0);
    let result = &workflow_results(&events)[0];
    assert!(result.is_error);
    let details = result.details.as_ref().expect("structured details");
    assert_eq!(details["error"]["code"], "workflow_input_invalid");
    assert_eq!(details["error"]["side_effect_occurred"], false);
    assert!(config.background_tasks.list(false, 10).await.is_empty());
}

// --- centralized launch coordination (no capability state) ---

fn base_launch_request(
    name: &str,
    launch_source: &str,
) -> neo_agent_core::workflow::WorkflowLaunchRequest {
    neo_agent_core::workflow::WorkflowLaunchRequest {
        name: name.to_owned(),
        description: format!("{name} workflow"),
        phases: vec![neo_agent_core::workflow::WorkflowPhase {
            id: "work".to_owned(),
            description: "Do the work".to_owned(),
        }],
        script: "neo.phase('work')".to_owned(),
        args: json!({"target": name}),
        launch_source: launch_source.to_owned(),
        parent_run_id: None,
        output_schema: None,
launch
    }
}

fn intent_for(
    request: neo_agent_core::workflow::WorkflowLaunchRequest,
    session: &std::path::Path,
    workspace: &std::path::Path,
    actor: neo_agent_core::workflow::WorkflowActor,
    mode: PermissionMode,
) -> neo_agent_core::workflow::WorkflowLaunchIntent {
    neo_agent_core::workflow::WorkflowLaunchIntent::from_parts(
        request,
        neo_agent_core::workflow::WorkflowLaunchBinding {
            session_identity: session.display().to_string(),
            workspace_identity: workspace.display().to_string(),
            actor,
            permission_mode: mode,
            parent_lineage: None,
            compiled_input_schema: None,
            schema_sha256: String::new(),
        },
    )
}

/// All launch adapters (model Workflow tool, named slash, headless CLI) must
/// call the single stateless coordinator — never a private create/register/
/// start path — and none of them needs capability state.
#[tokio::test]
async fn all_launch_adapters_reach_one_coordinator() {
    use neo_agent_core::workflow::{
        WorkflowActor, WorkflowLaunchCoordinator, WorkflowLaunchHosts, WorkflowRuntime,
    };

    let session = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::default();
    runtime
        .bind_runner(|_handle, _metadata, _session_dir| async move { Ok(()) })
        .unwrap();
    let background_tasks = neo_agent_core::BackgroundTaskManager::new();

    // Adapter 1: model Workflow tool path.
    let dynamic_intent = intent_for(
        base_launch_request("dynamic", "model:Workflow(run_inline)"),
        session.path(),
        workspace.path(),
        WorkflowActor::Model,
        PermissionMode::Auto,
    );
    let dynamic = WorkflowLaunchCoordinator
        .launch(
            &dynamic_intent,
            WorkflowLaunchHosts {
                runtime: &runtime,
                background_tasks: &background_tasks,
                session_dir: session.path(),
            },
        )
        .await
        .expect("model adapter");

    // Adapter 2: named slash path (human actor).
    let named_intent = intent_for(
        base_launch_request("named", "named:demo"),
        session.path(),
        workspace.path(),
        WorkflowActor::Human,
        PermissionMode::Yolo,
    );
    let named = WorkflowLaunchCoordinator
        .launch(
            &named_intent,
            WorkflowLaunchHosts {
                runtime: &runtime,
                background_tasks: &background_tasks,
                session_dir: session.path(),
            },
        )
        .await
        .expect("named adapter");

    // Adapter 3: headless CLI path.
    let headless_intent = intent_for(
        base_launch_request("headless", "headless:neo workflow run"),
        session.path(),
        workspace.path(),
        WorkflowActor::Human,
        PermissionMode::Auto,
    );
    let headless = WorkflowLaunchCoordinator
        .launch(
            &headless_intent,
            WorkflowLaunchHosts {
                runtime: &runtime,
                background_tasks: &background_tasks,
                session_dir: session.path(),
            },
        )
        .await
        .expect("headless adapter");

    // Three independent launches share no one-shot authorization state.
    let tasks = background_tasks.list(false, 10).await;
    assert_eq!(tasks.len(), 3);
    for outcome in [&dynamic, &named, &headless] {
        assert!(
            background_tasks
                .workflow_handle(&outcome.task_id)
                .await
                .is_some(),
            "adapter outcome must be registered via coordinator"
        );
    }
}

/// A tampered intent hash fails preflight with zero durable create; a valid
/// intent still launches afterwards.
#[tokio::test]
async fn invalid_preflight_creates_no_run() {
    use neo_agent_core::workflow::{
        WorkflowActor, WorkflowErrorCode, WorkflowLaunchCoordinator, WorkflowLaunchHosts,
        WorkflowRuntime,
    };

    let session = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::default();
    runtime
        .bind_runner(|_handle, _metadata, _session_dir| async move { Ok(()) })
        .unwrap();
    let background_tasks = neo_agent_core::BackgroundTaskManager::new();

    let mut intent = intent_for(
        base_launch_request("tampered", "test"),
        session.path(),
        workspace.path(),
        WorkflowActor::Model,
        PermissionMode::Auto,
    );
    intent.source_sha256 = "0000".to_owned();
    let err = WorkflowLaunchCoordinator
        .launch(
            &intent,
            WorkflowLaunchHosts {
                runtime: &runtime,
                background_tasks: &background_tasks,
                session_dir: session.path(),
            },
        )
        .await
        .expect_err("tampered source hash must fail preflight");
    assert_eq!(err.code(), WorkflowErrorCode::InvalidInput);
    assert!(background_tasks.list(false, 10).await.is_empty());
    assert!(
        runtime
            .rehydrate(session.path())
            .await
            .expect("rehydrate")
            .is_empty(),
        "preflight failure must create no durable run"
    );

    let valid = intent_for(
        base_launch_request("valid", "test"),
        session.path(),
        workspace.path(),
        WorkflowActor::Model,
        PermissionMode::Auto,
    );
    let outcome = WorkflowLaunchCoordinator
        .launch(
            &valid,
            WorkflowLaunchHosts {
                runtime: &runtime,
                background_tasks: &background_tasks,
                session_dir: session.path(),
            },
        )
        .await
        .expect("valid intent launches");
    assert!(
        background_tasks
            .workflow_handle(&outcome.task_id)
            .await
            .is_some()
    );
}

/// Compile, schema, and storage failures create no run and leave no task.
#[tokio::test]
async fn compile_schema_and_storage_failures_create_no_run() {
    use neo_agent_core::workflow::{
        CompiledSchema, WorkflowActor, WorkflowErrorCode, WorkflowLaunchCoordinator,
        WorkflowLaunchHosts, WorkflowLimits, WorkflowRuntime,
    };

    let session = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let background_tasks = neo_agent_core::BackgroundTaskManager::new();

    // 1) Lua compile failure.
    {
        let runtime = WorkflowRuntime::default();
        let mut request = base_launch_request("compile-fail", "test");
        request.script = "function (".to_owned();
        let intent = intent_for(
            request,
            session.path(),
            workspace.path(),
            WorkflowActor::Model,
            PermissionMode::Auto,
        );
        let err = WorkflowLaunchCoordinator
            .launch(
                &intent,
                WorkflowLaunchHosts {
                    runtime: &runtime,
                    background_tasks: &background_tasks,
                    session_dir: session.path(),
                },
            )
            .await
            .expect_err("compile failure");
        assert_eq!(err.code(), WorkflowErrorCode::LuaCompileFailed);
        assert!(background_tasks.list(false, 10).await.is_empty());
    }

    // 2) Input schema validation failure.
    {
        let runtime = WorkflowRuntime::default();
        let schema = CompiledSchema::compile(&json!({
            "type": "object",
            "properties": { "target": { "type": "integer" } },
            "required": ["target"],
            "additionalProperties": false
        }))
        .expect("compile schema");
        let mut request = base_launch_request("schema-fail", "test");
        request.args = json!({"target": "not-an-integer"});
        let intent = neo_agent_core::workflow::WorkflowLaunchIntent::from_parts(
            request,
            neo_agent_core::workflow::WorkflowLaunchBinding {
                session_identity: session.path().display().to_string(),
                workspace_identity: workspace.path().display().to_string(),
                actor: WorkflowActor::Model,
                permission_mode: PermissionMode::Auto,
                parent_lineage: None,
                compiled_input_schema: Some(schema),
                schema_sha256: "schema-binding".to_owned(),
            },
        );
        let err = WorkflowLaunchCoordinator
            .launch(
                &intent,
                WorkflowLaunchHosts {
                    runtime: &runtime,
                    background_tasks: &background_tasks,
                    session_dir: session.path(),
                },
            )
            .await
            .expect_err("schema failure");
        assert_eq!(err.code(), WorkflowErrorCode::InvalidInput);
        assert!(background_tasks.list(false, 10).await.is_empty());
    }

    // 3) Storage admission denial during durable create.
    {
        let limits = WorkflowLimits {
            global_storage_bytes: 1,
            ..WorkflowLimits::default()
        };
        let runtime = WorkflowRuntime::new(limits);
        runtime
            .bind_runner(|_handle, _metadata, _session_dir| async move { Ok(()) })
            .unwrap();
        let intent = intent_for(
            base_launch_request("storage-fail", "test"),
            session.path(),
            workspace.path(),
            WorkflowActor::Model,
            PermissionMode::Auto,
        );
        let err = WorkflowLaunchCoordinator
            .launch(
                &intent,
                WorkflowLaunchHosts {
                    runtime: &runtime,
                    background_tasks: &background_tasks,
                    session_dir: session.path(),
                },
            )
            .await
            .expect_err("storage failure");
        assert_eq!(err.code(), WorkflowErrorCode::StorageAdmissionDenied);
        assert!(background_tasks.list(false, 10).await.is_empty());
    }
}
