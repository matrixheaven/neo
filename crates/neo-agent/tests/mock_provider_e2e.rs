use std::{
    collections::BTreeMap,
    fmt::Write as _,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

use serde_json::{Value, json};
use tempfile::TempDir;

#[derive(Debug, Clone)]
struct RecordedRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Value,
}

struct MockSseServer {
    url: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

impl MockSseServer {
    fn start(responses: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock provider");
        let url = format!("http://{}", listener.local_addr().expect("local addr"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured_requests = Arc::clone(&requests);

        std::thread::spawn(move || {
            for response in responses {
                let (mut socket, _) = listener.accept().expect("accept provider request");
                let request = read_http_request(&mut socket);
                captured_requests
                    .lock()
                    .expect("lock requests")
                    .push(request);
                socket
                    .write_all(response.as_bytes())
                    .expect("write provider response");
            }
        });

        Self { url, requests }
    }

    fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("lock requests").clone()
    }
}

fn neo() -> Command {
    Command::new(env!("CARGO_BIN_EXE_neo"))
}

fn run(mut command: Command) -> String {
    let output = command.output().expect("neo command should run");
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("stdout should be utf8")
}

fn run_with_stdin(mut command: Command, stdin: &str) -> String {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("neo command should spawn");
    child
        .stdin
        .as_mut()
        .expect("stdin should be piped")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("neo command should run");
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("stdout should be utf8")
}

#[test]
fn print_uses_production_openai_responses_adapter_against_mock_provider() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-print", "hello from mock")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["print", "hello", "neo"]);

    let stdout = run(command);

    assert_eq!(stdout, "hello from mock\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let sent = &requests[0];
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.path, "/responses");
    assert_eq!(
        sent.headers.get("authorization").map(String::as_str),
        Some("Bearer test-key")
    );
    assert_eq!(sent.body["model"], "gpt-4.1");
    assert_eq!(sent.body["stream"], true);
    assert_eq!(sent.body["input"][0]["role"], "user");
    assert_eq!(sent.body["input"][0]["content"], "hello neo");

    let sessions = session_files(temp.path());
    assert_eq!(sessions.len(), 1);
    let content = std::fs::read_to_string(&sessions[0]).expect("read session");
    assert!(content.contains("\"User\""));
    assert!(content.contains("\"Assistant\""));
    assert!(content.contains("hello from mock"));
    assert!(!content.contains("fake response"));
    assert!(!content.contains("placeholder"));
}

#[test]
fn print_merges_piped_stdin_with_cli_prompt() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-stdin", "merged")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["print", "summarize"]);

    let stdout = run_with_stdin(command, "stdin context\nsecond line\n");

    assert_eq!(stdout, "merged\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "stdin context\nsecond line\nsummarize"
    );
}

#[test]
fn print_expands_workspace_relative_file_prompt_args() {
    let temp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(temp.path().join("docs")).expect("create docs");
    std::fs::write(
        temp.path().join("docs/context.txt"),
        "workspace context\nsecond line\n",
    )
    .expect("write prompt file");
    let server = MockSseServer::start(vec![openai_response_sse("resp-file", "expanded")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["print", "@docs/context.txt", "summarize"]);

    let stdout = run(command);

    assert_eq!(stdout, "expanded\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "workspace context\nsecond line\nsummarize"
    );
}

#[test]
fn run_expands_workspace_relative_file_prompt_args() {
    let temp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(temp.path().join("docs")).expect("create docs");
    std::fs::write(temp.path().join("docs/context.txt"), "run context\n")
        .expect("write prompt file");
    let server = MockSseServer::start(vec![openai_response_sse("resp-run-file", "jsonl file")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["run", "@docs/context.txt", "continue"]);

    let stdout = run(command);

    assert!(stdout.contains("\"TextDelta\":{\"turn\":1,\"text\":\"jsonl file\"}"));
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "run context\ncontinue"
    );
}

#[test]
fn print_rejects_prompt_file_args_outside_workspace() {
    let temp = TempDir::new().expect("tempdir");
    let outside = TempDir::new().expect("outside tempdir");
    std::fs::write(outside.path().join("escape.txt"), "outside context\n")
        .expect("write outside prompt file");

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg("http://127.0.0.1:9")
        .args([
            "print",
            &format!("@{}", outside.path().join("escape.txt").display()),
            "summarize",
        ]);

    let output = command.output().expect("neo command should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf8");
    assert!(stderr.contains("prompt file must stay inside project directory"));
}

#[test]
fn print_uses_provider_specific_api_base_from_project_config() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-provider-base",
        "provider base configured",
    )]);
    std::fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    std::fs::write(
        temp.path().join(".neo/config.toml"),
        format!(
            r#"
[providers.openai]
api_base = "{}"
api_key_env = "PROJECT_OPENAI_KEY"
"#,
            server.url
        ),
    )
    .expect("write config");

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("PROJECT_OPENAI_KEY", "test-key")
        .env_remove("OPENAI_API_KEY")
        .args(["print", "provider", "base"]);

    let stdout = run(command);

    assert_eq!(stdout, "provider base configured\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/responses");
    assert_eq!(
        requests[0].headers.get("authorization").map(String::as_str),
        Some("Bearer test-key")
    );
    assert_eq!(requests[0].body["input"][0]["content"], "provider base");
}

#[test]
fn print_applies_project_runtime_generation_options_to_provider_request() {
    let temp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    std::fs::write(
        temp.path().join(".neo/config.toml"),
        r#"
[runtime]
temperature = 0.25
max_tokens = 321
reasoning_effort = "high"
"#,
    )
    .expect("write config");
    let server = MockSseServer::start(vec![openai_response_sse("resp-runtime", "configured")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["print", "runtime", "options"]);

    let stdout = run(command);

    assert_eq!(stdout, "configured\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["temperature"], 0.25);
    assert_eq!(requests[0].body["max_output_tokens"], 321);
    assert_eq!(requests[0].body["reasoning"]["effort"], "high");
}

#[test]
fn run_merges_piped_stdin_with_cli_prompt() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-run-stdin", "jsonl merged")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["run", "continue"]);

    let stdout = run_with_stdin(command, "piped task\n");

    assert!(stdout.contains("\"TextDelta\":{\"turn\":1,\"text\":\"jsonl merged\"}"));
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "piped task\ncontinue"
    );
}

#[test]
fn run_emits_jsonl_events_from_mock_provider_without_fake_output() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-run", "jsonl text")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["run", "stream", "events"]);

    let stdout = run(command);

    assert!(stdout.contains("\"MessageStarted\":{\"turn\":1,\"id\":\"resp-run\"}"));
    assert!(stdout.contains("\"TextDelta\":{\"turn\":1,\"text\":\"jsonl text\"}"));
    assert!(stdout.contains("\"TurnFinished\":{\"turn\":1,\"stop_reason\":\"EndTurn\"}"));
    assert!(!stdout.contains("fake response"));
    assert!(!stdout.contains("placeholder"));

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["input"][0]["content"], "stream events");
}

#[test]
fn print_approve_allows_ask_file_write_tool_and_continues_agent_loop() {
    let temp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    std::fs::write(
        temp.path().join(".neo/config.toml"),
        r#"
[permissions]
file_read = "Allow"
file_write = "Ask"
shell = "Deny"
tool = "Allow"
"#,
    )
    .expect("write config");
    let server = MockSseServer::start(vec![
        openai_tool_call_sse(
            "resp-approve-1",
            "call-write",
            "write",
            &json!({
                "path": "approved.txt",
                "content": "approved by flag"
            }),
        ),
        openai_response_sse("resp-approve-2", "wrote it"),
    ]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--approve", "print", "write", "file"]);

    let stdout = run(command);

    assert_eq!(stdout, "wrote it\n");
    assert_eq!(
        std::fs::read_to_string(temp.path().join("approved.txt")).expect("written file"),
        "approved by flag"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].body["input"][2]["type"], "function_call_output");
    assert_eq!(requests[1].body["input"][2]["call_id"], "call-write");
    assert!(
        requests[1].body["input"][2]["output"]
            .as_str()
            .expect("tool output")
            .contains("wrote")
    );
}

fn session_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut entries = std::fs::read_dir(root.join(".neo/sessions"))
        .expect("read sessions")
        .map(|entry| entry.expect("session entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

fn openai_response_sse(id: &str, text: &str) -> String {
    sse_response(&[
        json!({ "type": "response.created", "response": { "id": id } }),
        json!({ "type": "response.output_text.delta", "delta": text }),
        json!({
            "type": "response.completed",
            "response": {
                "status": "completed",
                "usage": { "input_tokens": 7, "output_tokens": 3 }
            }
        }),
    ])
}

fn openai_tool_call_sse(id: &str, call_id: &str, name: &str, arguments: &Value) -> String {
    let arguments = arguments.to_string();
    sse_response(&[
        json!({ "type": "response.created", "response": { "id": id } }),
        json!({
            "type": "response.output_item.added",
            "item": { "type": "function_call", "id": "item-1", "call_id": call_id, "name": name }
        }),
        json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "item-1",
            "delta": arguments
        }),
        json!({
            "type": "response.completed",
            "response": {
                "status": "completed",
                "usage": { "input_tokens": 7, "output_tokens": 3 }
            }
        }),
    ])
}

fn sse_response(events: &[Value]) -> String {
    let mut body = String::new();
    for event in events {
        write!(&mut body, "data: {event}\n\n").expect("write SSE event");
    }
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n{}",
        body.len(),
        body
    )
}

fn read_http_request(socket: &mut TcpStream) -> RecordedRequest {
    let mut buffer = Vec::new();
    let mut temp = [0_u8; 1024];
    let header_end;

    loop {
        let read = socket.read(&mut temp).expect("read request");
        assert_ne!(read, 0, "client closed before sending headers");
        buffer.extend_from_slice(&temp[..read]);
        if let Some(index) = find_header_end(&buffer) {
            header_end = index;
            break;
        }
    }

    let headers_raw = String::from_utf8(buffer[..header_end].to_vec()).expect("utf8 headers");
    let mut lines = headers_raw.split("\r\n");
    let request_line = lines.next().expect("request line");
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().expect("method").to_owned();
    let path = request_parts.next().expect("path").to_owned();
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
        let read = socket.read(&mut temp).expect("read body");
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..read]);
    }
    let body_bytes = &buffer[body_start..body_start + content_length];
    let body = serde_json::from_slice(body_bytes).expect("json body");

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
