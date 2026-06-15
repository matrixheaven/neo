use neo_agent_core::{
    DEFAULT_BASH_TIMEOUT, PermissionPolicy, ToolContext, ToolError, ToolRegistry,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[test]
fn bash_default_timeout_allows_long_workspace_commands() {
    let workspace = tempfile::tempdir().expect("workspace");
    let context = ToolContext::new(workspace.path()).expect("context");

    assert_eq!(context.bash_timeout, DEFAULT_BASH_TIMEOUT);
    assert_eq!(
        context.bash_timeout,
        std::time::Duration::from_secs(10 * 60)
    );
}

#[tokio::test]
async fn bash_background_start_poll_and_finish_returns_real_process_output() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all());

    let started = registry
        .run(
            "bash",
            &context,
            json!({
                "mode": "start",
                "command": "printf started; sleep 0.05; printf done",
                "max_output_bytes": 64
            }),
        )
        .await
        .expect("background start should succeed");
    let start_details = started.details.as_ref().expect("start details");
    let handle = start_details
        .get("handle")
        .and_then(serde_json::Value::as_str)
        .expect("start should return a handle");
    assert!(!handle.is_empty());
    assert_eq!(start_details["status"], "running");

    let running = registry
        .run(
            "bash",
            &context,
            json!({ "mode": "poll", "handle": handle, "max_output_bytes": 64 }),
        )
        .await
        .expect("background poll should succeed");
    let running_details = running.details.as_ref().expect("running details");
    assert_eq!(running_details["handle"], handle);
    assert!(matches!(
        running_details["status"].as_str(),
        Some("running" | "exited")
    ));

    let mut finished_details = running_details.clone();
    for _ in 0..20 {
        if finished_details["status"] == "exited" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let polled = registry
            .run(
                "bash",
                &context,
                json!({ "mode": "poll", "handle": handle, "max_output_bytes": 64 }),
            )
            .await
            .expect("background poll should succeed");
        finished_details = polled.details.expect("poll details");
    }

    assert_eq!(finished_details["status"], "exited");
    assert_eq!(finished_details["exit_code"], 0);
    assert_eq!(finished_details["stdout"], "starteddone");
    assert_eq!(finished_details["stderr"], "");
    assert_eq!(finished_details["truncated"], false);
}

#[tokio::test]
async fn bash_background_handles_are_removed_after_finished_poll() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all());

    let started = registry
        .run(
            "bash",
            &context,
            json!({ "mode": "start", "command": "printf once" }),
        )
        .await
        .expect("background start should succeed");
    let handle = started.details.as_ref().expect("start details")["handle"]
        .as_str()
        .expect("handle")
        .to_owned();

    for _ in 0..20 {
        let polled = registry
            .run(
                "bash",
                &context,
                json!({ "mode": "poll", "handle": handle }),
            )
            .await
            .expect("poll should succeed");
        if polled.details.expect("poll details")["status"] == "exited" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let missing = registry
        .run(
            "bash",
            &context,
            json!({ "mode": "poll", "handle": handle }),
        )
        .await
        .expect_err("finished handle should be removed");
    assert!(matches!(missing, ToolError::InvalidInput { .. }));
}

#[tokio::test]
async fn bash_without_mode_remains_foreground_compatible() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all());

    let result = registry
        .run("bash", &context, json!({ "command": "printf foreground" }))
        .await
        .expect("foreground bash should still run");

    assert_eq!(result.details.expect("details")["stdout"], "foreground");
}

#[tokio::test]
async fn bash_foreground_details_do_not_leak_output_past_max_output_bytes() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all());

    let result = registry
        .run(
            "bash",
            &context,
            json!({
                "command": "printf 'keep-secret-leak-tail'",
                "max_output_bytes": 4
            }),
        )
        .await
        .expect("foreground bash should run");
    let serialized = serde_json::to_string(&result).expect("result serializes");

    assert!(result.content.contains("truncated: true"));
    assert!(!result.content.contains("secret-leak-tail"));
    assert!(!serialized.contains("secret-leak-tail"));
    let details = result.details.expect("details");
    assert_eq!(details["stdout"], "keep");
    assert_eq!(details["stdout_truncated"], true);
}

#[tokio::test]
async fn bash_background_details_do_not_leak_output_past_max_output_bytes() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all());

    let started = registry
        .run(
            "bash",
            &context,
            json!({
                "mode": "start",
                "command": "printf 'keep-background-leak-tail'",
                "max_output_bytes": 4
            }),
        )
        .await
        .expect("background start should succeed");
    let handle = started.details.as_ref().expect("start details")["handle"]
        .as_str()
        .expect("handle")
        .to_owned();

    let mut result = None;
    for _ in 0..20 {
        let polled = registry
            .run(
                "bash",
                &context,
                json!({ "mode": "poll", "handle": handle, "max_output_bytes": 4 }),
            )
            .await
            .expect("background poll should succeed");
        if polled.details.as_ref().expect("poll details")["status"] == "exited" {
            result = Some(polled);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let result = result.expect("background command should exit");
    let serialized = serde_json::to_string(&result).expect("result serializes");

    assert!(result.content.contains("truncated: true"));
    assert!(!result.content.contains("background-leak-tail"));
    assert!(!serialized.contains("background-leak-tail"));
    let details = result.details.expect("details");
    assert_eq!(details["stdout"], "keep");
    assert_eq!(details["stdout_truncated"], true);
}

#[tokio::test]
async fn bash_foreground_kills_child_when_context_is_cancelled() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let cancel = CancellationToken::new();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all())
        .with_cancel_token(cancel.clone());

    let command = tokio::spawn(async move {
        registry
            .run(
                "bash",
                &context,
                json!({
                    "command": "printf $$ > child.pid; sleep 5",
                    "timeout_ms": 10000
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
        .with_permission_policy(PermissionPolicy::allow_all())
        .with_cancel_token(cancel.clone());

    let command = tokio::spawn(async move {
        registry
            .run(
                "bash",
                &context,
                json!({
                    "command": "sleep 5 & echo $! > descendant.pid; wait",
                    "timeout_ms": 10000
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
async fn bash_background_stop_kills_descendant_process_group_and_removes_handle() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all());

    let started = registry
        .run(
            "bash",
            &context,
            json!({
                "mode": "start",
                "command": "sleep 5 & echo $! > background-descendant.pid; wait",
                "max_output_bytes": 64
            }),
        )
        .await
        .expect("background start should succeed");
    let handle = started.details.as_ref().expect("start details")["handle"]
        .as_str()
        .expect("handle")
        .to_owned();

    let descendant_pid_path = workspace.path().join("background-descendant.pid");
    let descendant_pid = wait_for_pid_file(&descendant_pid_path).await;

    let stopped = registry
        .run(
            "bash",
            &context,
            json!({ "mode": "stop", "handle": handle, "max_output_bytes": 64 }),
        )
        .await;
    if stopped.is_err() {
        terminate_process(&descendant_pid).await;
        drain_background_handle(&registry, &context, &handle).await;
    }
    let stopped = stopped.expect("background stop should succeed");
    let stopped_details = stopped.details.as_ref().expect("stop details");
    assert_eq!(stopped_details["handle"], handle);
    assert_eq!(stopped_details["status"], "stopped");

    let descendant_exited = wait_for_process_exit(&descendant_pid).await;
    if !descendant_exited {
        terminate_process(&descendant_pid).await;
    }
    assert!(
        descendant_exited,
        "background stop should terminate descendant processes in the shell process group"
    );

    let missing = registry
        .run(
            "bash",
            &context,
            json!({ "mode": "poll", "handle": handle }),
        )
        .await
        .expect_err("stopped handle should be removed");
    assert!(matches!(missing, ToolError::InvalidInput { .. }));
}

#[cfg(unix)]
async fn drain_background_handle(registry: &ToolRegistry, context: &ToolContext, handle: &str) {
    for _ in 0..20 {
        let Ok(polled) = registry
            .run("bash", context, json!({ "mode": "poll", "handle": handle }))
            .await
        else {
            break;
        };
        if polled
            .details
            .as_ref()
            .and_then(|details| details["status"].as_str())
            == Some("exited")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
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

fn process_exists(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
