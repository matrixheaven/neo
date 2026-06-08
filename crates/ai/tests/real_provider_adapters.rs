use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use futures::StreamExt;
use neo_ai::{
    AiStreamEvent, ApiKind, ChatMessage, ChatRequest, ContentPart, ImageData, ModelCapabilities,
    ModelClient, ModelSpec, ProviderId, ReasoningEffort, RequestOptions, StopReason, ToolSpec,
    providers::{
        anthropic::AnthropicMessagesClient, google::GoogleGenerativeAiClient,
        openai_responses::OpenAiResponsesClient,
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
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n{}",
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
        .stream_chat(request(ApiKind::OpenAiResponses))
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
                arguments: json!({ "path": "Cargo.toml" })
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::ToolUse,
                usage: Some(neo_ai::TokenUsage {
                    input_tokens: 9,
                    output_tokens: 4,
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
async fn openai_responses_client_serializes_reasoning_effort_without_encrypted_handoff() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-reasoning" } }),
        json!({
            "type": "response.completed",
            "response": { "status": "completed" }
        }),
    ])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");
    let mut request = request(ApiKind::OpenAiResponses);
    request.options.reasoning_effort = Some(ReasoningEffort::High);

    client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    assert_eq!(sent.body["reasoning"]["effort"], "high");
    assert_eq!(sent.body["reasoning"]["summary"], "auto");
    assert!(
        sent.body.get("include").is_none(),
        "Neo must not request encrypted reasoning continuity until it can persist and replay it"
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
        .stream_chat(request(ApiKind::OpenAiResponses))
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
        .stream_chat(request(ApiKind::OpenAiResponses))
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
        .stream_chat(request(ApiKind::OpenAiResponses))
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
        .stream_chat(request(ApiKind::OpenAiResponses))
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
        .stream_chat(request(ApiKind::OpenAiResponses))
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
        .stream_chat(request(ApiKind::OpenAiResponses))
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
            ApiKind::OpenAiResponses,
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
            "usage": { "input_tokens": 11, "output_tokens": 3 }
        }),
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
                arguments: json!({ "path": "Cargo.toml" })
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::ToolUse,
                usage: Some(neo_ai::TokenUsage {
                    input_tokens: 11,
                    output_tokens: 3,
                })
            },
        ]
    );

    let sent = server.requests().pop().unwrap();
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.path, "/messages");
    assert_eq!(sent.headers.get("x-api-key").unwrap(), "test-key");
    assert_eq!(sent.headers.get("anthropic-version").unwrap(), "2023-06-01");
    assert_eq!(sent.body["model"], "model-test");
    assert_eq!(sent.body["stream"], true);
    assert_eq!(sent.body["max_tokens"], 64);
    assert_eq!(sent.body["tools"][0]["name"], "read_file");
    assert_eq!(sent.body["messages"][0]["role"], "user");
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
                arguments: json!({ "path": "Cargo.toml" })
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::ToolUse,
                usage: Some(neo_ai::TokenUsage {
                    input_tokens: 9,
                    output_tokens: 4,
                })
            },
        ]
    );

    let sent = server.requests().pop().unwrap();
    assert_eq!(sent.method, "POST");
    assert_eq!(
        sent.path,
        "/models/model-test:streamGenerateContent?alt=sse&key=test-key"
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
