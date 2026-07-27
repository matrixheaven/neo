//! Headless workflow CLI adapter tests (Task 19 surface; Task 22 check harness).
//!
//! Mechanical command-family coverage lives in `cli_commands.rs`. This target
//! holds adapter-focused cases that need local fixtures without the full
//! interactive binary path noise.

use std::{
    fs,
    path::PathBuf,
    process::Command,
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
definition_format_version = 2
display_name = "Check Demo"
description = "stable check json"
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

/// `neo workflow check --output json` is stable across invocations and creates no runs.
#[test]
fn workflow_check_json_is_stable_and_read_only() {
    write_user_workflow("check-demo", "return { ok = true }\n");

    let (ok1, stdout1, stderr1) =
        run_args(&["workflow", "check", "check-demo", "--output", "json"]);
    assert!(
        ok1,
        "check should succeed\nstdout:\n{stdout1}\nstderr:\n{stderr1}"
    );

    let (ok2, stdout2, stderr2) =
        run_args(&["workflow", "check", "check-demo", "--output", "json"]);
    assert!(
        ok2,
        "second check should succeed\nstdout:\n{stdout2}\nstderr:\n{stderr2}"
    );
    assert_eq!(
        stdout1, stdout2,
        "check JSON must be stable across invocations"
    );

    let value: serde_json::Value = serde_json::from_str(stdout1.trim()).expect("json report");
    assert_eq!(value["ok"], serde_json::json!(true));
    assert_eq!(value["name"], serde_json::json!("check-demo"));
    assert_eq!(value["revision"].as_str().unwrap().len(), 64);
    assert!(
        value["diagnostics"].as_array().unwrap().is_empty()
            || value["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .all(|d| d["severity"] != "error")
    );

    // Read-only: no session workflow run directories under NEO_HOME.
    let home = neo_home_for_test();
    let mut found_run = false;
    let mut stack = vec![home.clone()];
    while let Some(path) = stack.pop() {
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if p.file_name().and_then(|n| n.to_str()) == Some("workflows") && path != home {
                    // user definitions live at $NEO_HOME/workflows; run storage is nested.
                    if p.join("run.json").exists()
                        || fs::read_dir(&p).is_ok_and(|mut d| {
                            d.any(|e| e.is_ok_and(|e| e.path().join("run.json").exists()))
                        })
                    {
                        found_run = true;
                    }
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

    // Path-based check of an invalid definition is also read-only JSON.
    let bad_dir = home.join("bad");
    fs::create_dir_all(&bad_dir).unwrap();
    let bad_script = "!!! not lua";
    let bad_sha = source_sha256_hex(bad_script.as_bytes());
    let bad_toml = format!(
        r#"
definition_format_version = 2
display_name = "Bad"
description = "bad lua"
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

    let (ok_bad, stdout_bad, _stderr_bad) = run_args(&[
        "workflow",
        "check",
        bad_toml_path.to_str().unwrap(),
        "--output",
        "json",
    ]);
    // JSON mode returns a report even on failure (exit may be success for json).
    let report: serde_json::Value =
        serde_json::from_str(stdout_bad.trim()).expect("bad check json");
    assert_eq!(report["ok"], serde_json::json!(false));
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["severity"] == "error"),
        "{report}"
    );
    let _ = ok_bad;
}

/// `neo workflow run` launches headlessly through the shared coordinator and
/// reaches a terminal state with no slash or model prerequisite.
#[test]
fn workflow_run_executes_headless_to_terminal() {
    write_user_workflow("run-demo", "return { ok = true }\n");

    let (ok, stdout, stderr) = run_args(&["workflow", "run", "run-demo"]);
    assert!(
        ok,
        "headless run should succeed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(!stdout.trim().is_empty(), "run prints a result");
}
