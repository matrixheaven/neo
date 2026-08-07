use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use neo_agent_core::harness::FakeHarness;
use neo_agent_core::instructions::{InstructionRegistry, InstructionRegistryConfig};
use neo_agent_core::runtime::{
    WorkflowDispatchEventDrainLease, WorkflowDispatchEventLease, WorkflowDispatchHandle,
};
use neo_agent_core::tools::{
    ProcessSupervisor, Tool, ToolContext, ToolFuture, ToolRegistry, ToolResult,
};
use neo_agent_core::workflow::journal::{JournalPayload, collect_journal};
use neo_agent_core::workflow::{
    WorkflowActor, WorkflowInvocationContext, WorkflowInvocationKind, WorkflowInvocationOutcome,
    WorkflowLaunchRequest, WorkflowLimits, WorkflowOutcomeStatus, WorkflowRuntime, WorkflowState,
    journal_path,
};
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, ApprovalAction, ApprovalCancelReason,
    ApprovalPresentation, ApprovalRequest, ApprovalResponse, PermissionMode,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

pub(crate) fn invocation(id: &str) -> WorkflowInvocationContext {
    WorkflowInvocationContext {
        invocation_id: id.to_owned(),
        cancel_token: CancellationToken::new(),
    }
}

pub(crate) fn handle(
    config: AgentConfig,
    harness: &FakeHarness,
    registry: Arc<ToolRegistry>,
    context: AgentContext,
) -> WorkflowDispatchHandle {
    WorkflowDispatchHandle {
        config,
        model_client: harness.client(),
        registry,
        process_supervisor: ProcessSupervisor::default(),
        context,
    }
}

pub(crate) fn capture_events(
    handle: &WorkflowDispatchHandle,
) -> (
    Arc<Mutex<Vec<AgentEvent>>>,
    WorkflowDispatchEventLease,
    WorkflowDispatchEventDrainLease,
) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&events);
    let resolver = handle.resolver().expect("resolver");
    let (lease, drain_lease) = resolver
        .lease_event_route(
            handle.config.session_directory.as_deref(),
            0,
            Arc::new(move |event| {
                captured.lock().expect("events").push(event);
            }),
        )
        .expect("event handler");
    (events, lease, drain_lease)
}

#[tokio::test]
async fn verify_command_uses_canonical_bash_permission_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = dir.path().canonicalize().expect("workspace");
    let harness = FakeHarness::from_turns([]);
    let requested = Arc::new(AtomicBool::new(false));
    let saw_request = Arc::clone(&requested);
    let expected_cwd = workspace.clone();
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(&workspace)
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Ask)
        .with_approval_handler(move |request| {
            match &request.presentation {
                ApprovalPresentation::Command { command, cwd, .. } => {
                    assert_eq!(command, "sudo --version");
                    assert_eq!(cwd.as_ref(), Some(&expected_cwd));
                }
                other => panic!("expected command approval, got {other:?}"),
            }
            saw_request.store(true, Ordering::SeqCst);
            ApprovalResponse::Selected {
                request_id: request.id.clone(),
                action: ApprovalAction::Reject,
                feedback: None,
            }
        });
    let handle = handle(
        config,
        &harness,
        Arc::new(ToolRegistry::with_builtin_tools()),
        AgentContext::new(),
    );
    let (events, _event_lease, _event_drain_lease) = capture_events(&handle);

    let outcome = handle
        .run_one(
            invocation("inv_exact_bash"),
            "Bash",
            json!({
                "command": "sudo --version",
                "cwd": workspace,
            }),
        )
        .await;

    assert_eq!(outcome.status, WorkflowOutcomeStatus::Denied);
    assert_eq!(outcome.details["kind"], "permission");
    assert_eq!(outcome.details["decision"], "denied");
    assert_eq!(outcome.details["side_effect_occurred"], false);
    assert!(requested.load(Ordering::SeqCst));
    let events = events.lock().expect("events");
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished { id, .. } if id == "inv_exact_bash"
    )));
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionStarted { id, .. }
            | AgentEvent::ShellCommandStarted { id, .. }
            if id == "inv_exact_bash"
    )));
}

/// Permission terminal decisions map to typed workflow outcomes on the shared
/// `tool_result_to_outcome` branch: a cancelled approval and an unanswered
/// required approval differ only by decision input, never by mapping.
#[tokio::test]
async fn permission_decisions_map_to_typed_workflow_outcomes() {
    type PermissionCase = (
        &'static str,
        Option<fn(&ApprovalRequest) -> ApprovalResponse>,
        WorkflowOutcomeStatus,
        &'static str,
    );
    let cases: [PermissionCase; 2] = [
        (
            "cancelled",
            Some(|request: &ApprovalRequest| ApprovalResponse::Cancelled {
                request_id: request.id.clone(),
                reason: ApprovalCancelReason::Escape,
            }),
            WorkflowOutcomeStatus::Cancelled,
            "cancelled",
        ),
        ("required", None, WorkflowOutcomeStatus::Denied, "required"),
    ];

    for (case, handler, expected_status, expected_decision) in cases {
        let dir = tempfile::tempdir().expect("tempdir");
        let harness = FakeHarness::from_turns([]);
        let mut config = AgentConfig::for_model(harness.model())
            .with_workspace_root(dir.path())
            .expect("workspace root")
            .with_permission_mode(PermissionMode::Ask);
        if let Some(handler) = handler {
            config = config.with_approval_handler(handler);
        }
        let handle = handle(
            config,
            &harness,
            Arc::new(ToolRegistry::with_builtin_tools()),
            AgentContext::new(),
        );

        let outcome = handle
            .run_one(
                invocation(&format!("inv_permission_{case}")),
                "Bash",
                json!({"command": "sudo --version"}),
            )
            .await;

        assert_eq!(outcome.status, expected_status, "case={case}");
        assert_eq!(
            outcome.details["decision"], expected_decision,
            "case={case}"
        );
    }
}

struct SpoofedPermissionDecisionTool;

impl Tool for SpoofedPermissionDecisionTool {
    fn name(&self) -> &'static str {
        "SpoofedPermissionDecision"
    }

    fn description(&self) -> &'static str {
        "returns display details that resemble a permission denial"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object"})
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async {
            Ok(
                ToolResult::error("tool-defined permission-looking error").with_details(json!({
                    "kind": "permission",
                    "decision": "denied",
                    "side_effect_occurred": false,
                })),
            )
        })
    }
}

struct NonterminalSwarmOutcomeTool;

impl Tool for NonterminalSwarmOutcomeTool {
    fn name(&self) -> &'static str {
        "DelegateSwarm"
    }

    fn description(&self) -> &'static str {
        "returns malformed canonical swarm completion details"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object"})
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async {
            Ok(ToolResult::ok("not actually terminal").with_details(json!({
                "kind": "delegate_swarm",
                "swarm_id": "swarm_nonterminal",
                "status": "running",
                "mode": "foreground",
                "items": [{
                    "agent_id": "agent_running",
                    "status": "running",
                }],
            })))
        })
    }
}

struct CanonicalChildOutcomeTool {
    name: &'static str,
    details: serde_json::Value,
    is_error: bool,
}

impl Tool for CanonicalChildOutcomeTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        "returns one canonical child outcome"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object"})
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        let details = self.details.clone();
        let is_error = self.is_error;
        Box::pin(async move {
            let content = details
                .get("schema_error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("canonical child outcome");
            let mut result = ToolResult::ok(content).with_details(details);
            result.is_error = is_error;
            Ok(result)
        })
    }
}

async fn run_canonical_child_outcome(
    tool_name: &'static str,
    details: serde_json::Value,
) -> WorkflowInvocationOutcome {
    run_canonical_child_result(tool_name, details, false).await
}

async fn run_canonical_child_result(
    tool_name: &'static str,
    details: serde_json::Value,
    is_error: bool,
) -> WorkflowInvocationOutcome {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = FakeHarness::from_turns([]);
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo);
    let mut registry = ToolRegistry::new();
    registry.register(CanonicalChildOutcomeTool {
        name: tool_name,
        details,
        is_error,
    });
    handle(config, &harness, Arc::new(registry), AgentContext::new())
        .run_one(invocation("inv_canonical_child"), tool_name, json!({}))
        .await
}

#[tokio::test]
async fn failed_delegate_maps_to_failed_workflow_outcome_and_preserves_correlation() {
    let outcome = run_canonical_child_outcome(
        "Delegate",
        json!({
            "kind": "delegate",
            "agent_id": "agent_failed",
            "status": "failed",
            "mode": "foreground",
            "task_id": "task_failed",
            "actual_usage": {
                "input_tokens": 11,
                "output_tokens": 7,
            },
        }),
    )
    .await;

    assert_eq!(outcome.status, WorkflowOutcomeStatus::Failed);
    assert!(!outcome.is_completed());
    assert_eq!(outcome.actual_usage.expect("usage").input_tokens, 11);
    assert_eq!(outcome.child_refs.len(), 2);
    assert_eq!(outcome.child_refs[0].id, "agent_failed");
    assert_eq!(outcome.child_refs[1].id, "task_failed");
}

#[tokio::test]
async fn cancelled_delegate_maps_to_cancelled_workflow_outcome_and_preserves_correlation() {
    let outcome = run_canonical_child_outcome(
        "Delegate",
        json!({
            "kind": "delegate",
            "agent_id": "agent_cancelled",
            "status": "cancelled",
            "mode": "foreground",
            "actual_usage": {
                "input_tokens": 5,
                "output_tokens": 3,
            },
        }),
    )
    .await;

    assert_eq!(outcome.status, WorkflowOutcomeStatus::Cancelled);
    assert!(!outcome.is_completed());
    assert_eq!(outcome.actual_usage.expect("usage").output_tokens, 3);
    assert_eq!(outcome.child_refs.len(), 1);
    assert_eq!(outcome.child_refs[0].id, "agent_cancelled");
}

#[tokio::test]
async fn interrupted_delegate_has_typed_status_and_explicit_reason() {
    let outcome = run_canonical_child_outcome(
        "Delegate",
        json!({
            "kind": "delegate",
            "agent_id": "agent_interrupted",
            "status": "interrupted",
            "mode": "foreground",
        }),
    )
    .await;

    assert_eq!(outcome.status, WorkflowOutcomeStatus::Interrupted);
    assert_eq!(outcome.details["reason"], "child_interrupted");
    assert_eq!(outcome.child_refs[0].id, "agent_interrupted");
}

#[tokio::test]
async fn background_or_running_delegate_fails_closed() {
    for details in [
        json!({
            "kind": "delegate",
            "agent_id": "agent_background",
            "status": "completed",
            "mode": "background",
        }),
        json!({
            "kind": "delegate",
            "agent_id": "agent_running",
            "status": "running",
            "mode": "foreground",
        }),
    ] {
        let outcome = run_canonical_child_outcome("Delegate", details).await;
        assert_eq!(outcome.status, WorkflowOutcomeStatus::Failed);
        assert!(outcome.summary.contains("nonterminal"));
        assert!(outcome.actual_usage.is_none());
        assert!(outcome.child_refs.is_empty());
    }
}

#[tokio::test]
async fn failed_mixed_swarm_preserves_usage_and_all_child_refs() {
    let outcome = run_canonical_child_outcome(
        "DelegateSwarm",
        json!({
            "kind": "delegate_swarm",
            "swarm_id": "swarm_failed",
            "status": "failed",
            "mode": "foreground",
            "task_id": "task_swarm",
            "items": [
                {"agent_id": "agent_completed", "status": "completed"},
                {"agent_id": "agent_failed", "status": "failed"},
            ],
            "actual_usage": {
                "input_tokens": 19,
                "output_tokens": 13,
            },
        }),
    )
    .await;

    assert_eq!(outcome.status, WorkflowOutcomeStatus::Failed);
    assert_eq!(outcome.actual_usage.expect("usage").output_tokens, 13);
    assert_eq!(outcome.child_refs.len(), 4);
    assert_eq!(outcome.child_refs[0].id, "swarm_failed");
    assert_eq!(outcome.child_refs[1].id, "agent_completed");
    assert_eq!(outcome.child_refs[2].id, "agent_failed");
    assert_eq!(outcome.child_refs[3].id, "task_swarm");
}

#[tokio::test]
async fn malformed_delegate_status_fails_closed() {
    let outcome = run_canonical_child_outcome(
        "Delegate",
        json!({
            "kind": "delegate",
            "agent_id": "agent_future",
            "status": "future_state",
            "mode": "foreground",
        }),
    )
    .await;

    assert_eq!(outcome.status, WorkflowOutcomeStatus::Failed);
    assert!(
        outcome
            .summary
            .contains("invalid canonical Delegate outcome details")
    );
    assert!(outcome.actual_usage.is_none());
    assert!(outcome.child_refs.is_empty());
}

#[tokio::test]
async fn non_child_tool_cannot_spoof_canonical_child_outcome() {
    let outcome = run_canonical_child_outcome(
        "CanonicalChildOutcome",
        json!({
            "kind": "delegate",
            "agent_id": "spoofed-agent",
            "status": "completed",
            "mode": "foreground",
            "actual_usage": {"input_tokens": 99, "output_tokens": 99},
        }),
    )
    .await;

    assert_eq!(outcome.status, WorkflowOutcomeStatus::Failed);
    assert!(outcome.summary.contains("cannot report kind delegate"));
    assert!(outcome.actual_usage.is_none());
    assert!(outcome.child_refs.is_empty());
}

#[tokio::test]
async fn expected_child_tool_rejects_missing_or_mismatched_kind() {
    for (tool_name, details) in [
        ("Delegate", json!({})),
        ("Delegate", json!({"kind": "delegate_swarm"})),
        ("DelegateSwarm", json!({"kind": "delegate"})),
    ] {
        let outcome = run_canonical_child_outcome(tool_name, details).await;
        assert_eq!(outcome.status, WorkflowOutcomeStatus::Failed);
        assert!(outcome.summary.contains("expected kind"));
        assert!(outcome.actual_usage.is_none());
        assert!(outcome.child_refs.is_empty());
    }
}

#[tokio::test]
async fn completed_delegate_with_schema_error_maps_to_failed_and_preserves_correlation() {
    let outcome = run_canonical_child_result(
        "Delegate",
        json!({
            "kind": "delegate",
            "agent_id": "contradictory-agent",
            "status": "completed",
            "mode": "foreground",
            "schema_error_code": "schema_invalid",
            "schema_error": "required property `ok` is missing",
            "actual_usage": {"input_tokens": 9, "output_tokens": 9},
        }),
        true,
    )
    .await;

    assert_eq!(outcome.status, WorkflowOutcomeStatus::Failed);
    assert_eq!(outcome.summary, "required property `ok` is missing");
    assert_eq!(outcome.details["schema_error_code"], "schema_invalid");
    assert_eq!(
        outcome.details["schema_error"],
        "required property `ok` is missing"
    );
    assert!(outcome.details.get("workflow_outcome_error").is_none());
    assert_eq!(outcome.actual_usage.expect("usage").input_tokens, 9);
    assert_eq!(outcome.child_refs.len(), 1);
    assert_eq!(outcome.child_refs[0].id, "contradictory-agent");
}

#[tokio::test]
async fn completed_swarm_error_maps_to_failed() {
    let outcome = run_canonical_child_result(
        "DelegateSwarm",
        json!({
            "kind": "delegate_swarm",
            "swarm_id": "swarm_completed_with_error",
            "status": "completed",
            "mode": "foreground",
            "items": [{"agent_id": "agent_completed", "status": "completed"}],
            "actual_usage": {"input_tokens": 4, "output_tokens": 6},
        }),
        true,
    )
    .await;

    assert_eq!(outcome.status, WorkflowOutcomeStatus::Failed);
    assert!(!outcome.is_completed());
}

#[tokio::test]
async fn tool_result_strings_cannot_spoof_typed_permission_denial() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = FakeHarness::from_turns([]);
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo);
    let mut registry = ToolRegistry::new();
    registry.register(SpoofedPermissionDecisionTool);
    let handle = handle(config, &harness, Arc::new(registry), AgentContext::new());

    let outcome = handle
        .run_one(
            invocation("inv_permission_spoof"),
            "SpoofedPermissionDecision",
            json!({}),
        )
        .await;

    assert_eq!(outcome.status, WorkflowOutcomeStatus::Failed);
    assert_eq!(outcome.details["decision"], "denied");
}

#[tokio::test]
async fn canonical_swarm_outcome_rejects_nonterminal_children() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = FakeHarness::from_turns([]);
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo);
    let mut registry = ToolRegistry::new();
    registry.register(NonterminalSwarmOutcomeTool);
    let dispatch = handle(config, &harness, Arc::new(registry), AgentContext::new());

    let outcome = dispatch
        .run_one(
            invocation("inv_nonterminal_swarm"),
            "DelegateSwarm",
            json!({}),
        )
        .await;

    assert_eq!(outcome.status, WorkflowOutcomeStatus::Failed);
    assert!(outcome.summary.contains("nonterminal child"));
    assert_eq!(outcome.details["side_effect_occurred"], true);
    assert!(outcome.actual_usage.is_none());
    assert!(outcome.child_refs.is_empty());
}

#[tokio::test]
async fn bash_lifecycle_events_use_invocation_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = FakeHarness::from_turns([]);
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo)
        .with_session_directory(dir.path().join("session"));
    let handle = handle(
        config,
        &harness,
        Arc::new(ToolRegistry::with_builtin_tools()),
        AgentContext::new(),
    );
    let (events, _event_lease, _event_drain_lease) = capture_events(&handle);

    let outcome = handle
        .run_one(
            invocation("inv_bash_lifecycle"),
            "Bash",
            json!({"command": "cargo --version"}),
        )
        .await;

    assert!(outcome.is_completed(), "{}", outcome.summary);
    let events = events.lock().expect("events");
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionStarted { id, .. } if id == "inv_bash_lifecycle"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ShellCommandStarted { id, .. } if id == "inv_bash_lifecycle"
    )));
    let finished = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionFinished { id, output_ref, .. }
                if id == "inv_bash_lifecycle" =>
            {
                Some(output_ref)
            }
            _ => None,
        })
        .expect("finished event");
    // Workflow direct tools run under the main agent reference: the finished
    // event carries the typed artifact, complete with final metadata.
    let reference = finished
        .as_ref()
        .expect("workflow direct bash must carry a captured output reference");
    assert_eq!(reference.agent_id, neo_agent_core::session::MAIN_AGENT_ID);
    assert!(reference.complete, "{reference:?}");
    assert!(reference.byte_len > 0, "{reference:?}");
    assert!(reference.line_count > 0, "{reference:?}");
    // The artifact exists on disk with the recorded length.
    let log = dir
        .path()
        .join("session")
        .join("agents")
        .join(&reference.agent_id)
        .join("tasks")
        .join(format!("{}.log", reference.task_id));
    assert_eq!(
        std::fs::metadata(&log).expect("artifact log").len(),
        reference.byte_len,
        "{}",
        log.display()
    );
}

#[tokio::test]
async fn instruction_replan_blocks_effect_without_model_turn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let nested = workspace.join("nested");
    std::fs::create_dir_all(&nested).expect("nested");
    std::fs::write(workspace.join("AGENTS.md"), "# newly applicable rules").expect("instructions");
    let workspace = workspace.canonicalize().expect("workspace");
    let nested = nested.canonicalize().expect("nested");
    let registry = Arc::new(
        InstructionRegistry::new(InstructionRegistryConfig {
            primary_workspace: workspace.clone(),
            neo_home: None,
            project_trusted: true,
        })
        .expect("instruction registry"),
    );
    let harness = FakeHarness::from_turns([]);
    let reached_authorization = Arc::new(AtomicUsize::new(0));
    let reached = Arc::clone(&reached_authorization);
    let mut config = AgentConfig::for_model(harness.model())
        .with_workspace_root(&workspace)
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo)
        .with_before_tool_call(move |_| {
            reached.fetch_add(1, Ordering::SeqCst);
            None
        });
    config.instruction_registry = Some(Arc::clone(&registry));
    let mut context = AgentContext::new();
    context.attach_instruction_registry(registry);
    let handle = handle(
        config,
        &harness,
        Arc::new(ToolRegistry::with_builtin_tools()),
        context,
    );
    let (events, _event_lease, _event_drain_lease) = capture_events(&handle);

    let workflow_runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let workflow = workflow_runtime
        .create_run(temp.path(), workflow_launch_request())
        .await
        .expect("workflow");
    workflow
        .enter_running_for_direct_execution()
        .await
        .expect("enter running");
    let dispatch = handle.clone();
    let canonical_input = json!({"command": "echo must-not-run", "cwd": nested});
    let tool_input = canonical_input.clone();
    let outcome = workflow
        .invoke(
            0,
            WorkflowInvocationKind::VerifyCommand,
            canonical_input,
            false,
            move |invocation| async move { dispatch.run_one(invocation, "Bash", tool_input).await },
        )
        .await
        .expect("workflow invocation");

    assert_eq!(outcome.status, WorkflowOutcomeStatus::Interrupted);
    assert_eq!(outcome.details["reason"], "instruction_replan_required");
    assert_eq!(outcome.details["side_effect_occurred"], false);
    assert_eq!(reached_authorization.load(Ordering::SeqCst), 0);
    assert!(harness.requests().is_empty(), "must not open a model turn");
    let snapshot = workflow.snapshot().await;
    assert_eq!(snapshot.state, WorkflowState::Paused);
    assert_eq!(
        snapshot.terminal_reason.as_deref(),
        Some("instruction_replan_required")
    );
    let envelopes = collect_journal(
        &journal_path(temp.path(), &workflow.run_id),
        Some(&workflow.run_id),
        WorkflowLimits::default().journal_record_bytes,
        WorkflowLimits::default().journal_total_bytes,
    )
    .expect("journal");
    let invocation_id = envelopes
        .iter()
        .find_map(|record| match &record.payload {
            JournalPayload::InvocationStarted { invocation_id, .. } => Some(invocation_id.clone()),
            _ => None,
        })
        .expect("journaled invocation id");
    assert!(envelopes.iter().any(|record| matches!(
        &record.payload,
        JournalPayload::StateChanged {
            new: WorkflowState::Paused,
            reason,
            actor: WorkflowActor::Runtime,
            ..
        } if reason == "instruction_replan_required"
    )));
    let live_context = handle
        .resolver()
        .expect("resolver")
        .resolve()
        .expect("snapshot")
        .context;
    assert_eq!(
        live_context.instruction_state().visible_generation,
        outcome.details["instruction_generation"]
            .as_u64()
            .expect("generation"),
        "the canonical epoch must update resolver-owned instruction authority",
    );
    let events = events.lock().expect("events");
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::InstructionEpoch { epoch }
            if epoch.deferred_tool_ids == [invocation_id.as_str()]
    )));
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionStarted { id, .. } if id == &invocation_id
    )));
}

pub(crate) fn workflow_launch_request() -> WorkflowLaunchRequest {
    WorkflowLaunchRequest {
        name: "dispatch-test".to_owned(),
        description: "dispatch-test".to_owned(),
        phases: Vec::new(),
        script: String::new(),
        args: json!({}),
        launch_source: "test".to_owned(),
        output_schema: None,
        display_name: None,
        input_schema: None,
        definition_origin: None,
        inline_unsaved: false,
    }
}
