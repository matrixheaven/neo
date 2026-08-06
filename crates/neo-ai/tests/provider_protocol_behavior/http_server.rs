use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use neo_ai::{
    ApiKind, ChatMessage, ChatRequest, ContentPart, ImageData, ModelCapabilities, ModelSpec,
    ProviderId, RequestOptions, ToolCall, ToolSpec,
};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: Value,
}

pub struct MockServer {
    pub url: String,
    pub requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

impl MockServer {
    pub fn start(responses: Vec<String>) -> Self {
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

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub fn start_unfinished_chunked_error(body: Vec<u8>) -> Self {
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

pub fn read_http_request(socket: &mut TcpStream) -> RecordedRequest {
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

pub fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

pub fn sse_response(events: &[Value]) -> String {
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

pub fn truncated_sse_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len() + 1,
        body
    )
}

pub fn status_response(status: u16) -> String {
    format!("HTTP/1.1 {status} Test\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
}

pub fn json_response(value: &Value) -> String {
    let body = value.to_string();
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

pub fn request(api: ApiKind) -> ChatRequest {
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

pub fn tool_result_request(api: ApiKind, is_error: bool) -> ChatRequest {
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

pub fn multi_tool_result_request(api: ApiKind) -> ChatRequest {
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

pub fn image_request(api: ApiKind, image: ImageData) -> ChatRequest {
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

pub fn assistant_image_request(api: ApiKind, image: ImageData) -> ChatRequest {
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
