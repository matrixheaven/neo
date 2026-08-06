use std::time::Duration;

use futures::StreamExt;
use neo_ai::{
    AiError, AiStreamEvent, ApiKind, ChatMessage, ContentPart, ImageData, MessagePhase,
    ModelClient, ReasoningEffort, ReasoningSelection, StopReason, ThinkingKind,
    providers::google::GoogleGenerativeAiClient,
};
use serde_json::json;

use super::http_server::{
    MockServer, RecordedRequest, image_request, request, sse_response, status_response,
    tool_result_request, truncated_sse_response,
};

#[tokio::test]
async fn google_generative_ai_client_posts_generate_content_payload_and_streams_events() {
    let server = MockServer::start(vec![sse_response(&[
        json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{ "text": "hi " }]
                }
            }]
        }),
        json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{
                        "functionCall": {
                            "name": "read_file",
                            "args": { "path": "Cargo.toml" }
                        }
                    }]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 9,
                "candidatesTokenCount": 4
            }
        }),
    ])]);
    let client = GoogleGenerativeAiClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(ApiKind::GoogleGenerativeAi))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(events.len(), 6);
    assert_eq!(
        events[0],
        AiStreamEvent::MessageStart {
            id: "google-generative-ai".to_owned(),
            phase: MessagePhase::Unknown,
        }
    );
    assert_eq!(
        events[1],
        AiStreamEvent::TextDelta {
            text: "hi ".to_owned()
        }
    );
    let tool_id = match &events[2] {
        AiStreamEvent::ToolCallStart { id, name } => {
            assert_eq!(name, "read_file");
            id.clone()
        }
        other => panic!("expected ToolCallStart, got {other:?}"),
    };
    assert_eq!(
        events[3],
        AiStreamEvent::ToolCallArgsDelta {
            id: tool_id.clone(),
            json_fragment: "{\"path\":\"Cargo.toml\"}".to_owned()
        }
    );
    assert_eq!(
        events[4],
        AiStreamEvent::ToolCallEnd {
            id: tool_id,
            raw_arguments: r#"{"path":"Cargo.toml"}"#.to_owned()
        }
    );
    assert_eq!(
        events[5],
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::ToolUse,
            usage: Some(neo_ai::TokenUsage {
                input_tokens: 9,
                output_tokens: 4,
                input_cache_read_tokens: 0,
                input_cache_write_tokens: 0,
            }),
            phase: MessagePhase::Unknown,
        }
    );

    assert_google_request(&server.requests().pop().unwrap());
}

fn assert_google_request(sent: &RecordedRequest) {
    assert_eq!(sent.method, "POST");
    assert_eq!(
        sent.path,
        "/models/model-test:streamGenerateContent?alt=sse"
    );
    assert_eq!(
        sent.headers.get("x-goog-api-key").map(String::as_str),
        Some("test-key")
    );
    assert_eq!(sent.body["contents"][0]["role"], "user");
    assert_eq!(sent.body["contents"][0]["parts"][0]["text"], "hello");
    assert_eq!(
        sent.body["tools"][0]["functionDeclarations"][0]["name"],
        "read_file"
    );
    assert_eq!(
        sent.body["tools"][0]["functionDeclarations"][0]["parameters"]["properties"]["path"]["type"],
        "string"
    );
    assert_eq!(sent.body["generationConfig"]["maxOutputTokens"], 64);
    assert!(
        sent.body["generationConfig"]
            .get("thinkingConfig")
            .is_none(),
        "thinkingConfig must be omitted unless reasoning is requested"
    );
}

#[tokio::test]
async fn google_uses_header_auth_and_maps_bounded_error_body() {
    let body = r#"{"error":{"message":"context_length exceeded"}}"#;
    let server = MockServer::start(vec![format!(
        "HTTP/1.1 413 Content Too Large\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )]);
    let client = GoogleGenerativeAiClient::new(server.url.clone(), "secret-key");
    let mut request = request(ApiKind::GoogleGenerativeAi);
    request.options.headers.insert(
        "x-goog-api-key".to_owned(),
        "attacker-controlled".to_owned(),
    );

    let err = client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]
            .headers
            .get("x-goog-api-key")
            .map(String::as_str),
        Some("secret-key")
    );
    assert!(!requests[0].path.contains("secret-key"));
    assert_eq!(err.code(), "provider.context_overflow");
}

#[tokio::test]
async fn google_stream_numeric_server_error_is_retryable() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "error": {
            "code": 503,
            "status": "UNAVAILABLE",
            "message": "provider busy"
        }
    })])]);
    let client = GoogleGenerativeAiClient::new(server.url.clone(), "test-key");

    let error = client
        .stream_chat(request(ApiKind::GoogleGenerativeAi))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert!(matches!(
        error,
        AiError::Server {
            status: 503,
            retry_after: None,
            message
        } if message == "provider busy"
    ));
}

#[tokio::test]
async fn provider_error_body_stops_reading_at_limit() {
    let server = MockServer::start_unfinished_chunked_error(vec![b'x'; 64 * 1024]);
    let client = GoogleGenerativeAiClient::new(server.url.clone(), "test-key");

    let events = tokio::time::timeout(
        Duration::from_secs(1),
        client
            .stream_chat(request(ApiKind::GoogleGenerativeAi))
            .collect::<Vec<_>>(),
    )
    .await
    .expect("provider should stop reading once the error body reaches its limit");
    let err = events
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert_eq!(err.code(), "provider.protocol_error");
    server.release();
}

#[tokio::test]
async fn google_generative_ai_client_opens_provider_response_once() {
    let server = MockServer::start(vec![status_response(503)]);
    let client = GoogleGenerativeAiClient::new(server.url.clone(), "test-key");
    let request = request(ApiKind::GoogleGenerativeAi);

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
async fn google_generative_ai_client_serializes_tool_result_errors() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [{ "text": "done" }]
            },
            "finishReason": "STOP"
        }]
    })])]);
    let client = GoogleGenerativeAiClient::new(server.url.clone(), "test-key");

    client
        .stream_chat(tool_result_request(ApiKind::GoogleGenerativeAi, true))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    assert_eq!(sent.body["contents"][2]["role"], "user");
    let function_response = &sent.body["contents"][2]["parts"][0]["functionResponse"];
    assert_eq!(function_response["name"], "read_file");
    assert_eq!(function_response["response"]["result"], "permission denied");
    assert_eq!(function_response["response"]["is_error"], true);
}

#[tokio::test]
async fn google_generative_ai_client_serializes_reasoning_selection_as_thinking_config() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [{ "text": "done" }]
            },
            "finishReason": "STOP"
        }]
    })])]);
    let client = GoogleGenerativeAiClient::new(server.url.clone(), "test-key");
    let mut request = request(ApiKind::GoogleGenerativeAi);
    request.options.reasoning = ReasoningSelection::Effort {
        effort: ReasoningEffort::medium(),
    };

    client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    assert_eq!(
        sent.body["generationConfig"]["thinkingConfig"]["includeThoughts"],
        true
    );
    assert_eq!(
        sent.body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
        2048
    );
}

#[tokio::test]
async fn google_generative_ai_client_rejects_custom_effort_without_posting() {
    let server = MockServer::start(Vec::new());
    let client = GoogleGenerativeAiClient::new(server.url.clone(), "test-key");
    let mut request = request(ApiKind::GoogleGenerativeAi);
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
async fn google_generative_ai_client_serializes_budget_reasoning_selection() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "candidates": [{
            "content": { "role": "model", "parts": [{ "text": "done" }] },
            "finishReason": "STOP"
        }]
    })])]);
    let client = GoogleGenerativeAiClient::new(server.url.clone(), "test-key");
    let mut request = request(ApiKind::GoogleGenerativeAi);
    request.options.reasoning = ReasoningSelection::BudgetTokens {
        budget_tokens: 8_192,
    };

    let events = client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert!(!events.is_empty());
    let sent = server.requests().pop().expect("request");
    assert_eq!(
        sent.body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
        8_192
    );
}

#[tokio::test]
async fn google_generative_ai_client_replays_signed_thought_parts() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [{ "text": "done" }]
            },
            "finishReason": "STOP"
        }]
    })])]);
    let client = GoogleGenerativeAiClient::new(server.url.clone(), "test-key");
    let mut request = request(ApiKind::GoogleGenerativeAi);
    request.messages.insert(
        1,
        ChatMessage::Assistant {
            content: vec![ContentPart::Thinking {
                text: "stored reasoning".to_owned(),
                signature: Some("sig-google".to_owned()),
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
    assert_eq!(sent.body["contents"][1]["role"], "model");
    assert_eq!(
        sent.body["contents"][1]["parts"][0],
        json!({
            "text": "stored reasoning",
            "thought": true,
            "thoughtSignature": "sig-google"
        })
    );
}

#[tokio::test]
async fn google_generative_ai_client_can_disable_thought_replay() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [{ "text": "done" }]
            },
            "finishReason": "STOP"
        }]
    })])]);
    let client = GoogleGenerativeAiClient::new(server.url.clone(), "test-key");
    let mut request = request(ApiKind::GoogleGenerativeAi);
    request.options.replay_reasoning = false;
    request.messages.insert(
        1,
        ChatMessage::Assistant {
            content: vec![
                ContentPart::Thinking {
                    text: "stored reasoning".to_owned(),
                    signature: Some("sig-google".to_owned()),
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
    assert_eq!(sent.body["contents"][1]["role"], "model");
    assert_eq!(
        sent.body["contents"][1]["parts"],
        json!([{ "text": "visible answer" }])
    );
}

#[tokio::test]
async fn google_generative_ai_client_streams_thought_parts_as_thinking_events() {
    let server = MockServer::start(vec![sse_response(&[
        json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{
                        "text": "Checked inputs.",
                        "thought": true,
                        "thoughtSignature": "sig-google"
                    }]
                }
            }]
        }),
        json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{ "text": "final answer" }]
                },
                "finishReason": "STOP"
            }]
        }),
    ])]);
    let client = GoogleGenerativeAiClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(ApiKind::GoogleGenerativeAi))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(
        events,
        vec![
            AiStreamEvent::MessageStart {
                id: "google-generative-ai".to_owned(),
                phase: MessagePhase::Unknown,
            },
            AiStreamEvent::ThinkingStart {
                id: "google-thought:0".to_owned(),
                kind: ThinkingKind::Full,
            },
            AiStreamEvent::ThinkingDelta {
                text: "Checked inputs.".to_owned()
            },
            AiStreamEvent::ThinkingEnd {
                signature: Some("sig-google".to_owned()),
                redacted: false,
            },
            AiStreamEvent::TextDelta {
                text: "final answer".to_owned()
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
async fn google_generative_ai_client_does_not_treat_signature_only_parts_as_thinking() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [{
                    "text": "plain signed text",
                    "thoughtSignature": "sig-not-thinking"
                }]
            },
            "finishReason": "STOP"
        }]
    })])]);
    let client = GoogleGenerativeAiClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(ApiKind::GoogleGenerativeAi))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(
        events,
        vec![
            AiStreamEvent::MessageStart {
                id: "google-generative-ai".to_owned(),
                phase: MessagePhase::Unknown,
            },
            AiStreamEvent::TextDelta {
                text: "plain signed text".to_owned()
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
async fn google_generative_ai_client_serializes_base64_image_parts_as_inline_data() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [{ "text": "done" }]
            },
            "finishReason": "STOP"
        }]
    })])]);
    let client = GoogleGenerativeAiClient::new(server.url.clone(), "test-key");

    client
        .stream_chat(image_request(
            ApiKind::GoogleGenerativeAi,
            ImageData::Base64("iVBORw0KGgo=".to_owned()),
        ))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    assert_eq!(
        sent.body["contents"][0]["parts"][0]["text"],
        "describe this"
    );
    assert_eq!(
        sent.body["contents"][0]["parts"][1]["inlineData"]["mimeType"],
        "image/png"
    );
    assert_eq!(
        sent.body["contents"][0]["parts"][1]["inlineData"]["data"],
        "iVBORw0KGgo="
    );
}

#[tokio::test]
async fn google_generative_ai_client_rejects_image_urls_without_dropping_them() {
    let server = MockServer::start(Vec::new());
    let client = GoogleGenerativeAiClient::new(server.url.clone(), "test-key");

    let err = client
        .stream_chat(image_request(
            ApiKind::GoogleGenerativeAi,
            ImageData::Url("https://example.test/cat.png".to_owned()),
        ))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert!(err.to_string().contains("image URL"));
    assert_eq!(server.requests().len(), 0);
}

#[tokio::test]
async fn google_body_error_respects_terminal_state() {
    let terminal = format!(
        "data: {}\n\n",
        json!({
            "candidates": [{
                "content": { "role": "model", "parts": [{ "text": "done" }] },
                "finishReason": "STOP"
            }]
        })
    );
    let incomplete = format!(
        "data: {}\n\n",
        json!({
            "candidates": [{
                "content": { "role": "model", "parts": [{ "text": "partial" }] }
            }]
        })
    );
    let server = MockServer::start(vec![
        truncated_sse_response(&terminal),
        truncated_sse_response(&incomplete),
    ]);
    let client = GoogleGenerativeAiClient::new(server.url.clone(), "test-key");

    let completed = client
        .stream_chat(request(ApiKind::GoogleGenerativeAi))
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
        .stream_chat(request(ApiKind::GoogleGenerativeAi))
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
