use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, AgentRuntimeError,
    AgentToolCall, ApprovalRequest, AskUserTool, CompactionSettings, Content,
    PermissionApprovalDecision, PermissionMode, PermissionOperation, QueueMode, ShellCommandOrigin,
    ShellCommandOutcome, StopReason, TodoEventData, Tool, ToolContext, ToolError,
    ToolExecutionMode, ToolFuture, ToolRegistry, ToolResult,
    harness::{FakeHarness, fake_model},
    session::{JsonlSessionWriter, workspace_sessions_dir},
    skills::SkillStore,
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
    sync::mpsc,
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
            AgentEvent::ContextWindowUpdated {
                turn: 1,
                used_tokens: 3,
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
            AgentEvent::ContextWindowUpdated {
                turn: 1,
                used_tokens: 5,
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
async fn runtime_injects_workspace_context_into_model_request() {
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "ok".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model())
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
        harness.client(),
    );
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("where am I?"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let request = harness.requests().pop().expect("model request");
    assert!(matches!(
        request.messages.first(),
        Some(neo_ai::ChatMessage::System { content })
            if content.iter().any(|part| matches!(
                part,
                neo_ai::ContentPart::Text { text }
                    if text.contains("<environment_context>")
                        && text.contains("<cwd>")
                        && text.contains(&workspace_root.display().to_string())
                        && text.contains("Do not prefix shell commands with `cd")
            ))
    ));
}

#[tokio::test]
async fn runtime_context_window_estimate_includes_effective_request_messages() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace_root = temp.path().join("workspace");
    std::fs::create_dir(&workspace_root).expect("workspace dir");
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model())
            .with_system_prompt("system prompt that must count toward context")
            .with_workspace_root(workspace_root)
            .expect("workspace root"),
        harness.client(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("short"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ContextWindowUpdated {
                turn: 1,
                used_tokens
            } if *used_tokens > 20
        )),
        "context estimate should include system/workspace request messages, not only the user buffer"
    );
}

#[tokio::test]
async fn runtime_emits_provider_token_usage() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "hello".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: Some(neo_ai::TokenUsage {
                input_tokens: 123,
                output_tokens: 45,
            }),
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

    assert!(events.contains(&AgentEvent::TokenUsage {
        turn: 1,
        usage: neo_agent_core::AgentTokenUsage {
            input_tokens: 123,
            output_tokens: 45,
        },
    }));
}

#[tokio::test]
async fn goal_mode_authoring_injects_exit_goal_mode_guidance() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_goal_mode_authoring(true),
        harness.client(),
    );
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("draft goal"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let request = harness.requests().pop().expect("model request");
    assert!(request.messages.iter().any(|message| matches!(
        message,
        neo_ai::ChatMessage::System { content }
            if content.iter().any(|part| matches!(
                part,
                neo_ai::ContentPart::Text { text }
                    if text.contains("Goal mode is active")
                        && text.contains("ExitGoalMode")
                        && text.contains("Do not start a durable goal directly")
            ))
    )));
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
            .expect("context tokens should stream before delayed message end")
            .expect("context tokens event")
            .expect("context tokens should be ok"),
        AgentEvent::ContextWindowUpdated {
            turn: 1,
            used_tokens: 2,
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
            name: "Read".to_owned(),
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
            id: "tool_1".to_owned(),
            name: "Read".to_owned(),
            arguments: json!({ "path": "README.md" }),
        },
    }));
    assert_eq!(
        context.messages()[1],
        AgentMessage::assistant(
            Vec::new(),
            vec![AgentToolCall {
                id: "tool_1".to_owned(),
                name: "Read".to_owned(),
                arguments: json!({ "path": "README.md" }),
            }],
            StopReason::ToolUse,
        )
    );
    assert_eq!(harness.requests()[0].tools, vec![tool]);
}

async fn assert_runtime_rejects_unsupported_capability(
    config: AgentConfig,
    harness: &FakeHarness,
    message: AgentMessage,
    expected_substring: &str,
    expectation: &str,
) {
    let runtime = AgentRuntime::new(config, harness.client());
    let mut context = AgentContext::new();
    let error = runtime
        .run_turn(&mut context, message)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect_err(expectation);

    assert!(matches!(
        error,
        AgentRuntimeError::Model(AiError::Configuration { message: _ })
    ));
    assert!(
        error.to_string().contains(expected_substring),
        "expected {expected_substring:?}, got {error}"
    );
    assert!(
        harness.requests().is_empty(),
        "request should not reach provider"
    );
}

#[tokio::test]
async fn runtime_rejects_tools_when_model_lacks_tools_before_request() {
    let harness = FakeHarness::from_events([AiStreamEvent::MessageEnd {
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
async fn runtime_rejects_image_content_when_model_lacks_images_before_request() {
    let harness = FakeHarness::from_events([AiStreamEvent::MessageEnd {
        stop_reason: neo_ai::StopReason::EndTurn,
        usage: None,
    }]);
    let config = AgentConfig::for_model(harness.model());

    assert_runtime_rejects_unsupported_capability(
        config,
        &harness,
        AgentMessage::User {
            content: vec![Content::Image {
                mime_type: "image/png".to_owned(),
                data: neo_agent_core::ImageRef::Url("https://example.test/cat.png".to_owned()),
            }],
        },
        "does not support image input",
        "unsupported images should fail before provider request",
    )
    .await;
}

#[tokio::test]
async fn runtime_rejects_reasoning_effort_when_model_lacks_reasoning_before_request() {
    let harness = FakeHarness::from_events([AiStreamEvent::MessageEnd {
        stop_reason: neo_ai::StopReason::EndTurn,
        usage: None,
    }]);
    let mut config = AgentConfig::for_model(harness.model());
    config.reasoning_effort = Some(ReasoningEffort::Low);

    assert_runtime_rejects_unsupported_capability(
        config,
        &harness,
        AgentMessage::user_text("think lightly"),
        "does not support reasoning",
        "unsupported reasoning should fail before provider request",
    )
    .await;
}

#[tokio::test]
async fn runtime_passes_reasoning_effort_into_chat_request_options() {
    let harness = FakeHarness::from_events([AiStreamEvent::MessageEnd {
        stop_reason: neo_ai::StopReason::EndTurn,
        usage: None,
    }]);
    let mut config = AgentConfig::for_model(model_with_capabilities(ModelCapabilities {
        reasoning: true,
        ..ModelCapabilities::tool_chat()
    }));
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
async fn runtime_sends_persisted_thinking_content_back_to_model() {
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
            content: vec![
                neo_ai::ContentPart::Thinking {
                    text: "local reasoning summary".to_owned(),
                    signature: Some("sig-1".to_owned()),
                    redacted: false,
                },
                neo_ai::ContentPart::Text {
                    text: "answer".to_owned(),
                },
            ],
            tool_calls: Vec::new(),
        }
    );
}

#[tokio::test]
async fn runtime_can_disable_persisted_thinking_replay() {
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
    let mut config = AgentConfig::for_model(harness.model());
    config.replay_reasoning = false;
    let runtime = AgentRuntime::new(config, harness.client());
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
    assert!(!requests[1].options.replay_reasoning);
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
async fn runtime_can_compact_again_after_context_grows_past_threshold() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "first answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        // Compaction summary call for the first compaction
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_compact_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "## Current Focus\nFirst compaction.".to_owned(),
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
                text: "second answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        // Compaction summary call for the second compaction
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_compact_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "## Current Focus\nSecond compaction.".to_owned(),
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
                text: "third answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_compaction(CompactionSettings::new(4, 1)),
        harness.client(),
    );
    let mut context = AgentContext::new();
    let mut compactions = Vec::new();

    for prompt in [
        "first long prompt that seeds compaction",
        "second long prompt that triggers compaction",
        "third long prompt that should trigger compaction again",
    ] {
        let events = runtime
            .run_turn(&mut context, AgentMessage::user_text(prompt))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("turn should succeed");
        compactions.extend(events.into_iter().filter_map(|event| match event {
            AgentEvent::CompactionApplied { summary } => Some(summary),
            _ => None,
        }));
    }

    assert_eq!(
        compactions.len(),
        2,
        "context should compact again after later turns grow past the threshold. Messages: {:?}",
        context.messages().len()
    );
    // After the second compaction, the context should contain:
    // 1. The injected compaction summary system message
    // 2. The third user prompt
    // 3. The third assistant response
    assert_eq!(context.messages().len(), 3);
    assert!(matches!(
        context.messages().first(),
        Some(AgentMessage::System { .. })
    ));
    assert_eq!(context.compaction_summary(), compactions.last());
}

#[tokio::test]
async fn runtime_emits_compaction_lifecycle_events_before_applying_summary() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "first answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        // Compaction summary call (no tools, returns structured summary text)
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_compact".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "## Current Focus\nWorking on compaction test.".to_owned(),
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
                text: "second answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_compaction(CompactionSettings::new(4, 1)),
        harness.client(),
    );
    let mut context = AgentContext::new();

    runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("first long prompt that seeds compaction"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("first turn should succeed");

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("second long prompt that triggers compaction"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("second turn should succeed");

    let lifecycle = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::CompactionStarted {
                reason,
                tokens_before,
                message_count,
            } => Some(format!("start:{reason:?}:{tokens_before}:{message_count}")),
            AgentEvent::CompactionProgress { phase, percent } => {
                Some(format!("progress:{phase:?}:{percent}"))
            }
            AgentEvent::CompactionApplied { summary } => Some(format!(
                "applied:{}:{}",
                summary.first_kept_message_index, summary.tokens_before
            )),
            AgentEvent::ContextWindowUpdated { used_tokens, .. } => {
                Some(format!("context:{used_tokens}"))
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    // Verify the lifecycle starts at 0%, goes through the visible phases, and
    // finishes smoothly at 100% instead of jumping from ~80% to done.
    assert_eq!(lifecycle.first(), Some(&"start:Threshold:24:3".to_owned()));
    assert!(lifecycle.contains(&"progress:Estimating:0".to_owned()));
    assert!(lifecycle.contains(&"progress:SelectingBoundary:15".to_owned()));
    assert!(lifecycle.contains(&"progress:Summarizing:15".to_owned()));
    assert!(
        lifecycle
            .iter()
            .any(|e| e.starts_with("progress:Summarizing:") && e != "progress:Summarizing:15"),
        "Summarizing should make progress beyond its starting percent: {lifecycle:?}"
    );
    assert_eq!(
        lifecycle
            .iter()
            .filter(|e| e.starts_with("progress:"))
            .last(),
        Some(&"progress:Applying:100".to_owned()),
        "last progress should reach 100%: {lifecycle:?}"
    );
    assert!(lifecycle.contains(&"applied:2:24".to_owned()));

    // Percentages must be non-decreasing across CompactionProgress events.
    let percents: Vec<u8> = lifecycle
        .iter()
        .filter_map(|e| {
            e.strip_prefix("progress:").and_then(|rest| {
                let parts: Vec<&str> = rest.split(':').collect();
                parts.last().and_then(|p| p.parse().ok())
            })
        })
        .collect();
    assert!(
        percents.windows(2).all(|w| w[0] <= w[1]),
        "progress percents should be monotonic: {percents:?}"
    );
}

#[tokio::test]
async fn runtime_compaction_keeps_valid_tool_result_boundaries() {
    let harness = FakeHarness::from_turns([
        // Compaction summary call
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_compact".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "## Current Focus\nInspecting files.".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        // Actual turn response
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_after_compaction".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "after compaction".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_compaction(CompactionSettings::new(1, 3)),
        harness.client(),
    );
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("inspect"));
    context.append_message(AgentMessage::assistant(
        [],
        [
            AgentToolCall {
                id: "tool_1".to_owned(),
                name: "Read".to_owned(),
                arguments: json!({ "path": "a.rs" }),
            },
            AgentToolCall {
                id: "tool_2".to_owned(),
                name: "List".to_owned(),
                arguments: json!({ "path": "src" }),
            },
        ],
        StopReason::ToolUse,
    ));
    context.append_message(AgentMessage::tool_result(
        "tool_1",
        "Read",
        [Content::text("large content")],
        false,
    ));
    context.append_message(AgentMessage::tool_result(
        "tool_2",
        "List",
        [Content::text("file list")],
        false,
    ));

    runtime
        .run_turn(&mut context, AgentMessage::user_text("continue"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let request = harness.requests().pop().expect("model request");
    assert!(
        !matches!(
            request.messages.first(),
            Some(neo_ai::ChatMessage::ToolResult { .. })
        ),
        "compaction must not keep orphaned tool results at the start of replay"
    );
    // The first message is now either the compaction summary system message or
    // the user prompt — never an orphaned tool result.
    assert!(matches!(
        request.messages.first(),
        Some(neo_ai::ChatMessage::System { .. }) | Some(neo_ai::ChatMessage::User { .. })
    ));
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
            AgentEvent::ContextWindowUpdated {
                turn: 1,
                used_tokens: 5,
            },
            AgentEvent::RunFinished {
                turn: 1,
                stop_reason: StopReason::Cancelled,
            },
        ]
    );
    assert!(harness.requests().is_empty());
}

#[tokio::test]
async fn runtime_resumed_cancelled_turn_accepts_followup_prompt() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_after_resume".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "resumed".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());
    let mut context = AgentContext::from_replay(
        [
            AgentEvent::RunStarted { turn: 1 },
            AgentEvent::MessageAppended {
                message: AgentMessage::user_text("cancel this turn"),
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
        .iter(),
    );

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("continue after resume"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("follow-up turn should run after replayed cancellation");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::TextDelta { text, .. } if text == "resumed"
    )));
    assert!(events.contains(&AgentEvent::TurnFinished {
        turn: 2,
        stop_reason: StopReason::EndTurn,
    }));
    assert_eq!(harness.requests().len(), 1);
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
async fn runtime_emits_todo_update_only_for_writes() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_write".to_owned(),
                name: "TodoList".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_write".to_owned(),
                arguments: json!({
                    "todos": [{ "title": "Read code", "status": "in_progress" }]
                }),
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
            AiStreamEvent::ToolCallStart {
                id: "tool_read".to_owned(),
                name: "TodoList".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_read".to_owned(),
                arguments: json!({}),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_3".to_owned(),
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
                    if tool_call_id == "tool_read"
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
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_clear".to_owned(),
                name: "TodoList".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_clear".to_owned(),
                arguments: json!({ "todos": [] }),
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
                text: "cleared".to_owned(),
            },
            AiStreamEvent::MessageEnd {
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
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Yolo),
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
            .with_permission_mode(PermissionMode::Yolo)
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
            .with_permission_mode(PermissionMode::Yolo)
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
            .with_tool_execution_mode(ToolExecutionMode::Parallel)
            .with_permission_mode(PermissionMode::Yolo),
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
            .with_permission_mode(PermissionMode::Yolo)
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
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Ask),
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
        session_scope: None,
        prefix_rule: None,
    }));
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::error("approval required for tool: echo"),
    }));
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

fn assert_tool_was_executed(executed: &[String], should_execute: bool) {
    let was_executed = !executed.is_empty();
    assert_eq!(
        was_executed, should_execute,
        "expected should_execute={should_execute}, executed list: {executed:?}"
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
            .with_permission_mode(PermissionMode::Ask)
            .with_approval_handler(|request| {
                assert_eq!(request.operation, PermissionOperation::Tool);
                assert_eq!(request.subject, "echo");
                PermissionApprovalDecision::AllowOnce
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
        session_scope: None,
        prefix_rule: None,
    }));
    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["approved".to_owned()]
    );
    assert_tool_was_executed(&executed.lock().expect("lock poisoned"), true);
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
#[allow(clippy::too_many_lines)]
async fn live_permission_switch_to_auto_skips_approval_for_later_tool_calls() {
    // One turn with two model ToolUse round-trips. The first echo requires
    // approval in Ask mode. The approval handler switches the shared live mode
    // to Auto while granting this first call; the second echo must therefore
    // run WITHOUT a second ApprovalRequested event.
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
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                arguments: json!({ "text": "second" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_3".to_owned(),
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
    let live_mode = Arc::new(std::sync::RwLock::new(PermissionMode::Ask));
    let live_for_handler = Arc::clone(&live_mode);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_live_permission_mode(Arc::clone(&live_mode))
            .with_approval_handler(move |_request| {
                // Flip the live mode to Auto before returning so the second tool
                // call is prepared under Auto and must not request approval again.
                if let Ok(mut mode) = live_for_handler.write() {
                    *mode = PermissionMode::Auto;
                }
                PermissionApprovalDecision::AllowOnce
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
        .expect("live-switch tool loop should succeed");

    let first_approval = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ApprovalRequested { id, .. } if id == "tool_1"
        )
    });
    let second_approval = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ApprovalRequested { id, .. } if id == "tool_2"
        )
    });
    assert!(
        first_approval,
        "first call should request approval under Ask"
    );
    assert!(
        !second_approval,
        "second call should NOT request approval after live switch to Auto"
    );
    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["first".to_owned(), "second".to_owned()]
    );
    assert_eq!(*live_mode.read().unwrap(), PermissionMode::Auto);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn live_permission_switch_to_ask_requests_approval_for_later_tool_calls() {
    // Inverse of the above: start Auto (no approval), flip live mode to Ask
    // mid-turn via the async after-tool hook, and the second generic tool call
    // must request approval.
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
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                arguments: json!({ "text": "second" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_3".to_owned(),
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
    let live_mode = Arc::new(std::sync::RwLock::new(PermissionMode::Auto));
    let live_for_hook = Arc::clone(&live_mode);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Auto)
            .with_live_permission_mode(Arc::clone(&live_mode))
            .with_async_after_tool_call(move |_call, result, _cancel| {
                let live = Arc::clone(&live_for_hook);
                async move {
                    if let Ok(mut mode) = live.write() {
                        *mode = PermissionMode::Ask;
                    }
                    result
                }
            })
            .with_approval_handler(|_request| PermissionApprovalDecision::AllowOnce),
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
        .expect("live-switch tool loop should succeed");

    let first_approval = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ApprovalRequested { id, .. } if id == "tool_1"
        )
    });
    let second_approval = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ApprovalRequested { id, .. } if id == "tool_2"
        )
    });
    assert!(
        !first_approval,
        "first call should NOT request approval under Auto"
    );
    assert!(
        second_approval,
        "second call should request approval after live switch to Ask"
    );
    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["first".to_owned(), "second".to_owned()]
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
            .with_permission_mode(PermissionMode::Ask)
            .with_approval_handler(|request| {
                assert_eq!(request.operation, PermissionOperation::Tool);
                assert_eq!(request.subject, "echo");
                PermissionApprovalDecision::Reject
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
        session_scope: None,
        prefix_rule: None,
    }));
    assert!(executed.lock().expect("executed lock poisoned").is_empty());
    assert_tool_was_executed(&executed.lock().expect("lock poisoned"), false);
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
    decision_sender: oneshot::Sender<PermissionApprovalDecision>,
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
            .with_permission_mode(PermissionMode::Ask)
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
    receiver: &Arc<Mutex<Option<oneshot::Receiver<PermissionApprovalDecision>>>>,
) -> oneshot::Receiver<PermissionApprovalDecision> {
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

/// Set the permission mode on a config while keeping the static snapshot and
/// the live runtime state in sync. Direct field mutation of
/// `config.permission_mode` no longer affects permission evaluation because
/// `permission_preparation_for_mode` reads the shared live handle.
fn set_config_permission_mode(config: &mut AgentConfig, mode: PermissionMode) {
    config.permission_mode = mode;
    if let Ok(mut live) = config.live_permission_mode.write() {
        *live = mode;
    }
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
            session_scope: None,
            prefix_rule: None,
        }]
    );
    assert!(events.contains(&AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool_1".to_owned(),
        operation: PermissionOperation::Tool,
        subject: "echo".to_owned(),
        arguments: json!({ "text": "async approved" }),
        session_scope: None,
        prefix_rule: None,
    }));
    assert!(executed.lock().expect("executed lock poisoned").is_empty());
    assert_waits_for_approval_decision(&mut stream, "executing").await;

    decision_sender
        .send(PermissionApprovalDecision::AllowOnce)
        .expect("send allow decision");
    while let Some(event) = stream.next().await {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["async approved".to_owned()]
    );
    assert_tool_was_executed(&executed.lock().expect("lock poisoned"), true);
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
        session_scope: None,
        prefix_rule: None,
    }));
    assert!(executed.lock().expect("executed lock poisoned").is_empty());
    assert_tool_was_executed(&executed.lock().expect("lock poisoned"), false);
    assert_waits_for_approval_decision(&mut stream, "denying").await;

    decision_sender
        .send(PermissionApprovalDecision::Reject)
        .expect("send deny decision");
    while let Some(event) = stream.next().await {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert!(executed.lock().expect("executed lock poisoned").is_empty());
    assert_tool_was_executed(&executed.lock().expect("lock poisoned"), false);
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
async fn runtime_cancels_while_waiting_for_async_approval_decision() {
    let harness = echo_tool_harness("async cancelled");
    let AsyncEchoRuntime {
        runtime,
        executed,
        decision_sender: _decision_sender,
        ..
    } = async_echo_runtime(&harness);
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();

    let mut stream = runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text("call approval-gated tool"),
        cancel.clone(),
    );
    let mut events = Vec::new();
    collect_until_approval(&mut stream, &mut events).await;

    assert!(events.contains(&AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool_1".to_owned(),
        operation: PermissionOperation::Tool,
        subject: "echo".to_owned(),
        arguments: json!({ "text": "async cancelled" }),
        session_scope: None,
        prefix_rule: None,
    }));
    assert!(executed.lock().expect("executed lock poisoned").is_empty());

    cancel.cancel();
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("cancelled approval wait should finish promptly")
    {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

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
    assert_eq!(context.messages().len(), 2);
    assert!(executed.lock().expect("executed lock poisoned").is_empty());
}

#[tokio::test]
async fn parallel_mode_serializes_ask_approval_batches() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = parallel_write_and_glob_harness();
    let (decision_sender, decision_receiver) = oneshot::channel();
    let decision_receiver = Arc::new(Mutex::new(Some(decision_receiver)));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Parallel)
            .with_permission_mode(PermissionMode::Ask)
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
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let mut stream = runtime.run_turn(&mut context, AgentMessage::user_text("call tools"));
    let mut events = Vec::new();
    collect_until_approval(&mut stream, &mut events).await;

    assert!(
        timeout(Duration::from_millis(250), stream.next())
            .await
            .is_err(),
        "later tools in an approval-gated batch must wait for the active approval"
    );
    assert!(
        !workspace.path().join("approved.txt").exists(),
        "approval-gated write should still be pending"
    );

    decision_sender
        .send(PermissionApprovalDecision::AllowOnce)
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
            && tool_name == "Write"
            && content
                .iter()
                .any(|part| matches!(part, Content::Text { text } if text.contains("approved.txt")))
            && !is_error
    ));
    assert!(matches!(
        &context.messages()[3],
        AgentMessage::ToolResult {
            tool_call_id,
            tool_name,
            content,
            is_error,
        } if tool_call_id == "tool_2"
            && tool_name == "Glob"
            && content
                .iter()
                .any(|part| matches!(part, Content::Text { text } if text.contains("Found")))
            && !is_error
    ));
}

fn parallel_write_and_glob_harness() -> FakeHarness {
    FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Write".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "path": "approved.txt", "content": "ok" }),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "Glob".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                arguments: json!({ "pattern": "*" }),
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
                name: "Write".to_owned(),
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
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace config")
            .with_approval_handler(|request| {
                assert_eq!(request.operation, PermissionOperation::FileWrite);
                assert_eq!(request.subject, "approved.txt");
                PermissionApprovalDecision::AllowOnce
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

    // Write now derives a reusable FileWrite scope (Layer 1). Use matches!
    // because the workspace path is dynamic (tempdir).
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ApprovalRequested {
            id,
            operation: PermissionOperation::FileWrite,
            subject,
            arguments,
            session_scope,
            ..
        } if id == "tool_1"
            && subject == "approved.txt"
            && arguments == &json!({ "path": "approved.txt", "content": "ok" })
            && session_scope.as_ref().is_some_and(|scope|
                scope.label == "Approve writes to this file for this session"
                && scope.keys.len() == 1
            )
    )));
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
                name: "Bash".to_owned(),
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
        cwd: workspace_root,
        origin: ShellCommandOrigin::ModelBashTool,
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

#[tokio::test]
async fn runtime_emits_shell_finished_when_model_bash_times_out() {
    let harness = FakeHarness::from_turns([vec![
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::ToolCallStart {
            id: "tool_1".to_owned(),
            name: "Bash".to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: "tool_1".to_owned(),
            arguments: json!({
                "command": "printf before-timeout; sleep 5",
                "timeout": 0
            }),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::ToolUse,
            usage: None,
        },
    ]]);
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

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run shell timeout"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should finish with tool error");

    assert!(events.contains(&AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "tool_1".to_owned(),
        command: "printf before-timeout; sleep 5".to_owned(),
        cwd: workspace_root,
        origin: ShellCommandOrigin::ModelBashTool,
    }));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ShellCommandFinished {
            turn: 1,
            id,
            origin: ShellCommandOrigin::ModelBashTool,
            outcome: ShellCommandOutcome::TimedOut,
            ..
        } if id == "tool_1"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            id,
            result,
            ..
        } if id == "tool_1"
            && result.is_error
            && result
                .details
                .as_ref()
                .and_then(|details| details["outcome"].as_str())
                == Some("timed_out")
    )));
}

#[tokio::test]
async fn runtime_marks_model_background_bash_as_backgrounded_shell_event() {
    let harness = FakeHarness::from_turns([vec![
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::ToolCallStart {
            id: "tool_1".to_owned(),
            name: "Bash".to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: "tool_1".to_owned(),
            arguments: json!({
                "command": "sleep 5",
                "run_in_background": true,
                "description": "sleep in background"
            }),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::ToolUse,
            usage: None,
        },
    ]]);
    let workspace = tempfile::tempdir().expect("tempdir");
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("run background shell"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should finish with background task");

    let task_id = events.iter().find_map(|event| match event {
        AgentEvent::ToolExecutionFinished { result, .. } => result
            .details
            .as_ref()
            .and_then(|details| details["task_id"].as_str())
            .map(str::to_owned),
        _ => None,
    });
    let task_id = task_id.expect("background task id");
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ShellCommandFinished {
            id,
            exit_code: None,
            outcome: ShellCommandOutcome::Backgrounded { task_id: event_task_id },
            ..
        } if id == "tool_1" && event_task_id == &task_id
    )));
    let _ = runtime
        .config()
        .background_tasks
        .stop(&task_id, "test cleanup", 1024)
        .await;
}

#[tokio::test]
async fn runtime_events_and_session_jsonl_do_not_leak_capped_bash_output() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({
                    "command": "printf keep; printf '%s%s%s%s' runtime -bash -leak -tail",
                    "max_output_bytes": 4
                }),
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
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run capped shell"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("shell tool should succeed");
    let event_json = persist_events_to_jsonl_and_read_back(&events).await;

    assert!(!event_json.contains("runtime-bash-leak-tail"));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ShellCommandFinished {
            stdout,
            truncated: true,
            ..
        } if stdout.len() <= 4
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            result,
            ..
        } if result
            .details
            .as_ref()
            .and_then(|details| details["stdout"].as_str())
            .is_some_and(|stdout| stdout.len() <= 4)
    )));
}

#[tokio::test]
async fn runtime_emits_terminal_lifecycle_events_for_terminal_tool() {
    let model = Arc::new(TerminalLifecycleModel::default());
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(fake_model())
            .with_workspace_root(workspace.path())
            .expect("workspace root")
            .with_permission_mode(PermissionMode::Yolo)
            .with_tool_execution_mode(ToolExecutionMode::Sequential),
        model,
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("open terminal"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("terminal tool turn should succeed");

    let handle = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::TerminalSessionStarted {
                handle,
                command,
                cols,
                rows,
                cwd,
                ..
            } if command == "bash --noprofile --norc"
                && *cols == 44
                && *rows == 9
                && cwd == &workspace_root =>
            {
                Some(handle.clone())
            }
            _ => None,
        })
        .expect("terminal start event should expose handle and PTY metadata");
    assert!(!handle.is_empty());

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::TerminalSessionOutput {
            handle: event_handle,
            output,
            ..
        } if event_handle == &handle && output.contains("terminal-event-ok")
    )));

    let finished = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::TerminalSessionFinished {
                handle: event_handle,
                status,
                ..
            } if event_handle == &handle && status == "stopped"
        )
    });
    assert!(
        finished,
        "terminal stop should emit a provider-neutral finished event"
    );
}

#[tokio::test]
async fn runtime_streams_terminal_prompt_updates_before_read() {
    let prompt = "Stage this hunk [y,n,q,a,d,j,J,g,/,s,e,p,?]?";
    let model = Arc::new(TerminalStreamingModel::default());
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(fake_model()).with_permission_mode(PermissionMode::Yolo),
        model,
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("open terminal prompt"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("terminal turn should succeed");

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionUpdate {
                name,
                partial_result,
                ..
            } if name == "Terminal" && partial_result.content.contains(prompt)
        )),
        "expected a Terminal streaming update carrying the prompt before read returned; events: {events:?}"
    );
}

#[tokio::test]
async fn runtime_events_and_session_jsonl_do_not_leak_capped_terminal_output() {
    let model = Arc::new(CappedTerminalOutputModel::default());
    let workspace = tempfile::tempdir().expect("workspace");
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(fake_model())
            .with_workspace_root(workspace.path())
            .expect("workspace root")
            .with_permission_mode(PermissionMode::Yolo)
            .with_tool_execution_mode(ToolExecutionMode::Sequential),
        model,
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("read capped terminal"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("terminal tool turn should succeed");
    let event_json = persist_events_to_jsonl_and_read_back(&events).await;

    assert!(!event_json.contains("terminal-runtime-leak-tail"));
    assert!(
        events.iter().any(|event| matches!(
        event,
        AgentEvent::TerminalSessionOutput {
            output,
            truncated: true,
            ..
        } if output.len() <= 4
        )),
        "events should include capped terminal output: {event_json}"
    );
    assert!(
        events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            name,
            result,
            ..
        } if name == "Terminal"
            && result
                .details
                .as_ref()
                .and_then(|details| details["output"].as_str())
                .is_some_and(|output| output.len() <= 4)
        )),
        "events should include capped terminal ToolExecutionFinished: {event_json}"
    );
}

async fn persist_events_to_jsonl_and_read_back(events: &[AgentEvent]) -> String {
    let temp = tempfile::tempdir().expect("session dir");
    let path = temp.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session writer");
    for event in events {
        writer
            .append_event(event)
            .await
            .expect("append session event");
    }
    writer.flush().await.expect("flush session writer");
    std::fs::read_to_string(path).expect("read session jsonl")
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

#[tokio::test]
async fn parallel_mode_serializes_non_background_ask_user_question() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "AskUserQuestion".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({
                    "questions": [{
                        "question": "Continue?",
                        "options": [
                            { "label": "Yes" },
                            { "label": "No" }
                        ]
                    }]
                }),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                arguments: json!({ "text": "should wait" }),
            },
            AiStreamEvent::MessageEnd {
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
async fn ask_mode_ask_user_question_dispatches_without_approval() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "AskUserQuestion".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({
                    "questions": [{
                        "question": "Which language?",
                        "options": [
                            { "label": "Rust" },
                            { "label": "TypeScript" }
                        ]
                    }]
                }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let (question_tx, mut question_rx) = mpsc::unbounded_channel();
    let mut tools = ToolRegistry::new();
    tools.register(neo_agent_core::AskUserTool::new(question_tx));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Ask),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let stream = runtime.run_turn(&mut context, AgentMessage::user_text("ask user"));
    let pending = timeout(Duration::from_millis(250), question_rx.recv())
        .await
        .expect("ask mode should dispatch AskUserQuestion to the host")
        .expect("question should be pending");
    assert_eq!(pending.questions[0].question, "Which language?");

    pending
        .response_tx
        .send(neo_agent_core::QuestionResponse {
            answers: vec!["Rust".to_owned()],
        })
        .expect("send question response");
    let events = stream
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    assert!(
        events
            .iter()
            .all(|event| !matches!(event, AgentEvent::ApprovalRequested { .. })),
        "AskUserQuestion must not be wrapped in the approval dialog"
    );
}

#[tokio::test]
async fn ask_mode_skill_tool_runs_without_approval() {
    let skills_dir = tempfile::tempdir().expect("skills dir");
    std::fs::write(
        skills_dir.path().join("SKILL.md"),
        r"---
name: review
description: Review the current change.
---
Review the current change carefully.
",
    )
    .expect("write skill");
    let skill_store = SkillStore::load(&[], &[skills_dir.path().to_path_buf()], Vec::new())
        .expect("load skill store");

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Skill".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({"skill": "review"}),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools_and_skills(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Ask),
        harness.client(),
        ToolRegistry::new(),
        skill_store,
    );
    let mut context = AgentContext::new();

    let events = timeout(
        Duration::from_secs(2),
        runtime
            .run_turn(&mut context, AgentMessage::user_text("use review skill"))
            .collect::<Vec<_>>(),
    )
    .await
    .expect("skill turn should finish")
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .expect("skill tool should run");

    assert!(
        events
            .iter()
            .all(|event| !matches!(event, AgentEvent::ApprovalRequested { .. })),
        "Skill must not be wrapped in the approval dialog"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionFinished { name, result, .. }
                if name == "Skill"
                    && !result.is_error
                    && result.content.contains("Review the current change carefully.")
        )),
        "Skill should execute successfully; events: {events:#?}"
    );
}

#[tokio::test]
async fn enter_plan_mode_continues_model_loop_after_mode_switch() {
    let home = tempfile::tempdir().expect("home dir");
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "EnterPlanMode".to_owned(),
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
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "continuing plan".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_home_dir(home.path())
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = timeout(
        Duration::from_secs(2),
        runtime
            .run_turn(&mut context, AgentMessage::user_text("make a plan"))
            .collect::<Vec<_>>(),
    )
    .await
    .expect("plan-mode turn should finish")
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .expect("turn should continue after entering plan mode");

    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::PlanModeEntered { .. })),
        "EnterPlanMode should still emit the plan-mode side effect"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::TextDelta { text, .. } if text == "continuing plan"
        )),
        "the model loop should continue after EnterPlanMode"
    );
    assert_eq!(
        harness.requests().len(),
        2,
        "EnterPlanMode should not stop the agent loop"
    );
}

#[tokio::test]
async fn runtime_yolo_mode_auto_approves_custom_tool() {
    let harness = echo_tool_harness("yolo approved");
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

    assert!(
        events
            .iter()
            .all(|event| !matches!(event, AgentEvent::ApprovalRequested { .. })),
        "yolo mode should not request approvals"
    );
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result(
            "tool_1",
            "echo",
            vec![Content::text("yolo approved")],
            false
        )
    );
}

#[tokio::test]
async fn runtime_auto_mode_denies_ask_user_question() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "AskUserQuestion".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({
                    "questions": [{
                        "question": "Continue?",
                        "options": [{ "label": "Yes" }, { "label": "No" }]
                    }]
                }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let (question_tx, mut question_rx) = mpsc::unbounded_channel();
    let mut tools = ToolRegistry::new();
    tools.register(AskUserTool::new(question_tx));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Auto),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("ask user"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "AskUserQuestion".to_owned(),
        result: ToolResult::error(
            "AskUserQuestion is disabled while auto permission mode is active"
        ),
    }));
    assert!(
        question_rx.try_recv().is_err(),
        "no question should be dispatched in auto mode"
    );
}

#[tokio::test]
async fn runtime_ask_mode_read_runs_and_custom_tool_asks() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("file.txt"), "hello").expect("seed file");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Read".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "path": "file.txt" }),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                arguments: json!({ "text": "needs approval" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let mut tools = ToolRegistry::with_builtin_tools();
    tools.register(EchoTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("read and call"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionFinished {
                id,
                name,
                result,
                ..
            } if id == "tool_1" && name == "Read" && result.content.contains("hello")
        )),
        "Read should run without approval in ask mode"
    );
    assert!(events.contains(&AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool_2".to_owned(),
        operation: PermissionOperation::Tool,
        subject: "echo".to_owned(),
        arguments: json!({ "text": "needs approval" }),
        session_scope: None,
        prefix_rule: None,
    }));
}

#[tokio::test]
async fn runtime_session_approval_persists_for_same_tool() {
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
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                arguments: json!({ "text": "second" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let approval_count = Arc::new(Mutex::new(0));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_approval_handler({
                let count = Arc::clone(&approval_count);
                move |_request| {
                    *count.lock().expect("count lock poisoned") += 1;
                    PermissionApprovalDecision::AllowForSession
                }
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("call echo twice"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    assert_eq!(
        *approval_count.lock().expect("count lock poisoned"),
        2,
        "generic tools have no reusable scope; each call must prompt even with AllowForSession"
    );
    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["first".to_owned(), "second".to_owned()]
    );
}

#[tokio::test]
async fn runtime_ask_mode_reviews_exit_plan_mode_with_non_empty_plan() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir =
        workspace_sessions_dir(&home.path().join("sessions"), &workspace_root).join("plans");
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Ask);
    {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let data = pm.enter(&plans_dir, true).expect("enter plan mode");
        std::fs::write(&data.path, "do the thing").expect("write plan");
    }

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "plan_summary": "Ready to execute" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let config = config.with_approval_handler(|request| {
        assert_eq!(request.operation, PermissionOperation::PlanTransition);
        PermissionApprovalDecision::AllowOnce
    });
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve plan"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ApprovalRequested {
            turn: 1,
            id,
            operation: PermissionOperation::PlanTransition,
            subject,
            arguments,
            session_scope: None,
            prefix_rule: None,
        } if id == "tool_1"
            && subject == "Exit plan mode"
            && arguments.get("plan_summary").and_then(|v| v.as_str()) == Some("Ready to execute")
            && arguments.get("plan_content").and_then(|v| v.as_str()) == Some("do the thing")
            && arguments.get("plan_path").is_some()
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::PlanModeExited { turn, .. } if *turn == 1
    )));
}

/// Regression: after an approved `ExitPlanMode`, the agent loop must continue
/// into the next turn so the model can execute the approved plan. Previously
/// `ExitPlanMode` set `terminate=true` and `continues_after_terminating_batch`
/// only matched `EnterPlanMode`, so the run ended and the user had to send
/// another prompt to resume. kimi-code's `ExitPlanMode` does not stop the turn.
#[tokio::test]
async fn exit_plan_mode_continues_loop_after_approval() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir =
        workspace_sessions_dir(&home.path().join("sessions"), &workspace_root).join("plans");
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Ask);
    {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let data = pm.enter(&plans_dir, true).expect("enter plan mode");
        std::fs::write(&data.path, "execute the plan").expect("write plan");
    }

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "plan_summary": "Ready to execute" }),
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
                text: "starting work".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let config = config.with_approval_handler(|request| {
        assert_eq!(request.operation, PermissionOperation::PlanTransition);
        PermissionApprovalDecision::AllowOnce
    });
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve plan"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::PlanModeExited { turn, .. } if *turn == 1
        )),
        "ExitPlanMode should still flip plan mode off"
    );
    assert!(
        events.iter().any(
            |event| matches!(event, AgentEvent::TextDelta { text, .. } if text == "starting work")
        ),
        "the model loop should continue after an approved ExitPlanMode"
    );
    assert_eq!(
        harness.requests().len(),
        2,
        "an approved ExitPlanMode must not stop the agent loop"
    );
}

/// Regression: when the user approves a specific model-supplied plan-review
/// option, the runtime prefixes the `ExitPlanMode` tool result with
/// "Selected approach: <label>" so the model executes only that branch. The
/// selected label reaches the runtime through the `plan_review_selected_label`
/// side-channel, mirroring the Revise-feedback channel.
#[tokio::test]
async fn exit_plan_mode_selected_option_label_prefixes_tool_result() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir =
        workspace_sessions_dir(&home.path().join("sessions"), &workspace_root).join("plans");
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Ask);
    let plan_path = {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let data = pm.enter(&plans_dir, true).expect("enter plan mode");
        std::fs::write(&data.path, "ship feature X").expect("write plan");
        data.path
    };

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({
                    "plan_summary": "Two approaches available",
                    "options": [
                        {"label": "Option A", "description": "fast"},
                        {"label": "Option B", "description": "safe"}
                    ]
                }),
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
                text: "running option a".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    // The TUI would populate this side-channel after the user picked "Option A".
    let selected_label_map = Arc::clone(&config.plan_review_selected_label);
    {
        let mut labels = selected_label_map.lock().expect("selected label lock");
        labels.insert("tool_1".to_owned(), "Option A".to_owned());
    }
    let config = config.with_approval_handler(|request| {
        assert_eq!(request.operation, PermissionOperation::PlanTransition);
        PermissionApprovalDecision::AllowOnce
    });
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let _events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve option A"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    // The selected-approach prefix must reach the next model turn. The harness
    // records every ChatRequest, so turn 2's messages must contain the prefix
    // in the ExitPlanMode tool result that was appended to the context.
    let requests = harness.requests();
    assert_eq!(
        requests.len(),
        2,
        "an approved ExitPlanMode should continue into a second model turn"
    );
    let turn2 = &requests[1];
    let turn2_text = serde_json::to_string(turn2).unwrap_or_default();
    assert!(
        turn2_text.contains("Selected approach: Option A"),
        "turn 2 request must carry the selected-approach prefix; got: {turn2_text}"
    );
    assert!(
        turn2_text.contains("Execute ONLY the selected approach"),
        "turn 2 request must carry the execute-only instruction"
    );
    // The label is consumed once.
    let labels = selected_label_map.lock().expect("selected label lock");
    assert!(
        !labels.contains_key("tool_1"),
        "selected label should be consumed after attach_exit_plan_details"
    );
    let _ = plan_path;
}

/// Regression: `ExitGoalMode` starts the durable goal and the run ends
/// cleanly. Unlike `ExitPlanMode`, the loop must NOT continue inline here —
/// goal continuation (`goal_continuation_messages`) drives subsequent turns on
/// the next `run_agent_turn` entry by design, and continuing inline would
/// re-feed the continuation message every turn and spin. This test pins the
/// boundary: the goal is started, the run finishes without a second model turn
/// from this entry, and the goal is resumable.
#[tokio::test]
async fn exit_goal_mode_starts_goal_and_ends_run_without_spinning() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Ask);

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitGoalMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({
                    "objective": "Ship goal mode",
                    "completion_criterion": "Goal tests pass",
                    "phases": ["Draft", "Implement", "Audit"],
                }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let config = config.with_approval_handler(|request| {
        assert_eq!(request.operation, PermissionOperation::GoalTransition);
        PermissionApprovalDecision::AllowOnce
    });
    let goal_manager = Arc::new(
        neo_agent_core::goal::GoalManager::load(home.path().to_path_buf())
            .await
            .expect("goal manager"),
    );
    let mut registry = ToolRegistry::with_builtin_tools();
    registry.register_goal_tools(Arc::clone(&goal_manager));
    let runtime = AgentRuntime::with_tools(config, harness.client(), registry)
        .with_goal_manager(&goal_manager);
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve goal"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        events.contains(&AgentEvent::GoalStarted {
            turn: 1,
            objective: "Ship goal mode".to_owned(),
        }),
        "ExitGoalMode should start the durable goal"
    );
    // The terminating batch must not continue inline: the goal is durable and
    // is resumed on the next run_agent_turn entry. Exactly one model request
    // means we did not spin on goal-continuation.
    assert_eq!(
        harness.requests().len(),
        1,
        "ExitGoalMode must end the run without spinning on goal continuation"
    );
    let active = goal_manager.active().expect("active goal");
    assert_eq!(active.phases, ["Draft", "Implement", "Audit"]);
}

/// Regression: even if a session-scoped approval (`AllowForSession`) is returned
/// for an `ExitPlanMode` review, the tool name must NOT be cached in
/// `session_approvals`. Plan/goal transitions are one-shot — caching the name
/// would silently auto-approve every future exit review for the rest of the
/// session. The decision is treated as `AllowOnce` for these operations.
#[tokio::test]
async fn runtime_allow_for_session_does_not_cache_exit_plan_mode() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir =
        workspace_sessions_dir(&home.path().join("sessions"), &workspace_root).join("plans");

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "plan_summary": "Ready to execute" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);

    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Ask);
    {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let data = pm.enter(&plans_dir, true).expect("enter plan mode");
        std::fs::write(&data.path, "do the thing").expect("write plan");
    }
    let session_approvals = Arc::clone(&config.session_approvals);
    let config = config.with_approval_handler(|request| {
        assert_eq!(request.operation, PermissionOperation::PlanTransition);
        // Pretend the (now-removed) "Approve for this session" option was chosen.
        PermissionApprovalDecision::AllowForSession
    });

    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("approve plan"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    // The decisive assertion: no approval key must be cached, otherwise every
    // future exit-plan review would be silently auto-approved for the session.
    let cached = session_approvals.lock().expect("session approvals lock");
    assert!(
        cached.is_empty(),
        "ExitPlanMode must not cache any session approval key (got {cached:?}); \
         AllowForSession must be treated as AllowOnce for plan/goal transitions"
    );
}

#[tokio::test]
async fn runtime_ask_mode_reviews_exit_goal_mode_and_emits_goal_started() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(
        workspace
            .path()
            .canonicalize()
            .expect("canonical workspace"),
    );
    set_config_permission_mode(&mut config, PermissionMode::Ask);

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitGoalMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({
                    "objective": "Ship goal mode",
                    "completion_criterion": "Goal tests pass",
                    "phases": ["Draft", "Implement", "Audit"],
                }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let config = config.with_approval_handler(|request| {
        assert_eq!(request.operation, PermissionOperation::GoalTransition);
        PermissionApprovalDecision::AllowOnce
    });
    let goal_manager = Arc::new(
        neo_agent_core::goal::GoalManager::load(home.path().to_path_buf())
            .await
            .expect("goal manager"),
    );
    let mut registry = ToolRegistry::with_builtin_tools();
    registry.register_goal_tools(Arc::clone(&goal_manager));
    let runtime = AgentRuntime::with_tools(config, harness.client(), registry)
        .with_goal_manager(&goal_manager);
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve goal"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.contains(&AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool_1".to_owned(),
        operation: PermissionOperation::GoalTransition,
        subject: "Start reviewed goal".to_owned(),
        arguments: json!({
            "objective": "Ship goal mode",
            "completion_criterion": "Goal tests pass",
            "phases": ["Draft", "Implement", "Audit"],
        }),
        session_scope: None,
        prefix_rule: None,
    }));
    assert!(events.contains(&AgentEvent::GoalStarted {
        turn: 1,
        objective: "Ship goal mode".to_owned(),
    }));
    let active = goal_manager.active().expect("active goal");
    assert_eq!(active.phases, ["Draft", "Implement", "Audit"]);
}

#[tokio::test]
async fn runtime_ask_mode_exit_plan_mode_reject_keeps_plan_active_with_feedback() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir =
        workspace_sessions_dir(&home.path().join("sessions"), &workspace_root).join("plans");
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Ask);
    {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let data = pm.enter(&plans_dir, true).expect("enter plan mode");
        std::fs::write(&data.path, "do the thing").expect("write plan");
    }
    let plan_mode = Arc::clone(&config.plan_mode);
    let plan_review_feedback = Arc::clone(&config.plan_review_feedback);

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "plan_summary": "Ready to execute" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let config = config.with_approval_handler(move |request| {
        if request.operation == PermissionOperation::PlanTransition {
            if let Ok(mut map) = plan_review_feedback.lock() {
                map.insert(request.id.clone(), "add more detail".to_owned());
            }
            PermissionApprovalDecision::Reject
        } else {
            PermissionApprovalDecision::AllowOnce
        }
    });
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("revise plan"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::PlanModeExited { .. })),
        "plan mode should remain active after revise"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            id,
            name,
            result,
            ..
        } if id == "tool_1" && name == "ExitPlanMode" && result.content.contains("User requested revisions")
    )));
    assert!(plan_mode.read().expect("plan mode lock").is_active());
}

#[tokio::test]
async fn runtime_plan_mode_guard_denies_write_outside_plan_file() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir =
        workspace_sessions_dir(&home.path().join("sessions"), &workspace_root).join("plans");
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Yolo);
    {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let _data = pm.enter(&plans_dir, true).expect("enter plan mode");
    }
    let plan_mode = Arc::clone(&config.plan_mode);

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Write".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "path": "other.txt", "content": "x" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("write while planning"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            id,
            name,
            result,
            ..
        } if id == "tool_1" && name == "Write" && result.is_error && result.content.contains("plan mode")
    )));
    assert!(
        plan_mode.read().expect("plan mode lock").is_active(),
        "plan mode should stay active after a blocked write"
    );
}

#[tokio::test]
async fn runtime_plan_mode_allows_writing_active_plan_file_outside_workspace() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir =
        workspace_sessions_dir(&home.path().join("sessions"), &workspace_root).join("plans");
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Yolo);
    let plan_path = {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        pm.enter(&plans_dir, true).expect("enter plan mode").path
    };

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Write".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({
                    "path": plan_path,
                    "content": "# Plan\n\nUse Write, not Bash."
                }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("write plan file"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            id,
            name,
            result,
            ..
        } if id == "tool_1" && name == "Write" && !result.is_error
    )));
    assert_eq!(
        std::fs::read_to_string(&plan_path).expect("read plan"),
        "# Plan\n\nUse Write, not Bash."
    );
}

/// Regression: `Edit` (which resolves via `resolve_workspace_path`, not
/// `resolve_parent_for_write`) must also be able to reach the active plan file
/// under the `NEO_HOME` sessions bucket while plan mode is active.
#[tokio::test]
async fn runtime_plan_mode_allows_editing_active_plan_file_outside_workspace() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir =
        workspace_sessions_dir(&home.path().join("sessions"), &workspace_root).join("plans");
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Yolo);
    let plan_path = {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let data = pm.enter(&plans_dir, true).expect("enter plan mode");
        std::fs::write(&data.path, "# Plan\n\nDraft.").expect("seed plan");
        data.path
    };

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Edit".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({
                    "path": plan_path,
                    "old": "Draft.",
                    "new": "Finalized."
                }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("edit plan file"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            id,
            name,
            result,
            ..
        } if id == "tool_1" && name == "Edit" && !result.is_error
    )));
    assert_eq!(
        std::fs::read_to_string(&plan_path).expect("read plan"),
        "# Plan\n\nFinalized."
    );
}

fn setup_active_plan(
    config: &mut AgentConfig,
    home: &tempfile::TempDir,
    workspace: &tempfile::TempDir,
    content: &str,
) {
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir =
        workspace_sessions_dir(&home.path().join("sessions"), &workspace_root).join("plans");
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    let mut pm = config.plan_mode.write().expect("plan mode lock");
    let data = pm.enter(&plans_dir, true).expect("enter plan mode");
    std::fs::write(&data.path, content).expect("write plan");
}

#[tokio::test]
async fn ask_mode_asks_for_bash() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "command": "mkdir test_dir" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run bash"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    // `mkdir test_dir` is NOT a known-safe command (mkdir isn't in the safe
    // list), so it must prompt. Use matches! because the scope carries a
    // dynamic workspace path.
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ApprovalRequested {
            id,
            operation: PermissionOperation::Shell,
            subject,
            arguments,
            session_scope,
            ..
        } if id == "tool_1"
            && subject == "mkdir test_dir"
            && arguments == &json!({ "command": "mkdir test_dir" })
            && session_scope.as_ref().is_some_and(|s| !s.is_empty())
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            id,
            name,
            result,
            ..
        } if id == "tool_1" && name == "Bash" && result.content.contains("approval required")
    )));
}

#[tokio::test]
async fn auto_mode_approves_bash_without_approval() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "command": "printf auto-ok" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Auto)
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run bash"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::ApprovalRequested { .. })),
        "auto mode should not request bash approval"
    );
    assert!(events.contains(&AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        exit_code: Some(0),
        stdout: "auto-ok".to_owned(),
        stderr: String::new(),
        truncated: false,
        origin: ShellCommandOrigin::ModelBashTool,
        outcome: ShellCommandOutcome::Completed,
    }));
}

#[tokio::test]
async fn yolo_mode_approves_write_without_approval() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Write".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "path": "yolo.txt", "content": "yolo" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
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
        .expect("turn should succeed");

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::ApprovalRequested { .. })),
        "yolo mode should not request write approval"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("yolo.txt")).expect("written file"),
        "yolo"
    );
}

#[tokio::test]
async fn auto_exit_plan_mode_does_not_request_review() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let mut config = AgentConfig::for_model(fake_model());
    setup_active_plan(&mut config, &home, &workspace, "do the thing");
    set_config_permission_mode(&mut config, PermissionMode::Auto);

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "plan_summary": "Ready" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve plan"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::ApprovalRequested { .. })),
        "auto mode should not review ExitPlanMode"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::PlanModeExited { turn, .. } if *turn == 1
    )));
}

#[tokio::test]
async fn yolo_exit_plan_mode_with_non_empty_plan_requests_review() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let mut config = AgentConfig::for_model(fake_model());
    setup_active_plan(&mut config, &home, &workspace, "do the thing");
    set_config_permission_mode(&mut config, PermissionMode::Yolo);

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "plan_summary": "Ready" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let config = config.with_approval_handler(|request| {
        assert_eq!(request.operation, PermissionOperation::PlanTransition);
        PermissionApprovalDecision::AllowOnce
    });
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve plan"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ApprovalRequested {
            turn: 1,
            id,
            operation: PermissionOperation::PlanTransition,
            subject,
            arguments,
            session_scope: None,
            prefix_rule: None,
        } if id == "tool_1"
            && subject == "Exit plan mode"
            && arguments.get("plan_summary").and_then(|v| v.as_str()) == Some("Ready")
            && arguments.get("plan_content").and_then(|v| v.as_str()) == Some("do the thing")
            && arguments.get("plan_path").is_some()
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::PlanModeExited { turn, .. } if *turn == 1
    )));
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

#[derive(Default)]
struct TerminalLifecycleModel {
    requests: Mutex<Vec<ChatRequest>>,
}

impl ModelClient for TerminalLifecycleModel {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        let next = terminal_lifecycle_events_for_request(&request);
        self.requests
            .lock()
            .expect("request lock poisoned")
            .push(request);
        futures::stream::iter(next.into_iter().map(Ok)).boxed()
    }
}

#[derive(Default)]
struct CappedTerminalOutputModel {
    requests: Mutex<Vec<ChatRequest>>,
}

impl ModelClient for CappedTerminalOutputModel {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        let next = capped_terminal_output_events_for_request(&request);
        self.requests
            .lock()
            .expect("request lock poisoned")
            .push(request);
        futures::stream::iter(next.into_iter().map(Ok)).boxed()
    }
}

#[derive(Default)]
struct TerminalStreamingModel {
    requests: Mutex<Vec<ChatRequest>>,
}

impl ModelClient for TerminalStreamingModel {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        self.requests
            .lock()
            .expect("request lock poisoned")
            .push(request.clone());
        let next = terminal_streaming_events_for_request(&request);
        futures::stream::iter(next.into_iter().map(Ok)).boxed()
    }
}

fn terminal_streaming_events_for_request(request: &ChatRequest) -> Vec<AiStreamEvent> {
    const PROMPT: &str = "Stage this hunk [y,n,q,a,d,j,J,g,/,s,e,p,?]?";
    let tool_results = request
        .messages
        .iter()
        .filter_map(tool_result_text)
        .collect::<Vec<_>>();
    let turn_index = tool_results.len() + 1;
    let handle = tool_results
        .iter()
        .find_map(|content| terminal_handle(content));
    let last = tool_results.last().map(String::as_str).unwrap_or_default();

    match handle {
        None => terminal_tool_turn(
            turn_index,
            "tool_1",
            json!({
                "mode": "start",
                "command": format!(
                    "python3 - <<'PY'\nimport sys, time\nsys.stdout.write('{PROMPT} ')\nsys.stdout.flush()\ntime.sleep(1)\nPY"
                ),
                "cols": 100,
                "rows": 24
            }),
        ),
        Some(handle) if !last.contains("status: stopped") && !last.contains("output:") => {
            terminal_tool_turn(
                turn_index,
                "tool_2",
                json!({
                    "mode": "read",
                    "handle": handle,
                    "max_output_bytes": 1024
                }),
            )
        }
        _ => end_turn_done(turn_index),
    }
}

fn capped_terminal_output_events_for_request(request: &ChatRequest) -> Vec<AiStreamEvent> {
    let tool_results = request
        .messages
        .iter()
        .filter_map(tool_result_text)
        .collect::<Vec<_>>();
    let turn_index = tool_results.len() + 1;
    let handle = tool_results
        .iter()
        .find_map(|content| terminal_handle(content));
    let last = tool_results.last().map(String::as_str).unwrap_or_default();
    match handle {
        None => terminal_tool_turn(
            turn_index,
            "tool_start",
            json!({
                "mode": "start",
                "command": "printf term; printf '%s%s%s%s' inal -runtime -leak -tail; sleep 1"
            }),
        ),
        Some(_) if last.contains("status: stopped") => vec![
            AiStreamEvent::MessageStart {
                id: format!("msg_{turn_index}"),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        Some(handle) if last.contains("truncated: true") => terminal_tool_turn(
            turn_index,
            "tool_stop",
            json!({
                "mode": "stop",
                "handle": handle,
                "max_output_bytes": 4
            }),
        ),
        Some(_) if last.contains("status: running") => bash_tool_turn(
            turn_index,
            "tool_wait",
            json!({
                "command": "sleep 0.05; printf waited"
            }),
        ),
        Some(handle) if !last.contains("status: stopped") => terminal_tool_turn(
            turn_index,
            "tool_read",
            json!({
                "mode": "read",
                "handle": handle,
                "max_output_bytes": 4
            }),
        ),
        _ => end_turn_done(turn_index),
    }
}

fn bash_tool_turn(
    turn_index: usize,
    tool_id: &str,
    arguments: serde_json::Value,
) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            id: format!("msg_{turn_index}"),
        },
        AiStreamEvent::ToolCallStart {
            id: tool_id.to_owned(),
            name: "Bash".to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: tool_id.to_owned(),
            arguments,
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::ToolUse,
            usage: None,
        },
    ]
}

fn terminal_lifecycle_events_for_request(request: &ChatRequest) -> Vec<AiStreamEvent> {
    let tool_results = request
        .messages
        .iter()
        .filter_map(tool_result_text)
        .collect::<Vec<_>>();
    let turn_index = tool_results.len() + 1;
    match tool_results
        .last()
        .and_then(|content| terminal_handle(content))
    {
        None => terminal_tool_turn(
            turn_index,
            "tool_start",
            json!({
                "mode": "start",
                "command": "bash --noprofile --norc",
                "cols": 44,
                "rows": 9
            }),
        ),
        Some(_)
            if tool_results
                .last()
                .is_some_and(|content| content.contains("status: stopped")) =>
        {
            end_turn_done(turn_index)
        }
        Some(handle)
            if tool_results
                .last()
                .is_some_and(|content| content.contains("terminal-event-ok")) =>
        {
            terminal_tool_turn(
                turn_index,
                "tool_stop",
                json!({
                    "mode": "stop",
                    "handle": handle
                }),
            )
        }
        Some(handle)
            if tool_results
                .last()
                .is_some_and(|content| content.contains("written: true")) =>
        {
            terminal_tool_turn(
                turn_index,
                "tool_read",
                json!({
                    "mode": "read",
                    "handle": handle,
                    "max_output_bytes": 4096
                }),
            )
        }
        Some(handle)
            if tool_results
                .last()
                .is_some_and(|content| content.contains("status: running")) =>
        {
            terminal_tool_turn(
                turn_index,
                "tool_write",
                json!({
                    "mode": "write",
                    "handle": handle,
                    "input": "printf terminal-event-ok\\n\n"
                }),
            )
        }
        _ => end_turn_done(turn_index),
    }
}

fn end_turn_done(turn_index: usize) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            id: format!("msg_{turn_index}"),
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

fn terminal_tool_turn(
    turn_index: usize,
    tool_id: &str,
    arguments: serde_json::Value,
) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            id: format!("msg_{turn_index}"),
        },
        AiStreamEvent::ToolCallStart {
            id: tool_id.to_owned(),
            name: "Terminal".to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: tool_id.to_owned(),
            arguments,
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::ToolUse,
            usage: None,
        },
    ]
}

fn tool_result_text(message: &neo_ai::ChatMessage) -> Option<String> {
    match message {
        neo_ai::ChatMessage::ToolResult { content, .. } => Some(
            content
                .iter()
                .filter_map(|part| match part {
                    neo_ai::ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        ),
        _ => None,
    }
}

fn terminal_handle(content: &str) -> Option<String> {
    content
        .lines()
        .find_map(|line| line.strip_prefix("handle: "))
        .map(str::trim)
        .filter(|handle| !handle.is_empty())
        .map(ToOwned::to_owned)
}

#[derive(Clone)]
struct DelayedHarness {
    model: ModelSpec,
    client: Arc<DelayedModelClient>,
}

fn model_with_capabilities(capabilities: ModelCapabilities) -> ModelSpec {
    ModelSpec {
        provider: ProviderId("capability-test".to_owned()),
        model: "capability-test-model".to_owned(),
        api: ApiKind::Local,
        capabilities,
    }
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
                    max_output_tokens: None,
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

#[tokio::test]
async fn runtime_drains_live_steer_input_at_step_boundary() {
    let harness = FakeHarness::from_turns([vec![
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
    ]]);
    let steer_input = neo_agent_core::SteerInputHandle::new();
    steer_input.push(neo_agent_core::ActiveTurnInput::SteerNow(
        AgentMessage::user_text("live steer"),
    ));
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model())
            .with_queue_modes(QueueMode::OneAtATime, QueueMode::All),
        harness.client(),
    )
    .with_steer_input(steer_input.clone());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("start"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("live-steer run should succeed");

    // The runtime must emit a SteeringQueued event when it drains the live
    // steer input, then inject the steer message before the second model call.
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::SteeringQueued { message }
            if message == &AgentMessage::user_text("live steer")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::QueueDrained { kind, count: 1 } if *kind == neo_agent_core::QueueKind::Steering
    )));
    // The steer text should appear as an appended user message before "second".
    let appended = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MessageAppended { message } => Some(message.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(appended.contains(&AgentMessage::user_text("live steer")));
    // The handle is drained after the turn.
    assert_eq!(steer_input.pending(), 0);
}

#[tokio::test]
async fn runtime_drains_live_follow_up_input_as_new_turn() {
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
    ]);
    let steer_input = neo_agent_core::SteerInputHandle::new();
    steer_input.push(neo_agent_core::ActiveTurnInput::FollowUp(
        AgentMessage::user_text("queued follow"),
    ));
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model())
            .with_queue_modes(QueueMode::OneAtATime, QueueMode::All),
        harness.client(),
    )
    .with_steer_input(steer_input.clone());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("start"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("live-follow-up run should succeed");

    // A FollowUpQueued event must be emitted, and the follow-up must start a
    // fresh model turn after the first one ends (FIFO).
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::FollowUpQueued { message }
            if message == &AgentMessage::user_text("queued follow")
    )));
    assert_eq!(
        harness.requests().len(),
        2,
        "follow-up should trigger a second model call"
    );
    assert_eq!(steer_input.pending(), 0);
}

#[tokio::test]
async fn runtime_reclassifies_promoted_follow_up_as_steer_without_running_follow_up() {
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
    ]);
    let steer_input = neo_agent_core::SteerInputHandle::new();
    steer_input.push(neo_agent_core::ActiveTurnInput::PromoteFollowUpToSteer);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model())
            .with_queue_modes(QueueMode::OneAtATime, QueueMode::All),
        harness.client(),
    )
    .with_steer_input(steer_input.clone());
    let mut context = AgentContext::new();
    context.queue_follow_up_message(AgentMessage::user_text("queued follow"));

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("start"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("promoted follow-up run should succeed");

    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::FollowUpQueued { message }
            if message == &AgentMessage::user_text("queued follow")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::QueueDrained { kind, count: 1 }
            if *kind == neo_agent_core::QueueKind::FollowUp
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::SteeringQueued { message }
            if message == &AgentMessage::user_text("queued follow")
    )));
    assert_eq!(
        harness.requests().len(),
        1,
        "promoted follow-up should run once as a steer, not again as a follow-up"
    );
    assert!(matches!(
        harness.requests()[0].messages.last(),
        Some(neo_ai::ChatMessage::User { content }) if matches!(
            content.first(),
            Some(neo_ai::ContentPart::Text { text }) if text == "queued follow"
        )
    ));
    assert_eq!(context.pending_follow_up_len(), 0);
    assert_eq!(context.pending_steering_len(), 0);
    assert_eq!(steer_input.pending(), 0);
}

// ---------------------------------------------------------------------------
// NEO-30 Layer 1/2/3: approval key scoping, prefix rules, safety classification
// ---------------------------------------------------------------------------

fn count_approval_requests(events: &[AgentEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, AgentEvent::ApprovalRequested { .. }))
        .count()
}

#[tokio::test]
async fn layer1_bash_session_approval_exact_command_only() {
    // Approving `git status` must NOT cover `git log`. Core regression test.
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "command": "git status" }),
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
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                arguments: json!({ "command": "python script.py" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let approval_count = Arc::new(Mutex::new(0));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace root")
            .with_approval_handler({
                let count = Arc::clone(&approval_count);
                move |_request| {
                    *count.lock().expect("count lock poisoned") += 1;
                    PermissionApprovalDecision::AllowForSession
                }
            }),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("git status then git log"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");
    // `git status` is auto-approved by Layer 3 (safe git subcommand), so only
    // `git log --oneline -20` reaches the handler. This proves the safe-command
    // path + that different commands don't share approval.
    assert!(
        count_approval_requests(&events) <= 1,
        "git status (safe) auto-approves; git log is a different command and must not inherit"
    );
}

#[tokio::test]
async fn allow_for_session_does_not_persist_prefix_rule() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "command": "python script.py" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let config = AgentConfig::for_model(harness.model())
        .with_permission_mode(PermissionMode::Ask)
        .with_workspace_root(workspace.path())
        .expect("workspace root")
        .with_approval_handler(|_request| PermissionApprovalDecision::AllowForSession);
    let prefix_store = Arc::clone(&config.prefix_approval_rules);
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("python script"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ApprovalRequested { prefix_rule: Some(rule), .. }
            if rule.prefix == vec!["python".to_owned()]
    )));
    assert!(
        prefix_store
            .lock()
            .expect("prefix store")
            .prefix_rules
            .is_empty(),
        "AllowForSession must not persist prefix approval rules"
    );
}

#[tokio::test]
async fn allow_for_prefix_persists_prefix_rule() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "command": "python script.py" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let config = AgentConfig::for_model(harness.model())
        .with_permission_mode(PermissionMode::Ask)
        .with_workspace_root(workspace.path())
        .expect("workspace root")
        .with_approval_handler(|_request| PermissionApprovalDecision::AllowForPrefix);
    let prefix_store = Arc::clone(&config.prefix_approval_rules);
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("python script"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert_eq!(
        prefix_store
            .lock()
            .expect("prefix store")
            .prefix_rules
            .iter()
            .map(|rule| rule.prefix.clone())
            .collect::<Vec<_>>(),
        vec![vec!["python".to_owned()]]
    );
}

#[tokio::test]
async fn layer3_safe_command_auto_approved() {
    // `cat README.md` is a known-safe command — it should not prompt at all.
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "command": "cat README.md" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let approval_count = Arc::new(Mutex::new(0));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace root")
            .with_approval_handler({
                let count = Arc::clone(&approval_count);
                move |_request| {
                    *count.lock().expect("count lock poisoned") += 1;
                    PermissionApprovalDecision::AllowOnce
                }
            }),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("cat readme"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");
    assert_eq!(
        count_approval_requests(&events),
        0,
        "known-safe commands like `cat` must be auto-approved without prompt"
    );
}

#[tokio::test]
async fn layer3_dangerous_command_forces_prompt_no_scope() {
    // `rm -rf /tmp/x` is dangerous — it must prompt and offer NO session scope.
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                arguments: json!({ "command": "rm -rf /tmp/x" }),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace root")
            .with_approval_handler(|_request| PermissionApprovalDecision::AllowOnce),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("rm"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");
    assert_eq!(
        count_approval_requests(&events),
        1,
        "dangerous commands must prompt"
    );
    // The approval event must carry NO session_scope (so it can't be cached).
    let has_scope = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ApprovalRequested { session_scope: Some(s), .. } if !s.is_empty()
        )
    });
    assert!(
        !has_scope,
        "dangerous commands must not offer a reusable session scope"
    );
}
