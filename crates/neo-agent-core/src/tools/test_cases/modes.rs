use super::*;
use crate::ToolAccess;
use crate::ToolContext;
use serde_json::json;

fn setup_workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("foo.rs"), "fn main() {}\nlet x = 1;\n").expect("write foo.rs");
    std::fs::write(dir.path().join("bar.txt"), "hello world\nHello World\n")
        .expect("write bar.txt");
    std::fs::write(dir.path().join("baz.rs"), "// baz\nfn foo() {}\n").expect("write baz.rs");
    std::fs::create_dir_all(dir.path().join("sub")).expect("mkdir sub");
    std::fs::write(
        dir.path().join("sub").join("qux.rs"),
        "// sub\nfn qux() {}\n",
    )
    .expect("write qux.rs");
    dir
}

async fn run_grep(ctx: &ToolContext, pattern: &str, extra: serde_json::Value) -> ToolResult {
    let mut input = json!({ "pattern": pattern });
    if let Some(obj) = input.as_object_mut()
        && let serde_json::Value::Object(extra_obj) = extra
    {
        for (k, v) in extra_obj {
            obj.insert(k, v);
        }
    }
    GrepTool.execute(ctx, input).await.expect("execute")
}

#[tokio::test]
async fn content_mode_returns_matching_lines() {
    let workspace = setup_workspace();
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = run_grep(&ctx, "fn main", json!({ "output_mode": "content" })).await;
    assert!(result.content.contains("foo.rs:1:fn main() {}"));
    assert!(result.content.contains("Found 1 matching line"));
    assert!(result.content.contains("<system>"));
}

#[tokio::test]
async fn files_with_matches_is_default() {
    let workspace = setup_workspace();
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = run_grep(&ctx, "fn", json!({})).await;
    assert!(result.content.contains("foo.rs"));
    assert!(result.content.contains("baz.rs"));
    assert!(result.content.contains("sub/qux.rs"));
    assert!(!result.content.contains(':'));
    assert!(result.content.contains("Found 3 files with matches"));
}

#[tokio::test]
async fn count_matches_mode() {
    let workspace = setup_workspace();
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = run_grep(&ctx, "fn", json!({ "output_mode": "count_matches" })).await;
    assert!(result.content.contains("foo.rs:1"));
    assert!(result.content.contains("baz.rs:1"));
    assert!(result.content.contains("sub/qux.rs:1"));
}

#[tokio::test]
async fn line_numbers_false_omits_numbers() {
    let workspace = setup_workspace();
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = run_grep(
        &ctx,
        "fn main",
        json!({ "output_mode": "content", "-n": false }),
    )
    .await;
    assert!(result.content.contains("foo.rs:fn main() {}"));
    assert!(!result.content.contains("foo.rs:1:fn main() {}"));
}

#[tokio::test]
async fn multiline_content_reports_line_numbers_for_multiple_matches() {
    let workspace = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        workspace.path().join("multi.txt"),
        "alpha\nfirst match\nbeta\ngamma\nsecond match\n",
    )
    .expect("write multi.txt");
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = run_grep(
        &ctx,
        "match",
        json!({ "output_mode": "content", "multiline": true }),
    )
    .await;

    assert!(result.content.contains("multi.txt:2:match"));
    assert!(result.content.contains("multi.txt:5:match"));
}

#[test]
fn match_line_numbers_advance_from_previous_match_offset() {
    let content = "one\ntwo\nthree\nfour\n";
    let mut line_numbers = MatchLineNumbers::new();

    assert_eq!(line_numbers.line_number_at(content, 0), 1);
    assert_eq!(line_numbers.line_number_at(content, 8), 3);
    assert_eq!(line_numbers.line_number_at(content, 14), 4);
}

#[tokio::test]
async fn count_summary_is_accurate() {
    let workspace = setup_workspace();
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = run_grep(&ctx, "fn", json!({ "output_mode": "count_matches" })).await;
    // foo.rs has 1, baz.rs has 1, sub/qux.rs has 1 => 3 total
    assert!(
        result
            .content
            .contains("Found 3 occurrences across 3 files")
    );
}
