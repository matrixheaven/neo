use super::tool_dispatch::assert_runtime_rejects_unsupported_capability;
use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, AgentRuntimeError,
    CompactionSettings, Content, StopReason, harness::FakeHarness,
};
use neo_ai::{
    AiError, AiStreamEvent, ApiKind, MessagePhase, ModelCapabilities, ModelSpec, ProviderId,
    ReasoningCapability, ReasoningEffort, ReasoningSelection, ThinkingKind,
};

#[tokio::test]
async fn runtime_rejects_reasoning_selection_when_model_lacks_reasoning_before_request() {
    let harness = FakeHarness::from_events([AiStreamEvent::MessageEnd {
        phase: MessagePhase::Unknown,
        stop_reason: neo_ai::StopReason::EndTurn,
        usage: None,
    }]);
    let mut config = AgentConfig::for_model(harness.model());
    config.reasoning = ReasoningSelection::Effort {
        effort: ReasoningEffort::low(),
    };

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
async fn runtime_rejects_unsupported_reasoning_selection_before_request() {
    let harness = FakeHarness::from_events([AiStreamEvent::MessageEnd {
        phase: MessagePhase::Unknown,
        stop_reason: neo_ai::StopReason::EndTurn,
        usage: None,
    }]);
    let mut config = AgentConfig::for_model(model_with_capabilities(ModelCapabilities {
        reasoning: ReasoningCapability::Effort {
            values: vec![ReasoningEffort::high()],
            disable_supported: true,
        },
        ..ModelCapabilities::tool_chat()
    }));
    config.reasoning = ReasoningSelection::BudgetTokens {
        budget_tokens: 8192,
    };
    let runtime = AgentRuntime::new(config, harness.client());
    let mut context = AgentContext::new();

    let error = runtime
        .run_turn(&mut context, AgentMessage::user_text("think with a budget"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect_err("unsupported reasoning selection should fail before provider request");
    let message = error.to_string();

    assert!(matches!(
        error,
        AgentRuntimeError::Model(AiError::Configuration { message: _ })
    ));
    assert!(
        message.contains("model capability-test/capability-test-model"),
        "error should identify the active provider/model: {message}"
    );
    assert!(
        message.contains("BudgetTokens"),
        "error should include the unsupported selection: {message}"
    );
    assert!(
        message.contains("Effort"),
        "error should include the model reasoning capability: {message}"
    );
    assert!(
        harness.requests().is_empty(),
        "request should not reach provider"
    );
}

#[tokio::test]
async fn runtime_passes_reasoning_selection_into_chat_request_options() {
    let harness = FakeHarness::from_events([AiStreamEvent::MessageEnd {
        phase: MessagePhase::Unknown,
        stop_reason: neo_ai::StopReason::EndTurn,
        usage: None,
    }]);
    let mut config = AgentConfig::for_model(model_with_capabilities(ModelCapabilities {
        reasoning: ReasoningCapability::Effort {
            values: vec![ReasoningEffort::try_from("UltraMax").expect("custom effort")],
            disable_supported: true,
        },
        ..ModelCapabilities::tool_chat()
    }));
    config.reasoning = ReasoningSelection::Effort {
        effort: ReasoningEffort::try_from("UltraMax").expect("custom effort"),
    };
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
        harness.requests()[0].options.reasoning,
        ReasoningSelection::Effort {
            effort: ReasoningEffort::try_from("UltraMax").expect("custom effort"),
        }
    );
}

#[tokio::test]
async fn runtime_streams_thinking_events_and_persists_thinking_content() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: "msg_thinking".to_owned(),
        },
        AiStreamEvent::ThinkingStart {
            id: "thinking_1".to_owned(),
            kind: ThinkingKind::Unknown,
        },
        AiStreamEvent::ThinkingDelta {
            text: "Checked ".to_owned(),
        },
        AiStreamEvent::ThinkingDelta {
            text: "the plan.".to_owned(),
        },
        AiStreamEvent::ThinkingEnd {
            signature: Some("sig-1".into()),
            redacted: false,
        },
        AiStreamEvent::TextDelta {
            text: "final answer".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
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
        kind: ThinkingKind::Unknown,
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
        signature: Some("sig-1".into()),
        redacted: false,
    }));
    assert_eq!(
        context.messages()[1],
        AgentMessage::assistant(
            [
                Content::thinking_with_kind_and_id(
                    "Checked the plan.",
                    Some("sig-1".into()),
                    false,
                    ThinkingKind::Unknown,
                    Some("thinking_1".into()),
                ),
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
            phase: MessagePhase::Unknown,
            id: "msg_multi_thinking".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "intro ".to_owned(),
        },
        AiStreamEvent::ThinkingStart {
            id: "thinking_1".to_owned(),
            kind: ThinkingKind::Unknown,
        },
        AiStreamEvent::ThinkingDelta {
            text: "first thought".to_owned(),
        },
        AiStreamEvent::ThinkingEnd {
            signature: Some("sig-1".into()),
            redacted: false,
        },
        AiStreamEvent::ThinkingStart {
            id: "thinking_2".to_owned(),
            kind: ThinkingKind::Unknown,
        },
        AiStreamEvent::ThinkingDelta {
            text: "second thought".to_owned(),
        },
        AiStreamEvent::ThinkingEnd {
            signature: Some("sig-2".into()),
            redacted: true,
        },
        AiStreamEvent::TextDelta {
            text: "outro".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
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
                Content::thinking_with_kind_and_id(
                    "first thought",
                    Some("sig-1".into()),
                    false,
                    ThinkingKind::Unknown,
                    Some("thinking_1".into()),
                ),
                Content::thinking_with_kind_and_id(
                    "second thought",
                    Some("sig-2".into()),
                    true,
                    ThinkingKind::Unknown,
                    Some("thinking_2".into()),
                ),
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
                phase: MessagePhase::Unknown,
                id: "msg_thinking".to_owned(),
            },
            AiStreamEvent::ThinkingStart {
                id: "thinking_1".to_owned(),
                kind: ThinkingKind::Unknown,
            },
            AiStreamEvent::ThinkingDelta {
                text: "local reasoning summary".to_owned(),
            },
            AiStreamEvent::ThinkingEnd {
                signature: Some("sig-1".into()),
                redacted: false,
            },
            AiStreamEvent::TextDelta {
                text: "answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_followup".to_owned(),
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
                    signature: Some("sig-1".into()),
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
                phase: MessagePhase::Unknown,
                id: "msg_thinking".to_owned(),
            },
            AiStreamEvent::ThinkingStart {
                id: "thinking_1".to_owned(),
                kind: ThinkingKind::Unknown,
            },
            AiStreamEvent::ThinkingDelta {
                text: "local reasoning summary".to_owned(),
            },
            AiStreamEvent::ThinkingEnd {
                signature: Some("sig-1".into()),
                redacted: false,
            },
            AiStreamEvent::TextDelta {
                text: "answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_followup".to_owned(),
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
            phase: MessagePhase::Unknown,
            id: "msg_after_thinking".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "kept".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
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

pub(crate) fn model_with_capabilities(capabilities: ModelCapabilities) -> ModelSpec {
    ModelSpec {
        provider: ProviderId("capability-test".to_owned()),
        model: "capability-test-model".to_owned(),
        api: ApiKind::Local,
        capabilities,
    }
}
