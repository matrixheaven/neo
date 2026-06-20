use schemars::JsonSchema;
use serde::Deserialize;

use super::{Tool, ToolContext, ToolFuture, ToolResult, parse_input, schema};

/// Maximum number of entries to list at the root level.
const LIST_DIR_ROOT_WIDTH: usize = 30;
/// Maximum number of entries to list inside each child directory.
const LIST_DIR_CHILD_WIDTH: usize = 10;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListInput {
    /// Path to the directory to list. Relative paths resolve against the
    /// working directory; paths outside the working directory must be absolute.
    /// Defaults to the current working directory.
    #[serde(default = "default_path")]
    path: std::path::PathBuf,
    /// When true, do not expand the children of hidden directories (names
    /// starting with `.`). Defaults to false.
    #[serde(default)]
    collapse_hidden_dirs: bool,
}

fn default_path() -> std::path::PathBuf {
    ".".into()
}

#[derive(Debug, Clone)]
struct Entry {
    name: String,
    is_dir: bool,
}

pub struct ListTool;

impl Tool for ListTool {
    fn name(&self) -> &'static str {
        "List"
    }

    fn description(&self) -> &'static str {
        "List files and directories inside the workspace as a compact 2-level tree.\
        \
        Use List to explore directory structure. If you already have a concrete file path, \
        call Read directly; do not use List as a pre-check for known files.\
        \
        Parameters:\
        - path: Path to the directory to list. Defaults to the current working directory.\
        - collapse_hidden_dirs: When true, do not expand the children of hidden directories \
          (names starting with `.`). Defaults to false.\
        \
        Output format:\
        - A 2-level tree using `├── ` / `└── ` connectors. Directories end with `/`.\
        - The root level shows up to 30 entries; each child directory shows up to 10 entries.\
        - Truncated levels show `... and N more` so you know more exists.\
        - Returns `(empty directory)` if the directory contains no entries."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<ListInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_file_read_allowed()?;
            let input: ListInput = parse_input(self.name(), input)?;
            let path = ctx.resolve_workspace_path(&input.path)?;

            let root = collect_entries(&path, LIST_DIR_ROOT_WIDTH).await?;
            if !root.readable {
                return Ok(ToolResult::error(format!(
                    "{} is not readable",
                    path.display()
                )));
            }

            let mut lines = Vec::new();
            let root_remaining = root.total.saturating_sub(root.entries.len());

            for (i, entry) in root.entries.iter().enumerate() {
                let is_last = i == root.entries.len() - 1 && root_remaining == 0;
                let connector = if is_last { "└── " } else { "├── " };

                if entry.is_dir {
                    lines.push(format!("{connector}{name}/", name = entry.name));
                    if input.collapse_hidden_dirs && entry.name.starts_with('.') {
                        continue;
                    }
                    let child_prefix = if is_last { "    " } else { "│   " };
                    let child_path = path.join(&entry.name);
                    let child = collect_entries(&child_path, LIST_DIR_CHILD_WIDTH).await?;
                    if !child.readable {
                        lines.push(format!("{child_prefix}└── [not readable]"));
                        continue;
                    }
                    let child_remaining = child.total.saturating_sub(child.entries.len());
                    for (j, ce) in child.entries.iter().enumerate() {
                        let c_is_last = j == child.entries.len() - 1 && child_remaining == 0;
                        let c_connector = if c_is_last {
                            "└── "
                        } else {
                            "├── "
                        };
                        let suffix = if ce.is_dir { "/" } else { "" };
                        lines.push(format!(
                            "{child_prefix}{c_connector}{name}{suffix}",
                            name = ce.name
                        ));
                    }
                    if child_remaining > 0 {
                        lines.push(format!("{child_prefix}└── ... and {child_remaining} more"));
                    }
                } else {
                    lines.push(format!("{connector}{name}", name = entry.name));
                }
            }

            if root_remaining > 0 {
                lines.push(format!("└── ... and {root_remaining} more entries"));
            }

            Ok(ToolResult::ok(if lines.is_empty() {
                "(empty directory)".to_owned()
            } else {
                lines.join("\n")
            }))
        })
    }
}

#[derive(Debug)]
struct CollectedEntries {
    entries: Vec<Entry>,
    total: usize,
    readable: bool,
}

async fn collect_entries(
    dir: &std::path::Path,
    max_width: usize,
) -> Result<CollectedEntries, std::io::Error> {
    let mut entries = Vec::new();
    let mut total: usize = 0;

    let mut read_dir = match tokio::fs::read_dir(dir).await {
        Ok(read_dir) => read_dir,
        Err(error) if error.kind() == std::io::ErrorKind::NotADirectory => {
            return Ok(CollectedEntries {
                entries: Vec::new(),
                total: 0,
                readable: false,
            });
        }
        Err(error) => return Err(error),
    };
    while let Some(entry) = read_dir.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().await.is_ok_and(|ft| ft.is_dir());
        entries.push(Entry { name, is_dir });
        total += 1;
    }

    entries.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            return if a.is_dir {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        a.name.cmp(&b.name)
    });

    let entries = entries.into_iter().take(max_width).collect();
    Ok(CollectedEntries {
        entries,
        total,
        readable: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PermissionPolicy, ToolContext};
    use serde_json::json;

    fn setup_workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("foo.rs"), "fn main() {}").expect("write foo.rs");
        std::fs::write(dir.path().join("bar.txt"), "hello").expect("write bar.txt");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir src");
        std::fs::write(dir.path().join("src").join("lib.rs"), "// lib").expect("write lib.rs");
        std::fs::create_dir_all(dir.path().join(".hidden")).expect("mkdir .hidden");
        std::fs::write(dir.path().join(".hidden").join("secret.rs"), "// secret")
            .expect("write secret.rs");
        dir
    }

    async fn run_list(ctx: &ToolContext, path: &str, collapse_hidden_dirs: bool) -> String {
        ListTool
            .execute(
                ctx,
                json!({
                    "path": path,
                    "collapse_hidden_dirs": collapse_hidden_dirs,
                }),
            )
            .await
            .expect("list execute")
            .content
    }

    #[tokio::test]
    async fn lists_two_level_tree() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        let result = run_list(&ctx, ".", false).await;
        assert!(result.contains("foo.rs"));
        assert!(result.contains("bar.txt"));
        assert!(result.contains("src/"));
        assert!(result.contains("lib.rs"));
        assert!(result.contains(".hidden/"));
        assert!(result.contains("secret.rs"));
        // Directories come before files and end with `/`.
        assert!(result.find("src/").unwrap() < result.find("foo.rs").unwrap());
    }

    #[tokio::test]
    async fn collapse_hidden_dirs_skips_children() {
        let workspace = setup_workspace();
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        let collapsed = run_list(&ctx, ".", true).await;
        assert!(collapsed.contains(".hidden/"));
        assert!(!collapsed.contains("secret.rs"));

        let expanded = run_list(&ctx, ".", false).await;
        assert!(expanded.contains("secret.rs"));
    }

    #[tokio::test]
    async fn empty_directory() {
        let workspace = tempfile::tempdir().expect("tempdir");
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        let result = run_list(&ctx, ".", false).await;
        assert_eq!(result, "(empty directory)");
    }

    #[tokio::test]
    async fn truncation_marker() {
        let workspace = tempfile::tempdir().expect("tempdir");
        for i in 0..35 {
            std::fs::write(workspace.path().join(format!("file{i}.txt")), "x").expect("write file");
        }
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        let result = run_list(&ctx, ".", false).await;
        assert!(result.contains("... and 5 more entries"));
    }

    #[tokio::test]
    async fn child_truncation_marker() {
        let workspace = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(workspace.path().join("dir")).expect("mkdir dir");
        for i in 0..15 {
            std::fs::write(
                workspace.path().join("dir").join(format!("file{i}.txt")),
                "x",
            )
            .expect("write file");
        }
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        let result = run_list(&ctx, ".", false).await;
        assert!(result.contains("... and 5 more"));
    }

    #[tokio::test]
    async fn file_path_is_not_readable() {
        let workspace = tempfile::tempdir().expect("tempdir");
        std::fs::write(workspace.path().join("not-a-dir.txt"), "x").expect("write file");
        let ctx = ToolContext::new(workspace.path())
            .expect("context")
            .with_permission_policy(PermissionPolicy::allow_all());

        let result = ListTool
            .execute(
                &ctx,
                json!({
                    "path": "not-a-dir.txt",
                    "collapse_hidden_dirs": false,
                }),
            )
            .await
            .expect("list execute");
        assert!(result.is_error);
        assert!(result.content.contains("is not readable"));
    }
}
