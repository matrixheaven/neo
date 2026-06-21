use crate::ansi::{Color, Style};
use crate::chrome::ToolStatusKind;
use crate::chrome::TuiTheme;
use crate::core::{Line, Span, Text};
use crate::tool_diff::{DiffModel, DiffRenderLine, DiffRenderLineKind, DiffRenderState};

use super::tool_call::ToolCallState;

const RESULT_PREVIEW_LINES: usize = 3;
const COMMAND_PREVIEW_LINES: usize = 10;

#[must_use]
pub fn tool_header(state: &ToolCallState) -> String {
    let symbol = tool_symbol(state.status);
    let verb = tool_verb(state.status);
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
        ToolStatusKind::Pending => "Queued",
        ToolStatusKind::Running => "Using",
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
    render_tool_body_with_palette(state, expanded, width, ToolBodyPalette::plain())
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
    render_tool_body_with_palette(state, expanded, width, ToolBodyPalette::themed(theme))
}

#[derive(Clone, Copy)]
struct ToolBodyPalette<'a> {
    theme: Option<&'a TuiTheme>,
}

impl<'a> ToolBodyPalette<'a> {
    const fn plain() -> Self {
        Self { theme: None }
    }

    const fn themed(theme: &'a TuiTheme) -> Self {
        Self { theme: Some(theme) }
    }

    fn weak_line(self, text: String) -> Line {
        self.styled_or_raw(text, |theme| Style::default().fg(theme.text_muted))
    }

    fn body_line(self, text: String) -> Line {
        self.styled_or_raw(text, |theme| Style::default().fg(theme.text_primary))
    }

    fn diff_line(self, line: &Line) -> Line {
        match self.theme {
            Some(theme) => diff_body_line(&line.to_ansi(), theme),
            None => Line::raw(format!("  {}", crate::ansi::strip_ansi(&line.to_ansi()))),
        }
    }

    fn styled_or_raw(self, text: String, style: impl FnOnce(&TuiTheme) -> Style) -> Line {
        match self.theme {
            Some(theme) => Line::styled(text, style(theme)),
            None => Line::raw(text),
        }
    }
}

fn render_tool_body_with_palette(
    state: &ToolCallState,
    expanded: bool,
    width: usize,
    palette: ToolBodyPalette<'_>,
) -> Vec<Line> {
    if hides_successful_todo_list_body(state) {
        return Vec::new();
    }

    render_diff_details(state, expanded, width, palette)
        .or_else(|| render_write_body(state, expanded, palette))
        .or_else(|| render_edit_body(state, expanded, palette))
        .or_else(|| render_result_body(state, expanded, width, palette))
        .unwrap_or_default()
}

fn render_diff_details(
    state: &ToolCallState,
    expanded: bool,
    width: usize,
    palette: ToolBodyPalette<'_>,
) -> Option<Vec<Line>> {
    if is_file_write_tool(&state.name)
        && let Some(model) = state
            .details
            .as_ref()
            .and_then(DiffModel::from_tool_details)
    {
        return Some(render_diff_model_lines(
            &model,
            expanded,
            width,
            palette.theme,
        ));
    }
    None
}

fn render_write_body(
    state: &ToolCallState,
    expanded: bool,
    palette: ToolBodyPalette<'_>,
) -> Option<Vec<Line>> {
    if state.name != "Write" {
        return None;
    }

    let (path, content) = parse_write_arguments(state.arguments.as_deref())?;
    if is_pending_or_running(state.status) {
        return Some(vec![palette.weak_line(format!("  {path}"))]);
    }

    Some(render_write_preview(&path, &content, expanded, palette))
}

fn render_write_preview(
    path: &str,
    content: &str,
    expanded: bool,
    palette: ToolBodyPalette<'_>,
) -> Vec<Line> {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let limit = preview_limit(total, expanded, COMMAND_PREVIEW_LINES);
    let mut rows = vec![palette.weak_line(format!("  {path} · {total} lines"))];
    for (index, line) in lines.iter().take(limit).enumerate() {
        rows.push(palette.body_line(format!("  {:>4} {line}", index + 1)));
    }
    if limit < total {
        rows.push(palette.weak_line(format!(
            "  ... ({} more lines, {total} total, ctrl+o to expand)",
            total - limit
        )));
    }
    rows
}

fn render_edit_body(
    state: &ToolCallState,
    expanded: bool,
    palette: ToolBodyPalette<'_>,
) -> Option<Vec<Line>> {
    if state.name != "Edit" {
        return None;
    }

    let arguments = state.arguments.as_deref().and_then(parse_edit_arguments)?;
    if is_pending_or_running(state.status) {
        return Some(vec![palette.weak_line(format!("  {}", arguments.path))]);
    }

    Some(render_edit_preview(&arguments, expanded, palette))
}

fn render_edit_preview(
    arguments: &EditArguments,
    expanded: bool,
    palette: ToolBodyPalette<'_>,
) -> Vec<Line> {
    let max = (!expanded).then_some(COMMAND_PREVIEW_LINES);
    crate::transcript::diff_preview::render_diff_lines_clustered(
        &arguments.old,
        &arguments.new,
        &arguments.path,
        3,
        max,
    )
    .into_iter()
    .map(|line| palette.diff_line(&line))
    .collect()
}

fn render_result_body(
    state: &ToolCallState,
    expanded: bool,
    width: usize,
    palette: ToolBodyPalette<'_>,
) -> Option<Vec<Line>> {
    let result = state.result.as_deref().filter(|value| !value.is_empty())?;
    Some(render_result_preview(result, expanded, width, palette))
}

fn render_result_preview(
    result: &str,
    expanded: bool,
    width: usize,
    palette: ToolBodyPalette<'_>,
) -> Vec<Line> {
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
                rows.push(
                    palette.weak_line(format!("  ... ({remaining} more lines, ctrl+o to expand)")),
                );
                return rows;
            }
            rows.push(
                palette.body_line(format!("  {}", crate::ansi::strip_ansi(&wrapped.to_ansi()))),
            );
            rendered += 1;
        }
    }
    rows
}

const fn is_pending_or_running(status: ToolStatusKind) -> bool {
    matches!(status, ToolStatusKind::Pending | ToolStatusKind::Running)
}

const fn preview_limit(total: usize, expanded: bool, collapsed_limit: usize) -> usize {
    if expanded || total < collapsed_limit {
        total
    } else {
        collapsed_limit
    }
}

fn hides_successful_todo_list_body(state: &ToolCallState) -> bool {
    state.name == "TodoList" && state.status == ToolStatusKind::Succeeded
}

fn is_file_write_tool(name: &str) -> bool {
    matches!(name, "Write" | "Edit")
}

fn render_diff_model_lines(
    model: &DiffModel,
    expanded: bool,
    width: usize,
    theme: Option<&TuiTheme>,
) -> Vec<Line> {
    let render_width = width.saturating_sub(2).max(1);
    let state = DiffRenderState::new(model.clone());
    let lines = state.render_display_lines(render_width);
    let total = lines.len();
    let limit = if expanded {
        total
    } else {
        COMMAND_PREVIEW_LINES.min(total)
    };
    let mut rows = lines
        .into_iter()
        .take(limit)
        .map(|line| render_diff_line(&line, theme))
        .collect::<Vec<_>>();
    if limit < total {
        let message = format!(
            "  ... ({} more diff lines, {total} total, ctrl+o to expand)",
            total - limit
        );
        rows.push(match theme {
            Some(theme) => Line::styled(message, Style::default().fg(theme.text_muted)),
            None => Line::raw(message),
        });
    }
    rows
}

fn render_diff_line(line: &DiffRenderLine, theme: Option<&TuiTheme>) -> Line {
    let text = format!("  {}", line.text);
    let Some(theme) = theme else {
        return Line::raw(text);
    };
    let color = match line.kind {
        DiffRenderLineKind::Summary | DiffRenderLineKind::Separator => theme.diff_hunk,
        DiffRenderLineKind::Added => theme.diff_added,
        DiffRenderLineKind::Removed => theme.diff_removed,
        DiffRenderLineKind::Context => theme.diff_context,
    };
    Line::styled(text, Style::default().fg(color))
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

const PATH_KEYS: &[&str] = &["path", "file_path"];
const OLD_KEYS: &[&str] = &["old_string", "old"];
const NEW_KEYS: &[&str] = &["new_string", "new"];

fn parse_write_arguments(arguments: Option<&str>) -> Option<(String, String)> {
    let value = serde_json::from_str::<serde_json::Value>(arguments?).ok()?;
    let path = string_field(&value, PATH_KEYS)?;
    let content = string_field(&value, &["content"])?;
    Some((path, content))
}

fn parse_edit_arguments(arguments: &str) -> Option<EditArguments> {
    let value = serde_json::from_str::<serde_json::Value>(arguments).ok()?;
    let path = string_field(&value, PATH_KEYS)?;
    let old = string_field(&value, OLD_KEYS)?;
    let new = string_field(&value, NEW_KEYS)?;
    Some(EditArguments { path, old, new })
}

fn string_field(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(key).and_then(serde_json::Value::as_str))
        .map(std::borrow::ToOwned::to_owned)
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
    if is_file_write_tool(&state.name)
        && let Some(model) = state
            .details
            .as_ref()
            .and_then(DiffModel::from_tool_details)
    {
        return format!(" · +{} -{}", model.stats().added, model.stats().removed);
    }
    let Some(result) = state.result.as_deref().filter(|value| !value.is_empty()) else {
        return String::new();
    };
    if state.name == "Read" {
        return format!(" · {} lines", result.lines().count());
    }
    String::new()
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
