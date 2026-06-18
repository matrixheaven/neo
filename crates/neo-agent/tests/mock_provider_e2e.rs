use std::{
    collections::BTreeMap,
    fmt::Write as _,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_neo"));
    command.env("HOME", isolated_home_path());
    command
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

/// Each test thread gets its own stable isolated home directory so that
/// multiple `neo()` calls within the same test share the same sessions root.
fn isolated_home_path() -> std::path::PathBuf {
    thread_local! {
        static HOME: std::cell::OnceCell<std::path::PathBuf> = std::cell::OnceCell::new();
    }
    HOME.with(|cell| {
        cell.get_or_init(|| {
            static NEXT_HOME_ID: AtomicU64 = AtomicU64::new(0);
            let id = NEXT_HOME_ID.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos();
            std::env::temp_dir().join(format!("neo-e2e-home-{nanos}-{id}"))
        })
        .clone()
    })
}

fn write_config(temp: &TempDir, content: &str) {
    std::fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    std::fs::write(temp.path().join(".neo/config.toml"), content).expect("write config");
}

fn write_api_base_config(temp: &TempDir, api_base: &str) {
    write_config(temp, &format!(r#"api_base = "{api_base}""#));
}

fn write_trust_store(home: &std::path::Path, project: &std::path::Path, trusted: bool) {
    let canonical = project.canonicalize().expect("canonicalize project");
    let store_dir = home.join(".neo");
    std::fs::create_dir_all(&store_dir).expect("create .neo");
    std::fs::write(
        store_dir.join("trust.json"),
        json!({ canonical.to_str().expect("utf8 project path"): trusted }).to_string(),
    )
    .expect("write trust store");
}

fn session_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    // Sessions are stored under the isolated home in workspace-scoped bucket dirs.
    let home_sessions = isolated_home_path().join(".neo").join("sessions");
    let mut entries = Vec::new();
    collect_jsonl_recursive(&home_sessions, &mut entries);
    // Also check project-local legacy layout.
    collect_jsonl_recursive(&root.join(".neo/sessions"), &mut entries);
    entries.sort();
    entries
}

fn collect_jsonl_recursive(dir: &std::path::Path, results: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_recursive(&path, results);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            results.push(path);
        }
    }
}

fn model_tool_names(body: &Value) -> Vec<&str> {
    let mut names = body["tools"]
        .as_array()
        .expect("model request tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect::<Vec<_>>();
    names.sort_unstable();
    names
}

fn write_echo_extension_at(extension: &std::path::Path) -> std::path::PathBuf {
    write_named_echo_extension_at(extension, "echo")
}

fn write_named_echo_extension_at(
    extension: &std::path::Path,
    extension_id: &str,
) -> std::path::PathBuf {
    std::fs::create_dir_all(extension).expect("create extension");
    let log = extension.join("extension-calls.jsonl");
    let script = extension.join("echo.py");
    std::fs::write(
        &script,
        format!(
            r#"
import json
import sys

log_path = {log_path}

for line in sys.stdin:
    message = json.loads(line)
    with open(log_path, "a", encoding="utf-8") as log:
        log.write(json.dumps(message, sort_keys=True) + "\n")
    method = message["method"]
    if method == "tools.list":
        result = [{{
            "name": "echo",
            "description": "Echo text from the Neo extension",
            "input_schema": {{
                "type": "object",
                "properties": {{"text": {{"type": "string"}}}},
                "required": ["text"]
            }},
            "method": "tool.echo"
        }}]
    elif method == "tool.echo":
        result = {{
            "content": "extension echo: " + message["params"]["text"],
            "details": {{"source": "extension-test"}}
        }}
    else:
        print(json.dumps({{
            "type": "response",
            "id": message["id"],
            "error": {{"code": "method_not_found", "message": method}}
        }}), flush=True)
        continue
    print(json.dumps({{"type": "response", "id": message["id"], "result": result}}), flush=True)
"#,
            log_path =
                serde_json::to_string(log.to_str().expect("utf8 log path")).expect("log path json")
        ),
    )
    .expect("write extension script");
    std::fs::write(
        extension.join("neo-extension.toml"),
        format!(
            r#"
id = "{extension_id}"
name = "Echo"
version = "0.1.0"

[runner]
command = "python3"
args = [{script}]
"#,
            extension_id = extension_id,
            script = serde_json::to_string(&script).expect("script path json")
        ),
    )
    .expect("write extension manifest");
    log
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

fn sse_response(events: &[Value]) -> String {
    let mut body = String::new();
    for event in events {
        write!(&mut body, "data: {event}\n\n").expect("write SSE event");
    }
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
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
    let body = if body_bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(body_bytes).expect("json body")
    };

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

#[test]
fn run_text_uses_production_openai_responses_adapter_against_mock_provider() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-run-text",
        "hello from mock",
    )]);
    write_api_base_config(&temp, &server.url);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "hello", "neo"]);

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
fn run_emits_jsonl_events_from_mock_provider_without_fake_output() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-run", "run reply")]);
    write_api_base_config(&temp, &server.url);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "hello"]);

    let stdout = run(command);

    assert!(stdout.contains("run reply"));
    assert!(!stdout.contains("fake response"));
    assert!(!stdout.contains("placeholder"));
    let sessions = session_files(temp.path());
    assert_eq!(sessions.len(), 1);
}

#[test]
fn run_output_json_emits_stable_typed_events_from_mock_provider() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-json", "json reply")]);
    write_config(
        &temp,
        &format!(
            r#"
api_base = "{}"

[defaults]
mode = "json"
"#,
            server.url
        ),
    );

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "hello"]);

    let stdout = run(command);

    assert!(stdout.contains("\"type\":\"agent_start\""));
    assert!(stdout.contains("\"type\":\"message_start\""));
    assert!(stdout.contains("\"type\":\"message_end\""));
    assert!(stdout.contains("json reply"));
    assert!(stdout.contains("\"type\":\"agent_end\""));
}

#[test]
fn run_output_json_emits_thinking_content_events_from_mock_provider() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_reasoning_response_sse("resp-think")]);
    write_config(
        &temp,
        &format!(
            r#"
api_base = "{}"
default_provider = "openai"
default_model = "reasoning-model"
model_catalogs = [".neo/models.json"]

[runtime]
reasoning_effort = "high"

[defaults]
mode = "json"
"#,
            server.url
        ),
    );
    std::fs::write(
        temp.path().join(".neo/models.json"),
        r#"
{
  "default": { "provider": "openai", "model": "reasoning-model" },
  "models": [
    {
      "provider": "openai",
      "model": "reasoning-model",
      "api": "OpenAiResponses",
      "capabilities": {
        "streaming": true,
        "tools": true,
        "images": false,
        "reasoning": true,
        "embeddings": false,
        "max_context_tokens": 128000
      }
    }
  ]
}
"#,
    )
    .expect("write model catalog");

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "think"]);

    let stdout = run(command);

    assert!(stdout.contains("thinking"));
    assert!(stdout.contains("done thinking"));
}

fn openai_reasoning_response_sse(id: &str) -> String {
    sse_response(&[
        json!({ "type": "response.created", "response": { "id": id } }),
        json!({
            "type": "response.reasoning_summary_text.added",
            "item_id": "think-1",
            "summary": [{"type": "summary_text", "text": "thinking"}]
        }),
        json!({ "type": "response.output_text.delta", "delta": "done thinking" }),
        json!({
            "type": "response.completed",
            "response": {
                "status": "completed",
                "usage": { "input_tokens": 7, "output_tokens": 3 }
            }
        }),
    ])
}

#[test]
fn run_continue_flag_uses_latest_session_in_stable_json_output() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![
        openai_response_sse("resp-cont-1", "first reply"),
        openai_response_sse("resp-cont-title", "title"),
        openai_response_sse("resp-cont-2", "second reply"),
    ]);
    write_config(
        &temp,
        &format!(
            r#"
api_base = "{}"

[defaults]
mode = "json"
"#,
            server.url
        ),
    );

    let mut first = neo();
    first
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "first prompt"]);
    run(first);
    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut second = neo();
    second
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["--continue", "run", "second prompt"]);
    let stdout = run(second);

    assert!(stdout.contains("second reply"));
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    let input = requests[2].body["input"]
        .as_array()
        .expect("input messages");
    let roles = input
        .iter()
        .map(|message| message["role"].as_str().expect("role"))
        .collect::<Vec<_>>();
    assert_eq!(roles, vec!["user", "assistant", "user"]);
    assert_eq!(input[0]["content"], "first prompt");
    assert_eq!(input[2]["content"], "second prompt");
}

#[test]
fn run_text_no_session_flag_runs_without_creating_session_files() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-no-session", "ephemeral")]);
    write_api_base_config(&temp, &server.url);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["--no-session", "run", "--output", "text", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "ephemeral\n");
    assert!(session_files(temp.path()).is_empty());
}

#[test]
fn run_text_models_scope_selects_first_matching_runtime_model() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-model-scope", "scoped")]);
    write_config(
        &temp,
        &format!(
            r#"
api_base = "{}"
model_catalogs = [".neo/models.json"]
model_scope = ["scoped"]
"#,
            server.url
        ),
    );
    std::fs::write(
        temp.path().join(".neo/models.json"),
        r#"
{
  "models": [
    {
      "provider": "openai",
      "model": "scoped-runtime-model",
      "api": "OpenAiResponses",
      "capabilities": {
        "streaming": true,
        "tools": true,
        "images": false,
        "reasoning": false,
        "embeddings": false,
        "max_context_tokens": 128000
      }
    }
  ]
}
"#,
    )
    .expect("write model catalog");

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "scoped\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["model"], "scoped-runtime-model");
}

#[test]
fn run_text_applies_project_runtime_generation_options_to_provider_request() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-opts", "opts")]);
    write_config(
        &temp,
        &format!(
            r#"
api_base = "{}"

[runtime]
temperature = 0.35
max_tokens = 512
"#,
            server.url
        ),
    );

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "hello"]);

    run(command);

    let requests = server.requests();
    assert_eq!(requests[0].body["temperature"], 0.35);
    assert_eq!(requests[0].body["max_output_tokens"], 512);
}

#[test]
fn run_text_continue_flag_replays_latest_session_and_appends_turn() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![
        openai_response_sse("resp-run-text-cont-1", "first"),
        openai_response_sse("resp-run-text-cont-title", "title"),
        openai_response_sse("resp-run-text-cont-2", "second"),
    ]);
    write_api_base_config(&temp, &server.url);

    let mut first = neo();
    first
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "first"]);
    run(first);
    std::thread::sleep(std::time::Duration::from_millis(200));

    let mut second = neo();
    second
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["--continue", "run", "--output", "text", "second"]);
    let stdout = run(second);

    assert_eq!(stdout, "second\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    let input = requests[2].body["input"]
        .as_array()
        .expect("input messages");
    let roles = input
        .iter()
        .map(|message| message["role"].as_str().expect("role"))
        .collect::<Vec<_>>();
    assert_eq!(roles, vec!["user", "assistant", "user"]);
    assert_eq!(input[0]["content"], "first");
    assert_eq!(input[2]["content"], "second");
}

#[test]
fn run_merges_piped_stdin_with_cli_prompt() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-stdin", "merged")]);
    write_api_base_config(&temp, &server.url);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "extra"]);

    let stdout = run_with_stdin(command, "piped");

    assert!(stdout.contains("merged"));
    let requests = server.requests();
    assert_eq!(requests[0].body["input"][0]["content"], "piped\nextra");
}

#[test]
fn run_text_merges_piped_stdin_with_cli_prompt() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-run-text-stdin", "merged")]);
    write_api_base_config(&temp, &server.url);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "extra"]);

    let stdout = run_with_stdin(command, "piped");

    assert_eq!(stdout, "merged\n");
    let requests = server.requests();
    assert_eq!(requests[0].body["input"][0]["content"], "piped\nextra");
}

#[test]
fn root_run_text_flag_expands_workspace_relative_file_prompt_args() {
    let temp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(temp.path().join("docs")).expect("create docs");
    std::fs::write(temp.path().join("docs/context.txt"), "root file context\n")
        .expect("write prompt file");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-root-run-text-file",
        "root file",
    )]);
    write_api_base_config(&temp, &server.url);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "@docs/context.txt", "summarize"]);

    let stdout = run(command);

    assert_eq!(stdout, "root file\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "root file context\nsummarize"
    );
}

#[test]
fn run_expands_project_prompt_template_before_json_output() {
    let temp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".neo/prompts")).expect("create prompts");
    std::fs::write(
        temp.path().join(".neo/prompts/review.md"),
        "Review the following code: $@",
    )
    .expect("write template");
    let server = MockSseServer::start(vec![openai_response_sse("resp-template", "reviewed")]);
    write_config(
        &temp,
        &format!(
            r#"
api_base = "{}"
prompt_templates = ["review"]

[defaults]
mode = "json"
"#,
            server.url
        ),
    );

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "fn main()"]);

    let stdout = run(command);

    assert!(stdout.contains("reviewed"));
    let requests = server.requests();
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "Review the following code: fn main()"
    );
}

#[test]
fn run_text_expands_project_prompt_template_with_arguments() {
    let temp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".neo/prompts")).expect("create prompts");
    std::fs::write(
        temp.path().join(".neo/prompts/review.md"),
        "Review the following code: $@",
    )
    .expect("write template");
    let server = MockSseServer::start(vec![openai_response_sse("resp-slash", "slash")]);
    write_api_base_config(&temp, &server.url);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "/review", "fn main()"]);

    let stdout = run(command);

    assert_eq!(stdout, "slash\n");
    let requests = server.requests();
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "Review the following code: fn main()"
    );
}

#[test]
fn run_text_includes_project_system_prompt_file_before_user_message() {
    let temp = TempDir::new().expect("tempdir");
    write_config(&temp, "default_model = \"gpt-4.1\"");
    write_trust_store(&isolated_home_path(), temp.path(), true);
    std::fs::write(
        temp.path().join(".neo/SYSTEM.md"),
        "You are a test assistant.",
    )
    .expect("write system prompt");
    let server = MockSseServer::start(vec![openai_response_sse("resp-sys", "sys")]);
    write_config(
        &temp,
        &format!(
            r#"
api_base = "{}"
"#,
            server.url
        ),
    );

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "hello"]);

    run(command);

    let requests = server.requests();
    let input = requests[0].body["input"].as_array().expect("input");
    assert_eq!(input[0]["role"], "system");
    assert_eq!(input[0]["content"], "You are a test assistant.");
    assert_eq!(input[1]["role"], "user");
}

#[test]
fn run_text_loads_project_context_after_persisted_trust() {
    let temp = TempDir::new().expect("tempdir");
    write_config(&temp, "");
    write_trust_store(&isolated_home_path(), temp.path(), true);
    std::fs::write(temp.path().join("AGENTS.md"), "Project context: use Rust.")
        .expect("write agents");
    let server = MockSseServer::start(vec![openai_response_sse("resp-trust", "trusted")]);
    write_config(
        &temp,
        &format!(
            r#"
api_base = "{}"
"#,
            server.url
        ),
    );

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "hello"]);

    run(command);

    let requests = server.requests();
    let system = &requests[0].body["input"][0];
    assert_eq!(system["role"], "system");
    let content = system["content"].as_str().expect("content");
    assert!(content.contains("Project context: use Rust."));
}

#[test]
fn run_text_yolo_skips_project_context_even_after_persisted_trust() {
    let temp = TempDir::new().expect("tempdir");
    write_config(&temp, "");
    write_trust_store(&isolated_home_path(), temp.path(), true);
    std::fs::write(temp.path().join("AGENTS.md"), "Project context: use Rust.")
        .expect("write agents");
    let server = MockSseServer::start(vec![openai_response_sse("resp-yolo", "yolo")]);
    write_config(
        &temp,
        &format!(
            r#"
api_base = "{}"
"#,
            server.url
        ),
    );

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["--yolo", "run", "--output", "text", "hello"]);

    run(command);

    let requests = server.requests();
    let has_context = requests[0].body["input"]
        .as_array()
        .expect("input")
        .iter()
        .any(|message| {
            message["role"] == "system"
                && message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("Project context: use Rust."))
        });
    assert!(!has_context);
}

#[test]
fn run_text_rejects_prompt_file_args_outside_workspace() {
    let temp = TempDir::new().expect("tempdir");
    let outside = TempDir::new().expect("outside tempdir");
    std::fs::write(outside.path().join("secret.txt"), "secret").expect("write outside file");
    let server = MockSseServer::start(vec![openai_response_sse("resp-reject", "no")]);
    write_api_base_config(&temp, &server.url);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args([
            "run",
            "--output",
            "text",
            &format!("@{}/secret.txt", outside.path().display()),
        ]);

    let output = command.output().expect("neo command should run");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("prompt file must stay inside project directory"));
}

#[test]
fn run_text_registers_enabled_extension_tool_in_model_request() {
    let temp = TempDir::new().expect("tempdir");
    write_echo_extension_at(&temp.path().join(".neo/extensions/echo"));
    let server = MockSseServer::start(vec![openai_response_sse("resp-ext", "extension ready")]);
    write_api_base_config(&temp, &server.url);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "list tools"]);

    let stdout = run(command);

    assert_eq!(stdout, "extension ready\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let tool_names = model_tool_names(&requests[0].body);
    assert!(tool_names.contains(&"extension__echo__echo"));
}

#[test]
fn run_text_registers_enabled_stdio_mcp_tools_from_project_config() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-mcp", "mcp tools listed")]);
    let mcp_fixture = temp.path().join("mcp-fixture.py");
    std::fs::write(&mcp_fixture, MCP_STDIO_FIXTURE).expect("write MCP fixture");
    write_config(
        &temp,
        &format!(
            r#"
api_base = "{}"

[[mcp.servers]]
id = "docs-server"
enabled = true
transport = "stdio"
command = "python3"
args = ["-u", "{}"]
"#,
            server.url,
            mcp_fixture.display()
        ),
    );

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "show", "tools"]);
    let stdout = run(command);

    assert_eq!(stdout, "mcp tools listed\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/responses");
    let tool_names = model_tool_names(&requests[0].body);
    assert!(
        tool_names.contains(&"mcp__docs_server__docs_search"),
        "model tools should include configured MCP stdio tools: {tool_names:?}"
    );
}

const MCP_STDIO_FIXTURE: &str = r#"
import json
import sys

for line in sys.stdin:
    request = json.loads(line)
    method = request["method"]
    if method == "initialize":
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "fixture", "version": "0.1.0"},
                "capabilities": {"tools": {}},
            },
        }
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "tools": [
                    {
                        "name": "docs-search",
                        "description": "Search project docs",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"query": {"type": "string"}},
                            "required": ["query"],
                        },
                    }
                ]
            },
        }
    elif method == "tools/call":
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "content": [{"type": "text", "text": "ok"}],
                "isError": False,
            },
        }
    else:
        response = {
            "jsonrpc": "2.0",
            "id": request.get("id"),
            "error": {"code": -32601, "message": f"unknown method {method}"},
        }
    print(json.dumps(response), flush=True)
"#;
