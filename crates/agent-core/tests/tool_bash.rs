use neo_agent_core::{PermissionPolicy, ToolContext, ToolError, ToolRegistry};
use serde_json::json;
use tokio_util::sync::CancellationToken;

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

fn process_exists(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
