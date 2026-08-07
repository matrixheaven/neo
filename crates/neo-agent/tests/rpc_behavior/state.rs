use super::sessions::{
    SESSION_A, isolated_home, neo, parse_jsonl, run_with_stdin, session_bucket,
    write_session_transcript,
};

use tempfile::TempDir;

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
