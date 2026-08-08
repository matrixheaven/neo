//! `OpenAI` responses behavior (moved from `responses.rs`).

use super::*;

#[test]
fn oversized_sse_frame_is_rejected() {
    let mut parser = IncrementalSse::default();
    let data = "x".repeat(crate::providers::common::sse::SseFramer::MAX_FRAME_BYTES + 1);
    let body = format!("data: {data}\n\n");
    let error = parser
        .push_chunk(body.as_bytes())
        .into_iter()
        .find_map(Result::err)
        .expect("should emit an error");
    assert!(matches!(error, AiError::Protocol { .. }));
    assert!(!error.is_retryable());
}

#[test]
fn response_format_maps_json_schema_for_responses_api() {
    use crate::{ApiKind, ModelCapabilities, ModelSpec, ProviderId, ResponseFormat};

    let format = ResponseFormat {
        name: "result".to_owned(),
        schema: json!({"type": "object", "properties": {"n": {"type": "integer"}}, "required": ["n"], "additionalProperties": false}),
        strict: true,
    };
    let request = ChatRequest {
        model: ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: "gpt-test".to_owned(),
            api: ApiKind::OpenAiResponse,
            capabilities: ModelCapabilities::chat(),
        },
        messages: vec![ChatMessage::User {
            content: vec![ContentPart::Text {
                text: "hi".to_owned(),
            }],
        }],
        tools: vec![],
        options: crate::RequestOptions {
            response_format: Some(format.clone()),
            ..crate::RequestOptions::default()
        },
    };
    let body = request_body(&request).expect("body");
    assert_eq!(
        body["text"]["format"],
        format.to_openai_responses_text_format()
    );
}

fn message_item(phase: Option<&str>) -> Value {
    let mut item = json!({"id": "message-1", "type": "message"});
    if let Some(phase) = phase {
        item["phase"] = Value::String(phase.to_owned());
    }
    item
}

fn parse_message_item_events(
    added_phase: Option<&str>,
    done_phase: Option<&str>,
) -> Vec<AiStreamEvent> {
    let mut parser = ParseState::default();
    parser
        .ingest(&json!({
            "type": "response.created",
            "response": {"id": "response-1"}
        }))
        .expect("response.created should parse");
    parser
        .ingest(&json!({
            "type": "response.output_item.added",
            "item": message_item(added_phase)
        }))
        .expect("output item added should parse");
    parser
        .ingest(&json!({
            "type": "response.output_text.delta",
            "delta": "text"
        }))
        .expect("output text delta should parse");
    parser
        .ingest(&json!({
            "type": "response.output_item.done",
            "item": message_item(done_phase)
        }))
        .expect("output item done should parse");
    parser
        .ingest(&json!({
            "type": "response.completed",
            "response": {"status": "completed"}
        }))
        .expect("response.completed should parse");
    parser.finish_events().expect("stream should finish")
}

#[test]
fn response_message_item_phase_maps_explicit_values_without_duplicate_start() {
    for (wire_phase, phase) in [
        ("commentary", MessagePhase::Commentary),
        ("final_answer", MessagePhase::FinalAnswer),
    ] {
        let events = parse_message_item_events(Some(wire_phase), Some(wire_phase));
        assert_eq!(
            events,
            vec![
                AiStreamEvent::MessageStart {
                    id: "response-1".to_owned(),
                    phase,
                },
                AiStreamEvent::TextDelta {
                    text: "text".to_owned(),
                },
                AiStreamEvent::MessageEnd {
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                    phase,
                },
            ]
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AiStreamEvent::MessageStart { .. }))
                .count(),
            1
        );
    }
}

#[test]
fn response_message_item_done_phase_promotes_buffered_text_start() {
    let events = parse_message_item_events(None, Some("final_answer"));
    assert_eq!(
        events,
        vec![
            AiStreamEvent::MessageStart {
                id: "response-1".to_owned(),
                phase: MessagePhase::FinalAnswer,
            },
            AiStreamEvent::TextDelta {
                text: "text".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
                phase: MessagePhase::FinalAnswer,
            },
        ]
    );
}

fn ingest_buffered_reasoning_tool_text(parser: &mut ParseState) {
    for value in [
        json!({
            "type": "response.created",
            "response": {"id": "response-1"}
        }),
        json!({
            "type": "response.reasoning_summary_part.added",
            "item_id": "reasoning-1",
            "summary_index": 0
        }),
        json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": "reasoning-1",
            "summary_index": 0,
            "delta": "think"
        }),
        json!({
            "type": "response.reasoning_summary_part.done",
            "item_id": "reasoning-1",
            "summary_index": 0,
            "part": {"text": "think"},
            "item": {"id": "reasoning-1", "type": "reasoning"}
        }),
        json!({
            "type": "response.output_item.added",
            "item": {
                "id": "function-1",
                "type": "function_call",
                "call_id": "call-1",
                "name": "lookup"
            }
        }),
        json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "function-1",
            "delta": "{\"query\":\"neo\"}"
        }),
        json!({
            "type": "response.output_item.done",
            "item": {
                "id": "function-1",
                "type": "function_call",
                "call_id": "call-1",
                "name": "lookup",
                "arguments": "{\"query\":\"neo\"}"
            }
        }),
        json!({
            "type": "response.output_item.added",
            "item": message_item(None)
        }),
        json!({
            "type": "response.output_text.delta",
            "delta": "answer"
        }),
    ] {
        parser
            .ingest(&value)
            .expect("buffered response event should parse");
    }
}

fn event_kinds(events: &[AiStreamEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match event {
            AiStreamEvent::MessageStart { .. } => "message_start",
            AiStreamEvent::ThinkingStart { .. } => "thinking_start",
            AiStreamEvent::ThinkingDelta { .. } => "thinking_delta",
            AiStreamEvent::ThinkingEnd { .. } => "thinking_end",
            AiStreamEvent::TextDelta { .. } => "text_delta",
            AiStreamEvent::ToolCallStart { .. } => "tool_call_start",
            AiStreamEvent::ToolCallArgsDelta { .. } => "tool_call_args_delta",
            AiStreamEvent::ToolCallEnd { .. } => "tool_call_end",
            AiStreamEvent::MessageEnd { .. } => "message_end",
        })
        .collect()
}

#[test]
fn response_message_item_done_phase_promotes_buffered_reasoning_tool_text_in_order() {
    let mut parser = ParseState::default();
    ingest_buffered_reasoning_tool_text(&mut parser);
    parser
        .ingest(&json!({
            "type": "response.output_item.done",
            "item": message_item(Some("final_answer"))
        }))
        .expect("message item done should parse");
    parser
        .ingest(&json!({
            "type": "response.completed",
            "response": {"status": "completed"}
        }))
        .expect("response.completed should parse");

    let events = parser.finish_events().expect("stream should finish");
    assert_eq!(
        event_kinds(&events),
        vec![
            "message_start",
            "thinking_start",
            "thinking_delta",
            "thinking_end",
            "tool_call_start",
            "tool_call_args_delta",
            "tool_call_end",
            "text_delta",
            "message_end",
        ]
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AiStreamEvent::MessageStart { .. }))
            .count(),
        1
    );
    assert!(matches!(
        events.first(),
        Some(AiStreamEvent::MessageStart {
            phase: MessagePhase::FinalAnswer,
            ..
        })
    ));
    assert!(matches!(
        events.last(),
        Some(AiStreamEvent::MessageEnd {
            phase: MessagePhase::FinalAnswer,
            ..
        })
    ));
}

#[test]
fn finish_events_is_idempotent() {
    let mut parser = ParseState::default();
    parser
        .ingest(&json!({
            "type": "response.output_text.delta",
            "delta": "text"
        }))
        .expect("output text delta should parse");

    let first = parser.finish_events().expect("first finish should succeed");
    let second = parser
        .finish_events()
        .expect("second finish should succeed");

    assert_eq!(
        first
            .iter()
            .filter(|event| matches!(event, AiStreamEvent::MessageEnd { .. }))
            .count(),
        1
    );
    assert!(second.is_empty());
}

#[test]
fn buffered_terminal_events_fall_back_to_unknown_phase() {
    let mut parser = ParseState::default();
    ingest_buffered_reasoning_tool_text(&mut parser);

    let events = parser.finish_events().expect("stream should finish");
    assert_eq!(
        event_kinds(&events),
        vec![
            "message_start",
            "thinking_start",
            "thinking_delta",
            "thinking_end",
            "tool_call_start",
            "tool_call_args_delta",
            "tool_call_end",
            "text_delta",
            "message_end",
        ]
    );
    let starts = events
        .iter()
        .filter_map(|event| match event {
            AiStreamEvent::MessageStart { phase, .. } => Some(*phase),
            _ => None,
        })
        .collect::<Vec<_>>();
    let ends = events
        .iter()
        .filter_map(|event| match event {
            AiStreamEvent::MessageEnd { phase, .. } => Some(*phase),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(starts, vec![MessagePhase::Unknown]);
    assert_eq!(ends, vec![MessagePhase::Unknown]);
}

#[test]
fn response_message_item_missing_or_unknown_phase_stays_unknown() {
    for done_phase in [None, Some("draft")] {
        let events = parse_message_item_events(None, done_phase);
        assert_eq!(
            events,
            vec![
                AiStreamEvent::MessageStart {
                    id: "response-1".to_owned(),
                    phase: MessagePhase::Unknown,
                },
                AiStreamEvent::TextDelta {
                    text: "text".to_owned(),
                },
                AiStreamEvent::MessageEnd {
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                    phase: MessagePhase::Unknown,
                },
            ]
        );
    }

    let event: AiStreamEvent = serde_json::from_value(json!({
        "MessageStart": {"id": "historical"}
    }))
    .expect("missing phase should deserialize");
    assert_eq!(
        event,
        AiStreamEvent::MessageStart {
            id: "historical".to_owned(),
            phase: MessagePhase::Unknown,
        }
    );
}

#[test]
fn historical_message_end_without_phase_defaults_to_unknown() {
    let event: AiStreamEvent = serde_json::from_value(json!({
        "MessageEnd": {
            "stop_reason": "EndTurn",
            "usage": null
        }
    }))
    .expect("missing phase should deserialize");
    assert_eq!(
        event,
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::EndTurn,
            usage: None,
            phase: MessagePhase::Unknown,
        }
    );
}
