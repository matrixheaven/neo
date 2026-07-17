use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use neo_ai::{
    AiError, AiStreamEvent, ApiKind, CacheRetention, ChatMessage, ChatRequest, ContentPart,
    ImageData, ImageGenerationClient, ImageGenerationRequest, ImageGenerationResponseImage,
    ModelCapabilities, ModelClient, ModelSpec, ProviderId, ReasoningEffort, ReasoningSelection,
    RequestMetadata, RequestOptions, StopReason, ToolCall, ToolSpec,
    providers::{
        anthropic::AnthropicMessagesClient, google::GoogleGenerativeAiClient,
        openai::compatible::OpenAiCompatibleClient, openai::images::OpenAiImagesClient,
        openai::responses::OpenAiResponsesClient,
    },
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

        std::thread::spawn(move || {
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

    fn start_unfinished_chunked_error(body: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured_requests = Arc::clone(&requests);

        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let request = read_http_request(&mut socket);
            captured_requests.lock().unwrap().push(request);
            write!(
                socket,
                "HTTP/1.1 400 Bad Request\r\ntransfer-encoding: chunked\r\nconnection: keep-alive\r\n\r\n{:x}\r\n",
                body.len()
            )
            .unwrap();
            socket.write_all(&body).unwrap();
            socket.write_all(b"\r\n").unwrap();
            socket.flush().unwrap();
            std::thread::sleep(Duration::from_secs(5));
        });

        Self { url, requests }
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
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

fn truncated_sse_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len() + 1,
        body
    )
}

fn status_response(status: u16) -> String {
    format!("HTTP/1.1 {status} Test\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
}

fn json_response(value: &Value) -> String {
    let body = value.to_string();
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

fn request(api: ApiKind) -> ChatRequest {
    ChatRequest {
        model: ModelSpec {
            provider: ProviderId("provider".to_owned()),
            model: "model-test".to_owned(),
            api,
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
        options: RequestOptions {
            max_tokens: Some(64),
            ..RequestOptions::default()
        },
    }
}

fn image_generation_request() -> ImageGenerationRequest {
    ImageGenerationRequest {
        model: ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: "gpt-image-1".to_owned(),
            api: ApiKind::OpenAiResponse,
            capabilities: ModelCapabilities::vision_chat(),
        },
        prompt: "draw a quiet terminal".to_owned(),
        size: "1024x1024".to_owned(),
    }
}

fn tool_result_request(api: ApiKind, is_error: bool) -> ChatRequest {
    ChatRequest {
        model: ModelSpec {
            provider: ProviderId("provider".to_owned()),
            model: "model-test".to_owned(),
            api,
            capabilities: ModelCapabilities::tool_chat(),
        },
        messages: vec![
            ChatMessage::User {
                content: vec![ContentPart::Text {
                    text: "read this".to_owned(),
                }],
            },
            ChatMessage::Assistant {
                content: Vec::new(),
                tool_calls: vec![ToolCall {
                    id: "call-1".to_owned(),
                    name: "read_file".to_owned(),
                    raw_arguments: r#"{"path":"Cargo.toml"}"#.to_owned(),
                }],
            },
            ChatMessage::ToolResult {
                tool_call_id: "call-1".to_owned(),
                content: vec![ContentPart::Text {
                    text: "permission denied".to_owned(),
                }],
                is_error,
            },
        ],
        tools: vec![ToolSpec::string_arg(
            "read_file",
            "Read a file",
            "path",
            "Path to read",
        )],
        options: RequestOptions {
            max_tokens: Some(64),
            ..RequestOptions::default()
        },
    }
}

fn multi_tool_result_request(api: ApiKind) -> ChatRequest {
    ChatRequest {
        model: ModelSpec {
            provider: ProviderId("provider".to_owned()),
            model: "model-test".to_owned(),
            api,
            capabilities: ModelCapabilities::tool_chat(),
        },
        messages: vec![
            ChatMessage::User {
                content: vec![ContentPart::Text {
                    text: "read this".to_owned(),
                }],
            },
            ChatMessage::Assistant {
                content: Vec::new(),
                tool_calls: vec![
                    ToolCall {
                        id: "call-1".to_owned(),
                        name: "read_file".to_owned(),
                        raw_arguments: r#"{"path":"Cargo.toml"}"#.to_owned(),
                    },
                    ToolCall {
                        id: "call-2".to_owned(),
                        name: "list_files".to_owned(),
                        raw_arguments: r#"{"path":"crates"}"#.to_owned(),
                    },
                ],
            },
            ChatMessage::ToolResult {
                tool_call_id: "call-1".to_owned(),
                content: vec![ContentPart::Text {
                    text: "workspace manifest".to_owned(),
                }],
                is_error: false,
            },
            ChatMessage::ToolResult {
                tool_call_id: "call-2".to_owned(),
                content: vec![ContentPart::Text {
                    text: "ai\nagent-core".to_owned(),
                }],
                is_error: false,
            },
        ],
        tools: vec![
            ToolSpec::string_arg("read_file", "Read a file", "path", "Path to read"),
            ToolSpec::string_arg("list_files", "List files", "path", "Path to list"),
        ],
        options: RequestOptions {
            max_tokens: Some(64),
            ..RequestOptions::default()
        },
    }
}

fn image_request(api: ApiKind, image: ImageData) -> ChatRequest {
    ChatRequest {
        model: ModelSpec {
            provider: ProviderId("provider".to_owned()),
            model: "model-test".to_owned(),
            api,
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

fn assistant_image_request(api: ApiKind, image: ImageData) -> ChatRequest {
    ChatRequest {
        model: ModelSpec {
            provider: ProviderId("provider".to_owned()),
            model: "model-test".to_owned(),
            api,
            capabilities: ModelCapabilities::vision_chat(),
        },
        messages: vec![
            ChatMessage::User {
                content: vec![ContentPart::Text {
                    text: "describe this".to_owned(),
                }],
            },
            ChatMessage::Assistant {
                content: vec![ContentPart::Image {
                    mime_type: "image/png".to_owned(),
                    data: image,
                }],
                tool_calls: Vec::new(),
            },
        ],
        tools: Vec::new(),
        options: RequestOptions::default(),
    }
}

#[tokio::test]
async fn openai_image_generation_client_serializes_request_and_decodes_base64_response() {
    let server = MockServer::start(vec![json_response(&json!({
        "created": 1_710_000_000,
        "data": [
            {
                "b64_json": "iVBORw0KGgo=",
                "revised_prompt": "draw a quiet terminal with soft light"
            }
        ]
    }))]);
    let client = OpenAiImagesClient::new(server.url.clone(), "test-key");

    let response = client
        .generate_image(image_generation_request())
        .await
        .expect("image generation should succeed");

    assert_eq!(
        response.images,
        vec![ImageGenerationResponseImage {
            mime_type: "image/png".to_owned(),
            data: ImageData::Base64("iVBORw0KGgo=".to_owned()),
            revised_prompt: Some("draw a quiet terminal with soft light".to_owned()),
        }]
    );
    let sent = server.requests().pop().expect("request");
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.path, "/images/generations");
    assert_eq!(
        sent.headers.get("authorization").unwrap(),
        "Bearer test-key"
    );
    assert_eq!(sent.body["model"], "gpt-image-1");
    assert_eq!(sent.body["prompt"], "draw a quiet terminal");
    assert_eq!(sent.body["size"], "1024x1024");
    assert_eq!(sent.body["n"], 1);
    assert!(sent.body.get("response_format").is_none());
}

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
                id: "resp-1".to_owned()
            },
            AiStreamEvent::TextDelta {
                text: "hi ".to_owned()
            },
            AiStreamEvent::ToolCallStart {
                id: "call-1".to_owned(),
                name: "read_file".to_owned()
            },
            AiStreamEvent::ToolCallArgsDelta {
                id: "call-1".to_owned(),
                json_fragment: "{\"path\":".to_owned()
            },
            AiStreamEvent::ToolCallArgsDelta {
                id: "call-1".to_owned(),
                json_fragment: "\"Cargo.toml\"}".to_owned()
            },
            AiStreamEvent::ToolCallEnd {
                id: "call-1".to_owned(),
                raw_arguments: r#"{"path":"Cargo.toml"}"#.to_owned()
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::ToolUse,
                usage: Some(neo_ai::TokenUsage {
                    input_tokens: 9,
                    output_tokens: 4,
                    input_cache_read_tokens: 0,
                    input_cache_write_tokens: 0,
                })
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
async fn openai_compatible_client_finishes_tool_call_on_tool_calls_finish_reason_without_done() {
    let body = [
        "data: {\"id\":\"chatcmpl-tool\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n\n",
        "data: {\"id\":\"chatcmpl-tool\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"Cargo.toml\\\"}\"}}]}}]}\n\n",
        "data: {\"id\":\"chatcmpl-tool\",\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":4}}\n\n",
    ]
    .concat();
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let server = MockServer::start(vec![response]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(ApiKind::OpenAi))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(
        events,
        vec![
            AiStreamEvent::MessageStart {
                id: "chatcmpl-tool".to_owned()
            },
            AiStreamEvent::ToolCallStart {
                id: "call-1".to_owned(),
                name: "read_file".to_owned()
            },
            AiStreamEvent::ToolCallArgsDelta {
                id: "call-1".to_owned(),
                json_fragment: "{\"path\":".to_owned()
            },
            AiStreamEvent::ToolCallArgsDelta {
                id: "call-1".to_owned(),
                json_fragment: "\"Cargo.toml\"}".to_owned()
            },
            AiStreamEvent::ToolCallEnd {
                id: "call-1".to_owned(),
                raw_arguments: r#"{"path":"Cargo.toml"}"#.to_owned()
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::ToolUse,
                usage: Some(neo_ai::TokenUsage {
                    input_tokens: 9,
                    output_tokens: 4,
                    input_cache_read_tokens: 0,
                    input_cache_write_tokens: 0,
                }),
            },
        ]
    );
}

#[tokio::test]
async fn openai_compatible_stream_rate_limit_error_is_retryable() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "error": { "code": "rate_limit_exceeded", "message": "slow down" }
    })])]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");

    let error = client
        .stream_chat(request(ApiKind::OpenAi))
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
                id: "resp-thinking".to_owned()
            },
            AiStreamEvent::ThinkingStart {
                id: "rs_1:summary:0".to_owned()
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
            },
        ]
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
                id: "resp-thinking-done".to_owned()
            },
            AiStreamEvent::ThinkingStart {
                id: "rs_done:summary:0".to_owned()
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
                id: "resp-interleaved-thinking".to_owned()
            },
            AiStreamEvent::ThinkingStart {
                id: "rs_1:summary:0".to_owned()
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
                id: "rs_2:summary:1".to_owned()
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
                id: "resp-shared-thinking-item".to_owned()
            },
            AiStreamEvent::ThinkingStart {
                id: "rs_item:summary:0".to_owned()
            },
            AiStreamEvent::ThinkingDelta {
                text: "First".to_owned()
            },
            AiStreamEvent::ThinkingEnd {
                signature: None,
                redacted: false,
            },
            AiStreamEvent::ThinkingStart {
                id: "rs_item:summary:1".to_owned()
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
                id: "resp-shared-output-index".to_owned()
            },
            AiStreamEvent::ThinkingStart {
                id: "rs_item:output:0:summary:0".to_owned()
            },
            AiStreamEvent::ThinkingDelta {
                text: "Output zero".to_owned()
            },
            AiStreamEvent::ThinkingEnd {
                signature: None,
                redacted: false,
            },
            AiStreamEvent::ThinkingStart {
                id: "rs_item:output:1:summary:0".to_owned()
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
                id: "resp-thinking-correction".to_owned()
            },
            AiStreamEvent::ThinkingStart {
                id: "rs_corrected:summary:0".to_owned()
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
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-failed" } }),
        json!({
            "type": "response.failed",
            "response": { "status": "failed" }
        }),
    ])]);
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
async fn anthropic_messages_client_posts_messages_payload_and_streams_events() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "message_start", "message": { "id": "msg-1" } }),
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
            "usage": {
                "input_tokens": 11,
                "output_tokens": 3,
                "cache_read_input_tokens": 8,
                "cache_creation_input_tokens": 2
            }
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
                id: "msg-1".to_owned()
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
                })
            },
        ]
    );

    let sent = server.requests().pop().unwrap();
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
    let cache_control = json!({ "type": "ephemeral", "ttl": "1h" });
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
                id: "msg-thinking-stream".to_owned()
            },
            AiStreamEvent::ThinkingStart {
                id: "thinking:0".to_owned()
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

    assert_eq!(
        events,
        vec![
            AiStreamEvent::MessageStart {
                id: "google-generative-ai".to_owned()
            },
            AiStreamEvent::TextDelta {
                text: "hi ".to_owned()
            },
            AiStreamEvent::ToolCallStart {
                id: "read_file".to_owned(),
                name: "read_file".to_owned()
            },
            AiStreamEvent::ToolCallArgsDelta {
                id: "read_file".to_owned(),
                json_fragment: "{\"path\":\"Cargo.toml\"}".to_owned()
            },
            AiStreamEvent::ToolCallEnd {
                id: "read_file".to_owned(),
                raw_arguments: r#"{"path":"Cargo.toml"}"#.to_owned()
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::ToolUse,
                usage: Some(neo_ai::TokenUsage {
                    input_tokens: 9,
                    output_tokens: 4,
                    input_cache_read_tokens: 0,
                    input_cache_write_tokens: 0,
                })
            },
        ]
    );

    let sent = server.requests().pop().unwrap();
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
    let client = GoogleGenerativeAiClient::new(server.url, "test-key");

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
    let function_response = &sent.body["contents"][2]["parts"][0]["functionResponse"];
    assert_eq!(function_response["name"], "call-1");
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
                id: "google-generative-ai".to_owned()
            },
            AiStreamEvent::ThinkingStart {
                id: "google-thought:0".to_owned()
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
                id: "google-generative-ai".to_owned()
            },
            AiStreamEvent::TextDelta {
                text: "plain signed text".to_owned()
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::EndTurn,
                usage: None,
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

#[tokio::test]
async fn openai_compatible_body_error_respects_terminal_state() {
    let terminal = concat!(
        "data: {\"id\":\"chatcmpl-terminal\",\"choices\":[{\"delta\":{\"content\":\"done\"},",
        "\"finish_reason\":\"stop\"}]}\n\n"
    );
    let incomplete = concat!(
        "data: {\"id\":\"chatcmpl-incomplete\",\"choices\":[{\"delta\":{",
        "\"content\":\"partial\"}}]}\n\n"
    );
    let server = MockServer::start(vec![
        truncated_sse_response(terminal),
        truncated_sse_response(incomplete),
    ]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");

    let completed = client
        .stream_chat(request(ApiKind::OpenAi))
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
        .stream_chat(request(ApiKind::OpenAi))
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
