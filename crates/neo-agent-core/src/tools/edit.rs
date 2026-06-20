use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::diff::unified_diff;
use super::{Tool, ToolContext, ToolFuture, ToolResult, parse_input, schema};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditInput {
    path: std::path::PathBuf,
    old: String,
    new: String,
    #[serde(default)]
    replace_all: bool,
}

pub struct EditTool;

impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "Edit"
    }

    fn description(&self) -> &'static str {
        "Replace text in a UTF-8 workspace file."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<EditInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_file_write_allowed()?;
            let input: EditInput = parse_input(self.name(), input)?;
            let path = ctx.resolve_workspace_path(&input.path)?;
            let content = tokio::fs::read_to_string(&path).await?;
            let occurrences = content.matches(&input.old).count();
            if occurrences == 0 {
                return Ok(ToolResult::error("old text not found"));
            }
            if !input.replace_all && occurrences > 1 {
                return Ok(ToolResult::error("old text appears more than once"));
            }
            let updated = if input.replace_all {
                content.replace(&input.old, &input.new)
            } else {
                content.replacen(&input.old, &input.new, 1)
            };
            let details = edit_details(&input, &content, &updated);
            tokio::fs::write(&path, updated).await?;
            Ok(ToolResult::ok(format!("edited {}", path.display())).with_details(details))
        })
    }
}

fn edit_details(input: &EditInput, before: &str, after: &str) -> serde_json::Value {
    let path = input.path.to_string_lossy();
    json!({
        "path": path,
        "old": input.old,
        "new": input.new,
        "replace_all": input.replace_all,
        "diff": unified_diff(&path, before, after),
    })
}
