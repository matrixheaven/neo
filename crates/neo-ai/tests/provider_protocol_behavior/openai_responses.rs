use std::collections::BTreeMap;

use futures::StreamExt;
use neo_ai::{
    AiError, AiStreamEvent, ApiKind, CacheRetention, ChatMessage, ContentPart, ImageData,
    MessagePhase, ModelClient, ReasoningEffort, ReasoningSelection, RequestMetadata,
    RequestOptions, StopReason, ThinkingKind, providers::openai::responses::OpenAiResponsesClient,
};
use serde_json::{Value, json};

use super::http_server::{
    MockServer, assistant_image_request, image_request, request, sse_response, status_response,
    truncated_sse_response,
};

#[tokio::test]
async fn openai_responses_client_posts_responses_payload_and_streams_events() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-1" } }),
        json!({ "type": "response.output_text.delta", "delta": "hi " }),
        json!({
            "type": "response.output_item.added",
            "item": { "type": "function_call", "id": "item-1", "call_id": "call-1", "name": "read_file" }
        }),
        json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "item-1",
            "delta": "{\"path\":"
        }),
        json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "item-1",
            "delta": "\"Cargo.toml\"}"
        }),
        json!({
            "type": "response.completed",
            "response": {
                "status": "completed",
                "usage": { "input_tokens": 9, "output_tokens": 4 }
            }
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
                id: "resp-1".to_owned(),
                phase: MessagePhase::Unknown,
            },
            AiStreamEvent::TextDelta {
                text: "hi ".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "call-1".to_owned(),
                name: "read_file".to_owned(),
            },
            AiStreamEvent::ToolCallArgsDelta {
                id: "call-1".to_owned(),
                json_fragment: "{\"path\":".to_owned(),
            },
            AiStreamEvent::ToolCallArgsDelta {
                id: "call-1".to_owned(),
                json_fragment: "\"Cargo.toml\"}".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "call-1".to_owned(),
                raw_arguments: r#"{"path":"Cargo.toml"}"#.to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::ToolUse,
                usage: Some(neo_ai::TokenUsage {
                    input_tokens: 9,
                    output_tokens: 4,
                    input_cache_read_tokens: 0,
                    input_cache_write_tokens: 0,
                }),
                phase: MessagePhase::Unknown,
            },
        ]
    );

    let sent = server.requests().pop().unwrap();
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.path, "/responses");
    assert_eq!(
        sent.headers.get("authorization").unwrap(),
        "Bearer test-key"
    );
    assert_eq!(sent.body["model"], "model-test");
    assert_eq!(sent.body["stream"], true);
    assert_eq!(sent.body["max_output_tokens"], 64);
    assert_eq!(sent.body["tools"][0]["name"], "read_file");
    assert_eq!(sent.body["input"][0]["role"], "user");
}

#[tokio::test]
async fn openai_responses_output_item_done_overrides_argument_preview() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-1" } }),
        json!({
            "type": "response.output_item.added",
            "item": { "id": "item-1", "type": "function_call", "call_id": "call-1", "name": "read_file" }
        }),
        json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "item-1",
            "delta": "{\"path\":\"Car"
        }),
        json!({
            "type": "response.output_item.done",
            "item": { "id": "item-1", "type": "function_call", "call_id": "call-1", "name": "read_file", "arguments": "{\"path\":\"Cargo.toml\"}" }
        }),
        json!({
            "type": "response.completed",
            "response": { "usage": { "input_tokens": 1, "output_tokens": 1 } }
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

    assert!(events.contains(&AiStreamEvent::ToolCallEnd {
        id: "call-1".to_owned(),
        raw_arguments: r#"{"path":"Cargo.toml"}"#.to_owned(),
    }));
}

#[tokio::test]
async fn openai_responses_output_item_done_without_added_is_tool_use() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-1" } }),
        json!({
            "type": "response.output_item.done",
            "item": {
                "id": "item-1",
                "type": "function_call",
                "call_id": "call-1",
                "name": "read_file",
                "arguments": "{\"path\":\"Cargo.toml\"}"
            }
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

    assert!(events.contains(&AiStreamEvent::ToolCallEnd {
        id: "call-1".to_owned(),
        raw_arguments: r#"{"path":"Cargo.toml"}"#.to_owned(),
    }));
    assert!(matches!(
        events.last(),
        Some(AiStreamEvent::MessageEnd {
            stop_reason: StopReason::ToolUse,
            ..
        })
    ));
}

#[tokio::test]
async fn openai_responses_client_posts_typed_options_cache_and_metadata() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-options" } }),
        json!({
            "type": "response.completed",
            "response": { "status": "completed" }
        }),
    ])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");
    let mut headers = BTreeMap::new();
    headers.insert("x-neo-trace".to_owned(), "trace-1".to_owned());
    let mut request = request(ApiKind::OpenAiResponse);
    request.options = RequestOptions {
        temperature: Some(0.4),
        max_tokens: Some(128),
        headers,
        reasoning: ReasoningSelection::Effort {
            effort: ReasoningEffort::medium(),
        },
        cache: CacheRetention::Long,
        session_id: Some("session-1".to_owned()),
        metadata: RequestMetadata::from_pairs([("user_id", "u-1"), ("trace_id", "trace-1")]),
        ..RequestOptions::default()
    };

    client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.path, "/responses");
    assert_eq!(
        sent.headers.get("authorization").unwrap(),
        "Bearer test-key"
    );
    assert_eq!(sent.headers.get("x-neo-trace").unwrap(), "trace-1");
    assert_eq!(
        sent.headers.get("x-client-request-id").unwrap(),
        "session-1"
    );
    assert_eq!(sent.body["model"], "model-test");
    assert_eq!(sent.body["stream"], true);
    assert_eq!(sent.body["temperature"], 0.4);
    assert_eq!(sent.body["max_output_tokens"], 128);
    assert_eq!(sent.body["reasoning"]["effort"], "medium");
    assert_eq!(sent.body["reasoning"]["summary"], "auto");
    assert_eq!(
        sent.body["metadata"],
        json!({ "trace_id": "trace-1", "user_id": "u-1" })
    );
    assert_eq!(sent.body["prompt_cache_key"], "session-1");
    assert_eq!(sent.body["prompt_cache_retention"], "24h");
    assert_eq!(sent.body["tools"][0]["name"], "read_file");
}

#[tokio::test]
async fn openai_responses_client_opens_provider_response_once() {
    let server = MockServer::start(vec![status_response(500)]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");
    let request = request(ApiKind::OpenAiResponse);

    let error = client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert_eq!(server.requests().len(), 1);
    assert_eq!(error.code(), "provider.server_error");
}

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

#[tokio::test]
async fn openai_responses_client_serializes_image_parts() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-image" } }),
        json!({
            "type": "response.completed",
            "response": { "status": "completed" }
        }),
    ])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    client
        .stream_chat(image_request(
            ApiKind::OpenAiResponse,
            ImageData::Url("https://example.test/cat.png".to_owned()),
        ))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    assert_eq!(sent.body["input"][0]["content"][0]["type"], "input_text");
    assert_eq!(sent.body["input"][0]["content"][0]["text"], "describe this");
    assert_eq!(sent.body["input"][0]["content"][1]["type"], "input_image");
    assert_eq!(
        sent.body["input"][0]["content"][1]["image_url"],
        "https://example.test/cat.png"
    );
}

#[tokio::test]
async fn openai_responses_client_serializes_base64_image_parts_as_data_urls() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-base64-image" } }),
        json!({
            "type": "response.completed",
            "response": { "status": "completed" }
        }),
    ])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    client
        .stream_chat(image_request(
            ApiKind::OpenAiResponse,
            ImageData::Base64("iVBORw0KGgo=".to_owned()),
        ))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    assert_eq!(sent.body["input"][0]["content"][1]["type"], "input_image");
    assert_eq!(
        sent.body["input"][0]["content"][1]["image_url"],
        "data:image/png;base64,iVBORw0KGgo="
    );
}

#[tokio::test]
async fn openai_responses_client_rejects_assistant_image_parts_without_posting() {
    let server = MockServer::start(Vec::new());
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let err = client
        .stream_chat(assistant_image_request(
            ApiKind::OpenAiResponse,
            ImageData::Base64("iVBORw0KGgo=".to_owned()),
        ))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert!(
        err.to_string()
            .contains("OpenAI Responses image content is only supported")
    );
    assert_eq!(server.requests().len(), 0);
}

#[tokio::test]
async fn openai_responses_client_returns_protocol_error_for_failed_streams() {
    let body = format!(
        "data: {}\n\ndata: {}\n\n",
        json!({ "type": "response.created", "response": { "id": "resp-failed" } }),
        json!({
            "type": "response.failed",
            "response": { "status": "failed" }
        })
    );
    let server = MockServer::start(vec![truncated_sse_response(&body)]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await;
    assert_eq!(
        events.iter().filter(|event| event.is_err()).count(),
        1,
        "classified provider failure must emit exactly one error: {events:?}"
    );
    let error = events
        .into_iter()
        .find_map(Result::err)
        .expect("classified provider failure must emit an error");

    assert_eq!(error.code(), "provider.protocol_error");
    assert!(error.to_string().contains("status failed"));
}

#[tokio::test]
async fn openai_responses_stream_server_error_is_retryable() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "type": "response.failed",
        "response": {
            "status": "failed",
            "error": { "code": "server_error", "message": "upstream busy" }
        }
    })])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let error = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert!(matches!(
        error,
        AiError::Server {
            status: 500,
            retry_after: None,
            message
        } if message == "upstream busy"
    ));
}

#[tokio::test]
async fn openai_responses_stream_rate_limit_error_is_retryable() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "type": "response.failed",
        "response": {
            "status": "failed",
            "error": { "code": "rate_limit_exceeded", "message": "slow down" }
        }
    })])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let error = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert!(matches!(
        error,
        AiError::RateLimit {
            retry_after: None,
            message
        } if message == "slow down"
    ));
}

#[tokio::test]
async fn openai_responses_top_level_stream_rate_limit_error_is_retryable() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "type": "error",
        "code": "rate_limit_exceeded",
        "message": "slow down"
    })])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let error = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert!(matches!(
        error,
        AiError::RateLimit {
            retry_after: None,
            message
        } if message == "slow down"
    ));
}

#[tokio::test]
async fn openai_responses_client_returns_protocol_error_for_incomplete_streams() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-incomplete" } }),
        json!({
            "type": "response.incomplete",
            "response": { "status": "incomplete" }
        }),
    ])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let error = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert_eq!(error.code(), "provider.protocol_error");
    assert!(error.to_string().contains("status incomplete"));
}

#[tokio::test]
async fn openai_responses_body_error_respects_terminal_state() {
    let terminal = format!(
        "data: {}\n\ndata: {}\n\n",
        json!({ "type": "response.created", "response": { "id": "resp-terminal" } }),
        json!({
            "type": "response.completed",
            "response": { "status": "completed" }
        })
    );
    let incomplete = format!(
        "data: {}\n\n",
        json!({ "type": "response.created", "response": { "id": "resp-incomplete" } })
    );
    let server = MockServer::start(vec![
        truncated_sse_response(&terminal),
        truncated_sse_response(&incomplete),
    ]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let completed = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("terminal marker must survive the body error");
    assert!(matches!(
        completed.last(),
        Some(AiStreamEvent::MessageEnd { .. })
    ));

    let incomplete_events = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await;
    assert_eq!(
        incomplete_events
            .iter()
            .filter(|event| event.is_err())
            .count(),
        1,
        "incomplete stream must emit exactly one error: {incomplete_events:?}"
    );
    let error = incomplete_events
        .into_iter()
        .find_map(Result::err)
        .expect("incomplete body must remain an error");
    assert!(matches!(
        error,
        AiError::Transport { message } if !message.starts_with("transport error:")
    ));
}

#[tokio::test]
async fn openai_responses_numeric_429_and_503_errors_are_retryable() {
    let server = MockServer::start(vec![
        sse_response(&[json!({
            "type": "error",
            "code": 429,
            "message": "slow down"
        })]),
        sse_response(&[json!({
            "type": "error",
            "status": 503,
            "message": "unavailable"
        })]),
    ]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let rate_limit = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();
    let unavailable = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert!(matches!(rate_limit, AiError::RateLimit { .. }));
    assert!(matches!(unavailable, AiError::Server { status: 503, .. }));
}

#[tokio::test]
async fn openai_responses_nested_rate_limit_and_overload_errors_are_retryable() {
    let server = MockServer::start(vec![
        sse_response(&[json!({
            "type": "error",
            "error": { "type": "rate_limit_error", "message": "slow down" }
        })]),
        sse_response(&[json!({
            "type": "error",
            "error": { "type": "overloaded_error", "message": "busy" }
        })]),
    ]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let rate_limit = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();
    let overloaded = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert!(matches!(rate_limit, AiError::RateLimit { .. }));
    assert!(matches!(overloaded, AiError::Server { status: 529, .. }));
}

#[tokio::test]
async fn openai_responses_unknown_error_is_protocol() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "type": "error",
        "error": { "type": "mystery_error", "message": "unknown failure" }
    })])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let error = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert!(matches!(error, AiError::Protocol { .. }));
}
