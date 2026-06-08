use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

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
        "edit"
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

fn unified_diff(path: &str, before: &str, after: &str) -> String {
    let before_lines = split_lines(before);
    let after_lines = split_lines(after);
    let table = lcs_table(&before_lines, &after_lines);
    let mut diff = format!("--- {path}\n+++ {path}\n@@\n");
    append_diff_lines(
        &before_lines,
        &after_lines,
        &table,
        before_lines.len(),
        after_lines.len(),
        &mut diff,
    );
    diff
}

fn split_lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

fn lcs_table(before: &[&str], after: &[&str]) -> Vec<Vec<usize>> {
    let mut table = vec![vec![0; after.len() + 1]; before.len() + 1];
    for i in 0..before.len() {
        for (j, after_line) in after.iter().enumerate() {
            if before[i] == *after_line {
                table[i + 1][j + 1] = table[i][j] + 1;
            } else {
                table[i + 1][j + 1] = table[i + 1][j].max(table[i][j + 1]);
            }
        }
    }
    table
}

fn append_diff_lines(
    before: &[&str],
    after: &[&str],
    table: &[Vec<usize>],
    i: usize,
    j: usize,
    diff: &mut String,
) {
    if i > 0 && j > 0 && before[i - 1] == after[j - 1] {
        append_diff_lines(before, after, table, i - 1, j - 1, diff);
        push_diff_line(diff, ' ', before[i - 1]);
    } else if j > 0 && (i == 0 || table[i][j - 1] >= table[i - 1][j]) {
        append_diff_lines(before, after, table, i, j - 1, diff);
        push_diff_line(diff, '+', after[j - 1]);
    } else if i > 0 {
        append_diff_lines(before, after, table, i - 1, j, diff);
        push_diff_line(diff, '-', before[i - 1]);
    }
}

fn push_diff_line(diff: &mut String, prefix: char, line: &str) {
    diff.push(prefix);
    diff.push_str(line);
    if !line.ends_with('\n') {
        diff.push('\n');
    }
}
