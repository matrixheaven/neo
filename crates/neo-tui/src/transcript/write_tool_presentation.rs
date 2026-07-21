//! Pure structured Write tool presentation.

use crate::diff_model::DiffModel;
use crate::markdown::{highlight_code_lines, wrap_spans};
use crate::primitive::theme::TuiTheme;
use crate::primitive::{Line, Span, Style};
use crate::shell::ToolStatusKind;
use neo_agent_core::WriteApprovalPresentation;
use serde_json::Value;

use super::tool_renderers::{
    expand_tabs, framed_content_width, hard_wrap_line, render_code_frame, render_mutation_frame,
};

#[derive(Debug, Clone, Copy)]
pub struct WriteRenderInput<'a> {
    pub status: ToolStatusKind,
    pub arguments: Option<&'a str>,
    pub details: Option<&'a Value>,
    pub result: Option<&'a str>,
    pub expanded: bool,
    pub width: usize,
    pub theme: &'a TuiTheme,
}

#[must_use]
pub fn render_write_body_structured(input: WriteRenderInput<'_>) -> Vec<Line> {
    let kind = input
        .details
        .and_then(|details| details.get("kind"))
        .and_then(Value::as_str);
    if matches!(
        input.status,
        ToolStatusKind::Failed | ToolStatusKind::Cancelled
    ) && matches!(kind, Some("write_progress" | "write_prepared"))
    {
        return render_interrupted(input);
    }

    if let Some(details) = input.details {
        match kind {
            Some("write_prepared") => {
                return render_prepared_or_committed(
                    details,
                    input.expanded,
                    input.width,
                    input.theme,
                    false,
                );
            }
            Some("write_progress") => {
                return render_progress(details, input.width, input.theme);
            }
            Some("write") => {
                let status = details.get("status").and_then(Value::as_str).unwrap_or("");
                return match status {
                    "committed" => render_prepared_or_committed(
                        details,
                        input.expanded,
                        input.width,
                        input.theme,
                        true,
                    ),
                    "partial_commit" => render_terminal_changes(
                        "partial commit · already written files remain",
                        details,
                        input.expanded,
                        input.width,
                        input.theme,
                        input.theme.status_error,
                    ),
                    "durability_uncertain" => render_terminal_changes(
                        "contents installed · durability uncertain",
                        details,
                        input.expanded,
                        input.width,
                        input.theme,
                        input.theme.status_warn,
                    ),
                    "cancelled" => render_terminal_changes(
                        "cancelled · zero writes",
                        details,
                        input.expanded,
                        input.width,
                        input.theme,
                        input.theme.status_warn,
                    ),
                    "commit_failed" => render_terminal_changes(
                        "commit failed · zero writes",
                        details,
                        input.expanded,
                        input.width,
                        input.theme,
                        input.theme.status_error,
                    ),
                    "prepare_failed" => render_failure(
                        "prepare · zero writes",
                        details,
                        input.width,
                        input.theme,
                        input.theme.status_error,
                    ),
                    "stale" => render_failure(
                        "stale · zero writes",
                        details,
                        input.width,
                        input.theme,
                        input.theme.status_warn,
                    ),
                    _ => render_failure(
                        status,
                        details,
                        input.width,
                        input.theme,
                        input.theme.status_error,
                    ),
                };
            }
            _ => {}
        }
    }

    if matches!(
        input.status,
        ToolStatusKind::Pending | ToolStatusKind::Queued | ToolStatusKind::Running
    ) {
        return render_streaming_or_intent(input.arguments, input.width, input.theme);
    }
    if matches!(
        input.status,
        ToolStatusKind::Failed | ToolStatusKind::Cancelled
    ) {
        return render_interrupted(input);
    }
    Vec::new()
}

#[must_use]
pub fn render_write_approval(
    write: &WriteApprovalPresentation,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    let details = serde_json::to_value(write).unwrap_or_default();
    render_approval_prepared(&details, expanded, width, theme)
}

fn render_interrupted(input: WriteRenderInput<'_>) -> Vec<Line> {
    let warn = Style::default().fg(input.theme.status_warn);
    let muted = Style::default().fg(input.theme.text_muted);
    let mut rows = styled_wrapped(
        "interrupted · final commit state unknown",
        input.width,
        warn,
    );
    if let Some(details) = input.details {
        let committed = details
            .get("committed")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let total = details.get("total").and_then(Value::as_u64).unwrap_or(0);
        if total > 0 {
            rows.extend(styled_wrapped(
                &format!("last observed progress: {committed}/{total} files"),
                input.width,
                muted,
            ));
        }
    }
    if let Some(result) = input.result.filter(|text| !text.is_empty()) {
        rows.extend(styled_wrapped(result, input.width, muted));
    }
    rows
}

fn render_streaming_or_intent(
    arguments: Option<&str>,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    let muted = Style::default().fg(theme.text_muted);
    let args = arguments.unwrap_or("");
    let Some(files) = serde_json::from_str::<Value>(args)
        .ok()
        .and_then(|value| value.get("files").and_then(Value::as_array).cloned())
    else {
        return styled_wrapped("receiving structured changes...", width, muted);
    };
    if files.is_empty() {
        return styled_wrapped("receiving structured changes...", width, muted);
    }

    let mut rows = styled_wrapped(
        &format!("{} files · unverified intent", files.len()),
        width,
        muted,
    );
    for file in &files {
        let path = file.get("path").and_then(Value::as_str).unwrap_or("?");
        rows.extend(render_code_frame(
            Line::from_spans(vec![
                Span::styled("? ", muted),
                Span::styled(path.to_owned(), Style::default().fg(theme.text_primary)),
            ]),
            Vec::new(),
            width,
            Some(theme),
        ));
    }
    rows
}

fn render_prepared_or_committed(
    details: &Value,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
    committed: bool,
) -> Vec<Line> {
    let mut rows = Vec::new();
    let Some(changes) = details.get("changes").and_then(Value::as_array) else {
        return rows;
    };
    let selected = select_change_indices(changes.len(), expanded);
    let mut omitted_start = None;
    for (index, change) in changes.iter().enumerate() {
        if !selected.contains(&index) {
            omitted_start.get_or_insert(index);
            continue;
        }
        if let Some(start) = omitted_start.take() {
            rows.extend(render_omission(&changes[start..index], width, theme));
        }
        rows.extend(render_change_frame(
            change,
            expanded,
            width,
            theme,
            committed.then_some("committed"),
        ));
    }
    if let Some(start) = omitted_start {
        rows.extend(render_omission(&changes[start..], width, theme));
    }
    rows
}

/// Approval presentation uses a `preview` sub-object with `operation` tag.
fn render_approval_prepared(
    details: &Value,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    let mut rows = render_summary(details, width, theme, Some("verified".to_owned()));
    let Some(changes) = details.get("changes").and_then(Value::as_array) else {
        return rows;
    };
    let selected = select_change_indices(changes.len(), expanded);
    let mut omitted_start = None;
    for (index, change) in changes.iter().enumerate() {
        if !selected.contains(&index) {
            omitted_start.get_or_insert(index);
            continue;
        }
        if let Some(start) = omitted_start.take() {
            rows.extend(render_omission(&changes[start..index], width, theme));
        }
        rows.extend(render_approval_change_frame(change, expanded, width, theme));
    }
    if let Some(start) = omitted_start {
        rows.extend(render_omission(&changes[start..], width, theme));
    }
    rows
}

fn render_progress(details: &Value, width: usize, theme: &TuiTheme) -> Vec<Line> {
    let latest = details
        .get("latest_path")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let mut rows = Vec::new();
    rows.extend(render_code_frame(
        Line::from_spans(vec![
            Span::styled("✓ ", Style::default().fg(theme.status_ok)),
            Span::styled(latest.to_owned(), Style::default().fg(theme.text_primary)),
            Span::styled(" · committed", Style::default().fg(theme.text_muted)),
        ]),
        Vec::new(),
        width,
        Some(theme),
    ));
    rows.extend(styled_wrapped(
        "remaining files pending",
        width,
        Style::default().fg(theme.text_muted),
    ));
    rows
}

fn render_terminal_changes(
    label: &str,
    details: &Value,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
    accent: crate::primitive::Color,
) -> Vec<Line> {
    let mut rows = styled_wrapped(label, width, Style::default().fg(accent));
    let changes = details.get("changes").and_then(Value::as_array);
    let has_per_file_diagnostic = changes.is_some_and(|changes| {
        changes.iter().any(|change| {
            change
                .get("message")
                .and_then(Value::as_str)
                .is_some_and(|message| !message.is_empty())
        })
    });
    if !has_per_file_diagnostic
        && let Some(message) = details.get("message").and_then(Value::as_str)
    {
        rows.extend(styled_wrapped(message, width, Style::default().fg(accent)));
    }
    if let Some(changes) = changes {
        for change in changes {
            rows.extend(render_change_frame(change, expanded, width, theme, None));
        }
    }
    render_created_directories(details, width, theme, rows)
}

fn render_failure(
    label: &str,
    details: &Value,
    width: usize,
    theme: &TuiTheme,
    accent: crate::primitive::Color,
) -> Vec<Line> {
    let muted = Style::default().fg(theme.text_muted);
    let mut rows = styled_wrapped(label, width, Style::default().fg(accent));
    let diagnostics = details
        .get("message")
        .and_then(Value::as_str)
        .map(|message| styled_wrapped(message, framed_content_width(width), muted))
        .unwrap_or_default();
    if !diagnostics.is_empty() {
        let header = details
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("error details");
        rows.extend(render_code_frame(
            Line::styled(header.to_owned(), Style::default().fg(accent).bold()),
            diagnostics,
            width,
            Some(theme),
        ));
    }
    rows.extend(styled_wrapped(
        "Re-read affected files and submit a new Write call.",
        width,
        muted,
    ));
    render_created_directories(details, width, theme, rows)
}

fn render_created_directories(
    details: &Value,
    width: usize,
    theme: &TuiTheme,
    mut rows: Vec<Line>,
) -> Vec<Line> {
    if let Some(dirs) = details
        .get("created_directories")
        .and_then(Value::as_array)
        .filter(|dirs| !dirs.is_empty())
    {
        let muted = Style::default().fg(theme.text_muted);
        rows.extend(styled_wrapped("created directories:", width, muted));
        for dir in dirs {
            if let Some(path) = dir.as_str() {
                rows.extend(styled_wrapped(&format!("  {path}"), width, muted));
            }
        }
    }
    rows
}

fn render_summary(
    details: &Value,
    width: usize,
    theme: &TuiTheme,
    prefix: Option<String>,
) -> Vec<Line> {
    let files = details
        .get("files")
        .or_else(|| details.get("total"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let created = details.get("created").and_then(Value::as_u64).unwrap_or(0);
    let overwritten = details
        .get("overwritten")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let added = details.get("added").and_then(Value::as_u64).unwrap_or(0);
    let removed = details.get("removed").and_then(Value::as_u64).unwrap_or(0);
    let mut spans = Vec::new();
    if let Some(prefix) = prefix {
        spans.push(Span::styled(
            format!("{prefix} · "),
            Style::default().fg(theme.text_muted),
        ));
    }
    spans.extend([
        Span::styled(
            format!("{files} files · {created} created · {overwritten} overwritten · "),
            Style::default().fg(theme.text_muted),
        ),
        Span::styled(format!("+{added}"), Style::default().fg(theme.diff_added)),
        Span::styled(" ", Style::default()),
        Span::styled(
            format!("-{removed}"),
            Style::default().fg(theme.diff_removed),
        ),
    ]);
    hard_wrap_line(&Line::from_spans(spans), width)
}

fn render_change_frame(
    change: &Value,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
    override_status: Option<&str>,
) -> Vec<Line> {
    let path = change.get("path").and_then(Value::as_str).unwrap_or("?");
    let operation = change
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("created");
    let added = change.get("added").and_then(Value::as_u64).unwrap_or(0);
    let removed = change.get("removed").and_then(Value::as_u64).unwrap_or(0);
    let status = override_status.map(str::to_owned).or_else(|| {
        change
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });

    let (marker, marker_color) = match status.as_deref() {
        Some("committed") => ("✓", theme.status_ok),
        Some("committed_unsynced") => ("✓", theme.status_warn),
        Some("failed") => ("✗", theme.status_error),
        Some("not_attempted") => ("·", theme.text_muted),
        Some(_) => ("·", theme.text_muted),
        None if operation == "created" => ("A", theme.status_ok),
        None => ("M", theme.diff_hunk),
    };

    let applied_or_planned = status
        .as_deref()
        .is_none_or(|s| matches!(s, "committed" | "committed_unsynced"));
    let added_style = if applied_or_planned {
        Style::default().fg(theme.diff_added)
    } else {
        Style::default().fg(theme.text_muted)
    };
    let removed_style = if applied_or_planned {
        Style::default().fg(theme.diff_removed)
    } else {
        Style::default().fg(theme.text_muted)
    };

    let mut suffix = vec![Span::styled(
        format!(" · {operation}"),
        Style::default().fg(theme.text_muted),
    )];
    if operation == "created"
        && let Some(line_count) = change.get("line_count").and_then(Value::as_u64)
    {
        suffix.push(Span::styled(
            format!(" · {line_count} lines"),
            Style::default().fg(theme.text_muted),
        ));
    }
    suffix.extend([
        Span::styled(" · ", Style::default()),
        Span::styled(format!("+{added}"), added_style),
        Span::styled(" ", Style::default()),
        Span::styled(format!("-{removed}"), removed_style),
    ]);
    if let Some(status) = &status {
        suffix.push(Span::styled(
            format!(" · {status}"),
            Style::default().fg(marker_color),
        ));
    }

    let diagnostic = change
        .get("message")
        .and_then(Value::as_str)
        .map(|message| {
            styled_wrapped(
                message,
                framed_content_width(width),
                Style::default().fg(marker_color),
            )
        })
        .unwrap_or_default();
    let mut body = if status.as_deref() == Some("failed") {
        diagnostic.clone()
    } else if applied_or_planned {
        render_change_body(change, operation, path, expanded, width, theme)
    } else {
        Vec::new()
    };
    if status.as_deref() == Some("committed_unsynced") && !diagnostic.is_empty() {
        let mut with_diagnostic = diagnostic;
        with_diagnostic.append(&mut body);
        body = with_diagnostic;
    }
    render_mutation_frame(
        Span::styled(format!("{marker} "), Style::default().fg(marker_color)),
        path,
        Style::default().fg(theme.text_primary).bold(),
        suffix,
        body,
        width,
        Some(theme),
    )
}

fn render_change_body(
    change: &Value,
    operation: &str,
    path: &str,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    match operation {
        "created" => change
            .get("content")
            .and_then(Value::as_str)
            .map(|content| render_created_content(content, path, expanded, width, theme))
            .unwrap_or_default(),
        "overwritten" => change
            .get("diff")
            .and_then(Value::as_str)
            .and_then(DiffModel::parse_unified)
            .map(|model| {
                super::edit_tool_presentation::render_diff_preview_pub(
                    &model, path, expanded, width, theme,
                )
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn render_created_content(
    content: &str,
    path: &str,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    let content = expand_tabs(content);
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let collapsed = !expanded && total > 10;
    let content_width = framed_content_width(width);
    let highlighted = highlight_code_lines(&content, path, theme);
    let number_width = total.to_string().len();
    let prefix_width = if width < 7 {
        0
    } else {
        (number_width + 2).min(content_width.saturating_sub(1))
    };
    let code_width = content_width.saturating_sub(prefix_width).max(1);
    let mut rows = Vec::new();

    let indices = if collapsed {
        (0..5).chain(total - 5..total).collect::<Vec<_>>()
    } else {
        (0..total).collect::<Vec<_>>()
    };
    for index in indices {
        if collapsed && index == total - 5 {
            rows.push(Line::styled(
                format!(
                    "  ... ({} lines hidden, {total} total, ctrl+o to expand)",
                    total - 10
                ),
                Style::default().fg(theme.text_muted),
            ));
        }
        let line = lines[index];
        let code_spans = highlighted.get(index).cloned().unwrap_or_else(|| {
            vec![Span::styled(
                line.to_owned(),
                Style::default().fg(theme.text_primary),
            )]
        });
        for (visual_index, visual) in wrap_spans(&code_spans, code_width).into_iter().enumerate() {
            let prefix = if prefix_width == 0 {
                String::new()
            } else if visual_index == 0 {
                format!("{:>number_width$}  ", index + 1)
            } else {
                " ".repeat(prefix_width)
            };
            let mut spans = vec![Span::styled(prefix, Style::default().fg(theme.text_muted))];
            spans.extend(visual);
            rows.push(Line::from_spans(spans));
        }
    }
    rows
}

/// Approval changes use `preview.operation` + `preview.content`/`preview.diff`.
fn render_approval_change_frame(
    change: &Value,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    let path = change.get("path").and_then(Value::as_str).unwrap_or("?");
    let added = change.get("added").and_then(Value::as_u64).unwrap_or(0);
    let removed = change.get("removed").and_then(Value::as_u64).unwrap_or(0);
    let preview = change.get("preview");
    let operation = preview
        .and_then(|p| p.get("operation"))
        .and_then(Value::as_str)
        .unwrap_or("created");

    let (marker, marker_color) = if operation == "created" {
        ("A", theme.status_ok)
    } else {
        ("M", theme.diff_hunk)
    };

    let operation_label = if operation == "created" {
        "create"
    } else {
        "overwrite"
    };
    let mut suffix = vec![Span::styled(
        format!(" · {operation_label}"),
        Style::default().fg(theme.text_muted),
    )];
    if operation == "created"
        && let Some(line_count) = change.get("line_count").and_then(Value::as_u64)
    {
        suffix.push(Span::styled(
            format!(" · {line_count} lines"),
            Style::default().fg(theme.text_muted),
        ));
    }
    suffix.extend([
        Span::styled(" · ", Style::default()),
        Span::styled(format!("+{added}"), Style::default().fg(theme.diff_added)),
        Span::styled(" ", Style::default()),
        Span::styled(
            format!("-{removed}"),
            Style::default().fg(theme.diff_removed),
        ),
    ]);

    let body = match operation {
        "created" => preview
            .and_then(|p| p.get("content"))
            .and_then(Value::as_str)
            .map(|content| render_created_content(content, path, expanded, width, theme))
            .unwrap_or_default(),
        "overwritten" => preview
            .and_then(|p| p.get("diff"))
            .and_then(Value::as_str)
            .and_then(DiffModel::parse_unified)
            .map(|model| {
                super::edit_tool_presentation::render_diff_preview_pub(
                    &model, path, expanded, width, theme,
                )
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    render_mutation_frame(
        Span::styled(format!("{marker} "), Style::default().fg(marker_color)),
        path,
        Style::default().fg(theme.text_primary).bold(),
        suffix,
        body,
        width,
        Some(theme),
    )
}

fn render_omission(changes: &[Value], width: usize, theme: &TuiTheme) -> Vec<Line> {
    let changed_lines = changes
        .iter()
        .map(|change| {
            change.get("added").and_then(Value::as_u64).unwrap_or(0)
                + change.get("removed").and_then(Value::as_u64).unwrap_or(0)
        })
        .sum::<u64>();
    styled_wrapped(
        &format!(
            "... {} files · {changed_lines} changed lines hidden · ctrl+o to expand",
            changes.len()
        ),
        width,
        Style::default().fg(theme.text_muted),
    )
}

fn select_change_indices(len: usize, expanded: bool) -> Vec<usize> {
    if expanded || len <= 3 {
        return (0..len).collect();
    }
    vec![0, 1, len - 1]
}

fn styled_wrapped(text: &str, width: usize, style: Style) -> Vec<Line> {
    text.split('\n')
        .flat_map(|line| hard_wrap_line(&Line::styled(line.to_owned(), style), width))
        .collect()
}
