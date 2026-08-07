use super::http_server::{MockSseServer, openai_response_sse};
use std::{
    io::Write,
    process::{Command, Stdio},
};

use tempfile::TempDir;

/// Control sequences owned by the fullscreen surface. Static modes must
/// never emit any of them.
const FULLSCREEN_SEQUENCES: &[&str] = &[
    "\x1b[?1049h", // enter alternate screen
    "\x1b[?1049l", // leave alternate screen
    "\x1b[?1000h", // mouse tracking enable
    "\x1b[?1002h",
    "\x1b[?1003h",
    "\x1b[?1006h",
    "\x1b[?2026h", // synchronized output transaction
    "\x1b[?2026l",
];

fn assert_no_fullscreen_sequences(mode: &str, output: &str) {
    for sequence in FULLSCREEN_SEQUENCES {
        assert!(
            !output.contains(sequence),
            "{mode} emitted fullscreen sequence {sequence:?}:\n{output}"
        );
    }
}

/// Each test thread gets its own stable isolated home directory so that
/// multiple `neo()` calls within the same test share the same sessions root.
fn isolated_home_path() -> std::path::PathBuf {
    thread_local! {
        static HOME: std::cell::OnceCell<(TempDir, std::path::PathBuf)> = const { std::cell::OnceCell::new() };
    }
    HOME.with(|cell| {
        let (_, path) = cell.get_or_init(|| {
            let home = TempDir::new().expect("isolated home");
            let path = home.path().to_path_buf();
            (home, path)
        });
        path.clone()
    })
}

fn neo() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_neo"));
    let home = isolated_home_path();
    command.env("NEO_HOME", &home);
    command.env("HOME", &home);
    command
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

fn write_config(base_url: &str) {
    let config_dir = isolated_home_path();
    std::fs::create_dir_all(&config_dir).expect("create neo home");
    let content = format!(
        r#"
default_provider = "mock"
default_model = "gpt-4.1"

[providers.mock]
type = "openai_response"
base_url = "{base_url}"
api_key_env = "OPENAI_API_KEY"

[models."mock/gpt-4.1"]
provider = "mock"
model = "gpt-4.1"
capabilities = ["streaming", "tools"]
"#
    );
    std::fs::write(config_dir.join("config.toml"), content).expect("write config");
}

fn session_files() -> Vec<std::path::PathBuf> {
    let home_sessions = isolated_home_path().join("sessions");
    let mut entries = Vec::new();
    collect_jsonl_recursive(&home_sessions, &mut entries);
    entries.sort();
    entries
}

fn collect_jsonl_recursive(dir: &std::path::Path, results: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_recursive(&path, results);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            results.push(path);
        }
    }
}

#[test]
fn static_modes_never_emit_fullscreen_sequences() {
    // Root `neo` with piped stdin/stdout stays in the static snapshot path.
    let root_output = run_with_stdin(neo(), "");
    assert_no_fullscreen_sequences("root", &root_output);

    // A mock provider serves `neo run` (arg prompt) and the piped-stdin
    // prompt in the same isolated home. Each run issues a main-turn request
    // plus a title-generation request, so two responses per run are queued.
    let server = MockSseServer::start(vec![
        openai_response_sse("resp-static-run", "static run reply"),
        openai_response_sse("resp-static-run-title", "static run title"),
        openai_response_sse("resp-static-pipe", "static pipe reply"),
        openai_response_sse("resp-static-pipe-title", "static pipe title"),
    ]);
    write_config(&server.url);

    let mut run_command = neo();
    run_command
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "hello"]);
    let run_output = run(run_command);
    assert_no_fullscreen_sequences("run", &run_output);
    assert!(run_output.contains("static run reply"), "{run_output}");

    // Prompt via piped stdin.
    let mut pipe_command = neo();
    pipe_command
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text"]);
    let pipe_output = run_with_stdin(pipe_command, "hello from pipe");
    assert_no_fullscreen_sequences("pipe", &pipe_output);
    assert!(pipe_output.contains("static pipe reply"), "{pipe_output}");

    // Session export and non-TTY resume operate on the persisted sessions.
    // Both runs persist a session; every session must export and resume as
    // static output.
    let sessions = session_files();
    assert!(!sessions.is_empty(), "run must persist a session");
    let mut saw_run_reply = false;
    for wire in &sessions {
        // Layout: sessions/<bucket>/<session_id>/agents/main/wire.jsonl
        let session_id = wire
            .parent()
            .and_then(std::path::Path::parent)
            .and_then(std::path::Path::parent)
            .and_then(std::path::Path::file_name)
            .and_then(std::ffi::OsStr::to_str)
            .expect("session id");

        let mut export_command = neo();
        export_command.args(["sessions", "export-json", session_id]);
        let export_output = run(export_command);
        assert_no_fullscreen_sequences("export", &export_output);
        saw_run_reply |= export_output.contains("static run reply");

        let mut resume_command = neo();
        resume_command.args(["resume", session_id]);
        let resume_output = run(resume_command);
        assert_no_fullscreen_sequences("resume", &resume_output);
        saw_run_reply |= resume_output.contains("static run reply");
    }
    assert!(saw_run_reply, "at least one session carries the run reply");
}
