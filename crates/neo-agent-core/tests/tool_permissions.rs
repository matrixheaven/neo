use std::time::Duration;

use neo_agent_core::{BackgroundTaskManager, ToolAccess, ToolContext, ToolError, ToolRegistry};
use serde_json::json;

#[tokio::test]
async fn write_requires_mutation_permission() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess {
            file_read: true,
            file_write: false,
            shell: false,
            tool: true,
            user_question: false,
        });

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
async fn read_allows_absolute_paths_outside_workspace() {
    let workspace = tempfile::tempdir().expect("workspace");
    let outside = tempfile::tempdir().expect("outside");
    let outside_file = outside.path().join("note.txt");
    std::fs::write(&outside_file, "external content").expect("write outside file");

    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess {
            file_read: true,
            file_write: false,
            shell: false,
            tool: false,
            user_question: false,
        });

    let result = registry
        .run("Read", &context, json!({ "path": outside_file }))
        .await
        .expect("outside read should be allowed");

    assert!(result.content.contains("external content"));
}

#[tokio::test]
async fn read_allows_symlink_to_external_file() {
    let workspace = tempfile::tempdir().expect("workspace");
    let outside = tempfile::tempdir().expect("outside");
    let outside_file = outside.path().join("note.txt");
    let symlink = workspace.path().join("external-link.txt");
    std::fs::write(&outside_file, "external content").expect("write outside file");
    std::os::unix::fs::symlink(&outside_file, &symlink).expect("symlink");

    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess {
            file_read: true,
            file_write: false,
            shell: false,
            tool: false,
            user_question: false,
        });

    let result = registry
        .run("Read", &context, json!({ "path": "external-link.txt" }))
        .await
        .expect("symlink to external file should be allowed");

    assert!(result.content.contains("external content"));
}

#[tokio::test]
async fn bash_requires_permission_and_honors_timeout() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let denied_context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess {
            file_read: true,
            file_write: false,
            shell: false,
            tool: true,
            user_question: false,
        });

    let denied = registry
        .run("Bash", &denied_context, json!({ "command": "echo denied" }))
        .await
        .expect_err("bash should be denied");
    assert!(matches!(denied, ToolError::PermissionDenied { .. }));

    let allowed_context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all())
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

#[tokio::test]
async fn task_stop_rejects_question_without_shell_permission() {
    let workspace = tempfile::tempdir().expect("workspace");
    let background_tasks = BackgroundTaskManager::new();
    background_tasks
        .start_question("question-permission".to_owned(), "Pick one".to_owned())
        .await;

    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess {
            file_read: true,
            file_write: false,
            shell: false,
            tool: true,
            user_question: false,
        })
        .with_background_tasks(background_tasks);

    let error = registry
        .run(
            "TaskStop",
            &context,
            json!({ "task_id": "question-permission" }),
        )
        .await
        .expect_err("TaskStop should require shell permission");

    assert!(matches!(
        error,
        ToolError::PermissionDenied { operation: "shell" }
    ));
}

#[tokio::test]
async fn task_stop_can_cancel_question_with_shell_permission() {
    let workspace = tempfile::tempdir().expect("workspace");
    let background_tasks = BackgroundTaskManager::new();
    background_tasks
        .start_question("question-permission".to_owned(), "Pick one".to_owned())
        .await;

    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all())
        .with_background_tasks(background_tasks);

    let stopped = registry
        .run(
            "TaskStop",
            &context,
            json!({ "task_id": "question-permission" }),
        )
        .await
        .expect("TaskStop should cancel question after shell approval");

    assert!(stopped.content.contains("status: cancelled"));
    assert_eq!(stopped.details.as_ref().unwrap()["kind"], "question");
}

#[tokio::test]
async fn task_stop_requires_shell_permission_by_default() {
    let workspace = tempfile::tempdir().expect("workspace");
    let background_tasks = BackgroundTaskManager::new();
    background_tasks
        .start_question("question-needs-approval".to_owned(), "Pick one".to_owned())
        .await;

    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess {
            file_read: true,
            file_write: false,
            shell: false,
            tool: true,
            user_question: false,
        })
        .with_background_tasks(background_tasks);

    let error = registry
        .run(
            "TaskStop",
            &context,
            json!({ "task_id": "question-needs-approval" }),
        )
        .await
        .expect_err("TaskStop should require shell approval by default");

    assert!(matches!(
        error,
        ToolError::PermissionDenied { operation: "shell" }
    ));
}
