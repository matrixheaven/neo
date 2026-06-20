use crate::ansi::{Color, Style};
use crate::chrome::ToolStatusKind;
use crate::chrome::TuiTheme;
use crate::core::{Line, Span, Text};

use super::tool_call::ToolCallState;

const RESULT_PREVIEW_LINES: usize = 3;
const COMMAND_PREVIEW_LINES: usize = 10;

#[must_use]
pub fn tool_header(state: &ToolCallState) -> String {
    let symbol = match state.status {
        ToolStatusKind::Pending | ToolStatusKind::Running | ToolStatusKind::Succeeded => "●",
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

/// Build the tool header as styled spans: `{symbol} {verb} {name} ({key}){chip}`.
///
/// Color mapping (mirrors Neo's tool header):
/// - symbol + verb → status color
/// - tool name → bold brand color
/// - `(key arg)` → weak text
/// - chip (`· N lines`) → weak text
#[must_use]
pub fn tool_header_spans(state: &ToolCallState, theme: &TuiTheme) -> Vec<Span> {
    let symbol = tool_symbol(state.status);
    let verb = tool_verb(state.status);
    let status_color = tool_status_color(state.status, theme);
    let name_color = theme.brand;
    let meta_color = theme.text_muted;

    let mut spans = vec![
        Span::styled(format!("{symbol} "), Style::default().fg(status_color)),
        Span::styled(format!("{verb} "), Style::default().fg(status_color)),
        Span::styled(state.name.clone(), Style::default().fg(name_color).bold()),
    ];
    let key = key_argument(state.arguments.as_deref());
    if !key.is_empty() {
        spans.push(Span::styled(
            format!(" ({key})"),
            Style::default().fg(meta_color),
        ));
    }
    let chip = result_chip(state);
    if !chip.is_empty() {
        spans.push(Span::styled(chip, Style::default().fg(meta_color)));
    }
    spans
}

fn tool_symbol(status: ToolStatusKind) -> &'static str {
    match status {
        // Running and succeeded both use ● and the ok status color, matching
        // the grouped tool card.
        ToolStatusKind::Pending | ToolStatusKind::Running | ToolStatusKind::Succeeded => "●",
        ToolStatusKind::Failed => "✗",
        ToolStatusKind::Cancelled => "⊘",
    }
}

fn tool_verb(status: ToolStatusKind) -> &'static str {
    match status {
        ToolStatusKind::Pending | ToolStatusKind::Running => "Using",
        ToolStatusKind::Succeeded => "Used",
        ToolStatusKind::Failed => "Failed",
        ToolStatusKind::Cancelled => "Cancelled",
    }
}

fn tool_status_color(status: ToolStatusKind, theme: &TuiTheme) -> Color {
    match status {
        ToolStatusKind::Pending => theme.status_pending,
        ToolStatusKind::Running | ToolStatusKind::Succeeded => theme.status_ok,
        ToolStatusKind::Failed => theme.status_error,
        ToolStatusKind::Cancelled => theme.status_cancelled,
    }
}

#[must_use]
pub fn render_tool_body(state: &ToolCallState, expanded: bool, width: usize) -> Vec<Line> {
    if hides_successful_todo_list_body(state) {
        return Vec::new();
    }

    if state.name == "Write"
        && let Some((path, content)) = parse_write_arguments(state.arguments.as_deref())
    {
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

    if state.name == "Edit"
        && let Some(arguments) = state.arguments.as_deref().and_then(parse_edit_arguments)
    {
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

/// Theme-aware variant of [`render_tool_body`]. Emits styled lines:
/// - Write/Edit preview headers and generic result bodies → `theme.text_muted`
/// - Edit diff lines → `theme.diff_added` / `theme.diff_removed` (kept
///   colored instead of ANSI-stripped)
/// - Collapsed overflow hints → `theme.text_muted`
#[must_use]
pub fn render_tool_body_themed(
    state: &ToolCallState,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    if hides_successful_todo_list_body(state) {
        return Vec::new();
    }

    let weak = Style::default().fg(theme.text_muted);
    let body_style = Style::default().fg(theme.text_primary);

    if state.name == "Write"
        && let Some((path, content)) = parse_write_arguments(state.arguments.as_deref())
    {
        let lines: Vec<&str> = content.lines().collect();
        let total = lines.len();
        let limit = if expanded {
            total
        } else {
            COMMAND_PREVIEW_LINES.min(total)
        };
        let mut rows = vec![Line::styled(format!("  {path} · {total} lines"), weak)];
        for (index, line) in lines.iter().take(limit).enumerate() {
            rows.push(Line::styled(
                format!("  {:>4} {line}", index + 1),
                body_style,
            ));
        }
        if limit < total {
            rows.push(Line::styled(
                format!(
                    "  ... ({} more lines, {total} total, ctrl+o to expand)",
                    total - limit
                ),
                weak,
            ));
        }
        return rows;
    }

    if state.name == "Edit"
        && let Some(arguments) = state.arguments.as_deref().and_then(parse_edit_arguments)
    {
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
        .map(|line| diff_body_line(&line.to_ansi(), theme))
        .collect();
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
                rows.push(Line::styled(
                    format!("  ... ({remaining} more lines, ctrl+o to expand)"),
                    weak,
                ));
                return rows;
            }
            rows.push(Line::styled(
                format!("  {}", crate::ansi::strip_ansi(&wrapped.to_ansi())),
                body_style,
            ));
            rendered += 1;
        }
    }
    rows
}

fn hides_successful_todo_list_body(state: &ToolCallState) -> bool {
    state.name == "TodoList" && state.status == ToolStatusKind::Succeeded
}

/// Render one diff body line with add/remove coloring. Indented 2 spaces,
/// the leading `+`/`-`/` ` drives the color.
fn diff_body_line(raw: &str, theme: &TuiTheme) -> Line {
    let plain = crate::ansi::strip_ansi(raw);
    let trimmed = plain.trim_start();
    let color = match trimmed.chars().next() {
        Some('+') => theme.diff_added,
        Some('-') => theme.diff_removed,
        Some('@') => theme.diff_hunk,
        _ => theme.diff_context,
    };
    Line::styled(format!("  {plain}"), Style::default().fg(color))
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

#[must_use]
pub fn key_argument(arguments: Option<&str>) -> String {
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
    if state.name == "Read" || state.name == "Write" {
        return format!(" · {} lines", result.lines().count());
    }
    String::new()
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
