use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::diff::unified_diff;
use super::{Tool, ToolContext, ToolFuture, ToolResult, parse_input, schema};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteInput {
    #[schemars(
        description = "Path to the file to write. Relative paths resolve against the working directory."
    )]
    path: std::path::PathBuf,
    #[schemars(description = "UTF-8 content to write to the file.")]
    content: String,
}

pub struct WriteTool;

impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        "Write"
    }

    fn description(&self) -> &'static str {
        "Write a UTF-8 file inside the workspace.\n\n\
         Use Write to create new files or completely replace the contents of existing files. \
         For targeted modifications to existing files (find-and-replace a specific block), use Edit \
         instead — Edit returns a unified diff and preserves unchanged content.\n\n\
         Parameters:\n\
         - path: Path to the file to write. Relative paths resolve against the working directory; \
         paths outside the working directory must be absolute.\n\
         - content: Full UTF-8 text content to write to the file.\n\n\
         Behavior:\n\
         - Overwrites the file if it already exists; creates the file if it does not.\n\
         - Creates parent directories as needed.\n\
         - Returns a confirmation with the number of bytes written and a unified diff in the details.\n\
         - Only UTF-8 text content is supported.\n\n\
         Guidelines:\n\
         - Prefer Edit for surgical changes to existing files; use Write when the entire file content \
         is new or being fully replaced.\n\
         - When writing code, ensure the content is complete and syntactically valid — partial writes \
         can leave files in a broken state.\n\
         - For large files, consider whether Edit (targeted replacement) would be more appropriate \
         than rewriting the entire file."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<WriteInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_file_write_allowed()?;
            let input: WriteInput = parse_input(self.name(), input)?;
            let path = ctx.resolve_parent_for_write(&input.path)?;
            let before = match tokio::fs::read_to_string(&path).await {
                Ok(content) => Some(content),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let details = write_details(&input, before.as_deref(), &input.content);
            tokio::fs::write(&path, input.content).await?;
            Ok(ToolResult::ok(format!("wrote {}", path.display())).with_details(details))
        })
    }
}

fn write_details(input: &WriteInput, before: Option<&str>, after: &str) -> serde_json::Value {
    let path = input.path.to_string_lossy();
    let operation = if before.is_some() {
        "overwritten"
    } else {
        "created"
    };
    let before = before.unwrap_or_default();
    let diff = unified_diff(&path, before, after);
    let (added, removed) = diff_stats(&diff);
    json!({
        "path": path,
        "operation": operation,
        "diff": diff,
        "added": added,
        "removed": removed,
        "line_count": after.lines().count(),
    })
}

fn diff_stats(diff: &str) -> (usize, usize) {
    let mut added = 0usize;
    let mut removed = 0usize;
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    (added, removed)
}

#[cfg(test)]
mod workspace_policy_tests {
    use super::*;
    use crate::{
        ToolAccess, ToolContext, WorkspaceAccessPolicy, WorkspaceAccessRoot,
        WorkspaceAccessRootKind,
    };
    use serde_json::json;

    #[tokio::test]
    async fn write_denies_read_only_added_root() {
        let primary = tempfile::tempdir().expect("primary");
        let added = tempfile::tempdir().expect("added");
        let policy = WorkspaceAccessPolicy::with_roots(
            primary.path(),
            [WorkspaceAccessRoot {
                path: added.path().canonicalize().expect("canonical added"),
                kind: WorkspaceAccessRootKind::Added,
                read: true,
                write: false,
            }],
        )
        .expect("policy");
        let ctx = ToolContext::new(primary.path())
            .expect("context")
            .with_workspace_policy(policy)
            .with_access(ToolAccess::all());
        let path = added.path().join("new.txt");

        let err = WriteTool
            .execute(&ctx, json!({ "path": path, "content": "hello" }))
            .await
            .expect_err("write denied");

        assert!(matches!(
            err,
            crate::tools::ToolError::PathOutsideWorkspace { .. }
        ));
    }
}
