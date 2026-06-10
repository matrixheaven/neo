use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, OnceLock},
    thread,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer as _, SigningKey};
use neo_cloud::{CloudServer, Store};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tar::{Builder, Header};
use tempfile::TempDir;

static ISOLATED_HOMES: OnceLock<Mutex<Vec<TempDir>>> = OnceLock::new();

fn neo() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_neo"));
    command.env("HOME", isolated_home());
    command
}

fn isolated_home() -> PathBuf {
    let home = TempDir::new().expect("isolated home");
    let path = home.path().to_path_buf();
    ISOLATED_HOMES
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .expect("isolated home lock")
        .push(home);
    path
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

fn model_tool_names(body: &Value) -> Vec<&str> {
    let mut names = body["tools"]
        .as_array()
        .expect("model request tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect::<Vec<_>>();
    names.sort_unstable();
    names
}

fn output_value(output: &str, key: &str) -> String {
    output
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}: ")))
        .unwrap_or_else(|| panic!("missing {key} in output:\n{output}"))
        .trim()
        .to_owned()
}

fn read_session_metadata(root: &Path) -> Value {
    let content = fs::read_to_string(root.join(".neo/sessions/sessions.metadata.json"))
        .expect("read session metadata");
    serde_json::from_str(&content).expect("session metadata json")
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
fn root_verbose_flag_renders_real_startup_details() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command
        .current_dir(temp.path())
        .args(["--verbose", "--models", "sonnet"]);

    let stdout = run(command);

    let project_name = temp
        .path()
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .expect("tempdir has utf8 basename");
    assert!(stdout.contains("Startup"));
    assert!(stdout.contains("project:"));
    assert!(stdout.contains(project_name));
    assert!(stdout.contains("sessions:"));
    assert!(stdout.contains(".neo/sessions"));
    assert!(stdout.contains("model scope: sonnet"));
    assert!(!stdout.contains("placeholder"));
    assert!(!stdout.contains("fake"));
}

#[test]
fn root_theme_flag_loads_theme_for_verbose_startup() {
    let temp = TempDir::new().expect("tempdir");
    let theme_path = temp.path().join("solarized-neo.json");
    fs::write(
        &theme_path,
        r##"
{
  "name": "Solarized Neo",
  "colors": {
    "header": "#268bd2",
    "prompt": "yellow",
    "user": "magenta",
    "assistant": "blue",
    "notice": "gray"
  }
}
"##,
    )
    .expect("write theme");

    let mut command = neo();
    command
        .current_dir(temp.path())
        .arg("--theme")
        .arg(&theme_path)
        .args(["--verbose"]);

    let stdout = run(command);

    assert!(stdout.contains("theme: Solarized Neo"));
}

#[test]
fn root_no_themes_disables_project_theme_discovery() {
    let temp = TempDir::new().expect("tempdir");
    let themes = temp.path().join(".neo/themes");
    fs::create_dir_all(&themes).expect("create themes");
    fs::write(
        themes.join("auto.json"),
        r#"
{
  "name": "Auto Theme",
  "colors": {
    "notice": "yellow"
  }
}
"#,
    )
    .expect("write auto theme");

    let mut command = neo();
    command
        .current_dir(temp.path())
        .args(["--no-themes", "--verbose"]);

    let stdout = run(command);

    assert!(stdout.contains("theme: default"));
    assert!(!stdout.contains("Auto Theme"));
}

#[test]
fn root_resume_flag_opens_real_local_session_picker() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions).expect("create sessions");
    fs::write(
        sessions.join("alpha.jsonl"),
        "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello\"}}]}}}}\n",
    )
    .expect("write session");

    let mut command = neo();
    command.current_dir(temp.path()).arg("-r");

    let stdout = run(command);

    assert!(stdout.contains("Sessions"));
    assert!(stdout.contains("alpha"));
    assert!(stdout.contains("session"));
    assert!(!stdout.contains("placeholder"));
    assert!(!stdout.contains("fake"));
}

#[test]
fn root_resume_flag_rejects_subcommands_instead_of_being_ignored() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command
        .current_dir(temp.path())
        .args(["-r", "config", "show"]);

    let output = command.output().expect("neo command should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--resume/-r starts the interactive session picker"));
}

#[test]
fn root_resume_flag_rejects_options_that_would_bypass_or_rename_the_picker() {
    let temp = TempDir::new().expect("tempdir");
    for args in [
        vec!["-r", "--list-models"],
        vec!["-r", "--name", "ignored"],
        vec!["-r", "--no-session"],
    ] {
        let mut command = neo();
        command.current_dir(temp.path()).args(args);

        let output = command.output().expect("neo command should run");

        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("cannot be used with")
                || stderr.contains("--resume/-r starts the interactive session picker"),
            "stderr did not explain resume conflict:\n{stderr}"
        );
    }
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
fn config_show_session_dir_cli_override_takes_precedence_over_env_and_files() {
    let home = TempDir::new().expect("home tempdir");
    let project = TempDir::new().expect("project tempdir");
    let cli_sessions = project.path().join("cli-sessions");
    let env_sessions = project.path().join("env-sessions");
    fs::create_dir_all(home.path().join(".neo")).expect("create global .neo");
    fs::write(
        home.path().join(".neo/config.toml"),
        r#"
sessions_dir = "~/.neo/global-sessions"
"#,
    )
    .expect("write global config");
    fs::create_dir_all(project.path().join(".neo")).expect("create project .neo");
    fs::write(
        project.path().join(".neo/config.toml"),
        r#"
sessions_dir = ".neo/project-sessions"
"#,
    )
    .expect("write project config");

    let mut command = neo();
    command
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("NEO_SESSIONS_DIR", &env_sessions)
        .args(["--session-dir"])
        .arg(&cli_sessions)
        .args(["config", "show"]);

    let stdout = run(command);

    assert!(stdout.contains(&format!("sessions_dir = \"{}\"", cli_sessions.display())));
    assert!(!stdout.contains(&env_sessions.display().to_string()));
    assert!(!stdout.contains("project-sessions"));
    assert!(!stdout.contains("global-sessions"));
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
fn cloud_login_status_logout_manage_auth_file_without_printing_tokens() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let temp = TempDir::new().expect("tempdir");
    let server = runtime.block_on(start_cloud_server(temp.path().join("cloud.sqlite")));
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        r#"
[cloud]
auth_file = ".neo/custom-auth.json"
"#,
    )
    .expect("write config");

    let mut login = neo();
    login
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .args(["login", "cloud", "--server", &server.base_url]);
    let login_stdout = run(login);

    assert!(login_stdout.contains("logged in to"));
    assert!(login_stdout.contains(&server.base_url));
    assert!(!login_stdout.contains("access_token"));
    assert!(!login_stdout.contains("device_token"));
    let auth_path = temp.path().join(".neo/custom-auth.json");
    let auth_json: Value =
        serde_json::from_str(&fs::read_to_string(&auth_path).expect("read auth file"))
            .expect("auth json");
    assert_eq!(auth_json["server_url"], server.base_url);
    assert!(
        auth_json["access_token"]
            .as_str()
            .is_some_and(|token| !token.is_empty())
    );
    assert!(
        auth_json["device_token"]
            .as_str()
            .is_some_and(|token| !token.is_empty())
    );

    let mut status = neo();
    status
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .args(["auth", "status"]);
    let status_stdout = run(status);
    assert!(status_stdout.contains("logged in"));
    assert!(status_stdout.contains(&server.base_url));
    assert!(!status_stdout.contains(auth_json["access_token"].as_str().expect("token")));
    assert!(!status_stdout.contains(auth_json["device_token"].as_str().expect("token")));

    let mut logout = neo();
    logout
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .arg("logout");
    let logout_stdout = run(logout);
    assert!(logout_stdout.contains("logged out"));
    assert!(!auth_path.exists());

    drop(server);
}

#[test]
fn cloud_status_reports_self_hosted_cloud_health_without_auth_file() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let temp = TempDir::new().expect("tempdir");
    let server = runtime.block_on(start_cloud_server(temp.path().join("cloud.sqlite")));

    let mut status = neo();
    status
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .args(["cloud", "status", "--api-base", &server.base_url]);
    let stdout = run(status);

    assert!(stdout.contains("cloud available"));
    assert!(stdout.contains(&server.base_url));

    drop(server);
}

#[test]
fn config_sync_push_status_and_pull_round_trip_global_profile_without_project_trust_or_sessions() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let home = TempDir::new().expect("home tempdir");
    let project = TempDir::new().expect("project tempdir");
    let server = runtime.block_on(start_cloud_server(home.path().join("cloud.sqlite")));
    fs::create_dir_all(home.path().join(".neo")).expect("create home .neo");
    fs::create_dir_all(project.path().join(".neo/sessions")).expect("create project sessions");
    fs::write(
        project.path().join(".neo/trust.toml"),
        "decision = \"deny\"\n",
    )
    .expect("write project trust");
    fs::write(
        project.path().join(".neo/sessions/local.jsonl"),
        "{\"local\":true}\n",
    )
    .expect("write project session");
    fs::write(
        home.path().join(".neo/config.toml"),
        r#"
default_provider = "anthropic"
default_model = "deepseek-v4-pro"
model_scope = ["anthropic/deepseek-*"]

[providers.anthropic]
api_base = "https://api.deepseek.com/anthropic"
api_key_env = "DEEPSEEK_API_KEY"

[runtime]
max_tokens = 2048
reasoning_effort = "high"
tool_execution_mode = "Sequential"

[tui.keybindings]
"tui.input.submit" = ["ctrl+j"]

[cloud]
auth_file = "~/.neo/auth.json"
"#,
    )
    .expect("write home config");
    fs::write(
        project.path().join(".neo/extensions-state.toml"),
        r#"
[extensions.echo]
status = "enabled"
name = "Echo"
version = "0.1.0"
manifest_path = ".neo/extensions/echo/neo-extension.toml"
"#,
    )
    .expect("write extension state");

    let mut login = neo();
    login
        .current_dir(project.path())
        .env("HOME", home.path())
        .args(["login", "cloud", "--server", &server.base_url]);
    run(login);

    let mut push = neo();
    push.current_dir(project.path())
        .env("HOME", home.path())
        .args(["config", "sync", "push"]);
    let push_stdout = run(push);
    assert!(push_stdout.contains("profile pushed"));
    assert!(push_stdout.contains("revision 1"));

    fs::write(
        home.path().join(".neo/config.toml"),
        format!(
            r#"
default_provider = "openai"
default_model = "gpt-4.1"

[cloud]
auth_file = "{}"
"#,
            home.path().join(".neo/auth.json").display()
        ),
    )
    .expect("replace home config");

    let mut status = neo();
    status
        .current_dir(project.path())
        .env("HOME", home.path())
        .args(["config", "sync", "status"]);
    let status_stdout = run(status);
    assert!(status_stdout.contains("remote revision 1"));

    let mut pull = neo();
    pull.current_dir(project.path())
        .env("HOME", home.path())
        .args(["config", "sync", "pull"]);
    let pull_stdout = run(pull);
    assert!(pull_stdout.contains("profile pulled"));

    let global_config = fs::read_to_string(home.path().join(".neo/config.toml"))
        .expect("read pulled global config");
    assert!(global_config.contains("default_provider = \"anthropic\""));
    assert!(global_config.contains("default_model = \"deepseek-v4-pro\""));
    assert!(global_config.contains("model_scope = [\"anthropic/deepseek-*\"]"));
    assert!(global_config.contains("api_base = \"https://api.deepseek.com/anthropic\""));
    assert!(global_config.contains("reasoning_effort = \"high\""));
    assert!(global_config.contains("\"tui.input.submit\" = [\"ctrl+j\"]"));
    assert!(project.path().join(".neo/trust.toml").exists());
    assert!(project.path().join(".neo/sessions/local.jsonl").exists());

    drop(server);
}

#[test]
fn config_show_does_not_print_cli_api_key_value() {
    let temp = TempDir::new().expect("tempdir");

    let mut command = neo();
    command
        .current_dir(temp.path())
        .args(["--api-key", "runtime-secret", "config", "show"]);

    let stdout = run(command);

    assert!(!stdout.contains("runtime-secret"));
    assert!(!stdout.contains("api_key"));
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
fn config_show_reads_tui_image_protocol_and_remote_fetch_policy() {
    let temp = TempDir::new().expect("tempdir");
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        r#"
[tui]
image_protocol = "kitty"
fetch_remote_images = true
"#,
    )
    .expect("write config");

    let mut command = neo();
    command.current_dir(temp.path()).args(["config", "show"]);
    let stdout = run(command);

    assert!(stdout.contains("[tui]"));
    assert!(stdout.contains("image_protocol = \"kitty\""));
    assert!(stdout.contains("fetch_remote_images = true"));
}

#[test]
fn config_set_writes_tui_image_protocol_and_remote_fetch_policy() {
    let temp = TempDir::new().expect("tempdir");

    let mut command = neo();
    command
        .current_dir(temp.path())
        .args(["config", "set", "tui.image_protocol", "sixel"]);
    let stdout = run(command);
    assert!(stdout.contains("set tui.image_protocol"));

    let mut command = neo();
    command
        .current_dir(temp.path())
        .args(["config", "set", "tui.fetch_remote_images", "true"]);
    let stdout = run(command);
    assert!(stdout.contains("set tui.fetch_remote_images"));

    let config = fs::read_to_string(temp.path().join(".neo/config.toml")).expect("read config");
    let value: toml::Value = toml::from_str(&config).expect("config should be valid toml");
    assert_eq!(value["tui"]["image_protocol"].as_str(), Some("sixel"));
    assert_eq!(value["tui"]["fetch_remote_images"].as_bool(), Some(true));
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
fn config_set_writes_app_keybinding_override() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command.current_dir(temp.path()).args([
        "config",
        "set",
        "tui.keybindings.app.exit",
        "[\"ctrl+x\"]",
    ]);
    let stdout = run(command);

    assert!(stdout.contains("set tui.keybindings.app.exit"));
    let config = fs::read_to_string(temp.path().join(".neo/config.toml")).expect("read config");
    let value: toml::Value = toml::from_str(&config).expect("config should be valid toml");
    let keys = value["tui"]["keybindings"]["app.exit"]
        .as_array()
        .expect("keybinding override should be an array")
        .iter()
        .map(|value| value.as_str().expect("key should be a string").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(keys, vec!["ctrl+x"]);
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
fn sessions_share_import_resume_and_sync_use_self_hosted_cloud() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let temp = TempDir::new().expect("tempdir");
    let server = runtime.block_on(start_cloud_server(temp.path().join("cloud.sqlite")));
    let sessions = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions).expect("create sessions");
    let session_path = sessions.join("alpha.jsonl");
    let secret = "sk-test-secret-token";
    fs::write(
        &session_path,
        format!(
            "{}\n",
            json!({
                "MessageAppended": {
                    "message": {
                        "User": {
                            "content": [{
                                "Text": {
                                    "text": format!(
                                        "please replay this, but not {secret} or {}",
                                        session_path.display()
                                    )
                                }
                            }]
                        }
                    }
                }
            })
        ),
    )
    .expect("write session");

    let mut login = neo();
    login
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .args(["login", "cloud", "--server", &server.base_url]);
    run(login);

    let mut push = neo();
    push.current_dir(temp.path())
        .env("HOME", temp.path())
        .args(["sessions", "sync", "push"]);
    let push_stdout = run(push);
    assert!(push_stdout.contains("pushed alpha"));
    assert!(push_stdout.contains("cloud_id=cs_"));

    let mut share = neo();
    share
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .args(["sessions", "share", "alpha", "--public"]);
    let share_stdout = run(share);
    let cloud_id = output_value(&share_stdout, "cloud_id");
    let share_id = output_value(&share_stdout, "share_id");
    assert!(cloud_id.starts_with("cs_"));
    assert!(share_id.starts_with("sh_"));
    assert!(share_stdout.contains(&format!("{}/v1/shares/{share_id}.html", server.base_url)));
    assert!(!share_stdout.contains(secret));
    assert!(!share_stdout.contains(temp.path().to_str().expect("temp path")));

    let metadata = read_session_metadata(temp.path());
    assert_eq!(metadata["sessions"]["alpha"]["cloud_id"], cloud_id);
    assert_eq!(
        metadata["sessions"]["alpha"]["share_ids"],
        json!([share_id])
    );

    let mut status = neo();
    status
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .args(["sessions", "sync", "status"]);
    let status_stdout = run(status);
    assert!(status_stdout.contains("alpha"));
    assert!(status_stdout.contains(&cloud_id));
    assert!(status_stdout.contains(&share_id));

    let mut import = neo();
    import
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .args(["sessions", "import", &share_id]);
    let import_stdout = run(import);
    let imported_id = output_value(&import_stdout, "session_id");
    let imported_jsonl =
        fs::read_to_string(sessions.join(format!("{imported_id}.jsonl"))).expect("read import");
    assert!(imported_jsonl.contains("please replay this"));
    assert!(!imported_jsonl.contains(secret));
    assert!(!imported_jsonl.contains(temp.path().to_str().expect("temp path")));

    let mut resume = neo();
    resume
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .args(["resume", &cloud_id]);
    let resume_stdout = run(resume);
    assert!(resume_stdout.contains("remote_parent_id:"));
    assert!(resume_stdout.contains("please replay this"));
    assert!(!resume_stdout.contains(secret));
    assert!(!resume_stdout.contains(temp.path().to_str().expect("temp path")));

    drop(server);
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
fn sessions_export_json_returns_sanitized_replayed_session_artifact() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions).expect("create sessions");
    fs::write(
        sessions.join("alpha-main.jsonl"),
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello json export\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"portable local reply\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    )
    .expect("write session");
    fs::write(sessions.join("alpha-main-fork-1.jsonl"), "{}\n").expect("write child session");
    fs::write(
        sessions.join("sessions.metadata.json"),
        json!({
            "sessions": {
                "alpha-main": {
                    "name": "Main thread",
                    "summary": "Local branch summary"
                },
                "alpha-main-fork-1": {
                    "parent_id": "alpha-main"
                }
            }
        })
        .to_string(),
    )
    .expect("write metadata");

    let mut export = neo();
    export
        .current_dir(temp.path())
        .args(["sessions", "export-json", "alpha-main"]);
    let stdout = run(export);

    assert!(
        !stdout.contains(temp.path().to_str().expect("temp path")),
        "export JSON should not leak absolute paths: {stdout}"
    );
    assert!(!stdout.contains("share_url"));
    assert!(!stdout.contains("hosted"));

    let artifact: Value = serde_json::from_str(&stdout).expect("export artifact JSON");
    assert_eq!(artifact["format"], "neo.session.export_json");
    assert_eq!(artifact["schema_version"], 1);
    assert_eq!(artifact["metadata"]["id"], "alpha-main");
    assert_eq!(artifact["metadata"]["name"], "Main thread");
    assert_eq!(artifact["metadata"]["summary"], "Local branch summary");
    assert!(artifact["metadata"]["parent_id"].is_null());
    assert_eq!(
        artifact["metadata"]["children"],
        json!(["alpha-main-fork-1"])
    );
    assert_eq!(artifact["metadata"]["message_count"], 2);
    assert_eq!(
        artifact["messages"][0]["User"]["content"][0]["Text"]["text"],
        "hello json export"
    );
    assert_eq!(
        artifact["messages"][1]["Assistant"]["content"][0]["Text"]["text"],
        "portable local reply"
    );
}

#[test]
fn root_export_flag_writes_default_html_file_from_session_jsonl() {
    let temp = TempDir::new().expect("tempdir");
    fs::write(
        temp.path().join("alpha.jsonl"),
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello <neo>\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"use **bold**\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    )
    .expect("write session");

    let mut export = neo();
    export
        .current_dir(temp.path())
        .args(["--export", "alpha.jsonl"]);
    let stdout = run(export);

    assert!(stdout.contains("Exported to: neo-session-alpha.html"));
    let html = fs::read_to_string(temp.path().join("neo-session-alpha.html")).expect("read html");
    assert!(html.contains("<!doctype html>"));
    assert!(html.contains("hello &lt;neo&gt;"));
    assert!(html.contains("<strong>bold</strong>"));
    assert!(!html.contains("fake"));
}

#[test]
fn root_export_flag_writes_explicit_html_output_path() {
    let temp = TempDir::new().expect("tempdir");
    fs::write(
        temp.path().join("alpha.jsonl"),
        "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"export me\"}}]}}}}\n",
    )
    .expect("write session");

    let mut export = neo();
    export
        .current_dir(temp.path())
        .args(["--export", "alpha.jsonl", "out.html"]);
    let stdout = run(export);

    assert!(stdout.contains("Exported to: out.html"));
    let html = fs::read_to_string(temp.path().join("out.html")).expect("read html");
    assert!(html.contains("export me"));
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
fn extensions_update_skips_git_source_when_offline_env_is_enabled() {
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
    run(install);

    write_extension_manifest(&repo, "git_echo", "Git Echo", "0.2.0");
    commit_git_repo(&repo, "update extension");

    let mut update = neo();
    update
        .current_dir(temp.path())
        .env("NEO_OFFLINE", "1")
        .args(["extensions", "update", "git_echo"]);
    let skipped = run(update);
    assert!(skipped.contains("offline: skipped extension update git_echo"));

    let manifest = fs::read_to_string(
        temp.path()
            .join(".neo/extensions/git_echo/neo-extension.toml"),
    )
    .expect("read installed extension manifest");
    assert!(manifest.contains("version = \"0.1.0\""));

    let mut list = neo();
    list.current_dir(temp.path()).args(["extensions", "list"]);
    let listed = run(list);
    assert!(listed.contains("git_echo"));
    assert!(listed.contains("0.1.0"));
    assert!(!listed.contains("0.2.0"));
}

#[test]
fn extensions_update_skips_local_source_when_offline_flag_is_set() {
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

    write_extension_manifest(&source, "echo", "Echo", "0.2.0");

    let mut update = neo();
    update
        .current_dir(temp.path())
        .args(["--offline", "extensions", "update", "echo", "--root"])
        .arg(&root);
    let skipped = run(update);
    assert!(skipped.contains("offline: skipped extension update echo"));

    let manifest = fs::read_to_string(root.join("echo/neo-extension.toml"))
        .expect("read installed extension manifest");
    assert!(manifest.contains("version = \"0.1.0\""));
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

#[test]
fn extensions_search_reads_real_marketplace_catalog() {
    let marketplace = MockSseServer::start(vec![json_response(&json!({
        "packages": [
            {
                "kind": "extension",
                "id": "echo",
                "version": "0.1.0",
                "name": "Echo",
                "description": "Echo extension",
                "publisher": "neo-test"
            }
        ]
    }))]);

    let mut search = neo();
    search
        .env("NEO_MARKETPLACE_URL", &marketplace.url)
        .args(["extensions", "search", "echo"]);
    let stdout = run(search);

    assert!(stdout.contains("echo"));
    assert!(stdout.contains("0.1.0"));
    assert!(stdout.contains("Echo extension"));
    let requests = marketplace.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert!(
        requests[0]
            .path
            .contains("/api/v1/marketplace/packages/search")
    );
    assert!(requests[0].path.contains("kind=extension"));
    assert!(requests[0].path.contains("q=echo"));
}

#[test]
fn extensions_marketplace_install_downloads_and_validates_package() {
    let temp = TempDir::new().expect("tempdir");
    let package_dir = TempDir::new().expect("package tempdir");
    let publisher_key = SigningKey::from_bytes(&[23_u8; 32]);
    trust_test_publisher(temp.path(), &publisher_key);
    let package = write_trusted_neo_package(
        package_dir.path(),
        "extension",
        "echo",
        "0.1.0",
        "neo-extension.toml",
        &publisher_key,
        &[PackageFixtureEntry::file(
            "neo-extension.toml",
            r#"
id = "echo"
name = "Echo"
version = "0.1.0"

[runner]
command = "python3"
"#,
        )],
    );
    let manifest = fs::read_to_string(&package).expect("read package manifest");
    let archive = fs::read(package_dir.path().join("echo-0.1.0.tar")).expect("read archive");
    let marketplace = MockSseServer::start(vec![
        json_response(&json!({
            "package": {
                "kind": "extension",
                "id": "echo",
                "version": "0.1.0",
                "manifest_url": "/api/v1/marketplace/packages/extension/echo/0.1.0/.neo-package.toml",
                "archive_url": "/api/v1/marketplace/packages/extension/echo/0.1.0/echo-0.1.0.tar"
            }
        })),
        text_response("application/toml", &manifest),
        binary_response("application/x-tar", &archive),
    ]);

    let mut install = neo();
    install
        .current_dir(temp.path())
        .env("NEO_MARKETPLACE_URL", &marketplace.url)
        .args([
            "extensions",
            "install",
            "echo@0.1.0",
            "--from",
            "marketplace",
        ]);
    let stdout = run(install);

    assert!(stdout.contains("echo installed 0.1.0"));
    assert!(stdout.contains("marketplace"));
    assert!(
        temp.path()
            .join(".neo/extensions/echo/neo-extension.toml")
            .exists()
    );
    let requests = marketplace.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests
            .iter()
            .map(|request| request.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "/api/v1/marketplace/packages/extension/echo/0.1.0",
            "/api/v1/marketplace/packages/extension/echo/0.1.0/.neo-package.toml",
            "/api/v1/marketplace/packages/extension/echo/0.1.0/echo-0.1.0.tar",
        ]
    );
}

#[test]
fn trust_publishers_add_list_remove_and_revoke_use_local_store() {
    let temp = TempDir::new().expect("tempdir");
    let publisher_key = SigningKey::from_bytes(&[23_u8; 32]);
    let public_key = STANDARD.encode(publisher_key.verifying_key().to_bytes());

    let mut add = neo();
    add.current_dir(temp.path()).args([
        "trust",
        "publishers",
        "add",
        "neo-test",
        "--name",
        "Neo Test",
        "--root",
        "local-root",
        "--key-id",
        "ed25519:2026-a",
        "--public-key",
        &public_key,
        "--account-id",
        "acct_neo_test",
    ]);
    let added = run(add);
    assert!(added.contains("trusted publisher neo-test"));
    assert!(temp.path().join(".neo/package-trust.toml").exists());

    let mut list = neo();
    list.current_dir(temp.path())
        .args(["trust", "publishers", "list"]);
    let listed = run(list);
    assert!(listed.contains("neo-test"));
    assert!(listed.contains("local-root"));
    assert!(listed.contains("ed25519:2026-a"));
    assert!(listed.contains("trusted"));

    let mut revoke = neo();
    revoke.current_dir(temp.path()).args([
        "trust",
        "publishers",
        "revoke-key",
        "neo-test",
        "ed25519:2026-a",
        "--reason",
        "rotated",
    ]);
    let revoked = run(revoke);
    assert!(revoked.contains("revoked publisher neo-test key ed25519:2026-a"));

    let mut remove = neo();
    remove
        .current_dir(temp.path())
        .args(["trust", "publishers", "remove", "neo-test"]);
    let removed = run(remove);
    assert!(removed.contains("removed publisher neo-test"));
}

#[test]
fn marketplace_install_requires_trusted_publisher_before_extracting_archive() {
    let temp = TempDir::new().expect("tempdir");
    let package_dir = TempDir::new().expect("package tempdir");
    let publisher_key = SigningKey::from_bytes(&[23_u8; 32]);
    let package = write_trusted_neo_package(
        package_dir.path(),
        "extension",
        "echo",
        "0.1.0",
        "neo-extension.toml",
        &publisher_key,
        &[PackageFixtureEntry::file(
            "neo-extension.toml",
            r#"
id = "echo"
name = "Echo"
version = "0.1.0"

[runner]
command = "python3"
"#,
        )],
    );
    let manifest = fs::read_to_string(&package).expect("read package manifest");
    let archive = fs::read(package_dir.path().join("echo-0.1.0.tar")).expect("read archive");
    let marketplace = MockSseServer::start(vec![
        json_response(&json!({
            "package": {
                "kind": "extension",
                "id": "echo",
                "version": "0.1.0",
                "manifest_url": "/api/v1/marketplace/packages/extension/echo/0.1.0/.neo-package.toml",
                "archive_url": "/api/v1/marketplace/packages/extension/echo/0.1.0/echo-0.1.0.tar"
            }
        })),
        text_response("application/toml", &manifest),
        binary_response("application/x-tar", &archive),
    ]);

    let output = neo()
        .current_dir(temp.path())
        .env("NEO_MARKETPLACE_URL", &marketplace.url)
        .args([
            "extensions",
            "install",
            "echo@0.1.0",
            "--from",
            "marketplace",
        ])
        .output()
        .expect("neo command should run");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not trusted"));
    assert!(!temp.path().join(".neo/extensions/echo").exists());
}

#[test]
fn marketplace_install_succeeds_for_trusted_publisher_and_reports_trust_state() {
    let temp = TempDir::new().expect("tempdir");
    let package_dir = TempDir::new().expect("package tempdir");
    let publisher_key = SigningKey::from_bytes(&[23_u8; 32]);
    trust_test_publisher(temp.path(), &publisher_key);
    let package = write_trusted_neo_package(
        package_dir.path(),
        "extension",
        "echo",
        "0.1.0",
        "neo-extension.toml",
        &publisher_key,
        &[PackageFixtureEntry::file(
            "neo-extension.toml",
            r#"
id = "echo"
name = "Echo"
version = "0.1.0"

[runner]
command = "python3"
"#,
        )],
    );
    let manifest = fs::read_to_string(&package).expect("read package manifest");
    let archive = fs::read(package_dir.path().join("echo-0.1.0.tar")).expect("read archive");
    let marketplace = MockSseServer::start(vec![
        json_response(&json!({
            "package": {
                "kind": "extension",
                "id": "echo",
                "version": "0.1.0",
                "manifest_url": "/api/v1/marketplace/packages/extension/echo/0.1.0/.neo-package.toml",
                "archive_url": "/api/v1/marketplace/packages/extension/echo/0.1.0/echo-0.1.0.tar"
            }
        })),
        text_response("application/toml", &manifest),
        binary_response("application/x-tar", &archive),
    ]);

    let mut install = neo();
    install
        .current_dir(temp.path())
        .env("NEO_MARKETPLACE_URL", &marketplace.url)
        .args([
            "extensions",
            "install",
            "echo@0.1.0",
            "--from",
            "marketplace",
        ]);
    let stdout = run(install);

    assert!(stdout.contains("echo installed 0.1.0"));
    assert!(stdout.contains("marketplace"));
    assert!(stdout.contains("neo-test"));
    assert!(stdout.contains("trusted"));
    assert!(
        temp.path()
            .join(".neo/extensions/echo/neo-extension.toml")
            .exists()
    );
}

#[test]
fn extensions_marketplace_install_rejects_cross_origin_package_urls() {
    let temp = TempDir::new().expect("tempdir");
    let marketplace = MockSseServer::start(vec![json_response(&json!({
        "package": {
            "kind": "extension",
            "id": "echo",
            "version": "0.1.0",
            "manifest_url": "https://example.invalid/.neo-package.toml",
            "archive_url": "/api/v1/marketplace/packages/extension/echo/0.1.0/echo-0.1.0.tar"
        }
    }))]);

    let output = neo()
        .current_dir(temp.path())
        .env("NEO_MARKETPLACE_URL", &marketplace.url)
        .args([
            "extensions",
            "install",
            "echo@0.1.0",
            "--from",
            "marketplace",
        ])
        .output()
        .expect("neo command should run");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("configured marketplace origin"));
    assert_eq!(marketplace.requests().len(), 1);
    assert!(!temp.path().join(".neo/extensions/echo").exists());
}

#[test]
fn extensions_marketplace_install_allows_cross_origin_only_with_explicit_policy() {
    let temp = TempDir::new().expect("tempdir");
    let package_dir = TempDir::new().expect("package tempdir");
    let publisher_key = SigningKey::from_bytes(&[23_u8; 32]);
    trust_test_publisher(temp.path(), &publisher_key);
    let package = write_trusted_neo_package(
        package_dir.path(),
        "extension",
        "echo",
        "0.1.0",
        "neo-extension.toml",
        &publisher_key,
        &[PackageFixtureEntry::file(
            "neo-extension.toml",
            r#"
id = "echo"
name = "Echo"
version = "0.1.0"

[runner]
command = "python3"
"#,
        )],
    );
    let manifest = fs::read_to_string(&package).expect("read package manifest");
    let archive = fs::read(package_dir.path().join("echo-0.1.0.tar")).expect("read archive");
    let asset_server = MockSseServer::start(vec![
        text_response("application/toml", &manifest),
        binary_response("application/x-tar", &archive),
    ]);
    let marketplace = MockSseServer::start(vec![json_response(&json!({
        "package": {
            "kind": "extension",
            "id": "echo",
            "version": "0.1.0",
            "manifest_url": format!("{}/manifest.toml", asset_server.url),
            "archive_url": format!("{}/echo-0.1.0.tar", asset_server.url)
        }
    }))]);

    let mut install = neo();
    install
        .current_dir(temp.path())
        .env("NEO_MARKETPLACE_URL", &marketplace.url)
        .env("NEO_MARKETPLACE_ALLOW_CROSS_ORIGIN", "1")
        .args([
            "extensions",
            "install",
            "echo@0.1.0",
            "--from",
            "marketplace",
        ]);
    let stdout = run(install);

    assert!(stdout.contains("echo installed 0.1.0"));
    assert_eq!(marketplace.requests().len(), 1);
    assert_eq!(asset_server.requests().len(), 2);
}

#[test]
fn extensions_marketplace_install_fails_without_catalog_configuration() {
    let temp = TempDir::new().expect("tempdir");
    let output = neo()
        .current_dir(temp.path())
        .env_remove("NEO_MARKETPLACE_URL")
        .args(["extensions", "install", "echo", "--from", "marketplace"])
        .output()
        .expect("neo command should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("NEO_MARKETPLACE_URL"));
    assert!(!temp.path().join(".neo/extensions/echo").exists());
}

#[test]
fn extensions_publish_validates_package_and_posts_to_marketplace() {
    let package_dir = TempDir::new().expect("package tempdir");
    let package = write_signed_neo_package(
        package_dir.path(),
        "extension",
        "echo",
        "0.1.0",
        "neo-extension.toml",
        &[PackageFixtureEntry::file(
            "neo-extension.toml",
            r#"
id = "echo"
name = "Echo"
version = "0.1.0"

[runner]
command = "python3"
"#,
        )],
    );
    let marketplace = MockSseServer::start(vec![json_response(&json!({
        "package": {
            "kind": "extension",
            "id": "echo",
            "version": "0.1.0",
            "name": "Echo",
            "description": "Echo extension",
            "publisher": "neo-test"
        }
    }))]);

    let mut publish = neo();
    publish
        .env("NEO_MARKETPLACE_URL", &marketplace.url)
        .args(["extensions", "publish"])
        .arg(&package);
    let stdout = run(publish);

    assert!(stdout.contains("echo published 0.1.0"));
    let requests = marketplace.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/api/v1/marketplace/packages/publish");
    assert_eq!(requests[0].body["manifest"]["id"], "echo");
    assert_eq!(requests[0].body["manifest"]["kind"], "extension");
    assert!(requests[0].body["archive_base64"].as_str().is_some());
}

#[test]
fn prompt_packages_publish_validates_package_and_posts_to_marketplace() {
    let package_dir = TempDir::new().expect("package tempdir");
    let package = write_signed_neo_package(
        package_dir.path(),
        "prompt-pack",
        "review-pack",
        "1.0.0",
        "review.md",
        &[PackageFixtureEntry::file(
            "review.md",
            "---\ndescription: Review code\n---\nReview $ARGUMENTS\n",
        )],
    );
    let marketplace = MockSseServer::start(vec![json_response(&json!({
        "package": {
            "kind": "prompt-pack",
            "id": "review-pack",
            "version": "1.0.0",
            "name": "Review Pack",
            "description": "Review prompts",
            "publisher": "neo-test"
        }
    }))]);

    let mut publish = neo();
    publish
        .env("NEO_MARKETPLACE_URL", &marketplace.url)
        .args(["prompts", "publish"])
        .arg(&package);
    let stdout = run(publish);

    assert!(stdout.contains("review-pack published 1.0.0"));
    let requests = marketplace.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/api/v1/marketplace/packages/publish");
    assert_eq!(requests[0].body["manifest"]["id"], "review-pack");
    assert_eq!(requests[0].body["manifest"]["kind"], "prompt-pack");
    assert!(requests[0].body["archive_base64"].as_str().is_some());
}

#[test]
fn theme_packages_publish_validates_package_and_posts_to_marketplace() {
    let package_dir = TempDir::new().expect("package tempdir");
    let package = write_signed_neo_package(
        package_dir.path(),
        "theme",
        "night-owl",
        "2.0.0",
        "night-owl.json",
        &[PackageFixtureEntry::file(
            "night-owl.json",
            r##"{"name":"Night Owl","colors":{"prompt":"#82aaff"}}"##,
        )],
    );
    let marketplace = MockSseServer::start(vec![json_response(&json!({
        "package": {
            "kind": "theme",
            "id": "night-owl",
            "version": "2.0.0",
            "name": "Night Owl",
            "description": "Night Owl theme",
            "publisher": "neo-test"
        }
    }))]);

    let mut publish = neo();
    publish
        .env("NEO_MARKETPLACE_URL", &marketplace.url)
        .args(["themes", "publish"])
        .arg(&package);
    let stdout = run(publish);

    assert!(stdout.contains("night-owl published 2.0.0"));
    let requests = marketplace.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/api/v1/marketplace/packages/publish");
    assert_eq!(requests[0].body["manifest"]["id"], "night-owl");
    assert_eq!(requests[0].body["manifest"]["kind"], "theme");
    assert!(requests[0].body["archive_base64"].as_str().is_some());
}

#[test]
fn prompt_packages_install_list_and_preview_from_marketplace() {
    let temp = TempDir::new().expect("tempdir");
    let package_dir = TempDir::new().expect("package tempdir");
    let publisher_key = SigningKey::from_bytes(&[23_u8; 32]);
    trust_test_publisher(temp.path(), &publisher_key);
    let package = write_trusted_neo_package(
        package_dir.path(),
        "prompt-pack",
        "review-pack",
        "1.0.0",
        "review.md",
        &publisher_key,
        &[PackageFixtureEntry::file(
            "review.md",
            "---\ndescription: Review code\n---\nReview $ARGUMENTS\n",
        )],
    );
    let manifest = fs::read_to_string(&package).expect("read package manifest");
    let archive = fs::read(package_dir.path().join("review-pack-1.0.0.tar")).expect("read archive");
    let marketplace = MockSseServer::start(vec![
        json_response(&json!({
            "packages": [
                {
                    "kind": "prompt-pack",
                    "id": "review-pack",
                    "version": "1.0.0",
                    "name": "Review Pack",
                    "description": "Review prompts",
                    "publisher": "neo-test"
                }
            ]
        })),
        json_response(&json!({
            "package": {
                "kind": "prompt-pack",
                "id": "review-pack",
                "version": "1.0.0",
                "manifest_url": "/api/v1/marketplace/packages/prompt-pack/review-pack/1.0.0/.neo-package.toml",
                "archive_url": "/api/v1/marketplace/packages/prompt-pack/review-pack/1.0.0/review-pack-1.0.0.tar"
            }
        })),
        text_response("application/toml", &manifest),
        binary_response("application/x-tar", &archive),
    ]);

    let mut search = neo();
    search
        .current_dir(temp.path())
        .env("NEO_MARKETPLACE_URL", &marketplace.url)
        .args(["prompts", "search", "review"]);
    let searched = run(search);
    assert!(searched.contains("review-pack"));

    let mut install = neo();
    install
        .current_dir(temp.path())
        .env("NEO_MARKETPLACE_URL", &marketplace.url)
        .args([
            "prompts",
            "install",
            "review-pack@1.0.0",
            "--from",
            "marketplace",
        ]);
    let installed = run(install);
    assert!(installed.contains("review-pack installed 1.0.0"));

    let mut list = neo();
    list.current_dir(temp.path()).args(["prompts", "list"]);
    let listed = run(list);
    assert!(listed.contains("review"));
    assert!(listed.contains("Review code"));

    let mut preview = neo();
    preview
        .current_dir(temp.path())
        .args(["prompts", "preview", "review"]);
    let previewed = run(preview);
    assert!(previewed.contains("Review $ARGUMENTS"));
    assert!(
        temp.path()
            .join(".neo/prompts/review-pack/review.md")
            .exists()
    );
}

#[test]
fn prompt_packages_update_uninstall_and_metadata_from_marketplace() {
    let temp = TempDir::new().expect("tempdir");
    let publisher_key = SigningKey::from_bytes(&[23_u8; 32]);
    trust_test_publisher(temp.path(), &publisher_key);
    let marketplace = prompt_update_marketplace(&publisher_key);

    let mut install = neo();
    install
        .current_dir(temp.path())
        .env("NEO_MARKETPLACE_URL", &marketplace.url)
        .args([
            "prompts",
            "install",
            "review-pack@1.0.0",
            "--from",
            "marketplace",
        ]);
    run(install);

    let mut update = neo();
    update
        .current_dir(temp.path())
        .env("NEO_MARKETPLACE_URL", &marketplace.url)
        .args(["prompts", "update", "review-pack"]);
    let updated = run(update);
    assert!(updated.contains("review-pack updated 1.1.0"));
    assert!(updated.contains("trusted"));

    let mut list = neo();
    list.current_dir(temp.path()).args(["prompts", "list"]);
    let listed = run(list);
    assert!(listed.contains("review-pack"));
    assert!(listed.contains("1.1.0"));
    assert!(listed.contains("marketplace"));
    assert!(listed.contains("neo-test"));
    assert!(listed.contains("trusted"));

    let mut preview = neo();
    preview
        .current_dir(temp.path())
        .args(["prompts", "preview", "review"]);
    let previewed = run(preview);
    assert!(previewed.contains("source: marketplace"));
    assert!(previewed.contains("publisher: neo-test"));
    assert!(previewed.contains("trust: trusted"));
    assert!(previewed.contains("Review v2"));

    let mut uninstall = neo();
    uninstall
        .current_dir(temp.path())
        .args(["prompts", "uninstall", "review-pack"]);
    let uninstalled = run(uninstall);
    assert!(uninstalled.contains("review-pack uninstalled"));
    assert!(!temp.path().join(".neo/prompts/review-pack").exists());
}

fn prompt_update_marketplace(publisher_key: &SigningKey) -> MockSseServer {
    let v1_dir = TempDir::new().expect("package tempdir");
    let v1 = write_trusted_neo_package(
        v1_dir.path(),
        "prompt-pack",
        "review-pack",
        "1.0.0",
        "review.md",
        publisher_key,
        &[PackageFixtureEntry::file(
            "review.md",
            "---\ndescription: Review code\n---\nReview v1\n",
        )],
    );
    let v2_dir = TempDir::new().expect("package tempdir");
    let v2 = write_trusted_neo_package(
        v2_dir.path(),
        "prompt-pack",
        "review-pack",
        "1.1.0",
        "review.md",
        publisher_key,
        &[PackageFixtureEntry::file(
            "review.md",
            "---\ndescription: Review code\n---\nReview v2\n",
        )],
    );
    MockSseServer::start(vec![
        json_response(&json!({
            "package": {
                "kind": "prompt-pack",
                "id": "review-pack",
                "version": "1.0.0",
                "manifest_url": "/p/review-pack/1.0.0/.neo-package.toml",
                "archive_url": "/p/review-pack/1.0.0/review-pack-1.0.0.tar"
            }
        })),
        text_response(
            "application/toml",
            &fs::read_to_string(&v1).expect("manifest v1"),
        ),
        binary_response(
            "application/x-tar",
            &fs::read(v1_dir.path().join("review-pack-1.0.0.tar")).expect("archive v1"),
        ),
        json_response(&json!({
            "package": {
                "kind": "prompt-pack",
                "id": "review-pack",
                "version": "1.1.0",
                "manifest_url": "/p/review-pack/latest/.neo-package.toml",
                "archive_url": "/p/review-pack/latest/review-pack-1.1.0.tar"
            }
        })),
        text_response(
            "application/toml",
            &fs::read_to_string(&v2).expect("manifest v2"),
        ),
        binary_response(
            "application/x-tar",
            &fs::read(v2_dir.path().join("review-pack-1.1.0.tar")).expect("archive v2"),
        ),
    ])
}

#[test]
fn theme_packages_install_list_and_preview_from_marketplace() {
    let temp = TempDir::new().expect("tempdir");
    let package_dir = TempDir::new().expect("package tempdir");
    let publisher_key = SigningKey::from_bytes(&[23_u8; 32]);
    trust_test_publisher(temp.path(), &publisher_key);
    let package = write_trusted_neo_package(
        package_dir.path(),
        "theme",
        "night-owl",
        "2.0.0",
        "night-owl.json",
        &publisher_key,
        &[PackageFixtureEntry::file(
            "night-owl.json",
            r##"{"name":"Night Owl","colors":{"prompt":"#82aaff"}}"##,
        )],
    );
    let manifest = fs::read_to_string(&package).expect("read package manifest");
    let archive = fs::read(package_dir.path().join("night-owl-2.0.0.tar")).expect("read archive");
    let marketplace = MockSseServer::start(vec![
        json_response(&json!({
            "package": {
                "kind": "theme",
                "id": "night-owl",
                "version": "2.0.0",
                "manifest_url": "/api/v1/marketplace/packages/theme/night-owl/2.0.0/.neo-package.toml",
                "archive_url": "/api/v1/marketplace/packages/theme/night-owl/2.0.0/night-owl-2.0.0.tar"
            }
        })),
        text_response("application/toml", &manifest),
        binary_response("application/x-tar", &archive),
    ]);

    let mut install = neo();
    install
        .current_dir(temp.path())
        .env("NEO_MARKETPLACE_URL", &marketplace.url)
        .args([
            "themes",
            "install",
            "night-owl@2.0.0",
            "--from",
            "marketplace",
        ]);
    let installed = run(install);
    assert!(installed.contains("night-owl installed 2.0.0"));

    let mut list = neo();
    list.current_dir(temp.path()).args(["themes", "list"]);
    let listed = run(list);
    assert!(listed.contains("night-owl"));
    assert!(listed.contains("Night Owl"));

    let mut preview = neo();
    preview
        .current_dir(temp.path())
        .args(["themes", "preview", "night-owl"]);
    let previewed = run(preview);
    assert!(previewed.contains("Night Owl"));
    assert!(previewed.contains("#82aaff"));
    assert!(
        temp.path()
            .join(".neo/themes/night-owl/night-owl.json")
            .exists()
    );
}

#[test]
fn theme_packages_update_uninstall_and_metadata_from_marketplace() {
    let temp = TempDir::new().expect("tempdir");
    let publisher_key = SigningKey::from_bytes(&[23_u8; 32]);
    trust_test_publisher(temp.path(), &publisher_key);
    let marketplace = theme_update_marketplace(&publisher_key);

    let mut install = neo();
    install
        .current_dir(temp.path())
        .env("NEO_MARKETPLACE_URL", &marketplace.url)
        .args([
            "themes",
            "install",
            "night-owl@2.0.0",
            "--from",
            "marketplace",
        ]);
    run(install);

    let mut update = neo();
    update
        .current_dir(temp.path())
        .env("NEO_MARKETPLACE_URL", &marketplace.url)
        .args(["themes", "update", "night-owl"]);
    let updated = run(update);
    assert!(updated.contains("night-owl updated 2.1.0"));
    assert!(updated.contains("trusted"));

    let mut list = neo();
    list.current_dir(temp.path()).args(["themes", "list"]);
    let listed = run(list);
    assert!(listed.contains("night-owl"));
    assert!(listed.contains("2.1.0"));
    assert!(listed.contains("marketplace"));
    assert!(listed.contains("neo-test"));
    assert!(listed.contains("trusted"));

    let mut preview = neo();
    preview
        .current_dir(temp.path())
        .args(["themes", "preview", "night-owl"]);
    let previewed = run(preview);
    assert!(previewed.contains("source: marketplace"));
    assert!(previewed.contains("publisher: neo-test"));
    assert!(previewed.contains("trust: trusted"));
    assert!(previewed.contains("#c792ea"));

    let mut uninstall = neo();
    uninstall
        .current_dir(temp.path())
        .args(["themes", "uninstall", "night-owl"]);
    let uninstalled = run(uninstall);
    assert!(uninstalled.contains("night-owl uninstalled"));
    assert!(!temp.path().join(".neo/themes/night-owl").exists());
}

fn theme_update_marketplace(publisher_key: &SigningKey) -> MockSseServer {
    let v1_dir = TempDir::new().expect("package tempdir");
    let v1 = write_trusted_neo_package(
        v1_dir.path(),
        "theme",
        "night-owl",
        "2.0.0",
        "night-owl.json",
        publisher_key,
        &[PackageFixtureEntry::file(
            "night-owl.json",
            r##"{"name":"Night Owl","colors":{"prompt":"#82aaff"}}"##,
        )],
    );
    let v2_dir = TempDir::new().expect("package tempdir");
    let v2 = write_trusted_neo_package(
        v2_dir.path(),
        "theme",
        "night-owl",
        "2.1.0",
        "night-owl.json",
        publisher_key,
        &[PackageFixtureEntry::file(
            "night-owl.json",
            r##"{"name":"Night Owl","colors":{"prompt":"#c792ea"}}"##,
        )],
    );
    MockSseServer::start(vec![
        json_response(&json!({
            "package": {
                "kind": "theme",
                "id": "night-owl",
                "version": "2.0.0",
                "manifest_url": "/t/night-owl/2.0.0/.neo-package.toml",
                "archive_url": "/t/night-owl/2.0.0/night-owl-2.0.0.tar"
            }
        })),
        text_response(
            "application/toml",
            &fs::read_to_string(&v1).expect("manifest v1"),
        ),
        binary_response(
            "application/x-tar",
            &fs::read(v1_dir.path().join("night-owl-2.0.0.tar")).expect("archive v1"),
        ),
        json_response(&json!({
            "package": {
                "kind": "theme",
                "id": "night-owl",
                "version": "2.1.0",
                "manifest_url": "/t/night-owl/latest/.neo-package.toml",
                "archive_url": "/t/night-owl/latest/night-owl-2.1.0.tar"
            }
        })),
        text_response(
            "application/toml",
            &fs::read_to_string(&v2).expect("manifest v2"),
        ),
        binary_response(
            "application/x-tar",
            &fs::read(v2_dir.path().join("night-owl-2.1.0.tar")).expect("archive v2"),
        ),
    ])
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

struct PackageFixtureEntry {
    path: PathBuf,
    content: String,
}

impl PackageFixtureEntry {
    fn file(path: impl Into<PathBuf>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
        }
    }
}

fn write_signed_neo_package(
    root: &Path,
    kind: &str,
    id: &str,
    version: &str,
    entry: &str,
    entries: &[PackageFixtureEntry],
) -> PathBuf {
    fs::create_dir_all(root).expect("create package root");
    let archive_name = format!("{id}-{version}.tar");
    let archive_path = root.join(&archive_name);
    write_package_archive(&archive_path, entries);
    let archive_bytes = fs::read(&archive_path).expect("read package archive");
    let digest = hex_sha256(&archive_bytes);
    let signing_key = SigningKey::from_bytes(&[11_u8; 32]);
    let verifying_key = signing_key.verifying_key();
    let signature = signing_key.sign(&archive_bytes);
    let manifest_path = root.join(".neo-package.toml");
    fs::write(
        &manifest_path,
        format!(
            r#"
kind = "{kind}"
id = "{id}"
version = "{version}"
entry = "{entry}"

[archive]
path = "{archive_name}"
sha256 = "{digest}"

[signature]
algorithm = "ed25519"
public_key = "{}"
signature = "{}"
"#,
            STANDARD.encode(verifying_key.to_bytes()),
            STANDARD.encode(signature.to_bytes()),
        ),
    )
    .expect("write package manifest");
    manifest_path
}

fn write_trusted_neo_package(
    root: &Path,
    kind: &str,
    id: &str,
    version: &str,
    entry: &str,
    signing_key: &SigningKey,
    entries: &[PackageFixtureEntry],
) -> PathBuf {
    fs::create_dir_all(root).expect("create package root");
    let archive_name = format!("{id}-{version}.tar");
    let archive_path = root.join(&archive_name);
    write_package_archive(&archive_path, entries);
    let archive_bytes = fs::read(&archive_path).expect("read package archive");
    let digest = hex_sha256(&archive_bytes);
    let verifying_key = signing_key.verifying_key();
    let signature = signing_key.sign(&archive_bytes);
    let manifest_path = root.join(".neo-package.toml");
    fs::write(
        &manifest_path,
        format!(
            r#"
kind = "{kind}"
id = "{id}"
version = "{version}"
entry = "{entry}"

[publisher]
id = "neo-test"
name = "Neo Test"
account_id = "acct_neo_test"

[archive]
path = "{archive_name}"
sha256 = "{digest}"

[signature]
algorithm = "ed25519"
root = "local-root"
public_key_id = "ed25519:2026-a"
public_key = "{}"
signature = "{}"
"#,
            STANDARD.encode(verifying_key.to_bytes()),
            STANDARD.encode(signature.to_bytes()),
        ),
    )
    .expect("write package manifest");
    manifest_path
}

fn trust_test_publisher(project: &Path, signing_key: &SigningKey) {
    let trust_dir = project.join(".neo");
    fs::create_dir_all(&trust_dir).expect("create trust dir");
    fs::write(
        trust_dir.join("package-trust.toml"),
        format!(
            r#"
[publishers.neo-test]
id = "neo-test"
name = "Neo Test"
root = "local-root"
account_id = "acct_neo_test"

[publishers.neo-test.keys."ed25519:2026-a"]
id = "ed25519:2026-a"
public_key = "{}"
revoked = false
"#,
            STANDARD.encode(signing_key.verifying_key().to_bytes()),
        ),
    )
    .expect("write trust store");
}

fn write_package_archive(path: &Path, entries: &[PackageFixtureEntry]) {
    let file = fs::File::create(path).expect("create package archive");
    let mut builder = Builder::new(file);
    for entry in entries {
        let bytes = entry.content.as_bytes();
        let mut header = Header::new_gnu();
        header.set_size(bytes.len().try_into().expect("archive entry length"));
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, &entry.path, bytes)
            .expect("append archive entry");
    }
    builder.finish().expect("finish package archive");
    builder
        .into_inner()
        .expect("package archive writer")
        .flush()
        .expect("flush package archive");
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
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
fn root_list_models_flag_lists_seeded_catalog_without_entering_interactive_mode() {
    let mut command = neo();
    command.arg("--list-models");

    let stdout = run(command);

    assert!(stdout.contains("models:"));
    assert!(stdout.contains("openai/gpt-4.1"));
    assert!(stdout.contains("anthropic/claude-sonnet-4-5"));
    assert!(stdout.contains("providers:"));
    assert!(!stdout.contains("neo | session:"));
}

#[test]
fn root_list_models_flag_filters_models_by_search_pattern() {
    let mut command = neo();
    command.args(["--list-models", "sonnet"]);

    let stdout = run(command);

    assert!(stdout.contains("anthropic/claude-sonnet-4-5"));
    assert!(!stdout.contains("openai/gpt-4.1 ("));
    assert!(stdout.contains("providers:"));
    assert!(!stdout.contains("neo | session:"));
}

#[test]
fn root_list_models_flag_reports_empty_search_results() {
    let mut command = neo();
    command.args(["--list-models", "definitely-not-a-model"]);

    let stdout = run(command);

    assert_eq!(stdout, "no models matching \"definitely-not-a-model\"\n");
}

#[test]
fn root_models_scope_selects_first_matching_model_for_interactive_start() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command
        .current_dir(temp.path())
        .args(["--models", "sonnet"]);

    let stdout = run(command);

    assert!(stdout.contains("model: anthropic/claude-sonnet-4-5"));
    assert!(!stdout.contains("model: openai/gpt-4.1"));
    assert!(!stdout.contains("placeholder"));
    assert!(!stdout.contains("fake"));
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
fn models_list_renders_pi_catalog_display_names() {
    let temp = TempDir::new().expect("tempdir");
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        "model_catalogs = [\".neo/pi-models.json\"]\n",
    )
    .expect("write config");
    fs::write(
        temp.path().join(".neo/pi-models.json"),
        r#"
{
  "providers": {
    "ollama": {
      "name": "Ollama Local",
      "api": "openai-completions",
      "models": [
        {
          "id": "llama3.1:8b",
          "name": "Llama 3.1 8B",
          "input": ["text"],
          "contextWindow": 128000
        }
      ]
    }
  }
}
"#,
    )
    .expect("write pi model catalog");

    let mut models = neo();
    models.current_dir(temp.path()).args(["models", "list"]);
    let stdout = run(models);

    assert!(stdout.contains("ollama/llama3.1:8b"));
    assert!(stdout.contains("Ollama Local / Llama 3.1 8B"));

    let mut filtered = neo();
    filtered
        .current_dir(temp.path())
        .args(["--list-models", "Llama 3.1"]);
    let stdout = run(filtered);

    assert!(stdout.contains("ollama/llama3.1:8b"));
    assert!(stdout.contains("Llama 3.1 8B"));
    assert!(!stdout.contains("openai/gpt-4.1"));
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
fn models_list_pricing_renders_generated_catalog_pricing_and_json() {
    let temp = TempDir::new().expect("tempdir");
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        r#"
default_provider = "openai"
default_model = "gpt-image-1"
model_catalogs = [".neo/generated-models.json"]
"#,
    )
    .expect("write config");
    fs::write(
        temp.path().join(".neo/generated-models.json"),
        r#"
{
  "generated_at": "2026-06-10T00:00:00Z",
  "source": {
    "name": "models.dev",
    "revision": "abc123",
    "url": "https://models.dev/api/models.json"
  },
  "models": [
    {
      "provider": "openai",
      "id": "gpt-image-1",
      "api": "openai-responses",
      "context_window": 128000,
      "capabilities": {
        "streaming": true,
        "tools": false,
        "images": true,
        "reasoning": false,
        "embeddings": false,
        "image_generation": true
      },
      "pricing": {
        "input_per_million_tokens": 5.0,
        "output_per_million_tokens": 40.0,
        "image_generation": {
          "unit": "image",
          "per_unit": 0.04
        }
      }
    }
  ]
}
"#,
    )
    .expect("write generated catalog");

    let mut plain = neo();
    plain
        .current_dir(temp.path())
        .args(["models", "list", "--pricing"]);
    let stdout = run(plain);
    assert!(stdout.contains("openai/gpt-image-1"));
    assert!(stdout.contains("input $5/1M"));
    assert!(stdout.contains("output $40/1M"));
    assert!(stdout.contains("image $0.04/image"));
    assert!(stdout.contains("source models.dev@abc123 generated 2026-06-10T00:00:00Z"));

    let mut json_cmd = neo();
    json_cmd
        .current_dir(temp.path())
        .args(["models", "list", "--pricing", "--json"]);
    let stdout = run(json_cmd);
    let value: Value = serde_json::from_str(&stdout).expect("models json output");
    let image_model = value["models"]
        .as_array()
        .expect("models array")
        .iter()
        .find(|model| model["provider"] == "openai" && model["model"] == "gpt-image-1")
        .expect("generated image model");
    assert_eq!(image_model["capabilities"]["image_generation"], true);
    assert_eq!(image_model["context_window"], 128_000);
    assert_eq!(image_model["pricing"]["input_per_million_tokens"], 5.0);
    assert_eq!(image_model["pricing"]["output_per_million_tokens"], 40.0);
    assert_eq!(
        image_model["pricing"]["image_generation"],
        json!({"unit": "image", "per_unit": 0.04})
    );
    assert_eq!(
        image_model["source"],
        json!({
            "generated_at": "2026-06-10T00:00:00Z",
            "name": "models.dev",
            "revision": "abc123",
            "url": "https://models.dev/api/models.json"
        })
    );
}

#[test]
fn images_generate_writes_base64_provider_response_to_output_file() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![json_response(&json!({
        "created": 1_710_000_000,
        "data": [
            {
                "b64_json": "aGVsbG8taW1hZ2U=",
                "revised_prompt": "draw a quiet terminal"
            }
        ]
    }))]);
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        format!(
            r#"
default_provider = "openai"
default_model = "gpt-image-1"
api_base = "{}"
model_catalogs = [".neo/generated-models.json"]
"#,
            server.url
        ),
    )
    .expect("write config");
    fs::write(
        temp.path().join(".neo/generated-models.json"),
        r#"
{
  "generated_at": "2026-06-10T00:00:00Z",
  "models": [
    {
      "provider": "openai",
      "id": "gpt-image-1",
      "api": "openai-responses",
      "capabilities": {
        "streaming": false,
        "tools": false,
        "images": true,
        "reasoning": false,
        "embeddings": false,
        "image_generation": true
      }
    }
  ]
}
"#,
    )
    .expect("write generated catalog");
    let output_path = temp.path().join("out/generated.png");

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "image-key")
        .args([
            "images",
            "generate",
            "draw terminal",
            "--model",
            "openai/gpt-image-1",
            "--output",
        ])
        .arg(&output_path)
        .args(["--size", "512x512"]);
    let stdout = run(command);

    assert!(stdout.contains(&format!("wrote image to {}", output_path.display())));
    assert_eq!(
        fs::read(&output_path).expect("generated image file"),
        b"hello-image"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/images/generations");
    assert_eq!(
        requests[0].headers.get("authorization").map(String::as_str),
        Some("Bearer image-key")
    );
    assert_eq!(requests[0].body["model"], "gpt-image-1");
    assert_eq!(requests[0].body["prompt"], "draw terminal");
    assert_eq!(requests[0].body["size"], "512x512");
    assert_eq!(requests[0].body["n"], 1);
}

#[test]
fn images_generate_rejects_url_only_response_without_remote_fetch_policy() {
    let temp = TempDir::new().expect("tempdir");
    let server = MockSseServer::start(vec![json_response(&json!({
        "created": 1_710_000_000,
        "data": [
            {
                "url": "https://images.example.test/generated.png",
                "revised_prompt": "draw a quiet terminal"
            }
        ]
    }))]);
    write_image_generation_config(&temp, &server.url, false);
    let output_path = temp.path().join("out/generated.png");

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "image-key")
        .args([
            "images",
            "generate",
            "draw terminal",
            "--model",
            "openai/gpt-image-1",
            "--output",
        ])
        .arg(&output_path);
    let output = command.output().expect("neo command should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("provider returned an image URL"));
    assert!(stderr.contains("tui.fetch_remote_images = true"));
    assert!(!output_path.exists());
}

#[test]
fn images_generate_fetches_url_only_response_when_remote_fetch_policy_enabled() {
    let temp = TempDir::new().expect("tempdir");
    let image_bytes = b"remote-image";
    let image_server = MockSseServer::start(vec![binary_response("image/png", image_bytes)]);
    let provider = MockSseServer::start(vec![json_response(&json!({
        "created": 1_710_000_000,
        "data": [
            {
                "url": format!("{}/generated.png", image_server.url),
                "revised_prompt": "draw a quiet terminal"
            }
        ]
    }))]);
    write_image_generation_config(&temp, &provider.url, true);
    let output_path = temp.path().join("out/generated.png");

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "image-key")
        .args([
            "images",
            "generate",
            "draw terminal",
            "--model",
            "openai/gpt-image-1",
            "--output",
        ])
        .arg(&output_path)
        .args(["--size", "512x512"]);
    let stdout = run(command);

    assert!(stdout.contains(&format!("wrote image to {}", output_path.display())));
    assert_eq!(
        fs::read(&output_path).expect("generated image file"),
        image_bytes
    );
    let image_requests = image_server.requests();
    assert_eq!(image_requests.len(), 1);
    assert_eq!(image_requests[0].method, "GET");
    assert_eq!(image_requests[0].path, "/generated.png");
}

#[test]
fn images_generate_rejects_remote_fetch_with_non_image_content_type() {
    let temp = TempDir::new().expect("tempdir");
    let image_server = MockSseServer::start(vec![text_response("text/plain", "not an image")]);
    let provider = MockSseServer::start(vec![json_response(&json!({
        "created": 1_710_000_000,
        "data": [
            {
                "url": format!("{}/generated.txt", image_server.url),
                "revised_prompt": "draw a quiet terminal"
            }
        ]
    }))]);
    write_image_generation_config(&temp, &provider.url, true);
    let output_path = temp.path().join("out/generated.png");

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "image-key")
        .args([
            "images",
            "generate",
            "draw terminal",
            "--model",
            "openai/gpt-image-1",
            "--output",
        ])
        .arg(&output_path);
    let output = command.output().expect("neo command should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("remote image response content-type text/plain is not allowed"));
    assert!(!output_path.exists());
}

#[test]
fn images_generate_rejects_remote_fetch_with_oversized_content_length() {
    let temp = TempDir::new().expect("tempdir");
    let image_server = MockSseServer::start(vec![oversized_image_response(20 * 1024 * 1024 + 1)]);
    let provider = MockSseServer::start(vec![json_response(&json!({
        "created": 1_710_000_000,
        "data": [
            {
                "url": format!("{}/generated.png", image_server.url),
                "revised_prompt": "draw a quiet terminal"
            }
        ]
    }))]);
    write_image_generation_config(&temp, &provider.url, true);
    let output_path = temp.path().join("out/generated.png");

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "image-key")
        .args([
            "images",
            "generate",
            "draw terminal",
            "--model",
            "openai/gpt-image-1",
            "--output",
        ])
        .arg(&output_path);
    let output = command.output().expect("neo command should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("remote image response is larger than 20971520 bytes"));
    assert!(!output_path.exists());
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
fn mcp_servers_add_enable_disable_remove_persists_project_config_without_printing_secrets() {
    let temp = TempDir::new().expect("tempdir");
    let secret_value = "token-secret-123456";

    let mut add = neo();
    add.current_dir(temp.path()).args([
        "mcp",
        "servers",
        "add",
        "remote-docs",
        "--transport",
        "http",
        "--url",
        "https://mcp.example.test/rpc",
        "--header",
        "authorization=Bearer token-secret-123456",
        "--env",
        "MCP_TOKEN=token-secret-123456",
    ]);
    let add_stdout = run(add);
    assert_eq!(add_stdout, "added MCP server remote-docs\n");
    assert!(!add_stdout.contains(secret_value));

    let config_path = temp.path().join(".neo/config.toml");
    let config_content = fs::read_to_string(&config_path).expect("read config");
    assert!(config_content.contains("id = \"remote-docs\""));
    assert!(config_content.contains("transport = \"http\""));
    assert!(config_content.contains("url = \"https://mcp.example.test/rpc\""));
    assert!(config_content.contains("authorization = \"Bearer token-secret-123456\""));
    assert!(config_content.contains("MCP_TOKEN = \"token-secret-123456\""));

    let mut list = neo();
    list.current_dir(temp.path()).args(["mcp", "list"]);
    let list_stdout = run(list);
    assert!(list_stdout.contains("remote-docs"));
    assert!(list_stdout.contains("https://mcp.example.test/rpc"));
    assert!(!list_stdout.contains(secret_value));
    assert!(!list_stdout.contains("authorization"));
    assert!(!list_stdout.contains("MCP_TOKEN"));

    let mut show = neo();
    show.current_dir(temp.path()).args(["config", "show"]);
    let show_stdout = run(show);
    assert!(show_stdout.contains("remote-docs"));
    assert!(show_stdout.contains("authorization = \"[REDACTED]\""));
    assert!(show_stdout.contains("MCP_TOKEN = \"[REDACTED]\""));
    assert!(!show_stdout.contains(secret_value));

    let mut disable = neo();
    disable
        .current_dir(temp.path())
        .args(["mcp", "servers", "disable", "remote-docs"]);
    assert_eq!(run(disable), "disabled MCP server remote-docs\n");
    let config_content = fs::read_to_string(&config_path).expect("read disabled config");
    assert!(config_content.contains("enabled = false"));

    let mut enable = neo();
    enable
        .current_dir(temp.path())
        .args(["mcp", "servers", "enable", "remote-docs"]);
    assert_eq!(run(enable), "enabled MCP server remote-docs\n");
    let config_content = fs::read_to_string(&config_path).expect("read enabled config");
    assert!(config_content.contains("enabled = true"));

    let mut remove = neo();
    remove
        .current_dir(temp.path())
        .args(["mcp", "servers", "remove", "remote-docs"]);
    assert_eq!(run(remove), "removed MCP server remote-docs\n");
    let config_content = fs::read_to_string(&config_path).expect("read removed config");
    assert!(!config_content.contains("remote-docs"));
    assert!(!config_content.contains(secret_value));
}

#[test]
fn mcp_servers_health_performs_real_enabled_server_probe() {
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
        mcp_json_response(2, &json!({ "tools": [] })),
    ]);
    write_remote_mcp_config(temp.path(), &mcp_server.url);

    let mut health = neo();
    health
        .current_dir(temp.path())
        .args(["mcp", "servers", "health", "remote-docs"]);
    let stdout = run(health);

    assert_eq!(stdout, "remote-docs\thealthy\t0 tools\n");
    assert_eq!(
        mcp_server
            .requests()
            .iter()
            .map(|request| request.body["method"].as_str().expect("method"))
            .collect::<Vec<_>>(),
        vec!["initialize", "tools/list"]
    );
}

#[test]
fn mcp_servers_start_and_stop_stdio_persist_state_and_cleanup_process() {
    let temp = TempDir::new().expect("tempdir");
    let fixture = temp.path().join("mcp-fixture.py");
    let pid_file = temp.path().join("mcp.pid");
    fs::write(&fixture, MCP_STDIO_PID_FIXTURE).expect("write MCP pid fixture");
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
MCP_PID_FILE = "{}"
"#,
            fixture.display(),
            pid_file.display()
        ),
    )
    .expect("write config");

    let mut start = neo();
    start
        .current_dir(temp.path())
        .args(["mcp", "servers", "start", "docs-server"]);
    let stdout = run(start);
    assert!(stdout.contains("started MCP server docs-server"));

    let pid = wait_for_pid_file(&pid_file);
    assert!(
        process_exists(&pid),
        "started MCP server process should live"
    );
    let state_path = temp.path().join(".neo/mcp-state.toml");
    let state = fs::read_to_string(&state_path).expect("read MCP state");
    assert!(state.contains("docs-server"));
    assert!(state.contains(&pid));

    let mut stop = neo();
    stop.current_dir(temp.path())
        .args(["mcp", "servers", "stop", "docs-server"]);
    assert_eq!(run(stop), "stopped MCP server docs-server\n");
    assert!(
        wait_for_process_exit(&pid),
        "stop should terminate MCP server process {pid}"
    );
    let state = fs::read_to_string(&state_path).expect("read stopped MCP state");
    assert!(!state.contains("docs-server"));
}

#[test]
fn mcp_cloud_self_hosted_servers_fail_closed_without_login() {
    let temp = TempDir::new().expect("tempdir");
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        r#"
[[mcp.servers]]
id = "hosted-docs"
enabled = true
transport = "cloud"
url = "cloud://mcp/hosted-docs"
"#,
    )
    .expect("write cloud MCP config");

    let mut health = neo();
    health
        .current_dir(temp.path())
        .args(["mcp", "servers", "health", "hosted-docs"]);
    let output = health.output().expect("neo command should run");

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cloud MCP server hosted-docs requires self-hosted neo-cloud auth"));
    assert!(!stdout.contains("mcp__hosted_docs__"));
    assert!(!stderr.contains("placeholder"));
    assert!(!stderr.contains("fake"));
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
fn print_tool_filters_apply_to_mcp_tools() {
    let temp = TempDir::new().expect("tempdir");
    let provider = MockSseServer::start(vec![openai_response_sse(
        "resp-mcp-filter",
        "filtered tools listed",
    )]);
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
        .args([
            "--tools",
            "read,mcp__docs_server__docs_search",
            "--exclude-tools",
            "read",
            "print",
            "show",
            "tools",
        ]);
    let stdout = run(command);

    assert_eq!(stdout, "filtered tools listed\n");
    let requests = provider.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        model_tool_names(&requests[0].body),
        vec!["mcp__docs_server__docs_search"]
    );
}

#[test]
fn print_pi_style_short_no_builtin_tools_alias_keeps_mcp_tools() {
    let temp = TempDir::new().expect("tempdir");
    let provider = MockSseServer::start(vec![openai_response_sse(
        "resp-no-builtin-tools",
        "mcp only",
    )]);
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
        .args(["-nbt", "print", "show", "tools"]);
    let stdout = run(command);

    assert_eq!(stdout, "mcp only\n");
    let requests = provider.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        model_tool_names(&requests[0].body),
        vec!["mcp__docs_server__docs_search"]
    );
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
fn print_rejects_hosted_cloud_mcp_when_cloud_client_is_unavailable() {
    let temp = TempDir::new().expect("tempdir");
    let provider = MockSseServer::start(vec![]);
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        format!(
            r#"
api_base = "{}"

[[mcp.servers]]
id = "hosted-docs"
enabled = true
transport = "http"
url = "cloud://mcp/hosted-docs"
"#,
            provider.url
        ),
    )
    .expect("write hosted MCP config");

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["print", "show", "hosted", "tools"]);
    let output = command.output().expect("neo command should run");

    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("hosted MCP server hosted-docs requires an available neo-cloud client"),
        "stderr should explain unavailable hosted MCP, got: {stderr}"
    );
    assert!(!stdout.contains("mcp__hosted_docs__"));
    assert!(!stderr.contains("placeholder"));
    assert!(!stderr.contains("fake"));
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

struct TestCloudServer {
    base_url: String,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for TestCloudServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn start_cloud_server(database_path: PathBuf) -> TestCloudServer {
    let store = Store::open(database_path).await.expect("open cloud store");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind cloud server");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let server = CloudServer::new(store);
    let handle = tokio::spawn(async move {
        server.serve(listener).await.expect("serve cloud");
    });
    TestCloudServer { base_url, handle }
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

fn json_response(body: &Value) -> String {
    text_response("application/json", &body.to_string())
}

fn text_response(content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn binary_response(content_type: &str, body: &[u8]) -> String {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    String::from_utf8(response).expect("test package archive response should be utf8-compatible")
}

fn oversized_image_response(content_length: usize) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: image/png\r\ncontent-length: {content_length}\r\nconnection: close\r\n\r\n"
    )
}

fn write_image_generation_config(temp: &TempDir, api_base: &str, fetch_remote_images: bool) {
    fs::create_dir_all(temp.path().join(".neo")).expect("create .neo");
    fs::write(
        temp.path().join(".neo/config.toml"),
        format!(
            r#"
default_provider = "openai"
default_model = "gpt-image-1"
api_base = "{api_base}"
model_catalogs = [".neo/generated-models.json"]

[tui]
fetch_remote_images = {fetch_remote_images}
"#
        ),
    )
    .expect("write config");
    fs::write(
        temp.path().join(".neo/generated-models.json"),
        r#"
{
  "generated_at": "2026-06-10T00:00:00Z",
  "models": [
    {
      "provider": "openai",
      "id": "gpt-image-1",
      "api": "openai-responses",
      "capabilities": {
        "streaming": false,
        "tools": false,
        "images": true,
        "reasoning": false,
        "embeddings": false,
        "image_generation": true
      }
    }
  ]
}
"#,
    )
    .expect("write generated catalog");
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
    let body = if body_bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(body_bytes).expect("json body")
    };

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

fn wait_for_pid_file(path: &Path) -> String {
    for _ in 0..50 {
        if let Ok(pid) = fs::read_to_string(path) {
            let pid = pid.trim();
            if !pid.is_empty() {
                return pid.to_owned();
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("pid file should be written: {}", path.display());
}

fn wait_for_process_exit(pid: &str) -> bool {
    for _ in 0..50 {
        if !process_exists(pid) {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    !process_exists(pid)
}

fn process_exists(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
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

const MCP_STDIO_PID_FIXTURE: &str = r#"
import json
import os
import sys

with open(os.environ["MCP_PID_FILE"], "w", encoding="utf-8") as pid_file:
    pid_file.write(str(os.getpid()))

for line in sys.stdin:
    request = json.loads(line)
    method = request["method"]
    if method == "initialize":
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "pid-fixture", "version": "0.1.0"},
                "capabilities": {"tools": {}},
            },
        }
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {"tools": []},
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
