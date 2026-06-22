use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::diff::unified_diff;
use super::{Tool, ToolContext, ToolFuture, ToolResult, parse_input, schema};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditInput {
    #[schemars(
        description = "Path to the file to edit. Relative paths resolve against the working directory."
    )]
    path: std::path::PathBuf,
    #[schemars(
        description = "Exact existing text to replace. Must match uniquely unless replace_all is true."
    )]
    old: String,
    #[schemars(description = "New text to insert in place of old.")]
    new: String,
    #[serde(default)]
    #[schemars(
        description = "If true, replace every occurrence of old. Defaults to false (replace first occurrence only)."
    )]
    replace_all: bool,
}

pub struct EditTool;

impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "Edit"
    }

    fn description(&self) -> &'static str {
        "Replace text in a UTF-8 workspace file. Use Edit for targeted modifications to existing \
         files — it finds the exact `old` text and replaces it with `new`. For creating new files \
         or full content replacement, use Write instead.\n\n\
         Parameters:\n\
         - path: Path to the file to edit. Relative paths resolve against the working directory; \
         paths outside the working directory must be absolute.\n\
         - old: Exact existing text to find and replace. Must match the file content \
         character-for-character, including whitespace and indentation.\n\
         - new: The replacement text that will be inserted in place of old.\n\
         - replace_all: When false (default), old must match exactly one location in the file; if it \
         matches multiple locations, the edit fails with the match count so you can provide more \
         context. When true, every occurrence of old is replaced.\n\n\
         CRITICAL — unique match requirement:\n\
         When replace_all is false, old must match exactly one location. If old matches zero \
         locations, the edit fails and the file is unchanged — re-read the file and adjust old to \
         match the current content. If old matches multiple locations, the edit fails with the \
         count — either add more surrounding context to old to make it unique, or set replace_all \
         to true.\n\n\
         Output:\n\
         Returns a unified diff showing the changes made, so you can verify the edit produced the \
         intended result.\n\n\
         Guidelines:\n\
         - Always read the file first to confirm the exact current content of old.\n\
         - Include enough surrounding context (imports, function signatures, closing braces) in old \
         to ensure a unique match.\n\
         - For renaming a variable or symbol across the entire file, use replace_all=true.\n\
         - If an edit fails, do not guess — re-read the file and try again with corrected old text."
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
