use std::borrow::Cow;
use std::path::Path;

use crate::diff_model::{DiffModel, DiffRenderLine, DiffRenderLineKind, DiffRenderState};
use crate::markdown::{highlight_code_lines, lang_from_path};
use crate::primitive::theme::TuiTheme;
use crate::primitive::{Color, Style};
use crate::primitive::{Line, Span, Text};
use crate::shell::ToolStatusKind;
use crate::token_estimate::{estimate_tokens, format_token_count};

use super::partial_json::extract_partial_string_field;
use super::tool_call::ToolCallState;

const RESULT_PREVIEW_LINES: usize = 3;
const COMMAND_PREVIEW_LINES: usize = 10;
/// Maximum visible length of the key argument shown in a tool header.
/// Borrowed from Kimi Code: the argument is capped so the closing `)` can
/// always be rendered, regardless of terminal width.
const MAX_ARG_LENGTH: usize = 60;

/// Build the tool header as styled spans: `{symbol} {verb} {name} ({key}){chip}`.
///
/// Color mapping (mirrors Neo's tool header):
/// - symbol + verb → status color
/// - tool name → bold brand color
/// - `(key arg)` → weak text
/// - chip (`· N lines`) → weak text
#[must_use]
pub fn tool_header_spans(
    state: &ToolCallState,
    theme: &TuiTheme,
    workspace_dir: Option<&Path>,
) -> Vec<Span> {
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
    if let Some((key, is_path)) = extract_key_argument(state.arguments.as_deref()) {
        let key_text = format_key_argument(&state.name, &key, is_path, workspace_dir);
        spans.push(Span::styled(" (", Style::default().fg(meta_color)));
        spans.push(Span::styled(key_text, Style::default().fg(meta_color)));
        spans.push(Span::styled(")", Style::default().fg(meta_color)));
    }
    let chip = result_chip(state);
    if !chip.is_empty() {
        spans.push(Span::styled(chip, Style::default().fg(meta_color)));
    }
    spans
}

/// Build a custom header for the `ExitPlanMode` tool card.
///
/// Replaces the generic "Used `ExitPlanMode`" with "Current plan",
/// optionally appending "· Approved: <label>" (on success with a chosen
/// approach) or "· Rejected" (on failure).
#[must_use]
pub fn exit_plan_mode_header_spans(state: &ToolCallState, theme: &TuiTheme) -> Vec<Span> {
    let symbol = tool_symbol(state.status);
    let status_color = tool_status_color(state.status, theme);
    let name_color = theme.brand;
    let success_color = theme.status_ok;

    let mut spans = vec![
        Span::styled(format!("{symbol} "), Style::default().fg(status_color)),
        Span::styled("Current plan", Style::default().fg(name_color).bold()),
    ];

    // On success, show "· Approved" or "· Approved: <label>"
    if state.status == ToolStatusKind::Succeeded {
        let label = state
            .details
            .as_ref()
            .and_then(|d| d.get("plan_selected_label"))
            .and_then(serde_json::Value::as_str);
        let chip = match label {
            Some(l) if !l.is_empty() => format!(" · Approved: {l}"),
            _ => " · Approved".to_string(),
        };
        spans.push(Span::styled(chip, Style::default().fg(success_color)));
    }

    // On failure, show "· Rejected"
    if state.status == ToolStatusKind::Failed {
        spans.push(Span::styled(
            " · Rejected".to_string(),
            Style::default().fg(theme.status_error),
        ));
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
            None => Line::raw(format!(
                "  {}",
                crate::primitive::strip_ansi(&line.to_ansi())
            )),
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
    // Only Edit uses unified diff rendering. Write always uses a
    // syntax-highlighted content preview (via render_write_body).
    if state.name != "Edit" {
        return None;
    }
    if let Some(model) = state
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

    let highlight_path = highlight_path_for_write_preview(path, content);
    let highlighted = palette
        .theme
        .and_then(|theme| {
            highlight_path
                .as_deref()
                .map(|path| highlight_code_lines(content, path, theme))
        })
        .unwrap_or_default();
    let use_highlight = !highlighted.is_empty() && highlight_path.is_some();

    for (index, line) in lines.iter().take(limit).enumerate() {
        let line_num = format!("  {:>4} ", index + 1);
        if use_highlight && let Some(highlighted_line) = highlighted.get(index) {
            let theme = palette.theme.expect("theme present when highlighted");
            let mut spans = vec![Span::styled(
                line_num,
                Style::default().fg(theme.text_muted),
            )];
            spans.extend(highlighted_line.clone());
            rows.push(Line::from_spans(spans));
        } else {
            rows.push(palette.body_line(format!("{line_num}{line}")));
        }
    }
    if limit < total {
        rows.push(palette.weak_line(format!(
            "  ... ({} more lines, {total} total, ctrl+o to expand)",
            total - limit
        )));
    }
    rows
}

fn highlight_path_for_write_preview<'a>(path: &'a str, content: &str) -> Option<Cow<'a, str>> {
    if lang_from_path(path).is_some() {
        return Some(Cow::Borrowed(path));
    }
    infer_live_content_path(content).map(Cow::Borrowed)
}

fn infer_live_content_path(content: &str) -> Option<&'static str> {
    let mut saw_jsonish = false;
    for line in content.lines().take(20).map(str::trim_start) {
        if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
            continue;
        }
        if line.starts_with("package ") || line.starts_with("func ") {
            return Some("live.go");
        }
        if line.starts_with("use ")
            || line.starts_with("pub ")
            || line.starts_with("impl ")
            || line.starts_with("fn ")
        {
            return Some("live.rs");
        }
        if line.starts_with("import ")
            || line.starts_with("from ")
            || line.starts_with("def ")
            || line.starts_with("class ")
        {
            return Some("live.py");
        }
        if line.starts_with("const ")
            || line.starts_with("let ")
            || line.starts_with("function ")
            || line.starts_with("export ")
        {
            return Some("live.ts");
        }
        if line.starts_with('{') || line.starts_with('[') {
            saw_jsonish = true;
        }
        if line.starts_with("[package]") || line.contains(" = ") {
            return Some("live.toml");
        }
        if line.contains(": ") && !line.ends_with('{') {
            return Some("live.yaml");
        }
    }
    saw_jsonish.then_some("live.json")
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
            rows.push(palette.body_line(format!(
                "  {}",
                crate::primitive::strip_ansi(&wrapped.to_ansi())
            )));
            rendered += 1;
        }
    }
    rows
}

pub(crate) const fn is_pending_or_running(status: ToolStatusKind) -> bool {
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

pub(crate) fn is_file_write_tool(name: &str) -> bool {
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
    let plain = crate::primitive::strip_ansi(raw);
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
    let path = string_field(&value, "path")?;
    let content = string_field(&value, "content")?;
    Some((path, content))
}

fn parse_edit_arguments(arguments: &str) -> Option<EditArguments> {
    let value = serde_json::from_str::<serde_json::Value>(arguments).ok()?;
    let path = string_field(&value, "path")?;
    let old = string_field(&value, "old")?;
    let new = string_field(&value, "new")?;
    Some(EditArguments { path, old, new })
}

fn string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(std::borrow::ToOwned::to_owned)
}

/// Extract the key argument value and whether it came from a path-like key.
fn extract_key_argument(arguments: Option<&str>) -> Option<(String, bool)> {
    let arguments = arguments.map(str::trim).filter(|value| !value.is_empty())?;
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(arguments) {
        for key in ["path", "command", "pattern", "query", "url", "description"] {
            if let Some(text) = value.get(key).and_then(serde_json::Value::as_str) {
                let is_path = key == "path";
                return Some((one_line(text), is_path));
            }
        }
        // Valid JSON but no recognized key — return None so the header
        // omits the `(...)` suffix entirely (e.g. EnterPlanMode with `{}`).
        return None;
    }
    for key in ["path", "command", "pattern", "query", "url", "description"] {
        if let Some(text) = extract_partial_string_field(arguments, key) {
            let is_path = key == "path";
            return Some((one_line(&text), is_path));
        }
    }
    if let Some(path) = arguments
        .strip_prefix(r#"{"path":"#)
        .and_then(|rest| rest.strip_suffix(r#""}"#))
    {
        return Some((one_line(path), true));
    }
    Some((one_line(arguments), false))
}

#[must_use]
pub fn key_argument(arguments: Option<&str>) -> String {
    extract_key_argument(arguments)
        .map(|(value, _)| value)
        .unwrap_or_default()
}

/// Format a key argument for display in a tool header: workspace-relative for
/// paths, truncated to [`MAX_ARG_LENGTH`], with path keys preserving the tail.
fn format_key_argument(
    _tool_name: &str,
    value: &str,
    is_path: bool,
    workspace_dir: Option<&Path>,
) -> String {
    let value = if is_path {
        make_workspace_relative(value, workspace_dir)
    } else {
        value.to_owned()
    };
    truncate_arg_value(is_path, &value)
}

fn make_workspace_relative(path: &str, workspace_dir: Option<&Path>) -> String {
    let Some(workspace_dir) = workspace_dir else {
        return path.to_owned();
    };
    let Ok(canonical_workspace) = std::fs::canonicalize(workspace_dir) else {
        return path.to_owned();
    };
    let path_obj = Path::new(path);
    // Only relativize absolute paths that exist or at least start with the workspace prefix.
    if !path_obj.is_absolute() {
        return path.to_owned();
    }
    let Ok(canonical_path) = std::fs::canonicalize(path_obj) else {
        // Fall back to a simple prefix strip for paths that may not exist yet.
        return path
            .strip_prefix(&format!("{}/", canonical_workspace.display()))
            .unwrap_or(path)
            .to_owned();
    };
    canonical_path
        .strip_prefix(&canonical_workspace)
        .map_or_else(|_| path.to_owned(), |p| p.to_string_lossy().into_owned())
}

fn truncate_arg_value(is_path: bool, value: &str) -> String {
    let len = value.chars().count();
    if len <= MAX_ARG_LENGTH {
        return value.to_owned();
    }
    if is_path {
        // Preserve the tail (filename / deepest dirs) so the user can still
        // tell which file is being touched.
        format!("…{}", tail_chars(value, MAX_ARG_LENGTH - 1))
    } else {
        // Drop the tail for non-path arguments (e.g. long commands).
        format!("{}...", prefix_chars(value, MAX_ARG_LENGTH - 3))
    }
}

fn tail_chars(s: &str, n: usize) -> String {
    s.chars()
        .rev()
        .take(n)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn prefix_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn result_chip(state: &ToolCallState) -> String {
    if state.name == "Edit"
        && let Some(model) = state
            .details
            .as_ref()
            .and_then(DiffModel::from_tool_details)
    {
        return format!(" · +{} -{}", model.stats().added, model.stats().removed);
    }
    if state.name == "Write" {
        if let Some(line_count) = state
            .details
            .as_ref()
            .and_then(|details| details.get("line_count"))
            .and_then(serde_json::Value::as_u64)
        {
            return format!(" · {line_count} lines");
        }
        if let Some((_, content)) = parse_write_arguments(state.arguments.as_deref()) {
            return format!(" · {} lines", content.lines().count());
        }
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

// ---------------------------------------------------------------------------
// Streaming preview for Write/Edit tool arguments
// ---------------------------------------------------------------------------

/// Render a live preview while a tool's arguments are still streaming from the
/// model. For Write, reuses the final `render_write_preview` format (with line
/// numbers and syntax highlighting) so there is no format switch on completion.
/// For Edit, shows only a brief progress line.
#[must_use]
pub fn render_streaming_preview(
    state: &ToolCallState,
    expanded: bool,
    _width: usize,
    theme: &TuiTheme,
    _started_at: Option<std::time::Instant>,
) -> Vec<Line> {
    let args = state.arguments.as_deref().unwrap_or("");

    if state.name == "Write" {
        let path = extract_partial_string_field(args, "path").unwrap_or_default();
        let content = extract_partial_string_field(args, "content").unwrap_or_default();
        if content.is_empty() {
            return vec![Line::styled(
                "  Waiting for content...",
                Style::default().fg(theme.text_muted),
            )];
        }
        // Reuse the final preview renderer for format consistency.
        let palette = ToolBodyPalette::themed(theme);
        return render_write_preview(&path, &content, expanded, palette);
    }

    if state.name == "Edit" {
        let path = extract_partial_string_field(args, "path").unwrap_or_default();
        let tokens = estimate_tokens(args);
        return vec![Line::styled(
            format!("  Editing {path}... ~{} tok", format_token_count(tokens)),
            Style::default().fg(theme.text_muted),
        )];
    }

    Vec::new()
}

/// Public wrapper for token estimation (used by tool header streaming chip).
#[must_use]
pub fn estimate_tool_tokens(args: &str) -> usize {
    estimate_tokens(args)
}

/// Public wrapper for token count formatting (used by tool header streaming chip).
#[must_use]
pub fn format_tool_token_count(tokens: usize) -> String {
    format_token_count(tokens)
}
