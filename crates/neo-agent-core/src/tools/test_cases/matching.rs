use super::*;
use crate::ToolAccess;
use crate::ToolContext;
use serde_json::json;

/// Create a temporary workspace with a known file layout:
///
/// ```text
/// foo.rs
/// bar.txt
/// baz.toml
/// sub/qux.rs
/// sub/deep/inner.rs
/// ```
fn setup_workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("foo.rs"), "fn main() {}").expect("write foo.rs");
    std::fs::write(dir.path().join("bar.txt"), "hello").expect("write bar.txt");
    std::fs::write(dir.path().join("baz.toml"), "[package]").expect("write baz.toml");
    std::fs::create_dir_all(dir.path().join("sub")).expect("mkdir sub");
    std::fs::write(dir.path().join("sub").join("qux.rs"), "// sub").expect("write qux.rs");
    std::fs::create_dir_all(dir.path().join("sub/deep")).expect("mkdir sub/deep");
    std::fs::write(dir.path().join("sub/deep").join("inner.rs"), "// deep")
        .expect("write inner.rs");
    dir
}

async fn run_glob(
    ctx: &ToolContext,
    pattern: &str,
    path: &str,
    max_matches: usize,
    include_dirs: bool,
) -> String {
    GlobTool
        .execute(
            ctx,
            json!({
                "pattern": pattern,
                "path": path,
                "max_matches": max_matches,
                "include_dirs": include_dirs,
            }),
        )
        .await
        .expect("glob execute")
        .content
}

#[tokio::test]
async fn basic_pattern_matching() {
    let workspace = setup_workspace();
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = run_glob(&ctx, "*.rs", ".", 100, true).await;
    // `*.rs` with literal_separator only matches root-level .rs files.
    assert!(result.contains("foo.rs"));
    assert!(!result.contains("bar.txt"));
    assert!(!result.contains("baz.toml"));
    assert!(!result.contains("sub/qux.rs"));
}

#[tokio::test]
async fn structured_details_are_bounded_and_accompany_match_text() {
    let workspace = tempfile::tempdir().expect("workspace");
    for index in 0..101 {
        std::fs::write(
            workspace.path().join(format!("file{index:03}.rs")),
            "content",
        )
        .expect("write file");
    }
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = GlobTool
        .execute(
            &ctx,
            json!({
                "pattern": "*.rs",
                "path": ".",
                "max_matches": 101,
                "include_dirs": true,
            }),
        )
        .await
        .expect("glob execute");

    assert!(result.content.contains("Found 101 matches"));
    let details = result.details.expect("glob details");
    assert_eq!(details["kind"], "glob");
    assert_eq!(details["pattern"], "*.rs");
    assert_eq!(details["path"], ".");
    assert_eq!(details["matches"].as_array().expect("matches").len(), 100);
    assert_eq!(details["total_matched"], 101);
    assert_eq!(details["returned"], 101);
    assert_eq!(details["truncated"], false);
    assert_eq!(details["details_truncated"], true);
}

#[tokio::test]
async fn brace_expansion() {
    let workspace = setup_workspace();
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = run_glob(&ctx, "*.{rs,toml}", ".", 100, true).await;
    assert!(result.contains("foo.rs"));
    assert!(result.contains("baz.toml"));
    assert!(!result.contains("bar.txt"));
}

#[tokio::test]
async fn max_matches_truncation() {
    let workspace = setup_workspace();
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    // `*.{rs,toml}` matches two files; cap at one.
    let result = run_glob(&ctx, "*.{rs,toml}", ".", 1, true).await;
    let count = result
        .lines()
        .filter(|l| {
            !l.starts_with('[')
                && !l.starts_with("Only")
                && !l.is_empty()
                && !l.starts_with("Found")
        })
        .count();
    assert_eq!(count, 1);
    assert!(result.contains("Truncated at 1 matches"));
    assert!(result.contains("2 matched so far"));
}

#[tokio::test]
async fn empty_results() {
    let workspace = setup_workspace();
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = run_glob(&ctx, "*.xyz", ".", 100, true).await;
    assert!(result.is_empty());
}

#[tokio::test]
async fn path_parameter_searches_subdirectory() {
    let workspace = setup_workspace();
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    // Searching in `sub` with `*.rs` matches `qux.rs` relative to `sub`,
    // displayed as `sub/qux.rs` relative to the workspace.
    let result = run_glob(&ctx, "*.rs", "sub", 100, true).await;
    assert!(result.contains("sub/qux.rs"));
    // `deep/inner.rs` should not match `*.rs` (literal separator).
    assert!(!result.contains("deep/inner.rs"));
}

#[tokio::test]
async fn recursive_globstar() {
    let workspace = setup_workspace();
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    // `sub/**/*.rs` matches all .rs files under `sub/`.
    let result = run_glob(&ctx, "sub/**/*.rs", ".", 100, true).await;
    assert!(result.contains("sub/qux.rs"));
    assert!(result.contains("sub/deep/inner.rs"));
    assert!(!result.contains("foo.rs"));
}

#[tokio::test]
async fn include_dirs_true_returns_directories() {
    let workspace = setup_workspace();
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = run_glob(&ctx, "sub", ".", 100, true).await;
    assert!(result.contains("sub/"));
}

#[tokio::test]
async fn include_dirs_false_filters_directories() {
    let workspace = setup_workspace();
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = run_glob(&ctx, "sub", ".", 100, false).await;
    assert!(!result.contains("sub/"));
    assert!(result.is_empty());
}

#[tokio::test]
async fn truncation_message_includes_count() {
    let workspace = setup_workspace();
    let ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let result = run_glob(&ctx, "**/*.rs", ".", 2, true).await;
    assert!(result.contains("[Truncated at 2 matches"));
    assert!(result.contains("matched so far"));
    assert!(result.contains("Only the first 2 matches are returned."));
}
