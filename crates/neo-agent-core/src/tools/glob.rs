use globset::GlobSetBuilder;
use ignore::WalkBuilder;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{Tool, ToolContext, ToolError, ToolFuture, ToolResult, parse_input, schema};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GlobInput {
    pattern: String,
    #[serde(default = "default_path")]
    path: std::path::PathBuf,
    #[serde(default = "default_max_matches")]
    max_matches: usize,
}

fn default_path() -> std::path::PathBuf {
    ".".into()
}

const fn default_max_matches() -> usize {
    100
}

pub struct GlobTool;

impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        "Glob"
    }

    fn description(&self) -> &'static str {
        "Find workspace files matching a glob pattern (supports *, **, and brace expansion)."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<GlobInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_file_read_allowed()?;
            let input: GlobInput = parse_input(self.name(), input)?;
            let walk_root = ctx.resolve_workspace_path(&input.path)?;
            let workspace = ctx.workspace_root().to_path_buf();

            // Brace-expand the pattern into individual sub-patterns.
            let sub_patterns = expand_braces(&input.pattern);
            let mut builder = GlobSetBuilder::new();
            for sub_pattern in &sub_patterns {
                let glob = globset::GlobBuilder::new(sub_pattern)
                    .literal_separator(true)
                    .build()
                    .map_err(|err| ToolError::InvalidInput {
                        tool: self.name().to_owned(),
                        message: format!("invalid glob pattern '{sub_pattern}': {err}"),
                    })?;
                builder.add(glob);
            }
            let glob_set = builder.build().map_err(|err| ToolError::InvalidInput {
                tool: self.name().to_owned(),
                message: format!("invalid glob pattern: {err}"),
            })?;

            let max_matches = input.max_matches;
            let result = tokio::task::spawn_blocking(move || {
                let mut matches: Vec<(String, std::time::SystemTime)> = Vec::new();
                for entry in WalkBuilder::new(&walk_root).standard_filters(true).build() {
                    let Ok(entry) = entry else {
                        continue;
                    };
                    if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                        continue;
                    }
                    // Match the path relative to the walk root so that the
                    // `path` parameter scopes the search naturally.
                    let relative = entry
                        .path()
                        .strip_prefix(&walk_root)
                        .unwrap_or(entry.path());
                    if !glob_set.is_match(relative) {
                        continue;
                    }
                    // Display the path relative to the workspace root for
                    // consistency with grep / find.
                    let display = entry
                        .path()
                        .strip_prefix(&workspace)
                        .unwrap_or(entry.path());
                    let mtime = entry
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .unwrap_or(std::time::UNIX_EPOCH);
                    matches.push((display.display().to_string(), mtime));
                }
                // Sort by modification time, most recent first.
                matches.sort_by_key(|b| std::cmp::Reverse(b.1));
                Ok::<_, std::io::Error>(
                    matches
                        .into_iter()
                        .take(max_matches)
                        .map(|(p, _)| p)
                        .collect::<Vec<_>>(),
                )
            })
            .await
            .map_err(std::io::Error::other)??;

            Ok(ToolResult::ok(result.join("\n")))
        })
    }
}

/// Expand brace alternatives in a glob pattern.
///
/// `*.{ts,tsx}` → `["*.ts", "*.tsx"]`
/// `{src,tests}/*.rs` → `["src/*.rs", "tests/*.rs"]`
fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_string()];
    };
    let Some(close_rel) = pattern[open..].find('}') else {
        // No closing brace — treat the `{` as a literal.
        return vec![pattern.to_string()];
    };
    let close = open + close_rel;
    let prefix = &pattern[..open];
    let group = &pattern[open + 1..close];
    let suffix = &pattern[close + 1..];

    let mut results = Vec::new();
    for option in group.split(',') {
        let expanded = format!("{prefix}{option}{suffix}");
        // Recurse to handle additional brace groups in prefix/suffix.
        results.extend(expand_braces(&expanded));
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PermissionPolicy, ToolContext};
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
        std::fs::write(dir.path().join("sub/deep/inner.rs"), "// deep").expect("write inner.rs");
        dir
    }

    async fn run_glob(ctx: &ToolContext, pattern: &str, path: &str, max_matches: usize) -> String {
        GlobTool
            .execute(
                ctx,
                json!({
                    "pattern": pattern,
                    "path": path,
                    "max_matches": max_matches,
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
            .with_permission_policy(PermissionPolicy::allow_all());

        let result = run_glob(&ctx, "*.rs", ".", 100).await;
        // `*.rs` with literal_separator only matches root-level .rs files.
        assert!(result.contains("foo.rs"));
        assert!(!result.contains("bar.txt"));
        assert!(!result.contains("baz.toml"));
        assert!(!result.contains("sub/qux.rs"));
    }

    #[tokio::test]
    async fn brace_expansion() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        let result = run_glob(&ctx, "*.{rs,toml}", ".", 100).await;
        assert!(result.contains("foo.rs"));
        assert!(result.contains("baz.toml"));
        assert!(!result.contains("bar.txt"));
    }

    #[tokio::test]
    async fn max_matches_truncation() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        // `*.{rs,toml}` matches two files; cap at one.
        let result = run_glob(&ctx, "*.{rs,toml}", ".", 1).await;
        let count = result.lines().filter(|l| !l.is_empty()).count();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn empty_results() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        let result = run_glob(&ctx, "*.xyz", ".", 100).await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn path_parameter_searches_subdirectory() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        // Searching in `sub` with `*.rs` matches `qux.rs` relative to `sub`,
        // displayed as `sub/qux.rs` relative to the workspace.
        let result = run_glob(&ctx, "*.rs", "sub", 100).await;
        assert!(result.contains("sub/qux.rs"));
        // `deep/inner.rs` should not match `*.rs` (literal separator).
        assert!(!result.contains("deep/inner.rs"));
    }

    #[tokio::test]
    async fn recursive_globstar() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        // `sub/**/*.rs` matches all .rs files under `sub/`.
        let result = run_glob(&ctx, "sub/**/*.rs", ".", 100).await;
        assert!(result.contains("sub/qux.rs"));
        assert!(result.contains("sub/deep/inner.rs"));
        assert!(!result.contains("foo.rs"));
    }

    #[test]
    fn expand_braces_simple() {
        assert_eq!(expand_braces("*.rs"), vec!["*.rs"]);
    }

    #[test]
    fn expand_braces_alternation() {
        let result = expand_braces("*.{rs,toml}");
        assert!(result.contains(&"*.rs".to_string()));
        assert!(result.contains(&"*.toml".to_string()));
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn expand_braces_prefix() {
        assert_eq!(expand_braces("{foo,bar}.rs"), vec!["foo.rs", "bar.rs"]);
    }

    #[test]
    fn expand_braces_multiple_groups() {
        let result = expand_braces("{a,b}/{c,d}");
        assert!(result.contains(&"a/c".to_string()));
        assert!(result.contains(&"a/d".to_string()));
        assert!(result.contains(&"b/c".to_string()));
        assert!(result.contains(&"b/d".to_string()));
        assert_eq!(result.len(), 4);
    }
}
