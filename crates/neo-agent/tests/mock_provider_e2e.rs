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
    let home = isolated_home_path();
    // NEO_HOME is the single source of truth for config/skills/prompts/themes.
    command.env("NEO_HOME", &home);
    command.env("HOME", &home);
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
        static HOME: std::cell::OnceCell<std::path::PathBuf> = const { std::cell::OnceCell::new() };
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

/// Write config into the isolated `NEO_HOME` (not the workspace `temp`). Config
/// now lives only under ~/.neo; the `temp` arg is retained for call-site
/// compatibility but ignored.
fn write_config(_temp: &TempDir, content: &str) {
    let config_dir = isolated_home_path();
    std::fs::create_dir_all(&config_dir).expect("create neo home");
    std::fs::write(config_dir.join("config.toml"), content).expect("write config");
}

fn mock_responses_config(base_url: &str) -> String {
    format!(
        r#"
default_provider = "mock"
default_model = "gpt-4.1"

[providers.mock]
type = "openai-responses"
base_url = "{base_url}"
api_key_env = "OPENAI_API_KEY"

[models."mock/gpt-4.1"]
provider = "mock"
model = "gpt-4.1"
capabilities = ["streaming", "tools"]
"#
    )
}

fn write_mock_responses_config(temp: &TempDir, base_url: &str) {
    write_config(temp, &mock_responses_config(base_url));
}

fn write_trust_store(home: &std::path::Path, project: &std::path::Path, trusted: bool) {
    let canonical = project.canonicalize().expect("canonicalize project");
    std::fs::create_dir_all(home).expect("create neo home");
    std::fs::write(
        home.join("trust.json"),
        json!({ canonical.to_str().expect("utf8 project path"): trusted }).to_string(),
    )
    .expect("write trust store");
}

fn session_files(_root: &std::path::Path) -> Vec<std::path::PathBuf> {
    // Sessions are stored under the isolated home in workspace-scoped bucket dirs.
    let home_sessions = isolated_home_path().join("sessions");
    let mut entries = Vec::new();
    collect_jsonl_recursive(&home_sessions, &mut entries);
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

fn assert_model_function_name_safe(name: &str) {
    assert!(
        name.chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'),
        "tool name `{name}` must be safe for production model function-name APIs"
    );
}

fn input_messages(request: &RecordedRequest) -> &[Value] {
    request.body["input"].as_array().expect("input messages")
}

fn input_roles_without_system(request: &RecordedRequest) -> Vec<&str> {
    input_messages(request)
        .iter()
        .filter_map(|message| {
            let role = message["role"].as_str().expect("role");
            (role != "system").then_some(role)
        })
        .collect()
}

fn user_input_contents(request: &RecordedRequest) -> Vec<&str> {
    input_messages(request)
        .iter()
        .filter(|message| message["role"] == "user")
        .map(|message| message["content"].as_str().expect("user content"))
        .collect()
}

fn system_input_contents(request: &RecordedRequest) -> Vec<&str> {
    input_messages(request)
        .iter()
        .filter(|message| message["role"] == "system")
        .filter_map(|message| message["content"].as_str())
        .collect()
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
    write_mock_responses_config(&temp, &server.url);

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
    assert_eq!(user_input_contents(sent), vec!["hello neo"]);

    let sessions = session_files(temp.path());
    assert_eq!(sessions.len(), 1);
    let session_id = sessions[0]
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .expect("session id");
    assert!(session_id.starts_with("session_"));
    assert_eq!(session_id.len(), "session_".len() + 36);
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
    write_mock_responses_config(&temp, &server.url);

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
            "{}\n[defaults]\nmode = \"json\"\n",
            mock_responses_config(&server.url)
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
default_provider = "mock"
default_model = "reasoning-model"

[providers.mock]
type = "openai-responses"
base_url = "{}"
api_key_env = "OPENAI_API_KEY"

[models."mock/reasoning-model"]
provider = "mock"
model = "reasoning-model"
max_context_tokens = 128000
capabilities = ["streaming", "tools", "reasoning"]

[runtime]
reasoning_effort = "high"

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
            "{}\n[defaults]\nmode = \"json\"\n",
            mock_responses_config(&server.url)
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
    assert_eq!(
        input_roles_without_system(&requests[2]),
        vec!["user", "assistant", "user"]
    );
    assert_eq!(
        user_input_contents(&requests[2]),
        vec!["first prompt", "second prompt"]
    );
}

#[test]
fn run_text_no_session_flag_runs_without_creating_session_files() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-no-session", "ephemeral")]);
    write_mock_responses_config(&temp, &server.url);

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
default_provider = "mock"
default_model = "scoped-runtime-model"
model_scope = ["scoped"]

[providers.mock]
type = "openai-responses"
base_url = "{}"
api_key_env = "OPENAI_API_KEY"

[models."mock/scoped-runtime-model"]
provider = "mock"
model = "scoped-runtime-model"
max_context_tokens = 128000
capabilities = ["streaming", "tools"]
"#,
            server.url
        ),
    );

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
            "{}\n[runtime]\n\
temperature = 0.35
max_tokens = 512
",
            mock_responses_config(&server.url)
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
fn run_text_falls_back_to_model_max_output_tokens_when_runtime_max_tokens_unset() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-mout", "mout")]);
    write_config(
        &temp,
        &format!(
            r#"
default_provider = "mock"
default_model = "gpt-4.1"

[providers.mock]
type = "openai-responses"
base_url = "{base_url}"
api_key_env = "OPENAI_API_KEY"

[models."mock/gpt-4.1"]
provider = "mock"
model = "gpt-4.1"
max_output_tokens = 64000
capabilities = ["streaming", "tools"]
"#,
            base_url = server.url,
        ),
    );

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "hello"]);

    run(command);

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    // No [runtime].max_tokens set, so the model-declared max_output_tokens
    // should flow through to the request body.
    assert_eq!(requests[0].body["max_output_tokens"], 64_000);
}

#[test]
fn run_text_runtime_max_tokens_overrides_model_max_output_tokens() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-over", "over")]);
    write_config(
        &temp,
        &format!(
            r#"
default_provider = "mock"
default_model = "gpt-4.1"

[providers.mock]
type = "openai-responses"
base_url = "{base_url}"
api_key_env = "OPENAI_API_KEY"

[models."mock/gpt-4.1"]
provider = "mock"
model = "gpt-4.1"
max_output_tokens = 64000
capabilities = ["streaming", "tools"]

[runtime]
max_tokens = 2048
"#,
            base_url = server.url,
        ),
    );

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "hello"]);

    run(command);

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    // Explicit runtime override wins over the model-declared value.
    assert_eq!(requests[0].body["max_output_tokens"], 2048);
}

#[test]
fn run_text_continue_flag_replays_latest_session_and_appends_turn() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![
        openai_response_sse("resp-run-text-cont-1", "first"),
        openai_response_sse("resp-run-text-cont-title", "title"),
        openai_response_sse("resp-run-text-cont-2", "second"),
    ]);
    write_mock_responses_config(&temp, &server.url);

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
    assert_eq!(
        input_roles_without_system(&requests[2]),
        vec!["user", "assistant", "user"]
    );
    assert_eq!(user_input_contents(&requests[2]), vec!["first", "second"]);
}

#[test]
fn run_merges_piped_stdin_with_cli_prompt() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-stdin", "merged")]);
    write_mock_responses_config(&temp, &server.url);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "extra"]);

    let stdout = run_with_stdin(command, "piped");

    assert!(stdout.contains("merged"));
    let requests = server.requests();
    assert_eq!(user_input_contents(&requests[0]), vec!["piped\nextra"]);
}

#[test]
fn run_text_merges_piped_stdin_with_cli_prompt() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-run-text-stdin", "merged")]);
    write_mock_responses_config(&temp, &server.url);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "extra"]);

    let stdout = run_with_stdin(command, "piped");

    assert_eq!(stdout, "merged\n");
    let requests = server.requests();
    assert_eq!(user_input_contents(&requests[0]), vec!["piped\nextra"]);
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
    write_mock_responses_config(&temp, &server.url);

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
        user_input_contents(&requests[0]),
        vec!["root file context\nsummarize"]
    );
}

#[test]
fn run_expands_project_prompt_template_before_json_output() {
    let temp = TempDir::new().expect("tempdir");
    let prompts_dir = isolated_home_path().join("prompts");
    std::fs::create_dir_all(&prompts_dir).expect("create prompts");
    std::fs::write(
        prompts_dir.join("review.md"),
        "Review the following code: $@",
    )
    .expect("write template");
    let server = MockSseServer::start(vec![openai_response_sse("resp-template", "reviewed")]);
    write_config(
        &temp,
        &format!(
            "prompt_templates = [\"review\"]\n{}\n[defaults]\nmode = \"json\"\n",
            mock_responses_config(&server.url)
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
        user_input_contents(&requests[0]),
        vec!["Review the following code: fn main()"]
    );
}

#[test]
fn run_text_expands_project_prompt_template_with_arguments() {
    let temp = TempDir::new().expect("tempdir");
    let prompts_dir = isolated_home_path().join("prompts");
    std::fs::create_dir_all(&prompts_dir).expect("create prompts");
    std::fs::write(
        prompts_dir.join("review.md"),
        "Review the following code: $@",
    )
    .expect("write template");
    let server = MockSseServer::start(vec![openai_response_sse("resp-slash", "slash")]);
    write_mock_responses_config(&temp, &server.url);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "/review", "fn main()"]);

    let stdout = run(command);

    assert_eq!(stdout, "slash\n");
    let requests = server.requests();
    assert_eq!(
        user_input_contents(&requests[0]),
        vec!["Review the following code: fn main()"]
    );
}

#[test]
fn run_text_includes_project_system_prompt_file_before_user_message() {
    let temp = TempDir::new().expect("tempdir");
    write_trust_store(&isolated_home_path(), temp.path(), true);
    std::fs::write(
        isolated_home_path().join("SYSTEM.md"),
        "You are a test assistant.",
    )
    .expect("write system prompt");
    let server = MockSseServer::start(vec![openai_response_sse("resp-sys", "sys")]);
    write_mock_responses_config(&temp, &server.url);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "hello"]);

    run(command);

    let requests = server.requests();
    assert!(
        system_input_contents(&requests[0])
            .iter()
            .any(|content| content.contains("You are a test assistant.")),
        "expected system prompt to contain project SYSTEM.md"
    );
    assert_eq!(user_input_contents(&requests[0]), vec!["hello"]);
}

#[test]
fn run_text_loads_project_context_after_persisted_trust() {
    let temp = TempDir::new().expect("tempdir");
    write_trust_store(&isolated_home_path(), temp.path(), true);
    std::fs::write(temp.path().join("AGENTS.md"), "Project context: use Rust.")
        .expect("write agents");
    let server = MockSseServer::start(vec![openai_response_sse("resp-trust", "trusted")]);
    write_mock_responses_config(&temp, &server.url);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "hello"]);

    run(command);

    let requests = server.requests();
    assert!(
        system_input_contents(&requests[0])
            .iter()
            .any(|content| content.contains("Project context: use Rust."))
    );
}

#[test]
fn run_text_yolo_skips_project_context_even_after_persisted_trust() {
    let temp = TempDir::new().expect("tempdir");
    write_trust_store(&isolated_home_path(), temp.path(), true);
    std::fs::write(temp.path().join("AGENTS.md"), "Project context: use Rust.")
        .expect("write agents");
    let server = MockSseServer::start(vec![openai_response_sse("resp-yolo", "yolo")]);
    write_mock_responses_config(&temp, &server.url);

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
    write_mock_responses_config(&temp, &server.url);

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
    write_echo_extension_at(&isolated_home_path().join("extensions/echo"));
    let server = MockSseServer::start(vec![openai_response_sse("resp-ext", "extension ready")]);
    write_mock_responses_config(&temp, &server.url);

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
    for name in &tool_names {
        assert_model_function_name_safe(name);
    }
    assert!(tool_names.contains(&"extension__echo__echo"));
    assert!(tool_names.contains(&"CreateSkill"));
    assert!(tool_names.contains(&"ListSkills"));
    assert!(tool_names.contains(&"MoveSkill"));
    assert!(tool_names.contains(&"SummarizeSessions"));
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
            r#"{}

[[mcp.servers]]
id = "docs-server"
enabled = true
transport = "stdio"
command = "python3"
args = ["-u", "{}"]
"#,
            mock_responses_config(&server.url),
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
