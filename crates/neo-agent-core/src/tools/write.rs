use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::diff::unified_diff;
use super::{Tool, ToolContext, ToolFuture, ToolResult, parse_input, schema};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteInput {
    path: std::path::PathBuf,
    content: String,
}

pub struct WriteTool;

impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        "Write"
    }

    fn description(&self) -> &'static str {
        "Write a UTF-8 file inside the workspace."
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
