//! `OpenAI` Responses reasoning selection and reasoning-summary stream behavior.

use futures::StreamExt;
use neo_ai::{
    AiStreamEvent, ApiKind, ChatMessage, ContentPart, MessagePhase, ModelClient, ReasoningEffort,
    ReasoningSelection, StopReason, ThinkingKind,
    providers::openai::responses::OpenAiResponsesClient,
};
use serde_json::{Value, json};

use super::http_server::{MockServer, request, sse_response};

#[tokio::test]
async fn openai_responses_client_serializes_reasoning_selection_with_encrypted_handoff() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-reasoning" } }),
        json!({
            "type": "response.completed",
            "response": { "status": "completed" }
        }),
    ])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");
    let mut request = request(ApiKind::OpenAiResponse);
    request.options.reasoning = ReasoningSelection::Effort {
        effort: ReasoningEffort::try_from("UltraMax").expect("custom effort"),
    };

    client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    assert_eq!(sent.body["reasoning"]["effort"], "UltraMax");
    assert_eq!(sent.body["reasoning"]["summary"], "auto");
    assert_eq!(sent.body["include"], json!(["reasoning.encrypted_content"]));
}

#[tokio::test]
async fn openai_responses_client_rejects_budget_reasoning_selection_without_posting() {
    let server = MockServer::start(Vec::new());
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");
    let mut request = request(ApiKind::OpenAiResponse);
    request.options.reasoning = ReasoningSelection::BudgetTokens {
        budget_tokens: 8_192,
    };

    let err = client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    let message = err.to_string();
    assert!(
        message.contains("does not support budget reasoning selections"),
        "{message}"
    );
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn openai_responses_client_replays_signed_reasoning_items() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-replay" } }),
        json!({
            "type": "response.completed",
            "response": { "status": "completed" }
        }),
    ])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");
    let mut request = request(ApiKind::OpenAiResponse);
    request.messages.insert(
        1,
        ChatMessage::Assistant {
            content: vec![ContentPart::Thinking {
                text: "stored reasoning".to_owned(),
                signature: Some(
                    json!({
                        "type": "reasoning",
                        "id": "rs_1",
                        "summary": [{ "type": "summary_text", "text": "stored reasoning" }],
                        "encrypted_content": "opaque-reasoning"
                    })
                    .to_string(),
                ),
                redacted: false,
            }],
            tool_calls: Vec::new(),
        },
    );

    client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    assert_eq!(sent.body["input"][1]["type"], "reasoning");
    assert_eq!(sent.body["input"][1]["id"], "rs_1");
    assert_eq!(
        sent.body["input"][1]["encrypted_content"],
        "opaque-reasoning"
    );
}

#[tokio::test]
async fn openai_responses_client_can_disable_reasoning_replay() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-replay-off" } }),
        json!({
            "type": "response.completed",
            "response": { "status": "completed" }
        }),
    ])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");
    let mut request = request(ApiKind::OpenAiResponse);
    request.options.replay_reasoning = false;
    request.messages.insert(
        1,
        ChatMessage::Assistant {
            content: vec![
                ContentPart::Thinking {
                    text: "stored reasoning".to_owned(),
                    signature: Some(
                        json!({
                            "type": "reasoning",
                            "id": "rs_1",
                            "summary": [{ "type": "summary_text", "text": "stored reasoning" }],
                            "encrypted_content": "opaque-reasoning"
                        })
                        .to_string(),
                    ),
                    redacted: false,
                },
                ContentPart::Text {
                    text: "visible answer".to_owned(),
                },
            ],
            tool_calls: Vec::new(),
        },
    );

    client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    assert_eq!(sent.body["input"][1]["type"], "message");
    assert_eq!(
        sent.body["input"][1]["content"][0]["text"],
        "visible answer"
    );
    assert!(
        sent.body["input"]
            .as_array()
            .expect("input array")
            .iter()
            .all(|item| item["type"] != "reasoning"),
        "reasoning replay should be fully suppressed when replay_reasoning is false"
    );
}

#[tokio::test]
async fn openai_responses_client_persists_reasoning_item_signature_from_stream() {
    let reasoning_item = json!({
        "type": "reasoning",
        "id": "rs_1",
        "summary": [{ "type": "summary_text", "text": "stored reasoning" }],
        "encrypted_content": "opaque-reasoning"
    });
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-thinking-item" } }),
        json!({
            "type": "response.reasoning_summary_part.added",
            "item_id": "rs_1",
            "summary_index": 0,
            "part": { "type": "summary_text", "text": "" }
        }),
        json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": "rs_1",
            "summary_index": 0,
            "delta": "stored reasoning"
        }),
        json!({
            "type": "response.output_item.done",
            "item": reasoning_item.clone()
        }),
        json!({
            "type": "response.completed",
            "response": { "status": "completed" }
        }),
    ])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let Some(AiStreamEvent::ThinkingEnd {
        signature: Some(signature),
        redacted: false,
    }) = events
        .iter()
        .find(|event| matches!(event, AiStreamEvent::ThinkingEnd { .. }))
    else {
        panic!("expected signed thinking end event, got {events:?}");
    };
    assert_eq!(
        serde_json::from_str::<Value>(signature).expect("signature JSON"),
        reasoning_item
    );
}

#[tokio::test]
async fn openai_responses_client_streams_reasoning_summary_events() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-thinking" } }),
        json!({
            "type": "response.reasoning_summary_part.added",
            "item_id": "rs_1",
            "summary_index": 0,
            "part": { "type": "summary_text", "text": "" }
        }),
        json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": "rs_1",
            "summary_index": 0,
            "delta": "Checked "
        }),
        json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": "rs_1",
            "summary_index": 0,
            "delta": "the plan."
        }),
        json!({
            "type": "response.reasoning_summary_part.done",
            "item_id": "rs_1",
            "summary_index": 0,
            "part": { "type": "summary_text", "text": "Checked the plan." }
        }),
        json!({ "type": "response.output_text.delta", "delta": "final" }),
        json!({
            "type": "response.completed",
            "response": { "status": "completed" }
        }),
    ])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(
        events,
        vec![
            AiStreamEvent::MessageStart {
                id: "resp-thinking".to_owned(),
                phase: MessagePhase::Unknown,
            },
            AiStreamEvent::ThinkingStart {
                id: "rs_1:summary:0".to_owned(),
                kind: ThinkingKind::Summary,
            },
            AiStreamEvent::ThinkingDelta {
                text: "Checked ".to_owned()
            },
            AiStreamEvent::ThinkingDelta {
                text: "the plan.".to_owned()
            },
            AiStreamEvent::ThinkingEnd {
                signature: None,
                redacted: false,
            },
            AiStreamEvent::TextDelta {
                text: "final".to_owned()
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
                phase: MessagePhase::Unknown,
            },
        ]
    );
}

#[tokio::test]
async fn openai_responses_client_buffers_pre_phase_reasoning_and_tool_events_until_message_phase() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-pre-phase" } }),
        json!({
            "type": "response.reasoning_summary_part.added",
            "item_id": "rs_pre_phase",
            "summary_index": 0,
            "part": { "type": "summary_text", "text": "" }
        }),
        json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": "rs_pre_phase",
            "summary_index": 0,
            "delta": "Checked the tool."
        }),
        json!({
            "type": "response.reasoning_summary_part.done",
            "item_id": "rs_pre_phase",
            "summary_index": 0,
            "part": { "type": "summary_text", "text": "Checked the tool." }
        }),
        json!({
            "type": "response.output_item.added",
            "item": {
                "type": "function_call",
                "id": "item-pre-phase",
                "call_id": "call-pre-phase",
                "name": "read_file"
            }
        }),
        json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "item-pre-phase",
            "delta": "{\"path\":\"Cargo.toml\"}"
        }),
        json!({
            "type": "response.output_item.done",
            "item": {
                "type": "function_call",
                "id": "item-pre-phase",
                "call_id": "call-pre-phase",
                "name": "read_file",
                "arguments": "{\"path\":\"Cargo.toml\"}"
            }
        }),
        json!({
            "type": "response.output_item.added",
            "item": { "type": "message", "id": "message-1", "phase": "commentary" }
        }),
        json!({ "type": "response.output_text.delta", "delta": "answer" }),
        json!({
            "type": "response.completed",
            "response": { "status": "completed" }
        }),
    ])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(
        events,
        vec![
            AiStreamEvent::MessageStart {
                id: "resp-pre-phase".to_owned(),
                phase: MessagePhase::Commentary,
            },
            AiStreamEvent::ThinkingStart {
                id: "rs_pre_phase:summary:0".to_owned(),
                kind: ThinkingKind::Summary,
            },
            AiStreamEvent::ThinkingDelta {
                text: "Checked the tool.".to_owned(),
            },
            AiStreamEvent::ThinkingEnd {
                signature: None,
                redacted: false,
            },
            AiStreamEvent::ToolCallStart {
                id: "call-pre-phase".to_owned(),
                name: "read_file".to_owned(),
            },
            AiStreamEvent::ToolCallArgsDelta {
                id: "call-pre-phase".to_owned(),
                json_fragment: "{\"path\":\"Cargo.toml\"}".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "call-pre-phase".to_owned(),
                raw_arguments: r#"{"path":"Cargo.toml"}"#.to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::ToolUse,
                usage: None,
                phase: MessagePhase::Commentary,
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

#[tokio::test]
async fn openai_responses_client_streams_reasoning_summary_text_done_without_deltas() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-thinking-done" } }),
        json!({
            "type": "response.reasoning_summary_part.added",
            "item_id": "rs_done",
            "summary_index": 0,
            "part": { "type": "summary_text", "text": "" }
        }),
        json!({
            "type": "response.reasoning_summary_text.done",
            "item_id": "rs_done",
            "summary_index": 0,
            "text": "Read the inputs."
        }),
        json!({
            "type": "response.reasoning_summary_part.done",
            "item_id": "rs_done",
            "summary_index": 0,
            "part": { "type": "summary_text", "text": "Read the inputs." }
        }),
        json!({
            "type": "response.completed",
            "response": { "status": "completed" }
        }),
    ])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(
        events,
        vec![
            AiStreamEvent::MessageStart {
                id: "resp-thinking-done".to_owned(),
                phase: MessagePhase::Unknown,
            },
            AiStreamEvent::ThinkingStart {
                id: "rs_done:summary:0".to_owned(),
                kind: ThinkingKind::Summary,
            },
            AiStreamEvent::ThinkingDelta {
                text: "Read the inputs.".to_owned()
            },
            AiStreamEvent::ThinkingEnd {
                signature: None,
                redacted: false,
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
                phase: MessagePhase::Unknown,
            },
        ]
    );
}

#[tokio::test]
async fn openai_responses_client_serializes_interleaved_reasoning_summaries_by_start_order() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-interleaved-thinking" } }),
        json!({
            "type": "response.reasoning_summary_part.added",
            "item_id": "rs_1",
            "summary_index": 0,
            "part": { "type": "summary_text", "text": "" }
        }),
        json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": "rs_1",
            "summary_index": 0,
            "delta": "First "
        }),
        json!({
            "type": "response.reasoning_summary_part.added",
            "item_id": "rs_2",
            "summary_index": 1,
            "part": { "type": "summary_text", "text": "" }
        }),
        json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": "rs_2",
            "summary_index": 1,
            "delta": "Second"
        }),
        json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": "rs_1",
            "summary_index": 0,
            "delta": "thought."
        }),
        json!({
            "type": "response.reasoning_summary_text.done",
            "item_id": "rs_2",
            "summary_index": 1,
            "text": "Second thought."
        }),
        json!({
            "type": "response.reasoning_summary_part.done",
            "item_id": "rs_2",
            "summary_index": 1,
            "part": { "type": "summary_text", "text": "Second thought." }
        }),
        json!({
            "type": "response.reasoning_summary_part.done",
            "item_id": "rs_1",
            "summary_index": 0,
            "part": { "type": "summary_text", "text": "First thought." }
        }),
        json!({
            "type": "response.completed",
            "response": { "status": "completed" }
        }),
    ])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(
        events,
        vec![
            AiStreamEvent::MessageStart {
                id: "resp-interleaved-thinking".to_owned(),
                phase: MessagePhase::Unknown,
            },
            AiStreamEvent::ThinkingStart {
                id: "rs_1:summary:0".to_owned(),
                kind: ThinkingKind::Summary,
            },
            AiStreamEvent::ThinkingDelta {
                text: "First ".to_owned()
            },
            AiStreamEvent::ThinkingDelta {
                text: "thought.".to_owned()
            },
            AiStreamEvent::ThinkingEnd {
                signature: None,
                redacted: false,
            },
            AiStreamEvent::ThinkingStart {
                id: "rs_2:summary:1".to_owned(),
                kind: ThinkingKind::Summary,
            },
            AiStreamEvent::ThinkingDelta {
                text: "Second thought.".to_owned()
            },
            AiStreamEvent::ThinkingEnd {
                signature: None,
                redacted: false,
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
                phase: MessagePhase::Unknown,
            },
        ]
    );
}

#[tokio::test]
async fn openai_responses_client_keeps_reasoning_summaries_with_shared_item_id_separate() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-shared-thinking-item" } }),
        json!({
            "type": "response.reasoning_summary_part.added",
            "item_id": "rs_item",
            "summary_index": 0,
            "part": { "type": "summary_text", "text": "" }
        }),
        json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": "rs_item",
            "summary_index": 0,
            "delta": "First"
        }),
        json!({
            "type": "response.reasoning_summary_part.added",
            "item_id": "rs_item",
            "summary_index": 1,
            "part": { "type": "summary_text", "text": "" }
        }),
        json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": "rs_item",
            "summary_index": 1,
            "delta": "Second"
        }),
        json!({
            "type": "response.reasoning_summary_part.done",
            "item_id": "rs_item",
            "summary_index": 0,
            "part": { "type": "summary_text", "text": "First" }
        }),
        json!({
            "type": "response.reasoning_summary_text.done",
            "item_id": "rs_item",
            "summary_index": 1,
            "text": "Second"
        }),
        json!({
            "type": "response.reasoning_summary_part.done",
            "item_id": "rs_item",
            "summary_index": 1,
            "part": { "type": "summary_text", "text": "Second" }
        }),
        json!({
            "type": "response.completed",
            "response": { "status": "completed" }
        }),
    ])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(
        events,
        vec![
            AiStreamEvent::MessageStart {
                id: "resp-shared-thinking-item".to_owned(),
                phase: MessagePhase::Unknown,
            },
            AiStreamEvent::ThinkingStart {
                id: "rs_item:summary:0".to_owned(),
                kind: ThinkingKind::Summary,
            },
            AiStreamEvent::ThinkingDelta {
                text: "First".to_owned()
            },
            AiStreamEvent::ThinkingEnd {
                signature: None,
                redacted: false,
            },
            AiStreamEvent::ThinkingStart {
                id: "rs_item:summary:1".to_owned(),
                kind: ThinkingKind::Summary,
            },
            AiStreamEvent::ThinkingDelta {
                text: "Second".to_owned()
            },
            AiStreamEvent::ThinkingEnd {
                signature: None,
                redacted: false,
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
                phase: MessagePhase::Unknown,
            },
        ]
    );
}

#[tokio::test]
async fn openai_responses_client_keeps_reasoning_summaries_with_shared_output_item_indexes_separate()
 {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-shared-output-index" } }),
        json!({
            "type": "response.reasoning_summary_part.added",
            "output_index": 0,
            "item_id": "rs_item",
            "summary_index": 0,
            "part": { "type": "summary_text", "text": "" }
        }),
        json!({
            "type": "response.reasoning_summary_text.delta",
            "output_index": 0,
            "item_id": "rs_item",
            "summary_index": 0,
            "delta": "Output zero"
        }),
        json!({
            "type": "response.reasoning_summary_part.added",
            "output_index": 1,
            "item_id": "rs_item",
            "summary_index": 0,
            "part": { "type": "summary_text", "text": "" }
        }),
        json!({
            "type": "response.reasoning_summary_text.delta",
            "output_index": 1,
            "item_id": "rs_item",
            "summary_index": 0,
            "delta": "Output one"
        }),
        json!({
            "type": "response.reasoning_summary_part.done",
            "output_index": 0,
            "item_id": "rs_item",
            "summary_index": 0,
            "part": { "type": "summary_text", "text": "Output zero" }
        }),
        json!({
            "type": "response.reasoning_summary_part.done",
            "output_index": 1,
            "item_id": "rs_item",
            "summary_index": 0,
            "part": { "type": "summary_text", "text": "Output one" }
        }),
        json!({
            "type": "response.completed",
            "response": { "status": "completed" }
        }),
    ])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(
        events,
        vec![
            AiStreamEvent::MessageStart {
                id: "resp-shared-output-index".to_owned(),
                phase: MessagePhase::Unknown,
            },
            AiStreamEvent::ThinkingStart {
                id: "rs_item:output:0:summary:0".to_owned(),
                kind: ThinkingKind::Summary,
            },
            AiStreamEvent::ThinkingDelta {
                text: "Output zero".to_owned()
            },
            AiStreamEvent::ThinkingEnd {
                signature: None,
                redacted: false,
            },
            AiStreamEvent::ThinkingStart {
                id: "rs_item:output:1:summary:0".to_owned(),
                kind: ThinkingKind::Summary,
            },
            AiStreamEvent::ThinkingDelta {
                text: "Output one".to_owned()
            },
            AiStreamEvent::ThinkingEnd {
                signature: None,
                redacted: false,
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
                phase: MessagePhase::Unknown,
            },
        ]
    );
}

#[tokio::test]
async fn openai_responses_client_keeps_streamed_summary_when_done_text_is_non_prefix() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-thinking-correction" } }),
        json!({
            "type": "response.reasoning_summary_part.added",
            "item_id": "rs_corrected",
            "summary_index": 0,
            "part": { "type": "summary_text", "text": "" }
        }),
        json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": "rs_corrected",
            "summary_index": 0,
            "delta": "streamed summary"
        }),
        json!({
            "type": "response.reasoning_summary_text.done",
            "item_id": "rs_corrected",
            "summary_index": 0,
            "text": "corrected summary"
        }),
        json!({
            "type": "response.reasoning_summary_part.done",
            "item_id": "rs_corrected",
            "summary_index": 0,
            "part": { "type": "summary_text", "text": "corrected summary" }
        }),
        json!({
            "type": "response.completed",
            "response": { "status": "completed" }
        }),
    ])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(
        events,
        vec![
            AiStreamEvent::MessageStart {
                id: "resp-thinking-correction".to_owned(),
                phase: MessagePhase::Unknown,
            },
            AiStreamEvent::ThinkingStart {
                id: "rs_corrected:summary:0".to_owned(),
                kind: ThinkingKind::Summary,
            },
            AiStreamEvent::ThinkingDelta {
                text: "streamed summary".to_owned()
            },
            AiStreamEvent::ThinkingEnd {
                signature: None,
                redacted: false,
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
                phase: MessagePhase::Unknown,
            },
        ],
        "Neo's provider-neutral thinking stream is append-only; final text corrections need a future replacement event contract"
    );
}
