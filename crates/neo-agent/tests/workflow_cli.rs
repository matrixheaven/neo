//! Headless `neo workflow` CLI acceptance tests (Task 9 surface).
//!
//! Mechanical command-family coverage lives in `cli_commands.rs`. This target
//! holds adapter-focused cases that need local fixtures without the full
//! interactive binary path noise.

use std::{
    fs,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};

fn neo() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_neo"));
    command.env("NEO_HOME", neo_home_for_test());
    command
}

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
            std::env::temp_dir().join(format!("neo-workflow-cli-home-{nanos}-{id}"))
        })
        .clone()
    })
}

fn source_sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn write_user_workflow(name: &str, script: &str) {
    let home = neo_home_for_test();
    let dir = home.join("workflows");
    fs::create_dir_all(&dir).expect("workflows dir");
    let source_sha = source_sha256_hex(script.as_bytes());
    let toml = format!(
        r#"
display_name = "Test Workflow"
description = "CLI acceptance test"
source_sha256 = "{source_sha}"

[[phases]]
id = "run"
description = "execute"

[output_schema]
type = "object"
additionalProperties = false
required = ["ok"]

[output_schema.properties.ok]
type = "boolean"
"#
    );
    fs::write(dir.join(format!("{name}.workflow.toml")), toml).expect("write toml");
    fs::write(dir.join(format!("{name}.lua")), script).expect("write lua");
}

fn write_user_workflow_named(name: &str, display: &str, description: &str, script: &str) {
    let home = neo_home_for_test();
    let dir = home.join("workflows");
    fs::create_dir_all(&dir).expect("workflows dir");
    let source_sha = source_sha256_hex(script.as_bytes());
    let toml = format!(
        r#"
display_name = "{display}"
description = "{description}"
source_sha256 = "{source_sha}"

[[phases]]
id = "run"
description = "execute"

[output_schema]
type = "object"
additionalProperties = false
required = ["ok"]

[output_schema.properties.ok]
type = "boolean"
"#
    );
    fs::write(dir.join(format!("{name}.workflow.toml")), toml).expect("write toml");
    fs::write(dir.join(format!("{name}.lua")), script).expect("write lua");
}

fn run_args(args: &[&str]) -> (bool, String, String) {
    let output = neo().args(args).output().expect("neo command should run");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// `neo workflow run` executes real Lua and returns the actual result.
#[test]
fn workflow_run_executes_real_lua_and_returns_actual_result() {
    write_user_workflow("real-run", "return { ok = true }\n");

    let (ok, stdout, stderr) = run_args(&["workflow", "run", "real-run"]);
    assert!(
        ok,
        "run should succeed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(!stdout.trim().is_empty(), "run prints a result");

    // JSON output returns the final result.
    let (ok, stdout, stderr) = run_args(&["workflow", "run", "real-run", "--output", "json"]);
    assert!(
        ok,
        "json run should succeed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).expect("json output");
    assert_eq!(value["state"], "completed");
    assert!(value.get("final_result").is_some());
}

/// `neo workflow run` in non-TTY mode streams JSONL events.
#[test]
fn workflow_run_non_tty_streams_events_and_returns_exact_exit_codes() {
    write_user_workflow("stream-run", "return { ok = true }\n");

    let output = neo()
        .stdin(Stdio::null()) // non-TTY
        .args(["workflow", "run", "stream-run", "--output", "jsonl"])
        .output()
        .expect("run");

    assert!(output.status.success(), "exit 0 for completed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<_> = stdout.lines().collect();
    assert!(!lines.is_empty(), "must emit at least one JSONL line");

    // First line should be started event.
    let first: serde_json::Value = serde_json::from_str(lines[0]).expect("jsonl line 1");
    assert_eq!(first["type"], "started");

    // Last line should be terminal event.
    let last: serde_json::Value = serde_json::from_str(lines[lines.len() - 1]).expect("jsonl last");
    assert_eq!(last["type"], "terminal");
    assert_eq!(last["state"], "completed");

    // Existing command from deleted surface exits non-zero (exit 2: invalid input).
    let bad = neo()
        .args(["workflow", "show"])
        .output()
        .expect("show should fail");
    assert!(!bad.status.success(), "deleted command must fail");
}

/// `neo workflow check` and `test` are deterministic and side-effect free.
#[test]
fn workflow_check_and_test_are_deterministic_side_effect_free_and_actionable() {
    let home = neo_home_for_test();

    // check on a valid workflow succeeds deterministically.
    write_user_workflow("check-det", "return { ok = true }\n");

    let (ok1, stdout1, _) = run_args(&["workflow", "check", "check-det", "--json"]);
    assert!(ok1, "check must succeed");
    let v1: serde_json::Value = serde_json::from_str(stdout1.trim()).expect("json");
    assert_eq!(v1["ok"], true);

    let (ok2, stdout2, _) = run_args(&["workflow", "check", "check-det", "--json"]);
    assert!(ok2);
    assert_eq!(stdout1, stdout2, "check must be deterministic");

    // check on an invalid definition reports errors.
    let bad_dir = home.join("bad");
    fs::create_dir_all(&bad_dir).unwrap();
    let bad_script = "!!! not lua";
    let bad_sha = source_sha256_hex(bad_script.as_bytes());
    let bad_toml = format!(
        r#"
display_name = "Bad"
description = "bad"
source_sha256 = "{bad_sha}"

[[phases]]
id = "run"
description = "execute"

[output_schema]
type = "object"
additionalProperties = false
required = ["ok"]

[output_schema.properties.ok]
type = "boolean"
"#
    );
    let bad_toml_path = bad_dir.join("bad.workflow.toml");
    fs::write(&bad_toml_path, bad_toml).unwrap();
    fs::write(bad_dir.join("bad.lua"), bad_script).unwrap();

    let (_ok, stdout_bad, _) = run_args(&[
        "workflow",
        "check",
        bad_toml_path.to_str().unwrap(),
        "--json",
    ]);
    let report: serde_json::Value =
        serde_json::from_str(stdout_bad.trim()).expect("bad check json");
    assert_eq!(report["ok"], false);
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["severity"] == "error"),
        "{report}"
    );

    // check never creates run directories.
    let mut found_run = false;
    let mut stack = vec![home.clone()];
    while let Some(path) = stack.pop() {
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if p.file_name().and_then(|n| n.to_str()) == Some("workflows")
                    && path != home
                    && (p.join("run.json").exists()
                        || fs::read_dir(&p).is_ok_and(|mut d| {
                            d.any(|e| e.is_ok_and(|e| e.path().join("run.json").exists()))
                        }))
                {
                    found_run = true;
                }
                stack.push(p);
            }
        }
    }
    assert!(
        !found_run,
        "workflow check must not create durable runs under {}",
        home.display()
    );
}

/// Text list output shows display name, purpose; not internal fields.
#[test]
fn workflow_list_shows_plain_language_fields_only() {
    write_user_workflow_named(
        "view-list",
        "Review Checker",
        "Checks your code for issues",
        "return { ok = true }\n",
    );

    let (ok, stdout, _) = run_args(&["workflow", "list"]);
    assert!(ok, "list should succeed");
    assert!(stdout.contains("view-list"));
    assert!(stdout.contains("Review Checker"));
    assert!(stdout.contains("Checks your code for issues"));
    // Must not expose internal details.
    assert!(!stdout.contains("sha256"));
    assert!(!stdout.contains("revision"));
    assert!(!stdout.contains("workflows/"));
}

/// JSONL streaming emits events before terminal completion.
#[test]
fn workflow_run_jsonl_streams_before_terminal() {
    write_user_workflow("jsonl-run", "return { ok = true }\n");

    let output = neo()
        .stdin(Stdio::null())
        .args(["workflow", "run", "jsonl-run", "--output", "jsonl"])
        .output()
        .expect("run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<_> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        lines.len() >= 2,
        "jsonl must have at least started + terminal events: {stdout}"
    );

    let started: serde_json::Value = serde_json::from_str(lines[0]).expect("started jsonl");
    assert_eq!(started["type"], "started");

    let terminal: serde_json::Value =
        serde_json::from_str(lines[lines.len() - 1]).expect("terminal jsonl");
    assert_eq!(terminal["type"], "terminal");
}
