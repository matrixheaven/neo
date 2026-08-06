use futures::StreamExt;
use neo_ai::{
    AiError, AiStreamEvent, ApiKind, ChatMessage, ContentPart, ImageData, MessagePhase,
    ModelClient, ReasoningEffort, ReasoningSelection, StopReason, ThinkingKind,
    providers::anthropic::AnthropicMessagesClient,
};
use serde_json::json;

use super::http_server::{
    MockServer, RecordedRequest, image_request, multi_tool_result_request, request, sse_response,
    status_response, tool_result_request, truncated_sse_response,
};

#[tokio::test]
async fn anthropic_messages_client_posts_messages_payload_and_streams_events() {
    let server = MockServer::start(vec![sse_response(&[
        json!({
            "type": "message_start",
            "message": {
                "id": "msg-1",
                "usage": {
                    "input_tokens": 11,
                    "output_tokens": 1,
                    "cache_read_input_tokens": 8,
                    "cache_creation_input_tokens": 2
                }
            }
        }),
        json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "tool_use", "id": "toolu-1", "name": "read_file" }
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "input_json_delta", "partial_json": "{\"path\":" }
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "input_json_delta", "partial_json": "\"Cargo.toml\"}" }
        }),
        json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": { "type": "text_delta", "text": "done" }
        }),
        json!({
            "type": "message_delta",
            "delta": { "stop_reason": "tool_use" },
            "usage": { "output_tokens": 3 }
        }),
        json!({ "type": "message_stop" }),
    ])]);
    let client = AnthropicMessagesClient::new(server.url.clone(), "test-key");
    let mut request = request(ApiKind::AnthropicMessages);
    request.options.session_id = Some("session-anthropic".to_owned());

    let events = client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(
        events,
        vec![
            AiStreamEvent::MessageStart {
                id: "msg-1".to_owned(),
                phase: MessagePhase::Unknown,
            },
            AiStreamEvent::ToolCallStart {
                id: "toolu-1".to_owned(),
                name: "read_file".to_owned()
            },
            AiStreamEvent::ToolCallArgsDelta {
                id: "toolu-1".to_owned(),
                json_fragment: "{\"path\":".to_owned()
            },
            AiStreamEvent::ToolCallArgsDelta {
                id: "toolu-1".to_owned(),
                json_fragment: "\"Cargo.toml\"}".to_owned()
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned()
            },
            AiStreamEvent::ToolCallEnd {
                id: "toolu-1".to_owned(),
                raw_arguments: r#"{"path":"Cargo.toml"}"#.to_owned()
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::ToolUse,
                usage: Some(neo_ai::TokenUsage {
                    input_tokens: 11,
                    output_tokens: 3,
                    input_cache_read_tokens: 8,
                    input_cache_write_tokens: 2,
                }),
                phase: MessagePhase::Unknown,
            },
        ]
    );

    assert_anthropic_request(&server.requests().pop().unwrap());
}

#[tokio::test]
async fn anthropic_missing_tool_name_is_protocol_error_without_tool_lifecycle_events() {
    let server = MockServer::start(vec![sse_response(&[
        json!({
            "type": "message_start",
            "message": { "id": "msg-missing-name" }
        }),
        json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "tool_use", "id": "toolu-missing" }
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "input_json_delta", "partial_json": "{\"path\":\"Cargo.toml\"}" }
        }),
        json!({
            "type": "message_delta",
            "delta": { "stop_reason": "tool_use" },
            "usage": { "output_tokens": 2 }
        }),
        json!({ "type": "message_stop" }),
    ])]);
    let client = AnthropicMessagesClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(ApiKind::AnthropicMessages))
        .collect::<Vec<_>>()
        .await;

    assert!(
        events.iter().all(|event| {
            !matches!(
                event,
                Ok(AiStreamEvent::ToolCallStart { .. }
                    | AiStreamEvent::ToolCallArgsDelta { .. }
                    | AiStreamEvent::ToolCallEnd { .. })
            )
        }),
        "missing tool name must not emit tool lifecycle events: {events:?}"
    );
    assert_eq!(
        events.iter().filter(|event| event.is_err()).count(),
        1,
        "missing tool name must emit exactly one protocol error: {events:?}"
    );
    let error = events
        .into_iter()
        .find_map(Result::err)
        .expect("missing tool name must emit a protocol error");
    assert_eq!(error.code(), "provider.protocol_error");
    assert!(
        error.to_string().contains("without a function name"),
        "unexpected error message: {error}"
    );
}

fn assert_anthropic_request(sent: &RecordedRequest) {
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.path, "/messages");
    assert_eq!(sent.headers.get("x-api-key").unwrap(), "test-key");
    assert_eq!(sent.headers.get("anthropic-version").unwrap(), "2023-06-01");
    assert_eq!(
        sent.body["metadata"],
        json!({ "user_id": "session-anthropic" })
    );
    assert_eq!(sent.body["model"], "model-test");
    assert_eq!(sent.body["stream"], true);
    assert_eq!(sent.body["max_tokens"], 64);
    assert_eq!(sent.body["tools"][0]["name"], "read_file");
    assert_eq!(sent.body["messages"][0]["role"], "user");
    assert!(sent.body.get("thinking").is_none());
}

#[tokio::test]
async fn anthropic_messages_client_marks_system_tools_and_last_message_for_prompt_cache() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "message_start", "message": { "id": "msg-cache" } }),
        json!({ "type": "message_stop" }),
    ])]);
    let client = AnthropicMessagesClient::new(server.url.clone(), "test-key");
    let mut request = request(ApiKind::AnthropicMessages);
    request.messages = vec![
        ChatMessage::System {
            content: vec![ContentPart::Text {
                text: "stable system".to_owned(),
            }],
        },
        ChatMessage::User {
            content: vec![ContentPart::Text {
                text: "hello".to_owned(),
            }],
        },
    ];

    client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    let cache_control = json!({ "type": "ephemeral", "ttl": "5m" });
    assert_eq!(
        sent.body["system"],
        json!([{ "type": "text", "text": "stable system", "cache_control": cache_control.clone() }])
    );
    assert_eq!(
        sent.body["tools"][0]["cache_control"],
        cache_control.clone()
    );
    assert_eq!(
        sent.body["messages"][0]["content"][0]["cache_control"],
        cache_control
    );
}

#[tokio::test]
async fn anthropic_messages_client_opens_provider_response_once() {
    let server = MockServer::start(vec![status_response(529)]);
    let client = AnthropicMessagesClient::new(server.url.clone(), "test-key");
    let request = request(ApiKind::AnthropicMessages);

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
async fn anthropic_stream_overloaded_error_is_retryable_server() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "type": "error",
        "error": { "type": "overloaded_error", "message": "provider busy" }
    })])]);
    let client = AnthropicMessagesClient::new(server.url.clone(), "test-key");

    let error = client
        .stream_chat(request(ApiKind::AnthropicMessages))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert!(matches!(
        error,
        AiError::Server {
            status: 529,
            retry_after: None,
            message
        } if message == "provider busy"
    ));
}

#[tokio::test]
async fn anthropic_messages_client_reports_non_retryable_http_response_body() {
    let server = MockServer::start(vec![format!(
        "HTTP/1.1 400 Test\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        r#"{"error":{"message":"tool schema is invalid","type":"invalid_request_error"}}"#.len(),
        r#"{"error":{"message":"tool schema is invalid","type":"invalid_request_error"}}"#
    )]);
    let client = AnthropicMessagesClient::new(server.url.clone(), "test-key");

    let err = client
        .stream_chat(request(ApiKind::AnthropicMessages))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    let text = err.to_string();
    assert!(text.contains("http status 400"));
    assert!(text.contains("tool schema is invalid"));
}

#[tokio::test]
async fn anthropic_messages_client_serializes_tool_result_errors() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "message_start", "message": { "id": "msg-tool-result" } }),
        json!({ "type": "message_stop" }),
    ])]);
    let client = AnthropicMessagesClient::new(server.url.clone(), "test-key");

    client
        .stream_chat(tool_result_request(ApiKind::AnthropicMessages, true))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    let tool_result = &sent.body["messages"][2]["content"][0];
    assert_eq!(tool_result["type"], "tool_result");
    assert_eq!(tool_result["tool_use_id"], "call-1");
    assert_eq!(tool_result["content"], "permission denied");
    assert_eq!(tool_result["is_error"], true);
}

#[tokio::test]
async fn anthropic_messages_client_groups_consecutive_tool_results_in_one_user_message() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "message_start", "message": { "id": "msg-multi-tool-result" } }),
        json!({ "type": "message_stop" }),
    ])]);
    let client = AnthropicMessagesClient::new(server.url.clone(), "test-key");

    client
        .stream_chat(multi_tool_result_request(ApiKind::AnthropicMessages))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    assert_eq!(sent.body["messages"].as_array().expect("messages").len(), 3);
    let result_message = &sent.body["messages"][2];
    assert_eq!(result_message["role"], "user");
    assert_eq!(
        result_message["content"].as_array().expect("content").len(),
        2
    );
    assert_eq!(result_message["content"][0]["type"], "tool_result");
    assert_eq!(result_message["content"][0]["tool_use_id"], "call-1");
    assert_eq!(
        result_message["content"][0]["content"],
        "workspace manifest"
    );
    assert_eq!(result_message["content"][1]["type"], "tool_result");
    assert_eq!(result_message["content"][1]["tool_use_id"], "call-2");
    assert_eq!(result_message["content"][1]["content"], "ai\nagent-core");
}

#[tokio::test]
async fn anthropic_messages_client_serializes_reasoning_selection_as_budget_thinking() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "message_start", "message": { "id": "msg-thinking" } }),
        json!({ "type": "message_stop" }),
    ])]);
    let client = AnthropicMessagesClient::new(server.url.clone(), "test-key");
    let mut request = request(ApiKind::AnthropicMessages);
    request.options.temperature = Some(0.4);
    request.options.reasoning = ReasoningSelection::Effort {
        effort: ReasoningEffort::high(),
    };

    client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    assert_eq!(sent.body["thinking"]["type"], "enabled");
    assert_eq!(sent.body["thinking"]["budget_tokens"], 8192);
    assert_eq!(sent.body["thinking"]["display"], "summarized");
    assert!(
        sent.body.get("temperature").is_none(),
        "Anthropic temperature is incompatible with extended thinking"
    );
    assert!(
        sent.body.get("output_config").is_none(),
        "Neo does not opt into adaptive Anthropic thinking without explicit model compat"
    );
}

#[tokio::test]
async fn anthropic_messages_client_rejects_custom_effort_without_posting() {
    let server = MockServer::start(Vec::new());
    let client = AnthropicMessagesClient::new(server.url.clone(), "test-key");
    let mut request = request(ApiKind::AnthropicMessages);
    request.options.reasoning = ReasoningSelection::Effort {
        effort: ReasoningEffort::try_from("UltraMax").expect("custom effort"),
    };

    let error = client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("custom reasoning effort 'UltraMax'")
    );
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn anthropic_messages_client_serializes_budget_reasoning_selection() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "message_start", "message": { "id": "msg-thinking-budget" } }),
        json!({ "type": "message_stop" }),
    ])]);
    let client = AnthropicMessagesClient::new(server.url.clone(), "test-key");
    let mut request = request(ApiKind::AnthropicMessages);
    request.options.temperature = Some(0.4);
    request.options.reasoning = ReasoningSelection::BudgetTokens {
        budget_tokens: 12_288,
    };

    client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    assert_eq!(sent.body["thinking"]["type"], "enabled");
    assert_eq!(sent.body["thinking"]["budget_tokens"], 12_288);
    assert_eq!(sent.body["thinking"]["display"], "summarized");
    assert!(
        sent.body.get("temperature").is_none(),
        "Anthropic temperature is incompatible with extended thinking"
    );
}

#[tokio::test]
async fn anthropic_messages_client_replays_signed_thinking_blocks() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "message_start", "message": { "id": "msg-replay" } }),
        json!({ "type": "message_stop" }),
    ])]);
    let client = AnthropicMessagesClient::new(server.url.clone(), "test-key");
    let mut request = request(ApiKind::AnthropicMessages);
    request.messages.insert(
        1,
        ChatMessage::Assistant {
            content: vec![
                ContentPart::Thinking {
                    text: "stored reasoning".to_owned(),
                    signature: Some("sig-anthropic".to_owned()),
                    redacted: false,
                },
                ContentPart::Thinking {
                    text: "[Reasoning redacted]".to_owned(),
                    signature: Some("opaque-redacted".to_owned()),
                    redacted: true,
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
    assert_eq!(sent.body["messages"][1]["role"], "assistant");
    assert_eq!(sent.body["messages"][1]["content"][0]["type"], "thinking");
    assert_eq!(
        sent.body["messages"][1]["content"][0]["thinking"],
        "stored reasoning"
    );
    assert_eq!(
        sent.body["messages"][1]["content"][0]["signature"],
        "sig-anthropic"
    );
    assert_eq!(
        sent.body["messages"][1]["content"][1],
        json!({ "type": "redacted_thinking", "data": "opaque-redacted" })
    );
}

#[tokio::test]
async fn anthropic_messages_client_can_disable_thinking_replay() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "message_start", "message": { "id": "msg-replay-off" } }),
        json!({ "type": "message_stop" }),
    ])]);
    let client = AnthropicMessagesClient::new(server.url.clone(), "test-key");
    let mut request = request(ApiKind::AnthropicMessages);
    request.options.replay_reasoning = false;
    request.messages.insert(
        1,
        ChatMessage::Assistant {
            content: vec![
                ContentPart::Thinking {
                    text: "stored reasoning".to_owned(),
                    signature: Some("sig-anthropic".to_owned()),
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
    let content = sent.body["messages"][1]["content"].as_array().unwrap();
    // Thinking blocks must be stripped; only the text block remains. The
    // cache-control injector may add a `cache_control` key to the text block,
    // so we assert on the meaningful fields rather than exact equality.
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "visible answer");
}

#[tokio::test]
async fn anthropic_messages_client_streams_extended_thinking_events() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "message_start", "message": { "id": "msg-thinking-stream" } }),
        json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "thinking", "thinking": "" }
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "thinking_delta", "thinking": "Checked " }
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "thinking_delta", "thinking": "the plan." }
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "signature_delta", "signature": "sig-test" }
        }),
        json!({ "type": "content_block_stop", "index": 0 }),
        json!({
            "type": "content_block_start",
            "index": 1,
            "content_block": { "type": "text", "text": "" }
        }),
        json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": { "type": "text_delta", "text": "final" }
        }),
        json!({ "type": "content_block_stop", "index": 1 }),
        json!({ "type": "message_stop" }),
    ])]);
    let client = AnthropicMessagesClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(ApiKind::AnthropicMessages))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(
        events,
        vec![
            AiStreamEvent::MessageStart {
                id: "msg-thinking-stream".to_owned(),
                phase: MessagePhase::Unknown,
            },
            AiStreamEvent::ThinkingStart {
                id: "thinking:0".to_owned(),
                kind: ThinkingKind::Full,
            },
            AiStreamEvent::ThinkingDelta {
                text: "Checked ".to_owned()
            },
            AiStreamEvent::ThinkingDelta {
                text: "the plan.".to_owned()
            },
            AiStreamEvent::ThinkingEnd {
                signature: Some("sig-test".to_owned()),
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
async fn anthropic_messages_client_serializes_image_parts() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "message_start", "message": { "id": "msg-image" } }),
        json!({ "type": "message_stop" }),
    ])]);
    let client = AnthropicMessagesClient::new(server.url.clone(), "test-key");

    client
        .stream_chat(image_request(
            ApiKind::AnthropicMessages,
            ImageData::Base64("iVBORw0KGgo=".to_owned()),
        ))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    assert_eq!(sent.body["messages"][0]["content"][0]["type"], "text");
    assert_eq!(
        sent.body["messages"][0]["content"][0]["text"],
        "describe this"
    );
    assert_eq!(sent.body["messages"][0]["content"][1]["type"], "image");
    assert_eq!(
        sent.body["messages"][0]["content"][1]["source"]["type"],
        "base64"
    );
    assert_eq!(
        sent.body["messages"][0]["content"][1]["source"]["media_type"],
        "image/png"
    );
    assert_eq!(
        sent.body["messages"][0]["content"][1]["source"]["data"],
        "iVBORw0KGgo="
    );
}

#[tokio::test]
async fn anthropic_body_error_respects_terminal_state() {
    let terminal = format!(
        "data: {}\n\ndata: {}\n\n",
        json!({ "type": "message_start", "message": { "id": "msg-terminal" } }),
        json!({ "type": "message_stop" })
    );
    let incomplete = format!(
        "data: {}\n\n",
        json!({ "type": "message_start", "message": { "id": "msg-incomplete" } })
    );
    let server = MockServer::start(vec![
        truncated_sse_response(&terminal),
        truncated_sse_response(&incomplete),
    ]);
    let client = AnthropicMessagesClient::new(server.url.clone(), "test-key");

    let completed = client
        .stream_chat(request(ApiKind::AnthropicMessages))
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
        .stream_chat(request(ApiKind::AnthropicMessages))
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
