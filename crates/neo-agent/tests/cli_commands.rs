use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};
use tempfile::TempDir;

const SESSION_A: &str = "session_00000000-0000-4000-8000-000000000201";
const SESSION_B: &str = "session_00000000-0000-4000-8000-000000000202";
const SESSION_CHILD: &str = "session_00000000-0000-4000-8000-000000000203";

fn neo() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_neo"));
    // Each test gets its own unique NEO_HOME so config writes (now under
    // ~/.neo, not the project .neo) don't collide between concurrent tests.
    command.env("NEO_HOME", neo_home_for_test());
    command
}

/// Unique per-test neo home directory. `NEO_HOME` is the single source of truth
/// for config, skills, prompts, themes, sessions — so each test isolates it.
/// `NEO_HOME` IS the neo root (equivalent to ~/.neo), so config lives at
/// `<NEO_HOME>/config.toml`, prompts at `<NEO_HOME>/prompts`, etc.
fn neo_home_for_test() -> PathBuf {
    thread_local! {
        static HOME: std::cell::OnceCell<PathBuf> = const { std::cell::OnceCell::new() };
    }
    HOME.with(|cell| {
        cell.get_or_init(|| {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time after epoch")
                .as_nanos();
            std::env::temp_dir().join(format!("neo-cli-home-{nanos}-{id}"))
        })
        .clone()
    })
}

/// Write the config.toml content into the test's isolated `NEO_HOME`.
fn write_home_config(content: &str) {
    let config_path = neo_home_for_test().join("config.toml");
    fs::create_dir_all(config_path.parent().expect("config parent")).expect("create neo home");
    fs::write(&config_path, content).expect("write home config");
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

/// Find `.jsonl` files in the bucket directory that corresponds to the
/// given project directory. The bucket name is `wd_<slug>_<hash12>`.
fn find_jsonl_files_in_bucket(sessions_root: &Path, project_dir: &Path) -> Vec<PathBuf> {
    // Search all buckets that match the slug prefix and check which one has our
    // session. Since temp dirs have unique basenames, the slug is unique enough.
    let basename = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace");
    let slug: String = basename
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "workspace" } else { slug };

    let prefix = format!("wd_{slug}_");

    let Ok(entries) = fs::read_dir(sessions_root) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with(&prefix) {
                let mut results = Vec::new();
                find_jsonl_files_recursive(&path, &mut results);
                return results;
            }
        }
    }
    Vec::new()
}

fn find_jsonl_files_recursive(dir: &Path, results: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            find_jsonl_files_recursive(&path, results);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            results.push(path);
        }
    }
}

fn sessions_metadata_json(entries: &[(&str, Value)]) -> String {
    let mut sessions = serde_json::Map::new();
    for (id, value) in entries {
        sessions.insert((*id).to_owned(), value.clone());
    }
    json!({ "sessions": sessions }).to_string()
}

fn session_bucket(project_dir: &Path) -> PathBuf {
    let sessions_root = neo_home_for_test().join("sessions");
    neo_agent_core::session::workspace_sessions_dir(&sessions_root, project_dir)
}

fn write_session_transcript(sessions: &Path, session_id: &str, content: &str) {
    let session_dir = sessions.join(session_id);
    fs::create_dir_all(&session_dir).expect("create session dir");
    fs::write(session_dir.join("transcript.jsonl"), content).expect("write transcript");
}

#[test]
fn root_command_reports_interactive_entrypoint_without_placeholders() {
    let command = neo();

    let stdout = run(command);

    assert!(stdout.contains("Welcome to neo"));
    assert!(stdout.contains("openai/gpt-4.1"));
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
fn root_resume_flag_opens_real_local_session_picker() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = session_bucket(temp.path());
    fs::create_dir_all(&sessions).expect("create sessions");
    write_session_transcript(
        &sessions,
        SESSION_A,
        "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello\"}}]}}}}\n",
    );

    let mut command = neo();
    command.current_dir(temp.path()).arg("-r");

    let stdout = run(command);

    assert!(stdout.contains("Sessions"));
    assert!(stdout.contains(SESSION_A));
    assert!(!stdout.contains("placeholder"));
    assert!(!stdout.contains("fake"));
}

#[test]
fn root_resume_flag_rejects_subcommands_instead_of_being_ignored() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command
        .current_dir(temp.path())
        .args(["-r", "sessions", "list"]);

    let output = command.output().expect("neo command should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--resume/-r starts the interactive session picker"));
}

#[test]
fn root_resume_flag_rejects_options_that_conflict_with_the_picker() {
    let temp = TempDir::new().expect("tempdir");
    for args in [vec!["-r", "-c"], vec!["-r", "--no-session"]] {
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
fn run_text_command_without_credentials_fails_without_local_response() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command
        .current_dir(temp.path())
        .env_remove("OPENAI_API_KEY")
        .args(["run", "--output", "text", "hello", "neo"]);

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
fn sessions_list_uses_workspace_session_bucket() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = session_bucket(temp.path());
    fs::create_dir_all(&sessions).expect("create sessions");
    write_session_transcript(&sessions, SESSION_A, "{}\n");

    let mut command = neo();
    command.current_dir(temp.path()).args(["sessions", "list"]);

    let stdout = run(command);

    assert!(stdout.contains(SESSION_A));
}

#[test]
fn sessions_rename_and_fork_surface_flat_metadata_without_tree_command() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = session_bucket(temp.path());
    fs::create_dir_all(&sessions).expect("create sessions");
    write_session_transcript(&sessions, SESSION_A, "{}\n");

    let mut rename = neo();
    rename
        .current_dir(temp.path())
        .args(["sessions", "rename", SESSION_A, "Main thread"]);
    let rename_stdout = run(rename);
    assert!(rename_stdout.contains(&format!("renamed {SESSION_A}")));
    assert!(rename_stdout.contains("Main thread"));

    let mut fork = neo();
    fork.current_dir(temp.path())
        .args(["sessions", "fork", SESSION_A, "--name", "Parser branch"]);
    let fork_stdout = run(fork);
    let fork_prefix = format!("forked {SESSION_A} -> ");
    assert!(fork_stdout.contains(&fork_prefix));
    assert!(fork_stdout.contains("Parser branch"));

    let child_id = fork_stdout
        .lines()
        .find_map(|line| line.strip_prefix(&fork_prefix))
        .and_then(|line| line.split_whitespace().next())
        .expect("fork output includes child id")
        .to_owned();
    assert!(child_id.starts_with("session_"));

    let mut list = neo();
    list.current_dir(temp.path()).args(["sessions", "list"]);
    let list_stdout = run(list);

    assert!(list_stdout.contains(SESSION_A));
    assert!(list_stdout.contains("Main thread"));
    assert!(list_stdout.contains(&child_id));
    assert!(list_stdout.contains("Parser branch"));
    assert!(list_stdout.contains(&format!("parent={SESSION_A}")));

    let mut tree = neo();
    tree.current_dir(temp.path()).args(["sessions", "tree"]);
    let tree_output = tree.output().expect("neo command should run");
    assert!(!tree_output.status.success());
    let stderr = String::from_utf8_lossy(&tree_output.stderr);
    assert!(stderr.contains("unrecognized subcommand"));
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
fn sessions_show_and_resume_read_jsonl_transcripts() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = session_bucket(temp.path());
    fs::create_dir_all(&sessions).expect("create sessions");
    write_session_transcript(
        &sessions,
        SESSION_A,
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"hi back\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    );

    let mut show = neo();
    show.current_dir(temp.path())
        .args(["sessions", "show", SESSION_A]);
    let show_stdout = run(show);
    assert!(show_stdout.contains("\"User\""));
    assert!(show_stdout.contains("hi back"));

    let mut resume = neo();
    resume.current_dir(temp.path()).args(["resume", SESSION_A]);
    let resume_stdout = run(resume);
    assert!(resume_stdout.contains(&format!("session {SESSION_A}")));
    assert!(resume_stdout.contains("user: hello"));
    assert!(resume_stdout.contains("assistant: hi back"));
    assert!(!resume_stdout.contains("placeholder"));
}

#[test]
fn sessions_accept_exact_workspace_bucket_ids() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = session_bucket(temp.path());
    fs::create_dir_all(&sessions).expect("create sessions");
    write_session_transcript(
        &sessions,
        SESSION_A,
        "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"alpha prompt\"}}]}}}}\n",
    );
    write_session_transcript(
        &sessions,
        SESSION_B,
        "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"beta prompt\"}}]}}}}\n",
    );

    let mut show = neo();
    show.current_dir(temp.path())
        .args(["sessions", "show", SESSION_A]);
    let show_stdout = run(show);
    assert!(show_stdout.contains("alpha prompt"));

    let mut resume_path = neo();
    resume_path
        .current_dir(temp.path())
        .args(["resume", SESSION_A]);
    let path_stdout = run(resume_path);
    assert!(path_stdout.contains(&format!("session {SESSION_A}")));
    assert!(path_stdout.contains("user: alpha prompt"));
}

#[test]
fn sessions_reject_invalid_session_ids() {
    fn assert_session_command_rejects(temp: &TempDir, args: &[&str], expected: &str) {
        let output = neo()
            .current_dir(temp.path())
            .args(args)
            .output()
            .expect("neo command should run");
        assert!(
            !output.status.success(),
            "command unexpectedly succeeded: {args:?}"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(expected),
            "expected {expected:?} in stderr for {args:?}, got {stderr}"
        );
    }

    struct Case {
        args: &'static [&'static str],
        expected: &'static str,
        existing_sessions: &'static [&'static str],
    }

    let cases = [
        Case {
            args: &["sessions", "show", "session_"],
            expected: "invalid session id",
            existing_sessions: &[SESSION_A, SESSION_B],
        },
        Case {
            args: &["sessions", "show", "../escape"],
            expected: "invalid session id",
            existing_sessions: &[SESSION_A],
        },
        Case {
            args: &["sessions", "fork", "../escape"],
            expected: "invalid session id",
            existing_sessions: &[SESSION_A],
        },
    ];

    for case in cases {
        let temp = TempDir::new().expect("tempdir");
        let sessions = session_bucket(temp.path());
        fs::create_dir_all(&sessions).expect("create sessions");
        for session_id in case.existing_sessions {
            write_session_transcript(&sessions, session_id, "{}\n");
        }
        fs::write(temp.path().join("escape.jsonl"), "{}\n").expect("write escape target");
        assert_session_command_rejects(&temp, case.args, case.expected);
    }
}

#[test]
fn sessions_compact_stores_algorithmic_summary_and_resume_replays_kept_context() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = session_bucket(temp.path());
    fs::create_dir_all(&sessions).expect("create sessions");
    write_session_transcript(
        &sessions,
        SESSION_A,
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"first task\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"first answer\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"latest task\"}}]}}}}\n"
        ),
    );

    let mut compact = neo();
    compact
        .current_dir(temp.path())
        .args(["sessions", "compact", SESSION_A, "--keep-recent", "1"]);
    let compact_stdout = run(compact);

    assert!(compact_stdout.contains(&format!("compacted {SESSION_A}")));
    assert!(compact_stdout.contains("kept 1"));
    assert!(compact_stdout.contains("Algorithmic transcript summary"));
    assert!(!compact_stdout.contains("fake"));

    // Verify compaction through the public session reader.
    let mut show = neo();
    show.current_dir(temp.path())
        .args(["sessions", "show", SESSION_A]);
    let show_stdout = run(show);
    assert!(show_stdout.contains("CompactionApplied"));
    assert!(show_stdout.contains("Algorithmic transcript summary"));

    let mut resume = neo();
    resume.current_dir(temp.path()).args(["resume", SESSION_A]);
    let resume_stdout = run(resume);
    assert!(resume_stdout.contains(&format!("session {SESSION_A}")));
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
    let sessions = session_bucket(temp.path());
    fs::create_dir_all(&sessions).expect("create sessions");
    write_session_transcript(
        &sessions,
        SESSION_A,
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello <neo>\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"use **bold**\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    );

    let mut export = neo();
    export
        .current_dir(temp.path())
        .args(["sessions", "export-html", SESSION_A]);
    let html = run(export);

    assert!(html.contains("<!doctype html>"));
    assert!(html.contains("hello &lt;neo&gt;"));
    assert!(html.contains("<strong>bold</strong>"));
    assert!(!html.contains("fake"));
}

#[test]
fn sessions_export_json_returns_sanitized_replayed_session_artifact() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = session_bucket(temp.path());
    fs::create_dir_all(&sessions).expect("create sessions");
    write_session_transcript(
        &sessions,
        SESSION_A,
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello json export\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"portable local reply\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    );
    write_session_transcript(&sessions, SESSION_CHILD, "{}\n");
    fs::write(
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
                    "parent_id": SESSION_A
                }),
            ),
        ]),
    )
    .expect("write metadata");

    let mut export = neo();
    export
        .current_dir(temp.path())
        .args(["sessions", "export-json", SESSION_A]);
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
    assert_eq!(artifact["metadata"]["id"], SESSION_A);
    assert_eq!(artifact["metadata"]["name"], "Main thread");
    assert_eq!(artifact["metadata"]["summary"], "Local branch summary");
    assert!(artifact["metadata"]["parent_id"].is_null());
    assert_eq!(artifact["metadata"]["children"], json!([SESSION_CHILD]));
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

    let state = fs::read_to_string(neo_home_for_test().join("extensions-state.toml"))
        .expect("read lifecycle state");
    assert!(state.contains("[extensions.echo]"));
    assert!(state.contains("enabled = false"));
}

#[test]
fn extensions_defaults_use_project_config_directory_when_invoked_from_another_cwd() {
    let project = TempDir::new().expect("project tempdir");
    let caller = TempDir::new().expect("caller tempdir");
    write_home_config("");
    let source = project.path().join("source");
    write_extension_manifest(&source, "echo", "Echo", "0.1.0");

    let mut install = neo();
    install
        .current_dir(caller.path())
        .args(["extensions", "install"])
        .arg(&source);
    let installed = run(install);
    assert!(installed.contains("echo installed"));

    let mut disable = neo();
    disable
        .current_dir(caller.path())
        .args(["extensions", "disable", "echo"]);
    let disabled = run(disable);
    assert!(disabled.contains("echo disabled"));

    write_extension_manifest(&source, "echo", "Echo", "0.2.0");

    let mut update = neo();
    update
        .current_dir(caller.path())
        .args(["extensions", "update", "echo"]);
    let updated = run(update);
    assert!(updated.contains("echo updated"));
    assert!(updated.contains("0.2.0"));

    let mut list = neo();
    list.current_dir(caller.path()).args(["extensions", "list"]);
    let listed = run(list);
    assert!(listed.contains("echo"));
    assert!(listed.contains("0.2.0"));
    assert!(listed.contains("disabled"));
    assert!(listed.contains(source.to_string_lossy().as_ref()));

    let project_state = fs::read_to_string(neo_home_for_test().join("extensions-state.toml"))
        .expect("read lifecycle state");
    assert!(project_state.contains("[extensions.echo]"));
    assert!(project_state.contains("enabled = false"));
    let project_registry = fs::read_to_string(neo_home_for_test().join("extensions-sources.toml"))
        .expect("read source registry");
    assert!(project_registry.contains("[extensions.echo"));
    assert!(project_registry.contains(source.to_string_lossy().as_ref()));
    assert!(
        neo_home_for_test()
            .join("extensions/echo/neo-extension.toml")
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

    let registry = fs::read_to_string(neo_home_for_test().join("extensions-sources.toml"))
        .expect("read extension source registry");
    assert!(!registry.contains("[extensions.echo"));
    assert!(!registry.contains(source.to_string_lossy().as_ref()));
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

    let state = fs::read_to_string(neo_home_for_test().join("extensions-state.toml"))
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
fn removed_remote_cli_surfaces_fail_parser() {
    let temp = TempDir::new().expect("tempdir");
    for args in [
        vec!["extensions", "search", "echo"],
        vec!["extensions", "install", "echo", "--from", "marketplace"],
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

fn canonical_project_dir(temp: &TempDir) -> PathBuf {
    temp.path().canonicalize().expect("canonicalize temp dir")
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

#[test]
fn config_model_scope_selects_first_matching_model_for_interactive_start() {
    let temp = TempDir::new().expect("tempdir");
    write_home_config(
        r#"
model_scope = ["sonnet"]
"#,
    );

    let mut command = neo();
    command.current_dir(temp.path());

    let stdout = run(command);

    assert!(stdout.contains("anthropic/claude-sonnet-4-5"));
    assert!(!stdout.contains("openai/gpt-4.1"));
    assert!(!stdout.contains("placeholder"));
    assert!(!stdout.contains("fake"));
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
    write_home_config(
        r#"
[[mcp.servers]]
id = "filesystem"
enabled = false
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]

[mcp.servers.env]
RUST_LOG = "info"
"#,
    );

    let mut mcp = neo();
    mcp.current_dir(temp.path()).args(["mcp", "list"]);
    let stdout = run(mcp);

    assert!(stdout.contains("[1]<filesystem>(studio)"));
}

#[test]
fn mcp_list_displays_remote_servers() {
    let temp = TempDir::new().expect("tempdir");
    write_home_config(
        r#"
[[mcp.servers]]
id = "remote-docs"
enabled = false
transport = "http"
url = "https://mcp.example.test/rpc"

[[mcp.servers]]
id = "stream-docs"
enabled = true
transport = "sse"
url = "https://mcp.example.test/sse"
"#,
    );

    let mut mcp = neo();
    mcp.current_dir(temp.path()).args(["mcp", "list"]);
    let stdout = run(mcp);

    assert!(stdout.contains("[1]<remote-docs>(remote-http)"));
    assert!(stdout.contains("{}"));
    assert!(stdout.contains("[2]<stream-docs>(remote-sse)"));
}

#[test]
fn mcp_add_enable_disable_del_persists_project_config_without_printing_secrets() {
    let temp = TempDir::new().expect("tempdir");
    let secret_value = "token-secret-123456";

    let mut add = neo();
    add.current_dir(temp.path()).args([
        "mcp",
        "add",
        "remote-docs",
        "-t",
        "remote-http",
        "--url",
        "https://mcp.example.test/rpc",
        "--header",
        "authorization=Bearer token-secret-123456",
        "--env",
        "MCP_TOKEN=token-secret-123456",
    ]);
    let add_stdout = run(add);
    assert!(add_stdout.contains("added MCP server remote-docs"));
    assert!(!add_stdout.contains(secret_value));

    let config_path = neo_home_for_test().join("config.toml");
    let config_content = fs::read_to_string(&config_path).expect("read config");
    assert!(config_content.contains("id = \"remote-docs\""));
    assert!(config_content.contains("transport = \"http\""));
    assert!(config_content.contains("url = \"https://mcp.example.test/rpc\""));
    assert!(config_content.contains("authorization = \"Bearer token-secret-123456\""));
    assert!(config_content.contains("MCP_TOKEN = \"token-secret-123456\""));

    let mut list = neo();
    list.current_dir(temp.path()).args(["mcp", "list"]);
    let list_stdout = run(list);
    assert!(list_stdout.contains("[1]<remote-docs>(remote-http)"));
    assert!(!list_stdout.contains(secret_value));
    assert!(!list_stdout.contains("authorization"));
    assert!(!list_stdout.contains("MCP_TOKEN"));

    let mut disable = neo();
    disable
        .current_dir(temp.path())
        .args(["mcp", "disable", "remote-docs"]);
    assert_eq!(run(disable), "disabled MCP server remote-docs\n");
    let config_content = fs::read_to_string(&config_path).expect("read disabled config");
    assert!(config_content.contains("enabled = false"));

    let mut enable = neo();
    enable
        .current_dir(temp.path())
        .args(["mcp", "enable", "remote-docs"]);
    assert_eq!(run(enable), "enabled MCP server remote-docs\n");
    let config_content = fs::read_to_string(&config_path).expect("read enabled config");
    assert!(config_content.contains("enabled = true"));

    let mut remove = neo();
    remove
        .current_dir(temp.path())
        .args(["mcp", "del", "remote-docs"]);
    assert_eq!(run(remove), "removed MCP server remote-docs\n");
    let config_content = fs::read_to_string(&config_path).expect("read removed config");
    assert!(!config_content.contains("remote-docs"));
    assert!(!config_content.contains(secret_value));
}

#[test]
fn mcp_add_remote_http_probes_and_reports_success() {
    let temp = TempDir::new().expect("tempdir");
    let mcp_server = MockSseServer::start(vec![
        mcp_json_response(
            0,
            &json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "remote-docs", "version": "0.1.0"},
                "capabilities": {"tools": {}}
            }),
        ),
        mcp_http_accept(),
        mcp_json_response(
            1,
            &json!({
                "tools": [
                    {
                        "name": "docs-search",
                        "description": "Search remote docs",
                        "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}
                    }
                ]
            }),
        ),
    ]);

    let mut add = neo();
    add.current_dir(temp.path()).args([
        "mcp",
        "add",
        "remote-docs",
        "-t",
        "remote-http",
        "--url",
        &mcp_server.url,
    ]);
    let stdout = run(add);
    assert!(stdout.contains("added MCP server remote-docs"));
    assert!(stdout.contains("remote-docs successfully connected!"));

    let config_path = neo_home_for_test().join("config.toml");
    let config_content = fs::read_to_string(&config_path).expect("read config");
    assert!(config_content.contains("transport = \"http\""));
    assert!(config_content.contains(&format!("url = \"{}\"", mcp_server.url)));
}

#[test]
fn mcp_add_remote_http_reports_failure_without_abort() {
    let temp = TempDir::new().expect("tempdir");

    let mut add = neo();
    add.current_dir(temp.path()).args([
        "mcp",
        "add",
        "bad-remote",
        "-t",
        "remote-http",
        "--url",
        "http://127.0.0.1:1/rpc",
        "--startup-timeout-ms",
        "200",
    ]);
    let stdout = run(add);
    assert!(stdout.contains("added MCP server bad-remote"));
    assert!(stdout.contains("bad-remote connect failed"));

    let config_path = neo_home_for_test().join("config.toml");
    let config_content = fs::read_to_string(&config_path).expect("read config");
    assert!(config_content.contains("id = \"bad-remote\""));
}

#[test]
fn mcp_add_with_disable_creates_enabled_false() {
    let temp = TempDir::new().expect("tempdir");

    let mut add = neo();
    add.current_dir(temp.path()).args([
        "mcp",
        "add",
        "offline-server",
        "-t",
        "remote-http",
        "--url",
        "http://127.0.0.1:1/rpc",
        "--disable",
    ]);
    let stdout = run(add);
    assert!(stdout.contains("added MCP server offline-server"));
    assert!(stdout.contains("offline-server added (disabled)"));

    let config_path = neo_home_for_test().join("config.toml");
    let config_content = fs::read_to_string(&config_path).expect("read config");
    assert!(config_content.contains("enabled = false"));
}

#[test]
fn mcp_add_studio_parses_command_string_and_cwd() {
    let temp = TempDir::new().expect("tempdir");

    let mut add = neo();
    add.current_dir(temp.path()).args([
        "mcp",
        "add",
        "filesystem",
        "-t",
        "studio",
        "-C",
        "npx -y @modelcontextprotocol/server-filesystem .",
        "--cwd",
        ".",
        "--disable",
    ]);
    let stdout = run(add);
    assert!(stdout.contains("added MCP server filesystem"));
    assert!(stdout.contains("filesystem added (disabled)"));

    let config_path = neo_home_for_test().join("config.toml");
    let config_content = fs::read_to_string(&config_path).expect("read config");
    assert!(config_content.contains("enabled = false"));
    assert!(config_content.contains("command = \"npx\""));
    assert!(config_content.contains("args = ["));
    assert!(config_content.contains("\"-y\""));
    assert!(config_content.contains("\"@modelcontextprotocol/server-filesystem\""));
    assert!(config_content.contains("\".\""));
    assert!(config_content.contains("cwd = \".\""));
}

#[test]
fn mcp_add_with_enabled_tools_filters_tool_list() {
    let temp = TempDir::new().expect("tempdir");
    let mcp_server = MockSseServer::start(vec![
        // first connection for `add` probe
        mcp_json_response(
            0,
            &json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "remote-docs", "version": "0.1.0"},
                "capabilities": {"tools": {}}
            }),
        ),
        mcp_http_accept(),
        mcp_json_response(
            1,
            &json!({
                "tools": [
                    {
                        "name": "docs-search",
                        "description": "Search docs",
                        "inputSchema": {"type": "object"}
                    },
                    {
                        "name": "docs-read",
                        "description": "Read docs",
                        "inputSchema": {"type": "object"}
                    }
                ]
            }),
        ),
        // second connection for `list`
        mcp_json_response(
            0,
            &json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "remote-docs", "version": "0.1.0"},
                "capabilities": {"tools": {}}
            }),
        ),
        mcp_http_accept(),
        mcp_json_response(
            1,
            &json!({
                "tools": [
                    {
                        "name": "docs-search",
                        "description": "Search docs",
                        "inputSchema": {"type": "object"}
                    },
                    {
                        "name": "docs-read",
                        "description": "Read docs",
                        "inputSchema": {"type": "object"}
                    }
                ]
            }),
        ),
    ]);

    let mut add = neo();
    add.current_dir(temp.path()).args([
        "mcp",
        "add",
        "remote-docs",
        "-t",
        "remote-http",
        "--url",
        &mcp_server.url,
        "--enabled-tools",
        "docs-search",
    ]);
    let stdout = run(add);
    assert!(stdout.contains("remote-docs successfully connected!"));

    let mut list = neo();
    list.current_dir(temp.path()).args(["mcp", "list"]);
    let list_stdout = run(list);
    assert!(list_stdout.contains("docs-search"));
    assert!(!list_stdout.contains("docs-read"));
}

#[test]
fn run_text_registers_enabled_stdio_mcp_tools_from_project_config() {
    let temp = TempDir::new().expect("tempdir");
    let provider = MockSseServer::start(vec![openai_response_sse("resp-mcp", "mcp tools listed")]);
    let mcp_fixture = temp.path().join("mcp-fixture.py");
    fs::write(&mcp_fixture, MCP_STDIO_FIXTURE).expect("write MCP fixture");
    write_home_config(&format!(
        r#"{}

[[mcp.servers]]
id = "docs-server"
enabled = true
transport = "stdio"
command = "python3"
args = ["-u", "{}"]
"#,
        mock_responses_config(&provider.url),
        mcp_fixture.display()
    ));

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "show", "tools"]);
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
fn run_text_registers_enabled_http_mcp_tools_from_project_config() {
    let temp = TempDir::new().expect("tempdir");
    let provider = MockSseServer::start(vec![openai_response_sse(
        "resp-mcp-http",
        "remote mcp tools listed",
    )]);
    let mcp_server = MockSseServer::start(vec![
        mcp_json_response(
            0,
            &json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "remote-docs", "version": "0.1.0"},
                "capabilities": {"tools": {}}
            }),
        ),
        mcp_http_accept(),
        mcp_json_response(
            1,
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
        mcp_json_response(
            2,
            &json!({"resources": []}),
        ),
    ]);
    write_home_config(&format!(
        r#"{}

[[mcp.servers]]
id = "remote-docs"
enabled = true
transport = "http"
url = "{}"

[mcp.servers.headers]
"x-neo-test" = "remote-mcp"
"#,
        mock_responses_config(&provider.url),
        mcp_server.url
    ));

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "show", "remote", "tools"]);
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
    let methods: Vec<_> = mcp_requests
        .iter()
        .map(|request| {
            request.body["method"]
                .as_str()
                .unwrap_or("(none)")
        })
        .collect();
    assert!(
        methods.contains(&"initialize"),
        "expected initialize request, got {methods:?}"
    );
    assert!(
        methods.contains(&"tools/list"),
        "expected tools/list request, got {methods:?}"
    );
    assert!(
        mcp_requests.iter().all(|request| {
            request.headers.get("x-neo-test").map(String::as_str) == Some("remote-mcp")
        }),
        "custom header missing from some requests: {mcp_requests:?}"
    );
}

#[test]
fn run_text_rejects_remote_mcp_server_missing_url() {
    let temp = TempDir::new().expect("tempdir");
    let provider = MockSseServer::start(vec![]);
    write_home_config(&format!(
        r#"{}

[[mcp.servers]]
id = "remote-docs"
enabled = true
transport = "http"
"#,
        mock_responses_config(&provider.url)
    ));

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "show", "remote", "tools"]);
    let output = command.output().expect("neo command should run");

    // The MCP server with a missing URL means no MCP tools are registered,
    // and the mock provider is unreachable so the model call fails. The
    // command should not succeed either way.
    assert!(!output.status.success());
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

fn mock_responses_config(base_url: &str) -> String {
    format!(
        r#"
default_provider = "mock"
default_model = "gpt-4.1"

[providers.mock]
type = "openai-responses"
base_url = "{base_url}"
api_key_env = "OPENAI_API_KEY"

[models."mock/gpt-4.1"]
provider = "mock"
model = "gpt-4.1"
capabilities = ["streaming", "tools"]
"#
    )
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

/// HTTP 202 response consumed by rmcp for the `notifications/initialized` POST.
/// rmcp's `expect_accepted_or_json` accepts either a 202 or any JSON body.
fn mcp_http_accept() -> String {
    "HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_owned()
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
