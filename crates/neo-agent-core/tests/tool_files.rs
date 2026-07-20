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
            json!({
                "files": [{
                    "path": "src/lib.txt",
                    "replacements": [{ "old": "beta", "new": "gamma" }]
                }]
            }),
        )
        .await
        .expect("Edit");
    assert!(!edit.is_error);
    let details = edit.details.expect("edit details");
    assert_eq!(details["kind"], "edit");
    assert_eq!(details["status"], "committed");
    assert_eq!(details["files"], 1);
    assert_eq!(details["replacements"], 1);
    assert_eq!(details["changes"][0]["path"], "src/lib.txt");
    assert_eq!(details["changes"][0]["status"], "committed");
    assert_eq!(
        details["changes"][0]["diff"],
        "--- src/lib.txt\n+++ src/lib.txt\n@@ -1,3 +1,3 @@\n alpha\n-beta\n+gamma\n alphabet\n"
    );

    let updated = std::fs::read_to_string(workspace.path().join("src/lib.txt")).expect("updated");
    assert_eq!(updated, "alpha\ngamma\nalphabet\n");
}

#[tokio::test]
async fn edit_batch_applies_ordered_replacements_across_files() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = neo_agent_core::ToolRegistry::with_builtin_tools();
    let context = neo_agent_core::ToolContext::new(workspace.path())
        .expect("context")
        .with_access(neo_agent_core::ToolAccess::all());

    std::fs::create_dir_all(workspace.path().join("src")).expect("mkdir");
    std::fs::write(workspace.path().join("src/a.txt"), "one two one\nthree\n").expect("seed a");
    std::fs::write(workspace.path().join("src/b.txt"), "alpha\nbeta\n").expect("seed b");

    let edit = registry
        .run(
            "Edit",
            &context,
            json!({
                "files": [
                    {
                        "path": "src/a.txt",
                        "replacements": [
                            { "old": "one", "new": "1", "expected_matches": 2 },
                            { "old": "1 two 1", "new": "1 TWO 1" }
                        ]
                    },
                    {
                        "path": "src/b.txt",
                        "replacements": [
                            { "old": "beta", "new": "BETA" }
                        ]
                    }
                ]
            }),
        )
        .await
        .expect("Edit");

    assert!(!edit.is_error);
    let details = edit.details.expect("details");
    assert_eq!(details["status"], "committed");
    assert_eq!(details["files"], 2);
    assert_eq!(details["replacements"], 3);
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("src/a.txt")).expect("a"),
        "1 TWO 1\nthree\n"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("src/b.txt")).expect("b"),
        "alpha\nBETA\n"
    );
}

#[tokio::test]
async fn edit_batch_prepare_mismatch_writes_nothing() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = neo_agent_core::ToolRegistry::with_builtin_tools();
    let context = neo_agent_core::ToolContext::new(workspace.path())
        .expect("context")
        .with_access(neo_agent_core::ToolAccess::all());

    std::fs::create_dir_all(workspace.path().join("src")).expect("mkdir");
    let a = workspace.path().join("src/a.txt");
    let b = workspace.path().join("src/b.txt");
    std::fs::write(&a, "aaa\n").expect("seed a");
    std::fs::write(&b, "bbb\n").expect("seed b");

    let edit = registry
        .run(
            "Edit",
            &context,
            json!({
                "files": [
                    {
                        "path": "src/a.txt",
                        "replacements": [{ "old": "aaa", "new": "AAA" }]
                    },
                    {
                        "path": "src/b.txt",
                        "replacements": [{ "old": "missing", "new": "x", "expected_matches": 1 }]
                    }
                ]
            }),
        )
        .await
        .expect("Edit result");

    assert!(edit.is_error);
    let details = edit.details.expect("details");
    assert_eq!(details["status"], "prepare_failed");
    assert_eq!(std::fs::read_to_string(&a).expect("a"), "aaa\n");
    assert_eq!(std::fs::read_to_string(&b).expect("b"), "bbb\n");
}

#[tokio::test]
async fn edit_batch_rejects_legacy_schema_and_link_like_targets() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = neo_agent_core::ToolRegistry::with_builtin_tools();
    let context = neo_agent_core::ToolContext::new(workspace.path())
        .expect("context")
        .with_access(neo_agent_core::ToolAccess::all());

    std::fs::create_dir_all(workspace.path().join("src")).expect("mkdir");
    std::fs::write(workspace.path().join("src/real.txt"), "hello\n").expect("seed");

    let legacy = registry
        .run(
            "Edit",
            &context,
            json!({ "path": "src/real.txt", "old": "hello", "new": "hi" }),
        )
        .await
        .expect("legacy result");
    assert!(legacy.is_error);
    let legacy_details = legacy.details.expect("details");
    assert_eq!(legacy_details["status"], "prepare_failed");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("src/real.txt")).expect("real"),
        "hello\n"
    );

    #[cfg(unix)]
    {
        let target = workspace.path().join("src/real.txt");
        let link = workspace.path().join("src/link.txt");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        let link_edit = registry
            .run(
                "Edit",
                &context,
                json!({
                    "files": [{
                        "path": "src/link.txt",
                        "replacements": [{ "old": "hello", "new": "hi" }]
                    }]
                }),
            )
            .await
            .expect("link result");
        assert!(link_edit.is_error);
        let details = link_edit.details.expect("details");
        assert_eq!(details["status"], "prepare_failed");
        assert_eq!(std::fs::read_to_string(&target).expect("target"), "hello\n");
    }
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

#[tokio::test]
async fn write_overwrites_non_utf8_without_diff_preview() {
    let workspace = tempfile::tempdir().expect("workspace");
    let path = workspace.path().join("binary.bin");
    std::fs::write(&path, [0xff, 0xfe]).expect("binary file");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let write = registry
        .run(
            "Write",
            &context,
            json!({ "path": "binary.bin", "content": "replacement\n" }),
        )
        .await
        .expect("Write");

    assert!(!write.is_error);
    assert_eq!(std::fs::read(&path).expect("read file"), b"replacement\n");
    let details = write.details.expect("details");
    assert_eq!(details["operation"], "overwritten");
    assert_eq!(details["diff_available"], false);
    assert!(details.get("diff").is_none());
    assert_eq!(
        details["diff_unavailable_reason"],
        "existing file content is not valid UTF-8"
    );
}
