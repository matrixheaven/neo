use schemars::JsonSchema;
use serde::Deserialize;

use super::{Tool, ToolContext, ToolFuture, ToolResult, parse_input, schema};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListInput {
    #[serde(default = "default_path")]
    path: std::path::PathBuf,
}

fn default_path() -> std::path::PathBuf {
    ".".into()
}

pub struct ListTool;

impl Tool for ListTool {
    fn name(&self) -> &'static str {
        "list"
    }

    fn description(&self) -> &'static str {
        "List files and directories inside the workspace."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<ListInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_file_read_allowed()?;
            let input: ListInput = parse_input(self.name(), input)?;
            let path = ctx.resolve_workspace_path(&input.path)?;
            let mut entries = tokio::fs::read_dir(path).await?;
            let mut names = Vec::new();
            while let Some(entry) = entries.next_entry().await? {
                let file_type = entry.file_type().await?;
                let suffix = if file_type.is_dir() { "/" } else { "" };
                names.push(format!("{}{}", entry.file_name().to_string_lossy(), suffix));
            }
            names.sort();
            Ok(ToolResult::ok(names.join("\n")))
        })
    }
}
