//! Google provider behavior (moved from `google.rs`).

use super::*;
use crate::ToolCall;
use crate::effective_media_capability;

#[test]
fn api_key_header_is_sensitive() {
    let headers = headers("secret-key", &BTreeMap::new()).unwrap();

    assert!(
        headers
            .get("x-goog-api-key")
            .expect("API key header should be present")
            .is_sensitive()
    );
}

#[test]
fn assistant_replay_rejects_invalid_raw_tool_arguments() {
    let result = content_body(
        &ChatMessage::Assistant {
            content: Vec::new(),
            tool_calls: vec![ToolCall {
                id: "call-1".to_owned(),
                name: "read".to_owned(),
                raw_arguments: r#"{"path":"Cargo"#.to_owned(),
            }],
        },
        false,
    )
    .expect("assistant message should produce content");

    let err = result.unwrap_err();
    assert!(
        matches!(err, ProviderError::Protocol(message) if message.contains("invalid raw tool arguments"))
    );
}

#[test]
fn duplicate_function_calls_keep_unique_ids_and_replay_names() {
    let mut parser = ParseState {
        tool_call_nonce: 1,
        ..ParseState::default()
    };
    let response = json!({
        "candidates": [{
            "content": {
                "parts": [
                    { "functionCall": { "name": "read", "args": { "path": "a" } } },
                    { "functionCall": { "name": "read", "args": { "path": "b" } } }
                ]
            },
            "finishReason": "STOP"
        }]
    });
    parser.ingest(&response).unwrap();
    let mut events = parser.drain_events();
    events.extend(parser.finish_events());

    let ids = events
        .iter()
        .filter_map(|event| match event {
            AiStreamEvent::ToolCallStart { id, name } if name == "read" => Some(id.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);

    for (id, arguments) in ids.iter().zip([r#"{"path":"a"}"#, r#"{"path":"b"}"#]) {
        assert!(events.contains(&AiStreamEvent::ToolCallArgsDelta {
            id: id.clone(),
            json_fragment: arguments.to_owned(),
        }));
        assert!(events.contains(&AiStreamEvent::ToolCallEnd {
            id: id.clone(),
            raw_arguments: arguments.to_owned(),
        }));
    }

    let mut second_parser = ParseState {
        tool_call_nonce: 2,
        ..ParseState::default()
    };
    second_parser
        .ingest(&json!({
            "candidates": [{
                "content": {
                    "parts": [
                        { "functionCall": { "name": "write", "args": { "path": "c" } } }
                    ]
                },
                "finishReason": "STOP"
            }]
        }))
        .unwrap();
    let second_id = second_parser
        .drain_events()
        .into_iter()
        .find_map(|event| match event {
            AiStreamEvent::ToolCallStart { id, name } if name == "write" => Some(id),
            _ => None,
        })
        .unwrap();
    assert!(!ids.contains(&second_id));

    assert_content_bodies_replay(&ids, second_id);
}

fn assert_content_bodies_replay(ids: &[String], second_id: String) {
    let messages = vec![
        ChatMessage::Assistant {
            content: Vec::new(),
            tool_calls: ids
                .iter()
                .zip([r#"{"path":"a"}"#, r#"{"path":"b"}"#])
                .map(|(id, raw_arguments)| ToolCall {
                    id: id.clone(),
                    name: "read".to_owned(),
                    raw_arguments: raw_arguments.to_owned(),
                })
                .collect(),
        },
        ChatMessage::ToolResult {
            tool_call_id: ids[1].clone(),
            content: vec![ContentPart::Text {
                text: "second".to_owned(),
            }],
            is_error: false,
        },
        ChatMessage::ToolResult {
            tool_call_id: ids[0].clone(),
            content: vec![ContentPart::Text {
                text: "first".to_owned(),
            }],
            is_error: false,
        },
        ChatMessage::Assistant {
            content: Vec::new(),
            tool_calls: vec![ToolCall {
                id: second_id.clone(),
                name: "write".to_owned(),
                raw_arguments: r#"{"path":"c"}"#.to_owned(),
            }],
        },
        ChatMessage::ToolResult {
            tool_call_id: second_id,
            content: vec![ContentPart::Text {
                text: "written".to_owned(),
            }],
            is_error: false,
        },
    ];
    let contents = content_bodies(&messages, false).unwrap();
    assert_eq!(contents.len(), 4);
    assert_eq!(contents[1]["role"], "user");
    assert_eq!(contents[1]["parts"][0]["functionResponse"]["name"], "read");
    assert_eq!(
        contents[1]["parts"][0]["functionResponse"]["response"]["result"],
        "first"
    );
    assert_eq!(contents[1]["parts"][1]["functionResponse"]["name"], "read");
    assert_eq!(
        contents[1]["parts"][1]["functionResponse"]["response"]["result"],
        "second"
    );
    assert_eq!(contents[3]["role"], "user");
    assert_eq!(contents[3]["parts"][0]["functionResponse"]["name"], "write");

    let unknown = vec![ChatMessage::ToolResult {
        tool_call_id: "unknown".to_owned(),
        content: vec![ContentPart::Text {
            text: "result".to_owned(),
        }],
        is_error: false,
    }];
    let err = content_bodies(&unknown, false).unwrap_err();
    assert!(matches!(
        err,
        ProviderError::Protocol(message) if message.contains("unknown tool call 'unknown'")
    ));
}

#[test]
fn tool_results_reject_incomplete_parallel_batch() {
    let messages = vec![
        ChatMessage::Assistant {
            content: Vec::new(),
            tool_calls: vec![
                ToolCall {
                    id: "call-1".to_owned(),
                    name: "read".to_owned(),
                    raw_arguments: "{}".to_owned(),
                },
                ToolCall {
                    id: "call-2".to_owned(),
                    name: "write".to_owned(),
                    raw_arguments: "{}".to_owned(),
                },
            ],
        },
        ChatMessage::ToolResult {
            tool_call_id: "call-1".to_owned(),
            content: vec![ContentPart::Text {
                text: "done".to_owned(),
            }],
            is_error: false,
        },
    ];

    let err = content_bodies(&messages, false).unwrap_err();
    assert!(matches!(
        err,
        ProviderError::Protocol(message)
            if message == "Google tool results are missing for tool calls: call-2"
    ));
}

#[test]
fn tool_results_reject_zero_result_batch_at_end_of_history() {
    let messages = vec![ChatMessage::Assistant {
        content: Vec::new(),
        tool_calls: vec![ToolCall {
            id: "call-1".to_owned(),
            name: "read".to_owned(),
            raw_arguments: "{}".to_owned(),
        }],
    }];

    let err = content_bodies(&messages, false).unwrap_err();
    assert!(matches!(
        err,
        ProviderError::Protocol(message)
            if message == "Google tool results are missing for tool calls: call-1"
    ));
}

#[test]
fn request_url_rejects_non_http_schemes_without_retry() {
    let error = request_url("file:///etc", "gemini-pro")
        .unwrap_err()
        .into_ai_error();
    assert!(matches!(error, AiError::Protocol { .. }));
    assert!(!error.is_retryable());
}

#[test]
fn oversized_sse_frame_is_rejected() {
    let mut parser = IncrementalSse::default();
    let data = "x".repeat(SseFramer::MAX_FRAME_BYTES + 1);
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
fn content_free_stop_emits_balanced_message() {
    let mut parser = IncrementalSse::default();
    let body = format!(
        "data: {}\n\n",
        serde_json::json!({ "candidates": [{ "finishReason": "STOP" }] })
    );
    let mut events = parser.push_chunk(body.as_bytes());
    events.extend(parser.finish());
    let events = events.into_iter().collect::<Result<Vec<_>, _>>().unwrap();

    assert_eq!(
        events,
        vec![
            AiStreamEvent::MessageStart {
                id: "google-generative-ai".to_owned(),
                phase: crate::MessagePhase::Unknown,
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
                phase: crate::MessagePhase::Unknown,
            },
        ]
    );
}

#[test]
fn content_free_error_finish_is_protocol() {
    let mut parser = IncrementalSse::default();
    let body = format!(
        "data: {}\n\n",
        serde_json::json!({ "candidates": [{ "finishReason": "SAFETY" }] })
    );
    let error = parser
        .push_chunk(body.as_bytes())
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert!(matches!(error, AiError::Protocol { .. }));
}

#[test]
fn google_omits_response_format_wire_hint() {
    use crate::{
        ApiKind, ModelCapabilities, ModelSpec, ProviderId, RequestOptions, ResponseFormat,
    };

    let request = ChatRequest {
        model: ModelSpec {
            provider: ProviderId("p".to_owned()),
            model: "m".to_owned(),
            api: ApiKind::GoogleGenerativeAi,
            capabilities: ModelCapabilities::chat(),
        },
        messages: vec![ChatMessage::User {
            content: vec![ContentPart::Text {
                text: "hi".to_owned(),
            }],
        }],
        tools: vec![],
        options: RequestOptions {
            response_format: Some(ResponseFormat {
                name: "result".to_owned(),
                schema: json!({"type": "object"}),
                strict: true,
            }),
            max_tokens: Some(64),
            ..RequestOptions::default()
        },
    };
    let body = request_body(&request).expect("unsupported providers omit, do not error");
    assert!(body.get("response_format").is_none());
    assert!(body.pointer("/text/format").is_none());
    assert!(body.pointer("/generationConfig/responseSchema").is_none());
    assert!(body.pointer("/generationConfig/responseMimeType").is_none());
}

#[test]
fn declared_video_and_tool_media_are_rejected_by_google_transport() {
    use crate::{EffectiveMediaCapability, MediaKind, MediaPosition, ModelCapabilities};

    let client = GoogleGenerativeAiClient::new("https://example.invalid", "key");
    let model = ModelCapabilities {
        images: true,
        videos: true,
        ..ModelCapabilities::chat()
    };
    let transport = client.media_transport();

    assert_eq!(
        effective_media_capability(
            MediaKind::Image,
            MediaPosition::UserMessage,
            &model,
            transport
        ),
        EffectiveMediaCapability::Sendable(MediaTransportMode::Inline)
    );
    for (kind, position) in [
        (MediaKind::Video, MediaPosition::UserMessage),
        (MediaKind::Image, MediaPosition::ToolResult),
        (MediaKind::Video, MediaPosition::ToolResult),
    ] {
        assert_eq!(
            effective_media_capability(kind, position, &model, transport),
            EffectiveMediaCapability::TransportUnsupported,
            "{kind:?} at {position:?}"
        );
    }
}
