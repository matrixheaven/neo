use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use futures::StreamExt;
use neo_ai::{
    AiStreamEvent, ApiKind, CacheRetention, ChatMessage, ChatRequest, ContentPart, ImageData,
    ModelCapabilities, ModelClient, ModelSpec, ProviderId, ReasoningEffort, RequestMetadata,
    RequestOptions, StopReason, ToolSpec, providers::openai::compatible::OpenAiCompatibleClient,
};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
struct RecordedRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Value,
}

struct MockServer {
    url: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

impl MockServer {
    fn start(responses: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured_requests = Arc::clone(&requests);

        thread::spawn(move || {
            for response in responses {
                let (mut socket, _) = listener.accept().unwrap();
                let request = read_http_request(&mut socket);
                captured_requests.lock().unwrap().push(request);
                socket.write_all(response.as_bytes()).unwrap();
            }
        });

        Self { url, requests }
    }

    fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }
}

fn read_http_request(socket: &mut TcpStream) -> RecordedRequest {
    let mut buffer = Vec::new();
    let mut temp = [0_u8; 1024];
    let header_end;

    loop {
        let read = socket.read(&mut temp).unwrap();
        assert_ne!(read, 0, "client closed before sending headers");
        buffer.extend_from_slice(&temp[..read]);
        if let Some(index) = find_header_end(&buffer) {
            header_end = index;
            break;
        }
    }

    let headers_raw = String::from_utf8(buffer[..header_end].to_vec()).unwrap();
    let mut lines = headers_raw.split("\r\n");
    let request_line = lines.next().unwrap();
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap().to_owned();
    let path = request_parts.next().unwrap().to_owned();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned()))
        .collect::<BTreeMap<_, _>>();
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let body_start = header_end + 4;
    while buffer.len() < body_start + content_length {
        let read = socket.read(&mut temp).unwrap();
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..read]);
    }
    let body_bytes = &buffer[body_start..body_start + content_length];
    let body = serde_json::from_slice(body_bytes).unwrap();

    RecordedRequest {
        method,
        path,
        headers,
        body,
    }
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn sse_response(events: &[Value]) -> String {
    let mut body = String::new();
    for event in events {
        write!(&mut body, "data: {event}\n\n").unwrap();
    }
    body.push_str("data: [DONE]\n\n");
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

fn status_response(status: u16) -> String {
    format!("HTTP/1.1 {status} Test\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
}

fn status_response_with_body(status: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} Test\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len(),
    )
}

fn request(options: RequestOptions) -> ChatRequest {
    ChatRequest {
        model: ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: "gpt-test".to_owned(),
            api: ApiKind::OpenAi,
            capabilities: ModelCapabilities::tool_chat(),
        },
        messages: vec![ChatMessage::User {
            content: vec![ContentPart::Text {
                text: "hello".to_owned(),
            }],
        }],
        tools: vec![ToolSpec::string_arg(
            "read_file",
            "Read a file",
            "path",
            "Path to read",
        )],
        options,
    }
}

fn image_request(image: ImageData) -> ChatRequest {
    ChatRequest {
        model: ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: "gpt-test".to_owned(),
            api: ApiKind::OpenAi,
            capabilities: ModelCapabilities::vision_chat(),
        },
        messages: vec![ChatMessage::User {
            content: vec![
                ContentPart::Text {
                    text: "describe this".to_owned(),
                },
                ContentPart::Image {
                    mime_type: "image/png".to_owned(),
                    data: image,
                },
            ],
        }],
        tools: Vec::new(),
        options: RequestOptions::default(),
    }
}

#[tokio::test]
async fn openai_compatible_client_posts_typed_options_and_normalizes_sse_events() {
    let server = MockServer::start(vec![sse_response(&[
        json!({
            "id": "chatcmpl-1",
            "choices": [{
                "delta": {
                    "content": "hi ",
                    "tool_calls": [{
                        "index": 0,
                        "id": "call-1",
                        "function": { "name": "read_file", "arguments": "{\"path\":" }
                    }]
                }
            }]
        }),
        json!({
            "choices": [{
                "delta": {
                    "content": "there",
                    "tool_calls": [{
                        "index": 0,
                        "function": { "arguments": "\"Cargo.toml\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 7,
                "completion_tokens": 5,
                "prompt_tokens_details": { "cached_tokens": 4 }
            }
        }),
    ])]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");
    let mut headers = BTreeMap::new();
    headers.insert("x-neo-trace".to_owned(), "trace-1".to_owned());
    let request = request(RequestOptions {
        temperature: Some(0.4),
        max_tokens: Some(128),
        headers,
        timeout: Some(Duration::from_secs(5)),
        reasoning_effort: Some(ReasoningEffort::Medium),
        replay_reasoning: true,
        retries: Some(0),
        cache: CacheRetention::Long,
        session_id: Some("session-1".to_owned()),
        metadata: RequestMetadata::from_pairs([("user_id", "u-1")]),
        cancel_token: None,
    });

    let events = client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(events, expected_tool_events());

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_typed_request(&requests[0]);
}

#[tokio::test]
async fn openai_compatible_client_serializes_image_parts() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "id": "chatcmpl-image",
        "choices": [{ "delta": { "content": "ok" }, "finish_reason": "stop" }]
    })])]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");

    client
        .stream_chat(image_request(ImageData::Url(
            "https://example.test/cat.png".to_owned(),
        )))
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
    assert_eq!(sent.body["messages"][0]["content"][1]["type"], "image_url");
    assert_eq!(
        sent.body["messages"][0]["content"][1]["image_url"]["url"],
        "https://example.test/cat.png"
    );
}

#[tokio::test]
async fn openai_serializes_assistant_without_empty_tool_calls() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "id": "chatcmpl-empty-tool-calls",
        "choices": [{ "delta": { "content": "ok" }, "finish_reason": "stop" }]
    })])]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");
    let mut request = request(RequestOptions::default());
    request.messages = vec![ChatMessage::Assistant {
        content: vec![ContentPart::Text {
            text: "previous answer".to_owned(),
        }],
        tool_calls: Vec::new(),
    }];
    request.tools = Vec::new();

    client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    assert_eq!(sent.body["messages"][0]["role"], "assistant");
    assert_eq!(sent.body["messages"][0]["content"], "previous answer");
    assert!(
        sent.body["messages"][0].get("tool_calls").is_none(),
        "empty assistant tool_calls must be omitted: {}",
        sent.body["messages"][0]
    );
}

#[tokio::test]
async fn openai_serializes_assistant_thinking_as_reasoning_content() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "id": "chatcmpl-reasoning-out",
        "choices": [{ "delta": { "content": "ok" }, "finish_reason": "stop" }]
    })])]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");
    let mut request = request(RequestOptions::default());
    request.messages = vec![ChatMessage::Assistant {
        content: vec![
            ContentPart::Thinking {
                text: "plan privately".to_owned(),
                signature: None,
                redacted: false,
            },
            ContentPart::Text {
                text: "visible answer".to_owned(),
            },
        ],
        tool_calls: Vec::new(),
    }];
    request.tools = Vec::new();

    client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    assert_eq!(sent.body["messages"][0]["content"], "visible answer");
    assert_eq!(
        sent.body["messages"][0]["reasoning_content"],
        "plan privately"
    );
}

#[tokio::test]
async fn openai_does_not_infer_reasoning_effort_when_replaying_thinking() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "id": "chatcmpl-reasoning-effort",
        "choices": [{ "delta": { "content": "ok" }, "finish_reason": "stop" }]
    })])]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");
    let mut request = request(RequestOptions::default());
    request.options.reasoning_effort = None;
    request.messages = vec![ChatMessage::Assistant {
        content: vec![ContentPart::Thinking {
            text: "plan privately".to_owned(),
            signature: None,
            redacted: false,
        }],
        tool_calls: Vec::new(),
    }];
    request.tools = Vec::new();

    client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    assert!(sent.body.get("reasoning_effort").is_none());
}

#[tokio::test]
async fn openai_streams_reasoning_content_as_thinking_events() {
    let server = MockServer::start(vec![sse_response(&[
        json!({
            "id": "chatcmpl-reasoning-in",
            "choices": [{
                "delta": {
                    "reasoning_content": "plan privately"
                }
            }]
        }),
        json!({
            "id": "chatcmpl-reasoning-in",
            "choices": [{ "delta": { "content": "done" }, "finish_reason": "stop" }]
        }),
    ])]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(RequestOptions::default()))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert!(events.contains(&AiStreamEvent::ThinkingStart {
        id: "reasoning".to_owned(),
    }));
    assert!(events.contains(&AiStreamEvent::ThinkingDelta {
        text: "plan privately".to_owned(),
    }));
    assert!(events.contains(&AiStreamEvent::ThinkingEnd {
        signature: None,
        redacted: false,
    }));
    assert!(events.contains(&AiStreamEvent::TextDelta {
        text: "done".to_owned(),
    }));
}

#[tokio::test]
async fn openai_tool_calls_finish_reason_without_structured_calls_is_error() {
    let server = MockServer::start(vec![sse_response(&[
        json!({
            "id": "chatcmpl-xml-tool",
            "choices": [{
                "delta": {
                    "content": "<tool_call><function=Bash></function></tool_call>"
                }
            }]
        }),
        json!({
            "id": "chatcmpl-xml-tool",
            "choices": [{ "delta": {}, "finish_reason": "tool_calls" }]
        }),
    ])]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(RequestOptions::default()))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert!(events.iter().any(|event| matches!(
        event,
        AiStreamEvent::TextDelta { text }
            if text.contains("<tool_call><function=Bash>")
    )));
    assert!(!events.iter().any(|event| matches!(
        event,
        AiStreamEvent::ToolCallStart { .. } | AiStreamEvent::ToolCallEnd { .. }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AiStreamEvent::Error { message }
            if message == "Provider reported tool calls but emitted no structured tool calls"
    )));
    assert!(matches!(
        events.last(),
        Some(AiStreamEvent::MessageEnd {
            stop_reason: StopReason::Error,
            ..
        })
    ));
}

#[tokio::test]
async fn openai_tool_calls_finish_reason_with_structured_calls_remains_tool_use() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "id": "chatcmpl-structured-tool",
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call-1",
                    "function": { "name": "read_file", "arguments": "{\"path\":\"Cargo.toml\"}" }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    })])]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(RequestOptions::default()))
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
async fn openai_http_status_error_includes_body_excerpt() {
    let server = MockServer::start(vec![status_response_with_body(
        400,
        r#"{"error":{"message":"bad tool_call_id call_1"}}"#,
    )]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");

    let err = client
        .stream_chat(request(RequestOptions::default()))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    let message = err.to_string();
    assert!(message.contains("http status 400"), "{message}");
    assert!(message.contains("bad tool_call_id call_1"), "{message}");
}

#[tokio::test]
async fn openai_rejects_unsupported_reasoning_effort_without_posting() {
    let server = MockServer::start(Vec::new());
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");
    let err = client
        .stream_chat(request(RequestOptions {
            reasoning_effort: Some(ReasoningEffort::Minimal),
            retries: Some(0),
            ..RequestOptions::default()
        }))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    let message = err.to_string();
    assert!(message.contains("low, medium, or high"), "{message}");
    assert!(message.contains("minimal"), "{message}");
    assert!(server.requests().is_empty());
}

fn expected_tool_events() -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            id: "chatcmpl-1".to_owned(),
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
        AiStreamEvent::TextDelta {
            text: "there".to_owned(),
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
                input_tokens: 7,
                output_tokens: 5,
                input_cache_read_tokens: 4,
                input_cache_write_tokens: 0,
            }),
        },
    ]
}

fn assert_typed_request(sent: &RecordedRequest) {
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.path, "/chat/completions");
    assert_eq!(
        sent.headers.get("authorization").unwrap(),
        "Bearer test-key"
    );
    assert_eq!(sent.headers.get("x-neo-trace").unwrap(), "trace-1");
    assert_eq!(
        sent.headers.get("x-client-request-id").unwrap(),
        "session-1"
    );
    assert_eq!(sent.body["model"], "gpt-test");
    assert_eq!(sent.body["stream"], true);
    assert_eq!(sent.body["temperature"], 0.4);
    assert_eq!(sent.body["max_tokens"], 128);
    assert_eq!(sent.body["reasoning_effort"], "medium");
    assert_eq!(sent.body["metadata"], json!({ "user_id": "u-1" }));
    assert_eq!(sent.body["prompt_cache_key"], "session-1");
    assert_eq!(sent.body["prompt_cache_retention"], "24h");
    assert_eq!(sent.body["tools"][0]["function"]["name"], "read_file");
}

#[tokio::test]
async fn openai_compatible_client_normalizes_tool_schema_before_sending() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "id": "chatcmpl-1",
        "choices": [{
            "delta": { "content": "ok" },
            "finish_reason": "stop"
        }]
    })])]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");
    let mut request = request(RequestOptions::default());
    request.tools = vec![ToolSpec::new(
        "Terminal",
        "Operate a PTY.",
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$defs": {
                "TerminalMode": {
                    "oneOf": [
                        { "const": "start", "type": "string" },
                        { "const": "read", "type": "string" }
                    ]
                }
            },
            "title": "TerminalInput",
            "type": "object",
            "properties": {
                "mode": { "$ref": "#/$defs/TerminalMode" },
                "timeout": {
                    "format": "uint64",
                    "type": ["integer", "null"]
                }
            },
            "required": ["mode"]
        }),
    )];

    client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    assert_eq!(
        sent.body["tools"][0]["function"]["parameters"],
        json!({
            "type": "object",
            "properties": {
                "mode": {
                    "type": "string",
                    "enum": ["start", "read"]
                },
                "timeout": {
                    "type": "integer"
                }
            },
            "required": ["mode"]
        })
    );
}

#[tokio::test]
async fn openai_compatible_client_retries_retryable_http_responses() {
    let server = MockServer::start(vec![
        status_response(500),
        sse_response(&[json!({
            "id": "chatcmpl-retry",
            "choices": [{ "delta": { "content": "ok" }, "finish_reason": "stop" }]
        })]),
    ]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");
    let request = request(RequestOptions {
        retries: Some(1),
        ..RequestOptions::default()
    });

    let events = client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(server.requests().len(), 2);
    assert!(matches!(
        events.as_slice(),
        [
            AiStreamEvent::MessageStart { .. },
            AiStreamEvent::TextDelta { text },
            AiStreamEvent::MessageEnd { stop_reason: StopReason::EndTurn, .. }
        ] if text == "ok"
    ));
}

#[tokio::test]
async fn openai_compatible_client_reports_non_retryable_http_failures() {
    let server = MockServer::start(vec![status_response(401)]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");
    let err = client
        .stream_chat(request(RequestOptions::default()))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert!(err.to_string().contains("authentication error"));
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn openai_compatible_half_json_arguments_emit_raw_tool_call_end() {
    let raw = r#"{"command":"uname -a","description": "#;
    let server = MockServer::start(vec![sse_response(&[json!({
        "id": "chatcmpl-half-json",
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call-1",
                    "function": {
                        "name": "Bash",
                        "arguments": raw
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    })])]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(RequestOptions {
            retries: Some(0),
            ..RequestOptions::default()
        }))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert!(events.contains(&AiStreamEvent::ToolCallEnd {
        id: "call-1".to_owned(),
        raw_arguments: raw.to_owned(),
    }));
}

#[tokio::test]
async fn openai_compatible_stable_index_survives_tool_id_mutation() {
    let server = MockServer::start(vec![sse_response(&[
        json!({
            "id": "chatcmpl-id-mutation",
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "functions.read:0",
                        "function": { "name": "read_file", "arguments": "{\"path\":" }
                    }]
                }
            }]
        }),
        json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "chatcmpl-tool-b",
                        "function": { "arguments": "\"Cargo.toml\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }),
    ])]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(RequestOptions::default()))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AiStreamEvent::ToolCallStart { .. }))
            .count(),
        1
    );
    assert!(events.contains(&AiStreamEvent::ToolCallEnd {
        id: "functions.read:0".to_owned(),
        raw_arguments: r#"{"path":"Cargo.toml"}"#.to_owned(),
    }));
}

#[tokio::test]
async fn openai_compatible_buffers_arguments_until_tool_name_arrives() {
    let server = MockServer::start(vec![sse_response(&[
        json!({
            "id": "chatcmpl-args-first",
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call-1",
                        "function": { "arguments": "{\"path\":\"Cargo" }
                    }]
                }
            }]
        }),
        json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "name": "read_file", "arguments": ".toml\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }),
    ])]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(RequestOptions::default()))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let start_pos = events
        .iter()
        .position(|event| matches!(event, AiStreamEvent::ToolCallStart { .. }))
        .expect("missing start");
    let delta_pos = events
        .iter()
        .position(|event| matches!(event, AiStreamEvent::ToolCallArgsDelta { .. }))
        .expect("missing delta");
    assert!(start_pos < delta_pos);
    assert!(events.contains(&AiStreamEvent::ToolCallEnd {
        id: "call-1".to_owned(),
        raw_arguments: r#"{"path":"Cargo.toml"}"#.to_owned(),
    }));
}

#[tokio::test]
async fn openai_compatible_interleaves_two_indexed_tool_calls() {
    let server = MockServer::start(vec![sse_response(&[
        json!({
            "id": "chatcmpl-interleave",
            "choices": [{
                "delta": {
                    "tool_calls": [
                        { "index": 0, "id": "call-a", "function": { "name": "read_file", "arguments": "{\"path\":" } },
                        { "index": 1, "id": "call-b", "function": { "name": "read_file", "arguments": "{\"path\":" } }
                    ]
                }
            }]
        }),
        json!({
            "choices": [{
                "delta": {
                    "tool_calls": [
                        { "index": 1, "function": { "arguments": "\"B.md\"}" } },
                        { "index": 0, "function": { "arguments": "\"A.md\"}" } }
                    ]
                },
                "finish_reason": "tool_calls"
            }]
        }),
    ])]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(RequestOptions::default()))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert!(events.contains(&AiStreamEvent::ToolCallEnd {
        id: "call-a".to_owned(),
        raw_arguments: r#"{"path":"A.md"}"#.to_owned(),
    }));
    assert!(events.contains(&AiStreamEvent::ToolCallEnd {
        id: "call-b".to_owned(),
        raw_arguments: r#"{"path":"B.md"}"#.to_owned(),
    }));
}

#[tokio::test]
async fn openai_compatible_ignores_empty_tool_argument_deltas() {
    let server = MockServer::start(vec![sse_response(&[
        json!({
            "id": "chatcmpl-empty-delta",
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call-1",
                        "function": { "name": "read_file", "arguments": "" }
                    }]
                }
            }]
        }),
        json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "arguments": "{\"path\":\"Cargo.toml\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }),
    ])]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(RequestOptions::default()))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AiStreamEvent::ToolCallArgsDelta { json_fragment, .. } if json_fragment.is_empty()))
            .count(),
        0
    );
    assert!(events.contains(&AiStreamEvent::ToolCallEnd {
        id: "call-1".to_owned(),
        raw_arguments: r#"{"path":"Cargo.toml"}"#.to_owned(),
    }));
}
