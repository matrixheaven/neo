use ignore::WalkBuilder;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{Tool, ToolContext, ToolFuture, ToolResult, parse_input, schema};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FindInput {
    pattern: String,
    #[serde(default = "default_path")]
    path: std::path::PathBuf,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_path() -> std::path::PathBuf {
    ".".into()
}

const fn default_limit() -> usize {
    100
}

pub struct FindTool;

impl Tool for FindTool {
    fn name(&self) -> &'static str {
        "Find"
    }

    fn description(&self) -> &'static str {
        "Find workspace paths whose file name contains the pattern."
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
            let result = tokio::task::spawn_blocking(move || {
                let mut paths = Vec::new();
                for entry in WalkBuilder::new(root).standard_filters(true).build() {
                    let entry = entry.map_err(std::io::Error::other)?;
                    let Some(name) = entry.file_name().to_str() else {
                        continue;
                    };
                    if name.contains(&input.pattern) {
                        let display = entry
                            .path()
                            .strip_prefix(&workspace)
                            .unwrap_or(entry.path());
                        paths.push(display.display().to_string());
                        if paths.len() >= input.limit {
                            return Ok::<_, std::io::Error>(paths);
                        }
                    }
                }
                Ok(paths)
            })
            .await
            .map_err(std::io::Error::other)??;

            Ok(ToolResult::ok(result.join("\n")))
        })
    }
}
