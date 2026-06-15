use ignore::WalkBuilder;
use regex::Regex;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{Tool, ToolContext, ToolFuture, ToolResult, parse_input, schema};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GrepInput {
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

pub struct GrepTool;

impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }

    fn description(&self) -> &'static str {
        "Search workspace text files with a regular expression."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<GrepInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_file_read_allowed()?;
            let input: GrepInput = parse_input(self.name(), input)?;
            let root = ctx.resolve_workspace_path(&input.path)?;
            let regex = Regex::new(&input.pattern)?;
            let workspace = ctx.workspace_root().to_path_buf();
            let result = tokio::task::spawn_blocking(move || {
                let mut matches = Vec::new();
                for entry in WalkBuilder::new(root).standard_filters(true).build() {
                    let entry = entry.map_err(std::io::Error::other)?;
                    if !entry
                        .file_type()
                        .is_some_and(|file_type| file_type.is_file())
                    {
                        continue;
                    }
                    let Ok(content) = std::fs::read_to_string(entry.path()) else {
                        continue;
                    };
                    let display = entry
                        .path()
                        .strip_prefix(&workspace)
                        .unwrap_or(entry.path());
                    for (index, line) in content.lines().enumerate() {
                        if regex.is_match(line) {
                            matches.push(format!("{}:{}:{}", display.display(), index + 1, line));
                            if matches.len() >= input.limit {
                                return Ok::<_, std::io::Error>(matches);
                            }
                        }
                    }
                }
                Ok(matches)
            })
            .await
            .map_err(std::io::Error::other)??;

            Ok(ToolResult::ok(result.join("\n")))
        })
    }
}
