//! Pure structured Edit tool presentation.
//!
//! Stateful ownership stays on `ToolCallComponent`. This module only builds
//! rows from verified structured details, raw streaming arguments, width, and
//! global expansion.

use crate::diff_model::{DiffLine, DiffModel};
use crate::primitive::theme::TuiTheme;
use crate::primitive::{Line, Style, wrap_width};
use crate::shell::ToolStatusKind;
use serde_json::Value;

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
    let muted = Style::default().fg(input.theme.text_muted);
    let body = Style::default().fg(input.theme.text_primary);
    let success = Style::default().fg(input.theme.status_ok);
    let error = Style::default().fg(input.theme.status_error);
    let warn = Style::default().fg(input.theme.status_warn);

    if let Some(details) = input.details {
        if let Some(kind) = details.get("kind").and_then(Value::as_str) {
            match kind {
                "edit_prepared" => {
                    return render_prepared_or_committed(
                        details,
                        input.expanded,
                        input.width,
                        body,
                        muted,
                        false,
                    );
                }
                "edit_progress" => {
                    return render_progress(details, input.width, body, muted, success);
                }
                "edit" => {
                    let status = details.get("status").and_then(Value::as_str).unwrap_or("");
                    return match status {
                        "committed" => render_prepared_or_committed(
                            details,
                            input.expanded,
                            input.width,
                            body,
                            muted,
                            true,
                        ),
                        "prepare_failed" => render_failure(
                            "prepare · zero writes",
                            details,
                            input.width,
                            error,
                            muted,
                        ),
                        "stale" => {
                            render_failure("stale · zero writes", details, input.width, warn, muted)
                        }
                        "partial_commit" => {
                            render_partial(details, input.expanded, input.width, body, muted, error)
                        }
                        "durability_uncertain" => render_failure(
                            "contents installed · durability uncertain",
                            details,
                            input.width,
                            warn,
                            muted,
                        ),
                        _ => render_failure(status, details, input.width, error, muted),
                    };
                }
                _ => {}
            }
        }
    }

    // Interrupted / running without structured details: argument intent only.
    if matches!(
        input.status,
        ToolStatusKind::Pending | ToolStatusKind::Queued | ToolStatusKind::Running
    ) {
        return render_streaming_or_intent(input.arguments, input.width, muted, body);
    }

    if matches!(
        input.status,
        ToolStatusKind::Failed | ToolStatusKind::Cancelled
    ) {
        let mut rows = vec![Line::styled(
            wrap_line("  interrupted · final commit state unknown", input.width),
            warn,
        )];
        if let Some(result) = input.result.filter(|text| !text.is_empty()) {
            for line in result.lines().take(4) {
                rows.push(Line::styled(
                    wrap_line(&format!("  {line}"), input.width),
                    muted,
                ));
            }
        }
        return rows;
    }

    Vec::new()
}

fn render_streaming_or_intent(
    arguments: Option<&str>,
    width: usize,
    muted: Style,
    body: Style,
) -> Vec<Line> {
    let args = arguments.unwrap_or("");
    if args.trim().is_empty() || !args.contains('{') {
        return vec![Line::styled(
            wrap_line("  receiving structured changes...", width),
            muted,
        )];
    }
    let parsed = serde_json::from_str::<Value>(args).ok();
    let Some(files) = parsed
        .as_ref()
        .and_then(|value| value.get("files"))
        .and_then(Value::as_array)
    else {
        return vec![Line::styled(
            wrap_line("  receiving structured changes...", width),
            muted,
        )];
    };
    let mut rows = Vec::new();
    let mut total_replacements = 0usize;
    for file in files {
        let path = file.get("path").and_then(Value::as_str).unwrap_or("?");
        let replacements = file
            .get("replacements")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        total_replacements += replacements;
        rows.push(Line::styled(
            wrap_line(
                &format!("  ? {path}       {replacements} replacements"),
                width,
            ),
            body,
        ));
    }
    if rows.is_empty() {
        return vec![Line::styled(
            wrap_line("  receiving structured changes...", width),
            muted,
        )];
    }
    let mut out = vec![Line::styled(
        wrap_line(
            &format!(
                "  {} files · {} replacements (unverified intent)",
                files.len(),
                total_replacements
            ),
            width,
        ),
        muted,
    )];
    out.extend(rows);
    out
}

fn render_prepared_or_committed(
    details: &Value,
    expanded: bool,
    width: usize,
    body: Style,
    muted: Style,
    show_check: bool,
) -> Vec<Line> {
    let files = details.get("files").and_then(Value::as_u64).unwrap_or(0);
    let replacements = details
        .get("replacements")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let added = details.get("added").and_then(Value::as_u64).unwrap_or(0);
    let removed = details.get("removed").and_then(Value::as_u64).unwrap_or(0);
    let mut rows = vec![Line::styled(
        wrap_line(
            &format!("  {files} files · {replacements} replacements · +{added} -{removed}"),
            width,
        ),
        muted,
    )];

    if let Some(changes) = details.get("changes").and_then(Value::as_array) {
        let selected = select_change_indices(changes.len(), expanded);
        let mut omitted_files = 0usize;
        let mut omitted_replacements = 0usize;
        let mut omitted_lines = 0usize;
        for (index, change) in changes.iter().enumerate() {
            if !selected.contains(&index) {
                omitted_files += 1;
                omitted_replacements += change
                    .get("replacements")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
                omitted_lines += change.get("added").and_then(Value::as_u64).unwrap_or(0) as usize
                    + change.get("removed").and_then(Value::as_u64).unwrap_or(0) as usize;
                continue;
            }
            if omitted_files > 0 {
                rows.push(Line::styled(
                    wrap_line(
                        &format!(
                            "  ... {omitted_files} files · {omitted_replacements} replacements · {omitted_lines} changed lines hidden · ctrl+o to expand"
                        ),
                        width,
                    ),
                    muted,
                ));
                omitted_files = 0;
                omitted_replacements = 0;
                omitted_lines = 0;
            }
            let path = change.get("path").and_then(Value::as_str).unwrap_or("?");
            let c_added = change.get("added").and_then(Value::as_u64).unwrap_or(0);
            let c_removed = change.get("removed").and_then(Value::as_u64).unwrap_or(0);
            let marker = if show_check { "✓" } else { "M" };
            rows.push(Line::styled(
                wrap_line(
                    &format!("  {marker} {path}              +{c_added} -{c_removed}"),
                    width,
                ),
                body,
            ));
            if let Some(diff) = change.get("diff").and_then(Value::as_str)
                && let Some(model) = DiffModel::parse_unified(diff)
            {
                for line in model_preview_lines(&model, expanded, width, muted, body) {
                    rows.push(line);
                }
            }
        }
        if omitted_files > 0 {
            rows.push(Line::styled(
                wrap_line(
                    &format!(
                        "  ... {omitted_files} files · {omitted_replacements} replacements · {omitted_lines} changed lines hidden · ctrl+o to expand"
                    ),
                    width,
                ),
                muted,
            ));
        }
    }
    rows
}

fn render_progress(
    details: &Value,
    width: usize,
    body: Style,
    muted: Style,
    success: Style,
) -> Vec<Line> {
    let committed = details
        .get("committed")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total = details.get("total").and_then(Value::as_u64).unwrap_or(0);
    let latest = details
        .get("latest_path")
        .and_then(Value::as_str)
        .unwrap_or("");
    let added = details.get("added").and_then(Value::as_u64).unwrap_or(0);
    let removed = details.get("removed").and_then(Value::as_u64).unwrap_or(0);
    vec![
        Line::styled(
            wrap_line(
                &format!("  committing {committed}/{total} files · +{added} -{removed}"),
                width,
            ),
            muted,
        ),
        Line::styled(wrap_line(&format!("  ✓ {latest}"), width), success),
        Line::styled(wrap_line("  remaining files pending", width), body),
    ]
}

fn render_partial(
    details: &Value,
    expanded: bool,
    width: usize,
    body: Style,
    muted: Style,
    error: Style,
) -> Vec<Line> {
    let mut rows = vec![Line::styled(
        wrap_line("  partial commit · already written files remain", width),
        error,
    )];
    if let Some(changes) = details.get("changes").and_then(Value::as_array) {
        for change in changes {
            let path = change.get("path").and_then(Value::as_str).unwrap_or("?");
            let status = change.get("status").and_then(Value::as_str).unwrap_or("");
            let marker = match status {
                "committed" | "committed_unsynced" => "✓",
                "failed" => "✗",
                _ => "·",
            };
            rows.push(Line::styled(
                wrap_line(&format!("  {marker} {path} · {status}"), width),
                if status == "committed" { body } else { muted },
            ));
            if expanded
                && status == "committed"
                && let Some(diff) = change.get("diff").and_then(Value::as_str)
                && let Some(model) = DiffModel::parse_unified(diff)
            {
                rows.extend(model_preview_lines(&model, true, width, muted, body));
            }
        }
    }
    rows
}

fn render_failure(
    label: &str,
    details: &Value,
    width: usize,
    accent: Style,
    muted: Style,
) -> Vec<Line> {
    let mut rows = vec![Line::styled(
        wrap_line(&format!("  {label}"), width),
        accent,
    )];
    if let Some(path) = details.get("path").and_then(Value::as_str) {
        rows.push(Line::styled(wrap_line(&format!("  {path}"), width), muted));
    }
    if let Some(message) = details.get("message").and_then(Value::as_str) {
        for line in message.lines().take(6) {
            rows.push(Line::styled(wrap_line(&format!("  {line}"), width), muted));
        }
    }
    rows.push(Line::styled(
        wrap_line(
            "  Re-read affected files and submit a new Edit call.",
            width,
        ),
        muted,
    ));
    rows
}

fn select_change_indices(len: usize, expanded: bool) -> Vec<usize> {
    if expanded || len <= 3 {
        return (0..len).collect();
    }
    // Head file(s) + explicit omission + tail file.
    let mut indices = vec![0usize];
    if len > 1 {
        indices.push(1);
    }
    indices.push(len - 1);
    indices.sort_unstable();
    indices.dedup();
    indices
}

fn model_preview_lines(
    model: &DiffModel,
    expanded: bool,
    width: usize,
    muted: Style,
    body: Style,
) -> Vec<Line> {
    let mut rows = Vec::new();
    let limit = if expanded { usize::MAX } else { 4 };
    let mut shown = 0usize;
    for file in model.files() {
        for hunk in &file.hunks {
            for line in &hunk.lines {
                if shown >= limit {
                    rows.push(Line::styled(
                        wrap_line(
                            "       │  ... changed lines hidden · ctrl+o to expand",
                            width,
                        ),
                        muted,
                    ));
                    return rows;
                }
                let text = match line {
                    DiffLine::Context(text) => format!("     {text}"),
                    DiffLine::Added(text) => format!("    +{text}"),
                    DiffLine::Removed(text) => format!("    -{text}"),
                };
                rows.push(Line::styled(wrap_line(text.trim_end(), width), body));
                shown += 1;
            }
        }
    }
    rows
}

fn wrap_line(text: &str, width: usize) -> String {
    if width == 0 {
        return text.to_owned();
    }
    wrap_width(text, width)
        .into_iter()
        .next()
        .unwrap_or_default()
}
