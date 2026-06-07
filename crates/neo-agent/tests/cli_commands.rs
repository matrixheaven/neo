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
fn root_command_reports_interactive_entrypoint_without_placeholders() {
    let command = neo();

    let stdout = run(command);

    assert!(stdout.contains("neo interactive"));
    assert!(!stdout.contains("placeholder"));
    assert!(!stdout.contains("fake"));
}

#[test]
fn config_show_defaults_to_real_catalog_model() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command.current_dir(temp.path()).args(["config", "show"]);

    let stdout = run(command);

    assert!(stdout.contains("default_provider = \"openai\""));
    assert!(stdout.contains("default_model = \"gpt-4.1\""));
    assert!(!stdout.contains("\"fake\""));
}

#[test]
fn print_command_without_credentials_fails_without_local_response() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command
        .current_dir(temp.path())
        .env_remove("OPENAI_API_KEY")
        .env_remove("NEO_API_KEY_ENV")
        .args(["print", "hello", "neo"]);

    let output = command.output().expect("neo command should run");

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("fake response"));
    assert!(!stderr.contains("fake response"));
    assert!(stderr.contains("OPENAI_API_KEY"));
}

#[test]
fn run_command_without_credentials_fails_without_local_response() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command
        .current_dir(temp.path())
        .env_remove("OPENAI_API_KEY")
        .env_remove("NEO_API_KEY_ENV")
        .args(["run", "build", "this"]);

    let output = command.output().expect("neo command should run");

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("fake response"));
    assert!(!stderr.contains("fake response"));
    assert!(!stdout.contains("placeholder"));
    assert!(!stderr.contains("placeholder"));
    assert!(stderr.contains("OPENAI_API_KEY"));
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
tool = "Allow"

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
    assert!(stdout.contains("tool = \"Allow\""));
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
fn print_with_missing_credentials_does_not_persist_assistant_response() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command
        .current_dir(temp.path())
        .env_remove("OPENAI_API_KEY")
        .env_remove("NEO_API_KEY_ENV")
        .args(["print", "hello", "neo"]);

    let output = command.output().expect("neo command should run");

    assert!(!output.status.success());
    let sessions = fs::read_dir(temp.path().join(".neo/sessions"))
        .expect("read sessions")
        .collect::<Result<Vec<_>, _>>()
        .expect("session entries");
    assert_eq!(sessions.len(), 1);
    let path = sessions[0].path();
    assert_eq!(path.extension().and_then(|ext| ext.to_str()), Some("jsonl"));
    let content = fs::read_to_string(path).expect("read jsonl session");
    assert!(content.contains("\"User\""));
    assert!(!content.contains("\"Assistant\""));
    assert!(!content.contains("fake response"));
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
fn sessions_export_html_renders_replayed_messages() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions).expect("create sessions");
    fs::write(
        sessions.join("alpha.jsonl"),
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello <neo>\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"use **bold**\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    )
    .expect("write session");

    let mut export = neo();
    export
        .current_dir(temp.path())
        .args(["sessions", "export-html", "alpha"]);
    let html = run(export);

    assert!(html.contains("<!doctype html>"));
    assert!(html.contains("hello &lt;neo&gt;"));
    assert!(html.contains("<strong>bold</strong>"));
    assert!(!html.contains("fake"));
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
fn skills_show_loads_frontmatter_body_and_resources() {
    let temp = TempDir::new().expect("tempdir");
    let skill = temp.path().join("skills/reviewer");
    fs::create_dir_all(skill.join("references")).expect("create skill resources");
    fs::write(
        skill.join("SKILL.md"),
        r#"---
name = "reviewer"
description = "Review repository changes"
version = "1.0.0"
resources = [{ path = "references/policy.md", kind = "text" }]
---

# Reviewer

Use focused findings.
"#,
    )
    .expect("write skill");
    fs::write(skill.join("references/policy.md"), "Policy text\n").expect("write resource");

    let mut show = neo();
    show.arg("skills").arg("show").arg(&skill);
    let stdout = run(show);

    assert!(stdout.contains("name: reviewer"));
    assert!(stdout.contains("description: Review repository changes"));
    assert!(stdout.contains("resources: 1"));
    assert!(stdout.contains("Use focused findings."));
}

#[test]
fn extensions_list_discovers_manifests() {
    let temp = TempDir::new().expect("tempdir");
    let extension = temp.path().join("extensions/echo");
    fs::create_dir_all(&extension).expect("create extension");
    fs::write(
        extension.join("neo-extension.toml"),
        r#"
id = "echo"
name = "Echo"
version = "0.1.0"

[runner]
command = "python3"
"#,
    )
    .expect("write manifest");

    let mut list = neo();
    list.arg("extensions")
        .arg("list")
        .arg(temp.path().join("extensions"));
    let stdout = run(list);

    assert!(stdout.contains("echo"));
    assert!(stdout.contains("Echo"));
    assert!(stdout.contains("0.1.0"));
}

#[test]
fn extensions_call_round_trips_json_rpc() {
    let temp = TempDir::new().expect("tempdir");
    let extension = temp.path().join("extensions/echo");
    fs::create_dir_all(&extension).expect("create extension");
    let script = extension.join("echo.py");
    fs::write(
        &script,
        r#"
import json
import sys

message = json.loads(sys.stdin.readline())
print(json.dumps({
  "type": "response",
  "id": message["id"],
  "result": {
    "method": message["method"],
    "params": message["params"]
  }
}), flush=True)
"#,
    )
    .expect("write script");
    fs::write(
        extension.join("neo-extension.toml"),
        format!(
            r#"
id = "echo"
name = "Echo"
version = "0.1.0"

[runner]
command = "python3"
args = [{}]
"#,
            serde_json::to_string(&script).expect("script path should serialize")
        ),
    )
    .expect("write manifest");

    let mut call = neo();
    call.args(["extensions", "call", "echo", "tool.echo", r#"{"value":42}"#])
        .arg("--root")
        .arg(temp.path().join("extensions"));
    let stdout = run(call);

    assert!(stdout.contains("\"method\":\"tool.echo\""));
    assert!(stdout.contains("\"value\":42"));
}

#[test]
fn models_list_uses_seeded_catalog_without_local_fake() {
    let mut models = neo();
    models.args(["models", "list"]);
    let models_stdout = run(models);
    assert!(models_stdout.contains("openai/gpt-4.1"));
    assert!(models_stdout.contains("anthropic/claude-sonnet-4-5"));
    assert!(!models_stdout.contains("fake"));

    let mut mcp = neo();
    mcp.args(["mcp", "list"]);
    let mcp_stdout = run(mcp);
    assert!(mcp_stdout.contains("no MCP servers configured"));
}

#[test]
fn mcp_list_reports_empty_configuration_without_placeholder_language() {
    let mut mcp = neo();
    mcp.args(["mcp", "list"]);
    let mcp_stdout = run(mcp);
    assert!(mcp_stdout.contains("no MCP servers configured"));
    assert!(!mcp_stdout.contains("placeholder"));
    assert!(!mcp_stdout.contains("fake"));
}

#[test]
fn mcp_list_reads_project_config_servers() {
    let temp = TempDir::new().expect("tempdir");
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        r#"
[[mcp.servers]]
id = "filesystem"
enabled = true
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]

[mcp.servers.env]
RUST_LOG = "info"
"#,
    )
    .expect("write config");

    let mut mcp = neo();
    mcp.current_dir(temp.path()).args(["mcp", "list"]);
    let stdout = run(mcp);

    assert!(stdout.contains("filesystem"));
    assert!(stdout.contains("enabled"));
    assert!(stdout.contains("stdio"));
    assert!(stdout.contains("npx"));
}
