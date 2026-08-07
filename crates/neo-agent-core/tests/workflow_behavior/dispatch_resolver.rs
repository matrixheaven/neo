use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use neo_agent_core::harness::FakeHarness;
use neo_agent_core::runtime::WorkflowDispatchSnapshot;
use neo_agent_core::tools::{Tool, ToolContext, ToolFuture, ToolRegistry, ToolResult};
use neo_agent_core::workflow::WorkflowOutcomeStatus;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, AgentTokenUsage,
    ApprovalAction, ApprovalResponse, PermissionMode,
};
use neo_ai::{AiStreamEvent, StopReason};
use serde_json::json;
use tokio::sync::Barrier;

use super::dispatch::{handle, invocation};

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
    resolver.refresh(snapshot).expect("refresh snapshot");

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

    assert!(outcome.is_completed(), "{}", outcome.summary);
    assert!(first.requests().is_empty(), "stale client must not be used");
    assert_eq!(second.requests().len(), 1);
    let snapshot = resolver.resolve().expect("updated snapshot");
    assert_eq!(snapshot.config.model.provider.0, "second-provider");
    assert_eq!(snapshot.config.model.model, "second-model");
}

pub(crate) fn child_text_turn(text: &str) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: format!("msg_{text}"),
        },
        AiStreamEvent::TextDelta {
            text: text.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]
}

pub(crate) fn child_text_turn_with_usage(text: &str, usage: AgentTokenUsage) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: format!("msg_{text}"),
        },
        AiStreamEvent::TextDelta {
            text: text.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
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

#[tokio::test]
async fn ordinary_tool_turn_finishes_while_session_resolver_remains_alive() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: neo_ai::MessagePhase::Unknown,
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
                phase: neo_ai::MessagePhase::Unknown,
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
    assert!(first.is_completed() && second.is_completed());
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
