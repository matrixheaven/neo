use super::*;

#[test]
fn retry_activity_fold_matches_live_snapshot_at_capacity() {
    let runtime = MultiAgentRuntime::new();
    let child = runtime.start_foreground_delegate_for_test("retry at activity cap");
    let started_at = Instant::now();
    let mut events = Vec::new();
    let mut record = |event| {
        let _ = runtime.apply_child_event(&child.id, started_at, &event);
        events.push(event);
    };

    record(AgentEvent::TextDelta {
        turn: 1,
        text: "prior answer".to_owned(),
    });
    record(AgentEvent::MessageAppended {
        message: AgentMessage::assistant(
            vec![Content::text("prior answer")],
            Vec::new(),
            StopReason::ToolUse,
        ),
    });
    for index in 0..24 {
        record(AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: format!("tool-{index}"),
            name: "Read".to_owned(),
            arguments: serde_json::json!({"path": format!("file-{index}")}),
            workflow_origin: None,
            output_ref: None,
        });
    }
    record(AgentEvent::ThinkingDelta {
        turn: 1,
        text: "failed reasoning".to_owned(),
    });
    record(AgentEvent::RetryScheduled {
        turn: 1,
        retry: 1,
        max_retries: 5,
        delay_ms: 500,
        error_code: "provider.transport_error".to_owned(),
        message: "transport error: body closed".to_owned(),
    });
    record(AgentEvent::RetryStarted {
        turn: 1,
        retry: 1,
        max_retries: 5,
    });

    let live = runtime.snapshot(&child.id).expect("live child snapshot");
    let summarized = summarize_child_activity(&events);

    assert_eq!(live.activity, summarized);
    assert_eq!(
        latest_text_activity(&summarized, false).as_deref(),
        Some("Reconnecting 1/5")
    );
}

#[test]
fn retry_exhaustion_fold_matches_live_and_preserves_error() {
    let runtime = MultiAgentRuntime::new();
    let child = runtime.start_foreground_delegate_for_test("retry exhaustion");
    let started_at = Instant::now();
    let mut events = Vec::new();
    let mut record = |event| {
        let _ = runtime.apply_child_event(&child.id, started_at, &event);
        events.push(event);
    };

    record(AgentEvent::TextDelta {
        turn: 1,
        text: "failed partial one".to_owned(),
    });
    record(AgentEvent::TokenUsage {
        turn: 1,
        usage: AgentTokenUsage {
            input_tokens: 13,
            output_tokens: 5,
            input_cache_read_tokens: 9,
            input_cache_write_tokens: 2,
        },
    });
    record(AgentEvent::RetryScheduled {
        turn: 1,
        retry: 1,
        max_retries: 1,
        delay_ms: 500,
        error_code: "provider.transport_error".to_owned(),
        message: "transport error: body closed".to_owned(),
    });
    record(AgentEvent::RetryStarted {
        turn: 1,
        retry: 1,
        max_retries: 1,
    });
    record(AgentEvent::RetryResumed { turn: 1, retry: 1 });
    record(AgentEvent::ThinkingDelta {
        turn: 1,
        text: "failed reasoning two".to_owned(),
    });
    record(AgentEvent::TextDelta {
        turn: 1,
        text: "failed partial two".to_owned(),
    });
    record(AgentEvent::RetryExhausted {
        turn: 1,
        retries_used: 1,
        error_code: "provider.transport_error".to_owned(),
        message: "transport error: connection reset".to_owned(),
    });
    record(AgentEvent::Error {
        turn: 1,
        message: "transport error: connection reset".to_owned(),
        code: Some("provider.transport_error".to_owned()),
        retry_after: None,
    });

    let live = runtime.snapshot(&child.id).expect("live child snapshot");
    let terminal = summarize_child_events(&events, Duration::ZERO);

    assert_eq!(live.activity, terminal.activity);
    assert_eq!(live.latest_text, terminal.latest_text);
    assert_eq!(live.token_count, 18);
    assert_eq!(live.input_token_count, 13);
    assert_eq!(terminal.token_count, 18);
    assert_eq!(terminal.input_token_count, 13);
    assert_eq!(
        terminal.latest_text.as_deref(),
        Some("transport error: connection reset")
    );
    assert_eq!(terminal.summary, "transport error: connection reset");
    assert!(terminal.activity.iter().all(|entry| !matches!(
        &entry.kind,
        AgentActivityKind::Text { text, .. }
            if text.contains("failed") || text.starts_with("Reconnecting ")
    )));
}

#[test]
fn failed_child_run_preserves_live_usage() {
    let runtime = MultiAgentRuntime::new();
    let child = runtime.start_foreground_delegate_for_test("preserve failed usage");
    let started_at = Instant::now();
    let _ = runtime.apply_child_event(
        &child.id,
        started_at,
        &AgentEvent::TextDelta {
            turn: 1,
            text: "partial answer".to_owned(),
        },
    );
    let _ = runtime.apply_child_event(
        &child.id,
        started_at,
        &AgentEvent::TokenUsage {
            turn: 1,
            usage: AgentTokenUsage {
                input_tokens: 13,
                output_tokens: 5,
                input_cache_read_tokens: 9,
                input_cache_write_tokens: 2,
            },
        },
    );

    let failed = runtime.finish_child_run(&child, started_at, Err("writer failed".to_owned()));

    assert_eq!(failed.snapshot.state, AgentLifecycleState::Failed);
    assert_eq!(failed.snapshot.token_count, 18);
    assert_eq!(failed.snapshot.input_token_count, 13);
    assert_eq!(failed.snapshot.cache_read_token_count, 9);
    assert_eq!(failed.snapshot.cache_write_token_count, 2);
    assert_eq!(
        failed.snapshot.latest_text.as_deref(),
        Some("partial answer")
    );
    assert!(!failed.snapshot.activity.is_empty());

    let pristine = runtime.start_foreground_delegate_for_test("prestart failure");
    let prestart_failed =
        runtime.finish_child_run(&pristine, Instant::now(), Err("setup failed".to_owned()));
    assert_eq!(
        (
            prestart_failed.snapshot.tool_count,
            prestart_failed.snapshot.token_count,
            prestart_failed.snapshot.input_token_count,
            prestart_failed.snapshot.cache_read_token_count,
            prestart_failed.snapshot.cache_write_token_count,
        ),
        (0, 0, 0, 0, 0)
    );
}

#[test]
fn cancelled_retry_backoff_fold_matches_live() {
    let runtime = MultiAgentRuntime::new();
    let child = runtime.start_foreground_delegate_for_test("cancelled retry backoff");
    let started_at = Instant::now();
    let mut events = Vec::new();
    let mut record = |event| {
        let _ = runtime.apply_child_event(&child.id, started_at, &event);
        events.push(event);
    };

    record(AgentEvent::TextDelta {
        turn: 1,
        text: "prior answer".to_owned(),
    });
    record(AgentEvent::MessageAppended {
        message: AgentMessage::assistant(
            vec![Content::text("prior answer")],
            Vec::new(),
            StopReason::ToolUse,
        ),
    });
    record(AgentEvent::TextDelta {
        turn: 1,
        text: "failed partial".to_owned(),
    });
    record(AgentEvent::RetryScheduled {
        turn: 1,
        retry: 1,
        max_retries: 5,
        delay_ms: 500,
        error_code: "provider.transport_error".to_owned(),
        message: "transport error: body closed".to_owned(),
    });
    record(AgentEvent::RetryStarted {
        turn: 1,
        retry: 1,
        max_retries: 5,
    });
    record(AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: StopReason::Cancelled,
    });
    record(AgentEvent::RunFinished {
        turn: 1,
        stop_reason: StopReason::Cancelled,
    });

    let live = runtime.snapshot(&child.id).expect("live child snapshot");
    let terminal = summarize_child_events(&events, Duration::ZERO);

    assert_eq!(live.activity, terminal.activity);
    assert_eq!(live.latest_text, terminal.latest_text);
    assert_eq!(terminal.latest_text.as_deref(), Some("prior answer"));
    assert_eq!(
        latest_text_activity(&terminal.activity, false).as_deref(),
        Some("prior answer")
    );
    assert!(terminal.activity.iter().all(|entry| !matches!(
        &entry.kind,
        AgentActivityKind::Text { text, .. }
            if text == "failed partial" || text.starts_with("Reconnecting ")
    )));
}
