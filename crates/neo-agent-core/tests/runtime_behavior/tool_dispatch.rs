use super::fake_harness::EchoTool;
use super::fake_harness::RecordingEchoTool;
use super::fake_harness::assert_runtime_rejects_unsupported_capability;
use super::fake_harness::end_turn_events;
use super::fake_harness::final_done_turn;
use super::fake_harness::model_with_capabilities;
use super::fake_harness::tool_call_turn;
use super::tool_dispatch_edit::InvokeStoredWorkflowDispatchHandleTool;
use super::tool_dispatch_edit::NestedWorkflowEchoTool;
use super::tool_dispatch_edit::StoreWorkflowDispatchHandleTool;
use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, AgentToolCall, Content,
    PermissionMode, SkillInvocationOutcome, SkillInvocationSource, StopReason, TodoEventData, Tool,
    ToolContext, ToolError, ToolExecutionMode, ToolFuture, ToolRegistry, ToolResult,
    harness::FakeHarness, skills::SkillStore,
};
use neo_ai::{AiStreamEvent, MessagePhase, ModelCapabilities, ToolSpec};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;

#[tokio::test]
async fn runtime_records_tool_calls_and_sends_tool_specs_to_model() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: "msg_2".to_owned(),
        },
        AiStreamEvent::ToolCallStart {
            id: "tool_1".to_owned(),
            name: "Read".to_owned(),
        },
        AiStreamEvent::ToolCallArgsDelta {
            id: "tool_1".to_owned(),
            json_fragment: r#"{"path":"README.md"}"#.to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: "tool_1".to_owned(),
            raw_arguments: json!({ "path": "README.md" }).to_string(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: neo_ai::StopReason::ToolUse,
            usage: None,
        },
    ]);
    let tool = ToolSpec {
        name: "Read".to_owned(),
        description: "read file".to_owned(),
        input_schema: json!({ "type": "object" }),
    };
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_tools(vec![tool.clone()]),
        harness.client(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("read README"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.contains(&AgentEvent::ToolCallFinished {
        turn: 1,
        tool_call: AgentToolCall {
            id: "tool_1".into(),
            name: "Read".into(),
            raw_arguments: json!({ "path": "README.md" }).to_string().into(),
        },
    }));
    assert_eq!(
        context.messages()[1],
        AgentMessage::assistant(
            Vec::new(),
            vec![AgentToolCall {
                id: "tool_1".into(),
                name: "Read".into(),
                raw_arguments: json!({ "path": "README.md" }).to_string().into(),
            }],
            StopReason::ToolUse,
        )
    );
    assert_eq!(harness.requests()[0].tools, vec![tool]);
}

#[tokio::test]
async fn runtime_rejects_tools_when_model_lacks_tools_before_request() {
    let harness = FakeHarness::from_events([AiStreamEvent::MessageEnd {
        phase: MessagePhase::Unknown,
        stop_reason: neo_ai::StopReason::EndTurn,
        usage: None,
    }]);
    let tool = ToolSpec::string_arg("Read", "read file", "path", "file path");
    let config = AgentConfig::for_model(model_with_capabilities(ModelCapabilities::chat()))
        .with_tools(vec![tool]);

    assert_runtime_rejects_unsupported_capability(
        config,
        &harness,
        AgentMessage::user_text("read README"),
        "does not support tools",
        "unsupported tools should fail before provider request",
    )
    .await;
}

#[tokio::test]
async fn runtime_executes_tool_call_and_continues_until_end_turn() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "text": "neo" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "tool said: neo".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call echo"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    assert!(events.contains(&AgentEvent::ToolCallFinished {
        turn: 1,
        tool_call: AgentToolCall {
            id: "tool_1".into(),
            name: "echo".into(),
            raw_arguments: json!({ "text": "neo" }).to_string().into(),
        },
    }));
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result("tool_1", "echo", vec![Content::text("neo")], false)
    );
    assert_eq!(
        context.messages()[3],
        AgentMessage::assistant(
            vec![Content::text("tool said: neo")],
            Vec::new(),
            StopReason::EndTurn
        )
    );
    assert_eq!(harness.requests().len(), 2);
    assert!(matches!(
        harness.requests()[1].messages.last(),
        Some(neo_ai::ChatMessage::ToolResult { tool_call_id, .. }) if tool_call_id == "tool_1"
    ));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::RunFinished { .. }))
            .count(),
        1
    );
    assert!(events.contains(&AgentEvent::MessageFinished {
        turn: 1,
        id: "msg_1".to_owned(),
        stop_reason: StopReason::ToolUse,
        phase: MessagePhase::Unknown,
    }));
    assert!(events.contains(&AgentEvent::MessageFinished {
        turn: 2,
        id: "msg_2".to_owned(),
        stop_reason: StopReason::EndTurn,
        phase: MessagePhase::Unknown,
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 2,
            stop_reason: StopReason::EndTurn,
        })
    );
}

#[tokio::test]
async fn runtime_emits_todo_update_only_for_writes() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_write".to_owned(),
                name: "TodoList".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_write".to_owned(),
                raw_arguments: json!({
                    "todos": [{ "title": "Read code", "status": "in_progress" }]
                })
                .to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_read".to_owned(),
                name: "TodoList".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_read".to_owned(),
                raw_arguments: json!({}).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_3".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let config = AgentConfig::for_model(harness.model());
    let tools = ToolRegistry::with_builtin_tools_and_todos(Arc::clone(&config.todos));
    let runtime = AgentRuntime::with_tools(config, harness.client(), tools);
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("track todos"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("todo tool loop should succeed");

    let todo_events = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TodoUpdated { todos, .. } => Some(todos),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(todo_events.len(), 1);
    assert_eq!(todo_events[0][0].title, "Read code");
    assert_eq!(todo_events[0][0].status, "in_progress");
    assert_eq!(context.todos(), todo_events[0].as_slice());
    assert!(
        context.messages().iter().any(|message| {
            matches!(
                message,
                AgentMessage::ToolResult { tool_call_id, content, .. }
                    if tool_call_id.as_ref() == "tool_read"
                        && content.iter().any(|part| matches!(part, Content::Text { text } if text.contains("[in_progress] Read code")))
            )
        }),
        "read-mode tool result should include current todos"
    );
}

#[tokio::test]
async fn runtime_emits_empty_todo_update_for_clear() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_clear".to_owned(),
                name: "TodoList".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_clear".to_owned(),
                raw_arguments: json!({ "todos": [] }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "cleared".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let config = AgentConfig::for_model(harness.model());
    config
        .todos
        .lock()
        .expect("todo state")
        .push(TodoEventData {
            title: "Old".to_owned(),
            status: "done".to_owned(),
        });
    let tools = ToolRegistry::with_builtin_tools_and_todos(Arc::clone(&config.todos));
    let runtime = AgentRuntime::with_tools(config, harness.client(), tools);
    let mut context = AgentContext::from_replay(
        [AgentEvent::TodoUpdated {
            turn: 0,
            todos: vec![TodoEventData {
                title: "Old".to_owned(),
                status: "done".to_owned(),
            }],
        }]
        .iter(),
    );

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("clear todos"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("todo clear should succeed");

    assert!(events.iter().any(|event| {
        matches!(event, AgentEvent::TodoUpdated { todos, .. } if todos.is_empty())
    }));
    assert!(context.todos().is_empty());
}

#[tokio::test]
async fn runtime_stops_on_tool_use_with_empty_tool_calls() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_empty_tools".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_should_not_run".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "followup".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()),
        harness.client(),
        ToolRegistry::new(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("try a tool"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("empty tool use should fail closed in-band");

    assert!(events.contains(&AgentEvent::Error {
        turn: 1,
        message: "Provider reported tool calls but emitted no structured tool calls".to_owned(),
        code: None,
        retry_after: None,
    }));
    assert!(events.contains(&AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: StopReason::Error,
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Error,
        })
    );
    assert_eq!(harness.requests().len(), 1);
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::MessageStarted { id, .. } if id == "msg_should_not_run"
    )));
}

#[tokio::test]
async fn runtime_returns_tool_errors_to_model_for_retry_instead_of_aborting() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "fallible".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "bad": true }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "retry noted".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(FallibleTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call fallible"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool error should be returned to the model");

    assert_eq!(harness.requests().len(), 2);
    assert!(events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ToolExecutionFinished {
                id,
                result: ToolResult { is_error: true, content, .. },
                ..
            } if id == "tool_1" && content.contains("invalid input for fallible")
        )
    }));
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result(
            "tool_1",
            "fallible",
            vec![Content::text("invalid input for fallible: expected text")],
            true
        )
    );
    assert_eq!(
        context.messages()[3],
        AgentMessage::assistant(
            vec![Content::text("retry noted")],
            Vec::new(),
            StopReason::EndTurn
        )
    );
    assert!(matches!(
        harness.requests()[1].messages.last(),
        Some(neo_ai::ChatMessage::ToolResult {
            tool_call_id,
            is_error: true,
            ..
        }) if tool_call_id == "tool_1"
    ));
}

#[tokio::test]
async fn runtime_emits_tool_execution_events_and_honors_block_and_terminate_hooks() {
    let harness = blocking_then_terminating_tool_harness();
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Sequential)
            .with_permission_mode(PermissionMode::Yolo)
            .with_before_tool_call(|call| {
                let args: serde_json::Value =
                    serde_json::from_str(&call.raw_arguments).unwrap_or_default();
                if args.get("text").and_then(serde_json::Value::as_str) == Some("blocked") {
                    Some(ToolResult::error("blocked by policy").terminate())
                } else {
                    None
                }
            })
            .with_after_tool_call(|call, mut result| {
                let args: serde_json::Value =
                    serde_json::from_str(&call.raw_arguments).unwrap_or_default();
                if args.get("text").and_then(serde_json::Value::as_str) == Some("stop") {
                    result = result.terminate();
                }
                result
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call tools"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["stop".to_owned()]
    );
    // Authorization runs before execution starts: a call blocked by the
    // before-hook finishes without ever emitting ToolExecutionStarted.
    assert!(
        !events.contains(&AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "tool_1".to_owned(),
            name: "echo".to_owned(),
            arguments: json!({ "text": "blocked" }),
            workflow_origin: None,
            output_ref: None,
        }),
        "a hook-blocked call never starts execution"
    );
    assert!(events.contains(&AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool_2".to_owned(),
        name: "echo".to_owned(),
        arguments: json!({ "text": "stop" }),
        workflow_origin: None,
        output_ref: None,
    }));
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::error("blocked by policy").terminate(),
        workflow_origin: None,
        output_ref: None,
    }));
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_2".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::ok("stop").terminate(),
        workflow_origin: None,
        output_ref: None,
    }));
    assert_eq!(harness.requests().len(), 1);
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result(
            "tool_1",
            "echo",
            vec![Content::text("blocked by policy")],
            true
        )
    );
    assert_eq!(
        context.messages()[3],
        AgentMessage::tool_result("tool_2", "echo", vec![Content::text("stop")], false)
    );
}

#[tokio::test]
async fn runtime_does_not_replay_partial_tool_arguments_to_followup_request() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: r#"{"command":"printf shell-ok","cwd":"#.to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let workspace = tempfile::tempdir().expect("tempdir");
    let workspace_root = workspace.path().canonicalize().expect("canonicalize");
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_workspace_root(&workspace_root)
            .expect("workspace root"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("run shell"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("shell tool should succeed");

    let requests = harness.requests();
    assert_eq!(requests.len(), 2);
    let assistant = requests[1]
        .messages
        .iter()
        .find_map(|message| match message {
            neo_ai::ChatMessage::Assistant { tool_calls, .. } => tool_calls.first(),
            _ => None,
        })
        .expect("assistant tool call replayed");
    assert_eq!(
        assistant.raw_arguments, r#"{"command":"printf shell-ok"}"#,
        "follow-up request must not replay partial JSON tool arguments"
    );
}

fn blocking_then_terminating_tool_harness() -> FakeHarness {
    FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "text": "blocked" }).to_string(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                raw_arguments: json!({ "text": "stop" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "should not run".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ])
}

#[tokio::test]
async fn runtime_parallel_tool_mode_finishes_by_completion_but_appends_in_source_order() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "sleep_echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "text": "slow", "delay_ms": 40 }).to_string(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "sleep_echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                raw_arguments: json!({ "text": "fast", "delay_ms": 0 }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(SleepEchoTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Parallel)
            .with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call tools"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    let execution_end_ids = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolExecutionFinished { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(execution_end_ids, vec!["tool_2", "tool_1"]);

    let appended_tool_ids = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MessageAppended {
                message: AgentMessage::ToolResult { tool_call_id, .. },
            } => Some(tool_call_id.as_ref()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(appended_tool_ids, vec!["tool_1", "tool_2"]);
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result("tool_1", "sleep_echo", vec![Content::text("slow")], false)
    );
    assert_eq!(
        context.messages()[3],
        AgentMessage::tool_result("tool_2", "sleep_echo", vec![Content::text("fast")], false)
    );
}

#[tokio::test]
async fn parallel_mode_serializes_non_background_ask_user_question() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "AskUserQuestion".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({
                    "questions": [{
                        "question": "Continue?",
                        "options": [
                            { "label": "Yes" },
                            { "label": "No" }
                        ]
                    }]
                })
                .to_string(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                raw_arguments: json!({ "text": "should wait" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let (question_tx, mut question_rx) = mpsc::unbounded_channel();
    let mut tools = ToolRegistry::new();
    tools.register(neo_agent_core::AskUserTool::new(question_tx));
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Parallel)
            .with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let mut stream = runtime.run_turn(&mut context, AgentMessage::user_text("ask and echo"));
    let pending = timeout(Duration::from_millis(250), question_rx.recv())
        .await
        .expect("question should be requested before other tools run")
        .expect("question should be pending");
    assert!(
        executed.lock().expect("executed lock poisoned").is_empty(),
        "non-dialog tools must wait while AskUserQuestion is waiting for the user"
    );

    pending
        .response_tx
        .send(neo_agent_core::QuestionResponse {
            answers: vec!["Yes".to_owned()],
        })
        .expect("send question response");
    while let Some(event) = stream.next().await {
        event.expect("event should be ok");
    }
    drop(stream);

    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["should wait".to_owned()]
    );
}

#[tokio::test]
async fn automatic_missing_skill_emits_failed_skill_invocation() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Skill".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({"skill": "missing"}).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools_and_skills(
        AgentConfig::for_model(harness.model()),
        harness.client(),
        ToolRegistry::new(),
        SkillStore::default(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("use missing skill"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("missing skill result should return to the model");

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::SkillInvocation {
                names,
                source: SkillInvocationSource::Auto,
                outcome: SkillInvocationOutcome::Failed,
                body,
            } if names == &["missing".to_owned()]
                && body.contains("skill `missing` is not available")
        )),
        "missing Skill should emit a semantic failure event; events: {events:#?}"
    );
}

struct FallibleTool;

impl Tool for FallibleTool {
    fn name(&self) -> &'static str {
        "fallible"
    }

    fn description(&self) -> &'static str {
        "Always returns a tool-layer error."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string" }
            },
            "required": ["text"]
        })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async {
            Err(ToolError::InvalidInput {
                tool: "fallible".to_owned(),
                message: "expected text".to_owned(),
            })
        })
    }
}

struct SleepEchoTool;

impl Tool for SleepEchoTool {
    fn name(&self) -> &'static str {
        "sleep_echo"
    }

    fn description(&self) -> &'static str {
        "Sleep and echo text."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string" },
                "delay_ms": { "type": "integer" }
            },
            "required": ["text", "delay_ms"]
        })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let text = input
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let delay_ms = input
                .get("delay_ms")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default();
            if delay_ms > 0 {
                let mut pending_once = true;
                futures::future::poll_fn(move |cx| {
                    if pending_once {
                        pending_once = false;
                        cx.waker().wake_by_ref();
                        std::task::Poll::Pending
                    } else {
                        std::task::Poll::Ready(())
                    }
                })
                .await;
            }
            Ok(ToolResult::ok(text))
        })
    }
}

#[tokio::test]
async fn stored_workflow_handle_routes_nested_events_only_to_the_active_turn() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[("store_dispatch", "StoreWorkflowDispatchHandle", json!({}))]),
        end_turn_events("stored"),
        tool_call_turn(&[(
            "invoke_dispatch",
            "InvokeStoredWorkflowDispatchHandle",
            json!({}),
        )]),
        end_turn_events("invoked"),
    ]);
    let slot = Arc::new(Mutex::new(None));
    let mut registry = ToolRegistry::new();
    registry.register(StoreWorkflowDispatchHandleTool {
        slot: Arc::clone(&slot),
    });
    registry.register(InvokeStoredWorkflowDispatchHandleTool {
        slot: Arc::clone(&slot),
    });
    registry.register(NestedWorkflowEchoTool);
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(workspace.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo);
    let runtime = AgentRuntime::with_tools(config, harness.client(), registry);
    let mut context = AgentContext::new();

    let turn_one = timeout(
        Duration::from_secs(5),
        runtime
            .run_turn(&mut context, AgentMessage::user_text("store handle"))
            .collect::<Vec<_>>(),
    )
    .await
    .expect("turn one stream closes while handle remains stored")
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .expect("turn one succeeds");
    assert!(slot.lock().expect("dispatch slot").is_some());
    assert!(!turn_one.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionStarted { id, .. }
            | AgentEvent::ToolExecutionFinished { id, .. }
            if id == "nested_turn_two"
    )));

    let turn_two = timeout(
        Duration::from_secs(5),
        runtime
            .run_turn(&mut context, AgentMessage::user_text("invoke handle"))
            .collect::<Vec<_>>(),
    )
    .await
    .expect("turn two stream closes after nested completion")
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .expect("turn two succeeds");
    let active_turn = turn_two
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionStarted { turn, id, .. } if id == "invoke_dispatch" => {
                Some(*turn)
            }
            _ => None,
        })
        .expect("turn two outer tool start");
    assert!(turn_two.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionStarted { turn, id, name, .. }
            if *turn == active_turn
                && id == "nested_turn_two"
                && name == "NestedWorkflowEcho"
    )));
    assert!(turn_two.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished { turn, id, name, .. }
            if *turn == active_turn
                && id == "nested_turn_two"
                && name == "NestedWorkflowEcho"
    )));
}
