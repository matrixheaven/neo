use super::sessions::{
    SESSION_A, isolated_home, neo, run_interactive_requests, session_bucket,
    write_session_transcript,
};

use tempfile::TempDir;

#[test]
fn rpc_prompt_failure_is_correlated_and_server_continues() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = session_bucket(temp.path());
    std::fs::create_dir_all(&sessions).expect("create sessions");
    write_session_transcript(&sessions, SESSION_A, "{}\n");
    std::fs::write(
        isolated_home().join("config.toml"),
        r#"
default_provider = "missing"
default_model = "no-such-model"
"#,
    )
    .expect("write config");

    let mut command = neo();
    command.current_dir(temp.path()).arg("rpc");
    let responses = run_interactive_requests(
        command,
        &[
            r#"{"type":"request","id":"prompt-fail","method":"prompt","params":{"message":"hi"}}"#,
            r#"{"type":"request","id":"after-fail","method":"get_state","params":{}}"#,
        ],
    );

    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["type"], "response");
    assert_eq!(responses[0]["id"], "prompt-fail");
    assert_eq!(responses[0]["error"]["code"], "internal_error");
    assert_eq!(responses[1]["type"], "response");
    assert_eq!(responses[1]["id"], "after-fail");
    assert!(responses[1]["result"]["session_count"].as_u64().is_some());
}
