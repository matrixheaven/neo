use neo_tui::primitive::strip_ansi;
use neo_tui::transcript::{TranscriptEntry, TranscriptPane};

fn plain_frame(transcript: &mut TranscriptPane, width: usize, height: usize) -> Vec<String> {
    transcript
        .render_frame(width, height)
        .expect("render frame")
        .iter()
        .map(|line| plain(line))
        .collect()
}
fn schedule_and_resume_retry(pane: &mut TranscriptPane, turn: u32) {
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryScheduled {
        turn,
        retry: 1,
        max_retries: 5,
        delay_ms: 12_000,
        error_code: "provider.transport_error".to_owned(),
        message: "transport error: connection reset".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryStarted {
        turn,
        retry: 1,
        max_retries: 5,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryResumed { turn, retry: 1 });
}
fn plain(line: &str) -> String {
    strip_ansi(line).trim_end().to_owned()
}

#[test]
fn retry_attempt_is_replaced_by_the_winning_attempt_in_the_document() {
    let mut pane = TranscriptPane::new(40, 8);
    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        phase: neo_ai::MessagePhase::Unknown,
        turn: 1,
        id: "attempt-1".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "thinking-1".to_owned(),
        kind: neo_ai::ThinkingKind::Unknown,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "failed reasoning".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
        turn: 1,
        signature: None,
        redacted: false,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "failed answer prefix\n\nfailed mutable tail".to_owned(),
    });

    let provisional = pane
        .render_visible_slice(40, 8)
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(provisional.contains("failed mutable tail"), "{provisional}");

    schedule_and_resume_retry(&mut pane, 1);
    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        phase: neo_ai::MessagePhase::Unknown,
        turn: 1,
        id: "attempt-2".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "winning answer".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageFinished {
        phase: neo_ai::MessagePhase::Unknown,
        turn: 1,
        id: "attempt-2".to_owned(),
        stop_reason: neo_agent_core::StopReason::EndTurn,
    });

    let finished = pane
        .render_visible_slice(40, 8)
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!finished.contains("failed reasoning"), "{finished}");
    assert!(!finished.contains("failed answer prefix"), "{finished}");
    assert_eq!(finished.matches("winning answer").count(), 1, "{finished}");
}

#[test]
fn retry_error_interrupts_connecting_status_and_keeps_terminal_error_visible() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryScheduled {
        turn: 1,
        retry: 1,
        max_retries: 5,
        delay_ms: 12_000,
        error_code: "provider.transport_error".to_owned(),
        message: "transport error: connection reset".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryStarted {
        turn: 1,
        retry: 1,
        max_retries: 5,
    });
    let retry_entry_id = pane.transcript().entry_ids()[0];

    pane.apply_agent_event(neo_agent_core::AgentEvent::Error {
        turn: 1,
        message: "terminal connection failure".to_owned(),
        code: None,
        retry_after: None,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: neo_agent_core::StopReason::Error,
    });

    assert_eq!(pane.transcript().entry_ids()[0], retry_entry_id);
    assert!(matches!(
        &pane.transcript().entries()[0],
        TranscriptEntry::Status { text, .. }
            if text == "Reconnect interrupted during attempt 1"
    ));
    assert!(
        pane.transcript()
            .entries()
            .iter()
            .all(|entry| !matches!(entry, TranscriptEntry::RetryStatus { .. }))
    );
    let rendered = plain_frame(&mut pane, 80, 20).join("\n");
    assert!(rendered.contains("Reconnect interrupted"), "{rendered}");
    assert!(
        rendered.contains("Error: terminal connection failure"),
        "{rendered}"
    );
    assert!(!rendered.contains("connecting"), "{rendered}");
}

#[test]
fn retry_exhaustion_suppresses_followup_error_card() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "partial".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryExhausted {
        turn: 1,
        retries_used: 5,
        error_code: "provider.transport_error".to_owned(),
        message: "transport error: connection reset".to_owned(),
    });
    let entry_count = pane.transcript().entries().len();
    pane.apply_agent_event(neo_agent_core::AgentEvent::Error {
        turn: 1,
        message: "transport error: connection reset".to_owned(),
        code: Some("provider.transport_error".to_owned()),
        retry_after: None,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::RunFinished {
        turn: 1,
        stop_reason: neo_agent_core::StopReason::Error,
    });

    assert_eq!(pane.transcript().entries().len(), entry_count);
    assert_eq!(
        pane.transcript()
            .entries()
            .iter()
            .filter(|entry| matches!(entry, TranscriptEntry::RetryStatus { .. }))
            .count(),
        1
    );
    let rendered = plain_frame(&mut pane, 80, 20).join("\n");
    assert!(!rendered.contains("runtime error"), "{rendered}");
}

#[test]
fn retry_reset_preserves_earlier_turn_live_entry() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "older-tool".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({ "path": "README.md" }),

        workflow_origin: None,
        output_ref: None,
    });
    let older_id = pane.transcript().entry_ids()[0];
    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        phase: neo_ai::MessagePhase::Unknown,
        turn: 2,
        id: "attempt-2".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 2,
        text: "discard current turn only".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryScheduled {
        turn: 2,
        retry: 1,
        max_retries: 5,
        delay_ms: 12_000,
        error_code: "provider.transport_error".to_owned(),
        message: "transport error: connection reset".to_owned(),
    });

    assert_eq!(pane.transcript().entry_ids()[0], older_id);
    assert!(matches!(
        pane.transcript().entries()[0],
        TranscriptEntry::ToolRun { .. }
    ));
    assert!(matches!(
        pane.transcript().entries()[1],
        TranscriptEntry::RetryStatus { .. }
    ));
}

#[test]
fn retry_status_mutates_original_position() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.push_user_message("question");
    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        phase: neo_ai::MessagePhase::Unknown,
        turn: 1,
        id: "attempt-1".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "discard me".to_owned(),
    });
    let original_id = pane.transcript().entry_ids()[1];

    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryScheduled {
        turn: 1,
        retry: 1,
        max_retries: 5,
        delay_ms: 12_000,
        error_code: "provider.transport_error".to_owned(),
        message: "error decoding response body".to_owned(),
    });
    assert_eq!(pane.transcript().entries().len(), 2);
    assert_eq!(pane.transcript().entry_ids()[1], original_id);
    assert!(matches!(
        pane.transcript().entries()[1],
        TranscriptEntry::RetryStatus { .. }
    ));

    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryStarted {
        turn: 1,
        retry: 1,
        max_retries: 5,
    });
    assert_eq!(pane.transcript().entry_ids()[1], original_id);

    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryResumed { turn: 1, retry: 1 });
    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        phase: neo_ai::MessagePhase::Unknown,
        turn: 1,
        id: "attempt-2".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "replacement".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetrySucceeded {
        turn: 1,
        retries_used: 1,
    });

    assert_eq!(pane.transcript().entries().len(), 2);
    assert_eq!(pane.transcript().entry_ids()[1], original_id);
    assert!(matches!(
        &pane.transcript().entries()[1],
        TranscriptEntry::AssistantMessage { content } if content == "replacement"
    ));

    let mut exhausted = TranscriptPane::new(80, 20);
    exhausted.push_user_message("question");
    exhausted.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 2,
        text: "first partial".to_owned(),
    });
    let original_id = exhausted.transcript().entry_ids()[1];
    exhausted.apply_agent_event(neo_agent_core::AgentEvent::RetryScheduled {
        turn: 2,
        retry: 1,
        max_retries: 1,
        delay_ms: 12_000,
        error_code: "provider.transport_error".to_owned(),
        message: "connection reset".to_owned(),
    });
    exhausted.apply_agent_event(neo_agent_core::AgentEvent::RetryStarted {
        turn: 2,
        retry: 1,
        max_retries: 1,
    });
    exhausted.apply_agent_event(neo_agent_core::AgentEvent::RetryResumed { turn: 2, retry: 1 });
    exhausted.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 2,
        text: "last partial".to_owned(),
    });
    exhausted.apply_agent_event(neo_agent_core::AgentEvent::RetryExhausted {
        turn: 2,
        retries_used: 1,
        error_code: "provider.transport_error".to_owned(),
        message: "connection reset".to_owned(),
    });

    assert_eq!(exhausted.transcript().entries().len(), 2);
    assert_eq!(exhausted.transcript().entry_ids()[1], original_id);
    assert!(matches!(
        &exhausted.transcript().entries()[1],
        TranscriptEntry::RetryStatus { data }
            if data.phase == neo_tui::transcript::entry::RetryPhase::Exhausted
                && data.message == "connection reset"
    ));
}

#[test]
fn retry_status_renders_fixed_waiting_connecting_and_exhausted_states() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryScheduled {
        turn: 1,
        retry: 1,
        max_retries: 5,
        delay_ms: 12_000,
        error_code: "provider.transport_error".to_owned(),
        message: "transport error: error decoding response body".to_owned(),
    });

    let waiting_frame_0 = plain_frame(&mut pane, 80, 20).join("\n");
    assert!(
        waiting_frame_0.contains("⠋ Reconnecting 1/5 · retry in 12s · esc interrupt"),
        "waiting retry status: {waiting_frame_0}"
    );
    assert_eq!(
        waiting_frame_0
            .matches("Network · error decoding response body")
            .count(),
        1,
        "waiting retry detail: {waiting_frame_0}"
    );
    assert!(!waiting_frame_0.contains("Network · transport error:"));
    pane.advance_animation_at_ms(80);
    let waiting_frame_1 = plain_frame(&mut pane, 80, 20).join("\n");
    assert!(
        waiting_frame_1.contains("⠙ Reconnecting 1/5 · retry in 12s · esc interrupt"),
        "waiting retry animation: {waiting_frame_1}"
    );

    let mut connecting_pane = TranscriptPane::new(80, 20);
    connecting_pane.apply_agent_event(neo_agent_core::AgentEvent::RetryScheduled {
        turn: 1,
        retry: 1,
        max_retries: 5,
        delay_ms: 12_000,
        error_code: "provider.transport_error".to_owned(),
        message: "transport error: error decoding response body".to_owned(),
    });
    connecting_pane.apply_agent_event(neo_agent_core::AgentEvent::RetryStarted {
        turn: 1,
        retry: 1,
        max_retries: 5,
    });
    let connecting_frame_0 = plain_frame(&mut connecting_pane, 80, 20).join("\n");
    assert!(
        connecting_frame_0.contains("⠋ Reconnecting 1/5 · connecting · esc interrupt"),
        "connecting retry status: {connecting_frame_0}"
    );
    connecting_pane.advance_animation_at_ms(80);
    let connecting_frame_1 = plain_frame(&mut connecting_pane, 80, 20).join("\n");
    assert!(
        connecting_frame_1.contains("⠙ Reconnecting 1/5 · connecting · esc interrupt"),
        "connecting retry animation: {connecting_frame_1}"
    );

    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryExhausted {
        turn: 1,
        retries_used: 5,
        error_code: "provider.transport_error".to_owned(),
        message: "error decoding response body".to_owned(),
    });
    let exhausted = plain_frame(&mut pane, 80, 20).join("\n");
    assert!(
        exhausted.contains("Reconnect failed after 5 retries"),
        "exhausted retry status: {exhausted}"
    );
    assert!(
        exhausted.contains("Network · error decoding response body"),
        "exhausted retry detail: {exhausted}"
    );

    for (turn, retries_used, expected) in [
        (3, 0, "Reconnect failed · retry disabled"),
        (4, 1, "Reconnect failed after 1 retry"),
    ] {
        let mut terminal = TranscriptPane::new(80, 20);
        terminal.apply_agent_event(neo_agent_core::AgentEvent::RetryExhausted {
            turn,
            retries_used,
            error_code: "provider.transport_error".to_owned(),
            message: String::new(),
        });
        let rendered = plain_frame(&mut terminal, 80, 20).join("\n");
        assert!(
            rendered.contains(expected),
            "terminal retry status: {rendered}"
        );
    }

    let mut high_attempt = TranscriptPane::new(80, 20);
    high_attempt.apply_agent_event(neo_agent_core::AgentEvent::RetryScheduled {
        turn: 2,
        retry: 99,
        max_retries: 100,
        delay_ms: 12_000,
        error_code: "provider.transport_error".to_owned(),
        message: "connection reset".to_owned(),
    });
    let waiting = plain_frame(&mut high_attempt, 80, 20).join("\n");
    high_attempt.apply_agent_event(neo_agent_core::AgentEvent::RetryStarted {
        turn: 2,
        retry: 99,
        max_retries: 100,
    });
    let connecting = plain_frame(&mut high_attempt, 80, 20).join("\n");
    assert!(waiting.contains("Reconnecting 99/100 · retry in 12s"));
    assert!(connecting.contains("Reconnecting 99/100 · connecting"));
    high_attempt.apply_agent_event(neo_agent_core::AgentEvent::RetrySucceeded {
        turn: 2,
        retries_used: 99,
    });
    assert!(
        high_attempt
            .transcript()
            .entries()
            .iter()
            .all(|entry| !matches!(entry, TranscriptEntry::RetryStatus { .. }))
    );
}

#[test]
fn retry_thinking_first_reuses_anchor_before_intervening_finalized_entry() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "failed answer".to_owned(),
    });
    let anchor_id = pane.transcript().entry_ids()[0];
    schedule_and_resume_retry(&mut pane, 1);
    pane.transcript_mut()
        .push(TranscriptEntry::status("intervening"));
    let intervening_id = pane.transcript().entry_ids()[1];

    pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "thinking-2".to_owned(),
        kind: neo_ai::ThinkingKind::Unknown,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "winning reasoning".to_owned(),
    });

    assert_eq!(pane.transcript().entries().len(), 2);
    assert_eq!(pane.transcript().entry_ids(), &[anchor_id, intervening_id]);
    assert!(matches!(
        &pane.transcript().entries()[0],
        TranscriptEntry::ThinkingBlock { parts, .. }
            if parts.len() == 1 && parts[0].text == "winning reasoning"
    ));
    assert!(matches!(
        &pane.transcript().entries()[1],
        TranscriptEntry::Status { text, .. } if text == "intervening"
    ));
}

#[test]
fn retry_tool_first_reuses_anchor_before_intervening_finalized_entry() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "failed answer".to_owned(),
    });
    let anchor_id = pane.transcript().entry_ids()[0];
    schedule_and_resume_retry(&mut pane, 1);
    pane.transcript_mut()
        .push(TranscriptEntry::status("intervening"));
    let intervening_id = pane.transcript().entry_ids()[1];

    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "tool-2".to_owned(),
        name: "Read".to_owned(),
    });

    assert_eq!(pane.transcript().entries().len(), 2);
    assert_eq!(pane.transcript().entry_ids(), &[anchor_id, intervening_id]);
    assert!(matches!(
        &pane.transcript().entries()[0],
        TranscriptEntry::ToolRun { component } if component.id() == "tool-2"
    ));
    assert!(matches!(
        &pane.transcript().entries()[1],
        TranscriptEntry::Status { text, .. } if text == "intervening"
    ));
}

#[test]
fn retry_wait_cancel_becomes_interrupted_terminal_status() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryScheduled {
        turn: 1,
        retry: 1,
        max_retries: 5,
        delay_ms: 12_000,
        error_code: "provider.transport_error".to_owned(),
        message: "transport error: connection reset".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: neo_agent_core::StopReason::Cancelled,
    });

    assert!(pane.transcript().entries().iter().all(|entry| !matches!(
        entry,
        TranscriptEntry::RetryStatus { data }
            if data.phase != neo_tui::transcript::entry::RetryPhase::Exhausted
    )));
    let rendered = plain_frame(&mut pane, 80, 20).join("\n");
    assert!(rendered.contains("Reconnect interrupted"), "{rendered}");
    assert!(!rendered.contains("Reconnect failed"), "{rendered}");
}
