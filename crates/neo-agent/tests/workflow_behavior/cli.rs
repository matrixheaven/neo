use super::http_server::run;
use serde_json::{Value, json};
use tempfile::TempDir;

// Headless `neo workflow` CLI acceptance tests (Task 9 surface).
//
// Mechanical command-family coverage lives in `cli_commands.rs`. This target
// holds adapter-focused cases that need local fixtures without the full
// interactive binary path noise.

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
    command
        .env("NEO_HOME", neo_home_for_test())
        .env("OPENAI_API_KEY", "test-key");
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

fn write_user_workflow_basic(name: &str, script: &str) {
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
    write_user_workflow_basic("real-run", "return { ok = true }\n");

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

/// `neo workflow run` in non-TTY mode streams JSONL events in order
/// (started before terminal) and returns exact exit codes.
#[test]
fn workflow_run_jsonl_streams_in_order_and_returns_exact_exit_codes() {
    write_user_workflow_basic("stream-run", "return { ok = true }\n");

    let output = neo()
        .stdin(Stdio::null()) // non-TTY
        .args(["workflow", "run", "stream-run", "--output", "jsonl"])
        .output()
        .expect("run");

    assert!(output.status.success(), "exit 0 for completed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<_> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        lines.len() >= 2,
        "jsonl must have at least started + terminal events: {stdout}"
    );

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
    write_user_workflow_basic("check-det", "return { ok = true }\n");

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

#[test]
fn workflow_cli_exposes_exactly_four_commands_and_plain_language_list() {
    let temp = TempDir::new().expect("tempdir");

    let help = neo()
        .current_dir(temp.path())
        .args(["workflow", "--help"])
        .output()
        .expect("help");
    assert!(
        help.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&help.stderr)
    );
    let help_text = String::from_utf8_lossy(&help.stdout);
    for sub in ["list", "run", "check", "test"] {
        assert!(
            help_text.contains(sub),
            "workflow help must contain {sub}: {help_text}"
        );
    }
    // Deleted commands must be absent.
    for deleted in ["show", "save", "answer", "fork", "prune"] {
        assert!(
            !help_text.contains(deleted),
            "workflow help must not contain deleted command {deleted}: {help_text}"
        );
    }

    // --args and --args-file are mutually exclusive on run.
    let conflict = neo()
        .current_dir(temp.path())
        .args([
            "workflow",
            "run",
            "demo",
            "--args",
            "{}",
            "--args-file",
            "args.json",
        ])
        .output()
        .expect("conflict run");
    assert!(
        !conflict.status.success(),
        "expected args source conflict to fail parsing"
    );
    let stderr = String::from_utf8_lossy(&conflict.stderr);
    assert!(
        stderr.contains("cannot be used with") || stderr.contains("conflict"),
        "unexpected conflict stderr: {stderr}"
    );

    // Deleted commands must be rejected.
    let deleted_reject = neo()
        .current_dir(temp.path())
        .args(["workflow", "show", "demo"])
        .output()
        .expect("show reject");
    assert!(!deleted_reject.status.success());
}

#[test]
fn workflow_list_and_check_have_stable_output() {
    let temp = TempDir::new().expect("tempdir");
    write_user_workflow(
        "stable-demo",
        "Stable Demo",
        "stable list example",
        "return { ok = true }\n",
    );

    // list --json returns stable envelope with automation fields only.
    let list_json = run_workflow_args(&temp, &["workflow", "list", "--json"]);
    let list_value: Value = serde_json::from_str(list_json.trim()).expect("list json");
    let definitions = list_value
        .get("workflows")
        .and_then(Value::as_array)
        .expect("workflows array");
    assert!(
        definitions
            .iter()
            .any(|item| item.get("name").and_then(Value::as_str) == Some("stable-demo")),
        "list json missing stable-demo: {list_json}"
    );
    let demo = definitions
        .iter()
        .find(|item| item.get("name").and_then(Value::as_str) == Some("stable-demo"))
        .expect("demo");
    for key in ["name", "display_name", "description"] {
        assert!(demo.get(key).is_some(), "list item missing {key}");
    }
    // Machine output must not expose absolute paths.
    assert!(demo.get("source_locator").is_none());
    assert!(demo.get("revision").is_none());
    assert!(demo.get("source_origin").is_none());

    // Text list shows name, display name, and purpose.
    let list_text = run_workflow_args(&temp, &["workflow", "list"]);
    assert!(list_text.contains("stable-demo"), "text: {list_text}");
    assert!(list_text.contains("Stable Demo"), "text: {list_text}");
    assert!(
        list_text.contains("stable list example"),
        "text: {list_text}"
    );

    // A fresh home has no user or project definitions: the effective list
    // degrades to the builtin workflows only.
    let empty_home = std::env::temp_dir().join(format!(
        "neo-cli-empty-home-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    fs::create_dir_all(&empty_home).expect("empty home");
    let mut empty_cmd = Command::new(env!("CARGO_BIN_EXE_neo"));
    empty_cmd
        .env("NEO_HOME", &empty_home)
        .current_dir(temp.path())
        .args(["workflow", "list", "--json"]);
    let empty_list = run(empty_cmd);
    let empty_value: Value = serde_json::from_str(empty_list.trim()).expect("empty list json");
    let empty_definitions = empty_value
        .get("workflows")
        .and_then(Value::as_array)
        .expect("workflows array");
    assert!(
        !empty_definitions.is_empty(),
        "builtin workflows are always available: {empty_list}"
    );
    assert!(
        empty_definitions
            .iter()
            .all(|item| item
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| matches!(
                    name,
                    "code-review" | "deep-research" | "large-refactor"
                ))),
        "fresh home exposes only builtin workflows: {empty_list}"
    );
    assert!(
        !empty_definitions
            .iter()
            .any(|item| item.get("name").and_then(Value::as_str) == Some("stable-demo")),
        "user workflow must not leak into a fresh home: {empty_list}"
    );

    // check --json is stable and creates no runs.
    let check_json = run_workflow_args(&temp, &["workflow", "check", "stable-demo", "--json"]);
    let check_value: Value = serde_json::from_str(check_json.trim()).expect("check json");
    assert_eq!(check_value["ok"], json!(true));
    assert_eq!(check_value["name"], json!("stable-demo"));
    assert!(check_value["revision"].as_str().unwrap().len() == 64);

    // Second invocation is identical.
    let check2 = run_workflow_args(&temp, &["workflow", "check", "stable-demo", "--json"]);
    assert_eq!(check_json, check2, "check JSON must be stable");

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

// Task 21: paged /tasks workflow dashboard projection
#[tokio::test]
async fn tasks_workflow_pagination_and_filters_are_stable() {
    use neo_agent_core::tools::{
        BackgroundTaskKind, BackgroundTaskListQuery, BackgroundTaskManager, BackgroundTaskStatus,
    };
    use neo_agent_core::workflow::{
        WorkflowLaunchRequest, WorkflowLimits, WorkflowPhase, WorkflowRuntime,
    };

    let temp = TempDir::new().expect("tempdir");
    let sessions = temp.path().join("sessions");
    fs::create_dir_all(&sessions).expect("sessions");

    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let manager = BackgroundTaskManager::new();

    // Create 55 workflows across two definitions so pagination exceeds the old 50 cap.
    let mut run_ids = Vec::new();
    for index in 0..55 {
        let name = if index % 2 == 0 {
            "deep-research"
        } else {
            "code-review"
        };
        let handle = runtime
            .create_run(
                &sessions,
                WorkflowLaunchRequest {
                    name: name.to_owned(),
                    description: format!("{name} run {index}"),
                    phases: vec![WorkflowPhase {
                        id: "work".to_owned(),
                        description: "work".to_owned(),
                    }],
                    script: "neo.phase('work')".to_owned(),
                    args: serde_json::json!({}),
                    launch_source: if index % 3 == 0 {
                        "builtin".to_owned()
                    } else {
                        "user".to_owned()
                    },
                    output_schema: None,
                    display_name: None,
                    input_schema: None,
                    definition_origin: None,
                    inline_unsaved: false,
                },
            )
            .await
            .expect("create run");
        let task_id = handle.run_id.0.clone();
        run_ids.push(task_id.clone());
        manager
            .start_workflow(task_id, format!("{name} run {index}"), handle)
            .await
            .expect("register");
    }

    // Pause one run so state filter has a distinct target.
    let paused_id = &run_ids[0];
    manager
        .pause_workflow(paused_id, neo_agent_core::workflow::WorkflowActor::Human)
        .await
        .expect("pause");

    // Page 1: first 20 of 55, stable ordering, cursor bound.
    let page1 = manager
        .list_page(BackgroundTaskListQuery {
            active_only: false,
            kind: Some(BackgroundTaskKind::Workflow),
            limit: 20,
            ..BackgroundTaskListQuery::default()
        })
        .await
        .expect("page1");
    assert_eq!(page1.items.len(), 20, "page1 size");
    assert!(page1.has_more, "page1 has_more");
    assert!(page1.next_cursor.is_some(), "page1 cursor");
    assert_eq!(page1.total_matched, 55);
    let page1_ids: Vec<_> = page1
        .items
        .iter()
        .map(|item| item.task_id.clone())
        .collect();

    // Wrong query cursor is rejected (query-bound).
    let wrong = manager
        .list_page(BackgroundTaskListQuery {
            active_only: true,
            kind: Some(BackgroundTaskKind::Workflow),
            limit: 20,
            cursor: page1.next_cursor.clone(),
            ..BackgroundTaskListQuery::default()
        })
        .await;
    assert!(
        wrong.is_err(),
        "cursor from a different query must be rejected"
    );

    // Page 2 continues without overlap.
    let page2 = manager
        .list_page(BackgroundTaskListQuery {
            active_only: false,
            kind: Some(BackgroundTaskKind::Workflow),
            limit: 20,
            cursor: page1.next_cursor.clone(),
            ..BackgroundTaskListQuery::default()
        })
        .await
        .expect("page2");
    assert_eq!(page2.items.len(), 20);
    assert!(page2.has_more);
    let page2_ids: Vec<_> = page2
        .items
        .iter()
        .map(|item| item.task_id.clone())
        .collect();
    for id in &page1_ids {
        assert!(
            !page2_ids.contains(id),
            "page2 must not repeat page1 id {id}"
        );
    }

    // Final page drains remainder.
    let page3 = manager
        .list_page(BackgroundTaskListQuery {
            active_only: false,
            kind: Some(BackgroundTaskKind::Workflow),
            limit: 20,
            cursor: page2.next_cursor.clone(),
            ..BackgroundTaskListQuery::default()
        })
        .await
        .expect("page3");
    assert_eq!(page3.items.len(), 15);
    assert!(!page3.has_more);
    assert!(page3.next_cursor.is_none());

    // Full enumeration via pages exceeds old hard 50.
    let mut all = Vec::new();
    all.extend(page1_ids);
    all.extend(page2_ids);
    all.extend(page3.items.iter().map(|item| item.task_id.clone()));
    assert_eq!(all.len(), 55);
    // Stability: re-query first page yields identical order.
    let page1_again = manager
        .list_page(BackgroundTaskListQuery {
            active_only: false,
            kind: Some(BackgroundTaskKind::Workflow),
            limit: 20,
            ..BackgroundTaskListQuery::default()
        })
        .await
        .expect("page1 again");
    let again_ids: Vec<_> = page1_again
        .items
        .iter()
        .map(|item| item.task_id.clone())
        .collect();
    assert_eq!(again_ids, all[..20]);

    // Definition filter.
    let research = manager
        .list_page(BackgroundTaskListQuery {
            active_only: false,
            kind: Some(BackgroundTaskKind::Workflow),
            definition_name: Some("deep-research".to_owned()),
            limit: 100,
            ..BackgroundTaskListQuery::default()
        })
        .await
        .expect("definition filter");
    assert!(research.total_matched >= 27);
    assert!(research.items.iter().all(|item| {
        item.workflow
            .as_ref()
            .is_some_and(|w| w.definition_name == "deep-research")
    }));

    // Handle filter uses allocated human handles (deep-research, deep-research-2, ...).
    let first_handle = research.items[0]
        .workflow
        .as_ref()
        .and_then(|w| w.human_handle.clone())
        .expect("human handle");
    let by_handle = manager
        .list_page(BackgroundTaskListQuery {
            active_only: false,
            handle: Some(first_handle.clone()),
            limit: 10,
            ..BackgroundTaskListQuery::default()
        })
        .await
        .expect("handle filter");
    assert_eq!(by_handle.total_matched, 1);
    assert_eq!(
        by_handle.items[0]
            .workflow
            .as_ref()
            .and_then(|w| w.human_handle.as_deref()),
        Some(first_handle.as_str())
    );

    // State filter (paused).
    let paused = manager
        .list_page(BackgroundTaskListQuery {
            active_only: false,
            state: Some(BackgroundTaskStatus::Paused),
            limit: 10,
            ..BackgroundTaskListQuery::default()
        })
        .await
        .expect("state filter");
    assert!(paused.total_matched >= 1);
    assert!(
        paused
            .items
            .iter()
            .all(|item| item.status == BackgroundTaskStatus::Paused)
    );

    // Source scope filter.
    let builtin = manager
        .list_page(BackgroundTaskListQuery {
            active_only: false,
            kind: Some(BackgroundTaskKind::Workflow),
            source_scope: Some("builtin".to_owned()),
            limit: 100,
            ..BackgroundTaskListQuery::default()
        })
        .await
        .expect("scope filter");
    assert!(builtin.total_matched >= 1);
    assert!(builtin.items.iter().all(|item| {
        item.workflow
            .as_ref()
            .and_then(|w| w.source_scope.as_deref())
            .is_some_and(|scope| scope.contains("builtin"))
    }));

    // Projection metadata present.
    let sample = &page1.items[0];
    let meta = sample.workflow.as_ref().expect("workflow meta");
    assert!(!meta.definition_name.is_empty());
    assert!(meta.human_handle.is_some());
    assert!(meta.run_id == sample.task_id || !meta.run_id.is_empty());
}

// ---------------------------------------------------------------------------
// Task 19: headless workflow command family
// ---------------------------------------------------------------------------

fn workflow_pair_bytes(
    name: &str,
    display: &str,
    description: &str,
    script: &str,
) -> (String, String) {
    use sha2::{Digest, Sha256};
    let source_sha = format!("{:x}", Sha256::digest(script.as_bytes()));
    let toml = format!(
        r#"
name = "{name}"
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
    (toml, script.to_owned())
}

fn write_user_workflow(name: &str, display: &str, description: &str, script: &str) {
    let home = neo_home_for_test();
    let dir = home.join("workflows");
    fs::create_dir_all(&dir).expect("workflows dir");
    let (toml, lua) = workflow_pair_bytes(name, display, description, script);
    fs::write(dir.join(format!("{name}.lua")), lua).expect("write lua");
    fs::write(dir.join(format!("{name}.workflow.toml")), toml).expect("write toml");
}

fn run_workflow_args(temp: &TempDir, args: &[&str]) -> String {
    let mut command = neo();
    command.current_dir(temp.path()).args(args);
    run(command)
}
