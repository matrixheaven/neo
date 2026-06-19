use std::time::Duration;

use neo_agent_core::{PermissionPolicy, ToolContext, ToolError, ToolRegistry};
use serde_json::json;

#[tokio::test]
async fn write_requires_mutation_permission() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::read_only());

    let error = registry
        .run(
            "Write",
            &context,
            json!({ "path": "note.txt", "content": "nope" }),
        )
        .await
        .expect_err("write should be denied");

    assert!(matches!(error, ToolError::PermissionDenied { .. }));
    assert!(!workspace.path().join("note.txt").exists());
}

#[tokio::test]
async fn read_rejects_paths_outside_workspace() {
    let workspace = tempfile::tempdir().expect("workspace");
    let outside = tempfile::tempdir().expect("outside");
    let outside_file = outside.path().join("secret.txt");
    std::fs::write(&outside_file, "secret").expect("write outside file");

    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path()).expect("context");

    let error = registry
        .run("Read", &context, json!({ "path": outside_file }))
        .await
        .expect_err("outside read should be denied");

    assert!(matches!(error, ToolError::PathOutsideWorkspace { .. }));
}

#[tokio::test]
async fn read_rejects_symlink_escape_from_workspace() {
    let workspace = tempfile::tempdir().expect("workspace");
    let outside = tempfile::tempdir().expect("outside");
    let outside_file = outside.path().join("secret.txt");
    let symlink = workspace.path().join("secret-link.txt");
    std::fs::write(&outside_file, "secret").expect("write outside file");
    std::os::unix::fs::symlink(&outside_file, &symlink).expect("symlink");

    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path()).expect("context");

    let error = registry
        .run("Read", &context, json!({ "path": "secret-link.txt" }))
        .await
        .expect_err("symlink escape should be denied");

    assert!(matches!(error, ToolError::PathOutsideWorkspace { .. }));
}

#[tokio::test]
async fn bash_requires_permission_and_honors_timeout() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let denied_context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::read_only());

    let denied = registry
        .run("Bash", &denied_context, json!({ "command": "echo denied" }))
        .await
        .expect_err("bash should be denied");
    assert!(matches!(denied, ToolError::PermissionDenied { .. }));

    let allowed_context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all())
        .with_bash_timeout(Duration::from_secs(5));

    let capped = registry
        .run(
            "Bash",
            &allowed_context,
            json!({ "command": "printf 1234567890", "max_output_bytes": 4 }),
        )
        .await
        .expect("bash should run");
    assert!(capped.content.contains("1234"));
    assert!(capped.content.contains("[output truncated]"));

    let timed_out = registry
        .run(
            "Bash",
            &allowed_context,
            json!({ "command": "sleep 1", "timeout": 0 }),
        )
        .await
        .expect_err("bash should time out");
    assert!(matches!(timed_out, ToolError::CommandTimedOut { .. }));
}
