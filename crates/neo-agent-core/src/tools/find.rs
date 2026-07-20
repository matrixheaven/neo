use ignore::WalkBuilder;
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::Path;

use super::{Tool, ToolContext, ToolFuture, ToolResult, parse_input, schema};

const DEFAULT_LIMIT: usize = 100;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FindInput {
    #[schemars(description = "Substring to search for in file and directory names.")]
    pattern: String,
    #[serde(default = "default_path")]
    #[schemars(
        description = "Directory to search. Relative paths resolve against the current working directory. Omit to search the current working directory."
    )]
    path: std::path::PathBuf,
    #[serde(default = "default_limit")]
    #[schemars(
        description = "Maximum number of matching paths to return. Defaults to 100. Use a lower value when only a few results are needed."
    )]
    limit: usize,
    #[serde(default = "default_include_dirs")]
    #[schemars(
        description = "Whether to include directories in results. Defaults to true. Set to false to return only files."
    )]
    include_dirs: bool,
}

fn default_path() -> std::path::PathBuf {
    ".".into()
}

const fn default_limit() -> usize {
    DEFAULT_LIMIT
}

const fn default_include_dirs() -> bool {
    true
}

fn display_path(path: &Path) -> String {
    path.to_str()
        .map_or_else(|| format!("<non-UTF-8 path:{path:?}>"), ToOwned::to_owned)
}

pub struct FindTool;

impl Tool for FindTool {
    fn name(&self) -> &'static str {
        "Find"
    }

    fn description(&self) -> &'static str {
        "Find workspace paths whose file or directory name contains the given substring.\n\
        \n\
        Use Find when you want to locate files or directories by a substring of their name. \
        For pattern-based file searches (globs such as '*.rs' or 'src/**/*.ts'), use Glob instead. \
        If you already know a concrete file path and need to inspect its contents, use Read directly.\n\
        \n\
        Parameters:\n\
        - pattern: Substring to search for in file and directory names.\n\
        - path: Directory to search. Relative paths resolve against the working directory.\n\
        - limit: Maximum number of matching paths to return. Defaults to 100.\n\
        - include_dirs: Whether to include directories in results. Defaults to true; set to false \
          to return only files.\n\
        \n\
        Output format:\n\
        - Returns one relative path per line, sorted by modification time (most recent first).\n\
        - A `<system>...</system>` status block is appended with the total found, the number \
          returned, and a truncation notice if the limit was reached."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<FindInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_file_read_allowed()?;
            let input: FindInput = parse_input(self.name(), input)?;
            let root = ctx.resolve_workspace_path(&input.path)?;
            let workspace = ctx.workspace_root().to_path_buf();

            let result = tokio::task::spawn_blocking({
                let input = input.clone();
                move || {
                    let mut paths: Vec<(String, std::time::SystemTime)> = Vec::new();
                    let limit = input.limit;
                    for entry in WalkBuilder::new(root).standard_filters(true).build() {
                        let entry = entry.map_err(std::io::Error::other)?;
                        let name = entry.file_name().to_string_lossy();
                        let is_dir = entry
                            .file_type()
                            .is_some_and(|file_type| file_type.is_dir());
                        if is_dir && !input.include_dirs {
                            continue;
                        }
                        if name.contains(&input.pattern) {
                            let display = entry
                                .path()
                                .strip_prefix(&workspace)
                                .unwrap_or(entry.path());
                            let mtime = entry
                                .metadata()
                                .ok()
                                .and_then(|m| m.modified().ok())
                                .unwrap_or(std::time::UNIX_EPOCH);
                            paths.push((display_path(display), mtime));
                            if limit > 0 && paths.len() >= limit {
                                break;
                            }
                        }
                    }
                    // Sort by modification time, most recent first.
                    paths.sort_by_key(|b| std::cmp::Reverse(b.1));
                    Ok::<_, std::io::Error>(paths.into_iter().map(|(p, _)| p).collect::<Vec<_>>())
                }
            })
            .await
            .map_err(std::io::Error::other)??;

            let total = result.len();
            let truncated = input.limit > 0 && total >= input.limit;
            let rendered = result.join("\n");
            let message = format_find_message(total, truncated, input.limit);

            if rendered.is_empty() {
                Ok(ToolResult::ok(format!("<system>{message}</system>")))
            } else {
                Ok(ToolResult::ok(format!(
                    "{rendered}\n<system>{message}</system>"
                )))
            }
        })
    }
}

fn format_find_message(total: usize, truncated: bool, limit: usize) -> String {
    let mut parts = Vec::new();
    let path_word = if total == 1 { "path" } else { "paths" };
    parts.push(format!("Found {total} matching {path_word}."));
    if truncated {
        parts.push(format!("Results truncated to the first {limit} matches; use a more specific pattern or increase limit to see more."));
    }
    if total == 0 {
        parts.push("No matching paths found.".to_owned());
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ToolAccess, ToolContext};
    use serde_json::json;

    fn setup_workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("foo.rs"), "fn main() {}").expect("write foo.rs");
        std::fs::write(dir.path().join("bar.txt"), "hello").expect("write bar.txt");
        std::fs::write(dir.path().join("lib.rs"), "pub mod foo;").expect("write lib.rs");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir src");
        std::fs::write(dir.path().join("src").join("main.rs"), "fn main() {}")
            .expect("write main.rs");
        dir
    }

    async fn run_find(ctx: &ToolContext, pattern: &str, extra: serde_json::Value) -> ToolResult {
        let mut input = json!({ "pattern": pattern });
        if let Some(obj) = input.as_object_mut()
            && let serde_json::Value::Object(extra_obj) = extra
        {
            for (k, v) in extra_obj {
                obj.insert(k, v);
            }
        }
        FindTool.execute(ctx, input).await.expect("execute")
    }

    #[tokio::test]
    async fn finds_files_by_substring() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_access(ToolAccess::all());

        let result = run_find(&ctx, "lib", json!({})).await;
        assert!(result.content.contains("lib.rs"));
        assert!(!result.content.contains("foo.rs"));
        assert!(!result.content.contains("bar.txt"));
        assert!(result.content.contains("Found 1 matching path"));
    }

    #[tokio::test]
    async fn include_dirs_false_omits_directories() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_access(ToolAccess::all());

        let result = run_find(&ctx, "main", json!({ "include_dirs": false })).await;
        assert!(result.content.contains("src/main.rs"));
        assert!(!result.content.lines().any(|l| l == "src/"));
        assert!(result.content.contains("Found 1 matching path"));
    }

    #[tokio::test]
    async fn include_dirs_true_includes_directories() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_access(ToolAccess::all());

        let result = run_find(&ctx, "src", json!({ "include_dirs": true })).await;
        assert!(result.content.lines().any(|l| l == "src"));
    }

    #[tokio::test]
    async fn limit_truncates_results() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_access(ToolAccess::all());

        let result = run_find(&ctx, "main", json!({ "limit": 1 })).await;
        let non_system_lines = result
            .content
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with("<system>"))
            .count();
        assert_eq!(non_system_lines, 1);
        assert!(result.content.contains("truncated"));
    }

    #[tokio::test]
    async fn empty_results_report_no_matches() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_access(ToolAccess::all());

        let result = run_find(&ctx, "nonexistent", json!({})).await;
        assert!(result.content.contains("No matching paths found"));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn non_utf8_names_are_reported_instead_of_skipped() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let workspace = tempfile::tempdir().expect("tempdir");
        let name = OsString::from_vec(b"needle-\xff".to_vec());
        std::fs::write(workspace.path().join(name), "x").expect("write non-UTF-8 file");
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_access(ToolAccess::all());

        let result = run_find(&ctx, "needle", json!({})).await;
        assert!(result.content.contains("<non-UTF-8 path:"));
        assert!(result.content.contains("Found 1 matching path"));
    }
}
