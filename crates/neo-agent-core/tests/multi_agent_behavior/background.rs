#![allow(clippy::duration_suboptimal_units)]
use std::time::Duration;

use futures::StreamExt;
use neo_agent_core::harness::FakeHarness;
use neo_agent_core::multi_agent::{
    AgentId, AgentLifecycleState, AgentRole, AgentRunMode, AgentTerminalReason, DelegateContext,
    DelegateRequest, MultiAgentRuntime, SwarmAggregate,
};
use neo_agent_core::tools::{
    BackgroundTaskKind, BackgroundTaskManager, ToolContext, ToolRegistry, ToolResult,
};
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, PermissionMode, ToolAccess,
    ToolExecutionMode,
};
use neo_ai::{AiStreamEvent, StopReason};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn swarm_text_deltas_are_bounded_and_background_updates_stay_ordered() {
    let harness = FakeHarness::from_turns([vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "a".repeat(20_000),
        },
        AiStreamEvent::TextDelta {
            text: "latest".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]]);
    let events = Arc::new(Mutex::new(Vec::new()));
    let events_for_callback = Arc::clone(&events);
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_access(ToolAccess::all())
        .with_child_runtime(
            AgentConfig::for_model(harness.model())
                .with_permission_mode(PermissionMode::Yolo)
                .with_tool_execution_mode(ToolExecutionMode::Sequential),
            harness.client(),
            Arc::new(ToolRegistry::new()),
            1,
        )
        .with_tool_event(Arc::new(move |event| {
            events_for_callback
                .lock()
                .expect("event capture poisoned")
                .push(event);
        }));
    let registry = ToolRegistry::with_builtin_tools();
    let started = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "bounded progress",
                "items": [{"title": "agent-1", "value": "agent-1"}],
                "prompt_template": "Process {{item}}",
                "mode": "background",
                "max_concurrency": 1
            }),
        )
        .await
        .expect("background swarm should start");
    let task_id = started
        .details
        .as_ref()
        .and_then(|details| details.get("task_id"))
        .and_then(serde_json::Value::as_str)
        .expect("background task id")
        .to_owned();

    let waited = registry
        .run(
            "WaitDelegate",
            &ctx,
            serde_json::json!({ "ids": [task_id], "timeout_ms": 5_000 }),
        )
        .await
        .expect("background swarm should complete");
    assert!(
        waited.content.contains("\"status\":\"completed\""),
        "{}",
        waited.content
    );

    {
        let events = events.lock().expect("event capture poisoned");
        assert!(
            events
                .iter()
                .all(|event| serde_json::to_vec(event).unwrap().len() < 8 * 1024),
            "swarm progress events must remain bounded: {events:#?}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::DelegateSwarmProgressUpdated { .. })),
            "child updates must use bounded progress events: {events:#?}"
        );
    }

    let snapshot = ctx
        .background_tasks
        .snapshot(&task_id)
        .await
        .expect("background snapshot");
    let swarm = snapshot.swarm.expect("swarm snapshot");
    let child = swarm.children.first().expect("swarm child");
    assert!(
        child
            .agent
            .latest_text
            .as_deref()
            .is_some_and(|text| text.ends_with("latest"))
    );
    assert!(child.agent.state.is_terminal());
}

#[tokio::test]
async fn background_manager_lists_delegate_tasks() {
    let runtime = MultiAgentRuntime::new();
    let agent = runtime.start_foreground_delegate_for_test("inspect task browser");
    let manager = BackgroundTaskManager::new();

    manager.start_delegate(agent.clone()).await;
    let snapshots = manager.list(false, 10).await;

    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].kind, BackgroundTaskKind::Delegate);
    assert_eq!(snapshots[0].task_id, agent.id.as_str());
    assert!(snapshots[0].delegate.is_some());
}

#[tokio::test]
async fn background_manager_lists_swarm_tasks() {
    use neo_agent_core::multi_agent::{
        AgentDisplayName, AgentPath, AgentRole, AgentSnapshot, SwarmChildSnapshot, SwarmSnapshot,
    };
    let _runtime = MultiAgentRuntime::new();
    let name = AgentDisplayName::new("Zeno");
    let child_agent = AgentSnapshot {
        id: AgentId::from_suffix_for_test("sw-0"),
        display_name: name.clone(),
        path: AgentPath::root_child(&name),
        role: AgentRole::Coder,
        mode: AgentRunMode::Background,
        context: neo_agent_core::multi_agent::DelegateContext::None,
        state: AgentLifecycleState::Running,
        task: "item 0".to_owned(),
        task_title: "item 0".to_owned(),
        created_at_ms: 1,
        updated_at_ms: 1,
        started_at_ms: Some(1),
        terminal_at_ms: None,
        detached_from_foreground: true,
        terminal_reason: None,
        run_count: 1,
        live_messages_received: 0,
        previous_status: None,
        terminal_status_history: Vec::new(),
        resumed_from: None,
        tool_count: 0,
        token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: Duration::ZERO,
        latest_text: None,
        activity: Vec::new(),
        prior_messages: Vec::new(),
        outcome: None,
    };
    let children = vec![SwarmChildSnapshot {
        item_index: 0,
        item: "check".to_owned(),
        agent: child_agent,
    }];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|c| c.agent.state));
    let swarm = SwarmSnapshot {
        swarm_id: "swarm-test".to_owned(),
        description: "test swarm".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Background,
        state: AgentLifecycleState::Running,
        max_concurrency: 1,
        aggregate,
        children,
    };
    let manager = BackgroundTaskManager::new();
    manager.start_delegate_swarm(swarm).await;

    let snapshots = manager.list(false, 10).await;
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].kind, BackgroundTaskKind::DelegateSwarm);
    assert!(snapshots[0].swarm.is_some());
}

#[tokio::test]
async fn delegate_background_registers_task() {
    let harness = FakeHarness::from_turns([vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::ToolCallStart {
            id: "tool_1".to_owned(),
            name: "Delegate".to_owned(),
        },
        AiStreamEvent::ToolCallArgsDelta {
            id: "tool_1".to_owned(),
            json_fragment: r#"{"task":"bg task","mode":"background"}"#.to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: "tool_1".to_owned(),
            raw_arguments: json!({ "task": "bg task", "mode": "background" }).to_string(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::ToolUse,
            usage: None,
        },
    ]]);
    let tools = ToolRegistry::with_builtin_tools();
    let config = AgentConfig::for_model(harness.model())
        .with_tool_execution_mode(ToolExecutionMode::Sequential)
        .with_permission_mode(PermissionMode::Yolo);
    let runtime = AgentRuntime::with_tools(config, harness.client(), tools);
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run bg delegate"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    // The tool result should mention background mode.
    let tool_finished: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolExecutionFinished { result, .. } => Some(result),
            _ => None,
        })
        .collect();
    let delegate_result = tool_finished
        .iter()
        .find(|r| r.content.contains("kind: delegate"))
        .expect("should have a delegate result");
    assert!(
        delegate_result.content.contains("status: running"),
        "{}",
        delegate_result.content
    );

    // Details should include background mode.
    let details = delegate_result.details.as_ref().expect("details");
    assert_eq!(details["mode"], "background");
}

#[tokio::test]
async fn task_output_reports_delegate_context_mode_from_current_run() {
    let (registry, ctx) = registry_with_multi_agent();
    let summary_output = task_output_for_background_delegate(
        &registry,
        &ctx,
        "summarize visible context",
        "summary",
    )
    .await;
    assert_task_output_context(&summary_output, "summary");

    let none_output =
        task_output_for_background_delegate(&registry, &ctx, "run without parent context", "none")
            .await;
    assert_task_output_context(&none_output, "none");

    let resume_dir = tempfile::tempdir().unwrap();
    let resume_ctx = ToolContext::new(resume_dir.path()).unwrap();
    let original = resume_ctx.multi_agent.start_delegate(
        "finish first run",
        None,
        AgentRole::Coder,
        AgentRunMode::Background,
        DelegateContext::None,
        neo_agent_core::multi_agent::AgentPathKind::Root,
    );
    let _ = resume_ctx
        .multi_agent
        .complete_delegate_for_test(&original.id, "first run complete");
    let resume_request = DelegateRequest {
        task: "resume with summarized context".to_owned(),
        resume: Some(original.id.as_str().to_owned()),
        title: None,
        role: None,
        mode: AgentRunMode::Background,
        context: DelegateContext::Summary,
        output_schema: None,
    };
    resume_ctx
        .multi_agent
        .start_resume_delegate(original.id.as_str(), &resume_request)
        .expect("delegate should resume");

    let resumed_output = ToolRegistry::with_builtin_tools()
        .run(
            "TaskOutput",
            &resume_ctx,
            serde_json::json!({ "task_id": original.id.as_str() }),
        )
        .await
        .expect("TaskOutput should return resumed delegate output");

    assert_task_output_context(&resumed_output, "summary");
}

#[tokio::test]
async fn ctrl_b_detach_preserves_agent_id_and_registers_background_task() {
    let runtime = MultiAgentRuntime::new();
    let manager = BackgroundTaskManager::new();
    let running = runtime.start_foreground_delegate_for_test("detach me");

    let detached = runtime
        .detach_agent(&running.id)
        .expect("agent should detach");
    manager.start_delegate(detached.clone()).await;
    let tasks = manager.list(false, 10).await;

    assert_eq!(detached.id, running.id);
    assert_eq!(detached.mode, AgentRunMode::Background);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task_id, running.id.as_str());
}

#[tokio::test]
async fn list_delegates_reports_background_delegate() {
    use neo_agent_core::tools::ToolContext;
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_access(ToolAccess::all());
    let agent = ctx
        .multi_agent
        .start_foreground_delegate_for_test("inspect background registry");
    ctx.background_tasks.start_delegate(agent.clone()).await;

    let result = ToolRegistry::with_builtin_tools()
        .run(
            "ListDelegates",
            &ctx,
            serde_json::json!({ "include_completed": true }),
        )
        .await
        .expect("list should succeed");

    assert!(result.content.contains(agent.id.as_str()));
    assert!(result.content.contains("inspect background registry"));
}

#[tokio::test]
async fn restored_running_delegate_is_reported_lost_with_resume_hint() {
    let runtime = MultiAgentRuntime::new();
    let running = runtime.start_foreground_delegate_for_test("long audit");
    let id = running.id.as_str().to_owned();
    let restored = MultiAgentRuntime::new();
    restored.restore_from_replay(
        [AgentEvent::DelegateStarted {
            turn: 1,
            agent: running,
            workflow_origin: None,
        }]
        .iter(),
    );

    let snapshot = restored.agent_snapshot(&id).expect("restored");
    assert_eq!(snapshot.state, AgentLifecycleState::Interrupted);
    assert_eq!(
        snapshot.terminal_reason,
        Some(AgentTerminalReason::ProcessExited)
    );
    assert!(
        snapshot
            .outcome
            .as_ref()
            .expect("outcome")
            .summary
            .contains(&format!("Delegate(resume=\"{id}\"")),
        "{}",
        snapshot.outcome.as_ref().expect("outcome").summary
    );

    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_multi_agent(restored);
    let registry = ToolRegistry::with_builtin_tools();

    let output = registry
        .run("TaskOutput", &ctx, json!({ "task_id": id }))
        .await
        .expect("TaskOutput should return delegate output");
    assert_eq!(
        output
            .details
            .as_ref()
            .and_then(|details| details.get("resume_hint"))
            .and_then(serde_json::Value::as_str),
        Some(format!("Delegate(resume=\"{id}\", task=\"continue\")").as_str())
    );

    let listed = registry
        .run(
            "ListDelegates",
            &ctx,
            json!({
                "kind": "agent",
                "include_completed": true,
                "include": ["summary"]
            }),
        )
        .await
        .expect("ListDelegates should return restored delegate");
    let listed_hint = listed
        .details
        .as_ref()
        .and_then(|details| details.get("delegates"))
        .and_then(serde_json::Value::as_array)
        .and_then(|delegates| delegates.iter().find(|row| row["id"] == id))
        .and_then(|row| row.get("resume_hint"))
        .and_then(serde_json::Value::as_str);
    assert_eq!(
        listed_hint,
        Some(format!("Delegate(resume=\"{id}\", task=\"continue\")").as_str())
    );

    let waited = registry
        .run(
            "WaitDelegate",
            &ctx,
            json!({ "ids": [id], "timeout_ms": 1 }),
        )
        .await
        .expect("WaitDelegate should return restored delegate");
    assert_eq!(
        waited
            .details
            .as_ref()
            .and_then(|details| details.get("items"))
            .and_then(serde_json::Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("resume_hint"))
            .and_then(serde_json::Value::as_str),
        Some(format!("Delegate(resume=\"{id}\", task=\"continue\")").as_str())
    );
}

#[tokio::test]
async fn list_delegates_paginates_with_cursor_without_repeating_rows() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_access(ToolAccess::all());
    let first = ctx.multi_agent.start_delegate(
        "first page candidate",
        None,
        AgentRole::Coder,
        AgentRunMode::Background,
        neo_agent_core::multi_agent::DelegateContext::None,
        neo_agent_core::multi_agent::AgentPathKind::Root,
    );
    let second = ctx.multi_agent.start_delegate(
        "second page candidate",
        None,
        AgentRole::Coder,
        AgentRunMode::Background,
        neo_agent_core::multi_agent::DelegateContext::None,
        neo_agent_core::multi_agent::AgentPathKind::Root,
    );
    ctx.background_tasks.start_delegate(first.clone()).await;
    ctx.background_tasks.start_delegate(second.clone()).await;
    let tools = ToolRegistry::with_builtin_tools();

    let first_page = tools
        .run(
            "ListDelegates",
            &ctx,
            serde_json::json!({
                "kind": "agent",
                "include_completed": true,
                "limit": 1,
                "order": "newest"
            }),
        )
        .await
        .expect("first page should succeed");
    let first_details = first_page.details.as_ref().expect("first page details");
    let first_rows = first_details["delegates"].as_array().expect("delegates");
    assert_eq!(first_rows.len(), 1);
    let first_id = first_rows[0]["id"].as_str().expect("id");
    let cursor = first_details["next_cursor"]
        .as_str()
        .expect("first page should include next_cursor");

    let second_page = tools
        .run(
            "ListDelegates",
            &ctx,
            serde_json::json!({
                "kind": "agent",
                "include_completed": true,
                "limit": 1,
                "order": "newest",
                "cursor": cursor
            }),
        )
        .await
        .expect("second page should succeed");
    let second_details = second_page.details.as_ref().expect("second page details");
    let second_rows = second_details["delegates"].as_array().expect("delegates");
    assert_eq!(second_rows.len(), 1);
    let second_id = second_rows[0]["id"].as_str().expect("id");

    assert_ne!(first_id, second_id, "cursor page repeated the same row");
}

#[tokio::test]
async fn list_delegates_treats_blank_cursor_as_first_page_but_rejects_zero() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_access(ToolAccess::all());
    let agent = ctx.multi_agent.start_delegate(
        "blank cursor candidate",
        None,
        AgentRole::Coder,
        AgentRunMode::Background,
        neo_agent_core::multi_agent::DelegateContext::None,
        neo_agent_core::multi_agent::AgentPathKind::Root,
    );
    ctx.background_tasks.start_delegate(agent).await;
    let tools = ToolRegistry::with_builtin_tools();

    for cursor in ["", "   "] {
        let page = tools
            .run(
                "ListDelegates",
                &ctx,
                serde_json::json!({
                    "kind": "agent",
                    "include_completed": true,
                    "limit": 1,
                    "order": "newest",
                    "cursor": cursor
                }),
            )
            .await
            .expect("blank cursor should select the first page");
        let rows = page
            .details
            .as_ref()
            .and_then(|details| details["delegates"].as_array())
            .expect("delegate rows");
        assert_eq!(
            rows.len(),
            1,
            "cursor {cursor:?} should return the first page"
        );
    }

    let error = tools
        .run(
            "ListDelegates",
            &ctx,
            serde_json::json!({
                "kind": "agent",
                "include_completed": true,
                "limit": 1,
                "order": "newest",
                "cursor": "0"
            }),
        )
        .await
        .expect_err("a fabricated zero cursor must be rejected");
    assert!(
        error
            .to_string()
            .contains("cursor must be a ListDelegates next_cursor value")
    );
}

#[tokio::test]
async fn wait_delegate_times_out_without_completion() {
    use neo_agent_core::tools::ToolContext;
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path()).unwrap();
    let agent = ctx
        .multi_agent
        .start_foreground_delegate_for_test("long running task");
    ctx.background_tasks.start_delegate(agent.clone()).await;

    let result = ToolRegistry::with_builtin_tools()
        .run(
            "WaitDelegate",
            &ctx,
            serde_json::json!({ "ids": [agent.id.as_str()], "timeout_ms": 1 }),
        )
        .await
        .expect("wait should return timeout result");

    assert!(result.content.contains("timed_out"));
}

#[tokio::test]
async fn list_delegates_can_filter_swarms_and_orders_newest_first() {
    let (registry, ctx) = registry_with_multi_agent();
    registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "first swarm",
                "items": [{"title": "a", "value": "a"}],
                "prompt_template": "inspect {{item}}",
                "mode": "background"
            }),
        )
        .await
        .expect("first swarm starts");
    let second = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "second swarm",
                "items": [{"title": "b", "value": "b"}],
                "prompt_template": "inspect {{item}}",
                "mode": "background"
            }),
        )
        .await
        .expect("second swarm starts");
    let second_id = second
        .details
        .as_ref()
        .and_then(|details| {
            details
                .get("swarm_id")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    details
                        .get("swarm")
                        .and_then(|swarm| swarm.get("swarm_id"))
                        .and_then(serde_json::Value::as_str)
                })
                .or_else(|| details.get("task_id").and_then(serde_json::Value::as_str))
        })
        .expect("swarm_id")
        .to_owned();

    // List with kind=swarm should return swarm rows.
    let listed = registry
        .run(
            "ListDelegates",
            &ctx,
            serde_json::json!({
                "kind": "swarm",
                "include_completed": true,
                "order": "newest"
            }),
        )
        .await
        .expect("list should succeed");

    // Both swarms should appear in swarm listing.
    assert!(
        listed.content.contains(second_id.as_str()),
        "{}",
        listed.content
    );
    assert!(listed.content.contains("kind: swarm"), "{}", listed.content);
    assert!(listed.content.contains("aggregate:"), "{}", listed.content);

    // kind=agent should not include swarms.
    let agents_only = registry
        .run(
            "ListDelegates",
            &ctx,
            serde_json::json!({
                "kind": "agent",
                "include_completed": true
            }),
        )
        .await
        .expect("list agents should succeed");
    assert!(
        !agents_only.content.contains("kind: swarm"),
        "{}",
        agents_only.content
    );
}

#[tokio::test]
async fn wait_and_task_output_return_swarm_aggregate_and_items() {
    let (registry, ctx) = registry_with_multi_agent();
    let started = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "read-only audit",
                "items": [{"title": "core", "value": "core"}, {"title": "tui", "value": "tui"}],
                "prompt_template": "Audit {{item}}",
                "mode": "foreground"
            }),
        )
        .await
        .expect("swarm starts");
    let swarm_id = started
        .details
        .as_ref()
        .and_then(|details| details.get("swarm_id"))
        .and_then(serde_json::Value::as_str)
        .expect("swarm_id")
        .to_owned();

    let waited = registry
        .run(
            "WaitDelegate",
            &ctx,
            serde_json::json!({ "ids": [swarm_id], "timeout_ms": 5000 }),
        )
        .await
        .expect("wait succeeds");
    assert!(
        waited
            .content
            .contains("\"kind\":\"delegate_swarm_result\""),
        "{}",
        waited.content
    );
    assert!(
        waited.content.contains("\"aggregate\":"),
        "{}",
        waited.content
    );
    assert!(waited.content.contains("\"items\":"), "{}", waited.content);

    let output = registry
        .run(
            "TaskOutput",
            &ctx,
            serde_json::json!({ "task_id": swarm_id, "block": false }),
        )
        .await
        .expect("task output succeeds");
    assert!(output.content.contains("kind: swarm"), "{}", output.content);
    assert!(output.content.contains("aggregate:"), "{}", output.content);
}

#[tokio::test]
async fn background_worker_panics_terminalize_delegate_and_swarm() {
    let registry = ToolRegistry::with_builtin_tools();

    // --- single background Delegate ---
    let ctx = progress_panic_context();
    let started = registry
        .run(
            "Delegate",
            &ctx,
            json!({
                "task": "will panic on progress",
                "mode": "background",
            }),
        )
        .await
        .expect("background delegate should start");
    let agent_id = started
        .details
        .as_ref()
        .and_then(|details| details.get("id"))
        .and_then(serde_json::Value::as_str)
        .expect("delegate agent id")
        .to_owned();

    let waited = registry
        .run(
            "WaitDelegate",
            &ctx,
            json!({ "ids": [agent_id.clone()], "timeout_ms": 5_000 }),
        )
        .await
        .expect("WaitDelegate should resolve after panic terminalization");
    assert!(
        waited.content.contains("\"status\":\"failed\""),
        "delegate wait content: {}",
        waited.content
    );

    let runtime_snapshot = ctx
        .multi_agent
        .agent_snapshot(&agent_id)
        .expect("canonical runtime snapshot");
    assert_eq!(runtime_snapshot.state, AgentLifecycleState::Failed);
    assert_eq!(
        runtime_snapshot.terminal_reason,
        Some(AgentTerminalReason::Error)
    );
    assert_eq!(
        runtime_snapshot
            .outcome
            .as_ref()
            .map(|outcome| outcome.summary.as_str()),
        Some("worker_panicked")
    );

    let bg = ctx
        .background_tasks
        .snapshot(&agent_id)
        .await
        .expect("background manager must mirror runtime");
    assert_eq!(bg.status.as_str(), "failed");
    let bg_agent = bg.delegate.expect("delegate snapshot on background task");
    assert_eq!(bg_agent.state, AgentLifecycleState::Failed);
    assert_eq!(
        bg_agent
            .outcome
            .as_ref()
            .map(|outcome| outcome.summary.as_str()),
        Some("worker_panicked")
    );

    let output = registry
        .run("TaskOutput", &ctx, json!({ "task_id": agent_id }))
        .await
        .expect("TaskOutput should read mirrored failed snapshot");
    assert!(
        output.content.contains("failed") || output.content.contains("worker_panicked"),
        "TaskOutput content: {}",
        output.content
    );

    // --- background DelegateSwarm with multiple children ---
    let ctx = progress_panic_context();
    let started = registry
        .run(
            "DelegateSwarm",
            &ctx,
            json!({
                "description": "panic swarm",
                "items": [
                    {"title": "child-a", "value": "a"},
                    {"title": "child-b", "value": "b"},
                ],
                "prompt_template": "Process {{item}}",
                "mode": "background",
                "max_concurrency": 2
            }),
        )
        .await
        .expect("background swarm should start");
    let swarm_id = started
        .details
        .as_ref()
        .and_then(|details| details.get("task_id"))
        .and_then(serde_json::Value::as_str)
        .expect("swarm task id")
        .to_owned();

    let waited = registry
        .run(
            "WaitDelegate",
            &ctx,
            json!({ "ids": [swarm_id.clone()], "timeout_ms": 5_000 }),
        )
        .await
        .expect("WaitDelegate should resolve after swarm panic terminalization");
    assert!(
        waited.content.contains("\"status\":\"failed\""),
        "swarm wait content: {}",
        waited.content
    );

    let swarm = ctx
        .multi_agent
        .swarm_snapshot(&swarm_id)
        .expect("canonical swarm snapshot");
    assert_eq!(swarm.state, AgentLifecycleState::Failed);
    assert!(
        swarm
            .children
            .iter()
            .all(|child| child.agent.state.is_terminal()),
        "every swarm child must be terminal after worker panic: {swarm:#?}"
    );
    assert!(
        swarm.children.iter().all(|child| !matches!(
            child.agent.state,
            AgentLifecycleState::Running | AgentLifecycleState::Queued
        )),
        "no swarm child may remain Running/Queued: {swarm:#?}"
    );
    assert!(
        swarm.children.iter().any(|child| {
            child.agent.state == AgentLifecycleState::Failed
                && child
                    .agent
                    .outcome
                    .as_ref()
                    .is_some_and(|outcome| outcome.summary == "worker_panicked")
        }),
        "at least one child must carry worker_panicked: {swarm:#?}"
    );
    for child in &swarm.children {
        let agent = ctx
            .multi_agent
            .agent_snapshot(child.agent.id.as_str())
            .expect("child agent in runtime");
        assert!(
            agent.state.is_terminal(),
            "runtime child {} still {:?}",
            child.agent.id.as_str(),
            agent.state
        );
    }

    let bg = ctx
        .background_tasks
        .snapshot(&swarm_id)
        .await
        .expect("background manager must mirror swarm");
    assert_eq!(bg.status.as_str(), "failed");
    let bg_swarm = bg.swarm.expect("swarm snapshot on background task");
    assert_eq!(bg_swarm.state, AgentLifecycleState::Failed);
    assert!(
        bg_swarm
            .children
            .iter()
            .all(|child| child.agent.state.is_terminal()),
        "background swarm mirror must terminalize children: {bg_swarm:#?}"
    );
}

fn registry_with_multi_agent() -> (ToolRegistry, ToolContext) {
    let turn_done = vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "msg_x".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "done".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ];
    let harness = FakeHarness::from_turns((0..10).map(|_| turn_done.clone()));
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_access(ToolAccess::all())
        .with_child_runtime(
            AgentConfig::for_model(harness.model())
                .with_permission_mode(PermissionMode::Yolo)
                .with_tool_execution_mode(ToolExecutionMode::Sequential),
            harness.client(),
            Arc::new(ToolRegistry::new()),
            1,
        );
    (ToolRegistry::with_builtin_tools(), ctx)
}

async fn task_output_for_background_delegate(
    registry: &ToolRegistry,
    ctx: &ToolContext,
    task: &str,
    context: &str,
) -> ToolResult {
    let delegate = registry
        .run(
            "Delegate",
            ctx,
            serde_json::json!({
                "task": task,
                "mode": "background",
                "context": context
            }),
        )
        .await
        .expect("background delegate should start");
    let agent_id = delegate
        .details
        .as_ref()
        .and_then(|details| details.get("agent_id"))
        .and_then(serde_json::Value::as_str)
        .expect("delegate result should include agent_id")
        .to_owned();

    registry
        .run(
            "TaskOutput",
            ctx,
            serde_json::json!({ "task_id": agent_id }),
        )
        .await
        .expect("TaskOutput should return delegate output")
}

fn assert_task_output_context(output: &ToolResult, expected: &str) {
    assert!(
        output
            .content
            .contains(&format!("\"context_mode\":\"{expected}\"")),
        "{}",
        output.content
    );
    assert_eq!(
        output
            .details
            .as_ref()
            .and_then(|details| details.get("context_mode"))
            .and_then(serde_json::Value::as_str),
        Some(expected)
    );
}

fn normal_turn_events() -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "msg_panic_test".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "working".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]
}

/// Panic on the first live progress event so the panic lands on the
/// supervised background worker task (not the fire-and-forget model turn
/// task nested inside AgentRuntime).
fn panic_on_worker_progress() -> Arc<dyn Fn(AgentEvent) + Send + Sync> {
    Arc::new(move |event: AgentEvent| match event {
        AgentEvent::DelegateProgressUpdated { .. }
        | AgentEvent::DelegateSwarmUpdated { .. }
        | AgentEvent::DelegateSwarmProgressUpdated { .. } => {
            panic!("delegate worker test panic");
        }
        _ => {}
    })
}

fn progress_panic_context() -> ToolContext {
    let harness = FakeHarness::from_turns((0..16).map(|_| normal_turn_events()));
    let dir = tempfile::tempdir().unwrap();
    ToolContext::new(dir.path())
        .unwrap()
        .with_access(ToolAccess::all())
        .with_child_runtime(
            AgentConfig::for_model(harness.model())
                .with_permission_mode(PermissionMode::Yolo)
                .with_tool_execution_mode(ToolExecutionMode::Sequential),
            harness.client(),
            Arc::new(ToolRegistry::new()),
            1,
        )
        .with_tool_event(panic_on_worker_progress())
}
