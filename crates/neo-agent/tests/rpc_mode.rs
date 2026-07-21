use std::{
    collections::BTreeMap,
    fmt::Write as _,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

use serde_json::{Value, json};
use tempfile::TempDir;

const SESSION_A: &str = "session_00000000-0000-4000-8000-000000000301";
const SESSION_CHILD: &str = "session_00000000-0000-4000-8000-000000000303";
const SESSION_EMPTY: &str = "session_00000000-0000-4000-8000-000000000304";

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

// (ISOLATED_HOMES removed — isolation is now per-thread via thread_local)

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
    let home = isolated_home();
    command.env("NEO_HOME", &home);
    command.env("HOME", &home);
    command
}

fn isolated_home() -> std::path::PathBuf {
    thread_local! {
        static HOME: std::cell::OnceCell<(TempDir, std::path::PathBuf)> = const { std::cell::OnceCell::new() };
    }
    HOME.with(|cell| {
        let (_, path) = cell.get_or_init(|| {
            let home = TempDir::new().expect("isolated home");
            let path = home.path().to_path_buf();
            (home, path)
        });
        path.clone()
    })
}

fn sessions_metadata_json(entries: &[(&str, Value)]) -> String {
    let mut sessions = serde_json::Map::new();
    for (id, value) in entries {
        sessions.insert((*id).to_owned(), value.clone());
    }
    json!({ "sessions": sessions }).to_string()
}

fn write_home_config(content: &str) {
    let config_dir = isolated_home();
    std::fs::create_dir_all(&config_dir).expect("create .neo");
    std::fs::write(config_dir.join("config.toml"), content).expect("write config");
}

fn session_bucket(project_dir: &Path) -> PathBuf {
    let sessions_root = isolated_home().join("sessions");
    neo_agent_core::session::workspace_sessions_dir(&sessions_root, project_dir)
}

fn write_session_transcript(sessions: &Path, session_id: &str, content: &str) -> PathBuf {
    let session_dir = sessions.join(session_id);
    let wire = neo_agent_core::session::main_agent_wire_path(&session_dir);
    std::fs::create_dir_all(wire.parent().expect("wire parent")).expect("create wire dir");
    std::fs::write(&wire, content).expect("write main wire");
    std::fs::write(
        neo_agent_core::session::session_state_path(&session_dir),
        "{\"schema_version\":1,\"agents\":{\"main\":{\"kind\":\"main\",\"record_dir\":\"agents/main\"}}}\n",
    )
    .expect("write session state");
    wire
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
    let sessions = session_bucket(temp.path());
    std::fs::create_dir_all(&sessions).expect("create sessions");
    write_session_transcript(&sessions, SESSION_A, "{}\n");
    std::fs::write(
        isolated_home().join("config.toml"),
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
            .ends_with("sessions")
    );
    assert_eq!(messages[0]["result"]["session_count"], 1);
}

#[test]
fn config_mode_rpc_uses_the_real_rpc_loop_without_subcommand() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = session_bucket(temp.path());
    std::fs::create_dir_all(&sessions).expect("create sessions");
    write_session_transcript(&sessions, SESSION_A, "{}\n");
    std::fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    std::fs::write(
        isolated_home().join("config.toml"),
        r#"
[defaults]
mode = "rpc"
"#,
    )
    .expect("write config");

    let mut command = neo();
    command.current_dir(temp.path());
    let stdout = run_with_stdin(
        command,
        r#"{"type":"request","id":"state-mode-rpc","method":"get_state","params":{}}"#,
    );

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "state-mode-rpc");
    assert_eq!(messages[0]["result"]["session_count"], 1);
    assert_eq!(messages[0]["result"]["mode"], "rpc");
}

#[test]
fn rpc_get_messages_replays_session_jsonl_messages() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = session_bucket(temp.path());
    std::fs::create_dir_all(&sessions).expect("create sessions");
    write_session_transcript(
        &sessions,
        SESSION_A,
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello rpc history\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"hi from jsonl\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    );

    let mut command = neo();
    command.current_dir(temp.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        &format!(
            r#"{{"type":"request","id":"messages-1","method":"get_messages","params":{{"session_id":"{SESSION_A}"}}}}"#
        ),
    );

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "messages-1");
    assert_eq!(messages[0]["result"]["session_id"], SESSION_A);
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
    let sessions = session_bucket(temp.path());
    std::fs::create_dir_all(&sessions).expect("create sessions");
    write_session_transcript(&sessions, SESSION_EMPTY, "");

    let mut command = neo();
    command.current_dir(temp.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        &format!(
            r#"{{"type":"request","id":"messages-empty","method":"get_messages","params":{{"session_id":"{SESSION_EMPTY}"}}}}"#
        ),
    );

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "messages-empty");
    assert_eq!(messages[0]["result"]["session_id"], SESSION_EMPTY);
    assert_eq!(
        messages[0]["result"]["messages"].as_array().unwrap().len(),
        0
    );
}

#[test]
fn rpc_session_methods_reject_invalid_or_missing_ids() {
    #[derive(Debug)]
    struct Case {
        method: &'static str,
        session_id: &'static str,
        expected: &'static str,
        create_existing_session: bool,
    }

    let cases = [
        Case {
            method: "get_messages",
            session_id: "session_",
            expected: "invalid session id",
            create_existing_session: true,
        },
        Case {
            method: "get_messages",
            session_id: "missing",
            expected: "missing",
            create_existing_session: false,
        },
        Case {
            method: "sessions.get",
            session_id: "session_",
            expected: "invalid session id",
            create_existing_session: true,
        },
        Case {
            method: "sessions.get",
            session_id: "missing",
            expected: "missing",
            create_existing_session: false,
        },
    ];

    for (i, case) in cases.iter().enumerate() {
        let temp = TempDir::new().expect("tempdir");
        if case.create_existing_session {
            let sessions = session_bucket(temp.path());
            std::fs::create_dir_all(&sessions).expect("create sessions");
            write_session_transcript(&sessions, SESSION_A, "");
        }
        let mut command = neo();
        command.current_dir(temp.path()).arg("rpc");
        let request = format!(
            r#"{{"type":"request","id":"req-{i}","method":"{method}","params":{{"session_id":"{session_id}"}}}}"#,
            method = case.method,
            session_id = case.session_id,
            i = i,
        );
        let stdout = run_with_stdin(command, &request);

        let messages = parse_jsonl(&stdout);
        assert_eq!(messages.len(), 1, "case {i}: {case:?}");
        assert_eq!(messages[0]["type"], "response", "case {i}: {case:?}");
        assert_eq!(messages[0]["id"], format!("req-{i}"), "case {i}: {case:?}");
        assert_eq!(
            messages[0]["error"]["code"], "invalid_params",
            "case {i}: {case:?}"
        );
        let message = messages[0]["error"]["message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {i}: missing error message"));
        assert!(
            message.contains(case.expected),
            "case {i} ({method} {session_id}): expected to contain {expected:?}, got {message:?}",
            i = i,
            method = case.method,
            session_id = case.session_id,
            expected = case.expected,
        );
    }
}

#[test]
fn rpc_get_messages_accepts_in_directory_jsonl_path() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = session_bucket(temp.path());
    std::fs::create_dir_all(&sessions).expect("create sessions");
    let session_path = write_session_transcript(&sessions, SESSION_A, "");

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
    assert_eq!(messages[0]["result"]["session_id"], SESSION_A);
    assert_eq!(
        messages[0]["result"]["messages"].as_array().unwrap().len(),
        0
    );
}

#[test]
fn rpc_sessions_list_returns_local_session_metadata() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = session_bucket(temp.path());
    std::fs::create_dir_all(&sessions).expect("create sessions");
    write_session_transcript(&sessions, SESSION_A, "{}\n");
    write_session_transcript(&sessions, SESSION_CHILD, "{}\n");
    std::fs::write(
        sessions.join("sessions.metadata.json"),
        sessions_metadata_json(&[
            (
                SESSION_A,
                json!({
                    "name": "Main thread",
                    "summary": "Local branch summary"
                }),
            ),
            (
                SESSION_CHILD,
                json!({
                    "name": "Parser branch",
                    "parent_id": SESSION_A
                }),
            ),
        ]),
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
    let alpha = sessions
        .iter()
        .find(|session| session["id"] == SESSION_A)
        .expect("alpha session");
    let child = sessions
        .iter()
        .find(|session| session["id"] == SESSION_CHILD)
        .expect("child session");
    assert_eq!(alpha["name"], "Main thread");
    assert_eq!(alpha["title"], "Main thread");
    assert_eq!(alpha["summary"], "Local branch summary");
    assert!(alpha["parent_id"].is_null());
    assert_eq!(alpha["children"], json!([SESSION_CHILD]));
    assert_eq!(child["name"], "Parser branch");
    assert_eq!(child["title"], "Parser branch");
    assert_eq!(child["parent_id"], SESSION_A);
}

#[test]
fn rpc_sessions_tree_method_is_not_exposed() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = session_bucket(temp.path());
    std::fs::create_dir_all(&sessions).expect("create sessions");
    write_session_transcript(&sessions, SESSION_A, "{}\n");

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
    assert_eq!(messages[0]["error"]["code"], "method_not_found");
}

#[test]
fn rpc_sessions_get_returns_local_session_metadata_and_messages() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = session_bucket(temp.path());
    std::fs::create_dir_all(&sessions).expect("create sessions");
    write_session_transcript(
        &sessions,
        SESSION_A,
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello session get\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"session get reply\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    );
    write_session_transcript(&sessions, SESSION_CHILD, "{}\n");
    std::fs::write(
        sessions.join("sessions.metadata.json"),
        sessions_metadata_json(&[
            (
                SESSION_A,
                json!({
                    "name": "Main thread",
                    "summary": "Resolved local branch summary"
                }),
            ),
            (
                SESSION_CHILD,
                json!({
                    "parent_id": SESSION_A
                }),
            ),
        ]),
    )
    .expect("write metadata");

    let mut command = neo();
    command.current_dir(temp.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        &format!(
            r#"{{"type":"request","id":"sessions-get","method":"sessions.get","params":{{"session_id":"{SESSION_A}"}}}}"#
        ),
    );

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "sessions-get");
    assert_eq!(messages[0]["result"]["id"], SESSION_A);
    assert_eq!(messages[0]["result"]["name"], "Main thread");
    assert_eq!(
        messages[0]["result"]["summary"],
        "Resolved local branch summary"
    );
    assert!(messages[0]["result"]["parent_id"].is_null());
    assert_eq!(messages[0]["result"]["children"], json!([SESSION_CHILD]));
    let returned_session_path = Path::new(
        messages[0]["result"]["path"]
            .as_str()
            .expect("session path"),
    );
    assert!(returned_session_path.ends_with(Path::new("agents").join("main").join("wire.jsonl")));
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
fn rpc_sessions_export_html_returns_rendered_local_session() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = session_bucket(temp.path());
    std::fs::create_dir_all(&sessions).expect("create sessions");
    write_session_transcript(
        &sessions,
        SESSION_A,
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello html export\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"rendered **bold** local reply <script>alert(1)</script>\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    );

    let mut command = neo();
    command.current_dir(temp.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        &format!(
            r#"{{"type":"request","id":"export-1","method":"sessions.export_html","params":{{"session_id":"{SESSION_A}"}}}}"#
        ),
    );

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "export-1");
    assert_eq!(messages[0]["result"]["session_id"], SESSION_A);
    let html = messages[0]["result"]["html"]
        .as_str()
        .expect("rendered html");
    assert!(html.contains("<!doctype html>"));
    assert!(html.contains(&format!("<title>neo session {SESSION_A}</title>")));
    assert!(html.contains("hello html export"));
    assert!(html.contains("rendered <strong>bold</strong> local reply"));
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(!html.contains("<script>alert(1)</script>"));
}

#[test]
fn rpc_sessions_export_json_returns_sanitized_replayed_session_artifact() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = session_bucket(temp.path());
    std::fs::create_dir_all(&sessions).expect("create sessions");
    write_session_transcript(
        &sessions,
        SESSION_A,
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello rpc json export\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"rpc portable reply\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    );
    write_session_transcript(&sessions, SESSION_CHILD, "{}\n");
    std::fs::write(
        sessions.join("sessions.metadata.json"),
        sessions_metadata_json(&[
            (
                SESSION_A,
                json!({
                    "name": "Main thread",
                    "summary": "Resolved local branch summary"
                }),
            ),
            (
                SESSION_CHILD,
                json!({
                    "parent_id": SESSION_A
                }),
            ),
        ]),
    )
    .expect("write metadata");

    let mut command = neo();
    command.current_dir(temp.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        &format!(
            r#"{{"type":"request","id":"export-json-1","method":"sessions.export_json","params":{{"session_id":"{SESSION_A}"}}}}"#
        ),
    );

    assert!(
        !stdout.contains(temp.path().to_str().expect("temp path")),
        "export JSON should not leak absolute paths: {stdout}"
    );
    assert!(!stdout.contains("share_url"));

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "export-json-1");
    let artifact = &messages[0]["result"];
    assert_eq!(artifact["format"], "neo.session.export_json");
    assert_eq!(artifact["schema_version"], 1);
    assert_eq!(artifact["metadata"]["id"], SESSION_A);
    assert_eq!(artifact["metadata"]["name"], "Main thread");
    assert_eq!(
        artifact["metadata"]["summary"],
        "Resolved local branch summary"
    );
    assert!(artifact["metadata"]["parent_id"].is_null());
    assert_eq!(artifact["metadata"]["children"], json!([SESSION_CHILD]));
    assert_eq!(artifact["metadata"]["message_count"], 2);
    assert_eq!(
        artifact["messages"][0]["User"]["content"][0]["Text"]["text"],
        "hello rpc json export"
    );
    assert_eq!(
        artifact["messages"][1]["Assistant"]["content"][0]["Text"]["text"],
        "rpc portable reply"
    );
}

#[test]
fn rpc_set_session_name_updates_local_session_metadata() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = session_bucket(temp.path());
    std::fs::create_dir_all(&sessions).expect("create sessions");
    write_session_transcript(&sessions, SESSION_A, "{}\n");

    let mut command = neo();
    command.current_dir(temp.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        &format!(
            r#"{{"type":"request","id":"rename-1","method":"set_session_name","params":{{"session_id":"{SESSION_A}","name":"Feature branch"}}}}"#
        ),
    );

    let messages = parse_jsonl(&stdout);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["type"], "response");
    assert_eq!(messages[0]["id"], "rename-1");
    assert_eq!(messages[0]["result"]["session_id"], SESSION_A);
    assert_eq!(messages[0]["result"]["name"], "Feature branch");

    let mut command = neo();
    command.current_dir(temp.path()).arg("rpc");
    let stdout = run_with_stdin(
        command,
        r#"{"type":"request","id":"sessions-list","method":"sessions.list","params":{}}"#,
    );
    let messages = parse_jsonl(&stdout);
    assert_eq!(messages[0]["result"]["sessions"][0]["id"], SESSION_A);
    assert_eq!(
        messages[0]["result"]["sessions"][0]["name"],
        "Feature branch"
    );
}

#[test]
fn rpc_get_commands_returns_local_prompt_template_commands() {
    // Prompt templates now live only under the single neo home (~/.neo/prompts).
    // There is no project tier, so configured selectors + user prompts are the
    // only sources.
    let project = TempDir::new().expect("project tempdir");
    write_home_config(
        r#"
prompt_templates = ["prompts"]
"#,
    );
    // Configured prompt template (relative selector resolved against home).
    let configured = isolated_home().join("prompts");
    std::fs::create_dir_all(&configured).expect("create configured prompts");
    std::fs::write(
        configured.join("review.md"),
        r#"---
description: Review a target
argument-hint: "<path>"
---
Review $1
"#,
    )
    .expect("write configured prompt template");
    // User prompt templates (auto-discovered from ~/.neo/prompts).
    let user_prompts = isolated_home().join("prompts");
    std::fs::write(user_prompts.join("explain.md"), "Explain the target\n")
        .expect("write user prompt template");

    let mut command = neo();
    command.current_dir(project.path()).arg("rpc");
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
    assert!(names.contains(&"/explain"));
    assert!(names.contains(&"/review"));

    let review = commands
        .iter()
        .find(|command| command["name"] == "/review")
        .expect("review command");
    assert_eq!(review["kind"], "prompt_template");
    assert_eq!(review["template"], "review");
    assert_eq!(review["description"], "Review a target");
    assert_eq!(review["argument_hint"], "<path>");
}

#[test]
fn rpc_get_commands_omits_excluded_auto_discovered_prompt_template() {
    let project = TempDir::new().expect("project tempdir");
    let prompts_dir = isolated_home().join("prompts");
    std::fs::create_dir_all(&prompts_dir).expect("create prompts");
    std::fs::write(prompts_dir.join("review.md"), "Review should be excluded\n")
        .expect("write excluded prompt template");
    std::fs::write(prompts_dir.join("fix.md"), "Fix remains\n")
        .expect("write kept prompt template");
    write_home_config(
        r#"
prompt_templates = ["-prompts/review.md"]
"#,
    );

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
    write_home_config(&mock_responses_config(&server.url));

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
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
    assert_eq!(user_input_contents(&requests[0]), vec!["hello rpc"]);
}

fn input_messages(request: &RecordedRequest) -> &[Value] {
    request.body["input"].as_array().expect("input messages")
}

fn user_input_contents(request: &RecordedRequest) -> Vec<&str> {
    input_messages(request)
        .iter()
        .filter(|message| {
            message["role"] == "user"
                && !message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("<available_skills>"))
        })
        .map(|message| message["content"].as_str().expect("user content"))
        .collect()
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

fn mock_responses_config(base_url: &str) -> String {
    format!(
        r#"
default_provider = "mock"
default_model = "gpt-4.1"

[providers.mock]
type = "openai_response"
base_url = "{base_url}"
api_key_env = "OPENAI_API_KEY"

[models."mock/gpt-4.1"]
provider = "mock"
model = "gpt-4.1"
capabilities = ["streaming", "tools"]
"#
    )
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
