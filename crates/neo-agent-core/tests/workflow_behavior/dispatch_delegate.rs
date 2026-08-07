use std::sync::Arc;
use std::time::Duration;

use neo_agent_core::harness::FakeHarness;
use neo_agent_core::tools::{Tool, ToolContext, ToolFuture, ToolRegistry, ToolResult};
use neo_agent_core::workflow::journal::{JournalPayload, collect_journal};
use neo_agent_core::workflow::{
    WorkflowInvocationContext, WorkflowInvocationKind, WorkflowLimits, WorkflowOutcomeStatus,
    WorkflowRuntime, journal_path,
};
use neo_agent_core::{AgentConfig, AgentContext, AgentEvent, AgentTokenUsage, PermissionMode};
use serde_json::json;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::dispatch::{capture_events, handle, invocation, workflow_launch_request};
use super::dispatch_resolver::{child_text_turn, child_text_turn_with_usage};

#[tokio::test]
async fn delegate_usage_and_child_ref_are_journaled_and_aggregated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let usage = AgentTokenUsage {
        input_tokens: 11,
        output_tokens: 7,
        input_cache_read_tokens: 5,
        input_cache_write_tokens: 3,
    };
    let harness = FakeHarness::from_turns([child_text_turn_with_usage(
        r#"{"result":"delegate done"}"#,
        usage,
    )]);
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo)
        .with_workflow_runtime(runtime.clone());
    let dispatch = handle(
        config,
        &harness,
        Arc::new(ToolRegistry::with_builtin_tools()),
        AgentContext::new(),
    );
    let workflow = runtime
        .create_run(dir.path(), workflow_launch_request())
        .await
        .expect("workflow");
    workflow
        .enter_running_for_direct_execution()
        .await
        .expect("enter running");
    let origin = workflow.execution_origin(None).await;
    let output_schema = json!({
        "type": "object",
        "properties": {"result": {"type": "string"}},
        "required": ["result"],
        "additionalProperties": false,
    });

    let outcome = workflow
        .invoke(
            0,
            WorkflowInvocationKind::Delegate,
            json!({"task": "review usage", "output_schema": output_schema}),
            true,
            move |invocation| {
                let dispatch = dispatch.clone();
                async move {
                    dispatch
                        .run_one_with_origin(
                            invocation,
                            "Delegate",
                            json!({
                                "task": "review usage",
                                "context": "none",
                                "output_schema": {
                                    "type": "object",
                                    "properties": {"result": {"type": "string"}},
                                    "required": ["result"],
                                    "additionalProperties": false,
                                }
                            }),
                            Some(origin),
                        )
                        .await
                }
            },
        )
        .await
        .expect("invoke");

    assert_eq!(outcome.actual_usage, Some(usage), "{outcome:?}");
    let agent_id = outcome.details["agent_id"]
        .as_str()
        .expect("agent_id")
        .to_owned();
    assert_eq!(
        outcome.child_refs,
        [neo_agent_core::workflow::WorkflowChildRef {
            kind: "delegate".to_owned(),
            id: agent_id.clone(),
        }]
    );
    let output = workflow.output().await.expect("output");
    assert_eq!(output.actual_usage, Some(usage));
    let envelopes = collect_journal(
        &journal_path(dir.path(), &workflow.run_id),
        Some(&workflow.run_id),
        WorkflowLimits::default().journal_record_bytes,
        WorkflowLimits::default().journal_total_bytes,
    )
    .expect("journal");
    assert!(envelopes.iter().any(|record| matches!(
        &record.payload,
        JournalPayload::InvocationFinished {
            outcome: journaled,
            ..
        } if journaled.actual_usage == Some(usage)
            && journaled.child_refs == outcome.child_refs
    )));
    let queued_index = envelopes
        .iter()
        .position(|record| matches!(&record.payload, JournalPayload::ChildQueued { .. }))
        .expect("queued child");
    let started_index = envelopes
        .iter()
        .position(|record| matches!(&record.payload, JournalPayload::ChildStarted { .. }))
        .expect("started child");
    let finished_index = envelopes
        .iter()
        .position(|record| matches!(&record.payload, JournalPayload::ChildFinished { .. }))
        .expect("finished child");
    assert!(queued_index < started_index && started_index < finished_index);
    assert!(matches!(
        &envelopes[started_index].payload,
        JournalPayload::ChildStarted {
            agent_id: Some(journaled_agent_id),
            ..
        } if journaled_agent_id == &agent_id
    ));
}

#[tokio::test]
async fn invalid_delegate_finishes_without_started_child() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = FakeHarness::from_turns([]);
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
    workflow
        .enter_running_for_direct_execution()
        .await
        .expect("enter running");
    let origin = workflow.execution_origin(None).await;

    let outcome = workflow
        .invoke(
            0,
            WorkflowInvocationKind::Delegate,
            json!({"task": ""}),
            true,
            move |invocation| async move {
                dispatch
                    .run_one_with_origin(invocation, "Delegate", json!({"task": ""}), Some(origin))
                    .await
            },
        )
        .await
        .expect("invoke");

    assert!(!outcome.is_completed());
    let envelopes = collect_journal(
        &journal_path(dir.path(), &workflow.run_id),
        Some(&workflow.run_id),
        WorkflowLimits::default().journal_record_bytes,
        WorkflowLimits::default().journal_total_bytes,
    )
    .expect("journal");
    assert!(
        envelopes
            .iter()
            .any(|record| matches!(&record.payload, JournalPayload::ChildQueued { .. }))
    );
    assert!(
        !envelopes
            .iter()
            .any(|record| matches!(&record.payload, JournalPayload::ChildStarted { .. }))
    );
    assert!(
        envelopes
            .iter()
            .any(|record| matches!(&record.payload, JournalPayload::ChildFinished { .. }))
    );
}

#[tokio::test]
async fn failed_delegate_binding_terminalizes_child_before_model_start() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = FakeHarness::from_turns([]);
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
    let multi_agent = dispatch.config.multi_agent.clone();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let workflow = runtime
        .create_run(dir.path(), workflow_launch_request())
        .await
        .expect("workflow");
    workflow
        .enter_running_for_direct_execution()
        .await
        .expect("enter running");
    let origin = workflow.execution_origin(None).await;

    let outcome = workflow
        .invoke(
            0,
            WorkflowInvocationKind::Delegate,
            json!({"task": "must not start"}),
            true,
            move |invocation| async move {
                dispatch
                    .run_one_with_origin(
                        invocation,
                        "Delegate",
                        json!({"task": "must not start"}),
                        Some(origin),
                    )
                    .await
            },
        )
        .await
        .expect("invoke");

    assert_eq!(outcome.status, WorkflowOutcomeStatus::Failed);
    assert!(harness.requests().is_empty());
    let agents = multi_agent.list_agents(true);
    assert_eq!(agents.len(), 1);
    assert_eq!(
        agents[0].state,
        neo_agent_core::multi_agent::AgentLifecycleState::Failed
    );
    let envelopes = collect_journal(
        &journal_path(dir.path(), &workflow.run_id),
        Some(&workflow.run_id),
        WorkflowLimits::default().journal_record_bytes,
        WorkflowLimits::default().journal_total_bytes,
    )
    .expect("journal");
    assert!(
        !envelopes
            .iter()
            .any(|record| matches!(&record.payload, JournalPayload::ChildStarted { .. }))
    );
    assert!(envelopes.iter().any(|record| matches!(
        &record.payload,
        JournalPayload::ChildFinished {
            agent_id: Some(agent_id),
            ..
        } if agent_id == agents[0].id.as_str()
    )));
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
                "prompt_template": "Review {{item}}",
                "max_concurrency": 1,
            }),
        )
        .await;

    assert!(outcome.is_completed(), "{}", outcome.summary);
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
            json!({"task": "review dispatch", "context": "none"}),
        )
        .await;
    assert!(delegate.is_completed(), "{}", delegate.summary);
    let swarm = handle
        .run_one(
            invocation("inv_swarm"),
            "DelegateSwarm",
            json!({
                "description": "review dispatch",
                "items": [{"title": "runtime", "value": "runtime"}],
                "prompt_template": "Review {{item}}",
            }),
        )
        .await;
    assert!(swarm.is_completed(), "{}", swarm.summary);

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

#[tokio::test]
async fn workflow_delegate_and_swarm_use_live_yolo_after_handle_creation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = FakeHarness::from_turns([
        child_text_turn("delegate done"),
        child_text_turn("swarm done"),
    ]);
    let live_mode = Arc::new(std::sync::RwLock::new(PermissionMode::Ask));
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Ask)
        .with_live_permission_mode(Arc::clone(&live_mode));
    let handle = handle(
        config,
        &harness,
        Arc::new(ToolRegistry::with_builtin_tools()),
        AgentContext::new(),
    );
    *live_mode.write().expect("live permission mode") = PermissionMode::Yolo;

    let delegate = handle
        .run_one(
            invocation("inv_live_delegate"),
            "Delegate",
            json!({"task": "review dispatch", "context": "none"}),
        )
        .await;
    assert!(delegate.is_completed(), "{}", delegate.summary);
    let swarm = handle
        .run_one(
            invocation("inv_live_swarm"),
            "DelegateSwarm",
            json!({
                "description": "review dispatch",
                "items": [{"title": "runtime", "value": "runtime"}],
                "prompt_template": "Review {{item}}",
            }),
        )
        .await;
    assert!(swarm.is_completed(), "{}", swarm.summary);
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
