use futures::StreamExt;
use neo_agent_core::harness::FakeHarness;
use neo_agent_core::multi_agent::{
    AgentLifecycleState, AgentRole, AgentRunMode, AgentTerminalReason, DEFAULT_AGENT_NAMES,
    DisplayNamePool, MultiAgentRuntime, SwarmAggregate,
};
use neo_agent_core::tools::{ToolContext, ToolRegistry, ToolResult};
use neo_agent_core::{AgentConfig, AgentEvent, PermissionMode, ToolExecutionMode};
use neo_ai::{AiStreamEvent, ChatMessage, ContentPart, StopReason};
use std::sync::{Arc, Mutex};

#[test]
fn display_name_pool_is_deterministic() {
    let mut pool = DisplayNamePool::default();

    let first = pool.next_name();
    let second = pool.next_name();
    let third = pool.next_name();

    assert_eq!(first.as_str(), DEFAULT_AGENT_NAMES[0]);
    assert_eq!(second.as_str(), DEFAULT_AGENT_NAMES[1]);
    assert_eq!(third.as_str(), DEFAULT_AGENT_NAMES[2]);
}

#[test]
fn display_name_pool_combines_names_after_default_names() {
    let mut pool = DisplayNamePool::default();
    for _ in 0..DEFAULT_AGENT_NAMES.len() {
        let _ = pool.next_name();
    }

    let combined = pool.next_name();

    assert_eq!(
        combined.as_str(),
        format!("{}{}", DEFAULT_AGENT_NAMES[0], DEFAULT_AGENT_NAMES[1])
    );
}

#[test]
fn child_runtime_deps_seed_permission_from_the_live_mode() {
    let harness = FakeHarness::from_turns([]);
    let live_mode = Arc::new(std::sync::RwLock::new(PermissionMode::Ask));
    let config = AgentConfig::for_model(harness.model())
        .with_permission_mode(PermissionMode::Yolo)
        .with_live_permission_mode(Arc::clone(&live_mode));
    *live_mode.write().expect("live permission mode") = PermissionMode::Auto;

    let deps = neo_agent_core::multi_agent::ChildRuntimeDeps::new(
        config,
        harness.client(),
        Arc::new(ToolRegistry::new()),
    );

    assert_eq!(deps.config.permission_mode, PermissionMode::Auto);
}

#[test]
fn foreground_delegate_lifecycle_records_running_and_completed_state() {
    let runtime = MultiAgentRuntime::new();

    let running = runtime.start_foreground_delegate_for_test("inspect queue");
    assert_eq!(running.state, AgentLifecycleState::Running);
    assert_eq!(running.display_name.as_str(), DEFAULT_AGENT_NAMES[0]);

    let completed = runtime.complete_delegate_for_test(&running.id, "queue is safe");
    assert_eq!(completed.state, AgentLifecycleState::Completed);
    assert_eq!(
        completed
            .outcome
            .as_ref()
            .map(|outcome| outcome.summary.as_str()),
        Some("queue is safe")
    );
}

#[test]
fn agent_snapshot_records_timestamps_detach_origin_and_terminal_reason() {
    let runtime = MultiAgentRuntime::new();
    let snapshot = runtime.start_foreground_delegate_for_test("inspect docs");

    assert!(snapshot.created_at_ms > 0);
    assert!(snapshot.updated_at_ms >= snapshot.created_at_ms);
    assert!(snapshot.started_at_ms.is_some());
    assert_eq!(snapshot.terminal_at_ms, None);
    assert!(!snapshot.detached_from_foreground);
    assert_eq!(snapshot.terminal_reason, None);

    let detached = runtime
        .detach_agent(&snapshot.id)
        .expect("detach running agent");
    assert!(detached.detached_from_foreground);
    assert_eq!(detached.state, AgentLifecycleState::Running);

    let completed = runtime.complete_delegate_for_test(&snapshot.id, "done");
    assert_eq!(completed.state, AgentLifecycleState::Completed);
    assert_eq!(
        completed.terminal_reason,
        Some(AgentTerminalReason::Completed)
    );
    assert!(completed.terminal_at_ms.is_some());
}

#[test]
fn agent_snapshot_records_run_metadata_and_resume_origin() {
    let runtime = MultiAgentRuntime::new();
    let first = runtime.start_foreground_delegate_for_test("inspect mvcc");

    assert_eq!(first.run_count, 1);
    assert_eq!(first.live_messages_received, 0);
    assert_eq!(first.previous_status, None);
    assert_eq!(first.resumed_from, None);

    let completed = runtime.complete_delegate_for_test(&first.id, "mvcc summary");
    assert_eq!(completed.state, AgentLifecycleState::Completed);

    let request = neo_agent_core::multi_agent::DelegateRequest {
        task: "continue with wraparound".to_owned(),
        resume: Some(first.id.as_str().to_owned()),
        title: None,
        role: None,
        mode: AgentRunMode::Foreground,
        context: neo_agent_core::multi_agent::DelegateContext::Inherit,
        output_schema: None,
    };
    let resumed = runtime
        .start_resume_delegate(first.id.as_str(), &request)
        .expect("completed agent can be resumed");

    assert_eq!(resumed.run_count, 2);
    assert_eq!(resumed.live_messages_received, 0);
    assert_eq!(
        resumed.previous_status,
        Some(AgentLifecycleState::Completed)
    );
    assert_eq!(
        resumed
            .resumed_from
            .as_ref()
            .map(neo_agent_core::multi_agent::AgentId::as_str),
        Some(first.id.as_str())
    );
    assert_eq!(resumed.state, AgentLifecycleState::Running);
}

#[test]
fn replayed_delegate_snapshot_can_be_resumed_after_session_restore() {
    use neo_agent_core::multi_agent::{DelegateContext, DelegateRequest};

    let runtime = MultiAgentRuntime::new();
    let snapshot = runtime.start_foreground_delegate_for_test("audit session paths");
    let agent_id = snapshot.id.as_str().to_owned();
    let events = [AgentEvent::DelegateFinished {
        turn: 3,
        agent: snapshot,
        workflow_origin: None,
    }];

    let restored = MultiAgentRuntime::new();
    restored.restore_from_replay(events.iter());

    let request = DelegateRequest {
        task: "continue audit".to_owned(),
        resume: Some(agent_id.clone()),
        title: None,
        role: None,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::Inherit,
        output_schema: None,
    };
    let resumed = restored
        .start_resume_delegate(&agent_id, &request)
        .expect("resume restored agent");

    assert_eq!(resumed.id.as_str(), agent_id);
    assert_eq!(resumed.run_count, 2);
}

#[test]
fn replayed_running_delegate_is_marked_lost_and_can_be_resumed() {
    use neo_agent_core::multi_agent::{DelegateContext, DelegateRequest};

    let runtime = MultiAgentRuntime::new();
    let snapshot = runtime.start_foreground_delegate_for_test("resume interrupted audit");
    let agent_id = snapshot.id.as_str().to_owned();
    let events = [AgentEvent::DelegateStarted {
        turn: 3,
        agent: snapshot,
        workflow_origin: None,
    }];

    let restored = MultiAgentRuntime::new();
    restored.restore_from_replay(events.iter());

    let lost = restored
        .agent_snapshot(&agent_id)
        .expect("restored agent snapshot");
    assert_eq!(lost.state, AgentLifecycleState::Interrupted);
    assert_eq!(
        lost.terminal_reason,
        Some(AgentTerminalReason::ProcessExited)
    );
    assert_eq!(
        lost.outcome.as_ref().map(|outcome| outcome.is_error),
        Some(true)
    );

    let request = DelegateRequest {
        task: "continue interrupted audit".to_owned(),
        resume: Some(agent_id.clone()),
        title: None,
        role: None,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::Inherit,
        output_schema: None,
    };
    let resumed = restored
        .start_resume_delegate(&agent_id, &request)
        .expect("resume lost restored agent");

    assert_eq!(resumed.id.as_str(), agent_id);
    assert_eq!(resumed.run_count, 2);
    assert_eq!(
        resumed.previous_status,
        Some(AgentLifecycleState::Interrupted)
    );
    assert_eq!(resumed.state, AgentLifecycleState::Running);
}

#[tokio::test]
async fn resumed_child_turn_replays_prior_messages_from_agent_wire() {
    use neo_agent_core::{
        multi_agent::{ChildRuntimeDeps, DelegateContext, DelegateRequest},
        session::{SessionState, SessionStateStore},
    };

    let temp = tempfile::tempdir().expect("tempdir");
    let session_dir = temp.path();
    let mut state = SessionState::new();
    state.ensure_main_agent();
    SessionStateStore::new(session_dir)
        .write(&state)
        .expect("state");

    let runtime = MultiAgentRuntime::new().with_session_directory(session_dir.to_path_buf());
    let harness = FakeHarness::from_turns([
        child_text_turn("first child answer"),
        child_text_turn("second child answer"),
    ]);
    let deps = ChildRuntimeDeps::new(
        AgentConfig::for_model(harness.model()),
        harness.client(),
        Arc::new(ToolRegistry::new()),
    );
    let first_request = DelegateRequest {
        task: "first task".to_owned(),
        resume: None,
        title: None,
        role: None,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::None,
        output_schema: None,
    };
    let first_output = runtime
        .run_child_turn(deps.clone(), &first_request, AgentRunMode::Foreground)
        .await
        .expect("first child run");

    let mut replayed_snapshot = first_output.snapshot;
    let agent_id = replayed_snapshot.id.as_str().to_owned();
    replayed_snapshot.prior_messages.clear();
    let restored = MultiAgentRuntime::new().with_session_directory(session_dir.to_path_buf());
    let events = [AgentEvent::DelegateFinished {
        turn: 1,
        agent: replayed_snapshot,
        workflow_origin: None,
    }];
    restored.restore_from_replay(events.iter());

    let resume_request = DelegateRequest {
        task: "second task".to_owned(),
        resume: Some(agent_id.clone()),
        title: None,
        role: None,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::None,
        output_schema: None,
    };
    let resumed = restored
        .start_resume_delegate(&agent_id, &resume_request)
        .expect("start resume");
    let _ = restored
        .run_started_child_turn(deps, resumed, DelegateContext::None, |_| {})
        .await;

    let requests = harness.requests();
    assert_eq!(requests.len(), 2, "{requests:#?}");
    let resumed_messages = request_text(&requests[1].messages);
    assert!(
        resumed_messages.contains("first child answer"),
        "{resumed_messages}"
    );
    assert!(
        resumed_messages.contains("second task"),
        "{resumed_messages}"
    );
}

#[tokio::test]
async fn concurrent_swarm_child_runs_preserve_all_state_records() {
    use neo_agent_core::{
        multi_agent::{ChildRuntimeDeps, DelegateSwarmItem, DelegateSwarmRequest},
        session::{SessionAgentKind, SessionState, SessionStateStore},
    };

    let temp = tempfile::tempdir().expect("tempdir");
    let session_dir = temp.path();
    let mut state = SessionState::new();
    state.ensure_main_agent();
    SessionStateStore::new(session_dir)
        .write(&state)
        .expect("state");

    let runtime = MultiAgentRuntime::new().with_session_directory(session_dir.to_path_buf());
    let harness = FakeHarness::from_turns([
        child_text_turn("core ok"),
        child_text_turn("tui ok"),
        child_text_turn("runtime ok"),
    ]);
    let deps = ChildRuntimeDeps::new(
        AgentConfig::for_model(harness.model()),
        harness.client(),
        Arc::new(ToolRegistry::new()),
    );
    let request = DelegateSwarmRequest {
        description: "inspect modules".to_owned(),
        items: vec![
            DelegateSwarmItem {
                title: "core".to_owned(),
                value: "core".to_owned(),
            },
            DelegateSwarmItem {
                title: "tui".to_owned(),
                value: "tui".to_owned(),
            },
            DelegateSwarmItem {
                title: "runtime".to_owned(),
                value: "runtime".to_owned(),
            },
        ],
        prompt_template: Some("Check {{item}}".to_owned()),
        resume_agent_ids: std::collections::BTreeMap::new(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        max_concurrency: Some(3),
    };
    let swarm_id = runtime.new_swarm_id();
    let outputs = futures::stream::iter(request.items.iter().map(|item| {
        runtime.run_swarm_child_turn(
            deps.clone(),
            &request,
            &swarm_id,
            item.value.as_str(),
            AgentRunMode::Foreground,
        )
    }))
    .buffer_unordered(3)
    .collect::<Vec<_>>()
    .await
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .expect("swarm children");

    let state = SessionStateStore::new(session_dir)
        .read()
        .await
        .expect("read state");
    for output in outputs {
        let record = state
            .agents
            .get(output.snapshot.id.as_str())
            .expect("child record should survive concurrent registration");
        assert_eq!(record.kind, SessionAgentKind::Sub);
        assert_eq!(record.swarm_id.as_deref(), Some(swarm_id.as_str()));
    }
}

#[test]
fn replayed_swarm_marks_running_children_lost_and_refreshes_aggregate() {
    let runtime = MultiAgentRuntime::new();
    let swarm_id = runtime.create_swarm_for_test(vec![
        ("interrupted child", AgentLifecycleState::Running),
        ("completed child", AgentLifecycleState::Completed),
    ]);
    let snapshot = runtime
        .swarm_snapshot(&swarm_id)
        .expect("source swarm snapshot");
    let events = [AgentEvent::DelegateSwarmStarted {
        turn: 4,
        swarm: snapshot,
        workflow_origin: None,
    }];

    let restored = MultiAgentRuntime::new();
    restored.restore_from_replay(events.iter());

    let restored_swarm = restored
        .swarm_snapshot(&swarm_id)
        .expect("restored swarm snapshot");
    assert_eq!(restored_swarm.state, AgentLifecycleState::Cancelled);
    assert_eq!(restored_swarm.aggregate.total, 2);
    assert_eq!(restored_swarm.aggregate.running, 0);
    assert_eq!(restored_swarm.aggregate.cancelled, 1);
    assert_eq!(restored_swarm.aggregate.completed, 1);

    let interrupted = &restored_swarm.children[0].agent;
    assert_eq!(interrupted.state, AgentLifecycleState::Interrupted);
    assert_eq!(
        interrupted.terminal_reason,
        Some(AgentTerminalReason::ProcessExited)
    );
    let completed = &restored_swarm.children[1].agent;
    assert_eq!(completed.state, AgentLifecycleState::Completed);
    assert_eq!(
        completed.terminal_reason,
        Some(AgentTerminalReason::Completed)
    );
    assert_eq!(
        restored
            .agent_snapshot(interrupted.id.as_str())
            .map(|agent| agent.state),
        Some(AgentLifecycleState::Interrupted)
    );
    assert_eq!(
        restored
            .agent_snapshot(completed.id.as_str())
            .map(|agent| agent.state),
        Some(AgentLifecycleState::Completed)
    );
}

#[test]
fn background_terminal_reason_records_lost_without_claiming_completion() {
    let runtime = MultiAgentRuntime::new();
    let snapshot = runtime.start_foreground_delegate_for_test("background work");
    let detached = runtime
        .detach_agent(&snapshot.id)
        .expect("detach running agent");
    assert!(detached.detached_from_foreground);

    let lost = runtime
        .mark_background_terminal_reason(
            &snapshot.id,
            AgentLifecycleState::Failed,
            AgentTerminalReason::Lost,
            Some("Background agent lost (session restarted before completion)".to_owned()),
        )
        .expect("lost update");

    assert_eq!(lost.state, AgentLifecycleState::Failed);
    assert_eq!(lost.terminal_reason, Some(AgentTerminalReason::Lost));
    assert!(lost.terminal_at_ms.is_some());
    assert_eq!(
        lost.outcome.as_ref().map(|outcome| outcome.is_error),
        Some(true)
    );
}

#[test]
fn builtin_tools_register_delegate_tools() {
    let specs = ToolRegistry::with_builtin_tools()
        .specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();

    assert!(specs.iter().any(|name| name == "Delegate"));
    assert!(specs.iter().any(|name| name == "DelegateSwarm"));
}

#[tokio::test]
async fn delegate_resume_rejects_role_override() {
    let (registry, ctx) = registry_with_multi_agent();

    let result = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "resume": "agent_existing",
                "role": "coder",
                "task": "continue"
            }),
        )
        .await
        .expect("tool should return validation result");

    assert!(result.is_error);
    assert!(
        result
            .content
            .contains("role must be omitted when resume is set"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn delegate_resume_rejects_swarm_id() {
    let (registry, ctx) = registry_with_multi_agent();

    let result = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "resume": "swarm_123",
                "task": "continue"
            }),
        )
        .await
        .expect("tool should return validation result");

    assert!(result.is_error);
    assert!(
        result.content.contains("resume must be an agent_id"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn delegate_resume_reuses_agent_identity_and_role() {
    let (registry, ctx) = registry_with_multi_agent();
    let first = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "first investigation",
                "role": "explorer",
                "mode": "foreground"
            }),
        )
        .await
        .expect("first delegate should complete");
    let agent_id = first
        .details
        .as_ref()
        .and_then(|details| details.get("agent_id"))
        .and_then(serde_json::Value::as_str)
        .expect("first delegate should expose agent_id")
        .to_owned();

    let second = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "resume": agent_id,
                "task": "continue with one more check",
                "mode": "foreground"
            }),
        )
        .await
        .expect("resume should complete");

    let details = second.details.as_ref().expect("resume details");
    assert_eq!(
        details.get("agent_id").and_then(serde_json::Value::as_str),
        Some(agent_id.as_str())
    );
    assert_eq!(
        details
            .get("actual_role")
            .and_then(serde_json::Value::as_str),
        Some("explorer")
    );
    assert_eq!(details["run_index"], 2);
    assert_eq!(details["run_count"], 2);
    assert_eq!(details["resumed_from"], agent_id.as_str());
    assert_eq!(details["previous_status"], "completed");
    assert_eq!(details["summary_scope"], "current_run");
    assert!(
        second.content.contains("previous_status: completed"),
        "{}",
        second.content
    );
    assert!(
        second.content.contains("status: completed"),
        "{}",
        second.content
    );
}

#[tokio::test]
async fn delegate_result_details_include_canonical_run_fields() {
    let (registry, ctx) = registry_with_multi_agent();

    let result = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "inspect result contract",
                "title": "Result contract",
                "context": "summary",
                "mode": "foreground"
            }),
        )
        .await
        .expect("delegate should complete");

    let details = result.details.as_ref().expect("delegate details");
    assert_eq!(details["kind"], "delegate");
    assert_eq!(details["mode"], "foreground");
    assert_eq!(details["status"], "completed");
    assert_eq!(details["title"], "Result contract");
    assert_eq!(details["context_mode"], "summary");
    assert_eq!(details["summary_scope"], "current_run");
    assert_eq!(details["run_index"], 1);
    assert_eq!(details["run_count"], 1);
    assert!(details["created_at_ms"].as_u64().is_some(), "{details}");
    assert!(details["started_at_ms"].as_u64().is_some(), "{details}");
    assert!(details["terminal_at_ms"].as_u64().is_some(), "{details}");
    assert!(details["elapsed_ms"].as_u64().is_some(), "{details}");
    assert!(details["tool_count"].as_u64().is_some(), "{details}");
    assert!(details["token_count"].as_u64().is_some(), "{details}");
    assert!(
        details.get("agent").is_none(),
        "old nested agent field should be gone: {details}"
    );
}

#[tokio::test]
async fn list_delegates_defaults_to_meta_only_rows_with_title() {
    let (registry, ctx) = registry_with_multi_agent();
    let _ = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "long prompt body that should not appear in default list",
                "title": "Short title",
                "mode": "foreground"
            }),
        )
        .await
        .expect("delegate should complete");

    let result = registry
        .run(
            "ListDelegates",
            &ctx,
            serde_json::json!({
                "include_completed": true,
                "kind": "agent"
            }),
        )
        .await
        .expect("list should succeed");

    let details = result.details.as_ref().expect("list details");
    assert_eq!(details["include"], serde_json::json!(["meta"]));
    let row = details["delegates"][0].as_object().expect("first row");
    assert_eq!(row["title"], "Short title");
    assert!(row.get("task").is_none(), "{row:#?}");
    assert!(row.get("summary").is_none(), "{row:#?}");
    assert!(
        !result.content.contains("long prompt body"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn list_delegates_includes_requested_summary_in_model_content() {
    let (registry, ctx) = registry_with_multi_agent();
    let _ = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "inspect summary output",
                "title": "Summary contract",
                "mode": "foreground"
            }),
        )
        .await
        .expect("delegate should complete");

    let result = registry
        .run(
            "ListDelegates",
            &ctx,
            serde_json::json!({
                "include_completed": true,
                "kind": "agent",
                "include": ["summary"]
            }),
        )
        .await
        .expect("list should succeed");

    let details = result.details.as_ref().expect("list details");
    assert_eq!(details["include"], serde_json::json!(["summary"]));
    let summary = details["delegates"][0]["summary"]
        .as_str()
        .filter(|summary| !summary.is_empty())
        .expect("completed delegate summary");
    assert!(
        result.content.contains(&format!("summary: {summary}")),
        "summary missing from model-facing content: {}",
        result.content
    );
}

#[tokio::test]
async fn list_delegates_rejects_cursor_reused_with_different_query() {
    let (registry, ctx) = registry_with_multi_agent();
    for index in 0..4 {
        let _ = registry
            .run(
                "Delegate",
                &ctx,
                serde_json::json!({
                    "task": format!("task {index}"),
                    "mode": "foreground"
                }),
            )
            .await
            .expect("delegate should complete");
    }

    let first_page = registry
        .run(
            "ListDelegates",
            &ctx,
            serde_json::json!({
                "include_completed": true,
                "state": "completed",
                "order": "oldest",
                "limit": 2
            }),
        )
        .await
        .expect("first page should succeed");
    let cursor = first_page.details.as_ref().unwrap()["next_cursor"]
        .as_str()
        .expect("next cursor")
        .to_owned();

    let mismatched = registry
        .run(
            "ListDelegates",
            &ctx,
            serde_json::json!({
                "include_completed": true,
                "order": "oldest",
                "limit": 2,
                "cursor": cursor
            }),
        )
        .await;

    let err = mismatched.expect_err("mismatched cursor should be rejected");
    assert!(
        err.to_string().contains("different ListDelegates query"),
        "{err}"
    );
}

#[tokio::test]
async fn wait_delegate_timeout_preserves_completed_partial_results() {
    let runtime = MultiAgentRuntime::new();
    let completed = runtime.start_foreground_delegate_for_test("already completed");
    let _ = runtime.complete_delegate_for_test(&completed.id, "finished summary");
    let running = runtime.start_foreground_delegate_for_test("still running");
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_multi_agent(runtime);
    let registry = ToolRegistry::with_builtin_tools();

    let result = registry
        .run(
            "WaitDelegate",
            &ctx,
            serde_json::json!({
                "ids": [completed.id.as_str(), running.id.as_str()],
                "timeout_ms": 1
            }),
        )
        .await
        .expect("wait should return timeout result");

    let details = result.details.as_ref().expect("wait details");
    assert_eq!(details["kind"], "delegate_wait");
    assert_eq!(details["outcome"], "wait_timed_out");
    assert_eq!(details["aggregate"]["total"], 2);
    assert_eq!(details["aggregate"]["terminal"], 1);
    assert_eq!(details["aggregate"]["pending"], 1);
    assert_eq!(details["items"][0]["id"], completed.id.as_str());
    assert_eq!(details["items"][0]["status"], "completed");
    assert_eq!(details["items"][0]["summary"], "finished summary");
    assert_eq!(details["items"][1]["id"], running.id.as_str());
    assert_eq!(details["items"][1]["status"], "running");
    let content: serde_json::Value = serde_json::from_str(&result.content).expect("wait JSON");
    assert_eq!(content["next_actions"][0]["tool"], "WaitDelegate");
    assert!(!result.content.contains("Sleep"));
    assert!(!result.content.contains("ListDelegates"));
}

#[test]
fn multi_agent_tool_descriptions_explain_contract_without_docs() {
    let registry = ToolRegistry::with_builtin_tools_and_todos(Arc::new(Mutex::new(Vec::new())));
    let specs = registry.specs();
    let spec = |name: &str| {
        specs
            .iter()
            .find(|spec| spec.name == name)
            .unwrap_or_else(|| panic!("{name} spec registered"))
    };

    assert_delegate_description(&spec("Delegate").description);
    assert_message_delegate_description(&spec("MessageDelegate").description);
    assert_list_delegates_description(&spec("ListDelegates").description);
    let wait = spec("WaitDelegate");
    assert_wait_delegate_contract(&wait.description, &wait.input_schema);
    assert_delegate_swarm_description(&spec("DelegateSwarm").description);
}

#[test]
fn swarm_aggregate_counts_child_states_and_derives_status() {
    let aggregate = SwarmAggregate::from_states([
        AgentLifecycleState::Completed,
        AgentLifecycleState::Failed,
        AgentLifecycleState::Cancelled,
        AgentLifecycleState::Queued,
    ]);

    assert_eq!(aggregate.total, 4);
    assert_eq!(aggregate.completed, 1);
    assert_eq!(aggregate.failed, 1);
    assert_eq!(aggregate.cancelled, 1);
    assert_eq!(aggregate.queued, 1);
    assert_eq!(aggregate.status(), AgentLifecycleState::Queued);
}

#[tokio::test]
async fn runtime_keeps_swarm_entity_after_foreground_completion() {
    let (registry, ctx) = registry_with_multi_agent();

    let result = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "count files",
                "items": [
                    {"title": "a", "value": "a"},
                    {"title": "b", "value": "b"}
                ],
                "prompt_template": "Inspect {{item}} for {{description}}",
                "mode": "foreground"
            }),
        )
        .await
        .expect("swarm should complete");

    let swarm_id = result
        .details
        .as_ref()
        .and_then(|details| details.get("swarm_id"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            result
                .details
                .as_ref()
                .and_then(|details| details.get("swarm"))
                .and_then(|swarm| swarm.get("swarm_id"))
                .and_then(serde_json::Value::as_str)
        })
        .expect("swarm_id");
    let snapshot = ctx
        .multi_agent
        .swarm_snapshot(swarm_id)
        .expect("swarm remains queryable");

    assert_eq!(snapshot.swarm_id, swarm_id);
    assert_eq!(snapshot.aggregate.total, 2);
    assert_eq!(snapshot.state, AgentLifecycleState::Completed);
}

#[tokio::test]
async fn delegate_swarm_resume_agent_ids_restarts_existing_children() {
    let (registry, ctx) = registry_with_multi_agent();
    let first = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "initial child",
                "mode": "foreground"
            }),
        )
        .await
        .expect("delegate should complete");
    let agent_id = first
        .details
        .as_ref()
        .and_then(|details| details.get("agent_id"))
        .and_then(serde_json::Value::as_str)
        .expect("agent_id")
        .to_owned();

    let mut resume_map = serde_json::Map::new();
    resume_map.insert(
        agent_id.clone(),
        serde_json::Value::String("continue inside swarm".to_owned()),
    );
    let swarm = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "resume unfinished child",
                "resume_agent_ids": serde_json::Value::Object(resume_map),
                "mode": "foreground"
            }),
        )
        .await
        .expect("swarm resume should complete");

    assert!(!swarm.is_error, "{}", swarm.content);
    let items = swarm
        .details
        .as_ref()
        .and_then(|details| details.get("items"))
        .and_then(serde_json::Value::as_array)
        .expect("swarm details include items");
    assert!(
        items.iter().any(|item| item["agent_id"] == agent_id),
        "{items:#?}"
    );
}

#[tokio::test]
async fn delegate_swarm_invalid_late_resume_is_atomic() {
    let (registry, ctx) = registry_with_multi_agent();
    let first = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "initial child",
                "mode": "foreground"
            }),
        )
        .await
        .expect("delegate should complete");
    let agent_id = first
        .details
        .as_ref()
        .and_then(|details| details.get("agent_id"))
        .and_then(serde_json::Value::as_str)
        .expect("agent_id")
        .to_owned();

    let before_agents = ctx.multi_agent.list_agents(true);
    let before_swarms = ctx.multi_agent.list_swarms();
    assert!(!before_agents.is_empty());

    let mut resume_map = serde_json::Map::new();
    resume_map.insert(
        agent_id.clone(),
        serde_json::Value::String("resume valid".to_owned()),
    );
    resume_map.insert(
        "agent_unknown".to_owned(),
        serde_json::Value::String("resume invalid".to_owned()),
    );
    let result = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "late invalid resume",
                "items": [{"title": "new", "value": "new task"}],
                "prompt_template": "Do {{item}}",
                "resume_agent_ids": serde_json::Value::Object(resume_map),
                "mode": "foreground"
            }),
        )
        .await;

    let result = result.unwrap_or_else(|err| ToolResult::error(err.to_string()));
    assert!(result.is_error);
    assert!(
        result
            .content
            .contains("unknown delegate target `agent_unknown`"),
        "{}",
        result.content
    );

    assert_eq!(
        ctx.multi_agent.list_agents(true),
        before_agents,
        "agent list must not change after failed swarm preparation"
    );
    assert_eq!(
        ctx.multi_agent.list_swarms(),
        before_swarms,
        "swarm list must not change after failed swarm preparation"
    );
    assert!(
        ctx.multi_agent.list_swarms().is_empty(),
        "no swarm should be registered"
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
    let ctx = ToolContext::new(dir.path()).unwrap().with_child_runtime(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_tool_execution_mode(ToolExecutionMode::Sequential),
        harness.client(),
        std::sync::Arc::new(ToolRegistry::new()),
        1,
    );
    (ToolRegistry::with_builtin_tools(), ctx)
}

fn assert_delegate_description(description: &str) {
    assert!(
        description.contains("Default mode is foreground"),
        "{}",
        description
    );
    assert!(description.contains("resume"), "{}", description);
    assert!(
        description.contains("role must be omitted"),
        "{}",
        description
    );
    assert!(description.contains("context"), "{}", description);
}

fn assert_message_delegate_description(description: &str) {
    assert!(description.contains("live"), "{}", description);
    assert!(description.contains("agent or swarm"), "{}", description);
    assert!(description.contains("running children"), "{}", description);
    assert!(
        description.contains("Delegate with resume"),
        "{}",
        description
    );
}

fn assert_list_delegates_description(description: &str) {
    assert!(description.contains("active-only"), "{}", description);
    assert!(description.contains("meta-only"), "{}", description);
    assert!(
        description.contains("include_completed=true"),
        "{}",
        description
    );
    assert!(description.contains("same query"), "{}", description);
    assert!(description.contains("does not wait"), "{}", description);
    assert!(
        description.contains("Never poll it with Sleep"),
        "{}",
        description
    );
    assert!(description.contains("WaitDelegate"), "{}", description);
}

fn assert_wait_delegate_contract(description: &str, input_schema: &serde_json::Value) {
    assert!(
        description.contains("Canonical blocking wait"),
        "{}",
        description
    );
    assert!(description.contains("wait_timed_out"), "{}", description);
    assert!(
        description.contains("delegate itself reached timed_out"),
        "{}",
        description
    );
    assert!(description.contains("one global"), "{}", description);
    let wait_schema = input_schema.get("schema").unwrap_or(input_schema);
    let required = wait_schema["required"].as_array().expect("required fields");
    assert!(required.iter().any(|field| field == "ids"), "{wait_schema}");
    assert!(
        wait_schema["properties"].get("ids").is_some(),
        "{wait_schema}"
    );
    assert!(
        wait_schema["properties"].get("id").is_none(),
        "{wait_schema}"
    );
}

fn assert_delegate_swarm_description(description: &str) {
    assert!(description.contains("foreground"), "{}", description);
    assert!(description.contains("WaitDelegate"), "{}", description);
    assert!(description.contains("TaskOutput"), "{}", description);
}

fn child_text_turn(text: &str) -> Vec<AiStreamEvent> {
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

fn request_text(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .flat_map(|message| match message {
            ChatMessage::System { content }
            | ChatMessage::User { content }
            | ChatMessage::Assistant { content, .. }
            | ChatMessage::ToolResult { content, .. } => content.iter(),
        })
        .filter_map(|part| match part {
            ContentPart::Text { text } | ContentPart::Thinking { text, .. } => Some(text.as_str()),
            ContentPart::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
