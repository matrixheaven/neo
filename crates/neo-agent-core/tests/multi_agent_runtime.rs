use futures::{StreamExt, stream};
use neo_agent_core::harness::FakeHarness;
use neo_agent_core::multi_agent::{
    AgentActivityKind, AgentLifecycleState, AgentPathKind, AgentRole, AgentRunMode,
    AgentTerminalReason, AgentToolActivityPhase, AgentToolOutputPreview, DEFAULT_AGENT_NAMES,
    DisplayNamePool, MultiAgentRuntime, SwarmAggregate,
};
use neo_agent_core::tools::{ToolContext, ToolRegistry, ToolResult};
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, PermissionMode,
    ToolExecutionMode,
};
use neo_ai::{
    AiError, AiStreamEvent, ChatMessage, ChatRequest, ContentPart, ModelClient, StopReason,
};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

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
fn agent_tool_activity_uses_explicit_phase_and_output_preview() {
    let activity = AgentActivityKind::Tool {
        id: "call_1".to_owned(),
        name: "Bash".to_owned(),
        summary: Some("cargo nextest run -p neo-tui".to_owned()),
        phase: AgentToolActivityPhase::Ongoing,
        output: Some(AgentToolOutputPreview {
            text: "Compiling neo-tui v0.1.0".to_owned(),
            is_error: false,
            truncated: false,
            tail: true,
        }),
    };

    let serialized = serde_json::to_value(&activity).expect("serialize activity");

    assert_eq!(serialized["phase"], "ongoing");
    assert_eq!(serialized["output"]["tail"], true);
    assert!(
        serialized.get("failed").is_none(),
        "old failed bool must not remain in the canonical schema: {serialized}"
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
    let events = vec![AgentEvent::DelegateFinished {
        turn: 3,
        agent: snapshot,
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
    let events = vec![AgentEvent::DelegateStarted {
        turn: 3,
        agent: snapshot,
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
async fn child_run_appends_events_to_agent_wire() {
    use neo_agent_core::{
        multi_agent::{ChildRuntimeDeps, DelegateContext, DelegateRequest},
        session::{
            MAIN_AGENT_ID, SessionAgentKind, SessionState, SessionStateStore, agent_wire_path,
        },
    };

    let temp = tempfile::tempdir().expect("tempdir");
    let session_dir = temp.path();
    let mut state = SessionState::new();
    state.ensure_main_agent();
    SessionStateStore::new(session_dir)
        .write(&state)
        .await
        .expect("state");

    let runtime = MultiAgentRuntime::new().with_session_directory(session_dir.to_path_buf());
    let harness = FakeHarness::from_turns([child_text_turn("child done")]);
    let deps = ChildRuntimeDeps::new(
        AgentConfig::for_model(harness.model()),
        harness.client(),
        Arc::new(ToolRegistry::new()),
    );
    let request = DelegateRequest {
        task: "say done".to_owned(),
        resume: None,
        title: None,
        role: None,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::None,
    };

    let output = runtime
        .run_child_turn(deps, &request, AgentRunMode::Foreground)
        .await
        .expect("child run");
    let wire = agent_wire_path(session_dir, output.snapshot.id.as_str());

    assert!(
        wire.is_file(),
        "child wire should exist at {}",
        wire.display()
    );
    let replayed = neo_agent_core::session::JsonlSessionReader::read_all(&wire)
        .await
        .expect("read wire");
    assert!(
        replayed.iter().any(|event| matches!(
            event,
            AgentEvent::MessageAppended {
                message: AgentMessage::Assistant { .. }
            }
        )),
        "{replayed:#?}"
    );

    let state = SessionStateStore::new(session_dir)
        .read()
        .await
        .expect("read state");
    let record = state
        .agents
        .get(output.snapshot.id.as_str())
        .expect("subagent record");
    assert_eq!(record.kind, SessionAgentKind::Sub);
    assert_eq!(record.parent_agent_id.as_deref(), Some(MAIN_AGENT_ID));
    assert_eq!(record.role.as_deref(), Some("coder"));
    assert_eq!(
        record.record_dir,
        std::path::PathBuf::from("agents").join(output.snapshot.id.as_str())
    );
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
        .await
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
    };
    let first_output = runtime
        .run_child_turn(deps.clone(), &first_request, AgentRunMode::Foreground)
        .await
        .expect("first child run");

    let mut replayed_snapshot = first_output.snapshot;
    let agent_id = replayed_snapshot.id.as_str().to_owned();
    replayed_snapshot.prior_messages.clear();
    let restored = MultiAgentRuntime::new().with_session_directory(session_dir.to_path_buf());
    let events = vec![AgentEvent::DelegateFinished {
        turn: 1,
        agent: replayed_snapshot,
    }];
    restored.restore_from_replay(events.iter());

    let resume_request = DelegateRequest {
        task: "second task".to_owned(),
        resume: Some(agent_id.clone()),
        title: None,
        role: None,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::None,
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
async fn failed_child_run_flushes_partial_agent_wire() {
    use neo_agent_core::{
        multi_agent::{ChildRuntimeDeps, DelegateContext, DelegateRequest},
        session::{JsonlSessionReader, SessionState, SessionStateStore, agent_wire_path},
    };

    let temp = tempfile::tempdir().expect("tempdir");
    let session_dir = temp.path();
    let mut state = SessionState::new();
    state.ensure_main_agent();
    SessionStateStore::new(session_dir)
        .write(&state)
        .await
        .expect("state");

    let runtime = MultiAgentRuntime::new().with_session_directory(session_dir.to_path_buf());
    let harness = FakeHarness::from_result_turns([vec![
        Ok(AiStreamEvent::MessageStart {
            id: "child_partial".to_owned(),
        }),
        Ok(AiStreamEvent::TextDelta {
            text: "partial child answer".to_owned(),
        }),
        Err(AiError::Stream {
            message: "child stream failed".to_owned(),
        }),
    ]]);
    let deps = ChildRuntimeDeps::new(
        AgentConfig::for_model(harness.model()),
        harness.client(),
        Arc::new(ToolRegistry::new()),
    );
    let request = DelegateRequest {
        task: "fail after partial".to_owned(),
        resume: None,
        title: None,
        role: None,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::None,
    };

    let output = runtime
        .run_child_turn(deps, &request, AgentRunMode::Foreground)
        .await
        .expect("child run returns failed snapshot");
    assert_eq!(output.snapshot.state, AgentLifecycleState::Failed);
    let wire = agent_wire_path(session_dir, output.snapshot.id.as_str());
    let replayed = JsonlSessionReader::read_all(&wire)
        .await
        .expect("read partial wire");

    assert!(
        replayed.iter().any(|event| matches!(
            event,
            AgentEvent::MessageAppended {
                message: AgentMessage::User { .. }
            }
        )),
        "{replayed:#?}"
    );
    assert!(
        replayed
            .iter()
            .any(|event| matches!(event, AgentEvent::TextDelta { text, .. } if text == "partial child answer")),
        "{replayed:#?}"
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
        .await
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

#[tokio::test]
async fn child_run_uses_parent_cancellation_token() {
    use neo_agent_core::multi_agent::{ChildRuntimeDeps, DelegateContext, DelegateRequest};

    let cancel = CancellationToken::new();
    cancel.cancel();
    let harness = FakeHarness::from_turns([child_text_turn("should not run")]);
    let deps = ChildRuntimeDeps::new(
        AgentConfig::for_model(harness.model()),
        harness.client(),
        Arc::new(ToolRegistry::new()),
    )
    .with_cancel_token(cancel);
    let request = DelegateRequest {
        task: "cancel child".to_owned(),
        resume: None,
        title: None,
        role: None,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::None,
    };

    let output = MultiAgentRuntime::new()
        .run_child_turn(deps, &request, AgentRunMode::Foreground)
        .await
        .expect("child run returns failed snapshot");

    assert_eq!(output.snapshot.state, AgentLifecycleState::Cancelled);
    assert!(harness.requests().is_empty(), "{:#?}", harness.requests());
}

#[tokio::test]
async fn foreground_delegate_cancel_marks_child_cancelled_when_tool_future_is_dropped() {
    let multi_agent = MultiAgentRuntime::new();
    let model = Arc::new(DelayedTurnModel::new(vec![
        vec![
            DelayedStep::Event(AiStreamEvent::MessageStart {
                id: "parent".to_owned(),
            }),
            DelayedStep::Event(AiStreamEvent::ToolCallStart {
                id: "delegate_call".to_owned(),
                name: "Delegate".to_owned(),
            }),
            DelayedStep::Event(AiStreamEvent::ToolCallArgsDelta {
                id: "delegate_call".to_owned(),
                json_fragment: r#"{"task":"slow child"}"#.to_owned(),
            }),
            DelayedStep::Event(AiStreamEvent::ToolCallEnd {
                id: "delegate_call".to_owned(),
                raw_arguments: r#"{"task":"slow child"}"#.to_owned(),
            }),
            DelayedStep::Event(AiStreamEvent::MessageEnd {
                stop_reason: StopReason::ToolUse,
                usage: None,
            }),
        ],
        vec![DelayedStep::Delay(std::time::Duration::from_secs(30))],
    ]));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(neo_agent_core::harness::fake_model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_multi_agent(multi_agent.clone()),
        model,
        ToolRegistry::with_builtin_tools(),
    );
    let cancel = CancellationToken::new();
    let mut context = AgentContext::new();
    let mut stream = runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text("delegate"),
        cancel.clone(),
    );
    let mut agent_id = None;

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while let Some(event) = stream.next().await {
            let event = event.expect("runtime event");
            if let AgentEvent::DelegateStarted { agent, .. } = event {
                agent_id = Some(agent.id.as_str().to_owned());
                cancel.cancel();
            }
        }
    })
    .await
    .expect("cancelled delegate turn should finish");

    let agent_id = agent_id.expect("delegate started");
    let snapshot = multi_agent
        .agent_snapshot(&agent_id)
        .expect("delegate snapshot");
    assert_eq!(snapshot.state, AgentLifecycleState::Cancelled);
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
    let events = vec![AgentEvent::DelegateSwarmStarted {
        turn: 4,
        swarm: snapshot,
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
fn child_activity_trim_preserves_visible_ongoing_tool_and_latest_text() {
    let runtime = MultiAgentRuntime::new();
    let snapshot = runtime.start_foreground_delegate_for_test("long running bash");
    let started_at = std::time::Instant::now();

    let _ = runtime.apply_child_event(
        &snapshot.id,
        started_at,
        &AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "bash-live".to_owned(),
            name: "Bash".to_owned(),
            arguments: json!({"cmd": "cargo nextest run -p neo-tui --test multi_agent_transcript"}),
        },
    );
    for index in 0..32 {
        let _ = runtime.apply_child_event(
            &snapshot.id,
            started_at,
            &AgentEvent::ThinkingDelta {
                turn: 1,
                text: format!("thinking chunk {index}"),
            },
        );
        let _ = runtime.apply_child_event(
            &snapshot.id,
            started_at,
            &AgentEvent::TextDelta {
                turn: 1,
                text: format!("body chunk {index}"),
            },
        );
    }

    let updated = runtime
        .snapshot(&snapshot.id)
        .expect("snapshot remains present");
    assert_eq!(updated.activity.len(), 24);
    assert_eq!(
        latest_tool_phase(&updated, "bash-live"),
        Some(AgentToolActivityPhase::Ongoing)
    );
    let latest_thinking = updated
        .activity
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            AgentActivityKind::Text { text, thinking } if *thinking => Some(text.as_str()),
            _ => None,
        });
    assert_eq!(latest_thinking, Some("thinking chunk 31"));
}

#[test]
fn child_text_and_thinking_deltas_accumulate_into_live_activity() {
    let runtime = MultiAgentRuntime::new();
    let snapshot = runtime.start_foreground_delegate_for_test("stream text");
    let started_at = std::time::Instant::now();

    for text in ["All ", "edits ", "applied."] {
        let _ = runtime.apply_child_event(
            &snapshot.id,
            started_at,
            &AgentEvent::TextDelta {
                turn: 1,
                text: text.to_owned(),
            },
        );
    }
    for text in ["Let ", "me ", "verify."] {
        let _ = runtime.apply_child_event(
            &snapshot.id,
            started_at,
            &AgentEvent::ThinkingDelta {
                turn: 1,
                text: text.to_owned(),
            },
        );
    }

    let updated = runtime
        .snapshot(&snapshot.id)
        .expect("snapshot remains present");
    assert_eq!(updated.latest_text.as_deref(), Some("All edits applied."));
    let latest_body = updated
        .activity
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            AgentActivityKind::Text { text, thinking } if !thinking => Some(text.as_str()),
            _ => None,
        });
    let latest_thinking = updated
        .activity
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            AgentActivityKind::Text { text, thinking } if *thinking => Some(text.as_str()),
            _ => None,
        });
    assert_eq!(latest_body, Some("All edits applied."));
    assert_eq!(latest_thinking, Some("Let me verify."));
}

#[test]
fn child_text_delta_accumulation_preserves_repeated_fragments() {
    let runtime = MultiAgentRuntime::new();
    let snapshot = runtime.start_foreground_delegate_for_test("stream repeated text");
    let started_at = std::time::Instant::now();

    for text in ["ha", "ha", "!"] {
        let _ = runtime.apply_child_event(
            &snapshot.id,
            started_at,
            &AgentEvent::TextDelta {
                turn: 1,
                text: text.to_owned(),
            },
        );
    }

    let updated = runtime
        .snapshot(&snapshot.id)
        .expect("snapshot remains present");
    assert_eq!(updated.latest_text.as_deref(), Some("haha!"));
    let latest_body = updated
        .activity
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            AgentActivityKind::Text { text, thinking } if !thinking => Some(text.as_str()),
            _ => None,
        });
    assert_eq!(latest_body, Some("haha!"));
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
async fn delegate_emits_foreground_events() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Delegate".to_owned(),
            },
            AiStreamEvent::ToolCallArgsDelta {
                id: "tool_1".to_owned(),
                json_fragment: r#"{"task":"test task"}"#.to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "task": "test task" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "child_msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "child inspected queue".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: Some(neo_ai::TokenUsage {
                    input_tokens: 11,
                    output_tokens: 7,
                    input_cache_read_tokens: 0,
                    input_cache_write_tokens: 0,
                }),
            },
        ],
    ]);
    let tools = ToolRegistry::with_builtin_tools();
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Sequential)
            .with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("delegate a task"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::DelegateStarted { .. })),
        "expected DelegateStarted in events"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::DelegateFinished { .. })),
        "expected DelegateFinished in events"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::DelegateStarted { turn: 1, .. } | AgentEvent::DelegateFinished { turn: 1, .. }
    )));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn foreground_delegate_runs_child_model_turn_and_reports_child_summary() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "parent_msg".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_delegate".to_owned(),
                name: "Delegate".to_owned(),
            },
            AiStreamEvent::ToolCallArgsDelta {
                id: "tool_delegate".to_owned(),
                json_fragment:
                    r#"{"task":"inspect queue","role":"reviewer","context":"parent facts"}"#
                        .to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_delegate".to_owned(),
                raw_arguments: json!({
                    "task": "inspect queue",
                    "role": "reviewer",
                    "context": "inherit"
                })
                .to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "child_msg".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "queue is safe".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: Some(neo_ai::TokenUsage {
                    input_tokens: 13,
                    output_tokens: 5,
                    input_cache_read_tokens: 9,
                    input_cache_write_tokens: 2,
                }),
            },
        ],
    ]);
    let tools = ToolRegistry::with_builtin_tools();
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Sequential)
            .with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("delegate a real task"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let requests = harness.requests();
    assert!(
        requests.len() >= 2,
        "parent and child model turns should run"
    );
    let child_request = requests
        .iter()
        .find(|request| format!("{:?}", request.messages).contains("inspect queue"))
        .expect("child model request");
    let child_text = format!("{:?}", child_request.messages);
    assert!(child_text.contains("inspect queue"), "{child_text}");
    assert!(child_text.contains("Context mode: inherit"), "{child_text}");
    assert!(child_text.contains("Reviewer"), "{child_text}");
    assert!(child_text.contains("git add"), "{child_text}");
    assert!(child_text.contains("git commit"), "{child_text}");

    let delegate_result = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionFinished { result, .. }
                if result.content.contains("agent_id:") =>
            {
                Some(result)
            }
            _ => None,
        })
        .expect("delegate tool result");
    assert!(delegate_result.content.contains("queue is safe"));
    assert!(
        !delegate_result
            .content
            .contains("Foreground delegate completed.")
    );

    let finished_agent = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::DelegateFinished { turn, agent } => Some((*turn, agent)),
            _ => None,
        })
        .expect("delegate finished event");
    assert_eq!(finished_agent.0, 1);
    assert_eq!(finished_agent.1.tool_count, 0);
    assert_eq!(finished_agent.1.token_count, 18);
    assert_eq!(finished_agent.1.cache_read_token_count, 9);
    assert_eq!(finished_agent.1.cache_write_token_count, 2);
    assert_eq!(
        finished_agent.1.latest_text.as_deref(),
        Some("queue is safe")
    );
    assert_eq!(
        finished_agent
            .1
            .outcome
            .as_ref()
            .map(|outcome| outcome.summary.as_str()),
        Some("queue is safe")
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn delegate_streams_child_activity_updates_before_finish() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "parent_msg".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_delegate".to_owned(),
                name: "Delegate".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_delegate".to_owned(),
                raw_arguments: json!({ "task": "inspect lib" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "child_msg".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "read_1".to_owned(),
                name: "Read".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "read_1".to_owned(),
                raw_arguments: json!({ "path": "crates/neo-agent-core/src/lib.rs" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "child_msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "34 lines".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: Some(neo_ai::TokenUsage {
                    input_tokens: 20,
                    output_tokens: 5,
                    input_cache_read_tokens: 0,
                    input_cache_write_tokens: 0,
                }),
            },
        ],
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Sequential)
            .with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("delegate with tool activity"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let updates = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::DelegateUpdated { agent, .. } => Some(agent),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        updates.iter().any(|agent| {
            agent.activity.iter().any(|entry| {
                matches!(
                    &entry.kind,
                    AgentActivityKind::Tool {
                        name,
                        summary,
                        phase: AgentToolActivityPhase::Ongoing,
                        ..
                    }
                        if name == "Read"
                            && summary.as_deref()
                                == Some("crates/neo-agent-core/src/lib.rs")
                )
            })
        }),
        "expected live DelegateUpdated with Read activity: {updates:#?}"
    );
    assert!(
        updates
            .iter()
            .any(|agent| agent.latest_text.as_deref() == Some("34 lines")),
        "expected live DelegateUpdated with child text: {updates:#?}"
    );
    let finished = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::DelegateFinished { agent, .. } => Some(agent),
            _ => None,
        })
        .expect("finished delegate");
    assert_eq!(finished.tool_count, 1);
    assert_eq!(finished.latest_text.as_deref(), Some("34 lines"));
}

#[tokio::test]
async fn subagent_request_hides_and_blocks_parent_orchestration_tools() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "parent_msg".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_delegate".to_owned(),
                name: "Delegate".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_delegate".to_owned(),
                raw_arguments: json!({ "task": "try recursive delegation" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "child_msg".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "blocked recursive delegate".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Sequential)
            .with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("delegate recursive check"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let requests = harness.requests();
    let child_request = requests
        .iter()
        .find(|request| format!("{:?}", request.messages).contains("try recursive delegation"))
        .expect("child request");
    let child_tool_names = child_request
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert!(
        !child_tool_names.contains(&"Delegate"),
        "{child_tool_names:?}"
    );
    assert!(
        !child_tool_names.contains(&"DelegateSwarm"),
        "{child_tool_names:?}"
    );
    assert!(
        !child_tool_names.contains(&"RunWorkflow"),
        "{child_tool_names:?}"
    );
    // The child should have completed with the text response since
    // orchestration tools are hidden from subagents.
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionFinished { name, result, .. }
                if name == "Delegate"
                    && result
                        .content
                        .contains("blocked recursive delegate")
        )),
        "expected delegate result with 'blocked recursive delegate'"
    );
}

#[tokio::test]
async fn subagent_cannot_force_call_hidden_parent_tools() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "parent_msg".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_delegate".to_owned(),
                name: "Delegate".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_delegate".to_owned(),
                raw_arguments: json!({ "task": "try hidden task output" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "child_msg".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "hidden_tool".to_owned(),
                name: "ListDelegates".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "hidden_tool".to_owned(),
                raw_arguments: json!({}).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Sequential)
            .with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("delegate hidden tool check"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let finished = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::DelegateFinished { agent, .. } => Some(agent),
            _ => None,
        })
        .expect("delegate should finish");
    assert!(
        finished.activity.iter().any(|entry| matches!(
            &entry.kind,
            AgentActivityKind::Tool {
                name,
                phase: AgentToolActivityPhase::Failed,
                ..
            } if name == "ListDelegates"
        )),
        "{:#?}",
        finished.activity
    );
}

#[tokio::test]
async fn child_activity_keeps_same_name_tool_failures_on_their_own_ids() {
    let runtime = MultiAgentRuntime::new();
    let agent = runtime.start_delegate(
        "same tool ids",
        None,
        neo_agent_core::multi_agent::AgentRole::Coder,
        neo_agent_core::multi_agent::AgentRunMode::Foreground,
        neo_agent_core::multi_agent::DelegateContext::None,
        neo_agent_core::multi_agent::AgentPathKind::Root,
    );
    let started_at = std::time::Instant::now();

    for (id, path) in [("read_ok", "ok.rs"), ("read_fail", "missing.rs")] {
        let _ = runtime.apply_child_event(
            &agent.id,
            started_at,
            &AgentEvent::ToolExecutionStarted {
                turn: 1,
                id: id.to_owned(),
                name: "Read".to_owned(),
                arguments: json!({ "path": path }),
            },
        );
    }
    let _ = runtime.apply_child_event(
        &agent.id,
        started_at,
        &AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "read_fail".to_owned(),
            name: "Read".to_owned(),
            result: neo_agent_core::ToolResult::error("missing file"),
        },
    );

    let snapshot = runtime.snapshot(&agent.id).expect("agent snapshot");
    let tools = snapshot
        .activity
        .iter()
        .filter_map(|entry| match &entry.kind {
            AgentActivityKind::Tool {
                id, summary, phase, ..
            } => Some((id.as_str(), summary.as_deref(), *phase)),
            AgentActivityKind::Text { .. } => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tools,
        vec![
            ("read_ok", Some("ok.rs"), AgentToolActivityPhase::Ongoing),
            (
                "read_fail",
                Some("missing.rs"),
                AgentToolActivityPhase::Failed
            )
        ]
    );
}

#[test]
fn child_tool_events_preserve_ongoing_done_and_failed_phase() {
    let runtime = MultiAgentRuntime::new();
    let snapshot = runtime.start_delegate(
        "run tests",
        Some("Run tests"),
        AgentRole::Coder,
        AgentRunMode::Foreground,
        neo_agent_core::multi_agent::DelegateContext::None,
        AgentPathKind::Root,
    );
    let started_at = std::time::Instant::now();

    let started = runtime
        .apply_child_event(
            &snapshot.id,
            started_at,
            &AgentEvent::ToolExecutionStarted {
                turn: 0,
                id: "call_bash".to_owned(),
                name: "Bash".to_owned(),
                arguments: json!({ "command": "cargo nextest run -p neo-tui" }),
            },
        )
        .expect("started update");

    let tool = started
        .activity
        .iter()
        .find_map(|entry| match &entry.kind {
            AgentActivityKind::Tool {
                phase,
                summary,
                output,
                ..
            } => Some((*phase, summary.clone(), output.clone())),
            AgentActivityKind::Text { .. } => None,
        })
        .expect("tool row");

    assert_eq!(tool.0, AgentToolActivityPhase::Ongoing);
    assert_eq!(tool.1.as_deref(), Some("cargo nextest run -p neo-tui"));
    assert!(tool.2.is_none());

    let updated = runtime
        .apply_child_event(
            &snapshot.id,
            started_at,
            &AgentEvent::ToolExecutionUpdate {
                turn: 0,
                id: "call_bash".to_owned(),
                name: "Bash".to_owned(),
                partial_result: ToolResult::ok("Compiling neo-tui v0.1.0"),
            },
        )
        .expect("live output update");
    let output = latest_tool_output(&updated, "call_bash").expect("output preview");
    assert!(output.text.contains("Compiling neo-tui"));
    assert!(output.tail);

    let finished = runtime
        .apply_child_event(
            &snapshot.id,
            started_at,
            &AgentEvent::ToolExecutionFinished {
                turn: 0,
                id: "call_bash".to_owned(),
                name: "Bash".to_owned(),
                result: ToolResult::ok("Finished test profile"),
            },
        )
        .expect("finished update");
    assert_eq!(
        latest_tool_phase(&finished, "call_bash"),
        Some(AgentToolActivityPhase::Done)
    );
    assert_eq!(finished.tool_count, 1);
}

fn latest_tool_phase(
    snapshot: &neo_agent_core::multi_agent::AgentSnapshot,
    id: &str,
) -> Option<AgentToolActivityPhase> {
    snapshot
        .activity
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            AgentActivityKind::Tool {
                id: entry_id,
                phase,
                ..
            } if entry_id == id => Some(*phase),
            _ => None,
        })
}

fn latest_tool_output(
    snapshot: &neo_agent_core::multi_agent::AgentSnapshot,
    id: &str,
) -> Option<AgentToolOutputPreview> {
    snapshot
        .activity
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            AgentActivityKind::Tool {
                id: entry_id,
                output,
                ..
            } if entry_id == id => output.clone(),
            _ => None,
        })
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn delegate_swarm_runs_children_with_named_agents_and_parent_turn() {
    let harness = FakeHarness::from_turns([vec![
        AiStreamEvent::MessageStart {
            id: "parent_msg".to_owned(),
        },
        AiStreamEvent::ToolCallStart {
            id: "tool_swarm".to_owned(),
            name: "DelegateSwarm".to_owned(),
        },
        AiStreamEvent::ToolCallArgsDelta {
            id: "tool_swarm".to_owned(),
            json_fragment: r#"{"description":"inspect modules","items":["api","tui","runtime"],"prompt_template":"Check {{item}}","max_concurrency":2}"#.to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: "tool_swarm".to_owned(),
            raw_arguments: json!({
                "description": "inspect modules",
                "items": ["api", "tui", "runtime"],
                "prompt_template": "Check {{item}}",
                "max_concurrency": 2
            }).to_string(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::ToolUse,
            usage: None,
        },
    ], vec![
        AiStreamEvent::MessageStart {
            id: "child_api".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "api ok".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ], vec![
        AiStreamEvent::MessageStart {
            id: "child_tui".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "tui ok".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ], vec![
        AiStreamEvent::MessageStart {
            id: "child_runtime".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "runtime ok".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]]);
    let tools = ToolRegistry::with_builtin_tools();
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Sequential)
            .with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run swarm"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        harness.requests().len() >= 4,
        "parent plus three child turns should run"
    );
    let finished_swarm = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::DelegateSwarmFinished { turn, swarm } => Some((*turn, swarm)),
            _ => None,
        })
        .expect("swarm finished event");
    assert_eq!(finished_swarm.0, 1);
    assert_eq!(finished_swarm.1.max_concurrency, 2);
    let started_swarm = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::DelegateSwarmStarted { swarm, .. } => Some(swarm),
            _ => None,
        })
        .expect("swarm started event");
    assert_eq!(started_swarm.max_concurrency, 2);
    assert!(
        started_swarm
            .children
            .iter()
            .all(|child| child.agent.state == AgentLifecycleState::Queued),
        "swarm should start in queued/orchestrating state: {started_swarm:#?}"
    );
    let updates = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::DelegateSwarmUpdated { turn, swarm } => Some((*turn, swarm)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        updates.len() >= 6,
        "updates should stream child start/text/finish progress, got {}",
        updates.len()
    );
    assert_eq!(updates[0].0, 1);
    assert!(
        updates.iter().any(|(_, swarm)| {
            swarm
                .children
                .iter()
                .any(|child| child.agent.latest_text.as_deref() == Some("api ok"))
        }),
        "updates should expose child text before final swarm: {updates:#?}"
    );
    let names = finished_swarm
        .1
        .children
        .iter()
        .map(|child| child.agent.display_name.as_str())
        .collect::<Vec<_>>();
    let expected_names = DEFAULT_AGENT_NAMES
        .iter()
        .take(3)
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(names, expected_names);
    assert!(!names.iter().any(|name| name.starts_with("child-")));

    let delegate_result = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionFinished { name, result, .. } if name == "DelegateSwarm" => {
                Some(result)
            }
            _ => None,
        })
        .expect("swarm tool result");
    let items = delegate_result
        .details
        .as_ref()
        .and_then(|details| details.get("items"))
        .and_then(serde_json::Value::as_array)
        .expect("swarm details include items");
    assert!(items.iter().any(|item| item["summary"] == "api ok"));
    assert!(items.iter().any(|item| item["summary"] == "tui ok"));
    assert!(items.iter().any(|item| item["summary"] == "runtime ok"));
}

#[tokio::test]
async fn delegate_swarm_substitutes_canonical_placeholders_only() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "parent_msg".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_swarm".to_owned(),
                name: "DelegateSwarm".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_swarm".to_owned(),
                raw_arguments: json!({
                    "description": "canonical title",
                    "items": ["alpha", "beta"],
                    "prompt_template": "Review {{item}} for {{description}}",
                    "max_concurrency": 2
                })
                .to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
        ],
        child_text_turn("alpha done"),
        child_text_turn("beta done"),
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Sequential)
            .with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run templated swarm"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let child_requests = harness
        .requests()
        .into_iter()
        .filter(|request| {
            format!("{:?}", request.messages).contains("Review alpha for canonical title")
                || format!("{:?}", request.messages).contains("Review beta for canonical title")
        })
        .collect::<Vec<_>>();
    assert_eq!(child_requests.len(), 2, "{child_requests:#?}");
    for request in child_requests {
        let text = format!("{:?}", request.messages);
        assert!(!text.contains("{{item}}"), "{text}");
        assert!(!text.contains("{{description}}"), "{text}");
    }

    let swarm_result = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionFinished { name, result, .. } if name == "DelegateSwarm" => {
                Some(result)
            }
            _ => None,
        })
        .expect("swarm result");
    assert!(
        swarm_result.content.contains("swarm_id:"),
        "{}",
        swarm_result.content
    );
    assert!(
        swarm_result.content.contains("status: completed"),
        "{}",
        swarm_result.content
    );
    let items = swarm_result
        .details
        .as_ref()
        .and_then(|details| details.get("items"))
        .and_then(serde_json::Value::as_array)
        .expect("swarm details include items");
    assert!(
        items.iter().all(|item| item["agent_id"].as_str().is_some()),
        "{items:#?}"
    );
}

#[tokio::test]
async fn delegate_tools_reject_empty_tasks_bad_context_and_zero_concurrency() {
    let harness = FakeHarness::from_turns([]);
    let registry = std::sync::Arc::new(ToolRegistry::with_builtin_tools());
    let ctx = neo_agent_core::tools::ToolContext::new(tempfile::tempdir().unwrap().path())
        .unwrap()
        .with_child_runtime(
            AgentConfig::for_model(harness.model())
                .with_tool_execution_mode(ToolExecutionMode::Sequential)
                .with_permission_mode(PermissionMode::Yolo),
            harness.client(),
            registry.clone(),
            1,
        );

    let empty_delegate = registry
        .run("Delegate", &ctx, json!({ "task": "" }))
        .await
        .expect("empty task should return validation result");
    assert!(empty_delegate.is_error);
    assert!(empty_delegate.content.contains("task must not be empty"));

    let bad_context = registry
        .run(
            "Delegate",
            &ctx,
            json!({ "task": "x", "context": "garbage" }),
        )
        .await
        .expect_err("bad context should be rejected");
    assert!(bad_context.to_string().contains("unknown variant"));

    let zero_concurrency = registry
        .run(
            "DelegateSwarm",
            &ctx,
            json!({
                "description": "bad concurrency",
                "items": ["a"],
                "prompt_template": "{{item}}",
                "max_concurrency": 0
            }),
        )
        .await
        .expect_err("zero concurrency should be rejected");
    assert!(zero_concurrency.to_string().contains("max_concurrency"));

    let legacy_template = registry
        .run(
            "DelegateSwarm",
            &ctx,
            json!({
                "description": "legacy placeholder",
                "items": ["a"],
                "prompt_template": "Review {task}"
            }),
        )
        .await
        .expect_err("legacy placeholder should be rejected");
    assert!(
        legacy_template
            .to_string()
            .contains("prompt_template must include {{item}}")
    );
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

struct DelayedTurnModel {
    turns: Mutex<Vec<Vec<DelayedStep>>>,
}

impl DelayedTurnModel {
    fn new(turns: Vec<Vec<DelayedStep>>) -> Self {
        let mut turns = turns;
        turns.reverse();
        Self {
            turns: Mutex::new(turns),
        }
    }
}

enum DelayedStep {
    Event(AiStreamEvent),
    Delay(std::time::Duration),
}

impl ModelClient for DelayedTurnModel {
    fn stream_chat(
        &self,
        _request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        let steps = self
            .turns
            .lock()
            .expect("turns lock poisoned")
            .pop()
            .unwrap_or_default();
        stream::unfold(steps.into_iter(), |mut steps| async move {
            loop {
                match steps.next()? {
                    DelayedStep::Event(event) => return Some((Ok(event), steps)),
                    DelayedStep::Delay(duration) => tokio::time::sleep(duration).await,
                }
            }
        })
        .boxed()
    }
}

fn registry_with_multi_agent() -> (ToolRegistry, ToolContext) {
    let harness = FakeHarness::from_turns([vec![
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "done".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]]);
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
async fn message_delegate_terminal_agent_error_explains_resume_without_immutable_confusion() {
    let (registry, ctx) = registry_with_multi_agent();
    let first = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "finish then reject live message",
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
        .expect("agent id")
        .to_owned();

    let result = registry
        .run(
            "MessageDelegate",
            &ctx,
            serde_json::json!({
                "id": agent_id,
                "message": "add one more note"
            }),
        )
        .await
        .expect("message tool should return an error result");

    assert!(result.is_error);
    assert!(
        result.content.contains("cannot receive live messages"),
        "{}",
        result.content
    );
    assert!(
        result.content.contains("Delegate with resume"),
        "{}",
        result.content
    );
    assert!(
        !result
            .content
            .contains("terminal delegate state is immutable"),
        "{}",
        result.content
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
async fn swarm_result_shape_matches_between_foreground_wait_and_task_output() {
    let (registry, ctx) = registry_with_multi_agent();
    let foreground = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "shape check",
                "items": ["a", "b"],
                "prompt_template": "Inspect {{item}}",
                "mode": "foreground"
            }),
        )
        .await
        .expect("foreground swarm should complete");
    let swarm_id = foreground.details.as_ref().unwrap()["swarm_id"]
        .as_str()
        .expect("swarm id")
        .to_owned();

    let waited = registry
        .run("WaitDelegate", &ctx, serde_json::json!({ "id": swarm_id }))
        .await
        .expect("wait should read completed swarm");
    let output = registry
        .run(
            "TaskOutput",
            &ctx,
            serde_json::json!({ "task_id": swarm_id }),
        )
        .await
        .expect("task output should read completed swarm");

    let foreground_details = foreground.details.as_ref().unwrap();
    let waited_details = waited.details.as_ref().unwrap();
    let output_details = output.details.as_ref().unwrap();

    for details in [foreground_details, waited_details, output_details] {
        assert_eq!(details["kind"], "delegate_swarm");
        assert_eq!(details["summary_scope"], "swarm_items");
        assert!(
            details["aggregate"]["total"].as_u64().is_some(),
            "{details}"
        );
        assert!(details["items"][0]["name"].as_str().is_some(), "{details}");
        assert!(
            details["items"][0]["elapsed_ms"].as_u64().is_some(),
            "{details}"
        );
        assert!(
            details["items"][0]["tool_count"].as_u64().is_some(),
            "{details}"
        );
        assert!(
            details["items"][0]["token_count"].as_u64().is_some(),
            "{details}"
        );
    }
}

#[tokio::test]
async fn wait_delegate_timeout_preserves_running_status_with_wait_timed_out_outcome() {
    let runtime = MultiAgentRuntime::new();
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
                "id": running.id.as_str(),
                "timeout_ms": 1
            }),
        )
        .await
        .expect("wait should return timeout result");

    let details = result.details.as_ref().expect("wait details");
    assert_eq!(details["kind"], "delegate_wait");
    assert_eq!(details["outcome"], "wait_timed_out");
    assert_eq!(details["status"], "running");
    assert_eq!(details["id"], running.id.as_str());
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

    let delegate = spec("Delegate");
    assert!(
        delegate.description.contains("Default mode is foreground"),
        "{}",
        delegate.description
    );
    assert!(
        delegate.description.contains("resume"),
        "{}",
        delegate.description
    );
    assert!(
        delegate.description.contains("role must be omitted"),
        "{}",
        delegate.description
    );
    assert!(
        delegate.description.contains("context"),
        "{}",
        delegate.description
    );

    let message = spec("MessageDelegate");
    assert!(
        message.description.contains("live"),
        "{}",
        message.description
    );
    assert!(
        message.description.contains("agent or swarm"),
        "{}",
        message.description
    );
    assert!(
        message.description.contains("running children"),
        "{}",
        message.description
    );
    assert!(
        message.description.contains("Delegate with resume"),
        "{}",
        message.description
    );

    let list = spec("ListDelegates");
    assert!(
        list.description.contains("active-only"),
        "{}",
        list.description
    );
    assert!(
        list.description.contains("meta-only"),
        "{}",
        list.description
    );
    assert!(
        list.description.contains("include_completed=true"),
        "{}",
        list.description
    );
    assert!(
        list.description.contains("same query"),
        "{}",
        list.description
    );

    let wait = spec("WaitDelegate");
    assert!(
        wait.description.contains("wait_timed_out"),
        "{}",
        wait.description
    );
    assert!(
        wait.description
            .contains("delegate itself reached timed_out"),
        "{}",
        wait.description
    );

    let swarm = spec("DelegateSwarm");
    assert!(
        swarm.description.contains("foreground"),
        "{}",
        swarm.description
    );
    assert!(
        swarm.description.contains("WaitDelegate"),
        "{}",
        swarm.description
    );
    assert!(
        swarm.description.contains("TaskOutput"),
        "{}",
        swarm.description
    );
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
                "items": ["a", "b"],
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
async fn delegate_swarm_rejects_unknown_template_placeholder() {
    let (registry, ctx) = registry_with_multi_agent();
    let result = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "audit",
                "items": ["one"],
                "prompt_template": "Audit {{task}} and {{item}}"
            }),
        )
        .await;

    let result = result.unwrap_or_else(|err| ToolResult::error(err.to_string()));
    assert!(result.is_error);
    assert!(
        result
            .content
            .contains("only {{item}} and {{description}} are supported"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn delegate_swarm_rejects_duplicate_expanded_prompts() {
    let (registry, ctx) = registry_with_multi_agent();
    let result = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "audit",
                "items": ["same", "same"],
                "prompt_template": "Audit {{item}}"
            }),
        )
        .await;

    let result = result.unwrap_or_else(|err| ToolResult::error(err.to_string()));
    assert!(result.is_error);
    assert!(
        result.content.contains("duplicate expanded child prompt"),
        "{}",
        result.content
    );
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
async fn summary_context_does_not_leak_role_setup_boilerplate() {
    let (registry, ctx) = registry_with_multi_agent();
    let result = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "Read crates/neo-agent-core/src/lib.rs and summarize in one sentence",
                "role": "explorer",
                "context": "summary",
                "mode": "foreground"
            }),
        )
        .await
        .expect("delegate should complete");

    assert!(
        !result.content.contains("Acknowledged. Ready"),
        "{}",
        result.content
    );
    assert!(
        !result.content.contains("You are an Explorer subagent"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn cancel_agent_stops_active_child_stream() {
    use neo_agent_core::multi_agent::{ChildRuntimeDeps, DelegateContext};

    let runtime = MultiAgentRuntime::new();
    let model = Arc::new(DelayedTurnModel::new(vec![vec![
        DelayedStep::Event(AiStreamEvent::MessageStart {
            id: "child".to_owned(),
        }),
        DelayedStep::Event(AiStreamEvent::ThinkingStart {
            id: "thinking".to_owned(),
        }),
        DelayedStep::Delay(std::time::Duration::from_secs(30)),
        DelayedStep::Event(AiStreamEvent::ThinkingDelta {
            text: "should not arrive".to_owned(),
        }),
    ]]));
    let deps = ChildRuntimeDeps::new(
        AgentConfig::for_model(neo_agent_core::harness::fake_model()),
        model,
        Arc::new(ToolRegistry::new()),
    );
    let snapshot = runtime.start_delegate(
        "slow child",
        None,
        AgentRole::Coder,
        AgentRunMode::Foreground,
        DelegateContext::None,
        AgentPathKind::Root,
    );
    let agent_id = snapshot.id.clone();
    let run = tokio::spawn({
        let runtime = runtime.clone();
        async move {
            runtime
                .run_started_child_turn(deps, snapshot, DelegateContext::None, |_| {})
                .await
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let cancelled = runtime.cancel_agent(&agent_id).expect("agent cancels");
    assert_eq!(cancelled.state, AgentLifecycleState::Cancelled);

    let output = tokio::time::timeout(std::time::Duration::from_secs(2), run)
        .await
        .expect("child run should stop after interrupt")
        .expect("join should succeed");
    assert_eq!(output.snapshot.state, AgentLifecycleState::Cancelled);
    assert!(
        !output
            .snapshot
            .activity
            .iter()
            .any(|entry| matches!(&entry.kind, AgentActivityKind::Text { text, .. } if text.contains("should not arrive")))
    );
}

#[test]
fn cancel_swarm_preserves_completed_canonical_child_when_swarm_snapshot_is_stale() {
    use neo_agent_core::multi_agent::{SwarmChildSnapshot, SwarmSnapshot};

    let runtime = MultiAgentRuntime::new();
    let swarm_id = runtime.new_swarm_id();
    let first = runtime.start_delegate(
        "already finished",
        Some("finished"),
        AgentRole::Coder,
        AgentRunMode::Foreground,
        neo_agent_core::multi_agent::DelegateContext::None,
        AgentPathKind::SwarmChild(&swarm_id),
    );
    let second = runtime.start_delegate(
        "still running",
        Some("running"),
        AgentRole::Coder,
        AgentRunMode::Foreground,
        neo_agent_core::multi_agent::DelegateContext::None,
        AgentPathKind::SwarmChild(&swarm_id),
    );
    let stale_swarm = SwarmSnapshot {
        swarm_id: swarm_id.clone(),
        description: "stale swarm".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: AgentLifecycleState::Running,
        max_concurrency: 2,
        aggregate: SwarmAggregate::from_states([
            AgentLifecycleState::Running,
            AgentLifecycleState::Running,
        ]),
        children: vec![
            SwarmChildSnapshot {
                item_index: 0,
                item: "first".to_owned(),
                agent: first.clone(),
            },
            SwarmChildSnapshot {
                item_index: 1,
                item: "second".to_owned(),
                agent: second.clone(),
            },
        ],
    };
    runtime.register_swarm(stale_swarm);
    let _ = runtime.complete_delegate_for_test(&first.id, "finished before interrupt");

    let cancelled = runtime
        .cancel_swarm(&swarm_id)
        .expect("stale running swarm should cancel unfinished children");

    assert_eq!(
        runtime
            .agent_snapshot(first.id.as_str())
            .expect("first agent")
            .state,
        AgentLifecycleState::Completed
    );
    assert_eq!(
        runtime
            .agent_snapshot(second.id.as_str())
            .expect("second agent")
            .state,
        AgentLifecycleState::Cancelled
    );
    assert_eq!(cancelled.aggregate.completed, 1);
    assert_eq!(cancelled.aggregate.cancelled, 1);
}
