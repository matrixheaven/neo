use super::http_server::*;

use std::fs;
use std::process::Command;

use serde_json::{Value, json};
use tempfile::TempDir;

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
fn sessions_help_does_not_expose_experimental_slim_command() {
    let temp = TempDir::new().expect("tempdir");
    let mut command = neo();
    command
        .current_dir(temp.path())
        .args(["sessions", "--help"]);

    let stdout = run(command);

    assert!(!stdout.contains("slim"), "{stdout}");
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
fn newly_created_session_with_custom_directory_can_resume_from_global_index() {
    let workspace = TempDir::new().expect("workspace tempdir");
    let launch_workspace = TempDir::new().expect("launch workspace tempdir");
    let custom_sessions = workspace.path().join("custom-sessions");
    write_home_config(&format!("sessions_dir = {custom_sessions:?}\n"));

    let output = neo()
        .current_dir(workspace.path())
        .env_remove("OPENAI_API_KEY")
        .args(["run", "--output", "text", "indexed prompt"])
        .output()
        .expect("neo command should run");
    assert!(!output.status.success(), "missing credentials should fail");

    let sessions = find_jsonl_files_in_bucket(&custom_sessions, workspace.path());
    let session_id = sessions
        .into_iter()
        .next()
        .and_then(|path| {
            path.parent()?
                .parent()?
                .parent()?
                .file_name()?
                .to_str()
                .map(str::to_owned)
        })
        .expect("created session id");

    let mut resume = neo();
    resume
        .current_dir(launch_workspace.path())
        .args(["resume", &session_id]);
    let stdout = run(resume);
    assert!(stdout.contains(&format!("session {session_id}")));
    assert!(stdout.contains("user: indexed prompt"));
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

    index_session(SESSION_A, &sessions, temp.path());
    let mut resume = neo();
    resume.current_dir(temp.path()).args(["resume", SESSION_A]);
    let resume_stdout = run(resume);
    assert!(resume_stdout.contains(&format!("session {SESSION_A}")));
    assert!(resume_stdout.contains("user: hello"));
    assert!(resume_stdout.contains("assistant: hi back"));
    assert!(!resume_stdout.contains("placeholder"));
}

#[test]
fn resume_specific_session_uses_indexed_workspace() {
    let indexed_workspace = TempDir::new().expect("indexed workspace tempdir");
    let launch_workspace = TempDir::new().expect("launch workspace tempdir");
    let sessions = session_bucket(indexed_workspace.path());
    fs::create_dir_all(&sessions).expect("create indexed sessions bucket");
    write_session_transcript(
        &sessions,
        SESSION_A,
        "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"indexed workspace prompt\"}}]}}}}\n",
    );
    index_session(SESSION_A, &sessions, indexed_workspace.path());

    let mut resume = neo();
    resume
        .current_dir(launch_workspace.path())
        .args(["resume", SESSION_A]);
    let resume_stdout = run(resume);

    assert!(resume_stdout.contains(&format!("session {SESSION_A}")));
    assert!(resume_stdout.contains("user: indexed workspace prompt"));
}

#[test]
fn resume_specific_session_rejects_missing_index_even_when_local_session_exists() {
    let launch_workspace = TempDir::new().expect("launch workspace tempdir");
    let neo_home = launch_workspace.path().join("neo-home");
    let sessions_root = neo_home.join("sessions");
    let local_bucket =
        neo_agent_core::session::workspace_sessions_dir(&sessions_root, launch_workspace.path());
    fs::create_dir_all(&local_bucket).expect("create local sessions bucket");
    write_session_transcript(
        &local_bucket,
        SESSION_A,
        "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"must not read local fallback\"}}]}}}}\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_neo"))
        .current_dir(launch_workspace.path())
        .env("NEO_HOME", &neo_home)
        .args(["resume", SESSION_A])
        .output()
        .expect("neo command should run");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("indexed session not found"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("must not read local fallback"));
}

#[test]
fn resume_specific_session_anchors_relative_config_to_launch_workspace() {
    let launch_workspace = TempDir::new().expect("launch workspace tempdir");
    let indexed_workspace = TempDir::new().expect("indexed workspace tempdir");
    let neo_home = launch_workspace.path().join("neo-home");
    let custom_sessions = launch_workspace.path().join("custom-sessions");
    let config_text = toml::to_string(&serde_json::json!({
        "sessions_dir": custom_sessions,
    }))
    .expect("serialize config");
    fs::write(launch_workspace.path().join("relative.toml"), config_text)
        .expect("write launch config");

    let indexed_bucket =
        neo_agent_core::session::workspace_sessions_dir(&custom_sessions, indexed_workspace.path());
    fs::create_dir_all(&indexed_bucket).expect("create indexed sessions bucket");
    write_session_transcript(
        &indexed_bucket,
        SESSION_A,
        "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"launch relative config transcript\"}}]}}}}\n",
    );
    neo_agent_core::session::SessionIndex::new(&neo_home)
        .append(&neo_agent_core::session::SessionIndexEntry {
            session_id: SESSION_A.to_owned(),
            session_dir: indexed_bucket,
            workdir: indexed_workspace.path().to_path_buf(),
        })
        .expect("index session");

    let output = Command::new(env!("CARGO_BIN_EXE_neo"))
        .current_dir(launch_workspace.path())
        .env("NEO_HOME", &neo_home)
        .args(["--config", "relative.toml", "resume", SESSION_A])
        .output()
        .expect("neo command should run");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("launch relative config transcript"));
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

    index_session(SESSION_A, &sessions, temp.path());
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

#[cfg(unix)]
#[test]
fn sessions_reject_existing_symlink_wire_path_outside_bucket() {
    let temp = TempDir::new().expect("tempdir");
    let sessions = session_bucket(temp.path());
    let outside = TempDir::new().expect("outside tempdir");
    let outside_wire = outside.path().join("wire.jsonl");
    fs::write(&outside_wire, "{}\n").expect("write outside wire");

    let session_dir = sessions.join(SESSION_A);
    let symlink_wire = neo_agent_core::session::main_agent_wire_path(&session_dir);
    fs::create_dir_all(symlink_wire.parent().expect("wire parent")).expect("create wire dir");
    std::os::unix::fs::symlink(&outside_wire, &symlink_wire).expect("symlink wire");

    let output = neo()
        .current_dir(temp.path())
        .args(["sessions", "show"])
        .arg(&symlink_wire)
        .output()
        .expect("neo command should run");

    assert!(
        !output.status.success(),
        "command unexpectedly accepted symlinked wire path outside bucket"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid session id"),
        "expected invalid session id, got {stderr}"
    );
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

    index_session(SESSION_A, &sessions, temp.path());
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
