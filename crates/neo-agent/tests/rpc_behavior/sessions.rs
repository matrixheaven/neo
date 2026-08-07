use super::http_server::RecordedRequest;
use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde_json::{Value, json};
use tempfile::TempDir;

pub(crate) const SESSION_A: &str = "session_00000000-0000-4000-8000-000000000301";

pub(crate) const SESSION_CHILD: &str = "session_00000000-0000-4000-8000-000000000303";

pub(crate) const SESSION_EMPTY: &str = "session_00000000-0000-4000-8000-000000000304";

pub(crate) fn neo() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_neo"));
    let home = isolated_home();
    command.env("NEO_HOME", &home);
    command.env("HOME", &home);
    command
}

pub(crate) fn isolated_home() -> std::path::PathBuf {
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

pub(crate) fn sessions_metadata_json(entries: &[(&str, Value)]) -> String {
    let mut sessions = serde_json::Map::new();
    for (id, value) in entries {
        sessions.insert((*id).to_owned(), value.clone());
    }
    json!({ "sessions": sessions }).to_string()
}

pub(crate) fn write_home_config(content: &str) {
    let config_dir = isolated_home();
    std::fs::create_dir_all(&config_dir).expect("create .neo");
    std::fs::write(config_dir.join("config.toml"), content).expect("write config");
}

pub(crate) fn session_bucket(project_dir: &Path) -> PathBuf {
    let sessions_root = isolated_home().join("sessions");
    neo_agent_core::session::workspace_sessions_dir(&sessions_root, project_dir)
}

pub(crate) fn write_session_transcript(
    sessions: &Path,
    session_id: &str,
    content: &str,
) -> PathBuf {
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

pub(crate) fn run_with_stdin(mut command: Command, stdin: &str) -> String {
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

pub(crate) fn input_messages(request: &RecordedRequest) -> &[Value] {
    request.body["input"].as_array().expect("input messages")
}

pub(crate) fn user_input_contents(request: &RecordedRequest) -> Vec<&str> {
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

pub(crate) fn parse_jsonl(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("valid JSONL response"))
        .collect()
}

pub(crate) fn run_interactive_requests(mut command: Command, requests: &[&str]) -> Vec<Value> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("neo command should spawn");
    let mut stdin = child.stdin.take().expect("stdin pipe");
    let stdout = child.stdout.take().expect("stdout pipe");
    let mut reader = BufReader::new(stdout);
    let mut responses = Vec::new();
    for request in requests {
        writeln!(stdin, "{request}").expect("write request");
        stdin.flush().expect("flush request");
        let mut line = String::new();
        reader.read_line(&mut line).expect("read response line");
        responses.push(serde_json::from_str::<Value>(&line).expect("valid JSONL response"));
    }
    drop(stdin);
    let output = child.wait_with_output().expect("neo command should run");
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    responses
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
