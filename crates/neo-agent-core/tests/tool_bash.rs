use neo_agent_core::execute_model_bash_for_runtime;
use neo_agent_core::{ToolAccess, ToolContext, ToolError, ToolRegistry};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[test]
fn bash_model_schema_matches_kimi_style_shape() {
    let registry = ToolRegistry::with_builtin_tools();
    let bash = registry
        .specs()
        .into_iter()
        .find(|spec| spec.name == "Bash")
        .expect("Bash tool spec");
    let schema = bash
        .input_schema
        .get("schema")
        .unwrap_or(&bash.input_schema);
    let required = schema["required"].as_array().expect("required array");
    let properties = schema["properties"].as_object().expect("schema properties");

    assert!(required.iter().any(|field| field == "command"));
    assert!(!required.iter().any(|field| field == "mode"));
    assert!(!properties.contains_key("mode"));
    for field in [
        "command",
        "cwd",
        "timeout_secs",
        "run_in_background",
        "description",
        "max_output_bytes",
    ] {
        assert!(
            properties
                .get(field)
                .and_then(|property| property.get("description"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|description| !description.trim().is_empty()),
            "{field} should have a non-empty description"
        );
    }
}

#[test]
fn builtin_tool_names_use_model_facing_kimi_style_casing() {
    let mut names = ToolRegistry::with_builtin_tools()
        .specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();
    names.sort();

    assert_eq!(
        names,
        vec![
            "Bash",
            "Delegate",
            "DelegateSwarm",
            "Edit",
            "EnterPlanMode",
            "ExitPlanMode",
            "Find",
            "Glob",
            "Grep",
            "InterruptDelegate",
            "List",
            "ListDelegates",
            "MessageDelegate",
            "Read",
            "RunWorkflow",
            "Sleep",
            "TaskList",
            "TaskOutput",
            "TaskStop",
            "Terminal",
            "TodoList",
            "WaitDelegate",
            "Write",
        ]
    );
}

#[tokio::test]
async fn bash_foreground_output_is_raw_terminal_text_with_structured_details() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = registry
        .run(
            "Bash",
            &context,
            json!({ "command": "printf out; printf err >&2" }),
        )
        .await
        .expect("Bash should run");

    assert_eq!(result.content, "outerr");
    assert!(!result.content.contains("exit_code:"));
    assert!(!result.content.contains("stdout:"));
    assert!(!result.content.contains("stderr:"));
    assert_eq!(
        result
            .details
            .as_ref()
            .and_then(|details| details["exit_code"].as_i64()),
        Some(0)
    );
    assert_eq!(
        result
            .details
            .as_ref()
            .and_then(|details| details["stdout"].as_str()),
        Some("out")
    );
    assert_eq!(
        result
            .details
            .as_ref()
            .and_then(|details| details["stderr"].as_str()),
        Some("err")
    );
}

#[tokio::test]
async fn bash_timeout_secs_enforces_supported_range() {
    let workspace = tempfile::tempdir().expect("workspace");
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    for timeout_secs in [299, 3_601] {
        let error = execute_model_bash_for_runtime(
            &context,
            json!({"command": "printf ready", "timeout_secs": timeout_secs}),
        )
        .await
        .expect_err("out-of-range timeout was accepted");
        assert!(
            error.to_string().contains("between 300 and 3600"),
            "{error}"
        );
    }

    for timeout_secs in [300, 3_600] {
        let result = execute_model_bash_for_runtime(
            &context,
            json!({"command": "printf ready", "timeout_secs": timeout_secs}),
        )
        .await
        .expect("boundary timeout should be accepted");
        assert_eq!(result.content, "ready");
    }
}

#[test]
fn bash_schema_uses_optional_timeout_secs_without_legacy_timeout() {
    let bash = ToolRegistry::with_builtin_tools()
        .specs()
        .into_iter()
        .find(|spec| spec.name == "Bash")
        .expect("Bash spec");
    let schema = bash
        .input_schema
        .get("schema")
        .unwrap_or(&bash.input_schema);
    let properties = schema["properties"].as_object().expect("properties");
    assert!(properties.contains_key("timeout_secs"));
    assert!(!properties.contains_key("timeout"));
    let timeout = &properties["timeout_secs"];
    assert_eq!(timeout["minimum"], 300);
    assert_eq!(timeout["maximum"], 3_600);
    let text = timeout.to_string();
    assert!(!text.to_lowercase().contains("rust"));
    assert!(!text.to_lowercase().contains("cargo"));
}

#[tokio::test]
async fn task_list_defaults_to_active_background_tasks() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let empty = registry
        .run("TaskList", &context, json!({}))
        .await
        .expect("TaskList should run");
    assert_eq!(
        empty.content,
        "active_background_tasks: 0\nNo background tasks found."
    );

    let started = registry
        .run(
            "Bash",
            &context,
            json!({
                "command": "sleep 1",
                "run_in_background": true,
                "description": "sleeping background command"
            }),
        )
        .await
        .expect("background bash should start");
    let task_id = started.details.as_ref().expect("start details")["task_id"]
        .as_str()
        .expect("task id")
        .to_owned();

    let listed = registry
        .run("TaskList", &context, json!({}))
        .await
        .expect("TaskList should list active tasks");
    assert!(listed.content.contains("active_background_tasks: 1"));
    assert!(listed.content.contains(&format!("task_id: {task_id}")));
    assert!(listed.content.contains("kind: bash"));
    assert!(listed.content.contains("status: running"));
    assert!(
        listed
            .content
            .contains("description: sleeping background command")
    );

    let details = listed.details.expect("list details");
    assert_eq!(details["active_background_tasks"], 1);
    assert_eq!(details["tasks"][0]["task_id"], task_id);
    assert_eq!(details["tasks"][0]["kind"], "bash");

    let _ = registry
        .run("TaskStop", &context, json!({ "task_id": task_id }))
        .await;
}

#[tokio::test]
async fn bash_background_run_returns_task_id_and_task_output_finishes() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let started = registry
        .run(
            "Bash",
            &context,
            json!({
                "command": "printf started; sleep 0.05; printf done",
                "run_in_background": true,
                "description": "short background command",
                "max_output_bytes": 64
            }),
        )
        .await
        .expect("background bash should start");
    let start_details = started.details.as_ref().expect("start details");
    let task_id = start_details["task_id"]
        .as_str()
        .expect("task id")
        .to_owned();
    assert!(task_id.starts_with("bash-"));
    assert_eq!(start_details["status"], "running");

    let finished = registry
        .run(
            "TaskOutput",
            &context,
            json!({ "task_id": task_id, "block": true, "timeout": 1, "max_output_bytes": 64 }),
        )
        .await
        .expect("TaskOutput should read background output");
    let details = finished.details.expect("output details");
    assert_eq!(details["status"], "completed");
    assert_eq!(details["exit_code"], 0);
    assert_eq!(details["stdout"], "starteddone");
    assert_eq!(details["stderr"], "");
    assert_eq!(details["truncated"], false);
}

#[tokio::test]
async fn bash_background_requires_description() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let error = registry
        .run(
            "Bash",
            &context,
            json!({ "command": "sleep 1", "run_in_background": true }),
        )
        .await
        .expect_err("background bash requires description");

    assert!(matches!(error, ToolError::InvalidInput { .. }));
}

#[tokio::test]
async fn task_output_block_times_out_while_task_is_running() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let started = registry
        .run(
            "Bash",
            &context,
            json!({
                "command": "sleep 1; printf done",
                "run_in_background": true,
                "description": "sleep briefly"
            }),
        )
        .await
        .expect("background bash should start");
    let task_id = started.details.as_ref().expect("start details")["task_id"]
        .as_str()
        .expect("task id")
        .to_owned();

    let output = registry
        .run(
            "TaskOutput",
            &context,
            json!({ "task_id": task_id, "block": true, "timeout": 0 }),
        )
        .await
        .expect("TaskOutput timeout should still return snapshot");
    let details = output.details.expect("output details");
    assert_eq!(details["status"], "running");

    let _ = registry
        .run("TaskStop", &context, json!({ "task_id": task_id }))
        .await;
}

#[tokio::test]
async fn task_stop_is_safe_for_finished_task() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let started = registry
        .run(
            "Bash",
            &context,
            json!({
                "command": "printf once",
                "run_in_background": true,
                "description": "quick command"
            }),
        )
        .await
        .expect("background bash should start");
    let task_id = started.details.as_ref().expect("start details")["task_id"]
        .as_str()
        .expect("task id")
        .to_owned();

    let _ = registry
        .run(
            "TaskOutput",
            &context,
            json!({ "task_id": task_id, "block": true, "timeout": 1 }),
        )
        .await
        .expect("task should finish");
    let stopped = registry
        .run("TaskStop", &context, json!({ "task_id": task_id }))
        .await
        .expect("TaskStop should be safe after completion");
    let details = stopped.details.expect("stop details");
    assert_eq!(details["status"], "completed");
    assert_eq!(details["stdout"], "once");
}

#[tokio::test]
async fn bash_cwd_runs_command_from_workspace_subdirectory() {
    let workspace = tempfile::tempdir().expect("workspace");
    let subdir = workspace.path().join("sub");
    std::fs::create_dir(&subdir).expect("subdir");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = registry
        .run("Bash", &context, json!({ "command": "pwd", "cwd": "sub" }))
        .await
        .expect("foreground bash should run");

    assert_eq!(
        result.details.expect("details")["stdout"],
        format!(
            "{}\n",
            subdir.canonicalize().expect("canonical subdir").display()
        )
    );
}

#[tokio::test]
async fn bash_cwd_rejects_paths_outside_workspace() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let error = registry
        .run("Bash", &context, json!({ "command": "pwd", "cwd": ".." }))
        .await
        .expect_err("cwd should stay inside workspace");

    assert!(matches!(error, ToolError::PathOutsideWorkspace { .. }));
}

#[tokio::test]
async fn bash_foreground_returns_after_shell_exits_with_inherited_background_output() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        registry.run(
            "Bash",
            &context,
            json!({
                "command": "sleep 5 & printf done",
                "timeout_secs": 300
            }),
        ),
    )
    .await
    .expect("foreground bash should not wait for orphaned pipe handles")
    .expect("foreground bash should run");

    assert_eq!(result.details.expect("details")["stdout"], "done");
}

#[tokio::test]
async fn bash_foreground_reports_missing_cd_promptly() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        registry.run(
            "Bash",
            &context,
            json!({
                "command": "cd /definitely/not/a/neo/workspace && printf nope",
            }),
        ),
    )
    .await
    .expect("missing cd should return promptly")
    .expect("foreground bash should return command output");
    let details = result.details.expect("details");

    assert_eq!(details["exit_code"], 1);
    assert!(
        details["stderr"]
            .as_str()
            .unwrap_or_default()
            .contains("No such file")
    );
}

#[tokio::test]
async fn bash_foreground_details_do_not_leak_output_past_max_output_bytes() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = registry
        .run(
            "Bash",
            &context,
            json!({
                "command": "printf 'keep-secret-leak-tail'",
                "max_output_bytes": 4
            }),
        )
        .await
        .expect("foreground bash should run");
    let serialized = serde_json::to_string(&result).expect("result serializes");

    assert!(result.content.contains("[output truncated]"));
    assert!(!result.content.contains("secret-leak-tail"));
    assert!(!serialized.contains("secret-leak-tail"));
    let details = result.details.as_ref().expect("details");
    assert_eq!(details["stdout"], "keep");
    assert_eq!(details["stdout_truncated"], true);
}

#[tokio::test]
async fn task_output_details_do_not_leak_output_past_max_output_bytes() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let started = registry
        .run(
            "Bash",
            &context,
            json!({
                "command": "printf 'keep-background-leak-tail'",
                "run_in_background": true,
                "description": "truncated output",
                "max_output_bytes": 4
            }),
        )
        .await
        .expect("background bash should start");
    let task_id = started.details.as_ref().expect("start details")["task_id"]
        .as_str()
        .expect("task id")
        .to_owned();

    let result = registry
        .run(
            "TaskOutput",
            &context,
            json!({ "task_id": task_id, "block": true, "timeout": 1, "max_output_bytes": 4 }),
        )
        .await
        .expect("TaskOutput should finish");
    let serialized = serde_json::to_string(&result).expect("result serializes");

    assert!(result.content.contains("[output truncated]"));
    assert!(!result.content.contains("background-leak-tail"));
    assert!(!serialized.contains("background-leak-tail"));
    let details = result.details.expect("details");
    assert_eq!(details["stdout"], "keep");
    assert_eq!(details["stdout_truncated"], true);
}

#[tokio::test]
async fn bash_foreground_kills_child_when_cancel_token_is_cancelled() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let cancel = CancellationToken::new();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all())
        .with_cancel_token(cancel.clone());

    let command = tokio::spawn(async move {
        registry
            .run(
                "Bash",
                &context,
                json!({
                    "command": "printf $$ > child.pid; sleep 5",
                }),
            )
            .await
    });
    let pid_path = workspace.path().join("child.pid");
    for _ in 0..20 {
        if pid_path.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let pid = std::fs::read_to_string(&pid_path)
        .expect("child pid should be written")
        .trim()
        .to_owned();
    cancel.cancel();

    let error = tokio::time::timeout(std::time::Duration::from_secs(1), command)
        .await
        .expect("cancelled foreground command should finish promptly")
        .expect("command task should not panic")
        .expect_err("cancelled command should return a tool error");

    assert!(matches!(error, ToolError::Cancelled));
    for _ in 0..20 {
        if !process_exists(&pid) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        !process_exists(&pid),
        "cancel should terminate the child shell process"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn bash_foreground_cancellation_kills_descendant_process_group() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let cancel = CancellationToken::new();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all())
        .with_cancel_token(cancel.clone());

    let command = tokio::spawn(async move {
        registry
            .run(
                "Bash",
                &context,
                json!({
                    "command": "sleep 5 & echo $! > descendant.pid; wait",
                }),
            )
            .await
    });
    let descendant_pid_path = workspace.path().join("descendant.pid");
    let descendant_pid = wait_for_pid_file(&descendant_pid_path).await;
    cancel.cancel();

    let error = tokio::time::timeout(std::time::Duration::from_secs(1), command)
        .await
        .expect("cancelled foreground command should finish promptly")
        .expect("command task should not panic")
        .expect_err("cancelled command should return a tool error");
    assert!(matches!(error, ToolError::Cancelled));

    let descendant_exited = wait_for_process_exit(&descendant_pid).await;
    if !descendant_exited {
        terminate_process(&descendant_pid).await;
    }
    assert!(
        descendant_exited,
        "cancel should terminate descendant processes in the shell process group"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn task_stop_kills_descendant_process_group() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let started = registry
        .run(
            "Bash",
            &context,
            json!({
                "command": "sleep 5 & echo $! > background-descendant.pid; wait",
                "run_in_background": true,
                "description": "sleep with descendant",
                "max_output_bytes": 64
            }),
        )
        .await
        .expect("background bash should start");
    let task_id = started.details.as_ref().expect("start details")["task_id"]
        .as_str()
        .expect("task id")
        .to_owned();

    let descendant_pid_path = workspace.path().join("background-descendant.pid");
    let descendant_pid = wait_for_pid_file(&descendant_pid_path).await;

    let stopped = registry
        .run(
            "TaskStop",
            &context,
            json!({ "task_id": task_id, "max_output_bytes": 64 }),
        )
        .await;
    if stopped.is_err() {
        terminate_process(&descendant_pid).await;
    }
    let stopped = stopped.expect("TaskStop should succeed");
    let stopped_details = stopped.details.as_ref().expect("stop details");
    assert_eq!(stopped_details["status"], "cancelled");

    let output_after_stop = registry
        .run("TaskOutput", &context, json!({ "task_id": task_id }))
        .await
        .expect("TaskOutput should retain cancelled task");
    let output_details = output_after_stop.details.expect("output details");
    assert_eq!(output_details["status"], "cancelled");

    let descendant_exited = wait_for_process_exit(&descendant_pid).await;
    if !descendant_exited {
        terminate_process(&descendant_pid).await;
    }
    assert!(
        descendant_exited,
        "TaskStop should terminate descendant processes in the shell process group"
    );
}

#[cfg(unix)]
async fn wait_for_pid_file(path: &std::path::Path) -> String {
    for _ in 0..100 {
        if let Ok(pid) = std::fs::read_to_string(path) {
            let pid = pid.trim();
            if !pid.is_empty() {
                return pid.to_owned();
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("pid file should be written: {}", path.display());
}

#[cfg(unix)]
async fn wait_for_process_exit(pid: &str) -> bool {
    for _ in 0..100 {
        if !process_exists(pid) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    !process_exists(pid)
}

#[cfg(unix)]
async fn terminate_process(pid: &str) {
    let _ = std::process::Command::new("kill")
        .args(["-TERM", pid])
        .stderr(std::process::Stdio::null())
        .status();
    if !wait_for_process_exit(pid).await {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", pid])
            .stderr(std::process::Stdio::null())
            .status();
        let _ = wait_for_process_exit(pid).await;
    }
}

#[tokio::test]
async fn bash_background_start_includes_task_id_and_next_steps() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let started = registry
        .run(
            "Bash",
            &context,
            json!({
                "command": "sleep 1",
                "run_in_background": true,
                "description": "next-step test"
            }),
        )
        .await
        .expect("background bash should start");

    assert!(started.content.contains("task_id:"));
    assert!(started.content.contains("next_step:"));
    assert!(started.content.contains("TaskOutput"));
    assert!(started.content.contains("TaskStop"));
    assert!(started.content.contains("automatic_notification: true"));

    let details = started.details.expect("start details");
    assert_eq!(details["status"], "running");
    assert_eq!(details["description"], "next-step test");
    assert!(details["next_steps"].is_array());

    let _ = registry
        .run(
            "TaskStop",
            &context,
            json!({ "task_id": details["task_id"] }),
        )
        .await;
}

fn process_exists(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
