use schemars::JsonSchema;
use serde::Deserialize;

use super::{Tool, ToolContext, ToolError, ToolFuture, ToolResult, parse_input, schema};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadInput {
    path: std::path::PathBuf,
}

pub struct ReadTool;

impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "read"
    }

    fn description(&self) -> &'static str {
        "Read a UTF-8 file from the workspace."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<ReadInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_file_read_allowed()?;
            let input: ReadInput = parse_input(self.name(), input)?;
            let path = ctx.resolve_workspace_path(&input.path)?;
            let content = tokio::fs::read_to_string(path)
                .await
                .map_err(ToolError::Io)?;
            Ok(ToolResult::ok(content))
        })
    }
}
