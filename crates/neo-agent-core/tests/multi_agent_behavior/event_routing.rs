use futures::StreamExt;
use neo_agent_core::harness::FakeHarness;
use neo_agent_core::multi_agent::{
    AgentActivityKind, AgentLifecycleState, AgentPathKind, AgentRole, AgentRunMode,
    AgentToolActivityPhase, DEFAULT_AGENT_NAMES, MultiAgentRuntime, SwarmAggregate,
};
use neo_agent_core::tools::{ToolContext, ToolRegistry, ToolResult};
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, PermissionMode,
    ToolExecutionMode,
};
use neo_ai::{AiError, AiStreamEvent, StopReason};
use serde_json::json;
use std::sync::Arc;

#[test]
fn delegate_events_do_not_serialize_prior_messages() {
    let runtime = MultiAgentRuntime::new();
    let mut snapshot = runtime.start_foreground_delegate_for_test("audit session bloat");
    snapshot.prior_messages = vec![
        AgentMessage::user_text("large child prompt"),
        AgentMessage::system_text("large child answer"),
    ];

    let event = AgentEvent::DelegateUpdated {
        turn: 7,
        agent: snapshot,
        workflow_origin: None,
    };
    let serialized = serde_json::to_value(&event).expect("serialize delegate event");

    assert_eq!(
        serialized.pointer("/DelegateUpdated/agent/prior_messages"),
        None,
        "main-agent delegate progress events must not persist child conversation history: {serialized}"
    );
}

#[test]
fn delegate_swarm_events_do_not_serialize_child_prior_messages() {
    use neo_agent_core::multi_agent::{SwarmChildSnapshot, SwarmSnapshot};

    let runtime = MultiAgentRuntime::new();
    let swarm_id = runtime.new_swarm_id();
    let mut child = runtime.start_delegate(
        "write docs",
        Some("docs"),
        AgentRole::Coder,
        AgentRunMode::Foreground,
        neo_agent_core::multi_agent::DelegateContext::None,
        AgentPathKind::SwarmChild(&swarm_id),
    );
    child.prior_messages = vec![AgentMessage::user_text("large child history")];
    let swarm = SwarmSnapshot {
        swarm_id,
        description: "docs".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: AgentLifecycleState::Running,
        max_concurrency: 1,
        aggregate: SwarmAggregate::from_states([AgentLifecycleState::Running]),
        children: vec![SwarmChildSnapshot {
            item_index: 0,
            item: "docs".to_owned(),
            agent: child,
        }],
    };

    let event = AgentEvent::DelegateSwarmUpdated {
        turn: 8,
        swarm,
        workflow_origin: None,
    };
    let serialized = serde_json::to_value(&event).expect("serialize swarm event");

    assert_eq!(
        serialized.pointer("/DelegateSwarmUpdated/swarm/children/0/agent/prior_messages"),
        None,
        "main-agent swarm progress events must not persist child conversation history: {serialized}"
    );
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
        output_schema: None,
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
async fn resumed_child_turn_fails_when_agent_wire_is_missing_or_corrupt() {
    use neo_agent_core::{
        multi_agent::{ChildRuntimeDeps, DelegateContext, DelegateRequest},
        session::{SessionState, SessionStateStore, agent_wire_path},
    };

    for corrupt_wire in [false, true] {
        let temp = tempfile::tempdir().expect("tempdir");
        let session_dir = temp.path();
        let mut state = SessionState::new();
        state.ensure_main_agent();
        SessionStateStore::new(session_dir)
            .write(&state)
            .expect("state");

        let runtime = MultiAgentRuntime::new().with_session_directory(session_dir.to_path_buf());
        let first = runtime.start_foreground_delegate_for_test("first task");
        let completed = runtime.complete_delegate_for_test(&first.id, "first answer");
        let agent_id = completed.id.as_str().to_owned();
        if corrupt_wire {
            let wire = agent_wire_path(session_dir, &agent_id);
            tokio::fs::create_dir_all(wire.parent().expect("agent directory"))
                .await
                .expect("create agent directory");
            tokio::fs::write(&wire, b"not json\n")
                .await
                .expect("write corrupt wire");
        }

        let request = DelegateRequest {
            task: "second task".to_owned(),
            resume: Some(agent_id.clone()),
            title: None,
            role: None,
            mode: AgentRunMode::Foreground,
            context: DelegateContext::None,
            output_schema: None,
        };
        let resumed = runtime
            .start_resume_delegate(&agent_id, &request)
            .expect("start resume");
        let harness = FakeHarness::from_turns([child_text_turn("must not run")]);
        let deps = ChildRuntimeDeps::new(
            AgentConfig::for_model(harness.model()),
            harness.client(),
            Arc::new(ToolRegistry::new()),
        );

        let output = runtime
            .run_started_child_turn(deps, resumed, DelegateContext::None, |_| {})
            .await;

        assert_eq!(output.snapshot.state, AgentLifecycleState::Failed);
        assert!(
            output
                .snapshot
                .outcome
                .as_ref()
                .is_some_and(|outcome| outcome.summary.contains("failed to replay delegate")),
            "{:?}",
            output.snapshot.outcome
        );
        assert!(
            harness.requests().is_empty(),
            "model must not run after replay failure"
        );
    }
}

#[tokio::test]
async fn failed_child_run_discards_partial_model_attempt_from_agent_wire() {
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
        .expect("state");

    let runtime = MultiAgentRuntime::new().with_session_directory(session_dir.to_path_buf());
    let harness = FakeHarness::from_result_turns([vec![
        Ok(AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "child_partial".to_owned(),
        }),
        Ok(AiStreamEvent::TextDelta {
            text: "partial child answer".to_owned(),
        }),
        Err(AiError::Transport {
            message: "child stream failed".to_owned(),
        }),
    ]]);
    let mut config = AgentConfig::for_model(harness.model());
    config.max_retries = 0;
    let deps = ChildRuntimeDeps::new(config, harness.client(), Arc::new(ToolRegistry::new()));
    let request = DelegateRequest {
        task: "fail after partial".to_owned(),
        resume: None,
        title: None,
        role: None,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::None,
        output_schema: None,
    };

    let output = runtime
        .run_child_turn(deps, &request, AgentRunMode::Foreground)
        .await
        .expect("child run returns failed snapshot");
    assert_eq!(output.snapshot.state, AgentLifecycleState::Failed);
    let wire = agent_wire_path(session_dir, output.snapshot.id.as_str());
    let replayed = JsonlSessionReader::read_all(&wire)
        .await
        .expect("read wire");

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
        !replayed
            .iter()
            .any(|event| matches!(event, AgentEvent::TextDelta { text, .. } if text == "partial child answer")),
        "{replayed:#?}"
    );
}

#[tokio::test]
async fn delegate_emits_foreground_events() {
    let harness = FakeHarness::from_turns([
        vec![
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
                json_fragment: r#"{"task":"test task"}"#.to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "task": "test task" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: neo_ai::MessagePhase::Unknown,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: neo_ai::MessagePhase::Unknown,
                id: "child_msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "child inspected queue".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: neo_ai::MessagePhase::Unknown,
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
async fn delegate_streams_child_activity_updates_before_finish() {
    let harness = delegate_activity_harness();
    let events = run_harness_turn(&harness, "delegate with tool activity").await;

    let updates = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::DelegateProgressUpdated { progress, .. } => Some(progress),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        updates.iter().any(|progress| {
            progress.last_tool.as_ref().is_some_and(|tool| {
                tool.name == "Read"
                    && tool.summary.as_deref() == Some("crates/neo-agent-core/src/lib.rs")
                    && tool.phase == AgentToolActivityPhase::Ongoing
            })
        }),
        "expected live DelegateProgressUpdated with Read activity: {updates:#?}"
    );
    assert!(
        updates
            .iter()
            .any(|progress| progress.latest_text.as_deref() == Some("34 lines")),
        "expected live DelegateProgressUpdated with child text: {updates:#?}"
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
                phase: neo_ai::MessagePhase::Unknown,
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
                phase: neo_ai::MessagePhase::Unknown,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: neo_ai::MessagePhase::Unknown,
                id: "child_msg".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "blocked recursive delegate".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: neo_ai::MessagePhase::Unknown,
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
    assert!(
        !child_tool_names.contains(&"Workflow"),
        "child must not see the root-only Workflow tool: {child_tool_names:?}"
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
                phase: neo_ai::MessagePhase::Unknown,
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
                phase: neo_ai::MessagePhase::Unknown,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: neo_ai::MessagePhase::Unknown,
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
                phase: neo_ai::MessagePhase::Unknown,
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
async fn delegate_swarm_runs_children_with_named_agents_and_parent_turn() {
    let harness = named_swarm_harness();
    let events = run_harness_turn(&harness, "run swarm").await;

    assert_named_swarm_lifecycle(&events, &harness);
    assert_named_swarm_result(&events);
}

#[tokio::test]
async fn delegate_swarm_substitutes_canonical_placeholders_only() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: neo_ai::MessagePhase::Unknown,
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
                    "items": [
                        {"title": "alpha", "value": "alpha"},
                        {"title": "beta", "value": "beta"}
                    ],
                    "prompt_template": "Review {{item}} for {{description}}",
                    "max_concurrency": 2
                })
                .to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: neo_ai::MessagePhase::Unknown,
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
async fn child_tool_output_reference_survives_wire_replay() {
    use neo_agent_core::session::ToolOutputRef;
    use neo_agent_core::session::{JsonlSessionReader, JsonlSessionWriter};

    let dir = tempfile::tempdir().expect("tempdir");
    let wire = dir.path().join("wire.jsonl");
    let runtime = MultiAgentRuntime::new();
    let child = runtime.start_foreground_delegate_for_test("run wire tests");
    let started_at = std::time::Instant::now();
    let reference = ToolOutputRef {
        agent_id: child.id.as_str().to_owned(),
        task_id: "bash-wire".to_owned(),
        byte_len: 512,
        line_count: 4,
        complete: true,
    };
    let events = vec![
        AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "call-wire".to_owned(),
            name: "Bash".to_owned(),
            arguments: json!({"command": "printf wire"}),
            workflow_origin: None,
            output_ref: Some(reference.clone()),
        },
        AgentEvent::ToolExecutionUpdate {
            turn: 1,
            id: "call-wire".to_owned(),
            name: "Bash".to_owned(),
            partial_result: ToolResult::ok("wire progress"),
            workflow_origin: None,
            output_ref: Some(reference.clone()),
        },
        AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "call-wire".to_owned(),
            name: "Bash".to_owned(),
            result: ToolResult::ok("wire done"),
            workflow_origin: None,
            output_ref: Some(reference.clone()),
        },
    ];

    // The child wire JSONL round-trips the typed reference byte-for-byte.
    let mut writer = JsonlSessionWriter::create(&wire)
        .await
        .expect("wire writer");
    for event in &events {
        writer.append(event).await.expect("append wire event");
    }
    writer.flush().await.expect("flush wire");
    let replayed = JsonlSessionReader::read_all(&wire)
        .await
        .expect("read wire events");
    assert_eq!(replayed, events, "typed references must survive the wire");

    // The parent projects the replayed child events into root activity with
    // the same typed reference.
    for event in &replayed {
        runtime
            .apply_child_event(&child.id, started_at, event)
            .expect("apply replayed child event");
    }
    let projected = runtime
        .agent_snapshot(child.id.as_str())
        .expect("child snapshot");
    let entry_ref = projected
        .activity
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            AgentActivityKind::Tool {
                id,
                output_ref,
                phase,
                ..
            } if id == "call-wire" => Some((output_ref, *phase)),
            AgentActivityKind::Text { .. } | AgentActivityKind::Tool { .. } => None,
        })
        .expect("tool row");
    assert_eq!(entry_ref.0, &Some(reference.clone()));
    assert_eq!(entry_ref.1, AgentToolActivityPhase::Done);

    // The delegate progress projection retains the reference and the final
    // metadata so swarm children and resume see the same artifact.
    let progress = projected.progress_snapshot();
    let last_tool = progress.last_tool.expect("last tool progress");
    assert_eq!(last_tool.output_ref, Some(reference));
    assert_eq!(last_tool.phase, AgentToolActivityPhase::Done);
}

async fn run_harness_turn(harness: &FakeHarness, prompt: &str) -> Vec<AgentEvent> {
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Sequential)
            .with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text(prompt))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed")
}

fn delegate_activity_harness() -> FakeHarness {
    FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: neo_ai::MessagePhase::Unknown,
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
                phase: neo_ai::MessagePhase::Unknown,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: neo_ai::MessagePhase::Unknown,
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
                phase: neo_ai::MessagePhase::Unknown,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: neo_ai::MessagePhase::Unknown,
                id: "child_msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "34 lines".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: neo_ai::MessagePhase::Unknown,
                stop_reason: StopReason::EndTurn,
                usage: Some(neo_ai::TokenUsage {
                    input_tokens: 20,
                    output_tokens: 5,
                    input_cache_read_tokens: 0,
                    input_cache_write_tokens: 0,
                }),
            },
        ],
    ])
}

fn named_swarm_harness() -> FakeHarness {
    FakeHarness::from_turns([vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "parent_msg".to_owned(),
        },
        AiStreamEvent::ToolCallStart {
            id: "tool_swarm".to_owned(),
            name: "DelegateSwarm".to_owned(),
        },
        AiStreamEvent::ToolCallArgsDelta {
            id: "tool_swarm".to_owned(),
            json_fragment: r#"{"description":"inspect modules","items":[{"title":"api","value":"api"},{"title":"tui","value":"tui"},{"title":"runtime","value":"runtime"}],"prompt_template":"Check {{item}}","max_concurrency":2}"#.to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: "tool_swarm".to_owned(),
            raw_arguments: json!({
                "description": "inspect modules",
                "items": [
                    {"title": "api", "value": "api"},
                    {"title": "tui", "value": "tui"},
                    {"title": "runtime", "value": "runtime"}
                ],
                "prompt_template": "Check {{item}}",
                "max_concurrency": 2
            }).to_string(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::ToolUse,
            usage: None,
        },
    ], vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "child_api".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "api ok".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ], vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "child_tui".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "tui ok".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ], vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "child_runtime".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "runtime ok".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]])
}

fn assert_named_swarm_lifecycle(events: &[AgentEvent], harness: &FakeHarness) {
    assert!(
        harness.requests().len() >= 4,
        "parent plus three child turns should run"
    );
    let finished_swarm = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::DelegateSwarmFinished { turn, swarm, .. } => Some((*turn, swarm)),
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
            AgentEvent::DelegateSwarmProgressUpdated {
                turn,
                child_progress,
                ..
            } => Some((*turn, child_progress)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        updates.len() >= 6,
        "progress updates should stream child start/text/finish, got {}",
        updates.len()
    );
    assert_eq!(updates[0].0, 1);
    assert!(
        updates
            .iter()
            .any(|(_, child)| { child.progress.latest_text.as_deref() == Some("api ok") }),
        "progress updates should expose child text before final swarm: {updates:#?}"
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
}

fn assert_named_swarm_result(events: &[AgentEvent]) {
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
