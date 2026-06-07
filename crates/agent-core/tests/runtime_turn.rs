use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, AgentToolCall, Content,
    PermissionDecision, PermissionOperation, PermissionPolicy, QueueMode, StopReason, Tool,
    ToolContext, ToolExecutionMode, ToolFuture, ToolRegistry, ToolResult, harness::FakeHarness,
};
use neo_ai::{
    AiError, AiStreamEvent, ApiKind, ChatRequest, ModelCapabilities, ModelClient, ModelSpec,
    ProviderId, ToolSpec,
};
use serde_json::json;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::{sleep, timeout};

#[tokio::test]
async fn runtime_streams_one_turn_text_and_updates_context() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "hel".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "lo".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("say hello"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert_eq!(
        events.first(),
        Some(&AgentEvent::MessageAppended {
            message: AgentMessage::user_text("say hello"),
        })
    );
    assert_eq!(events.get(1), Some(&AgentEvent::TurnStarted { turn: 1 }));
    assert!(events.contains(&AgentEvent::TextDelta {
        turn: 1,
        text: "hel".to_owned()
    }));
    assert!(events.contains(&AgentEvent::TextDelta {
        turn: 1,
        text: "lo".to_owned()
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::TurnFinished {
            turn: 1,
            stop_reason: StopReason::EndTurn,
        })
    );
    assert_eq!(context.messages()[0], AgentMessage::user_text("say hello"));
    assert_eq!(
        context.messages()[1],
        AgentMessage::assistant([Content::text("hello")], Vec::new(), StopReason::EndTurn)
    );
}

#[tokio::test]
async fn runtime_yields_model_events_before_model_stream_finishes() {
    let harness = DelayedHarness::new(vec![
        DelayedStep::Event(AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        }),
        DelayedStep::Event(AiStreamEvent::TextDelta {
            text: "early".to_owned(),
        }),
        DelayedStep::Delay(Duration::from_secs(5)),
        DelayedStep::Event(AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        }),
    ]);
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());
    let mut context = AgentContext::new();

    let mut stream = runtime.run_turn(&mut context, AgentMessage::user_text("stream"));

    assert_eq!(
        timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("prompt append should stream before delayed message end")
            .expect("prompt append event")
            .expect("prompt append should be ok"),
        AgentEvent::MessageAppended {
            message: AgentMessage::user_text("stream"),
        }
    );
    assert_eq!(
        timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("turn start should stream before delayed message end")
            .expect("turn start event")
            .expect("turn start should be ok"),
        AgentEvent::TurnStarted { turn: 1 }
    );
    assert_eq!(
        timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("message start should stream before delayed message end")
            .expect("message start event")
            .expect("message start should be ok"),
        AgentEvent::MessageStarted {
            turn: 1,
            id: "msg_1".to_owned(),
        }
    );
    assert_eq!(
        timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("text delta should stream before delayed message end")
            .expect("text delta event")
            .expect("text delta should be ok"),
        AgentEvent::TextDelta {
            turn: 1,
            text: "early".to_owned(),
        }
    );
}

#[tokio::test]
async fn runtime_records_tool_calls_and_sends_tool_specs_to_model() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_2".to_owned(),
        },
        AiStreamEvent::ToolCallStart {
            id: "tool_1".to_owned(),
            name: "read".to_owned(),
        },
        AiStreamEvent::ToolCallArgsDelta {
            id: "tool_1".to_owned(),
            json_fragment: r#"{"path":"README.md"}"#.to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: "tool_1".to_owned(),
            arguments: json!({ "path": "README.md" }),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::ToolUse,
            usage: None,
        },
    ]);
    let tool = ToolSpec {
        name: "read".to_owned(),
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
            id: "tool_1".to_owned(),
            name: "read".to_owned(),
            arguments: json!({ "path": "README.md" }),
        },
    }));
    assert_eq!(
        context.messages()[1],
        AgentMessage::assistant(
            Vec::new(),
            vec![AgentToolCall {
                id: "tool_1".to_owned(),
                name: "read".to_owned(),
                arguments: json!({ "path": "README.md" }),
            }],
            StopReason::ToolUse,
        )
    );
    assert_eq!(harness.requests()[0].tools, vec![tool]);
}

#[tokio::test]
async fn runtime_reports_max_turns_and_cancelled_without_calling_model() {
    let harness = FakeHarness::from_events([]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_max_turns(0),
        harness.client(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("stop"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("max turn event");

    assert_eq!(
        events,
        vec![AgentEvent::TurnFinished {
            turn: 1,
            stop_reason: StopReason::MaxTurns,
        }]
    );
    assert!(harness.requests().is_empty());

    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());
    context.cancel();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("cancelled"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("cancel event");
    assert_eq!(
        events,
        vec![AgentEvent::TurnFinished {
            turn: 1,
            stop_reason: StopReason::Cancelled,
        }]
    );
}

#[tokio::test]
async fn runtime_executes_tool_call_and_continues_until_end_turn() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "text": "neo" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "tool said: neo".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()),
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
            id: "tool_1".to_owned(),
            name: "echo".to_owned(),
            arguments: json!({ "text": "neo" }),
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
}

#[tokio::test]
async fn runtime_drains_queued_steering_before_followups() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "first".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "second".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_3".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "third".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model())
            .with_queue_modes(QueueMode::OneAtATime, QueueMode::All),
        harness.client(),
    );
    let mut context = AgentContext::new();
    context.queue_steering_message(AgentMessage::user_text("steer one"));
    context.queue_steering_message(AgentMessage::user_text("steer two"));
    context.queue_follow_up_message(AgentMessage::user_text("follow"));

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("start"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("queued run should succeed");

    let appended = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MessageAppended { message } => Some(message.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        appended,
        vec![
            AgentMessage::user_text("start"),
            AgentMessage::user_text("steer one"),
            AgentMessage::assistant([Content::text("first")], Vec::new(), StopReason::EndTurn),
            AgentMessage::user_text("steer two"),
            AgentMessage::assistant([Content::text("second")], Vec::new(), StopReason::EndTurn),
            AgentMessage::user_text("follow"),
            AgentMessage::assistant([Content::text("third")], Vec::new(), StopReason::EndTurn),
        ]
    );
    assert_eq!(context.pending_steering_len(), 0);
    assert_eq!(context.pending_follow_up_len(), 0);
    assert_eq!(harness.requests().len(), 3);
    assert!(matches!(
        harness.requests()[0].messages.last(),
        Some(neo_ai::ChatMessage::User { content }) if matches!(
            content.first(),
            Some(neo_ai::ContentPart::Text { text }) if text == "steer one"
        )
    ));
    assert!(matches!(
        harness.requests()[1].messages.last(),
        Some(neo_ai::ChatMessage::User { content }) if matches!(
            content.first(),
            Some(neo_ai::ContentPart::Text { text }) if text == "steer two"
        )
    ));
    assert!(matches!(
        events.last(),
        Some(AgentEvent::TurnFinished {
            turn: 3,
            stop_reason: StopReason::EndTurn,
        })
    ));
}

#[tokio::test]
async fn runtime_applies_context_transform_before_model_request() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "trimmed".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_context_transform(|messages| {
            messages
                .iter()
                .filter(|message| {
                    !matches!(
                        message,
                        AgentMessage::User { content }
                            if content.iter().any(|part| part.as_text() == Some("drop"))
                    )
                })
                .cloned()
                .collect()
        }),
        harness.client(),
    );
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("drop"));

    runtime
        .run_turn(&mut context, AgentMessage::user_text("keep"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert_eq!(harness.requests()[0].messages.len(), 1);
    assert!(matches!(
        &harness.requests()[0].messages[0],
        neo_ai::ChatMessage::User { content } if matches!(
            content.first(),
            Some(neo_ai::ContentPart::Text { text }) if text == "keep"
        )
    ));
    assert_eq!(context.messages()[0], AgentMessage::user_text("drop"));
    assert_eq!(context.messages()[1], AgentMessage::user_text("keep"));
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
            .with_before_tool_call(|call| {
                if call
                    .arguments
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    == Some("blocked")
                {
                    Some(ToolResult::error("blocked by policy").terminate())
                } else {
                    None
                }
            })
            .with_after_tool_call(|call, mut result| {
                if call
                    .arguments
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    == Some("stop")
                {
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
    assert!(events.contains(&AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        arguments: json!({ "text": "blocked" }),
    }));
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::error("blocked by policy").terminate(),
    }));
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_2".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::ok("stop").terminate(),
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
async fn runtime_emits_approval_request_for_ask_permission_and_skips_tool_execution() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "text": "needs approval" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let executed_for_hook = Arc::clone(&executed);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_permission_policy(PermissionPolicy {
                file_read: PermissionDecision::Allow,
                file_write: PermissionDecision::Deny,
                shell: PermissionDecision::Deny,
                tool: PermissionDecision::Ask,
            })
            .with_after_tool_call(move |call, result| {
                executed_for_hook
                    .lock()
                    .expect("executed lock poisoned")
                    .push(call.id.clone());
                result
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call tool"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    assert!(events.contains(&AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool_1".to_owned(),
        operation: PermissionOperation::Tool,
        subject: "echo".to_owned(),
        arguments: json!({ "text": "needs approval" }),
    }));
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::error("approval required for tool: echo"),
    }));
    assert!(executed.lock().expect("executed lock poisoned").is_empty());
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result(
            "tool_1",
            "echo",
            vec![Content::text("approval required for tool: echo")],
            true
        )
    );
}

#[tokio::test]
async fn runtime_executes_ask_permission_tool_after_approval_hook_allows_it() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "text": "approved" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_permission_policy(PermissionPolicy {
                file_read: PermissionDecision::Allow,
                file_write: PermissionDecision::Deny,
                shell: PermissionDecision::Deny,
                tool: PermissionDecision::Ask,
            })
            .with_approval_handler(|request| {
                assert_eq!(request.operation, PermissionOperation::Tool);
                assert_eq!(request.subject, "echo");
                PermissionDecision::Allow
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call tool"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("approved tool loop should succeed");

    assert!(events.contains(&AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool_1".to_owned(),
        operation: PermissionOperation::Tool,
        subject: "echo".to_owned(),
        arguments: json!({ "text": "approved" }),
    }));
    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["approved".to_owned()]
    );
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::ok("approved"),
    }));
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result("tool_1", "echo", vec![Content::text("approved")], false)
    );
}

#[tokio::test]
async fn runtime_skips_ask_permission_tool_after_approval_hook_denies_it() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "text": "denied" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_permission_policy(PermissionPolicy {
                file_read: PermissionDecision::Allow,
                file_write: PermissionDecision::Deny,
                shell: PermissionDecision::Deny,
                tool: PermissionDecision::Ask,
            })
            .with_approval_handler(|request| {
                assert_eq!(request.operation, PermissionOperation::Tool);
                assert_eq!(request.subject, "echo");
                PermissionDecision::Deny
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call tool"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("denied tool loop should succeed");

    assert!(events.contains(&AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool_1".to_owned(),
        operation: PermissionOperation::Tool,
        subject: "echo".to_owned(),
        arguments: json!({ "text": "denied" }),
    }));
    assert!(executed.lock().expect("executed lock poisoned").is_empty());
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::error("approval denied for tool: echo"),
    }));
}

#[tokio::test]
async fn runtime_approval_handler_allows_file_write_tool_permission() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "write".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "path": "approved.txt", "content": "ok" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_permission_policy(PermissionPolicy {
                file_read: PermissionDecision::Allow,
                file_write: PermissionDecision::Ask,
                shell: PermissionDecision::Deny,
                tool: PermissionDecision::Allow,
            })
            .with_workspace_root(workspace.path())
            .expect("workspace config")
            .with_approval_handler(|request| {
                assert_eq!(request.operation, PermissionOperation::FileWrite);
                assert_eq!(request.subject, "approved.txt");
                PermissionDecision::Allow
            }),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("write file"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("approved write should succeed");

    assert!(events.contains(&AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool_1".to_owned(),
        operation: PermissionOperation::FileWrite,
        subject: "approved.txt".to_owned(),
        arguments: json!({ "path": "approved.txt", "content": "ok" }),
    }));
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("approved.txt")).expect("written file"),
        "ok"
    );
}

#[tokio::test]
async fn runtime_emits_shell_lifecycle_for_bash_tool() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "command": "printf shell-ok" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_permission_policy(PermissionPolicy::allow_all()),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run shell"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("shell tool should succeed");

    assert!(events.contains(&AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "tool_1".to_owned(),
        command: "printf shell-ok".to_owned(),
        cwd: std::env::current_dir().expect("cwd"),
    }));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ShellCommandFinished {
            turn: 1,
            id,
            exit_code: Some(0),
            stdout,
            stderr,
            ..
        } if id == "tool_1" && stdout.contains("shell-ok") && stderr.is_empty()
    )));
}

fn blocking_then_terminating_tool_harness() -> FakeHarness {
    FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "text": "blocked" }),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                arguments: json!({ "text": "stop" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "should not run".to_owned(),
            },
            AiStreamEvent::MessageEnd {
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
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "sleep_echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "text": "slow", "delay_ms": 40 }),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "sleep_echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                arguments: json!({ "text": "fast", "delay_ms": 0 }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(SleepEchoTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Parallel),
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
            } => Some(tool_call_id.as_str()),
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

struct EchoTool;

impl Tool for EchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn description(&self) -> &'static str {
        "Echo text."
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

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            Ok(ToolResult::ok(
                input
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
            ))
        })
    }
}

struct RecordingEchoTool {
    executed: Arc<Mutex<Vec<String>>>,
}

impl Tool for RecordingEchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn description(&self) -> &'static str {
        "Record and echo text."
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

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let text = input
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            self.executed
                .lock()
                .expect("executed lock poisoned")
                .push(text.clone());
            Ok(ToolResult::ok(text))
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

#[derive(Clone)]
struct DelayedHarness {
    model: ModelSpec,
    client: Arc<DelayedModelClient>,
}

impl DelayedHarness {
    fn new(steps: Vec<DelayedStep>) -> Self {
        Self {
            model: ModelSpec {
                provider: ProviderId("delayed".to_owned()),
                model: "delayed-agent-model".to_owned(),
                api: ApiKind::Local,
                capabilities: ModelCapabilities {
                    streaming: true,
                    tools: true,
                    images: false,
                    reasoning: false,
                    embeddings: false,
                    max_context_tokens: None,
                },
            },
            client: Arc::new(DelayedModelClient {
                steps: Mutex::new(Some(steps)),
                requests: Mutex::new(Vec::new()),
            }),
        }
    }

    fn model(&self) -> ModelSpec {
        self.model.clone()
    }

    fn client(&self) -> Arc<dyn ModelClient> {
        self.client.clone()
    }
}

#[derive(Clone)]
enum DelayedStep {
    Event(AiStreamEvent),
    Delay(Duration),
}

struct DelayedModelClient {
    steps: Mutex<Option<Vec<DelayedStep>>>,
    requests: Mutex<Vec<ChatRequest>>,
}

impl ModelClient for DelayedModelClient {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        self.requests
            .lock()
            .expect("request lock poisoned")
            .push(request);
        let steps = self
            .steps
            .lock()
            .expect("steps lock poisoned")
            .take()
            .unwrap_or_default();
        futures::stream::unfold(steps.into_iter(), |mut steps| async move {
            loop {
                match steps.next()? {
                    DelayedStep::Event(event) => return Some((Ok(event), steps)),
                    DelayedStep::Delay(duration) => sleep(duration).await,
                }
            }
        })
        .boxed()
    }
}
