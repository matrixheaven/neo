use neo_agent_core::{ToolAccess, ToolContext, ToolRegistry};
use serde_json::json;

#[tokio::test]
async fn file_tools_read_search_write_and_edit_inside_workspace() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    registry
        .run(
            "Write",
            &context,
            json!({ "path": "src/lib.txt", "content": "alpha\nbeta\nalphabet\n" }),
        )
        .await
        .expect("Write");

    let read = registry
        .run("Read", &context, json!({ "path": "src/lib.txt" }))
        .await
        .expect("Read");
    assert!(read.content.contains("1\talpha"));
    assert!(read.content.contains("2\tbeta"));
    assert!(read.content.contains("3\talphabet"));
    assert!(read.content.contains("Total lines in file: 3."));

    let listed = registry
        .run("List", &context, json!({ "path": "." }))
        .await
        .expect("List");
    assert!(listed.content.contains("src/"));

    let found = registry
        .run("Find", &context, json!({ "path": ".", "pattern": "lib" }))
        .await
        .expect("Find");
    assert!(found.content.contains("src/lib.txt"));

    let grep = registry
        .run(
            "Grep",
            &context,
            json!({ "path": ".", "pattern": "alpha", "head_limit": 2, "output_mode": "content" }),
        )
        .await
        .expect("Grep");
    assert!(grep.content.contains("src/lib.txt:1:alpha"));
    assert!(grep.content.contains("src/lib.txt:3:alphabet"));

    let edit = registry
        .run(
            "Edit",
            &context,
            json!({ "path": "src/lib.txt", "old": "beta", "new": "gamma" }),
        )
        .await
        .expect("Edit");
    assert!(!edit.is_error);
    let details = edit.details.expect("edit details");
    assert_eq!(details["path"], "src/lib.txt");
    assert_eq!(details["old"], "beta");
    assert_eq!(details["new"], "gamma");
    assert_eq!(details["replace_all"], false);
    assert_eq!(
        details["diff"],
        "--- src/lib.txt\n+++ src/lib.txt\n@@ -1,3 +1,3 @@\n alpha\n-beta\n+gamma\n alphabet\n"
    );

    let updated = std::fs::read_to_string(workspace.path().join("src/lib.txt")).expect("updated");
    assert_eq!(updated, "alpha\ngamma\nalphabet\n");
}

#[tokio::test]
async fn write_tool_returns_created_file_diff_details() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let write = registry
        .run(
            "Write",
            &context,
            json!({ "path": "notes/list.txt", "content": "one\ntwo\n" }),
        )
        .await
        .expect("Write");

    assert!(!write.is_error);
    let details = write.details.expect("write details");
    assert_eq!(details["path"], "notes/list.txt");
    assert_eq!(details["operation"], "created");
    assert_eq!(details["added"], 2);
    assert_eq!(details["removed"], 0);
    assert_eq!(details["line_count"], 2);
    assert_eq!(
        details["diff"],
        "--- notes/list.txt\n+++ notes/list.txt\n@@ -0,0 +1,2 @@\n+one\n+two\n"
    );
}

#[tokio::test]
async fn write_tool_returns_overwritten_file_diff_details() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    registry
        .run(
            "Write",
            &context,
            json!({ "path": "notes/list.txt", "content": "one\ntwo\nthree\n" }),
        )
        .await
        .expect("initial Write");

    let write = registry
        .run(
            "Write",
            &context,
            json!({ "path": "notes/list.txt", "content": "one\nTWO\nthree\nfour\n" }),
        )
        .await
        .expect("overwrite Write");

    assert!(!write.is_error);
    let details = write.details.expect("write details");
    assert_eq!(details["path"], "notes/list.txt");
    assert_eq!(details["operation"], "overwritten");
    assert_eq!(details["added"], 2);
    assert_eq!(details["removed"], 1);
    assert_eq!(details["line_count"], 4);
    assert_eq!(
        details["diff"],
        "--- notes/list.txt\n+++ notes/list.txt\n@@ -1,3 +1,4 @@\n one\n-two\n+TWO\n three\n+four\n"
    );
}
