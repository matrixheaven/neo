//! History projection: JSONL-equivalent retry filtering and delta merging,
//! and the release/rebuild round-trip of an idle session's projection.

use neo_agent_core::{Content, StopReason};

use super::state_fixtures::{test_state, user_message};
use super::*;

#[test]
fn snapshot_projection_drops_failed_attempt_text_with_retry_semantics() {
    let relay = Relay::new("test_stream");
    let state = test_state(&relay, "session_1", None);
    {
        let mut guard = state.lock().expect("state lock");
        guard.ingest_event(AgentEvent::MessageStarted {
            turn: 1,
            id: "message_1".to_owned(),
            phase: neo_ai::MessagePhase::Unknown,
        });
        guard.ingest_event(AgentEvent::TextDelta {
            turn: 1,
            text: "failed attempt ".to_owned(),
        });
        guard.ingest_event(AgentEvent::TextDelta {
            turn: 1,
            text: "kept out of the projection".to_owned(),
        });
        guard.ingest_event(AgentEvent::RetryScheduled {
            turn: 1,
            retry: 1,
            max_retries: 2,
            delay_ms: 1,
            error_code: "provider.transport_error".to_owned(),
            message: "retry".to_owned(),
        });
        guard.ingest_event(AgentEvent::RetryResumed { turn: 1, retry: 1 });
        guard.ingest_event(AgentEvent::TextDelta {
            turn: 1,
            text: "final text".to_owned(),
        });
        guard.ingest_event(AgentEvent::MessageFinished {
            turn: 1,
            id: "message_1".to_owned(),
            stop_reason: StopReason::EndTurn,
            phase: neo_ai::MessagePhase::Unknown,
        });
        guard.ingest_event(AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                vec![Content::text("final text")],
                Vec::new(),
                StopReason::EndTurn,
            ),
        });
    }
    let guard = state.lock().expect("state lock");
    let events: Vec<&AgentEvent> = guard.history.iter().map(|entry| &entry.event).collect();
    assert!(
        events
            .iter()
            .all(|event| !format!("{event:?}").contains("failed attempt")),
        "failed attempt text must never reach the projection"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::RetryScheduled { .. })),
        "retry markers remain visible"
    );
    assert!(
        events.iter().any(
            |event| matches!(event, AgentEvent::TextDelta { text, .. } if text == "final text")
        ),
        "winning attempt text is present"
    );
    let sequences: Vec<u64> = guard.history.iter().map(|entry| entry.sequence).collect();
    assert_eq!(
        sequences,
        (1..=sequences.len() as u64).collect::<Vec<u64>>(),
        "published sequences stay contiguous"
    );
}

#[test]
fn consecutive_deltas_merge_like_the_jsonl_persistence_view() {
    let relay = Relay::new("test_stream");
    let state = test_state(&relay, "session_1", None);
    {
        let mut guard = state.lock().expect("state lock");
        guard.ingest_event(AgentEvent::TextDelta {
            turn: 1,
            text: "hello ".to_owned(),
        });
        guard.ingest_event(AgentEvent::TextDelta {
            turn: 1,
            text: "world".to_owned(),
        });
        guard.ingest_event(AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                vec![Content::text("hello world")],
                Vec::new(),
                StopReason::EndTurn,
            ),
        });
    }
    let guard = state.lock().expect("state lock");
    let deltas: Vec<&str> = guard
        .history
        .iter()
        .filter_map(|entry| match &entry.event {
            AgentEvent::TextDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, vec!["hello world"], "deltas merge like JSONL");
}

#[test]
fn released_projection_rebuilds_contiguous_history_ending_at_last_sequence() {
    let relay = Relay::new("test_stream");
    let state = test_state(&relay, "session_1", Some("turn_1"));
    let reference = ToolOutputRef {
        agent_id: "main".to_owned(),
        task_id: "task_1".to_owned(),
        byte_len: 12,
        line_count: 3,
        complete: false,
    };
    // The canonical stream keeps the structured output reference; only
    // the web projection strips it.
    let original_events = vec![
        AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "tool_1".to_owned(),
            name: "bash".to_owned(),
            arguments: serde_json::json!({}),
            workflow_origin: None,
            output_ref: Some(reference.clone()),
        },
        user_message("hello"),
        AgentEvent::TextDelta {
            turn: 1,
            text: "world".to_owned(),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                vec![Content::text("world")],
                Vec::new(),
                StopReason::EndTurn,
            ),
        },
    ];
    {
        let mut guard = state.lock().expect("state lock");
        for event in &original_events {
            guard.ingest_event(event.clone());
        }
    }
    let before: Vec<WebUiHistoryEntry> = {
        let guard = state.lock().expect("state lock");
        assert_eq!(guard.last_sequence, 4);
        guard.history.clone()
    };
    assert_eq!(before.len(), 4);
    let encoded = encode_output_ref(&reference).expect("encode");
    {
        let mut guard = state.lock().expect("state lock");
        guard.release_projection();
        assert!(guard.projection_released());
        assert!(guard.history.is_empty(), "history dropped at release");
        assert!(
            guard.output_refs.is_empty(),
            "output references dropped at release"
        );
        assert_eq!(
            guard.last_sequence, 4,
            "last known sequence is retained for the rebuild"
        );
    }
    // Rebuild from the canonical stream (the JSONL contents), exactly as
    // the host does on the next access.
    {
        let mut guard = state.lock().expect("state lock");
        guard.rebuild_projection(original_events);
        assert!(!guard.projection_released());
    }
    let guard = state.lock().expect("state lock");
    assert_eq!(guard.history.len(), before.len(), "history is complete");
    for (entry, expected) in guard.history.iter().zip(&before) {
        assert_eq!(
            entry.event, expected.event,
            "replayed content matches the pre-release projection"
        );
        assert_eq!(entry.sequence, expected.sequence);
        assert_eq!(entry.output, expected.output);
    }
    assert!(
        guard.output_refs.contains(&encoded),
        "output references rebuilt from canonical events"
    );
    let sequences: Vec<u64> = guard.history.iter().map(|entry| entry.sequence).collect();
    assert_eq!(
        sequences,
        (1..=4).collect::<Vec<u64>>(),
        "rebuilt sequences stay contiguous and end at the last known sequence"
    );
}
