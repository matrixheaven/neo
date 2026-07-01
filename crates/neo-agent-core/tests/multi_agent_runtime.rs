use futures::StreamExt;
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
use neo_ai::{AiStreamEvent, StopReason};
use serde_json::json;
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
fn display_name_pool_suffixes_after_exhaustion() {
    let mut pool = DisplayNamePool::default();
    for _ in 0..DEFAULT_AGENT_NAMES.len() {
        let _ = pool.next_name();
    }

    let wrapped = pool.next_name();

    assert_eq!(wrapped.as_str(), format!("{}2", DEFAULT_AGENT_NAMES[0]));
}

#[test]
fn foreground_delegate_lifecycle_records_running_and_completed_state() {
    let runtime = MultiAgentRuntime::new();

    let running = runtime.start_foreground_delegate_for_test("inspect queue");
    assert_eq!(running.state, AgentLifecycleState::Running);
    assert_eq!(running.display_name.as_str(), "Zeno");

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
                arguments: json!({ "task": "test task" }),
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
                arguments: json!({
                    "task": "inspect queue",
                    "role": "reviewer",
                    "context": "inherit"
                }),
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
                arguments: json!({ "task": "inspect lib" }),
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
                arguments: json!({ "file_path": "crates/neo-agent-core/src/lib.rs" }),
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
                arguments: json!({ "task": "try recursive delegation" }),
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
                arguments: json!({ "task": "try hidden task output" }),
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
                arguments: json!({}),
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
        neo_agent_core::multi_agent::AgentPathKind::Root,
    );
    let started_at = std::time::Instant::now();

    for (id, file_path) in [("read_ok", "ok.rs"), ("read_fail", "missing.rs")] {
        let _ = runtime.apply_child_event(
            &agent.id,
            started_at,
            &AgentEvent::ToolExecutionStarted {
                turn: 1,
                id: id.to_owned(),
                name: "Read".to_owned(),
                arguments: json!({ "file_path": file_path }),
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
            _ => None,
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
            arguments: json!({
                "description": "inspect modules",
                "items": ["api", "tui", "runtime"],
                "prompt_template": "Check {{item}}",
                "max_concurrency": 2
            }),
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
    assert_eq!(names, vec!["Zeno", "Gibbs", "Hokke"]);
    assert!(!names.iter().any(|name| name.starts_with("child-")));

    let delegate_result = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionFinished { result, .. }
                if result.content.contains("status: completed")
                    && result.content.contains("items: 3") =>
            {
                Some(result)
            }
            _ => None,
        })
        .expect("swarm tool result");
    assert!(delegate_result.content.contains("api ok"));
    assert!(delegate_result.content.contains("tui ok"));
    assert!(delegate_result.content.contains("runtime ok"));
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
                arguments: json!({
                    "description": "canonical title",
                    "items": ["alpha", "beta"],
                    "prompt_template": "Review {{item}} for {{description}}",
                    "max_concurrency": 2
                }),
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
        swarm_result.content.contains("agent_id:"),
        "{}",
        swarm_result.content
    );
    assert!(
        swarm_result.content.contains("status: completed"),
        "{}",
        swarm_result.content
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
    assert!(
        second.content.contains("status: completed"),
        "{}",
        second.content
    );
}

#[test]
fn delegate_and_message_descriptions_explain_resume_and_live_followup() {
    let registry = ToolRegistry::with_builtin_tools_and_todos(Arc::new(Mutex::new(Vec::new())));
    let specs = registry.specs();
    let delegate = specs
        .iter()
        .find(|spec| spec.name == "Delegate")
        .expect("Delegate spec registered");
    let message = specs
        .iter()
        .find(|spec| spec.name == "MessageDelegate")
        .expect("MessageDelegate spec registered");

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
        message.description.contains("running"),
        "{}",
        message.description
    );
    assert!(
        message.description.contains("Delegate with resume"),
        "{}",
        message.description
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
    assert!(
        swarm.content.contains(agent_id.as_str()),
        "{}",
        swarm.content
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
