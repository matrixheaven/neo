use super::http_server::*;

use std::fs;

use tempfile::TempDir;

#[test]
fn root_command_reports_interactive_entrypoint_without_placeholders() {
    let command = neo();

    let stdout = run(command);

    assert!(stdout.contains("Welcome to neo"));
    assert!(stdout.contains("No configured providers/models"));
    assert!(stdout.contains("ctx --/1m"));
    assert!(!stdout.contains("enter send"));
    assert!(!stdout.contains("placeholder"));
    assert!(!stdout.contains("fake"));
    assert!(!stdout.contains("commands: print, run"));
}

#[test]
fn root_command_renders_configured_tui_session_state() {
    let temp = TempDir::new().expect("tempdir");
    write_home_config(
        r#"
default_provider = "anthropic"
default_model = "claude-sonnet-4-5"
"#,
    );

    let mut command = neo();
    command.current_dir(temp.path());

    let stdout = run(command);

    assert!(stdout.contains("Welcome to neo"));
    assert!(stdout.contains("anthropic/claude-sonnet-4-5"));
    assert!(stdout.contains('>'));
    assert!(!stdout.contains("commands:"));
}

#[test]
fn root_verbose_flag_renders_real_startup_details() {
    let temp = TempDir::new().expect("tempdir");
    write_home_config(
        r#"
model_scope = ["sonnet"]
"#,
    );

    let mut command = neo();
    command.current_dir(temp.path()).arg("--verbose");

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
    assert!(stdout.contains("model scope: sonnet"));
    assert!(!stdout.contains("placeholder"));
    assert!(!stdout.contains("fake"));
}

#[test]
fn project_theme_auto_discovery_loads_theme_for_verbose_startup() {
    let temp = TempDir::new().expect("tempdir");
    let themes = neo_home_for_test().join("themes");
    fs::create_dir_all(&themes).expect("create themes");
    fs::write(
        themes.join("solarized-neo.json"),
        r##"
{
  "name": "Solarized Neo",
  "colors": {
    "text_primary": "#268bd2",
    "prompt": "yellow",
    "user_message": "magenta",
    "brand": "blue",
    "text_muted": "gray"
  }
}
"##,
    )
    .expect("write theme");

    let mut command = neo();
    command.current_dir(temp.path()).arg("--verbose");

    let stdout = run(command);

    assert!(stdout.contains("theme: Solarized Neo"));
}

#[test]
fn explicit_theme_in_config_wins_over_sorted_discovery() {
    let temp = TempDir::new().expect("tempdir");
    let themes = neo_home_for_test().join("themes");
    fs::create_dir_all(&themes).expect("create themes");
    fs::write(
        themes.join("zz-first.json"),
        r##"{"name": "Sorted First", "colors": {"brand": "blue"}}"##,
    )
    .expect("write sorted-first theme");
    fs::write(
        themes.join("configured.json"),
        r##"{"name": "Configured", "colors": {"brand": "red"}}"##,
    )
    .expect("write configured theme");
    write_home_config("[tui]\ntheme = \"configured.json\"\n");

    let mut command = neo();
    command.current_dir(temp.path()).arg("--verbose");

    let stdout = run(command);

    assert!(
        stdout.contains("theme: Configured"),
        "explicit theme id must select the configured theme, got:\n{stdout}"
    );
    assert!(!stdout.contains("theme: Sorted First"));
}

#[test]
fn missing_explicit_theme_falls_back_to_default_with_diagnostic() {
    let temp = TempDir::new().expect("tempdir");
    let themes = neo_home_for_test().join("themes");
    fs::create_dir_all(&themes).expect("create themes");
    fs::write(
        themes.join("aaa-sorted-first.json"),
        r##"{"name": "Sorted First", "colors": {"brand": "blue"}}"##,
    )
    .expect("write sorted-first theme");
    write_home_config("[tui]\ntheme = \"missing-theme.json\"\n");

    let mut command = neo();
    command.current_dir(temp.path()).arg("--verbose");

    let stdout = run(command);

    assert!(
        stdout.contains("theme: default"),
        "a missing explicit id must use the built-in default, got:\n{stdout}"
    );
    assert!(
        stdout.contains("missing-theme.json"),
        "the startup diagnostic must name the unusable id, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("theme: Sorted First"),
        "a missing explicit id must never fall back to another JSON file, got:\n{stdout}"
    );
}

#[test]
fn invalid_theme_in_config_reports_diagnostic_without_rewriting_config() {
    let temp = TempDir::new().expect("tempdir");
    let themes = neo_home_for_test().join("themes");
    fs::create_dir_all(&themes).expect("create themes");
    fs::write(
        themes.join("aaa-sorted-first.json"),
        r##"{"name": "Sorted First", "colors": {"brand": "blue"}}"##,
    )
    .expect("write sorted-first theme");
    write_home_config("[tui]\ntheme = \"../escape.json\"\n");

    let mut command = neo();
    command.current_dir(temp.path()).arg("--verbose");

    let stdout = run(command);

    assert!(
        stdout.contains("theme: default"),
        "an invalid explicit id must use the built-in default, got:\n{stdout}"
    );
    assert!(
        stdout.contains("../escape.json"),
        "the startup diagnostic must name the invalid id, got:\n{stdout}"
    );
    assert!(!stdout.contains("theme: Sorted First"));

    let config_path = neo_home_for_test().join("config.toml");
    let config_content = fs::read_to_string(&config_path).expect("read config");
    assert!(
        config_content.contains("theme = \"../escape.json\""),
        "startup must never auto-rewrite the persisted config"
    );
}

#[test]
fn theme_startup_default_id_persists_and_startup_is_read_only() {
    let temp = TempDir::new().expect("tempdir");
    let themes = neo_home_for_test().join("themes");
    fs::create_dir_all(&themes).expect("create themes");
    fs::write(
        themes.join("zz-first.json"),
        r##"{"name": "Sorted First", "colors": {"brand": "blue"}}"##,
    )
    .expect("write sorted-first theme");
    fs::write(
        themes.join("configured.json"),
        r##"{"name": "Configured", "colors": {"brand": "red"}}"##,
    )
    .expect("write configured theme");

    // The set-startup-default action persists exactly one logical id into the
    // TUI section; nothing else in the config changes.
    write_home_config("[tui]\ntheme = \"configured.json\"\n");
    let config_bytes = fs::read_to_string(neo_home_for_test().join("config.toml")).expect("config");

    let mut command = neo();
    command.current_dir(temp.path()).arg("--verbose");

    let stdout = run(command);

    assert!(
        stdout.contains("theme: Configured"),
        "the persisted startup id must be honored, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("theme: Sorted First"),
        "the persisted startup id must win over sorted discovery, got:\n{stdout}"
    );
    assert_eq!(
        fs::read_to_string(neo_home_for_test().join("config.toml")).expect("config"),
        config_bytes,
        "a startup read must never rewrite the persisted config"
    );
}

#[test]
fn run_command_without_credentials_fails_without_local_response() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command
        .current_dir(temp.path())
        .env_remove("OPENAI_API_KEY")
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

    // Also verify --output text mode behaves identically.
    let mut text_command = neo();
    text_command
        .current_dir(temp.path())
        .env_remove("OPENAI_API_KEY")
        .args(["run", "--output", "text", "hello", "neo"]);

    let text_output = text_command.output().expect("neo command should run");
    assert!(!text_output.status.success());
    let text_stderr = String::from_utf8_lossy(&text_output.stderr);
    assert!(text_stderr.contains("OPENAI_API_KEY"));
}

#[test]
fn run_text_with_missing_credentials_does_not_persist_assistant_response() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command
        .current_dir(temp.path())
        .env_remove("OPENAI_API_KEY")
        .args(["run", "--output", "text", "hello", "neo"]);

    let output = command.output().expect("neo command should run");

    assert!(!output.status.success());
    // Session files are stored under the isolated home in a workspace-scoped
    // bucket directory. Find them by searching for the project's bucket.
    let home_sessions = neo_home_for_test().join("sessions");
    let sessions: Vec<_> = find_jsonl_files_in_bucket(&home_sessions, temp.path());
    assert_eq!(sessions.len(), 1);
    let path = &sessions[0];
    assert_eq!(path.extension().and_then(|ext| ext.to_str()), Some("jsonl"));
    let content = fs::read_to_string(path).expect("read jsonl session");
    assert!(content.contains("\"User\""));
    assert!(!content.contains("\"Assistant\""));
    assert!(!content.contains("fake response"));
}

#[test]
fn removed_remote_cli_surfaces_fail_parser() {
    let temp = TempDir::new().expect("tempdir");
    for args in [
        vec!["trust", "publishers", "list"],
        vec!["sessions", "sync", "status"],
        vec!["models", "list", "--pricing"],
    ] {
        let output = neo()
            .current_dir(temp.path())
            .args(args)
            .output()
            .expect("neo command should run");
        assert!(!output.status.success());
    }
}

#[test]
fn extensions_subcommand_is_unknown() {
    let output = neo()
        .args(["extensions", "list"])
        .output()
        .expect("neo command should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unrecognized subcommand"));
    assert!(stderr.contains("extensions"));
}

#[test]
fn trust_status_reports_unknown_when_inputs_present_without_decision() {
    let temp = TempDir::new().expect("tempdir");
    fs::write(temp.path().join("AGENTS.md"), "rules").expect("write agents file");
    let project_dir = canonical_project_dir(&temp);

    let mut command = neo();
    command.current_dir(temp.path()).args(["trust", "status"]);
    let stdout = run(command);

    assert!(stdout.contains(&format!("Directory: {}", project_dir.display())));
    assert!(stdout.contains("Trust target:"));
    assert!(stdout.contains("Detected inputs:"));
    assert!(stdout.contains("AGENTS.md"));
    assert!(stdout.contains("context file"));
    assert!(stdout.contains("Effective decision: unknown"));
}

#[test]
fn trust_status_reports_trusted_when_no_inputs_exist() {
    let temp = TempDir::new().expect("tempdir");
    let project_dir = canonical_project_dir(&temp);

    let mut command = neo();
    command.current_dir(temp.path()).args(["trust", "status"]);
    let stdout = run(command);

    assert!(stdout.contains(&format!("Directory: {}", project_dir.display())));
    assert!(stdout.contains("Detected inputs: none"));
    assert!(stdout.contains("Effective decision: trusted"));
}

#[test]
fn trust_approve_and_clear_persist_and_remove_decision() {
    let temp = TempDir::new().expect("tempdir");
    fs::write(temp.path().join("AGENTS.md"), "rules").expect("write agents file");
    let project_dir = canonical_project_dir(&temp);

    let mut approve = neo();
    approve.current_dir(temp.path()).args(["trust", "approve"]);
    let approve_stdout = run(approve);
    assert!(approve_stdout.contains("approved trust"));
    assert!(approve_stdout.contains(&project_dir.display().to_string()));

    let mut status_after_approve = neo();
    status_after_approve
        .current_dir(temp.path())
        .args(["trust", "status"]);
    let status_stdout = run(status_after_approve);
    assert!(status_stdout.contains("Effective decision: trusted"));

    let mut clear = neo();
    clear.current_dir(temp.path()).args(["trust", "clear"]);
    let clear_stdout = run(clear);
    assert!(clear_stdout.contains("cleared trust decision"));

    let mut status_after_clear = neo();
    status_after_clear
        .current_dir(temp.path())
        .args(["trust", "status"]);
    let status_after_clear_stdout = run(status_after_clear);
    assert!(status_after_clear_stdout.contains("Effective decision: unknown"));
}

#[test]
fn trust_deny_persists_untrusted_decision() {
    let temp = TempDir::new().expect("tempdir");
    fs::write(temp.path().join("AGENTS.md"), "rules").expect("write agents file");

    let mut deny = neo();
    deny.current_dir(temp.path()).args(["trust", "deny"]);
    let deny_stdout = run(deny);
    assert!(deny_stdout.contains("denied trust"));

    let mut status = neo();
    status.current_dir(temp.path()).args(["trust", "status"]);
    let status_stdout = run(status);
    assert!(status_stdout.contains("Effective decision: untrusted"));
}
