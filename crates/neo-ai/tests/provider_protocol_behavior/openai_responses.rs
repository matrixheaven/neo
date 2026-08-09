//! `OpenAI` Responses request serialization, stream normalization, and image parts.

use std::collections::BTreeMap;

use futures::StreamExt;
use neo_ai::{
    AiStreamEvent, ApiKind, CacheRetention, ImageData, MessagePhase, ModelClient, ReasoningEffort,
    ReasoningSelection, RequestMetadata, RequestOptions, StopReason,
    providers::openai::responses::OpenAiResponsesClient,
};
use serde_json::json;

use super::http_server::{
    MockServer, assistant_image_request, image_request, request, sse_response, status_response,
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
                "output": [{
                    "type": "message",
                    "content": [{ "type": "output_text", "text": "hi " }]
                }],
                "usage": {
                    "input_tokens": 9,
                    "output_tokens": 4,
                    "input_tokens_details": {
                        "cached_tokens": 4,
                        "cache_write_tokens": 2
                    }
                }
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
                    input_cache_read_tokens: 4,
                    input_cache_write_tokens: 2,
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
async fn openai_responses_client_uses_final_text_when_provider_omits_deltas() {
    let cases = [
        (
            "output text done",
            json!({ "type": "response.output_text.done", "text": "Generated title" }),
        ),
        (
            "content part done",
            json!({
                "type": "response.content_part.done",
                "part": { "type": "output_text", "text": "Generated title" }
            }),
        ),
        (
            "output item done",
            json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "message",
                    "content": [{ "type": "output_text", "text": "Generated title" }]
                }
            }),
        ),
        (
            "response completed output",
            json!({
                "type": "response.completed",
                "response": {
                    "status": "completed",
                    "output": [{
                        "type": "message",
                        "content": [{ "type": "output_text", "text": "Generated title" }]
                    }]
                }
            }),
        ),
    ];

    for (case, final_event) in cases {
        let mut stream_events = vec![
            json!({ "type": "response.created", "response": { "id": "resp-final" } }),
            final_event,
        ];
        if stream_events[1]["type"] != "response.completed" {
            stream_events.push(json!({
                "type": "response.completed",
                "response": { "status": "completed" }
            }));
        }
        let server = MockServer::start(vec![sse_response(&stream_events)]);
        let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

        let events = client
            .stream_chat(request(ApiKind::OpenAiResponse))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("stream should succeed");

        assert!(
            events.contains(&AiStreamEvent::TextDelta {
                text: "Generated title".to_owned(),
            }),
            "{case}"
        );
    }
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
        prompt_cache_key: Some("lane-1".to_owned()),
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
    assert_eq!(
        sent.body["prompt_cache_key"], "lane-1",
        "the dedicated cache-lane field maps to prompt_cache_key"
    );
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
            .contains("OpenAI Responses media content is only supported")
    );
    assert_eq!(server.requests().len(), 0);
}
