use std::{fs, process::Command};

use tempfile::TempDir;

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

#[test]
fn root_command_enters_interactive_placeholder() {
    let command = neo();

    let stdout = run(command);

    assert!(stdout.contains("neo interactive"));
    assert!(stdout.contains("placeholder"));
}

#[test]
fn print_command_joins_prompt_arguments() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command
        .current_dir(temp.path())
        .args(["print", "hello", "neo"]);

    let stdout = run(command);

    assert_eq!(stdout, "fake response: hello neo\n");
}

#[test]
fn run_command_reports_placeholder_with_prompt() {
    let mut command = neo();
    let temp = TempDir::new().expect("tempdir");
    command
        .current_dir(temp.path())
        .args(["run", "build", "this"]);

    let stdout = run(command);

    assert!(stdout.contains("\"TurnStarted\""));
    assert!(stdout.contains("\"TextDelta\""));
    assert!(stdout.contains("fake response: build this"));
    assert!(!stdout.contains("placeholder"));

    let sessions = fs::read_dir(temp.path().join(".neo/sessions"))
        .expect("read sessions")
        .collect::<Result<Vec<_>, _>>()
        .expect("session entries");
    assert_eq!(sessions.len(), 1);
    let path = sessions[0].path();
    assert_eq!(path.extension().and_then(|ext| ext.to_str()), Some("jsonl"));
    let content = fs::read_to_string(path).expect("read jsonl session");
    assert!(content.contains("\"MessageAppended\""));
    assert!(content.contains("fake response: build this"));
}

#[test]
fn config_show_reads_project_config_and_env_overrides() {
    let temp = TempDir::new().expect("tempdir");
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        r#"
default_model = "config-model"
default_provider = "config-provider"
api_base = "https://config.example"
api_key_env = "CONFIG_API_KEY"

[permissions]
file_read = "Allow"
file_write = "Ask"
shell = "Deny"

[defaults]
mode = "print"
"#,
    )
    .expect("write config");

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("NEO_MODEL", "env-model")
        .env("NEO_PROVIDER", "env-provider")
        .arg("config")
        .arg("show");

    let stdout = run(command);

    assert!(stdout.contains("default_model = \"env-model\""));
    assert!(stdout.contains("default_provider = \"env-provider\""));
    assert!(stdout.contains("api_base = \"https://config.example\""));
    assert!(stdout.contains("api_key_env = \"CONFIG_API_KEY\""));
    assert!(stdout.contains("file_read = \"Allow\""));
    assert!(stdout.contains("file_write = \"Ask\""));
    assert!(stdout.contains("shell = \"Deny\""));
    assert!(stdout.contains("mode = \"print\""));
}

#[test]
fn config_set_writes_project_config_value() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command
        .current_dir(temp.path())
        .args(["config", "set", "default_model", "claude-test"]);

    let stdout = run(command);

    assert!(stdout.contains("set default_model"));
    let config = fs::read_to_string(temp.path().join(".neo/config.toml")).expect("read config");
    assert!(config.contains("default_model = \"claude-test\""));
}

#[test]
fn sessions_list_uses_project_session_directory() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions).expect("create sessions");
    fs::write(sessions.join("alpha.jsonl"), "{}\n").expect("write session");

    let mut command = neo();
    command.current_dir(temp.path()).args(["sessions", "list"]);

    let stdout = run(command);

    assert!(stdout.contains("alpha"));
}

#[test]
fn print_persists_jsonl_session_and_outputs_only_assistant_text() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command
        .current_dir(temp.path())
        .args(["print", "hello", "neo"]);

    let stdout = run(command);

    assert_eq!(stdout, "fake response: hello neo\n");
    let sessions = fs::read_dir(temp.path().join(".neo/sessions"))
        .expect("read sessions")
        .collect::<Result<Vec<_>, _>>()
        .expect("session entries");
    assert_eq!(sessions.len(), 1);
    let path = sessions[0].path();
    assert_eq!(path.extension().and_then(|ext| ext.to_str()), Some("jsonl"));
    let content = fs::read_to_string(path).expect("read jsonl session");
    assert!(content.contains("\"User\""));
    assert!(content.contains("\"Assistant\""));
    assert!(content.contains("fake response: hello neo"));
}

#[test]
fn sessions_show_and_resume_read_jsonl_transcripts() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions).expect("create sessions");
    fs::write(
        sessions.join("alpha.jsonl"),
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"hi back\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    )
    .expect("write session");

    let mut show = neo();
    show.current_dir(temp.path())
        .args(["sessions", "show", "alpha"]);
    let show_stdout = run(show);
    assert!(show_stdout.contains("\"User\""));
    assert!(show_stdout.contains("hi back"));

    let mut resume = neo();
    resume.current_dir(temp.path()).args(["resume", "alpha"]);
    let resume_stdout = run(resume);
    assert!(resume_stdout.contains("session alpha"));
    assert!(resume_stdout.contains("user: hello"));
    assert!(resume_stdout.contains("assistant: hi back"));
    assert!(!resume_stdout.contains("placeholder"));
}

#[test]
fn sessions_reject_path_traversal_ids() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions).expect("create sessions");
    fs::write(temp.path().join("escape.jsonl"), "{}\n").expect("write escape target");

    let output = neo()
        .current_dir(temp.path())
        .args(["sessions", "show", "../escape"])
        .output()
        .expect("neo command should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid session id"));
}

#[test]
fn models_and_mcp_list_are_wired_placeholders() {
    let mut models = neo();
    models.args(["models", "list"]);
    let models_stdout = run(models);
    assert!(models_stdout.contains("fake"));

    let mut mcp = neo();
    mcp.args(["mcp", "list"]);
    let mcp_stdout = run(mcp);
    assert!(mcp_stdout.contains("no MCP servers configured"));
}
