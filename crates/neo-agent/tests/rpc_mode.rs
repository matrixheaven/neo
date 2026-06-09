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
fn rpc_get_state_reports_project_runtime_state() {
    let temp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    std::fs::create_dir_all(temp.path().join(".neo/sessions")).expect("create sessions");
    std::fs::write(temp.path().join(".neo/sessions/alpha.jsonl"), "{}\n").expect("write session");
    std::fs::write(
        temp.path().join(".neo/config.toml"),
        r#"
default_provider = "anthropic"
default_model = "claude-sonnet-4-5"
"#,
    )
    .expect("write config");

    let mut command = neo();
    command.current_dir(temp.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        r#"{"type":"request","id":"state-1","method":"get_state","params":{}}"#,
    );

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "state-1");
    assert_eq!(messages[0]["result"]["provider"], "anthropic");
    assert_eq!(messages[0]["result"]["model"], "claude-sonnet-4-5");
    assert!(messages[0]["result"]["is_streaming"].is_null());
    assert!(
        messages[0]["result"]["sessions_dir"]
            .as_str()
            .expect("sessions dir")
            .ends_with(".neo/sessions")
    );
    assert_eq!(messages[0]["result"]["session_count"], 1);
}

#[test]
fn rpc_get_messages_replays_session_jsonl_messages() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    std::fs::create_dir_all(&sessions).expect("create sessions");
    std::fs::write(
        sessions.join("alpha.jsonl"),
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello rpc history\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"hi from jsonl\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    )
    .expect("write session");

    let mut command = neo();
    command.current_dir(temp.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        r#"{"type":"request","id":"messages-1","method":"get_messages","params":{"session_id":"alpha"}}"#,
    );

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "messages-1");
    assert_eq!(messages[0]["result"]["session_id"], "alpha");
    assert_eq!(
        messages[0]["result"]["messages"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        messages[0]["result"]["messages"][0]["User"]["content"][0]["Text"]["text"],
        "hello rpc history"
    );
    assert_eq!(
        messages[0]["result"]["messages"][1]["Assistant"]["content"][0]["Text"]["text"],
        "hi from jsonl"
    );
}

#[test]
fn rpc_get_messages_returns_empty_replay_for_empty_session() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    std::fs::create_dir_all(&sessions).expect("create sessions");
    std::fs::write(sessions.join("empty.jsonl"), "").expect("write empty session");

    let mut command = neo();
    command.current_dir(temp.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        r#"{"type":"request","id":"messages-empty","method":"get_messages","params":{"session_id":"empty"}}"#,
    );

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "messages-empty");
    assert_eq!(messages[0]["result"]["session_id"], "empty");
    assert_eq!(
        messages[0]["result"]["messages"].as_array().unwrap().len(),
        0
    );
}

#[test]
fn rpc_get_messages_resolves_unique_session_prefix() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    std::fs::create_dir_all(&sessions).expect("create sessions");
    std::fs::write(sessions.join("alpha-main.jsonl"), "").expect("write session");

    let mut command = neo();
    command.current_dir(temp.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        r#"{"type":"request","id":"messages-prefix","method":"get_messages","params":{"session_id":"alpha"}}"#,
    );

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "messages-prefix");
    assert_eq!(messages[0]["result"]["session_id"], "alpha-main");
    assert_eq!(
        messages[0]["result"]["messages"].as_array().unwrap().len(),
        0
    );
}

#[test]
fn rpc_get_messages_accepts_in_directory_jsonl_path() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    std::fs::create_dir_all(&sessions).expect("create sessions");
    let session_path = sessions.join("alpha-main.jsonl");
    std::fs::write(&session_path, "").expect("write session");

    let mut command = neo();
    command.current_dir(temp.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        &format!(
            r#"{{"type":"request","id":"messages-path","method":"get_messages","params":{{"session_id":{}}}}}"#,
            serde_json::to_string(session_path.to_str().expect("session path")).expect("json path")
        ),
    );

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "messages-path");
    assert_eq!(messages[0]["result"]["session_id"], "alpha-main");
    assert_eq!(
        messages[0]["result"]["messages"].as_array().unwrap().len(),
        0
    );
}

#[test]
fn rpc_get_messages_reports_missing_session_as_invalid_params() {
    let temp = TempDir::new().expect("tempdir");

    let mut command = neo();
    command.current_dir(temp.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        r#"{"type":"request","id":"messages-missing","method":"get_messages","params":{"session_id":"missing"}}"#,
    );

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "messages-missing");
    assert_eq!(messages[0]["error"]["code"], "invalid_params");
    assert!(
        messages[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("missing")
    );
}

#[test]
fn rpc_sessions_list_returns_local_session_metadata() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    std::fs::create_dir_all(&sessions).expect("create sessions");
    std::fs::write(sessions.join("alpha.jsonl"), "{}\n").expect("write parent session");
    std::fs::write(sessions.join("alpha-fork-1.jsonl"), "{}\n").expect("write child session");
    std::fs::write(
        sessions.join("sessions.metadata.json"),
        json!({
            "sessions": {
                "alpha": {
                    "name": "Main thread",
                    "summary": "Local branch summary"
                },
                "alpha-fork-1": {
                    "name": "Parser branch",
                    "parent_id": "alpha"
                }
            }
        })
        .to_string(),
    )
    .expect("write metadata");

    let mut command = neo();
    command.current_dir(temp.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        r#"{"type":"request","id":"sessions-list","method":"sessions.list","params":{}}"#,
    );

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "sessions-list");
    let sessions = messages[0]["result"]["sessions"]
        .as_array()
        .expect("sessions array");
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0]["id"], "alpha");
    assert_eq!(sessions[0]["name"], "Main thread");
    assert_eq!(sessions[0]["summary"], "Local branch summary");
    assert!(sessions[0]["parent_id"].is_null());
    assert_eq!(sessions[0]["children"], json!(["alpha-fork-1"]));
    assert_eq!(sessions[1]["id"], "alpha-fork-1");
    assert_eq!(sessions[1]["name"], "Parser branch");
    assert_eq!(sessions[1]["parent_id"], "alpha");
}

#[test]
fn rpc_sessions_tree_returns_depth_ordered_local_session_tree() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    std::fs::create_dir_all(&sessions).expect("create sessions");
    std::fs::write(sessions.join("alpha.jsonl"), "{}\n").expect("write parent session");
    std::fs::write(sessions.join("alpha-fork-1.jsonl"), "{}\n").expect("write child session");
    std::fs::write(sessions.join("orphan.jsonl"), "{}\n").expect("write orphan session");
    std::fs::write(
        sessions.join("sessions.metadata.json"),
        json!({
            "sessions": {
                "alpha": {
                    "name": "Main thread"
                },
                "alpha-fork-1": {
                    "name": "Parser branch",
                    "summary": "Investigates parser state",
                    "parent_id": "alpha"
                },
                "orphan": {
                    "parent_id": "missing-parent"
                }
            }
        })
        .to_string(),
    )
    .expect("write metadata");

    let mut command = neo();
    command.current_dir(temp.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        r#"{"type":"request","id":"sessions-tree","method":"sessions.tree","params":{}}"#,
    );

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "sessions-tree");
    let tree = messages[0]["result"]["tree"]
        .as_array()
        .expect("tree array");
    let ids = tree
        .iter()
        .map(|record| record["record"]["id"].as_str().expect("id"))
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["alpha", "alpha-fork-1", "orphan"]);
    assert_eq!(tree[0]["depth"], 0);
    assert_eq!(tree[0]["record"]["name"], "Main thread");
    assert_eq!(tree[1]["depth"], 1);
    assert_eq!(tree[1]["record"]["parent_id"], "alpha");
    assert_eq!(tree[1]["record"]["summary"], "Investigates parser state");
    assert_eq!(tree[2]["depth"], 0);
    assert_eq!(tree[2]["record"]["parent_id"], "missing-parent");
}

#[test]
fn rpc_sessions_get_returns_local_session_metadata_and_messages() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    std::fs::create_dir_all(&sessions).expect("create sessions");
    std::fs::write(
        sessions.join("alpha-main.jsonl"),
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello session get\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"session get reply\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    )
    .expect("write session");
    std::fs::write(sessions.join("alpha-main-fork-1.jsonl"), "{}\n").expect("write child session");
    std::fs::write(
        sessions.join("sessions.metadata.json"),
        json!({
            "sessions": {
                "alpha-main": {
                    "name": "Main thread",
                    "summary": "Resolved local branch summary"
                },
                "alpha-main-fork-1": {
                    "parent_id": "alpha-main"
                }
            }
        })
        .to_string(),
    )
    .expect("write metadata");

    let mut command = neo();
    command.current_dir(temp.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        r#"{"type":"request","id":"sessions-get","method":"sessions.get","params":{"session_id":"alpha-main"}}"#,
    );

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "sessions-get");
    assert_eq!(messages[0]["result"]["id"], "alpha-main");
    assert_eq!(messages[0]["result"]["name"], "Main thread");
    assert_eq!(
        messages[0]["result"]["summary"],
        "Resolved local branch summary"
    );
    assert!(messages[0]["result"]["parent_id"].is_null());
    assert_eq!(
        messages[0]["result"]["children"],
        json!(["alpha-main-fork-1"])
    );
    assert!(
        messages[0]["result"]["path"]
            .as_str()
            .expect("session path")
            .ends_with(".neo/sessions/alpha-main.jsonl")
    );
    assert_eq!(
        messages[0]["result"]["messages"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        messages[0]["result"]["messages"][0]["User"]["content"][0]["Text"]["text"],
        "hello session get"
    );
    assert_eq!(
        messages[0]["result"]["messages"][1]["Assistant"]["content"][0]["Text"]["text"],
        "session get reply"
    );
}

#[test]
fn rpc_sessions_get_resolves_unique_session_prefix() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    std::fs::create_dir_all(&sessions).expect("create sessions");
    std::fs::write(sessions.join("alpha-main.jsonl"), "").expect("write session");

    let mut command = neo();
    command.current_dir(temp.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        r#"{"type":"request","id":"sessions-get-prefix","method":"sessions.get","params":{"session_id":"alpha"}}"#,
    );

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "sessions-get-prefix");
    assert_eq!(messages[0]["result"]["id"], "alpha-main");
    assert_eq!(
        messages[0]["result"]["messages"].as_array().unwrap().len(),
        0
    );
}

#[test]
fn rpc_sessions_get_reports_missing_session_as_invalid_params() {
    let temp = TempDir::new().expect("tempdir");

    let mut command = neo();
    command.current_dir(temp.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        r#"{"type":"request","id":"sessions-get-missing","method":"sessions.get","params":{"session_id":"missing"}}"#,
    );

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "sessions-get-missing");
    assert_eq!(messages[0]["error"]["code"], "invalid_params");
    assert!(
        messages[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("missing")
    );
}

#[test]
fn rpc_sessions_export_html_returns_rendered_local_session() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    std::fs::create_dir_all(&sessions).expect("create sessions");
    std::fs::write(
        sessions.join("alpha.jsonl"),
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello html export\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"rendered **bold** local reply <script>alert(1)</script>\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    )
    .expect("write session");

    let mut command = neo();
    command.current_dir(temp.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        r#"{"type":"request","id":"export-1","method":"sessions.export_html","params":{"session_id":"alpha"}}"#,
    );

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "export-1");
    assert_eq!(messages[0]["result"]["session_id"], "alpha");
    let html = messages[0]["result"]["html"]
        .as_str()
        .expect("rendered html");
    assert!(html.contains("<!doctype html>"));
    assert!(html.contains("<title>neo session alpha</title>"));
    assert!(html.contains("hello html export"));
    assert!(html.contains("rendered <strong>bold</strong> local reply"));
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(!html.contains("<script>alert(1)</script>"));
}

#[test]
fn rpc_get_commands_returns_local_prompt_template_commands() {
    let home = TempDir::new().expect("home tempdir");
    let project = TempDir::new().expect("project tempdir");
    std::fs::create_dir_all(project.path().join(".neo")).expect("create .neo");
    std::fs::write(
        project.path().join(".neo/config.toml"),
        r#"
prompt_templates = ["prompts"]
"#,
    )
    .expect("write config");
    std::fs::create_dir_all(project.path().join("prompts")).expect("create configured prompts");
    std::fs::write(
        project.path().join("prompts/review.md"),
        r#"---
description: Review a target
argument-hint: "<path>"
---
Review $1
"#,
    )
    .expect("write configured prompt template");
    std::fs::create_dir_all(project.path().join(".neo/prompts")).expect("create project prompts");
    std::fs::write(
        project.path().join(".neo/prompts/fix.md"),
        "Fix the selected code\n",
    )
    .expect("write project prompt template");
    std::fs::write(
        project.path().join(".neo/prompts/review.md"),
        "Project review should not shadow configured review\n",
    )
    .expect("write duplicate project prompt template");
    std::fs::create_dir_all(home.path().join(".neo/prompts")).expect("create user prompts");
    std::fs::write(
        home.path().join(".neo/prompts/explain.md"),
        "Explain the target\n",
    )
    .expect("write user prompt template");
    std::fs::write(
        home.path().join(".neo/prompts/fix.md"),
        "User fix should not shadow project fix\n",
    )
    .expect("write duplicate user prompt template");

    let mut command = neo();
    command
        .current_dir(project.path())
        .env("HOME", home.path())
        .arg("rpc");
    let stdout = run_with_stdin(
        command,
        r#"{"type":"request","id":"commands-1","method":"get_commands","params":{}}"#,
    );

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "commands-1");
    let commands = messages[0]["result"]["commands"]
        .as_array()
        .expect("commands array");
    let names = commands
        .iter()
        .map(|command| command["name"].as_str().expect("name"))
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["/explain", "/fix", "/review"]);

    let review = commands
        .iter()
        .find(|command| command["name"] == "/review")
        .expect("review command");
    assert_eq!(review["kind"], "prompt_template");
    assert_eq!(review["template"], "review");
    assert_eq!(review["description"], "Review a target");
    assert_eq!(review["argument_hint"], "<path>");
    assert_eq!(review["location"], "configured");
    assert!(
        review["path"]
            .as_str()
            .unwrap()
            .ends_with("prompts/review.md")
    );

    let fix = commands
        .iter()
        .find(|command| command["name"] == "/fix")
        .expect("fix command");
    assert_eq!(fix["description"], "Fix the selected code");
    assert_eq!(fix["location"], "project");

    let explain = commands
        .iter()
        .find(|command| command["name"] == "/explain")
        .expect("explain command");
    assert_eq!(explain["location"], "user");
    assert!(
        explain["path"]
            .as_str()
            .unwrap()
            .ends_with(".neo/prompts/explain.md")
    );
}

#[test]
fn rpc_get_commands_omits_excluded_auto_discovered_prompt_template() {
    let project = TempDir::new().expect("project tempdir");
    std::fs::create_dir_all(project.path().join(".neo/prompts")).expect("create project prompts");
    std::fs::write(
        project.path().join(".neo/prompts/review.md"),
        "Review should be excluded\n",
    )
    .expect("write excluded prompt template");
    std::fs::write(project.path().join(".neo/prompts/fix.md"), "Fix remains\n")
        .expect("write kept prompt template");
    std::fs::write(
        project.path().join(".neo/config.toml"),
        r#"
prompt_templates = ["-prompts/review.md"]
"#,
    )
    .expect("write config");

    let mut command = neo();
    command.current_dir(project.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        r#"{"type":"request","id":"commands-1","method":"get_commands","params":{}}"#,
    );

    let messages = parse_jsonl(&stdout);
    let commands = messages[0]["result"]["commands"]
        .as_array()
        .expect("commands array");
    let names = commands
        .iter()
        .map(|command| command["name"].as_str().expect("name"))
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["/fix"]);
}

#[test]
fn rpc_prompt_streams_agent_events_and_returns_assistant_text() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![openai_response_sse("resp-rpc", "rpc answer")]);
    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .arg("--api-base")
        .arg(&server.url)
        .arg("rpc");

    let stdout = run_with_stdin(
        command,
        r#"{"type":"request","id":"prompt-1","method":"prompt","params":{"message":"hello rpc"}}"#,
    );

    let messages = parse_jsonl(&stdout);
    assert!(
        messages.iter().any(|message| {
            message["type"] == "notification"
                && message["method"] == "agent.event"
                && message["params"].to_string().contains("TextDelta")
        }),
        "RPC prompt should stream agent events: {messages:?}"
    );
    let response = messages.last().expect("response should be last");
    assert_eq!(response["type"], "response");
    assert_eq!(response["id"], "prompt-1");
    assert_eq!(response["result"]["assistant_text"], "rpc answer");

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/responses");
    assert_eq!(requests[0].body["input"][0]["content"], "hello rpc");
}

fn parse_jsonl(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid JSONL response"))
        .collect()
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

    RecordedRequest { method, path, body }
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}
