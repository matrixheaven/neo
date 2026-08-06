use super::fake_harness::DelayedHarness;
use super::fake_harness::DelayedStep;
use super::fake_harness::collect_turn_events;
use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, CompactionSettings, Content,
    StopReason, harness::FakeHarness,
};
use neo_ai::{AiError, AiStreamEvent, MessagePhase};
use std::time::Duration;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn retry_lifecycle_survives_context_overflow_recovery() {
    let harness = FakeHarness::from_result_turns([
        vec![Err(AiError::Transport {
            message: "connection reset".to_owned(),
        })],
        vec![Err(AiError::ContextOverflow {
            message: "too many tokens".to_owned(),
        })],
        vec![
            Ok(AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "summary".to_owned(),
            }),
            Ok(AiStreamEvent::TextDelta {
                text: "summary".to_owned(),
            }),
            Ok(AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            }),
        ],
        vec![
            Ok(AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "recovered".to_owned(),
            }),
            Ok(AiStreamEvent::TextDelta {
                text: "recovered".to_owned(),
            }),
            Ok(AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            }),
        ],
    ]);
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("history"));
    context.append_message(AgentMessage::assistant(
        [Content::text("old answer")],
        Vec::new(),
        StopReason::EndTurn,
    ));
    let mut config = AgentConfig::for_model(harness.model())
        .with_system_prompt("system ".repeat(4_000))
        .with_compaction(CompactionSettings::new(usize::MAX, 1));
    config.max_retries = 1;
    config.model.capabilities.max_context_tokens = Some(200_000);

    let events = collect_turn_events(
        &harness,
        config,
        &mut context,
        AgentMessage::user_text("continue"),
    )
    .await;

    let requests = harness.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(
        serde_json::to_value(&requests[0]).expect("serialize initial request"),
        serde_json::to_value(&requests[1]).expect("serialize ordinary retry")
    );
    let lifecycle = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::RetryScheduled { retry: 1, .. } => Some("retry_scheduled"),
            AgentEvent::RetryStarted { retry: 1, .. } => Some("retry_started"),
            AgentEvent::CompactionApplied { .. } => Some("compaction_applied"),
            AgentEvent::RetryResumed { retry: 1, .. } => Some("retry_resumed"),
            AgentEvent::RetrySucceeded {
                retries_used: 1, ..
            } => Some("retry_succeeded"),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        lifecycle,
        vec![
            "retry_scheduled",
            "retry_started",
            "compaction_applied",
            "retry_resumed",
            "retry_succeeded",
        ]
    );
}

#[tokio::test]
async fn stream_first_event_timeout_retries_same_request() {
    let harness = DelayedHarness::from_turns([
        vec![DelayedStep::Delay(Duration::from_secs(2))],
        vec![
            DelayedStep::Event(AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "retry".to_owned(),
            }),
            DelayedStep::Event(AiStreamEvent::TextDelta {
                text: "complete".to_owned(),
            }),
            DelayedStep::Event(AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            }),
        ],
    ]);
    let mut config = AgentConfig::for_model(harness.model());
    config.first_event_timeout_secs = 1;
    config.stream_idle_timeout_secs = 0;
    config.max_retries = 1;
    let runtime = AgentRuntime::new(config, harness.client());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("retry silence"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("retry should succeed");

    let requests = harness.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        serde_json::to_value(&requests[0]).expect("serialize first request"),
        serde_json::to_value(&requests[1]).expect("serialize retry request")
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::RetryScheduled {
            retry: 1,
            error_code,
            message,
            ..
        } if error_code == "provider.transport_error"
            && message.contains("first model stream event")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::RetrySucceeded {
            retries_used: 1,
            ..
        }
    )));
}

#[tokio::test]
async fn stream_idle_timeout_retries_and_discards_partial_attempt() {
    let harness = DelayedHarness::from_turns([
        vec![
            DelayedStep::Event(AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "discarded".to_owned(),
            }),
            DelayedStep::Event(AiStreamEvent::TextDelta {
                text: "discarded partial".to_owned(),
            }),
            DelayedStep::Delay(Duration::from_secs(2)),
        ],
        vec![
            DelayedStep::Event(AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "winning".to_owned(),
            }),
            DelayedStep::Event(AiStreamEvent::TextDelta {
                text: "winning answer".to_owned(),
            }),
            DelayedStep::Event(AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            }),
        ],
    ]);
    let mut config = AgentConfig::for_model(harness.model());
    config.first_event_timeout_secs = 0;
    config.stream_idle_timeout_secs = 1;
    config.max_retries = 1;
    let runtime = AgentRuntime::new(config, harness.client());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("retry idle stream"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("retry should succeed");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::RetryScheduled {
            error_code,
            message,
            ..
        } if error_code == "provider.transport_error"
            && message.contains("model stream idle for 1s")
    )));
    let appended = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MessageAppended {
                message: message @ AgentMessage::Assistant { .. },
            } => Some(message.text()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(appended, ["winning answer"]);
    assert!(!context.messages().iter().any(|message| {
        matches!(message, AgentMessage::Assistant { .. })
            && message.text().contains("discarded partial")
    }));
    let requests = harness.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        serde_json::to_value(&requests[0]).expect("serialize first request"),
        serde_json::to_value(&requests[1]).expect("serialize retry request")
    );
}

#[tokio::test]
async fn stream_timeout_zero_waits_until_cancelled() {
    let harness = DelayedHarness::new(vec![DelayedStep::Delay(Duration::from_secs(5))]);
    let mut config = AgentConfig::for_model(harness.model());
    config.first_event_timeout_secs = 0;
    config.stream_idle_timeout_secs = 0;
    config.max_retries = 0;
    let runtime = AgentRuntime::new(config, harness.client());
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    let mut stream = runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text("cancel silent stream"),
        cancel.clone(),
    );
    let mut events = Vec::new();

    loop {
        let event = timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("turn should start promptly")
            .expect("turn stream should remain open")
            .expect("turn event should be ok");
        let turn_started = matches!(event, AgentEvent::TurnStarted { .. });
        events.push(event);
        if turn_started {
            break;
        }
    }
    assert!(
        timeout(Duration::from_millis(50), stream.next())
            .await
            .is_err(),
        "zero timeouts must leave the pending model stream silent"
    );

    cancel.cancel();
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("silent stream cancellation should not stall")
    {
        events.push(event.expect("cancelled stream should remain in-band"));
    }
    drop(stream);

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
        AgentEvent::RetryScheduled { .. }
            | AgentEvent::RetryStarted { .. }
            | AgentEvent::RetryExhausted { .. }
    )));
}

#[tokio::test]
async fn stream_retries_transport_error() {
    let harness = FakeHarness::from_result_turns([
        vec![Err(AiError::Transport {
            message: "eof".into(),
        })],
        vec![
            Ok(AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "b".into(),
            }),
            Ok(AiStreamEvent::TextDelta {
                text: "complete".into(),
            }),
            Ok(AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            }),
        ],
    ]);
    let mut config = AgentConfig::for_model(harness.model());
    config.max_retries = 1;
    let mut context = AgentContext::new();

    let events = collect_turn_events(
        &harness,
        config,
        &mut context,
        AgentMessage::user_text("retry"),
    )
    .await;

    let requests = harness.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        serde_json::to_value(&requests[0]).expect("serialize first request"),
        serde_json::to_value(&requests[1]).expect("serialize replayed request")
    );

    let lifecycle = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TurnStarted { .. } => Some("turn_started"),
            AgentEvent::RetryScheduled { .. } => Some("retry_scheduled"),
            AgentEvent::RetryStarted { .. } => Some("retry_started"),
            AgentEvent::RetryResumed { .. } => Some("retry_resumed"),
            AgentEvent::RetrySucceeded { .. } => Some("retry_succeeded"),
            AgentEvent::TurnFinished { .. } => Some("turn_finished"),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        lifecycle,
        [
            "turn_started",
            "retry_scheduled",
            "retry_started",
            "retry_resumed",
            "retry_succeeded",
            "turn_finished",
        ]
    );
    let resumed = events
        .iter()
        .position(|event| matches!(event, AgentEvent::RetryResumed { .. }))
        .expect("retry should resume on its first valid event");
    assert!(matches!(
        events.get(resumed + 1),
        Some(AgentEvent::MessageStarted { id, .. }) if id == "b"
    ));

    let scheduled = events
        .iter()
        .find(|event| matches!(event, AgentEvent::RetryScheduled { .. }))
        .expect("retry should be scheduled");
    assert!(matches!(
        scheduled,
        AgentEvent::RetryScheduled {
            turn: 1,
            retry: 1,
            max_retries: 1,
            delay_ms: 500..=625,
            error_code,
            message,
        } if error_code == "provider.transport_error" && message == "transport error: eof"
    ));
}

#[tokio::test]
async fn retry_does_not_append_failed_attempt() {
    let harness = FakeHarness::from_result_turns([
        vec![
            Ok(AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "a".into(),
            }),
            Ok(AiStreamEvent::TextDelta {
                text: "partial".into(),
            }),
        ],
        vec![
            Ok(AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "b".into(),
            }),
            Ok(AiStreamEvent::TextDelta {
                text: "complete".into(),
            }),
            Ok(AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            }),
        ],
    ]);
    let mut config = AgentConfig::for_model(harness.model());
    config.max_retries = 1;
    let mut context = AgentContext::new();

    let events = collect_turn_events(
        &harness,
        config,
        &mut context,
        AgentMessage::user_text("retry partial"),
    )
    .await;

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::TextDelta { text, .. } if text == "partial"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::RetryScheduled {
            error_code,
            message,
            ..
        } if error_code == "provider.transport_error"
            && message == "transport error: model stream ended before MessageEnd"
    )));
    let appended = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MessageAppended {
                message: message @ AgentMessage::Assistant { .. },
            } => Some(message.text()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(appended, ["complete"]);
    assert!(
        !context
            .messages()
            .iter()
            .any(|message| matches!(message, AgentMessage::Assistant { .. })
                && message.text().contains("partial"))
    );
}

#[tokio::test]
async fn retry_budget_zero_emits_exhausted_error() {
    let harness = FakeHarness::from_result_turns([vec![Err(AiError::Transport {
        message: "provider failed".into(),
    })]]);
    let mut config = AgentConfig::for_model(harness.model());
    config.max_retries = 0;
    let mut context = AgentContext::new();

    let events = collect_turn_events(
        &harness,
        config,
        &mut context,
        AgentMessage::user_text("fail"),
    )
    .await;

    assert!(events.contains(&AgentEvent::Error {
        turn: 1,
        message: "transport error: provider failed".to_owned(),
        code: Some("provider.transport_error".to_owned()),
        retry_after: None,
    }));
    assert!(events.contains(&AgentEvent::RetryExhausted {
        turn: 1,
        retries_used: 0,
        error_code: "provider.transport_error".to_owned(),
        message: "transport error: provider failed".to_owned(),
    }));
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::RetryScheduled { .. } | AgentEvent::RetryStarted { .. }
    )));
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
}

#[tokio::test]
async fn retry_exhaustion_reports_final_error() {
    let harness = FakeHarness::from_result_turns([
        vec![Err(AiError::RateLimit {
            message: "busy".into(),
            retry_after: Some(Duration::ZERO),
        })],
        vec![Err(AiError::Server {
            status: 503,
            message: "still busy".into(),
            retry_after: Some(Duration::ZERO),
        })],
    ]);
    let mut config = AgentConfig::for_model(harness.model());
    config.max_retries = 1;
    let mut context = AgentContext::new();

    let events = collect_turn_events(
        &harness,
        config,
        &mut context,
        AgentMessage::user_text("retry once"),
    )
    .await;

    assert!(events.contains(&AgentEvent::RetryExhausted {
        turn: 1,
        retries_used: 1,
        error_code: "provider.server_error".to_owned(),
        message: "server error (503): still busy".to_owned(),
    }));
    assert!(events.contains(&AgentEvent::Error {
        turn: 1,
        message: "server error (503): still busy".to_owned(),
        code: Some("provider.server_error".to_owned()),
        retry_after: Some(0),
    }));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::RetryResumed { .. }))
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::RetrySucceeded { .. }))
    );
    assert_eq!(harness.requests().len(), 2);
}

#[tokio::test]
async fn retry_does_not_retry_protocol_failure() {
    let harness = FakeHarness::from_result_turns([vec![Err(AiError::Protocol {
        message: "invalid frame".into(),
    })]]);
    let mut config = AgentConfig::for_model(harness.model());
    config.max_retries = 5;
    let mut context = AgentContext::new();

    let events = collect_turn_events(
        &harness,
        config,
        &mut context,
        AgentMessage::user_text("broken protocol"),
    )
    .await;

    assert!(events.contains(&AgentEvent::Error {
        turn: 1,
        message: "protocol error: invalid frame".to_owned(),
        code: Some("provider.protocol_error".to_owned()),
        retry_after: None,
    }));
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::RetryScheduled { .. }
            | AgentEvent::RetryStarted { .. }
            | AgentEvent::RetryResumed { .. }
            | AgentEvent::RetrySucceeded { .. }
            | AgentEvent::RetryExhausted { .. }
    )));
    assert_eq!(harness.requests().len(), 1);
}

#[tokio::test]
async fn retry_backoff_is_cancellable() {
    let harness = FakeHarness::from_result_turns([
        vec![Err(AiError::Transport {
            message: "eof".into(),
        })],
        vec![Ok(AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        })],
    ]);
    let mut config = AgentConfig::for_model(harness.model());
    config.max_retries = 1;
    let runtime = AgentRuntime::new(config, harness.client());
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    let mut stream = runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text("cancel retry"),
        cancel.clone(),
    );
    let mut events = Vec::new();

    while let Some(event) = timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("retry lifecycle should not stall")
    {
        let event = event.expect("cancelled retry should remain in-band");
        let scheduled = matches!(event, AgentEvent::RetryScheduled { .. });
        events.push(event);
        if scheduled {
            cancel.cancel();
        }
    }
    drop(stream);

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
        AgentEvent::RetryStarted { .. } | AgentEvent::RetryResumed { .. }
    )));
    assert_eq!(harness.requests().len(), 1);
}
