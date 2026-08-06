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
async fn case_insensitive_search() {
    let workspace = setup_workspace();
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = run_grep(
        &ctx,
        "hello",
        json!({ "output_mode": "content", "-i": true }),
    )
    .await;
    assert!(result.content.contains("bar.txt:1:hello world"));
    assert!(result.content.contains("bar.txt:2:Hello World"));
}

#[tokio::test]
async fn glob_filter_restricts_files() {
    let workspace = setup_workspace();
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = run_grep(
        &ctx,
        "fn",
        json!({ "output_mode": "files_with_matches", "glob": "*.txt" }),
    )
    .await;
    assert!(!result.content.contains("foo.rs"));
    assert!(!result.content.contains("baz.rs"));
    assert!(!result.content.contains("sub/qux.rs"));
    assert!(result.content.contains("No matches found"));
}

#[tokio::test]
async fn type_filter_restricts_by_extension() {
    let workspace = setup_workspace();
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = run_grep(
        &ctx,
        "fn",
        json!({ "output_mode": "files_with_matches", "type": "rs" }),
    )
    .await;
    assert!(result.content.contains("foo.rs"));
    assert!(result.content.contains("baz.rs"));
    assert!(!result.content.contains("bar.txt"));
}

#[tokio::test]
async fn context_lines_group_matches() {
    let workspace = setup_workspace();
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = run_grep(
        &ctx,
        "fn main",
        json!({ "output_mode": "content", "-C": 1 }),
    )
    .await;
    assert!(result.content.contains("foo.rs:1:fn main() {}"));
    assert!(result.content.contains("foo.rs:2:let x = 1;"));
}

#[tokio::test]
async fn head_limit_and_offset_paginate() {
    let workspace = setup_workspace();
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let first = run_grep(
        &ctx,
        "fn",
        json!({ "output_mode": "files_with_matches", "head_limit": 1, "offset": 0 }),
    )
    .await;
    assert_eq!(
        first
            .content
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with("<system>"))
            .count(),
        1
    );
    assert!(first.content.contains("Results truncated"));

    let second = run_grep(
        &ctx,
        "fn",
        json!({ "output_mode": "files_with_matches", "head_limit": 1, "offset": 1 }),
    )
    .await;
    assert_eq!(
        second
            .content
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with("<system>"))
            .count(),
        1
    );
    assert_ne!(first.content, second.content);
}

#[tokio::test]
async fn invalid_regex_returns_error() {
    let workspace = setup_workspace();
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = GrepTool
        .execute(&ctx, json!({ "pattern": "[invalid" }))
        .await;
    assert!(result.is_err());
}
