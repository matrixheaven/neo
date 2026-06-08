use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, AgentRuntimeError,
    AgentToolCall, ApprovalRequest, CompactionSettings, Content, PermissionDecision,
    PermissionOperation, PermissionPolicy, QueueMode, StopReason, Tool, ToolContext, ToolError,
    ToolExecutionMode, ToolFuture, ToolRegistry, ToolResult, harness::FakeHarness,
};
use neo_ai::{
    AiError, AiStreamEvent, ApiKind, ChatRequest, ModelCapabilities, ModelClient, ModelSpec,
    ProviderId, ReasoningEffort, ToolSpec,
};
use serde_json::json;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{Notify, oneshot},
    time::{sleep, timeout},
};
use tokio_util::sync::CancellationToken;

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
        events,
        vec![
            AgentEvent::RunStarted { turn: 1 },
            AgentEvent::MessageAppended {
                message: AgentMessage::user_text("say hello"),
            },
            AgentEvent::TurnStarted { turn: 1 },
            AgentEvent::MessageStarted {
                turn: 1,
                id: "msg_1".to_owned(),
            },
            AgentEvent::TextDelta {
                turn: 1,
                text: "hel".to_owned(),
            },
            AgentEvent::TextDelta {
                turn: 1,
                text: "lo".to_owned(),
            },
            AgentEvent::MessageFinished {
                turn: 1,
                id: "msg_1".to_owned(),
                stop_reason: StopReason::EndTurn,
            },
            AgentEvent::MessageAppended {
                message: AgentMessage::assistant(
                    [Content::text("hello")],
                    Vec::new(),
                    StopReason::EndTurn,
                ),
            },
            AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            },
            AgentEvent::RunFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            },
        ]
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
            .expect("run start should stream before delayed message end")
            .expect("run start event")
            .expect("run start should be ok"),
        AgentEvent::RunStarted { turn: 1 }
    );
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
async fn runtime_cancels_in_flight_model_stream_and_emits_cancelled_barriers() {
    let harness = DelayedHarness::new(vec![
        DelayedStep::Event(AiStreamEvent::MessageStart {
            id: "msg_cancel".to_owned(),
        }),
        DelayedStep::Event(AiStreamEvent::TextDelta {
            text: "partial".to_owned(),
        }),
        DelayedStep::Delay(Duration::from_secs(5)),
        DelayedStep::Event(AiStreamEvent::TextDelta {
            text: "late".to_owned(),
        }),
        DelayedStep::Event(AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        }),
    ]);
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();

    let mut stream = runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text("cancel stream"),
        cancel.clone(),
    );

    let mut events = Vec::new();
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("event before cancellation")
    {
        let event = event.expect("event should be ok");
        let should_cancel = matches!(event, AgentEvent::TextDelta { .. });
        events.push(event);
        if should_cancel {
            cancel.cancel();
            break;
        }
    }
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("cancelled barriers should arrive promptly")
    {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert!(events.contains(&AgentEvent::MessageFinished {
        turn: 1,
        id: "msg_cancel".to_owned(),
        stop_reason: StopReason::Cancelled,
    }));
    assert!(events.contains(&AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: StopReason::Cancelled,
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Cancelled,
        })
    );
    assert!(context.is_cancelled());
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::TextDelta { text, .. } if text == "late"
    )));
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
async fn runtime_passes_reasoning_effort_into_chat_request_options() {
    let harness = FakeHarness::from_events([AiStreamEvent::MessageEnd {
        stop_reason: neo_ai::StopReason::EndTurn,
        usage: None,
    }]);
    let mut config = AgentConfig::for_model(harness.model());
    config.reasoning_effort = Some(ReasoningEffort::Low);
    let runtime = AgentRuntime::new(config, harness.client());
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("think lightly"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert_eq!(
        harness.requests()[0].options.reasoning_effort,
        Some(ReasoningEffort::Low)
    );
}

#[tokio::test]
async fn runtime_streams_thinking_events_and_persists_thinking_content() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_thinking".to_owned(),
        },
        AiStreamEvent::ThinkingStart {
            id: "thinking_1".to_owned(),
        },
        AiStreamEvent::ThinkingDelta {
            text: "Checked ".to_owned(),
        },
        AiStreamEvent::ThinkingDelta {
            text: "the plan.".to_owned(),
        },
        AiStreamEvent::ThinkingEnd {
            signature: Some("sig-1".to_owned()),
            redacted: false,
        },
        AiStreamEvent::TextDelta {
            text: "final answer".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("think"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.contains(&AgentEvent::ThinkingStarted {
        turn: 1,
        id: "thinking_1".to_owned(),
    }));
    assert!(events.contains(&AgentEvent::ThinkingDelta {
        turn: 1,
        text: "Checked ".to_owned(),
    }));
    assert!(events.contains(&AgentEvent::ThinkingDelta {
        turn: 1,
        text: "the plan.".to_owned(),
    }));
    assert!(events.contains(&AgentEvent::ThinkingFinished {
        turn: 1,
        signature: Some("sig-1".to_owned()),
        redacted: false,
    }));
    assert_eq!(
        context.messages()[1],
        AgentMessage::assistant(
            [
                Content::thinking("Checked the plan.", Some("sig-1".to_owned()), false),
                Content::text("final answer"),
            ],
            Vec::new(),
            StopReason::EndTurn,
        )
    );
}

#[tokio::test]
async fn runtime_preserves_multiple_thinking_parts_and_text_order() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_multi_thinking".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "intro ".to_owned(),
        },
        AiStreamEvent::ThinkingStart {
            id: "thinking_1".to_owned(),
        },
        AiStreamEvent::ThinkingDelta {
            text: "first thought".to_owned(),
        },
        AiStreamEvent::ThinkingEnd {
            signature: Some("sig-1".to_owned()),
            redacted: false,
        },
        AiStreamEvent::ThinkingStart {
            id: "thinking_2".to_owned(),
        },
        AiStreamEvent::ThinkingDelta {
            text: "second thought".to_owned(),
        },
        AiStreamEvent::ThinkingEnd {
            signature: Some("sig-2".to_owned()),
            redacted: true,
        },
        AiStreamEvent::TextDelta {
            text: "outro".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("think twice"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert_eq!(
        context.messages()[1],
        AgentMessage::assistant(
            [
                Content::text("intro "),
                Content::thinking("first thought", Some("sig-1".to_owned()), false),
                Content::thinking("second thought", Some("sig-2".to_owned()), true),
                Content::text("outro"),
            ],
            Vec::new(),
            StopReason::EndTurn,
        )
    );
}

#[tokio::test]
async fn runtime_does_not_send_persisted_thinking_content_back_to_model() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_thinking".to_owned(),
            },
            AiStreamEvent::ThinkingStart {
                id: "thinking_1".to_owned(),
            },
            AiStreamEvent::ThinkingDelta {
                text: "local reasoning summary".to_owned(),
            },
            AiStreamEvent::ThinkingEnd {
                signature: Some("sig-1".to_owned()),
                redacted: false,
            },
            AiStreamEvent::TextDelta {
                text: "answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_followup".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "followup".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("think"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("first turn should succeed");
    runtime
        .run_turn(&mut context, AgentMessage::user_text("continue"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("second turn should succeed");

    let requests = harness.requests();
    assert_eq!(requests.len(), 2);
    let assistant_message = requests[1]
        .messages
        .iter()
        .find(|message| matches!(message, neo_ai::ChatMessage::Assistant { .. }))
        .expect("previous assistant message should be sent");
    assert_eq!(
        assistant_message,
        &neo_ai::ChatMessage::Assistant {
            content: vec![neo_ai::ContentPart::Text {
                text: "answer".to_owned(),
            }],
            tool_calls: Vec::new(),
        }
    );
}

#[tokio::test]
async fn runtime_compaction_estimate_ignores_unsent_thinking_content() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_after_thinking".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "kept".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_compaction(CompactionSettings::new(32, 1)),
        harness.client(),
    );
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::assistant(
        [
            Content::thinking("x".repeat(4_000), None, false),
            Content::text("short text"),
        ],
        Vec::new(),
        StopReason::EndTurn,
    ));

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("next"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::CompactionApplied { .. })),
        "thinking content is not sent back to the provider and should not trigger compaction"
    );
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
        vec![
            AgentEvent::RunStarted { turn: 1 },
            AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::MaxTurns,
            },
            AgentEvent::RunFinished {
                turn: 1,
                stop_reason: StopReason::MaxTurns,
            },
        ]
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
        vec![
            AgentEvent::RunStarted { turn: 1 },
            AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::Cancelled,
            },
            AgentEvent::RunFinished {
                turn: 1,
                stop_reason: StopReason::Cancelled,
            },
        ]
    );
}

#[tokio::test]
async fn runtime_external_cancellation_before_model_emits_cancelled_barriers() {
    let harness = FakeHarness::from_events([]);
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    cancel.cancel();

    let events = runtime
        .run_turn_with_cancel(
            &mut context,
            AgentMessage::user_text("already cancelled"),
            cancel,
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("cancel event");

    assert_eq!(
        events,
        vec![
            AgentEvent::RunStarted { turn: 1 },
            AgentEvent::MessageAppended {
                message: AgentMessage::user_text("already cancelled"),
            },
            AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::Cancelled,
            },
            AgentEvent::RunFinished {
                turn: 1,
                stop_reason: StopReason::Cancelled,
            },
        ]
    );
    assert!(context.is_cancelled());
    assert!(harness.requests().is_empty());
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
    }));
    assert!(events.contains(&AgentEvent::MessageFinished {
        turn: 2,
        id: "msg_2".to_owned(),
        stop_reason: StopReason::EndTurn,
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
async fn runtime_finishes_message_turn_and_run_with_error_stop_reason() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_error".to_owned(),
        },
        AiStreamEvent::Error {
            message: "provider failed".to_owned(),
        },
    ]);
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("fail"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("error event should remain in-band");

    assert!(events.contains(&AgentEvent::Error {
        turn: 1,
        message: "provider failed".to_owned(),
    }));
    assert!(events.contains(&AgentEvent::MessageFinished {
        turn: 1,
        id: "msg_error".to_owned(),
        stop_reason: StopReason::Error,
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
}

#[tokio::test]
async fn runtime_returns_tool_errors_to_model_for_retry_instead_of_aborting() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "fallible".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "bad": true }),
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
                text: "retry noted".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(FallibleTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()),
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
async fn runtime_cancels_in_flight_tool_execution_and_finishes_run() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "never".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({}),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(NeverTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    let mut stream = runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text("call never"),
        cancel.clone(),
    );
    let mut events = Vec::new();

    loop {
        let event = timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("tool start should arrive promptly")
            .expect("event before cancellation")
            .expect("event should be ok");
        let should_cancel = matches!(
            event,
            AgentEvent::ToolExecutionStarted { ref id, .. } if id == "tool_1"
        );
        events.push(event);
        if should_cancel {
            cancel.cancel();
            break;
        }
    }
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("cancelled tool run should finish promptly")
    {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "never".to_owned(),
        result: ToolResult::error("tool execution cancelled"),
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Cancelled,
        })
    );
    assert!(context.is_cancelled());
    assert_eq!(context.messages().len(), 2);
}

#[tokio::test]
async fn runtime_cancels_while_waiting_for_async_before_tool_hook() {
    let harness = echo_tool_harness("should not execute");
    let (hook_wait_sender, hook_wait_receiver) = oneshot::channel::<()>();
    let hook_wait_receiver = Arc::new(Mutex::new(Some(hook_wait_receiver)));
    let hook_started = Arc::new(Notify::new());
    let hook_started_for_hook = Arc::clone(&hook_started);
    let hook_wait_receiver_for_hook = Arc::clone(&hook_wait_receiver);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Sequential)
            .with_async_before_tool_call(move |_call, _cancel| {
                let started = hook_started_for_hook.clone();
                let receiver = hook_wait_receiver_for_hook.clone();
                async move {
                    started.notify_one();
                    let wait = receiver
                        .lock()
                        .expect("receiver lock poisoned")
                        .take()
                        .expect("hook wait receiver should be present");
                    let _ = wait.await;
                    None
                }
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    let events =
        cancel_after_async_tool_hook_starts(&runtime, &mut context, cancel, &hook_started).await;
    drop(hook_wait_sender);

    assert_async_hook_cancelled_cleanly(&events, &context);
    assert!(executed.lock().expect("executed lock poisoned").is_empty());
}

#[tokio::test]
async fn runtime_cancels_while_waiting_for_async_after_tool_hook() {
    let harness = echo_tool_harness("executed");
    let (hook_wait_sender, hook_wait_receiver) = oneshot::channel::<()>();
    let hook_wait_receiver = Arc::new(Mutex::new(Some(hook_wait_receiver)));
    let hook_started = Arc::new(Notify::new());
    let hook_started_for_hook = Arc::clone(&hook_started);
    let hook_wait_receiver_for_hook = Arc::clone(&hook_wait_receiver);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Sequential)
            .with_async_after_tool_call(move |_call, result, _cancel| {
                let started = hook_started_for_hook.clone();
                let receiver = hook_wait_receiver_for_hook.clone();
                async move {
                    started.notify_one();
                    let wait = receiver
                        .lock()
                        .expect("receiver lock poisoned")
                        .take()
                        .expect("hook wait receiver should be present");
                    let _ = wait.await;
                    result
                }
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    let events =
        cancel_after_async_tool_hook_starts(&runtime, &mut context, cancel, &hook_started).await;
    drop(hook_wait_sender);

    assert_async_hook_cancelled_cleanly(&events, &context);
    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["executed".to_owned()]
    );
}

async fn cancel_after_async_tool_hook_starts(
    runtime: &AgentRuntime,
    context: &mut AgentContext,
    cancel: CancellationToken,
    hook_started: &Notify,
) -> Vec<AgentEvent> {
    let mut stream = runtime.run_turn_with_cancel(
        context,
        AgentMessage::user_text("call echo"),
        cancel.clone(),
    );
    let mut events = Vec::new();

    loop {
        let event = timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("tool start should arrive promptly")
            .expect("event before cancellation")
            .expect("event should be ok");
        let should_cancel = matches!(
            event,
            AgentEvent::ToolExecutionStarted { ref id, .. } if id == "tool_1"
        );
        events.push(event);
        if should_cancel {
            break;
        }
    }
    timeout(Duration::from_millis(250), hook_started.notified())
        .await
        .expect("async hook should start promptly");
    cancel.cancel();
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("cancelled async hook should finish promptly")
    {
        events.push(event.expect("event should be ok"));
    }
    events
}

fn assert_async_hook_cancelled_cleanly(events: &[AgentEvent], context: &AgentContext) {
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::error("tool execution cancelled"),
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Cancelled,
        })
    );
    assert!(context.is_cancelled());
    assert_eq!(context.messages().len(), 2);
}

#[tokio::test]
async fn runtime_parallel_cancellation_finishes_all_started_tool_wrappers() {
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
                arguments: json!({ "text": "fast" }),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "never".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                arguments: json!({}),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool);
    tools.register(NeverTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Parallel),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    let mut stream = runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text("call parallel tools"),
        cancel.clone(),
    );
    let mut events = Vec::new();

    loop {
        let event = timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("tool starts should arrive promptly")
            .expect("event before cancellation")
            .expect("event should be ok");
        let should_cancel = matches!(
            event,
            AgentEvent::ToolExecutionStarted { ref id, .. } if id == "tool_2"
        );
        events.push(event);
        if should_cancel {
            cancel.cancel();
            break;
        }
    }
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("cancelled parallel tool run should finish promptly")
    {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::ok("fast"),
    }));
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_2".to_owned(),
        name: "never".to_owned(),
        result: ToolResult::error("tool execution cancelled"),
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Cancelled,
        })
    );
    assert!(context.is_cancelled());
    assert_eq!(context.messages().len(), 2);
}

#[tokio::test]
async fn runtime_parallel_cancellation_does_not_start_later_tool_calls() {
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
                arguments: json!({ "text": "first" }),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "never".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                arguments: json!({}),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let cancel = CancellationToken::new();
    let cancel_from_hook = cancel.clone();
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool);
    tools.register(NeverTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Parallel)
            .with_before_tool_call(move |call| {
                if call.id == "tool_1" {
                    cancel_from_hook.cancel();
                    Some(ToolResult::ok("first"))
                } else {
                    None
                }
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn_with_cancel(
            &mut context,
            AgentMessage::user_text("call parallel tools"),
            cancel,
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::ok("first"),
    }));
    assert!(!events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ToolExecutionStarted { id, .. } if id == "tool_2"
        )
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Cancelled,
        })
    );
    assert!(context.is_cancelled());
    assert_eq!(context.messages().len(), 2);
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
        Some(AgentEvent::RunFinished {
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

struct AsyncEchoRuntime {
    runtime: AgentRuntime,
    executed: Arc<Mutex<Vec<String>>>,
    decision_sender: oneshot::Sender<PermissionDecision>,
    observed_requests: Arc<Mutex<Vec<ApprovalRequest>>>,
}

fn echo_tool_harness(text: &str) -> FakeHarness {
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
                arguments: json!({ "text": text }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ])
}

fn async_echo_runtime(harness: &FakeHarness) -> AsyncEchoRuntime {
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let (decision_sender, decision_receiver) = oneshot::channel();
    let decision_receiver = Arc::new(Mutex::new(Some(decision_receiver)));
    let observed_requests = Arc::new(Mutex::new(Vec::new()));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_permission_policy(PermissionPolicy {
                file_read: PermissionDecision::Allow,
                file_write: PermissionDecision::Deny,
                shell: PermissionDecision::Deny,
                tool: PermissionDecision::Ask,
            })
            .with_async_approval_handler({
                let decision_receiver = Arc::clone(&decision_receiver);
                let observed_requests = Arc::clone(&observed_requests);
                move |request| {
                    observed_requests
                        .lock()
                        .expect("observed requests lock poisoned")
                        .push(request.clone());
                    let decision_receiver = take_decision_receiver(&decision_receiver);
                    async move {
                        decision_receiver
                            .await
                            .expect("approval decision should be sent")
                    }
                }
            }),
        harness.client(),
        tools,
    );

    AsyncEchoRuntime {
        runtime,
        executed,
        decision_sender,
        observed_requests,
    }
}

fn take_decision_receiver(
    receiver: &Arc<Mutex<Option<oneshot::Receiver<PermissionDecision>>>>,
) -> oneshot::Receiver<PermissionDecision> {
    receiver
        .lock()
        .expect("decision receiver lock poisoned")
        .take()
        .expect("single approval decision receiver")
}

async fn collect_until_approval<S>(stream: &mut S, events: &mut Vec<AgentEvent>)
where
    S: futures::Stream<Item = Result<AgentEvent, AgentRuntimeError>> + Unpin,
{
    loop {
        let event = timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("event before approval request")
            .expect("stream should not end before approval request")
            .expect("event should be ok");
        let approval_requested = matches!(event, AgentEvent::ApprovalRequested { .. });
        events.push(event);
        if approval_requested {
            break;
        }
    }
}

async fn assert_waits_for_approval_decision<S>(stream: &mut S, action: &str)
where
    S: futures::Stream<Item = Result<AgentEvent, AgentRuntimeError>> + Unpin,
{
    assert!(
        timeout(Duration::from_millis(50), stream.next())
            .await
            .is_err(),
        "runtime should wait for the async approval decision before {action}"
    );
}

fn final_done_turn() -> Vec<AiStreamEvent> {
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
    ]
}

#[tokio::test]
async fn runtime_executes_ask_permission_tool_after_async_approval_wait_allows_it() {
    let harness = echo_tool_harness("async approved");
    let AsyncEchoRuntime {
        runtime,
        executed,
        decision_sender,
        observed_requests,
    } = async_echo_runtime(&harness);
    let mut context = AgentContext::new();

    let mut stream = runtime.run_turn(&mut context, AgentMessage::user_text("call tool"));
    let mut events = Vec::new();
    collect_until_approval(&mut stream, &mut events).await;

    assert_eq!(
        *observed_requests
            .lock()
            .expect("observed requests lock poisoned"),
        vec![ApprovalRequest {
            turn: 1,
            id: "tool_1".to_owned(),
            operation: PermissionOperation::Tool,
            subject: "echo".to_owned(),
            arguments: json!({ "text": "async approved" }),
        }]
    );
    assert!(events.contains(&AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool_1".to_owned(),
        operation: PermissionOperation::Tool,
        subject: "echo".to_owned(),
        arguments: json!({ "text": "async approved" }),
    }));
    assert!(executed.lock().expect("executed lock poisoned").is_empty());
    assert_waits_for_approval_decision(&mut stream, "executing").await;

    decision_sender
        .send(PermissionDecision::Allow)
        .expect("send allow decision");
    while let Some(event) = stream.next().await {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["async approved".to_owned()]
    );
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::ok("async approved"),
    }));
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result(
            "tool_1",
            "echo",
            vec![Content::text("async approved")],
            false
        )
    );
}

#[tokio::test]
async fn runtime_skips_ask_permission_tool_after_async_approval_wait_denies_it() {
    let harness = echo_tool_harness("async denied");
    let AsyncEchoRuntime {
        runtime,
        executed,
        decision_sender,
        ..
    } = async_echo_runtime(&harness);
    let mut context = AgentContext::new();

    let mut stream = runtime.run_turn(&mut context, AgentMessage::user_text("call tool"));
    let mut events = Vec::new();
    collect_until_approval(&mut stream, &mut events).await;

    assert!(events.contains(&AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool_1".to_owned(),
        operation: PermissionOperation::Tool,
        subject: "echo".to_owned(),
        arguments: json!({ "text": "async denied" }),
    }));
    assert!(executed.lock().expect("executed lock poisoned").is_empty());
    assert_waits_for_approval_decision(&mut stream, "denying").await;

    decision_sender
        .send(PermissionDecision::Deny)
        .expect("send deny decision");
    while let Some(event) = stream.next().await {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert!(executed.lock().expect("executed lock poisoned").is_empty());
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::error("approval denied for tool: echo"),
    }));
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result(
            "tool_1",
            "echo",
            vec![Content::text("approval denied for tool: echo")],
            true
        )
    );
}

#[tokio::test]
async fn runtime_parallel_mode_runs_allowed_tool_while_async_approval_is_pending() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = parallel_write_and_echo_harness();
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::with_builtin_tools();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let (decision_sender, decision_receiver) = oneshot::channel();
    let decision_receiver = Arc::new(Mutex::new(Some(decision_receiver)));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Parallel)
            .with_tool_permission_policy(PermissionPolicy {
                file_read: PermissionDecision::Allow,
                file_write: PermissionDecision::Ask,
                shell: PermissionDecision::Deny,
                tool: PermissionDecision::Allow,
            })
            .with_workspace_root(workspace.path())
            .expect("workspace config")
            .with_async_approval_handler({
                let decision_receiver = Arc::clone(&decision_receiver);
                move |request| {
                    assert_eq!(request.operation, PermissionOperation::FileWrite);
                    let decision_receiver = take_decision_receiver(&decision_receiver);
                    async move {
                        decision_receiver
                            .await
                            .expect("approval decision should be sent")
                    }
                }
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let mut stream = runtime.run_turn(&mut context, AgentMessage::user_text("call tools"));
    let mut events = Vec::new();
    collect_until_approval(&mut stream, &mut events).await;

    let allowed_finish = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("allowed tool should finish while approval is pending")
        .expect("stream should continue")
        .expect("event should be ok");
    assert_eq!(
        allowed_finish,
        AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "tool_2".to_owned(),
            name: "echo".to_owned(),
            result: ToolResult::ok("already allowed"),
        }
    );
    events.push(allowed_finish);
    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["already allowed".to_owned()]
    );
    assert!(
        !workspace.path().join("approved.txt").exists(),
        "approval-gated write should still be pending"
    );

    decision_sender
        .send(PermissionDecision::Allow)
        .expect("send allow decision");
    while let Some(event) = stream.next().await {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert_eq!(
        std::fs::read_to_string(workspace.path().join("approved.txt")).expect("written file"),
        "ok"
    );
    assert!(matches!(
        &context.messages()[2],
        AgentMessage::ToolResult {
            tool_call_id,
            tool_name,
            content,
            is_error,
        } if tool_call_id == "tool_1"
            && tool_name == "write"
            && content
                .iter()
                .any(|part| matches!(part, Content::Text { text } if text.contains("approved.txt")))
            && !is_error
    ));
    assert_eq!(
        context.messages()[3],
        AgentMessage::tool_result(
            "tool_2",
            "echo",
            vec![Content::text("already allowed")],
            false
        )
    );
}

fn parallel_write_and_echo_harness() -> FakeHarness {
    FakeHarness::from_turns([
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
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                arguments: json!({ "text": "already allowed" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ])
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

struct NeverTool;

impl Tool for NeverTool {
    fn name(&self) -> &'static str {
        "never"
    }

    fn description(&self) -> &'static str {
        "Never completes."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(std::future::pending())
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
