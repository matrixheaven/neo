//! Pure structured Edit tool presentation.

use crate::diff_model::{DiffLine, DiffModel};
use crate::markdown::{highlight_code_lines, wrap_spans};
use crate::primitive::theme::TuiTheme;
use crate::primitive::{Color, Line, Span, Style};
use crate::shell::ToolStatusKind;
use neo_agent_core::EditApprovalPresentation;
use serde_json::Value;

use super::tool_renderers::{
    expand_tabs, framed_content_width, hard_wrap_line, render_code_frame, render_mutation_frame,
};

#[derive(Debug, Clone, Copy)]
pub struct EditRenderInput<'a> {
    pub status: ToolStatusKind,
    pub arguments: Option<&'a str>,
    pub details: Option<&'a Value>,
    pub result: Option<&'a str>,
    pub expanded: bool,
    pub width: usize,
    pub theme: &'a TuiTheme,
}

#[must_use]
pub fn render_edit_body(input: EditRenderInput<'_>) -> Vec<Line> {
    let kind = input
        .details
        .and_then(|details| details.get("kind"))
        .and_then(Value::as_str);
    if matches!(
        input.status,
        ToolStatusKind::Failed | ToolStatusKind::Cancelled
    ) && matches!(kind, Some("edit_progress" | "edit_prepared"))
    {
        return render_interrupted(input);
    }

    if let Some(details) = input.details {
        match kind {
            Some("edit_prepared") => {
                return render_prepared_or_committed(
                    details,
                    input.expanded,
                    input.width,
                    input.theme,
                    false,
                );
            }
            Some("edit_progress") => {
                return render_progress(details, input.width, input.theme);
            }
            Some("edit") => {
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
pub fn render_edit_approval(
    edit: &EditApprovalPresentation,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    let details = serde_json::to_value(edit).unwrap_or_default();
    render_prepared_or_committed(&details, expanded, width, theme, false)
}

fn render_interrupted(input: EditRenderInput<'_>) -> Vec<Line> {
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
    let Some(edits) = serde_json::from_str::<Value>(args)
        .ok()
        .and_then(|value| value.get("edits").and_then(Value::as_array).cloned())
    else {
        return styled_wrapped("receiving structured changes...", width, muted);
    };
    if edits.is_empty() {
        return styled_wrapped("receiving structured changes...", width, muted);
    }

    let mut path_counts: Vec<(&str, usize)> = Vec::new();
    for edit in &edits {
        let path = edit.get("path").and_then(Value::as_str).unwrap_or("?");
        if let Some(entry) = path_counts.iter_mut().find(|(p, _)| *p == path) {
            entry.1 += 1;
        } else {
            path_counts.push((path, 1));
        }
    }
    let mut rows = styled_wrapped(
        &format!(
            "{} files · {} replacements · unverified intent",
            path_counts.len(),
            edits.len()
        ),
        width,
        muted,
    );
    for (path, count) in &path_counts {
        rows.extend(render_code_frame(
            Line::from_spans(vec![
                Span::styled("? ", muted),
                Span::styled((*path).to_owned(), Style::default().fg(theme.text_primary)),
                Span::styled(format!(" · {count} replacements"), muted),
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
    let mut rows = if committed {
        Vec::new()
    } else {
        render_summary(details, width, theme, Some("verified".to_owned()))
    };
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
            true,
        ));
    }
    if let Some(start) = omitted_start {
        rows.extend(render_omission(&changes[start..], width, theme));
    }
    rows
}

fn render_progress(details: &Value, width: usize, theme: &TuiTheme) -> Vec<Line> {
    let committed = details
        .get("committed")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total = details.get("total").and_then(Value::as_u64).unwrap_or(0);
    let latest = details
        .get("latest_path")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let mut rows = render_applied_stats(
        details,
        width,
        theme,
        &format!("committing {committed}/{total} files"),
    );
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
    rows.extend(render_applied_stats(
        details,
        width,
        theme,
        "applied changes",
    ));
    if let Some(changes) = details.get("changes").and_then(Value::as_array) {
        for change in changes {
            let status = change
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let show_diff = matches!(status, "committed" | "committed_unsynced");
            rows.extend(render_change_frame(
                change,
                expanded,
                width,
                theme,
                Some(status),
                show_diff,
            ));
        }
    }
    rows
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
        "Re-read affected files and submit a new Edit call.",
        width,
        muted,
    ));
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
    let replacements = details
        .get("replacements")
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
            format!("{files} files · {replacements} replacements · "),
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

fn render_applied_stats(details: &Value, width: usize, theme: &TuiTheme, label: &str) -> Vec<Line> {
    let added = details.get("added").and_then(Value::as_u64).unwrap_or(0);
    let removed = details.get("removed").and_then(Value::as_u64).unwrap_or(0);
    hard_wrap_line(
        &Line::from_spans(vec![
            Span::styled(format!("{label} · "), Style::default().fg(theme.text_muted)),
            Span::styled(format!("+{added}"), Style::default().fg(theme.diff_added)),
            Span::styled(" ", Style::default()),
            Span::styled(
                format!("-{removed}"),
                Style::default().fg(theme.diff_removed),
            ),
        ]),
        width,
    )
}

fn render_change_frame(
    change: &Value,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
    status: Option<&str>,
    show_diff: bool,
) -> Vec<Line> {
    let path = change.get("path").and_then(Value::as_str).unwrap_or("?");
    let added = change.get("added").and_then(Value::as_u64).unwrap_or(0);
    let removed = change.get("removed").and_then(Value::as_u64).unwrap_or(0);
    let replacements = change
        .get("replacements")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let (marker, marker_color) = match status {
        Some("committed") => ("✓", theme.status_ok),
        Some("committed_unsynced") => ("✓", theme.status_warn),
        Some("failed") => ("✗", theme.status_error),
        Some("not_attempted") => ("·", theme.text_muted),
        Some(_) => ("·", theme.text_muted),
        None => ("M", theme.diff_hunk),
    };
    let applied_or_planned =
        status.is_none() || matches!(status, Some("committed" | "committed_unsynced"));
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
    let mut suffix = Vec::new();
    if let Some(status) = status {
        suffix.push(Span::styled(
            format!(" · {status}"),
            Style::default().fg(marker_color),
        ));
    }
    suffix.extend([
        Span::styled(
            format!(" · {replacements} replacements · "),
            Style::default().fg(theme.text_muted),
        ),
        Span::styled(format!("+{added}"), added_style),
        Span::styled(" ", Style::default()),
        Span::styled(format!("-{removed}"), removed_style),
    ]);
    let body = if show_diff {
        change
            .get("diff")
            .and_then(Value::as_str)
            .and_then(DiffModel::parse_unified)
            .map(|model| render_diff_preview(&model, path, expanded, width, theme))
            .unwrap_or_default()
    } else {
        Vec::new()
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

pub(super) fn render_diff_preview_pub(
    model: &DiffModel,
    path: &str,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    render_diff_preview(model, path, expanded, width, theme)
}

fn render_diff_preview(
    model: &DiffModel,
    path: &str,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    let number_width = model
        .files()
        .iter()
        .flat_map(|file| &file.hunks)
        .map(|hunk| hunk.old_start.max(hunk.new_start) + hunk.lines.len())
        .max()
        .unwrap_or(1)
        .to_string()
        .len();
    let content_width = framed_content_width(width);
    let prefix_width = if width < 7 { 0 } else { number_width + 3 };
    let code_width = content_width.saturating_sub(prefix_width).max(1);
    let mut logical = Vec::new();

    for file in model.files() {
        for hunk in &file.hunks {
            let source = hunk
                .lines
                .iter()
                .map(|line| expand_tabs(diff_text(line)).into_owned())
                .collect::<Vec<_>>()
                .join("\n");
            let highlighted = highlight_code_lines(&source, path, theme);
            let mut old_line = hunk.old_start;
            let mut new_line = hunk.new_start;
            for (index, line) in hunk.lines.iter().enumerate() {
                let (line_number, sign, gutter_color) = match line {
                    DiffLine::Context(_) => {
                        let number = new_line;
                        old_line += 1;
                        new_line += 1;
                        (number, ' ', theme.diff_context)
                    }
                    DiffLine::Added(_) => {
                        let number = new_line;
                        new_line += 1;
                        (number, '+', theme.diff_added)
                    }
                    DiffLine::Removed(_) => {
                        let number = old_line;
                        old_line += 1;
                        (number, '-', theme.diff_removed)
                    }
                };
                let code = highlighted.get(index).cloned().unwrap_or_else(|| {
                    vec![Span::styled(
                        expand_tabs(diff_text(line)).into_owned(),
                        Style::default().fg(theme.text_primary),
                    )]
                });
                logical.push(LogicalDiffLine {
                    line_number,
                    sign,
                    gutter_color,
                    code,
                });
            }
        }
    }

    let selected = select_diff_lines(&logical, expanded);
    let mut rows = Vec::new();
    let mut omitted = 0usize;
    for (index, line) in logical.iter().enumerate() {
        if !selected[index] {
            omitted += 1;
            continue;
        }
        if omitted > 0 {
            rows.push(diff_omission_line(omitted, theme));
            omitted = 0;
        }
        for (visual_index, visual) in wrap_spans(&line.code, code_width).into_iter().enumerate() {
            let mut spans = if visual_index == 0 && prefix_width > 0 {
                vec![
                    Span::styled(
                        format!("{:>number_width$} ", line.line_number),
                        Style::default().fg(line.gutter_color),
                    ),
                    Span::styled(
                        format!("{} ", line.sign),
                        Style::default().fg(line.gutter_color),
                    ),
                ]
            } else {
                vec![Span::raw(" ".repeat(prefix_width))]
            };
            spans.extend(visual);
            rows.push(Line::from_spans(spans));
        }
    }
    if omitted > 0 {
        rows.push(diff_omission_line(omitted, theme));
    }
    rows
}

struct LogicalDiffLine {
    line_number: usize,
    sign: char,
    gutter_color: Color,
    code: Vec<Span>,
}

fn select_diff_lines(lines: &[LogicalDiffLine], expanded: bool) -> Vec<bool> {
    if expanded {
        return vec![true; lines.len()];
    }
    let changes = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (line.sign != ' ').then_some(index))
        .collect::<Vec<_>>();
    let Some((&first, rest)) = changes.split_first() else {
        return vec![true; lines.len()];
    };
    let mut clusters = Vec::new();
    let mut start = first;
    let mut end = first;
    for &index in rest {
        if index.saturating_sub(end) <= 5 {
            end = index;
        } else {
            clusters.push((start, end));
            start = index;
            end = index;
        }
    }
    clusters.push((start, end));

    let mut selected = vec![false; lines.len()];
    for &(start, end) in [clusters.first(), clusters.last()].into_iter().flatten() {
        let start = start.saturating_sub(2);
        let end = (end + 2).min(lines.len().saturating_sub(1));
        selected[start..=end].fill(true);
    }
    selected
}

fn diff_omission_line(count: usize, theme: &TuiTheme) -> Line {
    Line::styled(
        format!("... {count} diff lines hidden · ctrl+o to expand"),
        Style::default().fg(theme.text_muted),
    )
}

fn diff_text(line: &DiffLine) -> &str {
    match line {
        DiffLine::Context(text) | DiffLine::Added(text) | DiffLine::Removed(text) => text,
    }
}

fn render_omission(changes: &[Value], width: usize, theme: &TuiTheme) -> Vec<Line> {
    let replacements = changes
        .iter()
        .filter_map(|change| change.get("replacements").and_then(Value::as_u64))
        .sum::<u64>();
    let changed_lines = changes
        .iter()
        .map(|change| {
            change.get("added").and_then(Value::as_u64).unwrap_or(0)
                + change.get("removed").and_then(Value::as_u64).unwrap_or(0)
        })
        .sum::<u64>();
    styled_wrapped(
        &format!(
            "... {} files · {replacements} replacements · {changed_lines} changed lines hidden · ctrl+o to expand",
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
