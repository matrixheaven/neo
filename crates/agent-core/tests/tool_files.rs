use neo_agent_core::{PermissionPolicy, ToolContext, ToolRegistry};
use serde_json::json;

#[tokio::test]
async fn file_tools_read_search_write_and_edit_inside_workspace() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all());

    registry
        .run(
            "write",
            &context,
            json!({ "path": "src/lib.txt", "content": "alpha\nbeta\nalphabet\n" }),
        )
        .await
        .expect("write");

    let read = registry
        .run("read", &context, json!({ "path": "src/lib.txt" }))
        .await
        .expect("read");
    assert_eq!(read.content, "alpha\nbeta\nalphabet\n");

    let listed = registry
        .run("list", &context, json!({ "path": "." }))
        .await
        .expect("list");
    assert!(listed.content.contains("src/"));

    let found = registry
        .run("find", &context, json!({ "path": ".", "pattern": "lib" }))
        .await
        .expect("find");
    assert!(found.content.contains("src/lib.txt"));

    let grep = registry
        .run(
            "grep",
            &context,
            json!({ "path": ".", "pattern": "alpha", "limit": 2 }),
        )
        .await
        .expect("grep");
    assert!(grep.content.contains("src/lib.txt:1:alpha"));
    assert!(grep.content.contains("src/lib.txt:3:alphabet"));

    let edit = registry
        .run(
            "edit",
            &context,
            json!({ "path": "src/lib.txt", "old": "beta", "new": "gamma" }),
        )
        .await
        .expect("edit");
    assert!(!edit.is_error);
    let details = edit.details.expect("edit details");
    assert_eq!(details["path"], "src/lib.txt");
    assert_eq!(details["old"], "beta");
    assert_eq!(details["new"], "gamma");
    assert_eq!(details["replace_all"], false);
    assert_eq!(
        details["diff"],
        "--- src/lib.txt\n+++ src/lib.txt\n@@\n alpha\n-beta\n+gamma\n alphabet\n"
    );

    let updated = std::fs::read_to_string(workspace.path().join("src/lib.txt")).expect("updated");
    assert_eq!(updated, "alpha\ngamma\nalphabet\n");
}
