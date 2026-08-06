use super::fake_harness::DelayedHarness;
use super::fake_harness::DelayedStep;
use super::tool_dispatch::assert_runtime_rejects_unsupported_capability;
use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, Content, QueueMode,
    StopReason, harness::FakeHarness,
};
use neo_ai::{AiStreamEvent, MessagePhase};
use std::time::Duration;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn runtime_streams_one_turn_text_and_updates_context() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "hel".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "lo".to_owned(),
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
                used_tokens: 4,
                projected_tokens: Some(4),
                max_tokens: None,
                trigger_tokens: None,
                remaining_tokens: None,
                source: Some(neo_agent_core::ContextWindowSource::MissingModelWindow),
            },
            AgentEvent::TurnStarted { turn: 1 },
            AgentEvent::MessageStarted {
                turn: 1,
                id: "msg_1".to_owned(),
                phase: MessagePhase::Unknown,
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
                phase: MessagePhase::Unknown,
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
                used_tokens: 9,
                projected_tokens: Some(9),
                max_tokens: None,
                trigger_tokens: None,
                remaining_tokens: None,
                source: Some(neo_agent_core::ContextWindowSource::MissingModelWindow),
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
async fn runtime_emits_provider_token_usage() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "hello".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: Some(neo_ai::TokenUsage {
                input_tokens: 123,
                output_tokens: 45,
                input_cache_read_tokens: 100,
                input_cache_write_tokens: 7,
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
            input_cache_read_tokens: 100,
            input_cache_write_tokens: 7,
        },
    }));
}

#[tokio::test]
async fn runtime_yields_model_events_before_model_stream_finishes() {
    let harness = DelayedHarness::new(vec![
        DelayedStep::Event(AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: "msg_1".to_owned(),
        }),
        DelayedStep::Event(AiStreamEvent::TextDelta {
            text: "early".to_owned(),
        }),
        DelayedStep::Delay(Duration::from_secs(5)),
        DelayedStep::Event(AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
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
            used_tokens: 3,
            projected_tokens: Some(3),
            max_tokens: None,
            trigger_tokens: None,
            remaining_tokens: None,
            source: Some(neo_agent_core::ContextWindowSource::MissingModelWindow),
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
            phase: MessagePhase::Unknown,
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
            phase: MessagePhase::Unknown,
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
            phase: MessagePhase::Unknown,
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

    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::MessageFinished { id, .. } if id == "msg_cancel"
    )));
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
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::MessageAppended {
            message: AgentMessage::Assistant { .. },
        }
    )));
    assert!(!context.messages().iter().any(|message| {
        matches!(message, AgentMessage::Assistant { .. }) || message.text().contains("partial")
    }));
}

#[tokio::test]
async fn agent_event_stream_cancels_only_when_abandoned() {
    let harness = DelayedHarness::new(vec![
        DelayedStep::Event(AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: "msg_1".to_owned(),
        }),
        DelayedStep::Event(AiStreamEvent::TextDelta {
            text: "early".to_owned(),
        }),
        DelayedStep::Delay(Duration::from_millis(100)),
        DelayedStep::Event(AiStreamEvent::TextDelta {
            text: "late".to_owned(),
        }),
        DelayedStep::Event(AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        }),
    ]);
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());

    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    {
        let mut stream = runtime.run_turn_with_cancel(
            &mut context,
            AgentMessage::user_text("abandon"),
            cancel.clone(),
        );
        let event = timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("first event should stream")
            .expect("stream should not close")
            .expect("event should be ok");
        assert!(matches!(event, AgentEvent::RunStarted { turn: 1 }));
    }
    timeout(Duration::from_millis(500), cancel.cancelled())
        .await
        .expect("dropping an incomplete stream should cancel the turn");

    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    {
        let mut stream = runtime.run_turn_with_cancel(
            &mut context,
            AgentMessage::user_text("drain"),
            cancel.clone(),
        );
        while stream.next().await.is_some() {}
    }
    assert!(
        !cancel.is_cancelled(),
        "draining a stream to completion should not cancel the turn"
    );
}

#[tokio::test]
async fn runtime_rejects_image_content_when_model_lacks_images_before_request() {
    let harness = FakeHarness::from_events([AiStreamEvent::MessageEnd {
        phase: MessagePhase::Unknown,
        stop_reason: neo_ai::StopReason::EndTurn,
        usage: None,
    }]);
    let config = AgentConfig::for_model(harness.model());

    assert_runtime_rejects_unsupported_capability(
        config,
        &harness,
        AgentMessage::user_content(vec![Content::Image {
            mime_type: "image/png".into(),
            data: neo_agent_core::ImageRef::Url("https://example.test/cat.png".into()),
        }]),
        "does not support image input",
        "unsupported images should fail before provider request",
    )
    .await;
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
                used_tokens: 6,
                projected_tokens: Some(6),
                max_tokens: None,
                trigger_tokens: None,
                remaining_tokens: None,
                source: Some(neo_agent_core::ContextWindowSource::MissingModelWindow),
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
            phase: MessagePhase::Unknown,
            id: "msg_after_resume".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "resumed".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
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
async fn runtime_drains_queued_steering_before_followups() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "first".to_owned(),
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
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "second".to_owned(),
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
                id: "msg_3".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "third".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
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
async fn runtime_drains_live_steer_input_at_step_boundary() {
    let harness = FakeHarness::from_turns([vec![
        AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "first".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
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
    // The handle is drained and closed after the turn.
    assert_eq!(steer_input.pending(), 0);
    assert!(
        !steer_input.try_push(neo_agent_core::ActiveTurnInput::SteerNow(
            AgentMessage::user_text("too late"),
        ))
    );
}

#[tokio::test]
async fn runtime_drains_live_follow_up_input_as_new_turn() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "first".to_owned(),
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
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "second".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
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
async fn runtime_drains_multiple_live_follow_ups_all_by_default() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "first".to_owned(),
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
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "second".to_owned(),
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
                id: "msg_3".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "third".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let steer_input = neo_agent_core::SteerInputHandle::new();
    steer_input.push(neo_agent_core::ActiveTurnInput::FollowUp(
        AgentMessage::user_text("queued one"),
    ));
    steer_input.push(neo_agent_core::ActiveTurnInput::FollowUp(
        AgentMessage::user_text("queued two"),
    ));
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client())
        .with_steer_input(steer_input.clone());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("start"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("multi-follow-up run should succeed");

    assert_eq!(
        harness.requests().len(),
        2,
        "default follow-up queue mode should drain all queued follow-ups into the next model turn"
    );
    let drained_counts = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::QueueDrained {
                kind: neo_agent_core::QueueKind::FollowUp,
                count,
            } => Some(*count),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        drained_counts,
        vec![2],
        "default follow-up queue mode should preserve FIFO order while draining all pending items"
    );
    let appended_users = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MessageAppended {
                message: AgentMessage::User { content, .. },
            } => Some(
                content
                    .iter()
                    .filter_map(|part| match part {
                        Content::Text { text } => Some(text.as_ref()),
                        Content::Image { .. } | Content::Thinking { .. } => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(appended_users, vec!["start", "queued one", "queued two"]);
    assert_eq!(steer_input.pending(), 0);
}

#[tokio::test]
async fn runtime_drains_multiple_live_follow_ups_one_turn_at_a_time_when_configured() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "first".to_owned(),
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
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "second".to_owned(),
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
                id: "msg_3".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "third".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let steer_input = neo_agent_core::SteerInputHandle::new();
    steer_input.push(neo_agent_core::ActiveTurnInput::FollowUp(
        AgentMessage::user_text("queued one"),
    ));
    steer_input.push(neo_agent_core::ActiveTurnInput::FollowUp(
        AgentMessage::user_text("queued two"),
    ));
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model())
            .with_queue_modes(QueueMode::All, QueueMode::OneAtATime),
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
        .expect("configured one-at-a-time follow-up run should succeed");

    assert_eq!(
        harness.requests().len(),
        3,
        "configured OneAtATime mode should keep each queued follow-up in its own turn"
    );
    let drained_counts = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::QueueDrained {
                kind: neo_agent_core::QueueKind::FollowUp,
                count,
            } => Some(*count),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        drained_counts,
        vec![1, 1],
        "configured OneAtATime mode should drain follow-ups FIFO one item at a time"
    );
    assert_eq!(steer_input.pending(), 0);
}

#[tokio::test]
async fn runtime_reclassifies_promoted_follow_up_as_steer_without_running_follow_up() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "first".to_owned(),
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
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "second".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
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

#[tokio::test]
async fn runtime_dequeues_follow_up_for_edit_without_running_it() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "first".to_owned(),
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
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "second".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let steer_input = neo_agent_core::SteerInputHandle::new();
    steer_input.push(neo_agent_core::ActiveTurnInput::DequeueFollowUpForEdit);
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
        .expect("dequeued follow-up run should succeed");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::QueueDrained { kind, count: 1 }
            if *kind == neo_agent_core::QueueKind::FollowUp
    )));
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::SteeringQueued { message }
            if message == &AgentMessage::user_text("queued follow")
    )));
    assert_eq!(
        harness.requests().len(),
        1,
        "dequeued follow-up should not run as an automatic follow-up turn"
    );
    assert_eq!(context.pending_follow_up_len(), 0);
    assert_eq!(steer_input.pending(), 0);
}
