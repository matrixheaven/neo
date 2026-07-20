use std::borrow::Cow;
use std::fmt::Write as _;
use std::path::Path;

use crate::markdown::{highlight_code_lines, lang_from_path, wrap_spans};
use crate::primitive::theme::TuiTheme;
use crate::primitive::{Color, Style, clip_plain_to_width, truncate_width, visible_width};
use crate::primitive::{Line, Span, Text};
use crate::shell::ToolStatusKind;
use crate::token_estimate::{estimate_tokens, format_elapsed, format_token_count};

use super::partial_json::extract_partial_string_field;
use super::shell_tool_presentation;
use super::tool_call::ToolCallState;

const RESULT_PREVIEW_LINES: usize = 3;
const COMMAND_PREVIEW_LINES: usize = 10;
/// Maximum visible length of the key argument shown in a tool header.
/// Borrowed from Kimi Code: the argument is capped so the closing `)` can
/// always be rendered, regardless of terminal width.
const MAX_ARG_LENGTH: usize = 60;

/// Build the tool header as styled spans:
/// `{symbol} {verb} {name}[ · target] [({key})]{chip}`.
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
    header_width: usize,
) -> Vec<Span> {
    tool_header_spans_with_elapsed(state, theme, workspace_dir, header_width, None)
}

#[must_use]
pub fn tool_header_spans_with_elapsed(
    state: &ToolCallState,
    theme: &TuiTheme,
    workspace_dir: Option<&Path>,
    header_width: usize,
    elapsed_secs: Option<u64>,
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
    let shell_metadata = shell_tool_presentation::header_metadata(state, theme);
    if state.name == "WaitDelegate" {
        return wait_delegate_header_spans(state, theme, elapsed_secs, header_width);
    }
    if state.name == "Sleep" {
        return sleep_header_spans(state, theme, elapsed_secs, header_width);
    }
    if let Some(metadata) = shell_metadata {
        spans.extend(metadata);
    } else if let Some((key, is_path)) = extract_key_argument(state.arguments.as_deref()) {
        let key_text = format_key_argument(&state.name, &key, is_path, workspace_dir);
        spans.push(Span::styled(" (", Style::default().fg(meta_color)));
        spans.push(Span::styled(key_text, Style::default().fg(meta_color)));
        spans.push(Span::styled(")", Style::default().fg(meta_color)));
    }
    if let Some(chip) = list_delegates_header_chip(state) {
        spans.push(Span::styled(
            format!(" · {chip}"),
            Style::default().fg(meta_color),
        ));
    } else {
        let chip = result_chip(state);
        if !chip.is_empty() {
            spans.push(Span::styled(chip, Style::default().fg(meta_color)));
        }
    }
    spans
}

/// Render the semantic `WaitDelegate` header. `elapsed_secs` is intentionally
/// supplied by the live component; replayed cards have no trustworthy timer.
#[must_use]
pub fn wait_delegate_header_spans(
    state: &ToolCallState,
    theme: &TuiTheme,
    elapsed_secs: Option<u64>,
    max_width: usize,
) -> Vec<Span> {
    let (symbol, symbol_color, label) = if is_pending_or_running(state.status) {
        let (ids, timeout_ms) = wait_delegate_args(state.arguments.as_deref());
        let timeout = format_wait_duration(timeout_ms.unwrap_or(30_000));
        let elapsed = elapsed_secs
            .map(|seconds| format!(" · elapsed {}", format_elapsed(seconds)))
            .unwrap_or_default();
        let noun = if ids.len() == 1 {
            "delegate"
        } else {
            "delegates"
        };
        (
            "●",
            tool_status_color(state.status, theme),
            format!(
                "Waiting for {} {noun} · timeout {timeout}{elapsed}",
                ids.len(),
            ),
        )
    } else if let Some(details) = state.details.as_ref() {
        let outcome = details.get("outcome").and_then(serde_json::Value::as_str);
        let aggregate = details.get("aggregate");
        let total = aggregate_u64(aggregate, "total").unwrap_or_else(|| {
            details
                .get("items")
                .and_then(serde_json::Value::as_array)
                .map_or(0, Vec::len) as u64
        });
        let terminal = aggregate_u64(aggregate, "terminal").unwrap_or(0);
        let pending = aggregate_u64(aggregate, "pending").unwrap_or(0);
        let not_found = aggregate_u64(aggregate, "not_found").unwrap_or(0);
        let failed = wait_item_status_count(details, "failed");
        let cancelled = wait_item_status_count(details, "cancelled");
        let timed_out = wait_item_status_count(details, "timed_out");
        match outcome {
            Some("wait_timed_out") => (
                "◷",
                theme.status_cancelled,
                format!("Wait timed out · {terminal}/{total} terminal · {pending} still running"),
            ),
            Some("not_found") => (
                "?",
                theme.status_error,
                format!("Target not found · {not_found} unknown"),
            ),
            Some("all_terminal") => {
                let mut label = format!("Wait complete · {terminal} terminal");
                append_count(&mut label, failed, "failed");
                append_count(&mut label, cancelled, "cancelled");
                append_count(&mut label, timed_out, "timed out");
                ("●", theme.status_ok, label)
            }
            _ => return generic_wait_header(state, theme),
        }
    } else {
        return generic_wait_header(state, theme);
    };

    let prefix_width = visible_width(&format!("{symbol} "));
    let label = truncate_width(
        &label,
        max_width.saturating_sub(prefix_width).max(1),
        "...",
        false,
    );
    vec![
        Span::styled(format!("{symbol} "), Style::default().fg(symbol_color)),
        Span::styled(label, Style::default().fg(theme.brand).bold()),
    ]
}

fn generic_wait_header(state: &ToolCallState, theme: &TuiTheme) -> Vec<Span> {
    vec![
        Span::styled(
            format!("{} ", tool_symbol(state.status)),
            Style::default().fg(tool_status_color(state.status, theme)),
        ),
        Span::styled(
            format!("{} WaitDelegate", tool_verb(state.status)),
            Style::default().fg(theme.brand).bold(),
        ),
    ]
}

fn wait_delegate_args(arguments: Option<&str>) -> (Vec<String>, Option<u64>) {
    let Some(value) =
        arguments.and_then(|args| serde_json::from_str::<serde_json::Value>(args).ok())
    else {
        return (Vec::new(), None);
    };
    let ids = value
        .get("ids")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect();
    let timeout_ms = value.get("timeout_ms").and_then(serde_json::Value::as_u64);
    (ids, timeout_ms)
}

fn format_wait_duration(timeout_ms: u64) -> String {
    if timeout_ms > 0 && timeout_ms < 1_000 {
        return "<1s".to_owned();
    }
    format_elapsed(timeout_ms / 1_000)
}

fn aggregate_u64(value: Option<&serde_json::Value>, key: &str) -> Option<u64> {
    value?.get(key).and_then(serde_json::Value::as_u64)
}

fn wait_item_status_count(details: &serde_json::Value, status: &str) -> u64 {
    details
        .get("items")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("status").and_then(serde_json::Value::as_str) == Some(status))
        .count() as u64
}

fn append_count(label: &mut String, count: u64, noun: &str) {
    if count > 0 {
        let _ = write!(label, " · {count} {noun}");
    }
}

fn wait_status_marker(status: &str) -> &'static str {
    match status {
        "completed" => "✓",
        "failed" | "timed_out" => "✗",
        "cancelled" => "⊘",
        "not_found" => "?",
        _ => "…",
    }
}

fn wait_target_label(item: &serde_json::Value) -> String {
    item.get("title")
        .or_else(|| item.get("description"))
        .or_else(|| item.get("display_name"))
        .or_else(|| item.get("id"))
        .or_else(|| item.get("agent_id"))
        .or_else(|| item.get("swarm_id"))
        .and_then(serde_json::Value::as_str)
        .map(|value| truncate_arg_value(false, &one_line(value)))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown target".to_owned())
}

fn list_delegates_header_chip(state: &ToolCallState) -> Option<String> {
    if state.name != "ListDelegates" {
        return None;
    }
    let details = state.details.as_ref()?;
    if details.get("kind").and_then(serde_json::Value::as_str) != Some("delegate_list") {
        return None;
    }
    let count = details.get("count").and_then(serde_json::Value::as_u64)?;
    let total = details.get("total").and_then(serde_json::Value::as_u64)?;
    Some(format!("{count} of {total}"))
}

/// Structured `ListDelegates` body from `details.kind == "delegate_list"`.
/// Never exposes opaque pagination cursors. Falls back to generic rendering
/// when details are missing or malformed.
fn render_list_delegates_body(
    state: &ToolCallState,
    expanded: bool,
    width: usize,
    palette: ToolBodyPalette<'_>,
) -> Option<Vec<Line>> {
    if state.name != "ListDelegates" {
        return None;
    }
    let details = state.details.as_ref()?;
    if details.get("kind").and_then(serde_json::Value::as_str) != Some("delegate_list") {
        return None;
    }
    let delegates = details
        .get("delegates")
        .and_then(serde_json::Value::as_array)?;
    let content_width = width.saturating_sub(2).max(1);
    let mut rows = Vec::new();
    if delegates.is_empty() {
        rows.push(
            palette
                .weak_line("  No delegates found".to_owned())
                .truncate_to_width(content_width),
        );
    } else {
        let limit = if expanded {
            delegates.len()
        } else {
            RESULT_PREVIEW_LINES.min(delegates.len())
        };
        for (index, row) in delegates.iter().take(limit).enumerate() {
            let is_last = index + 1 == limit && (expanded || limit >= delegates.len());
            let branch = if is_last { "└─" } else { "├─" };
            let kind = row
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("agent");
            let line = if kind == "swarm" {
                let description = row
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .map(one_line)
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "swarm".to_owned());
                let status = row
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown");
                format!("  {branch} {description} · {status}")
            } else {
                let name = row
                    .get("display_name")
                    .and_then(serde_json::Value::as_str)
                    .map(one_line)
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "agent".to_owned());
                let status = row
                    .get("status")
                    .or_else(|| row.get("current_status"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown");
                let title = row
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .map(one_line)
                    .filter(|value| !value.is_empty());
                match title {
                    Some(title) => format!("  {branch} {name} · {status} · {title}"),
                    None => format!("  {branch} {name} · {status}"),
                }
            };
            rows.push(palette.body_line(line).truncate_to_width(content_width));
            if kind == "swarm"
                && let Some(aggregate_line) = list_delegates_swarm_aggregate_line(row)
            {
                let child_branch = if is_last { "   " } else { "│  " };
                rows.push(
                    palette
                        .weak_line(format!("  {child_branch}{aggregate_line}"))
                        .truncate_to_width(content_width),
                );
            }
        }
        if !expanded && delegates.len() > limit {
            rows.push(palette.weak_line(format!(
                "  ... ({} more, ctrl+o to expand)",
                delegates.len() - limit
            )));
        }
    }
    if let Some(steps) = details
        .get("next_steps")
        .and_then(serde_json::Value::as_array)
    {
        for step in steps {
            if let Some(text) = step
                .as_str()
                .map(one_line)
                .filter(|value| !value.is_empty())
            {
                rows.push(
                    palette
                        .weak_line(format!("  next: {text}"))
                        .truncate_to_width(content_width),
                );
            }
        }
    }
    Some(rows)
}

fn list_delegates_swarm_aggregate_line(row: &serde_json::Value) -> Option<String> {
    let aggregate = row.get("aggregate")?;
    let total = aggregate_u64(Some(aggregate), "total").unwrap_or(0);
    let running = aggregate_u64(Some(aggregate), "running").unwrap_or(0);
    let completed = aggregate_u64(Some(aggregate), "completed").unwrap_or(0);
    let failed = aggregate_u64(Some(aggregate), "failed").unwrap_or(0);
    let cancelled = aggregate_u64(Some(aggregate), "cancelled").unwrap_or(0);
    let timed_out = aggregate_u64(Some(aggregate), "timed_out").unwrap_or(0);
    let queued = aggregate_u64(Some(aggregate), "queued").unwrap_or(0);
    Some(format!(
        "aggregate total={total} queued={queued} running={running} completed={completed} failed={failed} cancelled={cancelled} timed_out={timed_out}"
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SleepArguments {
    pub duration_seconds: u64,
    pub reason: String,
}

#[must_use]
pub fn parse_sleep_arguments(arguments: Option<&str>) -> Option<SleepArguments> {
    let value = serde_json::from_str::<serde_json::Value>(arguments?).ok()?;
    let duration_seconds = value.get("duration_seconds")?.as_u64()?;
    let reason = value
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(one_line)?;
    Some(SleepArguments {
        duration_seconds,
        reason,
    })
}

/// Semantic Sleep header: total duration and, while running, remaining time.
#[must_use]
pub fn sleep_header_spans(
    state: &ToolCallState,
    theme: &TuiTheme,
    elapsed_secs: Option<u64>,
    max_width: usize,
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
    if let Some(args) = parse_sleep_arguments(state.arguments.as_deref()) {
        let total = format_elapsed(args.duration_seconds);
        spans.push(Span::styled(
            format!(" · {total} total"),
            Style::default().fg(meta_color),
        ));
        if state.status == ToolStatusKind::Running {
            let remaining = args
                .duration_seconds
                .saturating_sub(elapsed_secs.unwrap_or(0));
            spans.push(Span::styled(
                format!(" · {} remaining", format_elapsed(remaining)),
                Style::default().fg(meta_color),
            ));
        }
    }
    let _ = max_width;
    spans
}

/// Sleep body: reason while running/success; retain generic errors on failure.
fn render_sleep_body(
    state: &ToolCallState,
    expanded: bool,
    width: usize,
    palette: ToolBodyPalette<'_>,
) -> Option<Vec<Line>> {
    if state.name != "Sleep" {
        return None;
    }
    let args = parse_sleep_arguments(state.arguments.as_deref())?;
    let content_width = width.saturating_sub(2).max(1);
    let mut rows = vec![
        palette
            .body_line(format!("  {}", args.reason))
            .truncate_to_width(content_width),
    ];
    match state.status {
        ToolStatusKind::Succeeded => {
            // Suppress the generic "Waited ..." success body; reason remains.
            Some(rows)
        }
        ToolStatusKind::Failed | ToolStatusKind::Cancelled => {
            if let Some(mut error_rows) = render_result_body(state, expanded, width, palette) {
                rows.append(&mut error_rows);
            }
            Some(rows)
        }
        ToolStatusKind::Pending | ToolStatusKind::Queued | ToolStatusKind::Running => Some(rows),
    }
}

/// Render structured `WaitDelegate` target rows. Malformed/validation results
/// return `None` so the existing generic result renderer remains the fallback.
fn render_wait_delegate_body(
    state: &ToolCallState,
    expanded: bool,
    width: usize,
    palette: ToolBodyPalette<'_>,
) -> Option<Vec<Line>> {
    if state.name != "WaitDelegate" {
        return None;
    }
    if is_pending_or_running(state.status) {
        let (ids, _) = wait_delegate_args(state.arguments.as_deref());
        if ids.is_empty() {
            return None;
        }
        if !expanded {
            return Some(Vec::new());
        }
        return Some(
            ids.into_iter()
                .map(|id| {
                    palette
                        .body_line(format!("  … {id} · waiting"))
                        .truncate_to_width(width.saturating_sub(2).max(1))
                })
                .collect(),
        );
    }
    let details = state.details.as_ref()?;
    if details.get("kind").and_then(serde_json::Value::as_str) != Some("delegate_wait") {
        return None;
    }
    let items = details.get("items").and_then(serde_json::Value::as_array)?;
    let limit = if expanded {
        usize::MAX
    } else {
        RESULT_PREVIEW_LINES
    };
    let mut rows = Vec::new();
    for item in items.iter().take(limit) {
        let status = item
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        let label = wait_target_label(item);
        let line = format!("  {} {} · {status}", wait_status_marker(status), label);
        rows.push(
            palette
                .body_line(line)
                .truncate_to_width(width.saturating_sub(2).max(1)),
        );
    }
    if !expanded && items.len() > limit {
        rows.push(palette.weak_line(format!(
            "  ... ({} more targets, ctrl+o to expand)",
            items.len() - limit
        )));
    }
    Some(rows)
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
        // the grouped tool card. Queued keeps the same living-entry marker.
        ToolStatusKind::Pending
        | ToolStatusKind::Queued
        | ToolStatusKind::Running
        | ToolStatusKind::Succeeded => "●",
        ToolStatusKind::Failed => "✗",
        ToolStatusKind::Cancelled => "⊘",
    }
}

fn tool_verb(status: ToolStatusKind) -> &'static str {
    match status {
        ToolStatusKind::Pending => "Preparing",
        ToolStatusKind::Queued => "Queued",
        ToolStatusKind::Running => "Using",
        ToolStatusKind::Succeeded => "Used",
        ToolStatusKind::Failed => "Failed",
        ToolStatusKind::Cancelled => "Cancelled",
    }
}

fn tool_status_color(status: ToolStatusKind, theme: &TuiTheme) -> Color {
    match status {
        ToolStatusKind::Pending | ToolStatusKind::Queued => theme.status_pending,
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
        .or_else(|| render_write_body(state, expanded, width, palette))
        .or_else(|| {
            (state.name == "WaitDelegate")
                .then(|| render_wait_delegate_body(state, expanded, width, palette))
                .flatten()
        })
        .or_else(|| render_list_delegates_body(state, expanded, width, palette))
        .or_else(|| render_sleep_body(state, expanded, width, palette))
        .or_else(|| render_result_body(state, expanded, width, palette))
        .unwrap_or_default()
}

fn render_diff_details(
    state: &ToolCallState,
    expanded: bool,
    width: usize,
    palette: ToolBodyPalette<'_>,
) -> Option<Vec<Line>> {
    // Only Edit uses structured batch presentation. Write always uses a
    // syntax-highlighted content preview (via render_write_body).
    if state.name != "Edit" {
        return None;
    }
    let theme = palette.theme?;
    Some(super::edit_tool_presentation::render_edit_body(
        super::edit_tool_presentation::EditRenderInput {
            status: state.status,
            arguments: state.arguments.as_deref(),
            details: state.details.as_ref(),
            result: state.result.as_deref(),
            expanded,
            width,
            theme,
        },
    ))
}

fn render_write_body(
    state: &ToolCallState,
    expanded: bool,
    width: usize,
    palette: ToolBodyPalette<'_>,
) -> Option<Vec<Line>> {
    if state.name != "Write" {
        return None;
    }

    let (path, content) = parse_write_arguments(state.arguments.as_deref())?;
    Some(render_write_preview(
        &path, &content, expanded, width, palette,
    ))
}

fn render_write_preview(
    path: &str,
    content: &str,
    expanded: bool,
    width: usize,
    palette: ToolBodyPalette<'_>,
) -> Vec<Line> {
    let content = expand_tabs(content);
    let content = content.as_ref();
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let limit = preview_limit(total, expanded, COMMAND_PREVIEW_LINES);
    let mut rows = Vec::new();
    let content_width = framed_content_width(width);

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
        let prefix_width = if width < 7 {
            0
        } else {
            6.min(content_width.saturating_sub(1))
        };
        let code_width = content_width.saturating_sub(prefix_width).max(1);
        let code_spans = if use_highlight {
            highlighted.get(index).cloned().unwrap_or_default()
        } else {
            vec![Span::styled(
                (*line).to_owned(),
                palette.theme.map_or_else(Style::default, |theme| {
                    Style::default().fg(theme.text_primary)
                }),
            )]
        };
        for (visual_index, visual) in wrap_spans(&code_spans, code_width).into_iter().enumerate() {
            let prefix = if prefix_width == 0 {
                String::new()
            } else if visual_index == 0 {
                format!("{:>4}  ", index + 1)
            } else {
                " ".repeat(prefix_width)
            };
            let mut spans = vec![Span::styled(
                prefix,
                palette.theme.map_or_else(Style::default, |theme| {
                    Style::default().fg(theme.text_muted)
                }),
            )];
            spans.extend(visual);
            rows.push(Line::from_spans(spans));
        }
    }
    if limit < total {
        rows.push(palette.weak_line(format!(
            "  ... ({} more lines, {total} total, ctrl+o to expand)",
            total - limit
        )));
    }
    let header = match palette.theme {
        Some(theme) => Line::from_spans(vec![
            Span::styled(
                path.to_owned(),
                Style::default().fg(theme.text_primary).bold(),
            ),
            Span::styled(
                format!(" · {total} lines"),
                Style::default().fg(theme.text_muted),
            ),
        ]),
        None => Line::raw(format!("{path} · {total} lines")),
    };
    render_code_frame(header, rows, width, palette.theme)
}

pub(super) fn framed_content_width(width: usize) -> usize {
    if width < 7 {
        width.max(1)
    } else {
        width.saturating_sub(4)
    }
}

/// Full-width rounded frame shared by Edit and Write projections.
pub(super) fn render_code_frame(
    header: Line,
    body: Vec<Line>,
    width: usize,
    theme: Option<&TuiTheme>,
) -> Vec<Line> {
    if width < 7 {
        return std::iter::once(header)
            .chain(body)
            .flat_map(|line| hard_wrap_line(&line, width.max(1)))
            .collect();
    }

    let border = theme.map_or_else(Style::default, |theme| {
        Style::default().fg(theme.surface_border)
    });
    let inner = framed_content_width(width);
    let horizontal = "─".repeat(width - 2);
    let mut rows = vec![Line::styled(format!("╭{horizontal}╮"), border)];
    for line in std::iter::once(header).chain(body) {
        for wrapped in hard_wrap_line(&line, inner) {
            let padding = " ".repeat(inner.saturating_sub(wrapped.visible_width()));
            let mut spans = vec![Span::styled("│ ", border)];
            spans.extend(wrapped.into_spans());
            spans.push(Span::raw(padding));
            spans.push(Span::styled(" │", border));
            rows.push(Line::from_spans(spans));
        }
    }
    rows.push(Line::styled(format!("╰{horizontal}╯"), border));
    rows
}

pub(super) fn hard_wrap_line(line: &Line, width: usize) -> Vec<Line> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut current = Vec::new();
    let mut current_width = 0usize;

    for span in line.spans() {
        let expanded_tabs = expand_tabs(span.text());
        let mut remaining = expanded_tabs.as_ref();
        while !remaining.is_empty() {
            let available = width.saturating_sub(current_width);
            let chunk = clip_plain_to_width(remaining, available);
            if chunk.is_empty() {
                if !current.is_empty() {
                    rows.push(Line::from_spans(std::mem::take(&mut current)));
                    current_width = 0;
                    continue;
                }
                let end = remaining
                    .char_indices()
                    .nth(1)
                    .map_or(remaining.len(), |(index, _)| index);
                let chunk = &remaining[..end];
                current.push(Span::styled(chunk.to_owned(), span.style()));
                remaining = &remaining[end..];
                rows.push(Line::from_spans(std::mem::take(&mut current)));
                continue;
            }
            current_width += visible_width(&chunk);
            let consumed = chunk.len();
            current.push(Span::styled(chunk, span.style()));
            remaining = &remaining[consumed..];
            if !remaining.is_empty() {
                rows.push(Line::from_spans(std::mem::take(&mut current)));
                current_width = 0;
            }
        }
    }
    if !current.is_empty() || rows.is_empty() {
        rows.push(Line::from_spans(current));
    }
    rows
}

pub(super) fn expand_tabs(text: &str) -> Cow<'_, str> {
    if text.contains('\t') {
        Cow::Owned(text.replace('\t', "    "))
    } else {
        Cow::Borrowed(text)
    }
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

pub(super) fn render_text_preview_themed(
    text: &str,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    render_result_preview(text, expanded, width, ToolBodyPalette::themed(theme))
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

fn parse_write_arguments(arguments: Option<&str>) -> Option<(String, String)> {
    let value = serde_json::from_str::<serde_json::Value>(arguments?).ok()?;
    let path = string_field(&value, "path")?;
    let content = string_field(&value, "content")?;
    Some((path, content))
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
        if let Some(path) = value
            .get("files")
            .and_then(serde_json::Value::as_array)
            .and_then(|files| files.first())
            .and_then(|file| file.get("path"))
            .and_then(serde_json::Value::as_str)
        {
            return Some((one_line(path), true));
        }
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

pub(super) fn make_workspace_relative(path: &str, workspace_dir: Option<&Path>) -> String {
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
        && let Some(details) = state.details.as_ref()
        && matches!(
            details.get("kind").and_then(serde_json::Value::as_str),
            Some("edit" | "edit_progress")
        )
    {
        let added = details
            .get("added")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let removed = details
            .get("removed")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        return format!(" · +{added} -{removed}");
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
    width: usize,
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
        return render_write_preview(&path, &content, expanded, width, palette);
    }

    if state.name == "Edit" {
        return super::edit_tool_presentation::render_edit_body(
            super::edit_tool_presentation::EditRenderInput {
                status: state.status,
                arguments: Some(args),
                details: state.details.as_ref(),
                result: state.result.as_deref(),
                expanded,
                width,
                theme,
            },
        );
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
