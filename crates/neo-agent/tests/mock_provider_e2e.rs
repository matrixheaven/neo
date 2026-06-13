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

fn contains_responses_assistant_text(messages: &[Value], text: &str) -> bool {
    messages.iter().any(|message| {
        message["type"] == "message"
            && message["role"] == "assistant"
            && message["content"]
                .as_array()
                .is_some_and(|content| content.iter().any(|part| part["text"] == text))
    })
}

fn isolated_home_path() -> std::path::PathBuf {
    static NEXT_HOME_ID: AtomicU64 = AtomicU64::new(0);
    let id = NEXT_HOME_ID.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("neo-e2e-home-{nanos}-{id}"))
}

fn write_scoped_openai_model_catalog(temp: &TempDir) {
    std::fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    std::fs::write(
        temp.path().join(".neo/config.toml"),
        r#"
model_catalogs = [".neo/models.json"]
"#,
    )
    .expect("write config");
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
}

fn write_openai_reasoning_model_catalog(temp: &TempDir) {
    std::fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    std::fs::write(
        temp.path().join(".neo/config.toml"),
        r#"
default_provider = "openai"
default_model = "reasoning-model"
model_catalogs = [".neo/models.json"]
"#,
    )
    .expect("write config");
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
fn print_models_scope_selects_first_matching_runtime_model() {
    let temp = TempDir::new().expect("tempdir");
    write_scoped_openai_model_catalog(&temp);
    let server = MockSseServer::start(vec![openai_response_sse("resp-model-scope", "scoped")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--models", "scoped", "print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "scoped\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["model"], "scoped-runtime-model");
}

#[test]
fn print_explicit_model_wins_over_models_scope_for_runtime_model() {
    let temp = TempDir::new().expect("tempdir");
    write_scoped_openai_model_catalog(&temp);
    let server = MockSseServer::start(vec![openai_response_sse("resp-model-explicit", "explicit")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--model", "gpt-4.1", "--models", "scoped", "print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "explicit\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["model"], "gpt-4.1");
}

#[test]
fn root_print_flag_uses_production_print_mode_against_mock_provider() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-root-print", "root print")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--print", "hello", "neo"]);

    let stdout = run(command);

    assert_eq!(stdout, "root print\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["input"][0]["content"], "hello neo");
}

#[test]
fn root_print_short_flag_uses_production_print_mode_against_mock_provider() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-root-short-print",
        "short root print",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["-p", "hello", "neo"]);

    let stdout = run(command);

    assert_eq!(stdout, "short root print\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["input"][0]["content"], "hello neo");
}

#[test]
fn root_print_flag_expands_workspace_relative_file_prompt_args() {
    let temp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(temp.path().join("docs")).expect("create docs");
    std::fs::write(temp.path().join("docs/context.txt"), "root file context\n")
        .expect("write prompt file");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-root-print-file",
        "root file",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--print", "@docs/context.txt", "summarize"]);

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
fn print_session_dir_flag_writes_session_to_explicit_directory() {
    let temp = TempDir::new().expect("tempdir");
    let sessions_dir = temp.path().join("custom-sessions");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-session-dir",
        "custom session",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .arg("--session-dir")
        .arg(&sessions_dir)
        .args(["print", "remember", "this"]);

    let stdout = run(command);

    assert_eq!(stdout, "custom session\n");
    let custom_sessions = session_files_in(&sessions_dir);
    assert_eq!(custom_sessions.len(), 1);
    assert!(
        !temp.path().join(".neo/sessions").exists(),
        "default session directory should not be created when --session-dir is set"
    );
    let content = std::fs::read_to_string(&custom_sessions[0]).expect("read custom session");
    assert!(content.contains("remember this"));
    assert!(content.contains("custom session"));
}

#[test]
fn print_no_session_flag_runs_without_creating_session_files() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-no-session",
        "ephemeral answer",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--no-session", "print", "ephemeral"]);

    let stdout = run(command);

    assert_eq!(stdout, "ephemeral answer\n");
    assert!(
        !temp.path().join(".neo/sessions").exists(),
        "--no-session should not create a session directory"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["input"][0]["content"], "ephemeral");
}

#[test]
fn print_api_key_flag_supplies_runtime_provider_credential_without_env() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-cli-api-key", "cli key")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env_remove("OPENAI_API_KEY")
        .env_remove("NEO_API_KEY_ENV")
        .arg("--api-base")
        .arg(&server.url)
        .arg("--api-key")
        .arg("runtime-key")
        .args(["print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "cli key\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].headers.get("authorization").map(String::as_str),
        Some("Bearer runtime-key")
    );
}

#[test]
fn run_no_session_flag_uses_ephemeral_stable_json_session_without_files() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-run-no-session",
        "ephemeral json",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--no-session", "run", "--output", "json", "ephemeral"]);

    let stdout = run(command);

    assert!(stdout.contains("\"type\":\"session\""));
    assert!(stdout.contains("\"id\":\"ephemeral\""));
    assert!(
        !temp.path().join(".neo/sessions").exists(),
        "--no-session should not create a session directory"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["input"][0]["content"], "ephemeral");
}

#[test]
fn print_session_id_flag_creates_exact_session_file() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-session-id",
        "exact session",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--session-id", "alpha-123", "print", "remember", "exact"]);

    let stdout = run(command);

    assert_eq!(stdout, "exact session\n");
    let session_path = temp.path().join(".neo/sessions/alpha-123.jsonl");
    assert!(session_path.is_file());
    let content = std::fs::read_to_string(session_path).expect("read exact session");
    assert!(content.contains("remember exact"));
    assert!(content.contains("exact session"));
}

#[test]
fn print_session_id_flag_replays_existing_session_and_appends_turn() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![
        openai_response_sse("resp-session-id-1", "first answer"),
        openai_response_sse("resp-session-id-2", "second answer"),
    ]);

    let mut first = neo();
    first
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--session-id", "alpha-123", "print", "first"]);
    assert_eq!(run(first), "first answer\n");

    let mut second = neo();
    second
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--session-id", "alpha-123", "print", "second"]);
    assert_eq!(run(second), "second answer\n");

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    let replayed = requests[1].body["input"]
        .as_array()
        .expect("second request input");
    assert!(replayed.iter().any(|message| {
        message["role"] == "user" && message["content"].as_str() == Some("first")
    }));
    assert!(contains_responses_assistant_text(replayed, "first answer"));
    assert!(replayed.iter().any(|message| {
        message["role"] == "user" && message["content"].as_str() == Some("second")
    }));
    let sessions = session_files(temp.path());
    assert_eq!(sessions.len(), 1);
}

#[test]
fn print_thinking_off_suppresses_signed_reasoning_replay_from_existing_session() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    std::fs::create_dir_all(&sessions).expect("create sessions");
    let signed_reasoning = serde_json::json!({
        "type": "reasoning",
        "id": "rs_session",
        "summary": [{ "type": "summary_text", "text": "stored reasoning" }],
        "encrypted_content": "opaque-reasoning"
    })
    .to_string();
    let session = serde_json::json!({
        "MessageAppended": {
            "message": {
                "Assistant": {
                    "content": [
                        {
                            "Thinking": {
                                "text": "stored reasoning",
                                "signature": signed_reasoning,
                                "redacted": false
                            }
                        },
                        { "Text": { "text": "visible answer" } }
                    ],
                    "tool_calls": [],
                    "stop_reason": "EndTurn"
                }
            }
        }
    });
    std::fs::write(sessions.join("alpha-123.jsonl"), format!("{session}\n"))
        .expect("write session");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-thinking-off-session",
        "continued",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .arg("--thinking")
        .arg("off")
        .args(["--session-id", "alpha-123", "print", "continue"]);

    let stdout = run(command);

    assert_eq!(stdout, "continued\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let input = requests[0].body["input"].as_array().expect("input array");
    assert!(
        input.iter().all(|item| item["type"] != "reasoning"),
        "thinking off must not replay encrypted reasoning items"
    );
    assert!(input.iter().any(|item| {
        item["type"] == "message"
            && item["role"] == "assistant"
            && item["content"][0]["text"] == "visible answer"
    }));
    assert!(
        requests[0].body.get("reasoning").is_none(),
        "thinking off should also avoid requesting new reasoning"
    );
}

#[test]
fn run_session_id_flag_uses_exact_session_id_in_stable_json_output() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-run-session-id",
        "json exact",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args([
            "--session-id",
            "run-alpha-123",
            "run",
            "--output",
            "json",
            "stable",
            "json",
        ]);

    let stdout = run(command);

    assert!(stdout.contains("\"type\":\"session\""));
    assert!(stdout.contains("\"id\":\"run-alpha-123\""));
    assert!(
        temp.path()
            .join(".neo/sessions/run-alpha-123.jsonl")
            .is_file()
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["input"][0]["content"], "stable json");
}

#[test]
fn print_session_flag_replays_existing_session_by_local_id_and_appends_turn() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![
        openai_response_sse("resp-session-1", "first answer"),
        openai_response_sse("resp-session-2", "continued answer"),
    ]);

    let mut first = neo();
    first
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--session-id", "local-alpha", "print", "first"]);
    assert_eq!(run(first), "first answer\n");

    let mut second = neo();
    second
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--session", "local-alpha", "print", "continue"]);
    assert_eq!(run(second), "continued answer\n");

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    let replayed = requests[1].body["input"]
        .as_array()
        .expect("second request input");
    assert!(replayed.iter().any(|message| {
        message["role"] == "user" && message["content"].as_str() == Some("first")
    }));
    assert!(contains_responses_assistant_text(replayed, "first answer"));
    assert!(replayed.iter().any(|message| {
        message["role"] == "user" && message["content"].as_str() == Some("continue")
    }));
    let sessions = session_files(temp.path());
    assert_eq!(sessions.len(), 1);
}

#[test]
fn print_session_flag_replays_existing_session_by_jsonl_path_and_appends_turn() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![
        openai_response_sse("resp-session-path-1", "first path answer"),
        openai_response_sse("resp-session-path-2", "continued path answer"),
    ]);

    let mut first = neo();
    first
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--session-id", "path-alpha", "print", "first"]);
    assert_eq!(run(first), "first path answer\n");

    let session_path = temp.path().join(".neo/sessions/path-alpha.jsonl");
    let mut second = neo();
    second
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .arg("--session")
        .arg(&session_path)
        .args(["print", "continue"]);
    assert_eq!(run(second), "continued path answer\n");

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].body["input"][2]["content"], "continue");
}

#[test]
fn run_session_flag_uses_existing_session_in_stable_json_output() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![
        openai_response_sse("resp-run-session-1", "first run answer"),
        openai_response_sse("resp-run-session-2", "continued run answer"),
    ]);

    let mut first = neo();
    first
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--session-id", "run-existing", "print", "first"]);
    assert_eq!(run(first), "first run answer\n");

    let mut second = neo();
    second
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args([
            "--session",
            "run-existing",
            "run",
            "--output",
            "json",
            "continue",
        ]);

    let stdout = run(second);

    assert!(stdout.contains("\"type\":\"session\""));
    assert!(stdout.contains("\"id\":\"run-existing\""));
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    let replayed = requests[1].body["input"]
        .as_array()
        .expect("second request input");
    assert!(contains_responses_assistant_text(
        replayed,
        "first run answer"
    ));
    assert!(replayed.iter().any(|message| {
        message["role"] == "user" && message["content"].as_str() == Some("continue")
    }));
}

#[test]
fn print_continue_flag_replays_latest_session_and_appends_turn() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![
        openai_response_sse("resp-continue-old", "old answer"),
        openai_response_sse("resp-continue-latest", "latest answer"),
        openai_response_sse("resp-continue-next", "continued latest"),
    ]);

    let mut old = neo();
    old.current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--session-id", "old-session", "print", "old"]);
    assert_eq!(run(old), "old answer\n");
    std::thread::sleep(std::time::Duration::from_millis(5));

    let mut latest = neo();
    latest
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--session-id", "latest-session", "print", "latest"]);
    assert_eq!(run(latest), "latest answer\n");

    let mut continue_run = neo();
    continue_run
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--continue", "print", "next"]);
    assert_eq!(run(continue_run), "continued latest\n");

    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    let replayed = requests[2].body["input"]
        .as_array()
        .expect("continued request input");
    assert!(replayed.iter().any(|message| {
        message["role"] == "user" && message["content"].as_str() == Some("latest")
    }));
    assert!(contains_responses_assistant_text(replayed, "latest answer"));
    assert!(replayed.iter().any(|message| {
        message["role"] == "user" && message["content"].as_str() == Some("next")
    }));
    let old_content = std::fs::read_to_string(temp.path().join(".neo/sessions/old-session.jsonl"))
        .expect("read old session");
    assert!(!old_content.contains("continued latest"));
    let latest_content =
        std::fs::read_to_string(temp.path().join(".neo/sessions/latest-session.jsonl"))
            .expect("read latest session");
    assert!(latest_content.contains("continued latest"));
}

#[test]
fn print_continue_short_flag_replays_latest_session_and_appends_turn() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![
        openai_response_sse("resp-short-continue-1", "first answer"),
        openai_response_sse("resp-short-continue-2", "second answer"),
    ]);

    let mut first = neo();
    first
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--session-id", "short-continue", "print", "first"]);
    assert_eq!(run(first), "first answer\n");

    let mut second = neo();
    second
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["-c", "print", "second"]);
    assert_eq!(run(second), "second answer\n");

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].body["input"][2]["content"], "second");
}

#[test]
fn run_continue_flag_uses_latest_session_in_stable_json_output() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![
        openai_response_sse("resp-run-continue-1", "first answer"),
        openai_response_sse("resp-run-continue-2", "continued answer"),
    ]);

    let mut first = neo();
    first
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--session-id", "run-continue", "print", "first"]);
    assert_eq!(run(first), "first answer\n");

    let mut second = neo();
    second
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--continue", "run", "--output", "json", "second"]);

    let stdout = run(second);

    assert!(stdout.contains("\"type\":\"session\""));
    assert!(stdout.contains("\"id\":\"run-continue\""));
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].body["input"][2]["content"], "second");
}

#[test]
fn print_name_flag_sets_session_display_name_for_new_exact_session() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-name", "named answer")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args([
            "--session-id",
            "named-session",
            "--name",
            "Main Thread",
            "print",
            "hello",
        ]);

    assert_eq!(run(command), "named answer\n");

    let mut list = neo();
    list.current_dir(temp.path()).args(["sessions", "list"]);
    let stdout = run(list);
    assert!(stdout.contains("named-session\tMain Thread"));
}

#[test]
fn print_name_short_flag_renames_continued_latest_session() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![
        openai_response_sse("resp-short-name-1", "first answer"),
        openai_response_sse("resp-short-name-2", "second answer"),
    ]);

    let mut first = neo();
    first
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--session-id", "rename-latest", "print", "first"]);
    assert_eq!(run(first), "first answer\n");

    let mut second = neo();
    second
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["-c", "-n", "Renamed Latest", "print", "second"]);
    assert_eq!(run(second), "second answer\n");

    let mut list = neo();
    list.current_dir(temp.path()).args(["sessions", "list"]);
    let stdout = run(list);
    assert!(stdout.contains("rename-latest\tRenamed Latest"));
}

#[test]
fn run_name_flag_sets_session_display_name_for_exact_session() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-run-name", "named json")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args([
            "--session-id",
            "named-run",
            "--name",
            "Run Thread",
            "run",
            "--output",
            "json",
            "hello",
        ]);

    let stdout = run(command);

    assert!(stdout.contains("\"id\":\"named-run\""));
    let mut list = neo();
    list.current_dir(temp.path()).args(["sessions", "list"]);
    let list_stdout = run(list);
    assert!(list_stdout.contains("named-run\tRun Thread"));
}

#[test]
fn print_fork_flag_copies_existing_session_and_appends_turn_to_child() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![
        openai_response_sse("resp-fork-1", "parent answer"),
        openai_response_sse("resp-fork-2", "child answer"),
    ]);

    let mut parent = neo();
    parent
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--session-id", "fork-parent", "print", "parent"]);
    assert_eq!(run(parent), "parent answer\n");

    let mut child = neo();
    child
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--fork", "fork-parent", "print", "child"]);
    assert_eq!(run(child), "child answer\n");

    let mut list = neo();
    list.current_dir(temp.path()).args(["sessions", "list"]);
    let list_stdout = run(list);
    let child_id = list_stdout
        .lines()
        .find(|line| line.contains("parent=fork-parent"))
        .and_then(|line| line.split('\t').next())
        .expect("forked child listed with parent")
        .to_owned();
    assert_ne!(child_id, "fork-parent");

    let parent_content =
        std::fs::read_to_string(temp.path().join(".neo/sessions/fork-parent.jsonl"))
            .expect("read parent session");
    assert!(!parent_content.contains("child answer"));
    let child_content =
        std::fs::read_to_string(temp.path().join(format!(".neo/sessions/{child_id}.jsonl")))
            .expect("read child session");
    assert!(child_content.contains("parent answer"));
    assert!(child_content.contains("child answer"));

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    let replayed = requests[1].body["input"]
        .as_array()
        .expect("fork request input");
    assert!(contains_responses_assistant_text(replayed, "parent answer"));
    assert!(replayed.iter().any(|message| {
        message["role"] == "user" && message["content"].as_str() == Some("child")
    }));
}

#[test]
fn run_fork_flag_uses_child_session_in_stable_json_output_and_name_flag_names_child() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![
        openai_response_sse("resp-run-fork-1", "parent answer"),
        openai_response_sse("resp-run-fork-2", "child json"),
    ]);

    let mut parent = neo();
    parent
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--session-id", "run-fork-parent", "print", "parent"]);
    assert_eq!(run(parent), "parent answer\n");

    let mut child = neo();
    child
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args([
            "--fork",
            "run-fork-parent",
            "--name",
            "Child Branch",
            "run",
            "--output",
            "json",
            "child",
        ]);
    let stdout = run(child);

    let mut list = neo();
    list.current_dir(temp.path()).args(["sessions", "list"]);
    let list_stdout = run(list);
    let child_id = list_stdout
        .lines()
        .find(|line| line.contains("parent=run-fork-parent"))
        .and_then(|line| line.split('\t').next())
        .expect("forked child listed with parent")
        .to_owned();
    assert!(stdout.contains(&format!("\"id\":\"{child_id}\"")));
    assert!(list_stdout.contains("Child Branch"));
}

#[test]
fn print_no_tools_omits_all_model_tools() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-no-tools", "no tools")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--no-tools", "print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "no tools\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].body.get("tools").is_none(),
        "model request should not include tools when --no-tools is set: {}",
        requests[0].body
    );
}

#[test]
fn print_pi_style_short_no_tools_alias_omits_all_model_tools() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-short-no-tools",
        "short no tools",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["-nt", "print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "short no tools\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].body.get("tools").is_none());
}

#[test]
fn print_no_tools_with_tools_allowlist_reenables_only_named_model_tools() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-tools-allowlist",
        "allowlisted tools",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--no-tools", "-t", "read,bash", "print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "allowlisted tools\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(model_tool_names(&requests[0].body), vec!["bash", "read"]);
}

#[test]
fn print_exclude_tools_removes_named_model_tools_after_allowlist() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-tools-exclude",
        "excluded tools",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args([
            "--tools",
            "read,bash,write",
            "-xt",
            "read,write",
            "print",
            "hello",
        ]);

    let stdout = run(command);

    assert_eq!(stdout, "excluded tools\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(model_tool_names(&requests[0].body), vec!["bash"]);
}

#[test]
fn print_includes_project_system_prompt_file_before_user_message() {
    let temp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    std::fs::write(
        temp.path().join(".neo/SYSTEM.md"),
        "Use the project system prompt.\n",
    )
    .expect("write system prompt");
    let server = MockSseServer::start(vec![openai_response_sse("resp-system", "system loaded")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "system loaded\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["input"][0]["role"], "system");
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "Use the project system prompt."
    );
    assert_eq!(requests[0].body["input"][1]["role"], "user");
    assert_eq!(requests[0].body["input"][1]["content"], "hello");
}

#[test]
fn print_appends_project_append_system_prompt_file_to_system_message() {
    let temp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    std::fs::write(temp.path().join(".neo/SYSTEM.md"), "Base instructions.\n")
        .expect("write system prompt");
    std::fs::write(
        temp.path().join(".neo/APPEND_SYSTEM.md"),
        "Additional instructions.\n",
    )
    .expect("write append system prompt");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-append-system",
        "append loaded",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "append loaded\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["input"][0]["role"], "system");
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "Base instructions.\n\nAdditional instructions."
    );
    assert_eq!(requests[0].body["input"][1]["content"], "hello");
}

#[test]
fn print_approve_includes_agents_context_file_in_system_message() {
    let temp = TempDir::new().expect("tempdir");
    std::fs::write(
        temp.path().join("AGENTS.md"),
        "Always mention the project marker CONTEXT_MARKER_17.\n",
    )
    .expect("write AGENTS.md");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-context-file",
        "context loaded",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .arg("--approve")
        .args(["print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "context loaded\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["input"][0]["role"], "system");
    let system = requests[0].body["input"][0]["content"]
        .as_str()
        .expect("system content");
    assert!(system.contains("<project_context>"));
    assert!(system.contains("Project-specific instructions and guidelines:"));
    assert!(system.contains("<project_instructions path=\""));
    assert!(system.contains("AGENTS.md"));
    assert!(system.contains("CONTEXT_MARKER_17"));
}

#[test]
fn print_no_context_files_disables_agents_files_without_disabling_system_prompt() {
    let temp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    std::fs::write(
        temp.path().join(".neo/SYSTEM.md"),
        "Base system survives.\n",
    )
    .expect("write system prompt");
    std::fs::write(
        temp.path().join("AGENTS.md"),
        "This disabled marker must not appear: DISABLED_CONTEXT_MARKER_29.\n",
    )
    .expect("write AGENTS.md");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-no-context-files",
        "context disabled",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .arg("--approve")
        .arg("--no-context-files")
        .args(["print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "context disabled\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["input"][0]["role"], "system");
    let system = requests[0].body["input"][0]["content"]
        .as_str()
        .expect("system content");
    assert!(system.contains("Base system survives."));
    assert!(!system.contains("DISABLED_CONTEXT_MARKER_29"));
    assert!(!system.contains("<project_context>"));
}

#[test]
fn print_pi_style_short_no_context_files_alias_disables_agents_files() {
    let temp = TempDir::new().expect("tempdir");
    std::fs::write(
        temp.path().join("AGENTS.md"),
        "This disabled marker must not appear: SHORT_DISABLED_CONTEXT_MARKER_31.\n",
    )
    .expect("write AGENTS.md");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-short-no-context-files",
        "short context disabled",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .arg("--approve")
        .arg("-nc")
        .args(["print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "short context disabled\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["input"][0]["role"], "user");
    assert_eq!(requests[0].body["input"][0]["content"], "hello");
}

#[test]
fn print_loads_project_context_after_persisted_trust_without_approve() {
    let home = TempDir::new().expect("home tempdir");
    let project = TempDir::new().expect("project tempdir");
    std::fs::write(
        project.path().join("AGENTS.md"),
        "Always mention persisted trust marker TRUSTED_CONTEXT_MARKER_43.\n",
    )
    .expect("write AGENTS.md");

    let mut trust = neo();
    trust
        .current_dir(project.path())
        .env("HOME", home.path())
        .args(["trust", "approve"]);
    let stdout = run(trust);
    assert!(stdout.contains("trusted"));

    let trust_store =
        std::fs::read_to_string(home.path().join(".neo/trust.json")).expect("read trust store");
    assert!(trust_store.contains("true"));

    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-persisted-trust",
        "trusted context loaded",
    )]);
    let mut command = neo();
    command
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "trusted context loaded\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let system = requests[0].body["input"][0]["content"]
        .as_str()
        .expect("system content");
    assert!(system.contains("TRUSTED_CONTEXT_MARKER_43"));
}

#[test]
fn print_no_approve_skips_project_context_even_after_persisted_trust() {
    let home = TempDir::new().expect("home tempdir");
    let project = TempDir::new().expect("project tempdir");
    std::fs::write(
        project.path().join("AGENTS.md"),
        "This trusted marker is overridden: OVERRIDDEN_TRUST_MARKER_47.\n",
    )
    .expect("write AGENTS.md");

    let mut trust = neo();
    trust
        .current_dir(project.path())
        .env("HOME", home.path())
        .args(["trust", "approve"]);
    run(trust);

    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-no-approve-trust",
        "trust overridden",
    )]);
    let mut command = neo();
    command
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .arg("--no-approve")
        .args(["print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "trust overridden\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["input"][0]["role"], "user");
    assert_eq!(requests[0].body["input"][0]["content"], "hello");
}

#[test]
fn trust_status_deny_and_clear_update_project_trust_store() {
    let home = TempDir::new().expect("home tempdir");
    let project = TempDir::new().expect("project tempdir");
    std::fs::write(project.path().join("AGENTS.md"), "rules").expect("write AGENTS.md");

    let mut status = neo();
    status
        .current_dir(project.path())
        .env("HOME", home.path())
        .args(["trust", "status"]);
    let stdout = run(status);
    assert!(stdout.contains("untrusted"));

    let mut deny = neo();
    deny.current_dir(project.path())
        .env("HOME", home.path())
        .args(["trust", "deny"]);
    let stdout = run(deny);
    assert!(stdout.contains("untrusted"));
    let trust_store =
        std::fs::read_to_string(home.path().join(".neo/trust.json")).expect("read trust store");
    assert!(trust_store.contains("false"));

    let mut clear = neo();
    clear
        .current_dir(project.path())
        .env("HOME", home.path())
        .args(["trust", "clear"]);
    let stdout = run(clear);
    assert!(stdout.contains("cleared trust"));
    let trust_store =
        std::fs::read_to_string(home.path().join(".neo/trust.json")).expect("read trust store");
    assert_eq!(trust_store.trim(), "{}");
}

#[test]
fn print_prefers_project_system_resources_over_user_global_resources() {
    let home = TempDir::new().expect("home tempdir");
    let project = TempDir::new().expect("project tempdir");
    std::fs::create_dir_all(home.path().join(".neo")).expect("create home .neo");
    std::fs::create_dir_all(project.path().join(".neo")).expect("create project .neo");
    std::fs::write(home.path().join(".neo/SYSTEM.md"), "Global system")
        .expect("write global system");
    std::fs::write(
        home.path().join(".neo/APPEND_SYSTEM.md"),
        "Global append should not win",
    )
    .expect("write global append system");
    std::fs::write(project.path().join(".neo/SYSTEM.md"), "Project system")
        .expect("write project system");
    std::fs::write(
        project.path().join(".neo/APPEND_SYSTEM.md"),
        "Project append",
    )
    .expect("write project append system");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-precedence-system",
        "precedence",
    )]);

    let mut command = neo();
    command
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "precedence\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "Project system\n\nProject append"
    );
}

#[test]
fn print_cli_system_prompt_overrides_discovered_system_prompt_file() {
    let temp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    std::fs::write(
        temp.path().join(".neo/SYSTEM.md"),
        "Discovered instructions should not win",
    )
    .expect("write system prompt");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-cli-system",
        "cli system loaded",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .arg("--system-prompt")
        .arg("Use explicit CLI instructions.")
        .args(["print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "cli system loaded\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "Use explicit CLI instructions."
    );
}

#[test]
fn print_cli_append_system_prompt_overrides_discovered_append_file() {
    let temp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    std::fs::write(temp.path().join(".neo/SYSTEM.md"), "Base instructions.")
        .expect("write system prompt");
    std::fs::write(
        temp.path().join(".neo/APPEND_SYSTEM.md"),
        "Discovered append should not win",
    )
    .expect("write append system prompt");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-cli-append-system",
        "cli append loaded",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .arg("--append-system-prompt")
        .arg("CLI append one.")
        .arg("--append-system-prompt")
        .arg("CLI append two.")
        .args(["print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "cli append loaded\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "Base instructions.\n\nCLI append one.\n\nCLI append two."
    );
}

#[test]
fn print_skill_flag_injects_loaded_skill_body_into_system_prompt() {
    let temp = TempDir::new().expect("tempdir");
    let skill = temp.path().join("skills/reviewer");
    std::fs::create_dir_all(&skill).expect("create skill dir");
    std::fs::write(
        skill.join("SKILL.md"),
        r#"---
name = "reviewer"
description = "Review code changes"
---
Always mention the Neo skill marker: SKILL_MARKER_42.
"#,
    )
    .expect("write skill");
    let server = MockSseServer::start(vec![openai_response_sse("resp-skill", "skill loaded")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .arg("--skill")
        .arg(&skill)
        .args(["print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "skill loaded\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["input"][0]["role"], "system");
    assert!(
        requests[0].body["input"][0]["content"]
            .as_str()
            .expect("system content")
            .contains("SKILL_MARKER_42")
    );
}

#[test]
fn print_skill_flag_injects_declared_text_resource_into_system_prompt() {
    let temp = TempDir::new().expect("tempdir");
    let skill = temp.path().join("skills/reviewer");
    std::fs::create_dir_all(skill.join("references")).expect("create skill resources");
    std::fs::write(
        skill.join("SKILL.md"),
        r#"---
name = "reviewer"
description = "Review code changes"
resources = [{ path = "references/policy.md", kind = "text" }]
---
Skill body marker: RESOURCE_SKILL_BODY_71.
"#,
    )
    .expect("write skill");
    std::fs::write(
        skill.join("references/policy.md"),
        "Resource marker: RESOURCE_TEXT_MARKER_73.\n",
    )
    .expect("write skill resource");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-skill-resource",
        "skill resource loaded",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .arg("--skill")
        .arg(&skill)
        .args(["print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "skill resource loaded\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let system = requests[0].body["input"][0]["content"]
        .as_str()
        .expect("system content");
    assert!(system.contains("RESOURCE_SKILL_BODY_71"));
    assert!(system.contains("<skill_resource path=\"references/policy.md\" kind=\"text\">"));
    assert!(system.contains("RESOURCE_TEXT_MARKER_73"));
    assert!(system.contains("</skill_resource>"));
}

#[test]
fn print_skill_flag_fails_clearly_when_declared_resource_is_missing() {
    let temp = TempDir::new().expect("tempdir");
    let skill = temp.path().join("skills/reviewer");
    std::fs::create_dir_all(&skill).expect("create skill dir");
    std::fs::write(
        skill.join("SKILL.md"),
        r#"---
name = "reviewer"
description = "Review code changes"
resources = [{ path = "references/missing.md", kind = "text" }]
---
Skill body marker: MISSING_RESOURCE_BODY_79.
"#,
    )
    .expect("write skill");
    let server = MockSseServer::start(Vec::new());

    let output = neo()
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .arg("--skill")
        .arg(&skill)
        .args(["print", "hello"])
        .output()
        .expect("neo command should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to load skill"));
    assert!(stderr.contains("references/missing.md"));
    assert!(stderr.contains("failed to read skill resource"));
    assert!(server.requests().is_empty());
}

#[test]
fn print_discovers_project_skill_into_system_prompt() {
    let temp = TempDir::new().expect("tempdir");
    let skill = temp.path().join(".neo/skills/reviewer");
    std::fs::create_dir_all(&skill).expect("create skill dir");
    std::fs::write(
        skill.join("SKILL.md"),
        r#"---
name = "reviewer"
description = "Project reviewer"
---
Always mention the auto skill marker: AUTO_SKILL_MARKER_53.
"#,
    )
    .expect("write skill");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-auto-skill",
        "auto skill loaded",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "auto skill loaded\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let system = requests[0].body["input"][0]["content"]
        .as_str()
        .expect("system content");
    assert!(system.contains("AUTO_SKILL_MARKER_53"));
}

#[test]
fn print_no_skills_disables_discovered_skills_but_keeps_explicit_skill_flag() {
    let temp = TempDir::new().expect("tempdir");
    let auto_skill = temp.path().join(".neo/skills/auto");
    std::fs::create_dir_all(&auto_skill).expect("create auto skill dir");
    std::fs::write(
        auto_skill.join("SKILL.md"),
        r#"---
name = "auto"
description = "Auto skill"
---
AUTO_DISABLED_SKILL_MARKER_59
"#,
    )
    .expect("write auto skill");
    let explicit_skill = temp.path().join("explicit-skill");
    std::fs::create_dir_all(&explicit_skill).expect("create explicit skill dir");
    std::fs::write(
        explicit_skill.join("SKILL.md"),
        r#"---
name = "explicit"
description = "Explicit skill"
---
EXPLICIT_SKILL_MARKER_61
"#,
    )
    .expect("write explicit skill");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-no-skills",
        "explicit skill retained",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .arg("--no-skills")
        .arg("--skill")
        .arg(&explicit_skill)
        .args(["print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "explicit skill retained\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let system = requests[0].body["input"][0]["content"]
        .as_str()
        .expect("system content");
    assert!(system.contains("EXPLICIT_SKILL_MARKER_61"));
    assert!(!system.contains("AUTO_DISABLED_SKILL_MARKER_59"));
}

#[test]
fn print_pi_style_short_no_skills_alias_disables_discovered_skills() {
    let temp = TempDir::new().expect("tempdir");
    let auto_skill = temp.path().join(".neo/skills/auto");
    std::fs::create_dir_all(&auto_skill).expect("create auto skill dir");
    std::fs::write(
        auto_skill.join("SKILL.md"),
        r#"---
name = "auto"
description = "Auto skill"
---
SHORT_AUTO_DISABLED_SKILL_MARKER_67
"#,
    )
    .expect("write auto skill");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-short-no-skills",
        "short no skills",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .arg("-ns")
        .args(["print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "short no skills\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["input"][0]["role"], "user");
    assert_eq!(requests[0].body["input"][0]["content"], "hello");
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
fn print_expands_project_prompt_template_with_arguments() {
    let temp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".neo/prompts")).expect("create prompts");
    std::fs::write(
        temp.path().join(".neo/prompts/review.md"),
        r#"---
description: Review a target
argument-hint: "<path> [focus]"
---
Review target: $1
Second arg: $2
Focus: ${@:2}
All args: $ARGUMENTS
"#,
    )
    .expect("write prompt template");
    let server = MockSseServer::start(vec![openai_response_sse("resp-template", "reviewed")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["print", "/review", "src/lib.rs", "security pass"]);

    let stdout = run(command);

    assert_eq!(stdout, "reviewed\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "Review target: src/lib.rs\nSecond arg: security pass\nFocus: security pass\nAll args: src/lib.rs security pass"
    );
}

#[test]
fn print_expands_user_global_prompt_template_when_project_has_no_match() {
    let home = TempDir::new().expect("home tempdir");
    let project = TempDir::new().expect("project tempdir");
    std::fs::create_dir_all(home.path().join(".neo/prompts")).expect("create global prompts");
    std::fs::write(
        home.path().join(".neo/prompts/review.md"),
        "Global review target: $1\nAll: $ARGUMENTS\n",
    )
    .expect("write global prompt template");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-global-template",
        "global reviewed",
    )]);

    let mut command = neo();
    command
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["print", "/review", "src/main.rs"]);

    let stdout = run(command);

    assert_eq!(stdout, "global reviewed\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "Global review target: src/main.rs\nAll: src/main.rs"
    );
}

#[test]
fn print_prefers_project_prompt_template_over_user_global_template() {
    let home = TempDir::new().expect("home tempdir");
    let project = TempDir::new().expect("project tempdir");
    std::fs::create_dir_all(home.path().join(".neo/prompts")).expect("create global prompts");
    std::fs::write(
        home.path().join(".neo/prompts/review.md"),
        "Global review target: $1\n",
    )
    .expect("write global prompt template");
    std::fs::create_dir_all(project.path().join(".neo/prompts")).expect("create project prompts");
    std::fs::write(
        project.path().join(".neo/prompts/review.md"),
        "Project review target: $1\n",
    )
    .expect("write project prompt template");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-project-template",
        "project reviewed",
    )]);

    let mut command = neo();
    command
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["print", "/review", "src/main.rs"]);

    let stdout = run(command);

    assert_eq!(stdout, "project reviewed\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "Project review target: src/main.rs"
    );
}

#[test]
fn print_forces_prompt_template_by_name_without_slash_invocation() {
    let home = TempDir::new().expect("home tempdir");
    let project = TempDir::new().expect("project tempdir");
    std::fs::create_dir_all(home.path().join(".neo/prompts")).expect("create global prompts");
    std::fs::write(
        home.path().join(".neo/prompts/review.md"),
        "Forced review target: $1\nFocus: ${@:2}\n",
    )
    .expect("write global prompt template");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-forced-template",
        "forced reviewed",
    )]);

    let mut command = neo();
    command
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args([
            "--prompt-template",
            "review",
            "print",
            "src/main.rs",
            "safety pass",
        ]);

    let stdout = run(command);

    assert_eq!(stdout, "forced reviewed\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "Forced review target: src/main.rs\nFocus: safety pass"
    );
}

#[test]
fn print_forces_prompt_template_by_project_relative_path() {
    let project = TempDir::new().expect("project tempdir");
    std::fs::create_dir_all(project.path().join("prompts")).expect("create prompts");
    std::fs::write(
        project.path().join("prompts/explain.md"),
        "Explain target: $1\nDetails: $@\n",
    )
    .expect("write prompt template");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-path-template",
        "path reviewed",
    )]);

    let mut command = neo();
    command
        .current_dir(project.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args([
            "--prompt-template",
            "prompts/explain.md",
            "print",
            "src/lib.rs",
        ]);

    let stdout = run(command);

    assert_eq!(stdout, "path reviewed\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "Explain target: src/lib.rs\nDetails: src/lib.rs"
    );
}

#[test]
fn print_forces_prompt_template_from_explicit_directory() {
    let project = TempDir::new().expect("project tempdir");
    std::fs::create_dir_all(project.path().join("prompts/nested")).expect("create prompts");
    std::fs::write(project.path().join("prompts/review.md"), "Dir review: $1\n")
        .expect("write prompt template");
    std::fs::write(
        project.path().join("prompts/nested/ignored.md"),
        "Nested review: $1\n",
    )
    .expect("write nested prompt template");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-dir-template",
        "dir reviewed",
    )]);

    let mut command = neo();
    command
        .current_dir(project.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args([
            "--prompt-template",
            "prompts",
            "print",
            "/review",
            "src/lib.rs",
        ]);

    let stdout = run(command);

    assert_eq!(stdout, "dir reviewed\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "Dir review: src/lib.rs"
    );
}

#[test]
fn print_expands_prompt_template_from_project_config_selector() {
    let project = TempDir::new().expect("project tempdir");
    std::fs::create_dir_all(project.path().join(".neo")).expect("create .neo");
    std::fs::write(
        project.path().join(".neo/config.toml"),
        r#"
prompt_templates = ["prompts"]
"#,
    )
    .expect("write config");
    std::fs::create_dir_all(project.path().join("prompts")).expect("create prompts");
    std::fs::write(
        project.path().join("prompts/review.md"),
        "Configured review: $1\n",
    )
    .expect("write prompt template");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-config-template",
        "config reviewed",
    )]);

    let mut command = neo();
    command
        .current_dir(project.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["print", "/review", "src/lib.rs"]);

    let stdout = run(command);

    assert_eq!(stdout, "config reviewed\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "Configured review: src/lib.rs"
    );
}

#[test]
fn print_config_prompt_template_exclusion_skips_auto_discovered_project_prompt() {
    let project = TempDir::new().expect("project tempdir");
    std::fs::create_dir_all(project.path().join(".neo/prompts")).expect("create project prompts");
    std::fs::write(
        project.path().join(".neo/prompts/review.md"),
        "Project review: $1\n",
    )
    .expect("write project prompt template");
    std::fs::write(
        project.path().join(".neo/config.toml"),
        r#"
prompt_templates = ["-prompts/review.md"]
"#,
    )
    .expect("write config");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-excluded-template",
        "excluded",
    )]);

    let mut command = neo();
    command
        .current_dir(project.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["print", "/review", "src/lib.rs"]);

    let stdout = run(command);

    assert_eq!(stdout, "excluded\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "/review src/lib.rs"
    );
}

#[test]
fn print_prompt_template_exclusion_does_not_require_existing_path() {
    let project = TempDir::new().expect("project tempdir");
    std::fs::create_dir_all(project.path().join(".neo")).expect("create .neo");
    std::fs::write(
        project.path().join(".neo/config.toml"),
        r#"
prompt_templates = ["-prompts/missing.md"]
"#,
    )
    .expect("write config");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-missing-exclusion",
        "missing exclusion ignored",
    )]);

    let mut command = neo();
    command
        .current_dir(project.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["print", "hello"]);

    let stdout = run(command);

    assert_eq!(stdout, "missing exclusion ignored\n");
    let requests = server.requests();
    assert_eq!(requests[0].body["input"][0]["content"], "hello");
}

#[test]
fn print_prompt_template_exclusion_keeps_explicit_positive_selector_enabled() {
    let project = TempDir::new().expect("project tempdir");
    std::fs::create_dir_all(project.path().join(".neo")).expect("create .neo");
    std::fs::create_dir_all(project.path().join("prompts")).expect("create prompts");
    std::fs::write(
        project.path().join("prompts/review.md"),
        "Explicit review remains: $1\n",
    )
    .expect("write explicit prompt template");
    std::fs::write(
        project.path().join(".neo/config.toml"),
        r#"
prompt_templates = ["prompts", "-prompts/review.md"]
"#,
    )
    .expect("write config");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-explicit-with-exclusion",
        "explicit retained",
    )]);

    let mut command = neo();
    command
        .current_dir(project.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["print", "/review", "src/lib.rs"]);

    let stdout = run(command);

    assert_eq!(stdout, "explicit retained\n");
    let requests = server.requests();
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "Explicit review remains: src/lib.rs"
    );
}

#[test]
fn print_deduplicates_cli_and_config_prompt_template_selectors() {
    let project = TempDir::new().expect("project tempdir");
    std::fs::create_dir_all(project.path().join(".neo")).expect("create .neo");
    std::fs::write(
        project.path().join(".neo/config.toml"),
        r#"
prompt_templates = ["prompts"]
"#,
    )
    .expect("write config");
    std::fs::create_dir_all(project.path().join("prompts")).expect("create prompts");
    std::fs::write(
        project.path().join("prompts/review.md"),
        "Review once: $1\n",
    )
    .expect("write prompt template");
    let server = MockSseServer::start(vec![openai_response_sse("resp-dedup-template", "deduped")]);

    let mut command = neo();
    command
        .current_dir(project.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args([
            "--prompt-template",
            "prompts",
            "print",
            "/review",
            "src/lib.rs",
        ]);

    let stdout = run(command);

    assert_eq!(stdout, "deduped\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "Review once: src/lib.rs"
    );
}

#[test]
fn print_fails_when_explicit_directories_define_duplicate_prompt_templates() {
    let project = TempDir::new().expect("project tempdir");
    std::fs::create_dir_all(project.path().join("dir-a")).expect("create dir-a");
    std::fs::create_dir_all(project.path().join("dir-b")).expect("create dir-b");
    let first_path = project.path().join("dir-a/review.md");
    let second_path = project.path().join("dir-b/review.md");
    std::fs::write(&first_path, "First review: $1\n").expect("write first prompt template");
    std::fs::write(&second_path, "Second review: $1\n").expect("write second prompt template");

    let mut command = neo();
    command
        .current_dir(project.path())
        .env("OPENAI_API_KEY", "test-key")
        .args([
            "--prompt-template",
            "dir-a",
            "--prompt-template",
            "dir-b",
            "print",
            "/review",
            "x",
        ]);

    let output = command.output().expect("neo command should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf8");
    assert!(stderr.contains("duplicate prompt template `review`"));
    assert!(
        stderr.contains(
            &first_path
                .canonicalize()
                .expect("canonical first")
                .display()
                .to_string()
        )
    );
    assert!(
        stderr.contains(
            &second_path
                .canonicalize()
                .expect("canonical second")
                .display()
                .to_string()
        )
    );
}

#[test]
fn print_no_prompt_templates_keeps_explicit_prompt_template_enabled() {
    let project = TempDir::new().expect("project tempdir");
    std::fs::create_dir_all(project.path().join(".neo/prompts")).expect("create project prompts");
    std::fs::write(
        project.path().join(".neo/prompts/review.md"),
        "Auto review: $1\n",
    )
    .expect("write auto prompt template");
    std::fs::create_dir_all(project.path().join("prompts")).expect("create explicit prompts");
    std::fs::write(
        project.path().join("prompts/review.md"),
        "Explicit review: $1\n",
    )
    .expect("write explicit prompt template");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-explicit-with-disabled-auto",
        "explicit reviewed",
    )]);

    let mut command = neo();
    command
        .current_dir(project.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args([
            "--no-prompt-templates",
            "--prompt-template",
            "prompts",
            "print",
            "/review",
            "src/lib.rs",
        ]);

    let stdout = run(command);

    assert_eq!(stdout, "explicit reviewed\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "Explicit review: src/lib.rs"
    );
}

#[test]
fn print_no_prompt_templates_keeps_matching_slash_prompt_unchanged() {
    let project = TempDir::new().expect("project tempdir");
    std::fs::create_dir_all(project.path().join(".neo/prompts")).expect("create project prompts");
    std::fs::write(
        project.path().join(".neo/prompts/review.md"),
        "Review target: $1\n",
    )
    .expect("write project prompt template");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-disabled-template",
        "template disabled",
    )]);

    let mut command = neo();
    command
        .current_dir(project.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--no-prompt-templates", "print", "/review", "src/lib.rs"]);

    let stdout = run(command);

    assert_eq!(stdout, "template disabled\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "/review src/lib.rs"
    );
}

#[test]
fn print_pi_style_short_no_prompt_templates_alias_keeps_matching_slash_prompt_unchanged() {
    let project = TempDir::new().expect("project tempdir");
    std::fs::create_dir_all(project.path().join(".neo/prompts")).expect("create project prompts");
    std::fs::write(
        project.path().join(".neo/prompts/review.md"),
        "Review target: $1\n",
    )
    .expect("write project prompt template");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-short-disabled-template",
        "short template disabled",
    )]);

    let mut command = neo();
    command
        .current_dir(project.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["-np", "print", "/review", "src/lib.rs"]);

    let stdout = run(command);

    assert_eq!(stdout, "short template disabled\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "/review src/lib.rs"
    );
}

#[test]
fn run_expands_project_prompt_template_before_json_output() {
    let temp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".neo/prompts")).expect("create prompts");
    std::fs::write(
        temp.path().join(".neo/prompts/review.md"),
        r#"---
description = "Review a target"
argument-hint = "<path>"
---
Review target: $1
Trailing args: $@
"#,
    )
    .expect("write prompt template");
    let server = MockSseServer::start(vec![openai_response_sse("resp-template-json", "done")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["run", "--output", "json", "/review", "src/lib.rs"]);

    let stdout = run(command);

    let values = stdout
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("line should be json"))
        .collect::<Vec<_>>();
    assert_eq!(values[0]["type"], "session");
    assert!(values.iter().any(|value| value["type"] == "message_update"
        && value["assistantMessageEvent"]["delta"] == "done"));
    assert!(!stdout.contains("/review"));
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "Review target: src/lib.rs\nTrailing args: src/lib.rs"
    );
}

#[test]
fn print_leaves_unknown_slash_prompt_unchanged() {
    let temp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".neo/prompts")).expect("create prompts");
    std::fs::write(
        temp.path().join(".neo/prompts/review.md"),
        "Review target: $1\n",
    )
    .expect("write prompt template");
    let server = MockSseServer::start(vec![openai_response_sse("resp-unknown-slash", "kept")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["print", "/unknown", "leave", "alone"]);

    let stdout = run(command);

    assert_eq!(stdout, "kept\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "/unknown leave alone"
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
    write_openai_reasoning_model_catalog(&temp);
    std::fs::OpenOptions::new()
        .append(true)
        .open(temp.path().join(".neo/config.toml"))
        .expect("open config")
        .write_all(
            br#"

[runtime]
temperature = 0.25
max_tokens = 321
reasoning_effort = "high"
"#,
        )
        .expect("append config");
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
fn print_cli_thinking_overrides_project_runtime_reasoning_effort() {
    let temp = TempDir::new().expect("tempdir");
    write_openai_reasoning_model_catalog(&temp);
    std::fs::OpenOptions::new()
        .append(true)
        .open(temp.path().join(".neo/config.toml"))
        .expect("open config")
        .write_all(
            br#"

[runtime]
reasoning_effort = "low"
"#,
        )
        .expect("append config");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-cli-thinking",
        "thinking configured",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .arg("--thinking")
        .arg("high")
        .args(["print", "runtime", "options"]);

    let stdout = run(command);

    assert_eq!(stdout, "thinking configured\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["reasoning"]["effort"], "high");
}

#[test]
fn print_cli_thinking_off_disables_project_runtime_reasoning_effort() {
    let temp = TempDir::new().expect("tempdir");
    write_openai_reasoning_model_catalog(&temp);
    std::fs::OpenOptions::new()
        .append(true)
        .open(temp.path().join(".neo/config.toml"))
        .expect("open config")
        .write_all(
            br#"

[runtime]
reasoning_effort = "high"
"#,
        )
        .expect("append config");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-cli-thinking-off",
        "thinking disabled",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .arg("--thinking")
        .arg("off")
        .args(["print", "runtime", "options"]);

    let stdout = run(command);

    assert_eq!(stdout, "thinking disabled\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].body.get("reasoning").is_none());
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
fn run_output_json_emits_stable_typed_events_from_mock_provider() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-json", "json text")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["run", "--output", "json", "stream", "events"]);

    let stdout = run(command);

    let lines = stdout.lines().collect::<Vec<_>>();
    assert!(lines.len() >= 7, "stdout was:\n{stdout}");
    let values = lines
        .iter()
        .map(|line| serde_json::from_str::<Value>(line).expect("line should be json"))
        .collect::<Vec<_>>();
    assert_eq!(values[0]["type"], "session");
    assert_eq!(values[0]["version"], 1);
    let expected_cwd = temp.path().canonicalize().expect("canonical tempdir");
    assert_eq!(values[0]["cwd"], expected_cwd.to_string_lossy().as_ref());
    assert!(values[0]["id"].as_str().is_some_and(|id| !id.is_empty()));
    assert!(
        values[0]["timestamp"]
            .as_str()
            .is_some_and(|timestamp| !timestamp.is_empty())
    );

    let event_types = values
        .iter()
        .filter_map(|value| value["type"].as_str())
        .collect::<Vec<_>>();
    assert!(event_types.contains(&"agent_start"));
    assert!(event_types.contains(&"turn_start"));
    assert!(event_types.contains(&"message_start"));
    assert!(event_types.contains(&"message_update"));
    assert!(event_types.contains(&"message_end"));
    assert!(event_types.contains(&"turn_end"));
    assert!(event_types.contains(&"agent_end"));
    let update = values
        .iter()
        .find(|value| value["type"] == "message_update")
        .expect("message_update event");
    assert_eq!(update["assistantMessageEvent"]["type"], "text_delta");
    assert_eq!(update["assistantMessageEvent"]["delta"], "json text");
    assert_eq!(update["message"]["role"], "assistant");
    assert!(update["message"]["content"][0]["text"].as_str().is_some());
    assert!(!stdout.contains("TextDelta"));
    assert!(!stdout.contains("MessageStarted"));
    assert!(!stdout.contains("fake response"));
    assert!(!stdout.contains("placeholder"));

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["input"][0]["content"], "stream events");
}

#[test]
fn run_output_json_emits_thinking_content_events_from_mock_provider() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_thinking_response_sse(
        "resp-thinking-json",
        "thinking_1",
        &["Checked ", "the plan."],
        "final answer",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["run", "--output", "json", "stream", "thinking"]);

    let stdout = run(command);

    let values = stdout
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("line should be json"))
        .collect::<Vec<_>>();
    let updates = values
        .iter()
        .filter(|value| value["type"] == "message_update")
        .collect::<Vec<_>>();
    assert!(updates.len() >= 4, "stdout was:\n{stdout}");
    assert_eq!(
        updates[0]["assistantMessageEvent"]["type"],
        "thinking_start"
    );
    assert_eq!(updates[0]["assistantMessageEvent"]["contentIndex"], 0);
    assert_eq!(updates[0]["message"]["content"][0]["type"], "thinking");
    assert_eq!(
        updates[1]["assistantMessageEvent"]["type"],
        "thinking_delta"
    );
    assert_eq!(updates[1]["assistantMessageEvent"]["contentIndex"], 0);
    assert_eq!(updates[1]["assistantMessageEvent"]["delta"], "Checked ");
    assert_eq!(
        updates[2]["assistantMessageEvent"]["type"],
        "thinking_delta"
    );
    assert_eq!(updates[2]["assistantMessageEvent"]["delta"], "the plan.");
    assert_eq!(updates[3]["assistantMessageEvent"]["type"], "thinking_end");
    assert_eq!(
        updates[3]["assistantMessageEvent"]["content"],
        "Checked the plan."
    );
    let text_update = updates
        .iter()
        .find(|value| value["assistantMessageEvent"]["type"] == "text_delta")
        .expect("text delta update");
    assert_eq!(text_update["assistantMessageEvent"]["contentIndex"], 1);
    assert_eq!(
        text_update["assistantMessageEvent"]["delta"],
        "final answer"
    );
    assert_eq!(text_update["message"]["content"][0]["type"], "thinking");
    assert_eq!(
        text_update["message"]["content"][0]["thinking"],
        "Checked the plan."
    );
    assert_eq!(text_update["message"]["content"][1]["type"], "text");
    assert_eq!(text_update["message"]["content"][1]["text"], "final answer");
    assert!(!stdout.contains("fake response"));
    assert!(!stdout.contains("placeholder"));

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["input"][0]["content"], "stream thinking");
}

#[test]
fn run_output_json_preserves_multiple_thinking_content_indexes() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_multi_thinking_response_sse(
        "resp-multi-thinking-json",
        "final",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["run", "--output", "json", "stream", "multi", "thinking"]);

    let stdout = run(command);

    let values = stdout
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("line should be json"))
        .collect::<Vec<_>>();
    let updates = values
        .iter()
        .filter(|value| value["type"] == "message_update")
        .collect::<Vec<_>>();
    let event_indexes = updates
        .iter()
        .map(|value| {
            (
                value["assistantMessageEvent"]["type"]
                    .as_str()
                    .expect("event type"),
                value["assistantMessageEvent"]["contentIndex"]
                    .as_u64()
                    .expect("content index"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        event_indexes,
        vec![
            ("text_delta", 0),
            ("thinking_start", 1),
            ("thinking_delta", 1),
            ("thinking_end", 1),
            ("thinking_start", 2),
            ("thinking_delta", 2),
            ("thinking_end", 2),
            ("text_delta", 3),
        ]
    );
    let final_update = updates.last().expect("last update");
    assert_eq!(final_update["message"]["content"][0]["type"], "text");
    assert_eq!(final_update["message"]["content"][0]["text"], "intro ");
    assert_eq!(final_update["message"]["content"][1]["type"], "thinking");
    assert_eq!(
        final_update["message"]["content"][1]["thinking"],
        "first thought"
    );
    assert_eq!(final_update["message"]["content"][2]["type"], "thinking");
    assert_eq!(
        final_update["message"]["content"][2]["thinking"],
        "second thought"
    );
    assert_eq!(final_update["message"]["content"][3]["type"], "text");
    assert_eq!(final_update["message"]["content"][3]["text"], "final");

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["input"][0]["content"],
        "stream multi thinking"
    );
}

#[test]
fn run_mode_json_emits_stable_typed_events_without_output_flag() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-mode-json", "mode json")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["--mode", "json", "run", "stream", "events"]);

    let stdout = run(command);

    let values = stdout
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("line should be json"))
        .collect::<Vec<_>>();
    assert!(values.len() >= 7, "stdout was:\n{stdout}");
    assert_eq!(values[0]["type"], "session");
    assert!(values.iter().any(|value| value["type"] == "message_update"
        && value["assistantMessageEvent"]["delta"] == "mode json"));
    assert!(!stdout.contains("TextDelta"));
    assert!(!stdout.contains("fake response"));
    assert!(!stdout.contains("placeholder"));

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["input"][0]["content"], "stream events");
}

#[test]
fn run_output_events_overrides_global_json_mode() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-events", "event mode")]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args([
            "--mode", "json", "run", "--output", "events", "stream", "events",
        ]);

    let stdout = run(command);

    assert!(stdout.contains("\"TextDelta\":{\"turn\":1,\"text\":\"event mode\"}"));
    assert!(!stdout.contains("\"type\":\"session\""));
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

#[test]
fn print_pi_style_short_no_approve_alias_denies_ask_file_write_tool_and_continues_agent_loop() {
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
            "resp-no-approve-1",
            "call-write-denied",
            "write",
            &json!({
                "path": "denied.txt",
                "content": "should not be written"
            }),
        ),
        openai_response_sse("resp-no-approve-2", "write denied"),
    ]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["-na", "print", "write", "file"]);

    let stdout = run(command);

    assert_eq!(stdout, "write denied\n");
    assert!(
        !temp.path().join("denied.txt").exists(),
        "denied file write should not create a file"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].body["input"][2]["type"], "function_call_output");
    assert_eq!(requests[1].body["input"][2]["call_id"], "call-write-denied");
    let tool_output = requests[1].body["input"][2]["output"]
        .as_str()
        .expect("tool output");
    assert!(tool_output.contains("approval denied"));
    assert!(tool_output.contains("denied.txt"));
}

#[test]
fn print_registers_enabled_extension_tool_and_executes_it_through_agent_loop() {
    let temp = TempDir::new().expect("tempdir");
    let log = write_echo_extension(temp.path());
    let server = MockSseServer::start(vec![
        openai_tool_call_sse(
            "resp-extension-1",
            "call-extension-1",
            "extension__echo__echo",
            &json!({"text": "from model"}),
        ),
        openai_response_sse("resp-extension-2", "extension completed"),
    ]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["print", "use extension"]);

    let stdout = run(command);

    assert_eq!(stdout, "extension completed\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        model_tool_names(&requests[0].body).contains(&"extension__echo__echo"),
        "model tools should include enabled extension tool"
    );
    let tool_output = requests[1].body["input"][2]["output"]
        .as_str()
        .expect("tool output");
    assert!(tool_output.contains("extension echo: from model"));
    let calls = std::fs::read_to_string(log).expect("read extension call log");
    assert!(calls.contains(r#""method": "tools.list""#));
    assert!(calls.contains(r#""method": "tool.echo""#));
}

#[test]
fn print_fails_closed_when_extension_tool_schema_is_not_an_object() {
    let temp = TempDir::new().expect("tempdir");
    write_extension_with_input_schema(temp.path(), &json!("anything"));
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-invalid-extension-schema",
        "should not reach provider",
    )]);

    let output = neo()
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["print", "show tools"])
        .output()
        .expect("neo command should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("extension invalid_schema tool bad_tool"));
    assert!(stderr.contains("input_schema must be a JSON Schema object"));
    assert!(
        server.requests().is_empty(),
        "invalid extension tools must fail before provider calls"
    );
}

#[test]
fn print_fails_closed_when_extension_tool_schema_has_invalid_object_shape() {
    let temp = TempDir::new().expect("tempdir");
    write_extension_with_input_schema(temp.path(), &json!({"type": "definitely-not-a-json-type"}));
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-invalid-extension-schema-object",
        "should not reach provider",
    )]);

    let output = neo()
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["print", "show tools"])
        .output()
        .expect("neo command should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("extension invalid_schema tool bad_tool"));
    assert!(stderr.contains("input_schema has invalid JSON Schema type"));
    assert!(
        server.requests().is_empty(),
        "invalid extension tools must fail before provider calls"
    );
}

#[test]
fn print_pi_style_short_no_builtin_tools_keeps_extension_tools() {
    let temp = TempDir::new().expect("tempdir");
    write_echo_extension(temp.path());
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-extension-no-builtins",
        "extension tools listed",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args(["-nbt", "print", "show tools"]);

    let stdout = run(command);

    assert_eq!(stdout, "extension tools listed\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        model_tool_names(&requests[0].body),
        vec!["extension__echo__echo"]
    );
}

#[test]
fn print_exclude_tools_removes_extension_tools() {
    let temp = TempDir::new().expect("tempdir");
    write_echo_extension(temp.path());
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-extension-excluded",
        "extension excluded",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .args([
            "--tools",
            "extension__echo__echo",
            "--exclude-tools",
            "extension__echo__echo",
            "print",
            "show tools",
        ]);

    let stdout = run(command);

    assert_eq!(stdout, "extension excluded\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].body.get("tools").is_none());
}

#[test]
fn print_extension_flag_registers_explicit_extension_path_without_installing_it() {
    let temp = TempDir::new().expect("tempdir");
    let explicit = temp.path().join("external-extension");
    let log = write_echo_extension_at(&explicit);
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-explicit-extension",
        "explicit extension loaded",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .arg("--extension")
        .arg(&explicit)
        .args(["print", "show tools"]);

    let stdout = run(command);

    assert_eq!(stdout, "explicit extension loaded\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        model_tool_names(&requests[0].body).contains(&"extension__echo__echo"),
        "explicit extension path should register runtime tool"
    );
    assert!(log.exists(), "explicit extension should be queried");
    assert!(
        !temp.path().join(".neo/extensions/echo").exists(),
        "explicit --extension should not install into the project extension store"
    );
}

#[test]
fn print_no_extensions_disables_project_extensions_but_keeps_explicit_extension_flag() {
    let temp = TempDir::new().expect("tempdir");
    write_echo_extension(temp.path());
    let explicit = temp.path().join("external-extension");
    write_named_echo_extension_at(&explicit, "explicit_echo");
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-no-extensions",
        "explicit extension only",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .arg("--no-extensions")
        .arg("--extension")
        .arg(&explicit)
        .args(["print", "show tools"]);

    let stdout = run(command);

    assert_eq!(stdout, "explicit extension only\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let tools = model_tool_names(&requests[0].body);
    assert!(tools.contains(&"extension__explicit_echo__echo"));
    assert!(!tools.contains(&"extension__echo__echo"));
}

#[test]
fn print_pi_style_short_no_extensions_alias_disables_project_extensions() {
    let temp = TempDir::new().expect("tempdir");
    write_echo_extension(temp.path());
    let server = MockSseServer::start(vec![openai_response_sse(
        "resp-short-no-extensions",
        "extensions disabled",
    )]);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .arg("-ne")
        .args(["print", "show tools"]);

    let stdout = run(command);

    assert_eq!(stdout, "extensions disabled\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(!model_tool_names(&requests[0].body).contains(&"extension__echo__echo"));
}

fn write_echo_extension(root: &std::path::Path) -> std::path::PathBuf {
    write_echo_extension_at(&root.join(".neo/extensions/echo"))
}

fn write_extension_with_input_schema(root: &std::path::Path, input_schema: &Value) {
    let extension = root.join(".neo/extensions/invalid-schema");
    std::fs::create_dir_all(&extension).expect("create extension");
    let script = extension.join("invalid_schema.py");
    std::fs::write(
        &script,
        format!(
            r#"
import json
import sys

input_schema = {input_schema}

for line in sys.stdin:
    message = json.loads(line)
    method = message["method"]
    if method == "tools.list":
        result = [{{
            "name": "bad_tool",
            "description": "Tool with an invalid schema",
            "input_schema": input_schema,
            "method": "tool.bad"
        }}]
    else:
        result = {{"content": "unexpected"}}
    print(json.dumps({{"type": "response", "id": message["id"], "result": result}}), flush=True)
"#
        ),
    )
    .expect("write extension script");
    std::fs::write(
        extension.join("neo-extension.toml"),
        format!(
            r#"
id = "invalid_schema"
name = "Invalid Schema"
version = "0.1.0"

[runner]
command = "python3"
args = [{script}]
"#,
            script = serde_json::to_string(&script).expect("script path json")
        ),
    )
    .expect("write extension manifest");
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

fn session_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    session_files_in(&root.join(".neo/sessions"))
}

fn session_files_in(sessions_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut entries = std::fs::read_dir(sessions_dir)
        .expect("read sessions")
        .map(|entry| entry.expect("session entry").path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
        .collect::<Vec<_>>();
    entries.sort();
    entries
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

fn openai_thinking_response_sse(
    id: &str,
    thinking_id: &str,
    deltas: &[&str],
    text: &str,
) -> String {
    let mut events = vec![
        json!({ "type": "response.created", "response": { "id": id } }),
        json!({
            "type": "response.reasoning_summary_part.added",
            "item_id": thinking_id,
            "summary_index": 0,
            "part": { "type": "summary_text", "text": "" }
        }),
    ];
    for delta in deltas {
        events.push(json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": thinking_id,
            "summary_index": 0,
            "delta": delta
        }));
    }
    events.extend([
        json!({
            "type": "response.reasoning_summary_part.done",
            "item_id": thinking_id,
            "summary_index": 0,
            "part": { "type": "summary_text", "text": deltas.join("") }
        }),
        json!({ "type": "response.output_text.delta", "delta": text }),
        json!({
            "type": "response.completed",
            "response": {
                "status": "completed",
                "usage": { "input_tokens": 7, "output_tokens": 3 }
            }
        }),
    ]);
    sse_response(&events)
}

fn openai_multi_thinking_response_sse(id: &str, final_text: &str) -> String {
    sse_response(&[
        json!({ "type": "response.created", "response": { "id": id } }),
        json!({ "type": "response.output_text.delta", "delta": "intro " }),
        json!({
            "type": "response.reasoning_summary_part.added",
            "item_id": "thinking-1",
            "summary_index": 0,
            "part": { "type": "summary_text", "text": "" }
        }),
        json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": "thinking-1",
            "summary_index": 0,
            "delta": "first thought"
        }),
        json!({
            "type": "response.reasoning_summary_part.done",
            "item_id": "thinking-1",
            "summary_index": 0,
            "part": { "type": "summary_text", "text": "first thought" }
        }),
        json!({
            "type": "response.reasoning_summary_part.added",
            "item_id": "thinking-2",
            "summary_index": 1,
            "part": { "type": "summary_text", "text": "" }
        }),
        json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": "thinking-2",
            "summary_index": 1,
            "delta": "second thought"
        }),
        json!({
            "type": "response.reasoning_summary_part.done",
            "item_id": "thinking-2",
            "summary_index": 1,
            "part": { "type": "summary_text", "text": "second thought" }
        }),
        json!({ "type": "response.output_text.delta", "delta": final_text }),
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
