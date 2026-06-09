use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
};

use serde_json::{Value, json};
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

    assert!(stdout.contains("neo | session:"));
    assert!(stdout.contains("model: openai/gpt-4.1"));
    assert!(stdout.contains("Editing"));
    assert!(!stdout.contains("placeholder"));
    assert!(!stdout.contains("fake"));
    assert!(!stdout.contains("commands: print, run"));
}

#[test]
fn root_command_fallback_renders_configured_tui_session_state() {
    let temp = TempDir::new().expect("tempdir");
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        r#"
default_provider = "anthropic"
default_model = "claude-sonnet"
"#,
    )
    .expect("write config");

    let mut command = neo();
    command.current_dir(temp.path());

    let stdout = run(command);

    assert!(stdout.contains("neo | session:"));
    assert!(stdout.contains("model: anthropic/claude-sonnet"));
    assert!(stdout.contains('>'));
    assert!(!stdout.contains("commands:"));
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
fn config_show_layers_user_global_config_below_project_config_and_expands_home_paths() {
    let home = TempDir::new().expect("home tempdir");
    let project = TempDir::new().expect("project tempdir");
    fs::create_dir_all(home.path().join(".neo")).expect("create global .neo");
    fs::write(
        home.path().join(".neo/config.toml"),
        r#"
default_provider = "anthropic"
default_model = "claude-sonnet-4-5"
sessions_dir = "~/.neo/sessions"
api_key_env = "GLOBAL_KEY"

[runtime]
max_tokens = 1024
reasoning_effort = "low"
"#,
    )
    .expect("write global config");
    fs::create_dir_all(project.path().join(".neo")).expect("create project .neo");
    fs::write(
        project.path().join(".neo/config.toml"),
        r#"
default_model = "project-model"

[runtime]
temperature = 0.3
reasoning_effort = "high"
"#,
    )
    .expect("write project config");

    let mut command = neo();
    command
        .current_dir(project.path())
        .env("HOME", home.path())
        .args(["config", "show"]);

    let stdout = run(command);

    assert!(stdout.contains("default_provider = \"anthropic\""));
    assert!(stdout.contains("default_model = \"project-model\""));
    assert!(stdout.contains("api_key_env = \"GLOBAL_KEY\""));
    assert!(stdout.contains("max_tokens = 1024"));
    assert!(stdout.contains("temperature = 0.3"));
    assert!(stdout.contains("reasoning_effort = \"high\""));
    assert!(stdout.contains(&format!(
        "sessions_dir = \"{}\"",
        home.path().join(".neo/sessions").display()
    )));
}

#[test]
fn config_show_merges_provider_config_fields_across_global_and_project_layers() {
    let home = TempDir::new().expect("home tempdir");
    let project = TempDir::new().expect("project tempdir");
    fs::create_dir_all(home.path().join(".neo")).expect("create global .neo");
    fs::write(
        home.path().join(".neo/config.toml"),
        r#"
[providers.openai]
api_base = "https://global-openai.example/v1"
"#,
    )
    .expect("write global config");
    fs::create_dir_all(project.path().join(".neo")).expect("create project .neo");
    fs::write(
        project.path().join(".neo/config.toml"),
        r#"
[providers.openai]
api_key_env = "PROJECT_OPENAI_KEY"
"#,
    )
    .expect("write project config");

    let mut command = neo();
    command
        .current_dir(project.path())
        .env("HOME", home.path())
        .args(["config", "show"]);
    let stdout = run(command);

    assert!(stdout.contains("[providers.openai]"));
    assert!(stdout.contains("api_base = \"https://global-openai.example/v1\""));
    assert!(stdout.contains("api_key_env = \"PROJECT_OPENAI_KEY\""));
}

#[test]
fn config_show_merges_prompt_template_selectors_across_global_and_project_layers() {
    let home = TempDir::new().expect("home tempdir");
    let project = TempDir::new().expect("project tempdir");
    fs::create_dir_all(home.path().join(".neo")).expect("create global .neo");
    fs::write(
        home.path().join(".neo/config.toml"),
        r#"
prompt_templates = ["global-prompts", "shared-prompts"]
"#,
    )
    .expect("write global config");
    fs::create_dir_all(project.path().join(".neo")).expect("create project .neo");
    fs::write(
        project.path().join(".neo/config.toml"),
        r#"
prompt_templates = ["project-prompts", "shared-prompts"]
"#,
    )
    .expect("write project config");

    let mut command = neo();
    command
        .current_dir(project.path())
        .env("HOME", home.path())
        .args(["config", "show"]);
    let stdout = run(command);

    assert!(stdout.contains("prompt_templates = ["));
    let global_index = stdout
        .find("\"global-prompts\"")
        .expect("global prompt selector");
    let shared_index = stdout
        .find("\"shared-prompts\"")
        .expect("shared prompt selector");
    let project_index = stdout
        .find("\"project-prompts\"")
        .expect("project prompt selector");
    assert!(global_index < shared_index);
    assert!(shared_index < project_index);
}

#[test]
fn config_show_reads_provider_specific_api_key_env_without_secret_values() {
    let temp = TempDir::new().expect("tempdir");
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        r#"
[providers.openai]
api_key_env = "PROJECT_OPENAI_KEY"
"#,
    )
    .expect("write config");

    let mut command = neo();
    command.current_dir(temp.path()).args(["config", "show"]);

    let stdout = run(command);

    assert!(stdout.contains("[providers.openai]"));
    assert!(stdout.contains("api_key_env = \"PROJECT_OPENAI_KEY\""));
    assert!(!stdout.contains("secret"));
}

#[test]
fn config_show_reads_provider_specific_api_base_without_secret_values() {
    let temp = TempDir::new().expect("tempdir");
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        r#"
[providers.openai]
api_base = "https://project-openai.example/v1"
api_key_env = "PROJECT_OPENAI_KEY"
"#,
    )
    .expect("write config");

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("PROJECT_OPENAI_KEY", "secret-value")
        .args(["config", "show"]);

    let stdout = run(command);

    assert!(stdout.contains("[providers.openai]"));
    assert!(stdout.contains("api_base = \"https://project-openai.example/v1\""));
    assert!(stdout.contains("api_key_env = \"PROJECT_OPENAI_KEY\""));
    assert!(!stdout.contains("secret-value"));
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
fn config_set_writes_runtime_agent_options() {
    let temp = TempDir::new().expect("tempdir");
    for (key, value) in [
        ("runtime.temperature", "0.4"),
        ("runtime.max_tokens", "2048"),
        ("runtime.reasoning_effort", "xhigh"),
        ("runtime.steering_queue_mode", "OneAtATime"),
        ("runtime.follow_up_queue_mode", "OneAtATime"),
        ("runtime.tool_execution_mode", "Sequential"),
        ("runtime.compaction.max_estimated_tokens", "12000"),
        ("runtime.compaction.keep_recent_messages", "16"),
    ] {
        let mut command = neo();
        command
            .current_dir(temp.path())
            .args(["config", "set", key, value]);
        let stdout = run(command);
        assert!(stdout.contains(&format!("set {key}")));
    }

    let config = fs::read_to_string(temp.path().join(".neo/config.toml")).expect("read config");
    assert!(config.contains("temperature = 0.4"));
    assert!(config.contains("max_tokens = 2048"));
    assert!(config.contains("reasoning_effort = \"xhigh\""));
    assert!(config.contains("steering_queue_mode = \"OneAtATime\""));
    assert!(config.contains("follow_up_queue_mode = \"OneAtATime\""));
    assert!(config.contains("tool_execution_mode = \"Sequential\""));
    assert!(config.contains("max_estimated_tokens = 12000"));
    assert!(config.contains("keep_recent_messages = 16"));
}

#[test]
fn config_show_reads_tui_keybinding_overrides() {
    let temp = TempDir::new().expect("tempdir");
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        r#"
[tui.keybindings]
"tui.command.open" = ["ctrl+g"]
"tui.session.open" = ["ctrl+s"]
"#,
    )
    .expect("write config");

    let mut command = neo();
    command.current_dir(temp.path()).args(["config", "show"]);
    let stdout = run(command);

    assert!(stdout.contains("[tui.keybindings]"));
    assert!(stdout.contains("\"tui.command.open\" = [\"ctrl+g\"]"));
    assert!(stdout.contains("\"tui.session.open\" = [\"ctrl+s\"]"));
}

#[test]
fn config_set_writes_tui_keybinding_override() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command.current_dir(temp.path()).args([
        "config",
        "set",
        "tui.keybindings.tui.command.open",
        "[\"ctrl+g\", \"ctrl+p\"]",
    ]);
    let stdout = run(command);

    assert!(stdout.contains("set tui.keybindings.tui.command.open"));
    let config = fs::read_to_string(temp.path().join(".neo/config.toml")).expect("read config");
    let value: toml::Value = toml::from_str(&config).expect("config should be valid toml");
    let keys = value["tui"]["keybindings"]["tui.command.open"]
        .as_array()
        .expect("keybinding override should be an array")
        .iter()
        .map(|value| value.as_str().expect("key should be a string").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(keys, vec!["ctrl+g", "ctrl+p"]);
}

#[test]
fn config_set_rejects_tui_keybinding_default_conflicts() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command.current_dir(temp.path()).args([
        "config",
        "set",
        "tui.keybindings.tui.command.open",
        "[\"enter\"]",
    ]);

    let output = command.output().expect("neo command should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("tui.keybindings"));
    assert!(stderr.contains("enter"));
}

#[test]
fn config_set_rejects_tui_keybinding_bare_printable_chars() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command.current_dir(temp.path()).args([
        "config",
        "set",
        "tui.keybindings.tui.command.open",
        "[\"g\"]",
    ]);

    let output = command.output().expect("neo command should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("tui.keybindings"));
    assert!(stderr.contains('g'));
}

#[test]
fn config_set_writes_provider_specific_api_key_env_name() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command.current_dir(temp.path()).args([
        "config",
        "set",
        "providers.openai.api_key_env",
        "PROJECT_OPENAI_KEY",
    ]);

    let stdout = run(command);

    assert!(stdout.contains("set providers.openai.api_key_env"));
    let config = fs::read_to_string(temp.path().join(".neo/config.toml")).expect("read config");
    assert!(config.contains("[providers.openai]"));
    assert!(config.contains("api_key_env = \"PROJECT_OPENAI_KEY\""));
}

#[test]
fn config_set_writes_provider_specific_api_base() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command.current_dir(temp.path()).args([
        "config",
        "set",
        "providers.openai.api_base",
        "https://project-openai.example/v1",
    ]);

    let stdout = run(command);

    assert!(stdout.contains("set providers.openai.api_base"));
    let config = fs::read_to_string(temp.path().join(".neo/config.toml")).expect("read config");
    assert!(config.contains("[providers.openai]"));
    assert!(config.contains("api_base = \"https://project-openai.example/v1\""));
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
fn sessions_rename_and_fork_surface_tree_metadata() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions).expect("create sessions");
    fs::write(sessions.join("alpha.jsonl"), "{}\n").expect("write session");

    let mut rename = neo();
    rename
        .current_dir(temp.path())
        .args(["sessions", "rename", "alpha", "Main thread"]);
    let rename_stdout = run(rename);
    assert!(rename_stdout.contains("renamed alpha"));
    assert!(rename_stdout.contains("Main thread"));

    let mut fork = neo();
    fork.current_dir(temp.path())
        .args(["sessions", "fork", "alpha", "--name", "Parser branch"]);
    let fork_stdout = run(fork);
    assert!(fork_stdout.contains("forked alpha -> "));
    assert!(fork_stdout.contains("Parser branch"));

    let child_id = fork_stdout
        .lines()
        .find_map(|line| line.strip_prefix("forked alpha -> "))
        .and_then(|line| line.split_whitespace().next())
        .expect("fork output includes child id")
        .to_owned();

    let mut list = neo();
    list.current_dir(temp.path()).args(["sessions", "list"]);
    let list_stdout = run(list);

    assert!(list_stdout.contains("alpha"));
    assert!(list_stdout.contains("Main thread"));
    assert!(list_stdout.contains(&child_id));
    assert!(list_stdout.contains("Parser branch"));
    assert!(list_stdout.contains("parent=alpha"));

    let mut tree = neo();
    tree.current_dir(temp.path()).args(["sessions", "tree"]);
    let tree_stdout = run(tree);
    let parent_position = tree_stdout.find("alpha").expect("tree includes parent");
    let child_position = tree_stdout
        .find(&format!("  {child_id}"))
        .expect("tree indents child");
    assert!(parent_position < child_position);
    assert!(tree_stdout.contains("Main thread"));
    assert!(tree_stdout.contains("Parser branch"));
}

#[test]
fn sessions_summarize_stores_local_branch_summary() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions).expect("create sessions");
    fs::write(
        sessions.join("alpha.jsonl"),
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"investigate parser panic\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"panic comes from token split\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    )
    .expect("write session");

    let mut summarize = neo();
    summarize
        .current_dir(temp.path())
        .args(["sessions", "summarize", "alpha"]);
    let summarize_stdout = run(summarize);

    assert!(summarize_stdout.contains("summarized alpha"));
    assert!(summarize_stdout.contains("user: investigate parser panic"));
    assert!(summarize_stdout.contains("assistant: panic comes from token split"));

    let mut list = neo();
    list.current_dir(temp.path()).args(["sessions", "list"]);
    let list_stdout = run(list);
    assert!(list_stdout.contains("summary=Local branch summary"));

    let mut resume = neo();
    resume.current_dir(temp.path()).args(["resume", "alpha"]);
    let resume_stdout = run(resume);
    assert!(resume_stdout.contains("branch summary: Local branch summary"));
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
fn sessions_accept_unique_prefixes_and_local_jsonl_paths() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions).expect("create sessions");
    fs::write(
        sessions.join("alpha-main.jsonl"),
        "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"alpha prompt\"}}]}}}}\n",
    )
    .expect("write alpha session");
    fs::write(
        sessions.join("beta-main.jsonl"),
        "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"beta prompt\"}}]}}}}\n",
    )
    .expect("write beta session");

    let mut show_prefix = neo();
    show_prefix
        .current_dir(temp.path())
        .args(["sessions", "show", "alp"]);
    let prefix_stdout = run(show_prefix);
    assert!(prefix_stdout.contains("alpha prompt"));

    let mut resume_path = neo();
    resume_path
        .current_dir(temp.path())
        .arg("resume")
        .arg(sessions.join("alpha-main.jsonl"));
    let path_stdout = run(resume_path);
    assert!(path_stdout.contains("session alpha-main"));
    assert!(path_stdout.contains("user: alpha prompt"));
}

#[test]
fn sessions_reject_ambiguous_prefixes_without_guessing() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions).expect("create sessions");
    fs::write(sessions.join("alpha-main.jsonl"), "{}\n").expect("write alpha");
    fs::write(sessions.join("alpha-side.jsonl"), "{}\n").expect("write alpha side");

    let output = neo()
        .current_dir(temp.path())
        .args(["sessions", "show", "alp"])
        .output()
        .expect("neo command should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ambiguous session id"));
    assert!(stderr.contains("alpha-main"));
    assert!(stderr.contains("alpha-side"));
}

#[test]
fn sessions_compact_stores_algorithmic_summary_and_resume_replays_kept_context() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions).expect("create sessions");
    fs::write(
        sessions.join("alpha.jsonl"),
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"first task\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"first answer\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"latest task\"}}]}}}}\n"
        ),
    )
    .expect("write session");

    let mut compact = neo();
    compact
        .current_dir(temp.path())
        .args(["sessions", "compact", "alpha", "--keep-recent", "1"]);
    let compact_stdout = run(compact);

    assert!(compact_stdout.contains("compacted alpha"));
    assert!(compact_stdout.contains("kept 1"));
    assert!(compact_stdout.contains("Algorithmic transcript summary"));
    assert!(!compact_stdout.contains("fake"));

    let jsonl = fs::read_to_string(sessions.join("alpha.jsonl")).expect("read compacted session");
    assert!(jsonl.contains("\"CompactionApplied\""));
    assert!(jsonl.contains("Algorithmic transcript summary"));
    assert!(jsonl.contains("first task"));

    let mut resume = neo();
    resume.current_dir(temp.path()).args(["resume", "alpha"]);
    let resume_stdout = run(resume);
    assert!(resume_stdout.contains("session alpha"));
    assert!(resume_stdout.contains("compaction: Algorithmic transcript summary"));
    assert!(resume_stdout.contains("user: latest task"));
    assert!(
        !resume_stdout
            .lines()
            .any(|line| line == "user: first task" || line == "assistant: first answer")
    );
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
fn extensions_install_update_and_list_sources_from_local_directory() {
    let temp = TempDir::new().expect("tempdir");
    let source = temp.path().join("source");
    write_extension_manifest(&source, "echo", "Echo", "0.1.0");

    let mut install = neo();
    install
        .current_dir(temp.path())
        .args(["extensions", "install"])
        .arg(&source);
    let installed = run(install);
    assert!(installed.contains("echo installed"));
    assert!(installed.contains("0.1.0"));

    let mut disable = neo();
    disable
        .current_dir(temp.path())
        .args(["extensions", "disable", "echo"]);
    run(disable);

    write_extension_manifest(&source, "echo", "Echo", "0.2.0");

    let mut update = neo();
    update
        .current_dir(temp.path())
        .args(["extensions", "update", "echo"]);
    let updated = run(update);
    assert!(updated.contains("echo updated"));
    assert!(updated.contains("0.2.0"));

    let mut list = neo();
    list.current_dir(temp.path()).args(["extensions", "list"]);
    let listed = run(list);
    assert!(listed.contains("echo"));
    assert!(listed.contains("0.2.0"));
    assert!(listed.contains("disabled"));
    assert!(listed.contains(source.to_string_lossy().as_ref()));

    let state = fs::read_to_string(temp.path().join(".neo/extensions-state.toml"))
        .expect("read lifecycle state");
    assert!(state.contains("[extensions.echo]"));
    assert!(state.contains("enabled = false"));
}

#[test]
fn extensions_defaults_use_project_config_directory_when_invoked_from_another_cwd() {
    let project = TempDir::new().expect("project tempdir");
    let caller = TempDir::new().expect("caller tempdir");
    fs::create_dir_all(project.path().join(".neo")).expect("create project .neo");
    fs::write(project.path().join(".neo/config.toml"), "").expect("write project config");
    let source = project.path().join("source");
    write_extension_manifest(&source, "echo", "Echo", "0.1.0");

    let config_path = project.path().join(".neo/config.toml");
    let mut install = neo();
    install
        .current_dir(caller.path())
        .arg("--config")
        .arg(&config_path)
        .args(["extensions", "install"])
        .arg(&source);
    let installed = run(install);
    assert!(installed.contains("echo installed"));

    let mut disable = neo();
    disable
        .current_dir(caller.path())
        .arg("--config")
        .arg(&config_path)
        .args(["extensions", "disable", "echo"]);
    let disabled = run(disable);
    assert!(disabled.contains("echo disabled"));

    write_extension_manifest(&source, "echo", "Echo", "0.2.0");

    let mut update = neo();
    update
        .current_dir(caller.path())
        .arg("--config")
        .arg(&config_path)
        .args(["extensions", "update", "echo"]);
    let updated = run(update);
    assert!(updated.contains("echo updated"));
    assert!(updated.contains("0.2.0"));

    let mut list = neo();
    list.current_dir(caller.path())
        .arg("--config")
        .arg(&config_path)
        .args(["extensions", "list"]);
    let listed = run(list);
    assert!(listed.contains("echo"));
    assert!(listed.contains("0.2.0"));
    assert!(listed.contains("disabled"));
    assert!(listed.contains(source.to_string_lossy().as_ref()));

    let project_state = fs::read_to_string(project.path().join(".neo/extensions-state.toml"))
        .expect("read project lifecycle state");
    assert!(project_state.contains("[extensions.echo]"));
    assert!(project_state.contains("enabled = false"));
    let project_registry = fs::read_to_string(project.path().join(".neo/extensions-sources.toml"))
        .expect("read project source registry");
    assert!(project_registry.contains("[extensions.echo"));
    assert!(project_registry.contains(source.to_string_lossy().as_ref()));
    assert!(
        project
            .path()
            .join(".neo/extensions/echo/neo-extension.toml")
            .exists()
    );

    assert!(!caller.path().join(".neo/extensions-state.toml").exists());
    assert!(!caller.path().join(".neo/extensions-sources.toml").exists());
    assert!(!caller.path().join(".neo/extensions").exists());
}

#[test]
fn extensions_uninstall_removes_install_dir_and_source_entry() {
    let temp = TempDir::new().expect("tempdir");
    let source = temp.path().join("source");
    write_extension_manifest(&source, "echo", "Echo", "0.1.0");
    let root = temp.path().join("extensions");

    let mut install = neo();
    install
        .current_dir(temp.path())
        .args(["extensions", "install"])
        .arg(&source)
        .arg("--root")
        .arg(&root);
    run(install);
    assert!(root.join("echo/neo-extension.toml").exists());

    let mut uninstall = neo();
    uninstall
        .current_dir(temp.path())
        .args(["extensions", "uninstall", "echo", "--root"])
        .arg(&root);
    let uninstalled = run(uninstall);

    assert!(uninstalled.contains("echo uninstalled"));
    assert!(!root.join("echo").exists());

    let registry = fs::read_to_string(temp.path().join(".neo/extensions-sources.toml"))
        .expect("read extension source registry");
    assert!(!registry.contains("[extensions.echo"));
    assert!(!registry.contains(source.to_string_lossy().as_ref()));
}

#[test]
fn extensions_install_and_update_from_local_git_repo_without_marketplace_catalog() {
    let temp = TempDir::new().expect("tempdir");
    let repo = temp.path().join("repo");
    write_extension_manifest(&repo, "git_echo", "Git Echo", "0.1.0");
    init_git_repo(&repo);

    let source_url = format!("file://{}", repo.display());
    let mut install = neo();
    install
        .current_dir(temp.path())
        .args(["extensions", "install"])
        .arg(&source_url);
    let installed = run(install);
    assert!(installed.contains("git_echo installed"));
    assert!(installed.contains("0.1.0"));

    write_extension_manifest(&repo, "git_echo", "Git Echo", "0.2.0");
    commit_git_repo(&repo, "update extension");

    let mut update = neo();
    update
        .current_dir(temp.path())
        .args(["extensions", "update", "git_echo"]);
    let updated = run(update);
    assert!(updated.contains("git_echo updated"));
    assert!(updated.contains("0.2.0"));

    let mut list = neo();
    list.current_dir(temp.path()).args(["extensions", "list"]);
    let listed = run(list);
    assert!(listed.contains("git_echo"));
    assert!(listed.contains("0.2.0"));
    assert!(listed.contains(&source_url));
    assert!(!listed.contains("marketplace"));
    assert!(!listed.contains("fake"));
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
fn extensions_lifecycle_commands_persist_status_and_gate_call() {
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
  "result": {"ok": True}
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

    let root = temp.path().join("extensions");
    let mut disable = neo();
    disable
        .current_dir(temp.path())
        .args(["extensions", "disable", "echo", "--root"])
        .arg(&root);
    let disabled = run(disable);
    assert!(disabled.contains("echo disabled"));

    let state = fs::read_to_string(temp.path().join(".neo/extensions-state.toml"))
        .expect("read lifecycle state");
    assert!(state.contains("[extensions.echo]"));
    assert!(state.contains("enabled = false"));

    let mut status = neo();
    status
        .current_dir(temp.path())
        .args(["extensions", "status", "echo", "--root"])
        .arg(&root);
    let status_stdout = run(status);
    assert!(status_stdout.contains("echo"));
    assert!(status_stdout.contains("disabled"));
    assert!(status_stdout.contains("state_file"));

    let call = neo()
        .current_dir(temp.path())
        .args(["extensions", "call", "echo", "tool.echo", "{}", "--root"])
        .arg(&root)
        .output()
        .expect("neo command should run");
    assert!(!call.status.success());
    assert!(String::from_utf8_lossy(&call.stderr).contains("extension \"echo\" is disabled"));

    let mut enable = neo();
    enable
        .current_dir(temp.path())
        .args(["extensions", "enable", "echo", "--root"])
        .arg(&root);
    let enabled = run(enable);
    assert!(enabled.contains("echo enabled"));
}

fn write_extension_manifest(root: &std::path::Path, id: &str, name: &str, version: &str) {
    fs::create_dir_all(root).expect("create extension source");
    fs::write(
        root.join("neo-extension.toml"),
        format!(
            r#"
id = "{id}"
name = "{name}"
version = "{version}"

[runner]
command = "python3"
"#
        ),
    )
    .expect("write extension manifest");
}

fn init_git_repo(repo: &std::path::Path) {
    git(repo, ["init"]);
    git(repo, ["config", "user.email", "neo@example.invalid"]);
    git(repo, ["config", "user.name", "Neo Test"]);
    git(repo, ["add", "neo-extension.toml"]);
    git(repo, ["commit", "-m", "initial extension"]);
}

fn commit_git_repo(repo: &std::path::Path, message: &str) {
    git(repo, ["add", "neo-extension.toml"]);
    git(repo, ["commit", "-m", message]);
}

fn git<const N: usize>(repo: &std::path::Path, args: [&str; N]) {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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
fn models_list_loads_project_model_catalogs() {
    let temp = TempDir::new().expect("tempdir");
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        r#"
default_provider = "openrouter"
default_model = "anthropic/claude-sonnet-4.5"
model_catalogs = [".neo/models.json"]
"#,
    )
    .expect("write config");
    fs::write(
        temp.path().join(".neo/models.json"),
        r#"
{
  "models": [
    {
      "provider": "openrouter",
      "model": "anthropic/claude-sonnet-4.5",
      "api": "OpenAiCompatible",
      "capabilities": {
        "streaming": true,
        "tools": true,
        "images": false,
        "reasoning": true,
        "embeddings": false,
        "max_context_tokens": 200000
      }
    }
  ]
}
"#,
    )
    .expect("write model catalog");

    let mut models = neo();
    models.current_dir(temp.path()).args(["models", "list"]);
    let stdout = run(models);

    assert!(stdout.contains("openrouter/anthropic/claude-sonnet-4.5"));
    assert!(stdout.contains("OpenAiCompatible default"));
}

#[test]
fn models_list_applies_provider_specific_api_key_env_status() {
    let temp = TempDir::new().expect("tempdir");
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        r#"
[providers.openai]
api_key_env = "PROJECT_OPENAI_KEY"
"#,
    )
    .expect("write config");

    let mut models = neo();
    models
        .current_dir(temp.path())
        .env("PROJECT_OPENAI_KEY", "secret-value")
        .args(["models", "list"]);
    let stdout = run(models);

    assert!(stdout.contains("- openai (OpenAiResponses, configured)"));
    assert!(!stdout.contains("secret-value"));
}

#[test]
fn config_show_applies_selected_provider_api_key_env_without_secret_values() {
    let temp = TempDir::new().expect("tempdir");
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        r#"
[providers.openai]
api_key_env = "PROJECT_OPENAI_KEY"
"#,
    )
    .expect("write config");

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("PROJECT_OPENAI_KEY", "secret-value")
        .args(["config", "show"]);
    let stdout = run(command);
    assert!(stdout.contains("api_key_env = \"PROJECT_OPENAI_KEY\""));
    assert!(stdout.contains("[providers.openai]"));
    assert!(!stdout.contains("secret-value"));
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

#[test]
fn mcp_list_displays_remote_server_urls() {
    let temp = TempDir::new().expect("tempdir");
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        r#"
[[mcp.servers]]
id = "remote-docs"
enabled = true
transport = "http"
url = "https://mcp.example.test/rpc"

[[mcp.servers]]
id = "stream-docs"
enabled = false
transport = "sse"
url = "https://mcp.example.test/sse"
"#,
    )
    .expect("write config");

    let mut mcp = neo();
    mcp.current_dir(temp.path()).args(["mcp", "list"]);
    let stdout = run(mcp);

    assert!(stdout.contains("remote-docs"));
    assert!(stdout.contains("enabled"));
    assert!(stdout.contains("http"));
    assert!(stdout.contains("https://mcp.example.test/rpc"));
    assert!(stdout.contains("stream-docs"));
    assert!(stdout.contains("disabled"));
    assert!(stdout.contains("sse"));
    assert!(stdout.contains("https://mcp.example.test/sse"));
}

#[test]
fn print_registers_enabled_stdio_mcp_tools_from_project_config() {
    let temp = TempDir::new().expect("tempdir");
    let provider = MockSseServer::start(vec![openai_response_sse("resp-mcp", "mcp tools listed")]);
    let mcp_fixture = temp.path().join("mcp-fixture.py");
    fs::write(&mcp_fixture, MCP_STDIO_FIXTURE).expect("write MCP fixture");
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        format!(
            r#"
api_base = "{}"

[[mcp.servers]]
id = "docs-server"
enabled = true
transport = "stdio"
command = "python3"
args = ["-u", "{}"]
"#,
            provider.url,
            mcp_fixture.display()
        ),
    )
    .expect("write config");

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["print", "show", "tools"]);
    let stdout = run(command);

    assert_eq!(stdout, "mcp tools listed\n");
    let requests = provider.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/responses");
    assert_eq!(
        requests[0].headers.get("authorization").map(String::as_str),
        Some("Bearer test-key")
    );
    let tool_names = requests[0].body["tools"]
        .as_array()
        .expect("model request tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect::<Vec<_>>();
    assert!(
        tool_names.contains(&"mcp__docs_server__docs_search"),
        "model tools should include configured MCP stdio tools: {tool_names:?}"
    );
}

#[test]
fn print_registers_enabled_http_mcp_tools_from_project_config() {
    let temp = TempDir::new().expect("tempdir");
    let provider = MockSseServer::start(vec![openai_response_sse(
        "resp-mcp-http",
        "remote mcp tools listed",
    )]);
    let mcp_server = MockSseServer::start(vec![
        mcp_json_response(
            1,
            &json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "remote-docs", "version": "0.1.0"},
                "capabilities": {"tools": {}}
            }),
        ),
        mcp_json_response(
            2,
            &json!({
                "tools": [
                    {
                        "name": "docs-search",
                        "description": "Search remote docs",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"query": {"type": "string"}},
                            "required": ["query"]
                        }
                    }
                ]
            }),
        ),
    ]);
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        format!(
            r#"
api_base = "{}"

[[mcp.servers]]
id = "remote-docs"
enabled = true
transport = "http"
url = "{}"

[mcp.servers.headers]
"x-neo-test" = "remote-mcp"
"#,
            provider.url, mcp_server.url
        ),
    )
    .expect("write config");

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["print", "show", "remote", "tools"]);
    let stdout = run(command);

    assert_eq!(stdout, "remote mcp tools listed\n");
    let requests = provider.requests();
    let tool_names = requests[0].body["tools"]
        .as_array()
        .expect("model request tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect::<Vec<_>>();
    assert!(
        tool_names.contains(&"mcp__remote_docs__docs_search"),
        "model tools should include configured MCP HTTP tools: {tool_names:?}"
    );
    let mcp_requests = mcp_server.requests();
    assert_eq!(
        mcp_requests
            .iter()
            .map(|request| request.body["method"].as_str().expect("method"))
            .collect::<Vec<_>>(),
        vec!["initialize", "tools/list"]
    );
    assert!(mcp_requests.iter().all(
        |request| request.headers.get("x-neo-test").map(String::as_str) == Some("remote-mcp")
    ));
}

#[test]
fn mcp_tools_lists_remote_tool_catalog_with_schema() {
    let temp = TempDir::new().expect("tempdir");
    let mcp_server = MockSseServer::start(vec![
        mcp_json_response(
            1,
            &json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "remote-docs", "version": "0.1.0"},
                "capabilities": {"tools": {}}
            }),
        ),
        mcp_json_response(
            2,
            &json!({
                "tools": [
                    {
                        "name": "docs-search",
                        "description": "Search remote docs",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"query": {"type": "string"}},
                            "required": ["query"]
                        }
                    }
                ]
            }),
        ),
    ]);
    write_remote_mcp_config(temp.path(), &mcp_server.url);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .args(["mcp", "tools", "remote-docs"]);
    let stdout = run(command);

    assert!(stdout.contains("mcp__remote_docs__docs_search"));
    assert!(stdout.contains("Search remote docs"));
    assert!(stdout.contains("\"required\":[\"query\"]"));
    let mcp_requests = mcp_server.requests();
    assert_eq!(
        mcp_requests
            .iter()
            .map(|request| request.body["method"].as_str().expect("method"))
            .collect::<Vec<_>>(),
        vec!["initialize", "tools/list"]
    );
}

#[test]
fn print_rejects_remote_mcp_server_missing_url() {
    let temp = TempDir::new().expect("tempdir");
    let provider = MockSseServer::start(vec![]);
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        format!(
            r#"
api_base = "{}"

[[mcp.servers]]
id = "remote-docs"
enabled = true
transport = "http"
"#,
            provider.url
        ),
    )
    .expect("write config");

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["print", "show", "remote", "tools"]);
    let output = command.output().expect("neo command should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing MCP url for remote-docs"));
}

#[test]
fn mcp_resources_list_reads_remote_resource_catalog() {
    let temp = TempDir::new().expect("tempdir");
    let mcp_server = MockSseServer::start(vec![
        mcp_json_response(
            1,
            &json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "remote-docs", "version": "0.1.0"},
                "capabilities": {"resources": {}}
            }),
        ),
        mcp_json_response(
            2,
            &json!({
                "resources": [
                    {
                        "uri": "file://docs/readme.md",
                        "name": "README",
                        "description": "Project readme",
                        "mimeType": "text/markdown"
                    }
                ]
            }),
        ),
    ]);
    write_remote_mcp_config(temp.path(), &mcp_server.url);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .args(["mcp", "resources", "remote-docs", "list"]);
    let stdout = run(command);

    assert!(stdout.contains("file://docs/readme.md"));
    assert!(stdout.contains("README"));
    assert!(stdout.contains("text/markdown"));
}

#[test]
fn mcp_resources_read_fetches_remote_resource_content() {
    let temp = TempDir::new().expect("tempdir");
    let mcp_server = MockSseServer::start(vec![
        mcp_json_response(
            1,
            &json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "remote-docs", "version": "0.1.0"},
                "capabilities": {"resources": {}}
            }),
        ),
        mcp_json_response(
            2,
            &json!({
                "contents": [
                    {
                        "uri": "file://docs/readme.md",
                        "mimeType": "text/markdown",
                        "text": "# Neo"
                    }
                ]
            }),
        ),
    ]);
    write_remote_mcp_config(temp.path(), &mcp_server.url);

    let mut command = neo();
    command.current_dir(temp.path()).args([
        "mcp",
        "resources",
        "remote-docs",
        "read",
        "file://docs/readme.md",
    ]);
    let stdout = run(command);

    assert!(stdout.contains("file://docs/readme.md"));
    assert!(stdout.contains("text/markdown"));
    assert!(stdout.contains("# Neo"));
}

#[test]
fn mcp_resources_watch_receives_stdio_resource_update() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = temp.path().join("mcp-resource-update.py");
    let method_log = temp.path().join("mcp-methods.log");
    fs::write(&fixture, MCP_STDIO_RESOURCE_UPDATE_FIXTURE).expect("write MCP fixture");
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        format!(
            r#"
[[mcp.servers]]
id = "docs-server"
enabled = true
transport = "stdio"
command = "python3"
args = ["-u", "{}"]

[mcp.servers.env]
MCP_METHOD_LOG = "{}"
"#,
            fixture.display(),
            method_log.display()
        ),
    )
    .expect("write config");

    let mut command = neo();
    command.current_dir(temp.path()).args([
        "mcp",
        "resources",
        "docs-server",
        "watch",
        "file://docs/readme.md",
    ]);
    let stdout = run(command);

    assert_eq!(stdout, "file://docs/readme.md\n");
    let methods = fs::read_to_string(method_log).expect("read method log");
    assert_eq!(
        methods.lines().collect::<Vec<_>>(),
        vec![
            "initialize",
            "notifications/initialized",
            "resources/subscribe",
            "resources/unsubscribe"
        ]
    );
}

#[test]
fn mcp_resources_watch_respects_count_before_unsubscribe() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = temp.path().join("mcp-resource-update.py");
    let method_log = temp.path().join("mcp-methods.log");
    fs::write(&fixture, MCP_STDIO_RESOURCE_UPDATE_FIXTURE).expect("write MCP fixture");
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        format!(
            r#"
[[mcp.servers]]
id = "docs-server"
enabled = true
transport = "stdio"
command = "python3"
args = ["-u", "{}"]

[mcp.servers.env]
MCP_METHOD_LOG = "{}"
MCP_RESOURCE_UPDATE_COUNT = "2"
"#,
            fixture.display(),
            method_log.display()
        ),
    )
    .expect("write config");

    let mut command = neo();
    command.current_dir(temp.path()).args([
        "mcp",
        "resources",
        "docs-server",
        "watch",
        "file://docs/readme.md",
        "--count",
        "2",
    ]);
    let stdout = run(command);

    assert_eq!(
        stdout,
        "file://docs/readme.md\nfile://docs/readme.md?version=2\n"
    );
    let methods = fs::read_to_string(method_log).expect("read method log");
    assert_eq!(
        methods.lines().collect::<Vec<_>>(),
        vec![
            "initialize",
            "notifications/initialized",
            "resources/subscribe",
            "resources/unsubscribe"
        ]
    );
}

#[test]
fn mcp_resources_watch_receives_remote_sse_resource_update() {
    let temp = TempDir::new().expect("tempdir");
    let mcp_server = MockSseServer::start(vec![
        mcp_json_response(
            1,
            &json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "remote-resource-fixture", "version": "0.1.0"},
                "capabilities": {"resources": {"subscribe": true}}
            }),
        ),
        mcp_sse_resource_update_response(2, &json!({}), "file://docs/readme.md"),
        mcp_json_response(3, &json!({})),
    ]);
    write_remote_mcp_config(temp.path(), &mcp_server.url);

    let mut command = neo();
    command.current_dir(temp.path()).args([
        "mcp",
        "resources",
        "remote-docs",
        "watch",
        "file://docs/readme.md",
    ]);
    let stdout = run(command);

    assert_eq!(stdout, "file://docs/readme.md\n");
    assert_eq!(
        mcp_server
            .requests()
            .iter()
            .map(|request| request.body["method"].as_str().expect("method"))
            .collect::<Vec<_>>(),
        vec!["initialize", "resources/subscribe", "resources/unsubscribe"]
    );
}

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

fn mcp_json_response(id: u64, result: &Value) -> String {
    let body = json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
    .to_string();
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

fn mcp_sse_resource_update_response(id: u64, result: &Value, uri: &str) -> String {
    sse_response(&[
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }),
        json!({
            "jsonrpc": "2.0",
            "method": "notifications/resources/updated",
            "params": { "uri": uri },
        }),
    ])
}

fn write_remote_mcp_config(root: &Path, url: &str) {
    fs::create_dir_all(root.join(".neo")).expect("create .neo");
    fs::write(
        root.join(".neo/config.toml"),
        format!(
            r#"
[[mcp.servers]]
id = "remote-docs"
enabled = true
transport = "http"
url = "{url}"
"#
        ),
    )
    .expect("write remote MCP config");
}

fn sse_response(events: &[Value]) -> String {
    let mut body = String::new();
    for event in events {
        write!(&mut body, "data: {event}\n\n").expect("write SSE event");
    }
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
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

const MCP_STDIO_FIXTURE: &str = r#"
import json
import sys

for line in sys.stdin:
    request = json.loads(line)
    method = request["method"]
    if method == "initialize":
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "fixture", "version": "0.1.0"},
                "capabilities": {"tools": {}},
            },
        }
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "tools": [
                    {
                        "name": "docs-search",
                        "description": "Search project docs",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"query": {"type": "string"}},
                            "required": ["query"],
                        },
                    }
                ]
            },
        }
    elif method == "tools/call":
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "content": [{"type": "text", "text": "ok"}],
                "isError": False,
            },
        }
    else:
        response = {
            "jsonrpc": "2.0",
            "id": request.get("id"),
            "error": {"code": -32601, "message": f"unknown method {method}"},
        }
    print(json.dumps(response), flush=True)
"#;

const MCP_STDIO_RESOURCE_UPDATE_FIXTURE: &str = r#"
import json
import os
import sys

method_log = os.environ["MCP_METHOD_LOG"]
update_count = int(os.environ.get("MCP_RESOURCE_UPDATE_COUNT", "1"))

def log_method(method):
    with open(method_log, "a", encoding="utf-8") as log:
        log.write(method + "\n")

for line in sys.stdin:
    request = json.loads(line)
    method = request["method"]
    log_method(method)
    if method == "initialize":
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "resource-fixture", "version": "0.1.0"},
                "capabilities": {"resources": {"subscribe": True}},
            },
        }
    elif method == "notifications/initialized":
        continue
    elif method == "resources/subscribe":
        assert request["params"]["uri"] == "file://docs/readme.md"
        response = {"jsonrpc": "2.0", "id": request["id"], "result": {}}
        print(json.dumps(response), flush=True)
        for index in range(update_count):
            uri = "file://docs/readme.md" if index == 0 else f"file://docs/readme.md?version={index + 1}"
            notification = {
                "jsonrpc": "2.0",
                "method": "notifications/resources/updated",
                "params": {"uri": uri},
            }
            print(json.dumps(notification), flush=True)
        continue
    elif method == "resources/unsubscribe":
        assert request["params"]["uri"] == "file://docs/readme.md"
        response = {"jsonrpc": "2.0", "id": request["id"], "result": {}}
    else:
        response = {
            "jsonrpc": "2.0",
            "id": request.get("id"),
            "error": {"code": -32601, "message": f"unknown method {method}"},
        }
    print(json.dumps(response), flush=True)
"#;
