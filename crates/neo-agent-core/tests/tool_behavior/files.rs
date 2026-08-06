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
async fn edit_match_mismatch_reports_path_and_writes_nothing() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = neo_agent_core::ToolRegistry::with_builtin_tools();
    let context = neo_agent_core::ToolContext::new(workspace.path())
        .expect("context")
        .with_access(neo_agent_core::ToolAccess::all());

    std::fs::create_dir_all(workspace.path().join("src")).expect("mkdir");
    let b = workspace.path().join("src/b.txt");
    std::fs::write(&b, "bbb bbb\n").expect("seed b");

    let edit = registry
        .run(
            "Edit",
            &context,
            json!({ "path": "src/b.txt", "old": "bbb", "new": "BBB" }),
        )
        .await
        .expect("Edit result");

    assert!(edit.is_error);
    let details = edit.details.expect("details");
    assert_eq!(details["status"], "prepare_failed");
    assert!(details["edit_index"].is_null());
    assert!(details["file_index"].is_null());
    assert_eq!(details["path"], "src/b.txt");
    let content = &edit.content;
    assert!(content.contains("expected 1 exact matches"), "{content}");
    assert!(content.contains("found 2"), "{content}");
    assert!(content.contains("matches at lines"), "{content}");
    assert!(content.contains("make old more specific"), "{content}");
    assert!(content.contains("set expected_matches to 2"), "{content}");
    assert!(content.contains("smallest ranges"), "{content}");
    assert!(!content.contains("bbb bbb"), "{content}");
    assert!(!content.contains("Comparison snapshot"), "{content}");
    assert_eq!(content.lines().count(), 3, "{content}");
    assert_eq!(std::fs::read_to_string(&b).expect("b"), "bbb bbb\n");
}

#[tokio::test]
async fn edit_match_mismatch_returns_compact_recovery_guidance() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = neo_agent_core::ToolRegistry::with_builtin_tools();
    let context = neo_agent_core::ToolContext::new(workspace.path())
        .expect("context")
        .with_access(neo_agent_core::ToolAccess::all());
    let path = workspace.path().join("sample.txt");
    let original = "alpha\nbeta\n";
    std::fs::write(&path, original).expect("seed");

    let edit = registry
        .run(
            "Edit",
            &context,
            json!({ "path": "sample.txt", "old": "missing\n", "new": "replacement\n" }),
        )
        .await
        .expect("Edit result");

    assert!(edit.is_error);
    assert!(edit.content.contains("found 0"), "{}", edit.content);
    assert!(
        edit.content
            .contains("Grep on a distinctive fragment or Read the smallest relevant range"),
        "{}",
        edit.content
    );
    assert!(!edit.content.contains("alpha"), "{}", edit.content);
    assert!(
        !edit.content.contains("Comparison snapshot"),
        "{}",
        edit.content
    );
    assert_eq!(edit.content.lines().count(), 3, "{}", edit.content);
    assert_eq!(std::fs::read_to_string(path).expect("read"), original);
}

#[tokio::test]
async fn edit_single_file_contract_is_model_visible_and_strict() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = neo_agent_core::ToolRegistry::with_builtin_tools();
    let context = neo_agent_core::ToolContext::new(workspace.path())
        .expect("context")
        .with_access(neo_agent_core::ToolAccess::all());

    std::fs::create_dir_all(workspace.path().join("src")).expect("mkdir");
    std::fs::write(workspace.path().join("src/real.txt"), "hello\n").expect("seed");

    let spec = registry
        .specs()
        .into_iter()
        .find(|s| s.name == "Edit")
        .expect("Edit spec");
    let schema = &spec.input_schema;
    let properties = &schema["properties"];
    assert!(
        properties["path"].is_object(),
        "root must have path: {schema}"
    );
    assert!(
        properties["old"].is_object(),
        "root must have old: {schema}"
    );
    assert!(
        properties["new"].is_object(),
        "root must have new: {schema}"
    );
    assert!(
        properties["expected_matches"].is_object(),
        "root must have expected_matches: {schema}"
    );
    assert!(
        properties["edits"].is_null(),
        "root must not have edits: {schema}"
    );
    assert!(
        properties["files"].is_null(),
        "root must not have files: {schema}"
    );

    let old_array = registry
        .run(
            "Edit",
            &context,
            json!({
                "edits": [{ "path": "src/real.txt", "old": "hello", "new": "hi" }]
            }),
        )
        .await
        .expect("old array result");
    assert!(old_array.is_error);
    assert_eq!(
        old_array.details.expect("old array details")["status"],
        "prepare_failed"
    );

    let edit = registry
        .run(
            "Edit",
            &context,
            json!({ "path": "src/real.txt", "old": "hello", "new": "hi" }),
        )
        .await
        .expect("single edit");
    assert!(!edit.is_error);

    assert_eq!(
        std::fs::read_to_string(workspace.path().join("src/real.txt")).expect("real"),
        "hi\n"
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
                json!({ "path": "src/link.txt", "old": "hi", "new": "hello" }),
            )
            .await
            .expect("link result");
        assert!(link_edit.is_error);
        let details = link_edit.details.expect("details");
        assert_eq!(details["status"], "prepare_failed");
        assert_eq!(std::fs::read_to_string(&target).expect("target"), "hi\n");
    }
}

#[tokio::test]
async fn write_single_file_contract_is_model_visible_and_strict() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let spec = registry
        .specs()
        .into_iter()
        .find(|spec| spec.name == "Write")
        .expect("Write spec");
    let properties = &spec.input_schema["properties"];
    assert!(properties["path"].is_object());
    assert!(properties["content"].is_object());
    assert!(properties["files"].is_null());

    let old_array = registry
        .run(
            "Write",
            &context,
            json!({ "files": [{ "path": "src/a.rs", "content": "old\n" }] }),
        )
        .await
        .expect("old array result");
    assert!(old_array.is_error);

    let write = registry
        .run(
            "Write",
            &context,
            json!({ "path": "src/a.rs", "content": "fn main() {}\n" }),
        )
        .await
        .expect("Write");

    assert!(!write.is_error);
    let details = write.details.expect("write details");
    assert_eq!(details["kind"], "write");
    assert_eq!(details["status"], "committed");
    assert_eq!(details["files"], 1);
    assert_eq!(details["created"], 1);
    assert_eq!(details["overwritten"], 0);
    assert_eq!(details["changes"][0]["path"], "src/a.rs");
    assert_eq!(details["changes"][0]["operation"], "created");
    assert_eq!(details["changes"][0]["status"], "committed");
    assert!(details["changes"][0]["content"].is_string());

    assert_eq!(
        std::fs::read_to_string(workspace.path().join("src/a.rs")).expect("a"),
        "fn main() {}\n"
    );
}
