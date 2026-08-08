//! Tool dispatch behavior (moved from `tool_dispatch.rs`).

use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use neo_ai::ModelClient;
use serde_json::json;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::{
    EventEmitter, PreparedExecution, ToolExecutionDeps, execute_tool_calls, prepare_edit_calls,
    prepare_tool_calls_for_execution, prepare_write_calls, routed_tool_event_callback,
    run_tool_with_cancel, skill_batch_isolation_violation, stamp_workflow_origin,
};
use crate::harness::fake_model;
use crate::runtime::config::{AgentConfig, ToolExecutionMode};
use crate::runtime::events::EventSink;
use crate::tools::{
    ShellAdmissionClass, ShellAdmissionRequest, ShellLimits, ShellRuntime, Tool, ToolContext,
    ToolError, ToolFuture, ToolRegistry,
};
use crate::{
    AgentContext, AgentEvent, AgentMessage, AgentToolCall, ApprovalAction, ApprovalResponse,
    PermissionMode, ProcessSupervisor,
};

fn workflow_origin(run_id: &str) -> crate::workflow::WorkflowExecutionOrigin {
    crate::workflow::WorkflowExecutionOrigin {
        run_id: crate::workflow::WorkflowId(run_id.into()),
        human_handle: None,
        definition_name: "workflow".into(),
        definition_revision: None,
        phase_id: Some("phase".into()),
        invocation_id: Some("invocation".into()),
        swarm_item_id: None,
    }
}

fn delegate_family_events() -> Vec<AgentEvent> {
    let runtime = crate::multi_agent::MultiAgentRuntime::new();
    let agent = runtime.start_foreground_delegate_for_test("task");
    let progress = agent.progress_snapshot();
    let swarm = crate::multi_agent::SwarmSnapshot {
        swarm_id: "swarm".into(),
        description: "task".into(),
        role: crate::multi_agent::AgentRole::Coder,
        mode: crate::multi_agent::AgentRunMode::Foreground,
        state: crate::multi_agent::AgentLifecycleState::Running,
        max_concurrency: 1,
        aggregate: crate::multi_agent::SwarmAggregate::default(),
        children: Vec::new(),
    };
    vec![
        AgentEvent::QuestionRequested {
            turn: 1,
            id: "question".into(),
            questions: Vec::new(),
            workflow_origin: None,
        },
        AgentEvent::DelegateStarted {
            turn: 1,
            agent: agent.clone(),
            workflow_origin: None,
        },
        AgentEvent::DelegateUpdated {
            turn: 1,
            agent: agent.clone(),
            workflow_origin: None,
        },
        AgentEvent::DelegateProgressUpdated {
            turn: 1,
            progress: progress.clone(),
            workflow_origin: None,
        },
        AgentEvent::DelegateFinished {
            turn: 1,
            agent,
            workflow_origin: None,
        },
        AgentEvent::DelegateSwarmStarted {
            turn: 1,
            swarm: swarm.clone(),
            workflow_origin: None,
        },
        AgentEvent::DelegateSwarmUpdated {
            turn: 1,
            swarm: swarm.clone(),
            workflow_origin: None,
        },
        AgentEvent::DelegateSwarmProgressUpdated {
            turn: 1,
            swarm_id: "swarm".into(),
            state: crate::multi_agent::AgentLifecycleState::Running,
            aggregate: crate::multi_agent::SwarmAggregate::default(),
            child_progress: crate::multi_agent::SwarmChildProgress {
                item_index: 0,
                progress,
            },
            workflow_origin: None,
        },
        AgentEvent::DelegateSwarmFinished {
            turn: 1,
            swarm,
            workflow_origin: None,
        },
    ]
}

#[tokio::test]
async fn routed_tool_event_callback_does_not_retain_active_sender() {
    let session = tempfile::tempdir().expect("session");
    let resolver = crate::runtime::WorkflowDispatchResolver::default();
    let idle_events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_idle = Arc::clone(&idle_events);
    let _idle_lease = resolver
        .lease_idle_event_route(
            Some(session.path()),
            Arc::new(move |event| captured_idle.lock().expect("idle events").push(event)),
        )
        .expect("idle route");
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let (producer_lease, drain_lease) = resolver
        .lease_event_route(
            Some(session.path()),
            1,
            crate::runtime::events::make_tool_event_callback(EventSink {
                sender: sender.clone(),
            }),
        )
        .expect("active route");
    let config = AgentConfig::for_model(fake_model())
        .with_session_directory(session.path())
        .with_workflow_dispatch_resolver(resolver);
    let callback = routed_tool_event_callback(
        &config,
        EventSink {
            sender: sender.clone(),
        },
    );

    drop(sender);
    drop(producer_lease);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), receiver.recv())
            .await
            .expect("active event receiver must close")
            .is_none(),
        "the routed callback must not retain the active sender"
    );

    callback(AgentEvent::RunStarted { turn: 2 });
    assert!(idle_events.lock().expect("idle events").is_empty());
    drop(drain_lease);
    assert_eq!(idle_events.lock().expect("idle events").len(), 1);
}

fn workflow_origin_of(event: &AgentEvent) -> Option<&crate::workflow::WorkflowExecutionOrigin> {
    match event {
        AgentEvent::QuestionRequested {
            workflow_origin, ..
        }
        | AgentEvent::DelegateStarted {
            workflow_origin, ..
        }
        | AgentEvent::DelegateUpdated {
            workflow_origin, ..
        }
        | AgentEvent::DelegateProgressUpdated {
            workflow_origin, ..
        }
        | AgentEvent::DelegateFinished {
            workflow_origin, ..
        }
        | AgentEvent::DelegateSwarmStarted {
            workflow_origin, ..
        }
        | AgentEvent::DelegateSwarmUpdated {
            workflow_origin, ..
        }
        | AgentEvent::DelegateSwarmProgressUpdated {
            workflow_origin, ..
        }
        | AgentEvent::DelegateSwarmFinished {
            workflow_origin, ..
        } => workflow_origin.as_ref(),
        _ => None,
    }
}

#[test]
fn stamp_workflow_origin_covers_tools_and_delegate_families() {
    let origin = workflow_origin("workflow-run");
    let tool = AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "bash-call".into(),
        name: "Bash".into(),
        arguments: serde_json::json!({"command": "true"}),
        workflow_origin: None,
        output_ref: None,
    };
    let stamped = stamp_workflow_origin(tool, Some(&origin));
    assert!(matches!(
        stamped,
        AgentEvent::ToolExecutionStarted {
            workflow_origin: Some(ref stamped_origin),
            ..
        } if stamped_origin == &origin
    ));

    for event in delegate_family_events() {
        let stamped = stamp_workflow_origin(event, Some(&origin));
        assert_eq!(workflow_origin_of(&stamped), Some(&origin));
    }

    let existing = workflow_origin("existing-run");
    let event = AgentEvent::DelegateStarted {
        turn: 1,
        agent: crate::multi_agent::MultiAgentRuntime::new()
            .start_foreground_delegate_for_test("task"),
        workflow_origin: Some(existing.clone()),
    };
    let stamped = stamp_workflow_origin(event, Some(&origin));
    assert_eq!(workflow_origin_of(&stamped), Some(&existing));
}

struct CancellationSettlingTerminal {
    entered: Arc<Notify>,
    settled: Arc<AtomicBool>,
}

struct CustomEdit;

impl Tool for CustomEdit {
    fn name(&self) -> &'static str {
        "Edit"
    }

    fn description(&self) -> &'static str {
        "custom edit"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async { Ok(crate::ToolResult::ok("custom edit ran")) })
    }
}

struct CustomWrite;

#[test]
fn skill_activation_stays_isolated_without_workflow_choreography() {
    let mixed = [
        AgentToolCall {
            id: "skill".into(),
            name: "Skill".into(),
            raw_arguments: r#"{"skill":"create-workflow"}"#.into(),
        },
        AgentToolCall {
            id: "bash".into(),
            name: "Bash".into(),
            raw_arguments: r#"{"command":"cargo test"}"#.into(),
        },
    ];
    let registry = ToolRegistry::with_builtin_tools();
    let mixed_prepared = prepare_tool_calls_for_execution(&mixed, &registry.specs());
    assert!(
        skill_batch_isolation_violation(&mixed_prepared),
        "Skill mixed in a batch must still be rejected"
    );

    // No transcript scanning or keyword-based route enforcement remains.
    // Workflow actions (run_inline, run_saved, save) execute as the first
    // and only business tool without any mandatory prerequisite.
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text(
        "全面测试我的 dynamic workflow 功能并深度评测",
    ));
    context.append_message(AgentMessage::tool_result(
        "skill",
        "Skill",
        [crate::Content::text(
            "<neo-skill-loaded name=\"create-workflow\" source=\"builtin\">",
        )],
        false,
    ));
    let bash_prepared = prepare_tool_calls_for_execution(&mixed[1..], &registry.specs());
    // No skill_state or workflow_evaluation_route rejection happens.
    // A single Bash call after a create-workflow activation is not blocked
    // by any choreography gate.
    assert!(
        !skill_batch_isolation_violation(&bash_prepared),
        "single non-skill call must not be blocked by skill isolation"
    );

    for (_prompt, action) in [
        ("全面测试已保存的 workflow release-check", "run_saved"),
        ("创建一个 workflow 并立即测试它", "save"),
        ("evaluate my workflow", "run_inline"),
        ("check my workflow without running", "validate_inline"),
    ] {
        let call = [AgentToolCall {
            id: "workflow".into(),
            name: "Workflow".into(),
            raw_arguments: format!(r#"{{"action":"{action}"}}"#).into(),
        }];
        let prepared = prepare_tool_calls_for_execution(&call, &registry.specs());
        assert!(
            !skill_batch_isolation_violation(&prepared),
            "single Workflow {action} call must not be blocked by skill isolation"
        );
    }

    // Skill as the only call in the batch is not a violation.
    let solo_skill = [AgentToolCall {
        id: "skill".into(),
        name: "Skill".into(),
        raw_arguments: r#"{"skill":"create-workflow"}"#.into(),
    }];
    let solo_prepared = prepare_tool_calls_for_execution(&solo_skill, &registry.specs());
    assert!(!skill_batch_isolation_violation(&solo_prepared));
}

impl Tool for CustomWrite {
    fn name(&self) -> &'static str {
        "Write"
    }

    fn description(&self) -> &'static str {
        "custom write"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async { Ok(crate::ToolResult::ok("custom write ran")) })
    }
}

impl Tool for CancellationSettlingTerminal {
    fn name(&self) -> &'static str {
        "Terminal"
    }

    fn description(&self) -> &'static str {
        "test terminal"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            self.entered.notify_one();
            ctx.cancel_token.cancelled().await;
            tokio::task::yield_now().await;
            self.settled.store(true, Ordering::SeqCst);
            Err(ToolError::Cancelled)
        })
    }
}

#[tokio::test]
async fn same_file_edit_batch_commits_once_and_returns_results_for_each_call() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("file.txt"), "one\ntwo\n").expect("seed file");
    let config = AgentConfig::for_model(fake_model())
        .with_workspace_root(workspace.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo)
        .with_tool_execution_mode(ToolExecutionMode::Parallel)
        .with_after_tool_call(|_, result| result.terminate());
    let model: Arc<dyn ModelClient> =
        Arc::new(neo_ai::providers::fake::FakeModelClient::new(Vec::new()));
    let registry = Arc::new(ToolRegistry::with_builtin_tools());
    let calls = [
        AgentToolCall {
            id: "edit-1".into(),
            name: "Edit".into(),
            raw_arguments: r#"{"path":"file.txt","old":"one","new":"ONE"}"#.into(),
        },
        AgentToolCall {
            id: "edit-2".into(),
            name: "Edit".into(),
            raw_arguments: r#"{"path":"file.txt","old":"two","new":"TWO"}"#.into(),
        },
    ];
    let cancel = CancellationToken::new();
    let supervisor = ProcessSupervisor::default();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut emitter = EventEmitter::new(tx, AgentContext::new());

    let outcome = execute_tool_calls(
        ToolExecutionDeps {
            config: &config,
            model,
            registry,
            skills: None,
            cancel_token: &cancel,
            process_supervisor: &supervisor,
        },
        1,
        &calls,
        &mut emitter,
    )
    .await
    .expect("tool dispatch");

    assert_eq!(outcome.results.len(), 2);
    assert_eq!(outcome.permission_decisions.len(), 2);
    assert!(!outcome.results[0].1.is_error);
    assert!(!outcome.results[1].1.is_error);
    assert!(outcome.results[0].1.terminate);
    assert!(outcome.results[1].1.terminate);
    assert_eq!(
        outcome.results[0]
            .1
            .details
            .as_ref()
            .expect("primary details")["status"],
        "committed"
    );
    assert_eq!(
        outcome.results[1]
            .1
            .details
            .as_ref()
            .expect("follower details")["status"],
        "coalesced"
    );
    assert_eq!(
        outcome.results[1]
            .1
            .details
            .as_ref()
            .expect("follower details")["primary_call_id"],
        "edit-1"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("file.txt")).expect("read result"),
        "ONE\nTWO\n"
    );

    let events = std::iter::from_fn(|| rx.try_recv().ok())
        .collect::<Result<Vec<_>, _>>()
        .expect("runtime events");
    let started = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::ToolExecutionStarted { name, .. } if name == "Edit"
            )
        })
        .count();
    let progress = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::ToolExecutionUpdate { partial_result, .. }
                    if partial_result
                        .details
                        .as_ref()
                        .and_then(|details| details.get("kind"))
                        .and_then(serde_json::Value::as_str)
                        == Some("edit_progress")
            )
        })
        .count();
    assert_eq!(started, 1);
    assert_eq!(progress, 1);
}

#[tokio::test]
async fn same_file_edit_batch_conflict_writes_nothing() {
    let workspace = tempfile::tempdir().expect("workspace");
    let path = workspace.path().join("file.txt");
    std::fs::write(&path, "before\n").expect("seed file");
    let config = AgentConfig::for_model(fake_model())
        .with_workspace_root(workspace.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo)
        .with_tool_execution_mode(ToolExecutionMode::Parallel);
    let model: Arc<dyn ModelClient> =
        Arc::new(neo_ai::providers::fake::FakeModelClient::new(Vec::new()));
    let registry = Arc::new(ToolRegistry::with_builtin_tools());
    let calls = [
        AgentToolCall {
            id: "edit-1".into(),
            name: "Edit".into(),
            raw_arguments: r#"{"path":"file.txt","old":"before","new":"after"}"#.into(),
        },
        AgentToolCall {
            id: "edit-2".into(),
            name: "Edit".into(),
            raw_arguments: r#"{"path":"file.txt","old":"before","new":"again"}"#.into(),
        },
    ];
    let cancel = CancellationToken::new();
    let supervisor = ProcessSupervisor::default();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut emitter = EventEmitter::new(tx, AgentContext::new());

    let outcome = execute_tool_calls(
        ToolExecutionDeps {
            config: &config,
            model,
            registry,
            skills: None,
            cancel_token: &cancel,
            process_supervisor: &supervisor,
        },
        1,
        &calls,
        &mut emitter,
    )
    .await
    .expect("tool dispatch");

    assert_eq!(outcome.results.len(), 2);
    assert!(outcome.results.iter().all(|(_, result)| result.is_error));
    assert!(outcome.results.iter().all(|(_, result)| {
        result
            .details
            .as_ref()
            .and_then(|details| details.get("status"))
            == Some(&json!("same_batch_conflict"))
    }));
    assert!(!outcome.executed_any);
    assert_eq!(
        std::fs::read_to_string(path).expect("read file"),
        "before\n"
    );

    let events = std::iter::from_fn(|| rx.try_recv().ok())
        .collect::<Result<Vec<_>, _>>()
        .expect("runtime events");
    assert!(!events.iter().any(|event| {
        matches!(event, AgentEvent::ToolExecutionStarted { name, .. } if name == "Edit")
    }));
}

#[tokio::test]
async fn noncanonical_edit_calls_stay_on_direct_registry_execution() {
    let workspace = tempfile::tempdir().expect("workspace");
    let context = ToolContext::new(workspace.path()).expect("context");
    let calls = [AgentToolCall {
        id: "edit".into(),
        name: "Edit".into(),
        raw_arguments: "{}".into(),
    }];

    let unregistered = ToolRegistry::new();
    let mut prepared = prepare_tool_calls_for_execution(&calls, &unregistered.specs());
    prepare_edit_calls(&context, &unregistered, &mut prepared);
    assert!(matches!(
        prepared[0].1.as_ref().expect("parsed").execution,
        PreparedExecution::Direct
    ));

    let mut custom = ToolRegistry::with_builtin_tools();
    custom.register(CustomEdit);
    let mut prepared = prepare_tool_calls_for_execution(&calls, &custom.specs());
    prepare_edit_calls(&context, &custom, &mut prepared);
    assert!(matches!(
        prepared[0].1.as_ref().expect("parsed").execution,
        PreparedExecution::Direct
    ));
    let result = custom
        .run("Edit", &context, json!({}))
        .await
        .expect("custom Edit");
    assert_eq!(result.content, "custom edit ran");
}

#[tokio::test]
async fn noncanonical_write_calls_stay_on_direct_registry_execution() {
    let workspace = tempfile::tempdir().expect("workspace");
    let context = ToolContext::new(workspace.path()).expect("context");
    let calls = [AgentToolCall {
        id: "write".into(),
        name: "Write".into(),
        raw_arguments: "{}".into(),
    }];

    let unregistered = ToolRegistry::new();
    let mut prepared = prepare_tool_calls_for_execution(&calls, &unregistered.specs());
    prepare_write_calls(&context, &unregistered, &mut prepared);
    assert!(matches!(
        prepared[0].1.as_ref().expect("parsed").execution,
        PreparedExecution::Direct
    ));

    let mut custom = ToolRegistry::with_builtin_tools();
    custom.register(CustomWrite);
    let mut prepared = prepare_tool_calls_for_execution(&calls, &custom.specs());
    prepare_write_calls(&context, &custom, &mut prepared);
    assert!(matches!(
        prepared[0].1.as_ref().expect("parsed").execution,
        PreparedExecution::Direct
    ));
    let result = custom
        .run("Write", &context, json!({}))
        .await
        .expect("custom Write");
    assert_eq!(result.content, "custom write ran");
}

#[tokio::test]
async fn terminal_start_cancellation_allows_internal_cleanup_to_settle() {
    let workspace = tempfile::tempdir().expect("workspace");
    let cancel = CancellationToken::new();
    let context = ToolContext::new(workspace.path())
        .expect("tool context")
        .with_cancel_token(cancel.clone());
    let entered = Arc::new(Notify::new());
    let settled = Arc::new(AtomicBool::new(false));
    let mut registry = ToolRegistry::new();
    registry.register(CancellationSettlingTerminal {
        entered: Arc::clone(&entered),
        settled: Arc::clone(&settled),
    });
    let call = AgentToolCall {
        id: "terminal-start".into(),
        name: "Terminal".into(),
        raw_arguments: r#"{"mode":"start"}"#.into(),
    };
    let arguments = json!({ "mode": "start" });

    let run = run_tool_with_cancel(None, &registry, &call, &arguments, &context, &cancel);
    tokio::pin!(run);
    tokio::select! {
        () = entered.notified() => {}
        result = &mut run => panic!("Terminal returned before cancellation: {result:?}"),
    }
    cancel.cancel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), run)
        .await
        .expect("Terminal cleanup should settle after cancellation");

    assert!(result.0.is_error);
    assert!(
        settled.load(Ordering::SeqCst),
        "runtime returned before Terminal cleanup settled"
    );
}

#[tokio::test]
async fn approved_bash_emits_queued_then_started_only_after_grant() {
    let workspace = tempfile::tempdir().expect("workspace");
    let runtime = ShellRuntime::new(
        ShellLimits {
            max_active_commands: 1,
            ..ShellLimits::default()
        },
        PathBuf::from("missing-guardian"),
        workspace.path().join("runtime"),
    );
    let held = runtime
        .acquire(
            ShellAdmissionRequest {
                owner: "hold".to_owned(),
                class: ShellAdmissionClass::AgentForeground,
            },
            None,
        )
        .await;
    let config = AgentConfig::for_model(fake_model())
        .with_workspace_root(workspace.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Ask)
        .with_approval_handler(|request| ApprovalResponse::Selected {
            request_id: request.id.clone(),
            action: ApprovalAction::PermitOnce,
            feedback: None,
        })
        .with_tool_execution_mode(ToolExecutionMode::Sequential)
        .with_shell_runtime(runtime);
    let model: Arc<dyn ModelClient> =
        Arc::new(neo_ai::providers::fake::FakeModelClient::new(Vec::new()));
    let registry = Arc::new(ToolRegistry::with_builtin_tools());
    let calls = [AgentToolCall {
        id: "call-1".into(),
        name: "Bash".into(),
        raw_arguments: r#"{"command":"printf ready"}"#.into(),
    }];
    let cancel = CancellationToken::new();
    let supervisor = ProcessSupervisor::default();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut emitter = EventEmitter::new(tx, AgentContext::new());
    let run = execute_tool_calls(
        ToolExecutionDeps {
            config: &config,
            model,
            registry,
            skills: None,
            cancel_token: &cancel,
            process_supervisor: &supervisor,
        },
        1,
        &calls,
        &mut emitter,
    );
    tokio::pin!(run);
    let mut approval_seen = false;
    loop {
        tokio::select! {
            event = rx.recv() => {
                let event = event.expect("event channel").expect("runtime event");
                if matches!(event, AgentEvent::ApprovalRequested { .. }) {
                    approval_seen = true;
                }
                if matches!(event, AgentEvent::ToolExecutionQueued { .. }) {
                    assert!(approval_seen, "Bash queued before approval completed");
                    break;
                }
                assert!(!matches!(event, AgentEvent::ToolExecutionStarted { .. }));
            }
            result = &mut run => panic!(
                "Bash returned before admission: ok={}",
                result.is_ok()
            ),
        }
    }
    while let Ok(Ok(event)) = rx.try_recv() {
        assert!(!matches!(event, AgentEvent::ToolExecutionStarted { .. }));
    }
    drop(held);
    let results = run.await.expect("tool dispatch");
    let events = std::iter::from_fn(|| rx.try_recv().ok())
        .collect::<Result<Vec<_>, _>>()
        .expect("runtime events");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::ToolExecutionStarted { .. }))
    );
    let result = &results.results[0].1;
    let model_visible = format!("{} {:?}", result.content, result.details);
    assert!(!model_visible.contains("position"));
    assert!(!model_visible.contains("waiting_ms"));
}

#[tokio::test]
async fn parallel_shell_batch_reaches_shared_admission_for_every_call() {
    let workspace = tempfile::tempdir().expect("workspace");
    let runtime = ShellRuntime::new(
        ShellLimits {
            max_active_commands: 1,
            ..ShellLimits::default()
        },
        PathBuf::from("missing-guardian"),
        workspace.path().join("runtime"),
    );
    let held = runtime
        .acquire(
            ShellAdmissionRequest {
                owner: "hold".to_owned(),
                class: ShellAdmissionClass::AgentForeground,
            },
            None,
        )
        .await;
    let config = AgentConfig::for_model(fake_model())
        .with_workspace_root(workspace.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo)
        .with_tool_execution_mode(ToolExecutionMode::Parallel)
        .with_shell_runtime(runtime);
    let model: Arc<dyn ModelClient> =
        Arc::new(neo_ai::providers::fake::FakeModelClient::new(Vec::new()));
    let registry = Arc::new(ToolRegistry::with_builtin_tools());
    let calls = [
        AgentToolCall {
            id: "call-1".into(),
            name: "Bash".into(),
            raw_arguments: r#"{"command":"printf one"}"#.into(),
        },
        AgentToolCall {
            id: "call-2".into(),
            name: "Bash".into(),
            raw_arguments: r#"{"command":"printf two"}"#.into(),
        },
        AgentToolCall {
            id: "call-3".into(),
            name: "Terminal".into(),
            raw_arguments: r#"{"mode":"start","command":"printf three"}"#.into(),
        },
    ];
    let cancel = CancellationToken::new();
    let supervisor = ProcessSupervisor::default();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut emitter = EventEmitter::new(tx, AgentContext::new());
    let run = execute_tool_calls(
        ToolExecutionDeps {
            config: &config,
            model,
            registry,
            skills: None,
            cancel_token: &cancel,
            process_supervisor: &supervisor,
        },
        1,
        &calls,
        &mut emitter,
    );
    tokio::pin!(run);
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(1));
    tokio::pin!(deadline);
    let mut queued_ids = Vec::new();
    while queued_ids.len() < calls.len() {
        tokio::select! {
            event = rx.recv() => {
                let event = event.expect("event channel").expect("runtime event");
                match event {
                    AgentEvent::ToolExecutionQueued { id, .. } => queued_ids.push(id),
                    AgentEvent::ToolExecutionStarted { id, .. } => {
                        panic!("shell call {id} started while capacity was held")
                    }
                    _ => {}
                }
            }
            result = &mut run => panic!(
                "shell batch returned before every call queued: ok={}",
                result.is_ok()
            ),
            () = &mut deadline => panic!(
                "only {} of {} shell calls reached admission",
                queued_ids.len(),
                calls.len()
            ),
        }
    }
    queued_ids.sort();
    assert_eq!(queued_ids, ["call-1", "call-2", "call-3"]);

    cancel.cancel();
    let results = run.await.expect("tool dispatch cancellation");
    assert_eq!(results.results.len(), calls.len());
    drop(held);
}
