use crate::ToolStatusKind;
use crate::core::{Line, Text};

use super::tool_call::ToolCallState;

const RESULT_PREVIEW_LINES: usize = 3;
const COMMAND_PREVIEW_LINES: usize = 10;

#[must_use]
pub fn tool_header(state: &ToolCallState) -> String {
    let symbol = match state.status {
        ToolStatusKind::Pending | ToolStatusKind::Running => "●",
        ToolStatusKind::Succeeded => "✓",
        ToolStatusKind::Failed => "✗",
        ToolStatusKind::Cancelled => "⊘",
    };
    let verb = match state.status {
        ToolStatusKind::Pending | ToolStatusKind::Running => "Using",
        ToolStatusKind::Succeeded => "Used",
        ToolStatusKind::Failed => "Failed",
        ToolStatusKind::Cancelled => "Cancelled",
    };
    let key = key_argument(state.arguments.as_deref());
    let chip = result_chip(state);
    if key.is_empty() {
        format!("{symbol} {verb} {}{chip}", state.name)
    } else {
        format!("{symbol} {verb} {} ({key}){chip}", state.name)
    }
}

#[must_use]
pub fn render_tool_body(state: &ToolCallState, expanded: bool, width: usize) -> Vec<Line> {
    if state.name.eq_ignore_ascii_case("Write") {
        if let Some((path, content)) = parse_write_arguments(state.arguments.as_deref()) {
            let lines: Vec<&str> = content.lines().collect();
            let total = lines.len();
            let limit = if expanded {
                total
            } else {
                COMMAND_PREVIEW_LINES.min(total)
            };
            let mut rows = vec![Line::raw(format!("  {path} · {total} lines"))];
            for (index, line) in lines.iter().take(limit).enumerate() {
                rows.push(Line::raw(format!("  {:>4} {line}", index + 1)));
            }
            if limit < total {
                rows.push(Line::raw(format!(
                    "  ... ({} more lines, {total} total, ctrl+o to expand)",
                    total - limit
                )));
            }
            return rows;
        }
    }

    if state.name.eq_ignore_ascii_case("Edit") {
        if let Some(arguments) = state.arguments.as_deref().and_then(parse_edit_arguments) {
            let max = if expanded {
                None
            } else {
                Some(COMMAND_PREVIEW_LINES)
            };
            return crate::transcript::diff_preview::render_diff_lines_clustered(
                &arguments.old,
                &arguments.new,
                &arguments.path,
                3,
                max,
            )
            .into_iter()
            .map(|line| Line::raw(format!("  {}", crate::ansi::strip_ansi(&line.to_ansi()))))
            .collect();
        }
    }

    let Some(result) = state.result.as_deref().filter(|value| !value.is_empty()) else {
        return Vec::new();
    };

    let limit = if expanded {
        usize::MAX
    } else {
        RESULT_PREVIEW_LINES
    };
    let result_line_count = result.lines().count();
    let mut rows = Vec::new();
    let mut rendered = 0usize;
    for line in result.lines() {
        for wrapped in Text::new(line).render_lines(width.saturating_sub(2).max(1)) {
            if rendered >= limit {
                let remaining = result_line_count.saturating_sub(rendered);
                rows.push(Line::raw(format!(
                    "  ... ({remaining} more lines, ctrl+o to expand)"
                )));
                return rows;
            }
            rows.push(Line::raw(format!(
                "  {}",
                crate::ansi::strip_ansi(&wrapped.to_ansi())
            )));
            rendered += 1;
        }
    }
    rows
}

struct EditArguments {
    path: String,
    old: String,
    new: String,
}

fn parse_write_arguments(arguments: Option<&str>) -> Option<(String, String)> {
    let value = serde_json::from_str::<serde_json::Value>(arguments?).ok()?;
    let path = value
        .get("path")
        .or_else(|| value.get("file_path"))?
        .as_str()?
        .to_owned();
    let content = value.get("content")?.as_str()?.to_owned();
    Some((path, content))
}

fn parse_edit_arguments(arguments: &str) -> Option<EditArguments> {
    let value = serde_json::from_str::<serde_json::Value>(arguments).ok()?;
    let path = value
        .get("path")
        .or_else(|| value.get("file_path"))?
        .as_str()?
        .to_owned();
    let old = value
        .get("old_string")
        .or_else(|| value.get("old"))?
        .as_str()?
        .to_owned();
    let new = value
        .get("new_string")
        .or_else(|| value.get("new"))?
        .as_str()?
        .to_owned();
    Some(EditArguments { path, old, new })
}

fn key_argument(arguments: Option<&str>) -> String {
    let Some(arguments) = arguments.map(str::trim).filter(|value| !value.is_empty()) else {
        return String::new();
    };
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(arguments) {
        for key in [
            "path",
            "file_path",
            "command",
            "pattern",
            "query",
            "url",
            "description",
        ] {
            if let Some(text) = value.get(key).and_then(|value| value.as_str()) {
                return one_line(text);
            }
        }
    }
    if let Some(path) = arguments
        .strip_prefix(r#"{"path":"#)
        .and_then(|rest| rest.strip_suffix(r#""}"#))
    {
        return one_line(path);
    }
    one_line(arguments)
}

fn result_chip(state: &ToolCallState) -> String {
    let Some(result) = state.result.as_deref().filter(|value| !value.is_empty()) else {
        return String::new();
    };
    let lower = state.name.to_lowercase();
    if lower == "read" || lower == "write" {
        return format!(" · {} lines", result.lines().count());
    }
    if (lower == "bash" || lower == "shell")
        && let Some(code) = state.exit_code
        && code != 0
    {
        return format!(" · exit {code}");
    }
    String::new()
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
