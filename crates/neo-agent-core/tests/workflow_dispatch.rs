use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use neo_agent_core::harness::FakeHarness;
use neo_agent_core::instructions::{InstructionRegistry, InstructionRegistryConfig};
use neo_agent_core::runtime::{
    WorkflowDispatchEventDrainLease, WorkflowDispatchEventLease, WorkflowDispatchHandle,
    WorkflowDispatchSnapshot,
};
use neo_agent_core::tools::{
    ProcessSupervisor, Tool, ToolContext, ToolFuture, ToolRegistry, ToolResult,
};
use neo_agent_core::workflow::{
    JournalRecord, WorkflowActor, WorkflowInvocationContext, WorkflowInvocationKind,
    WorkflowInvocationOutcome, WorkflowLaunchRequest, WorkflowLimits, WorkflowOutcomeStatus,
    WorkflowRuntime, WorkflowState,
};
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, AgentTokenUsage,
    ApprovalAction, ApprovalCancelReason, ApprovalPresentation, ApprovalResponse, PermissionMode,
};
use neo_ai::{AiStreamEvent, StopReason};
use serde_json::json;
use tokio::sync::{Barrier, Notify};
use tokio_util::sync::CancellationToken;

fn invocation(id: &str) -> WorkflowInvocationContext {
    WorkflowInvocationContext {
        invocation_id: id.to_owned(),
        cancel_token: CancellationToken::new(),
    }
}

fn handle(
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

fn capture_events(
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

#[tokio::test]
async fn cancelled_permission_maps_to_cancelled_workflow_outcome() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = FakeHarness::from_turns([]);
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Ask)
        .with_approval_handler(|request| ApprovalResponse::Cancelled {
            request_id: request.id.clone(),
            reason: ApprovalCancelReason::Escape,
        });
    let handle = handle(
        config,
        &harness,
        Arc::new(ToolRegistry::with_builtin_tools()),
        AgentContext::new(),
    );

    let outcome = handle
        .run_one(
            invocation("inv_permission_cancelled"),
            "Bash",
            json!({"command": "sudo --version"}),
        )
        .await;

    assert_eq!(outcome.status, WorkflowOutcomeStatus::Cancelled);
    assert_eq!(outcome.details["decision"], "cancelled");
}

#[tokio::test]
async fn required_permission_maps_to_denied_workflow_outcome() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = FakeHarness::from_turns([]);
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Ask);
    let handle = handle(
        config,
        &harness,
        Arc::new(ToolRegistry::with_builtin_tools()),
        AgentContext::new(),
    );

    let outcome = handle
        .run_one(
            invocation("inv_permission_required"),
            "Bash",
            json!({"command": "sudo --version"}),
        )
        .await;

    assert_eq!(outcome.status, WorkflowOutcomeStatus::Denied);
    assert_eq!(outcome.details["decision"], "required");
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
            let mut result = ToolResult::ok("canonical child outcome").with_details(details);
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
    assert!(!outcome.ok);
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
    assert!(!outcome.ok);
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
async fn child_error_result_cannot_claim_completed_status() {
    let outcome = run_canonical_child_result(
        "Delegate",
        json!({
            "kind": "delegate",
            "agent_id": "contradictory-agent",
            "status": "completed",
            "mode": "foreground",
            "actual_usage": {"input_tokens": 9, "output_tokens": 9},
        }),
        true,
    )
    .await;

    assert_eq!(outcome.status, WorkflowOutcomeStatus::Failed);
    assert!(outcome.summary.contains("error result cannot be completed"));
    assert!(outcome.actual_usage.is_none());
    assert!(outcome.child_refs.is_empty());
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
        .with_permission_mode(PermissionMode::Yolo);
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

    assert!(outcome.ok, "{}", outcome.summary);
    let events = events.lock().expect("events");
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionStarted { id, .. } if id == "inv_bash_lifecycle"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ShellCommandStarted { id, .. } if id == "inv_bash_lifecycle"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished { id, .. } if id == "inv_bash_lifecycle"
    )));
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
    let output = workflow.output().await.expect("workflow output");
    let invocation_id = output
        .invocations
        .iter()
        .find_map(|record| match record {
            JournalRecord::InvocationStarted { invocation_id, .. } => Some(invocation_id.clone()),
            _ => None,
        })
        .expect("journaled invocation id");
    assert!(output.invocations.iter().any(|record| matches!(
        record,
        JournalRecord::StateChanged {
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

struct EchoTool(&'static str);

impl Tool for EchoTool {
    fn name(&self) -> &'static str {
        "WorkflowEcho"
    }

    fn description(&self) -> &'static str {
        "workflow resolver probe"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object"})
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        let value = self.0;
        Box::pin(async move { Ok(ToolResult::ok(value)) })
    }
}

#[tokio::test]
async fn each_run_one_resolves_current_live_registry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = FakeHarness::from_turns([]);
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo);
    let mut first = ToolRegistry::new();
    first.register(EchoTool("first"));
    let handle = handle(config, &harness, Arc::new(first), AgentContext::new());

    let first = handle
        .run_one(invocation("inv_first"), "WorkflowEcho", json!({}))
        .await;
    assert_eq!(first.summary, "first");

    let resolver = handle.resolver().expect("resolver");
    let mut snapshot: WorkflowDispatchSnapshot = resolver.resolve().expect("snapshot");
    let mut second = ToolRegistry::new();
    second.register(EchoTool("second"));
    snapshot.config.model.model = "second-live-model".to_owned();
    snapshot.registry = Arc::new(second);
    resolver.replace(snapshot).expect("replace snapshot");

    let second = handle
        .run_one(invocation("inv_second"), "WorkflowEcho", json!({}))
        .await;
    assert_eq!(second.summary, "second");
    assert_eq!(
        resolver
            .resolve()
            .expect("updated snapshot")
            .config
            .model
            .model,
        "second-live-model",
    );
}

#[tokio::test]
async fn workflow_handle_resolves_only_its_origin_session_snapshot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = FakeHarness::from_turns([]);
    let resolver = neo_agent_core::runtime::WorkflowDispatchResolver::default();
    let config_for = |session: &str| {
        AgentConfig::for_model(harness.model())
            .with_workspace_root(dir.path())
            .expect("workspace root")
            .with_session_directory(dir.path().join(session))
            .with_permission_mode(PermissionMode::Yolo)
            .with_workflow_dispatch_resolver(resolver.clone())
    };
    let mut registry_a = ToolRegistry::new();
    registry_a.register(EchoTool("session-a"));
    let handle_a = handle(
        config_for("session-a"),
        &harness,
        Arc::new(registry_a),
        AgentContext::new(),
    );
    handle_a.resolver().expect("bind session A");

    let mut registry_b = ToolRegistry::new();
    registry_b.register(EchoTool("session-b"));
    let handle_b = handle(
        config_for("session-b"),
        &harness,
        Arc::new(registry_b),
        AgentContext::new(),
    );
    handle_b.resolver().expect("bind session B");

    let outcome_a = handle_a
        .run_one(invocation("inv_session_a"), "WorkflowEcho", json!({}))
        .await;
    let outcome_b = handle_b
        .run_one(invocation("inv_session_b"), "WorkflowEcho", json!({}))
        .await;

    assert_eq!(outcome_a.summary, "session-a");
    assert_eq!(outcome_b.summary, "session-b");
}

#[tokio::test]
async fn active_route_is_exclusive_and_draining_events_release_to_idle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let session_directory = dir.path().join("session-route");
    let harness = FakeHarness::from_turns([]);
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_session_directory(&session_directory)
        .with_permission_mode(PermissionMode::Yolo);
    let mut registry = ToolRegistry::new();
    registry.register(EchoTool("ok"));
    let handle = handle(config, &harness, Arc::new(registry), AgentContext::new());
    let resolver = handle.resolver().expect("resolver");
    let active = Arc::new(Mutex::new(Vec::new()));
    let idle = Arc::new(Mutex::new(Vec::new()));
    let idle_events = Arc::clone(&idle);
    let _idle_lease = resolver
        .lease_idle_event_route(
            Some(&session_directory),
            Arc::new(move |event| idle_events.lock().expect("idle events").push(event)),
        )
        .expect("idle route");
    let active_events = Arc::clone(&active);
    let (producer_lease, drain_lease) = resolver
        .lease_event_route(
            Some(&session_directory),
            7,
            Arc::new(move |event| active_events.lock().expect("active events").push(event)),
        )
        .expect("active route");

    handle
        .run_one(invocation("inv_active"), "WorkflowEcho", json!({}))
        .await;
    let active_count = active.lock().expect("active events").len();
    assert!(active_count > 0);
    assert!(idle.lock().expect("idle events").is_empty());

    drop(producer_lease);
    handle
        .run_one(invocation("inv_draining"), "WorkflowEcho", json!({}))
        .await;
    assert_eq!(active.lock().expect("active events").len(), active_count);
    assert!(idle.lock().expect("idle events").is_empty());

    drop(drain_lease);
    assert!(!idle.lock().expect("idle events").is_empty());
}

#[test]
fn event_callback_can_reenter_resolver_without_lock_deadlock() {
    let dir = tempfile::tempdir().expect("tempdir");
    let session_directory = dir.path().join("session-reentrant-event");
    let harness = FakeHarness::from_turns([]);
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_session_directory(&session_directory)
        .with_permission_mode(PermissionMode::Yolo);
    let mut registry = ToolRegistry::new();
    registry.register(EchoTool("ok"));
    let handle = handle(config, &harness, Arc::new(registry), AgentContext::new());
    let resolver = handle.resolver().expect("resolver");
    let (callback_tx, callback_rx) = std::sync::mpsc::channel();
    let callback_resolver = resolver.clone();
    let _idle_lease = resolver
        .lease_idle_event_route(
            Some(&session_directory),
            Arc::new(move |_| {
                callback_resolver
                    .resolve()
                    .expect("callback re-enters resolver");
                let _ = callback_tx.send(());
            }),
        )
        .expect("idle route");

    let worker = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(handle.run_one(invocation("inv_reentrant_event"), "WorkflowEcho", json!({})))
    });

    callback_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("event callback must run without holding the resolver lock");
    let outcome = worker.join().expect("dispatch thread");
    assert_eq!(outcome.status, WorkflowOutcomeStatus::Completed);
}

#[tokio::test]
async fn stale_idle_route_lease_cannot_remove_replacement() {
    let dir = tempfile::tempdir().expect("tempdir");
    let session_directory = dir.path().join("session-route-replacement");
    let harness = FakeHarness::from_turns([]);
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_session_directory(&session_directory)
        .with_permission_mode(PermissionMode::Yolo);
    let mut registry = ToolRegistry::new();
    registry.register(EchoTool("ok"));
    let handle = handle(config, &harness, Arc::new(registry), AgentContext::new());
    let resolver = handle.resolver().expect("resolver");
    let first = Arc::new(Mutex::new(Vec::new()));
    let first_events = Arc::clone(&first);
    let first_lease = resolver
        .lease_idle_event_route(
            Some(&session_directory),
            Arc::new(move |event| first_events.lock().expect("first events").push(event)),
        )
        .expect("first idle route");
    let second = Arc::new(Mutex::new(Vec::new()));
    let second_events = Arc::clone(&second);
    let _second_lease = resolver
        .lease_idle_event_route(
            Some(&session_directory),
            Arc::new(move |event| second_events.lock().expect("second events").push(event)),
        )
        .expect("replacement idle route");

    drop(first_lease);
    handle
        .run_one(invocation("inv_replacement"), "WorkflowEcho", json!({}))
        .await;

    assert!(first.lock().expect("first events").is_empty());
    assert!(!second.lock().expect("second events").is_empty());
}

#[tokio::test]
async fn stale_approval_route_lease_cannot_remove_replacement() {
    let dir = tempfile::tempdir().expect("tempdir");
    let session_directory = dir.path().join("session-approval-replacement");
    let harness = FakeHarness::from_turns([]);
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_session_directory(&session_directory)
        .with_permission_mode(PermissionMode::Ask);
    let handle = handle(
        config,
        &harness,
        Arc::new(ToolRegistry::with_builtin_tools()),
        AgentContext::new(),
    );
    let resolver = handle.resolver().expect("resolver");
    let first_calls = Arc::new(AtomicUsize::new(0));
    let first_handler_calls = Arc::clone(&first_calls);
    let first_lease = resolver
        .lease_approval_route(
            Some(&session_directory),
            Arc::new(move |request| {
                first_handler_calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    ApprovalResponse::Selected {
                        request_id: request.id,
                        action: ApprovalAction::PermitOnce,
                        feedback: None,
                    }
                })
            }),
        )
        .expect("first approval route");
    let second_calls = Arc::new(AtomicUsize::new(0));
    let second_handler_calls = Arc::clone(&second_calls);
    let _second_lease = resolver
        .lease_approval_route(
            Some(&session_directory),
            Arc::new(move |request| {
                second_handler_calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    ApprovalResponse::Selected {
                        request_id: request.id,
                        action: ApprovalAction::Reject,
                        feedback: None,
                    }
                })
            }),
        )
        .expect("replacement approval route");

    drop(first_lease);
    let outcome = handle
        .run_one(
            invocation("inv_approval_replacement"),
            "Bash",
            json!({"command": "sudo --version"}),
        )
        .await;

    assert_eq!(outcome.status, WorkflowOutcomeStatus::Denied);
    assert_eq!(first_calls.load(Ordering::SeqCst), 0);
    assert_eq!(second_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn idle_model_update_replaces_client_before_next_workflow_invocation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = FakeHarness::from_turns([]);
    let second = FakeHarness::from_turns([child_text_turn("second client")]);
    let config = AgentConfig::for_model(first.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo);
    let handle = handle(
        config,
        &first,
        Arc::new(ToolRegistry::with_builtin_tools()),
        AgentContext::new(),
    );
    let resolver = handle.resolver().expect("bind initial client");
    let mut second_model = second.model();
    second_model.provider.0 = "second-provider".to_owned();
    second_model.model = "second-model".to_owned();

    resolver
        .update_model_for_session(
            handle.config.session_directory.as_deref(),
            second_model,
            second.client(),
        )
        .expect("idle model update");
    let outcome = handle
        .run_one(
            invocation("inv_after_idle_model_switch"),
            "Delegate",
            json!({"task": "use selected client", "context": "none"}),
        )
        .await;

    assert!(outcome.ok, "{}", outcome.summary);
    assert!(first.requests().is_empty(), "stale client must not be used");
    assert_eq!(second.requests().len(), 1);
    let snapshot = resolver.resolve().expect("updated snapshot");
    assert_eq!(snapshot.config.model.provider.0, "second-provider");
    assert_eq!(snapshot.config.model.model, "second-model");
}

fn child_text_turn(text: &str) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            id: format!("msg_{text}"),
        },
        AiStreamEvent::TextDelta {
            text: text.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]
}

fn child_text_turn_with_usage(text: &str, usage: AgentTokenUsage) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            id: format!("msg_{text}"),
        },
        AiStreamEvent::TextDelta {
            text: text.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::EndTurn,
            usage: Some(neo_ai::TokenUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                input_cache_read_tokens: usage.input_cache_read_tokens,
                input_cache_write_tokens: usage.input_cache_write_tokens,
            }),
        },
    ]
}

fn workflow_launch_request() -> WorkflowLaunchRequest {
    WorkflowLaunchRequest {
        name: "dispatch-test".to_owned(),
        description: "dispatch-test".to_owned(),
        phases: Vec::new(),
        script: String::new(),
        args: json!({}),
        launch_source: "test".to_owned(),
        parent_run_id: None,
    }
}

#[tokio::test]
async fn delegate_usage_and_child_ref_are_journaled_and_aggregated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let usage = AgentTokenUsage {
        input_tokens: 11,
        output_tokens: 7,
        input_cache_read_tokens: 5,
        input_cache_write_tokens: 3,
    };
    let harness = FakeHarness::from_turns([child_text_turn_with_usage("delegate done", usage)]);
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo);
    let dispatch = handle(
        config,
        &harness,
        Arc::new(ToolRegistry::with_builtin_tools()),
        AgentContext::new(),
    );
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let workflow = runtime
        .create_run(dir.path(), workflow_launch_request())
        .await
        .expect("workflow");

    let outcome = workflow
        .invoke(
            0,
            WorkflowInvocationKind::Delegate,
            json!({"task": "inspect usage"}),
            true,
            move |invocation| {
                let dispatch = dispatch.clone();
                async move {
                    dispatch
                        .run_one(
                            invocation,
                            "Delegate",
                            json!({"task": "inspect usage", "context": "none"}),
                        )
                        .await
                }
            },
        )
        .await
        .expect("invoke");

    assert_eq!(outcome.actual_usage, Some(usage));
    let agent_id = outcome.details["agent_id"]
        .as_str()
        .expect("agent_id")
        .to_owned();
    assert_eq!(
        outcome.child_refs,
        [neo_agent_core::workflow::WorkflowChildRef {
            kind: "delegate".to_owned(),
            id: agent_id,
        }]
    );
    let output = workflow.output().await.expect("output");
    assert_eq!(output.actual_usage, Some(usage));
    assert!(output.invocations.iter().any(|record| matches!(
        record,
        JournalRecord::InvocationFinished {
            outcome: journaled,
            ..
        } if journaled.actual_usage == Some(usage)
            && journaled.child_refs == outcome.child_refs
    )));
}

#[tokio::test]
async fn delegate_usage_at_token_cap_blocks_next_provider_invocation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let usage = AgentTokenUsage {
        input_tokens: 8,
        output_tokens: 4,
        input_cache_read_tokens: 2,
        input_cache_write_tokens: 1,
    };
    let harness = FakeHarness::from_turns([child_text_turn_with_usage("delegate done", usage)]);
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo);
    let dispatch = handle(
        config,
        &harness,
        Arc::new(ToolRegistry::with_builtin_tools()),
        AgentContext::new(),
    );
    let runtime = WorkflowRuntime::new(WorkflowLimits {
        token_cap: Some(u64::from(usage.input_tokens + usage.output_tokens)),
        ..WorkflowLimits::default()
    });
    let workflow = runtime
        .create_run(dir.path(), workflow_launch_request())
        .await
        .expect("workflow");

    let first_dispatch = dispatch.clone();
    let first = workflow
        .invoke(
            0,
            WorkflowInvocationKind::Delegate,
            json!({"task": "first"}),
            true,
            move |invocation| async move {
                first_dispatch
                    .run_one(
                        invocation,
                        "Delegate",
                        json!({"task": "first", "context": "none"}),
                    )
                    .await
            },
        )
        .await
        .expect("first invoke");
    assert_eq!(first.actual_usage, Some(usage));
    assert_eq!(harness.requests().len(), 1);

    let second = workflow
        .invoke(
            1,
            WorkflowInvocationKind::Delegate,
            json!({"task": "must not run"}),
            true,
            move |invocation| async move {
                dispatch
                    .run_one(
                        invocation,
                        "Delegate",
                        json!({"task": "must not run", "context": "none"}),
                    )
                    .await
            },
        )
        .await
        .expect("second invoke");

    assert_eq!(second.status, WorkflowOutcomeStatus::ResourceLimited);
    assert_eq!(
        harness.requests().len(),
        1,
        "second child client call blocked"
    );
}

#[tokio::test]
async fn swarm_preserves_ids_terminal_children_and_aggregate_usage() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first_usage = AgentTokenUsage {
        input_tokens: 3,
        output_tokens: 5,
        input_cache_read_tokens: 7,
        input_cache_write_tokens: 11,
    };
    let second_usage = AgentTokenUsage {
        input_tokens: 13,
        output_tokens: 17,
        input_cache_read_tokens: 19,
        input_cache_write_tokens: 23,
    };
    let harness = FakeHarness::from_turns([
        child_text_turn_with_usage("first", first_usage),
        child_text_turn_with_usage("second", second_usage),
    ]);
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo);
    let dispatch = handle(
        config,
        &harness,
        Arc::new(ToolRegistry::with_builtin_tools()),
        AgentContext::new(),
    );

    let outcome = dispatch
        .run_one(
            invocation("inv_swarm_usage"),
            "DelegateSwarm",
            json!({
                "description": "aggregate usage",
                "items": [
                    {"title": "first", "value": "first"},
                    {"title": "second", "value": "second"},
                ],
                "prompt_template": "Inspect {{item}}",
                "max_concurrency": 1,
            }),
        )
        .await;

    assert!(outcome.ok, "{}", outcome.summary);
    assert_eq!(
        outcome.actual_usage,
        Some(AgentTokenUsage {
            input_tokens: 16,
            output_tokens: 22,
            input_cache_read_tokens: 26,
            input_cache_write_tokens: 34,
        })
    );
    let swarm_id = outcome.details["swarm_id"].as_str().expect("swarm_id");
    assert_eq!(outcome.child_refs[0].kind, "delegate_swarm");
    assert_eq!(outcome.child_refs[0].id, swarm_id);
    assert_eq!(
        outcome
            .child_refs
            .iter()
            .filter(|child| child.kind == "delegate")
            .count(),
        2
    );
    assert!(
        outcome.details["items"]
            .as_array()
            .expect("items")
            .iter()
            .all(|item| item["status"].as_str() == Some("completed"))
    );
}

#[tokio::test]
async fn delegate_and_swarm_forward_canonical_lifecycle_events() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = FakeHarness::from_turns([
        child_text_turn("delegate done"),
        child_text_turn("swarm done"),
    ]);
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo);
    let handle = handle(
        config,
        &harness,
        Arc::new(ToolRegistry::with_builtin_tools()),
        AgentContext::new(),
    );
    let (events, _event_lease, _event_drain_lease) = capture_events(&handle);

    let delegate = handle
        .run_one(
            invocation("inv_delegate"),
            "Delegate",
            json!({"task": "inspect dispatch", "context": "none"}),
        )
        .await;
    assert!(delegate.ok, "{}", delegate.summary);
    let swarm = handle
        .run_one(
            invocation("inv_swarm"),
            "DelegateSwarm",
            json!({
                "description": "inspect dispatch",
                "items": [{"title": "runtime", "value": "runtime"}],
                "prompt_template": "Inspect {{item}}",
            }),
        )
        .await;
    assert!(swarm.ok, "{}", swarm.summary);

    let events = events.lock().expect("events");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::DelegateStarted { .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::DelegateFinished { .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::DelegateSwarmStarted { .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::DelegateSwarmFinished { .. }))
    );
}

struct BlockingTool {
    entered: Arc<Notify>,
}

impl Tool for BlockingTool {
    fn name(&self) -> &'static str {
        "WorkflowBlocking"
    }

    fn description(&self) -> &'static str {
        "waits for workflow cancellation"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object"})
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            self.entered.notify_one();
            ctx.cancel_token.cancelled().await;
            Ok(
                ToolResult::error("cancelled internally").with_details(json!({
                    "kind": "cancelled",
                    "side_effect_occurred": false,
                })),
            )
        })
    }
}

#[tokio::test]
async fn invocation_cancel_token_cancels_canonical_execution() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = FakeHarness::from_turns([]);
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo);
    let entered = Arc::new(Notify::new());
    let mut registry = ToolRegistry::new();
    registry.register(BlockingTool {
        entered: Arc::clone(&entered),
    });
    let handle = handle(config, &harness, Arc::new(registry), AgentContext::new());
    let cancel_token = CancellationToken::new();
    let run = tokio::spawn({
        let handle = handle.clone();
        let cancel_token = cancel_token.clone();
        async move {
            handle
                .run_one(
                    WorkflowInvocationContext {
                        invocation_id: "inv_cancel".to_owned(),
                        cancel_token,
                    },
                    "WorkflowBlocking",
                    json!({}),
                )
                .await
        }
    });
    entered.notified().await;
    cancel_token.cancel();

    let outcome = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("dispatch observes cancellation")
        .expect("dispatch task");
    assert_eq!(outcome.status, WorkflowOutcomeStatus::Cancelled);
    assert_eq!(outcome.details["kind"], "cancelled");
}

#[tokio::test]
async fn ordinary_tool_turn_finishes_while_session_resolver_remains_alive() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "parent_tool".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_bash".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_bash".to_owned(),
                raw_arguments: json!({"command": "echo turn-completes"}).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
        ],
        child_text_turn("done"),
    ]);
    let resolver = neo_agent_core::runtime::WorkflowDispatchResolver::default();
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo)
        .with_workflow_dispatch_resolver(resolver.clone());
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = tokio::time::timeout(
        Duration::from_secs(5),
        runtime
            .run_turn(&mut context, AgentMessage::user_text("run bash"))
            .collect::<Vec<_>>(),
    )
    .await
    .expect("turn event channel must close");

    assert!(events.into_iter().all(|event| event.is_ok()));
    assert!(resolver.resolve().is_ok(), "session resolver remains alive");
}

#[tokio::test]
async fn idle_route_waits_for_active_stream_drop_after_receiver_exhaustion() {
    let dir = tempfile::tempdir().expect("tempdir");
    let session_directory = dir.path().join("session-stream-drain");
    let harness = FakeHarness::from_turns([child_text_turn("done")]);
    let resolver = neo_agent_core::runtime::WorkflowDispatchResolver::default();
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_session_directory(&session_directory)
        .with_permission_mode(PermissionMode::Yolo)
        .with_workflow_dispatch_resolver(resolver.clone());
    let mut registry = ToolRegistry::new();
    registry.register(EchoTool("idle"));
    let registry = Arc::new(registry);
    let dispatch = handle(config.clone(), &harness, registry, AgentContext::new());
    let idle = Arc::new(Mutex::new(Vec::new()));
    let idle_events = Arc::clone(&idle);
    let _idle_lease = resolver
        .lease_idle_event_route(
            Some(&session_directory),
            Arc::new(move |event| idle_events.lock().expect("idle events").push(event)),
        )
        .expect("idle route");
    let runtime = AgentRuntime::with_tools(config, harness.client(), ToolRegistry::new());
    let mut context = AgentContext::new();
    let mut stream = runtime.run_turn(&mut context, AgentMessage::user_text("finish"));
    while stream.next().await.is_some() {}

    dispatch
        .run_one(
            invocation("inv_after_exhaustion"),
            "WorkflowEcho",
            json!({}),
        )
        .await;
    assert!(
        idle.lock().expect("idle events").is_empty(),
        "receiver exhaustion precedes the caller's final writer flush"
    );

    drop(stream);
    assert!(
        !idle.lock().expect("idle events").is_empty(),
        "dropping the stream releases events only after the caller's flush boundary"
    );
}

struct ConcurrentEventTool {
    barrier: Arc<Barrier>,
}

impl Tool for ConcurrentEventTool {
    fn name(&self) -> &'static str {
        "ConcurrentWorkflowEvent"
    }

    fn description(&self) -> &'static str {
        "emits one context event after a concurrency barrier"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {"message": {"type": "string"}},
            "required": ["message"]
        })
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            self.barrier.wait().await;
            ctx.emit_event(AgentEvent::FollowUpQueued {
                message: AgentMessage::user_text(
                    input["message"].as_str().expect("message").to_owned(),
                ),
            });
            Ok(ToolResult::ok("emitted"))
        })
    }
}

#[tokio::test]
async fn concurrent_workflow_calls_merge_context_events_without_last_writer_wins() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = FakeHarness::from_turns([]);
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo);
    let mut registry = ToolRegistry::new();
    registry.register(ConcurrentEventTool {
        barrier: Arc::new(Barrier::new(2)),
    });
    let handle = handle(config, &harness, Arc::new(registry), AgentContext::new());

    let (first, second) = tokio::join!(
        handle.run_one(
            invocation("inv_concurrent_1"),
            "ConcurrentWorkflowEvent",
            json!({"message": "first"}),
        ),
        handle.run_one(
            invocation("inv_concurrent_2"),
            "ConcurrentWorkflowEvent",
            json!({"message": "second"}),
        ),
    );
    assert!(first.ok && second.ok);
    assert_eq!(
        handle
            .resolver()
            .expect("resolver")
            .resolve()
            .expect("snapshot")
            .context
            .pending_follow_up_len(),
        2,
    );
}
