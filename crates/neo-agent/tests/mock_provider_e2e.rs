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
