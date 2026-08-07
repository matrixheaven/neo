use super::http_server::{MockSseServer, mock_responses_config, openai_response_sse};
use super::sessions::{
    SESSION_A, isolated_home, neo, parse_jsonl, run_interactive_requests, run_with_stdin,
    session_bucket, user_input_contents, write_home_config, write_session_transcript,
};

use tempfile::TempDir;

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

#[test]
fn rpc_responds_before_stdin_eof_and_accepts_next_request() {
    let temp = TempDir::new().expect("tempdir");
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
    let responses = run_interactive_requests(
        command,
        &[
            r#"{"type":"request","id":"req-1","method":"get_state","params":{}}"#,
            r#"{"type":"request","id":"req-2","method":"get_state","params":{}}"#,
        ],
    );

    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["type"], "response");
    assert_eq!(responses[0]["id"], "req-1");
    assert_eq!(responses[0]["result"]["session_count"], 1);
    assert_eq!(responses[1]["type"], "response");
    assert_eq!(responses[1]["id"], "req-2");
    assert!(responses[1]["result"]["session_count"].is_number());
}
