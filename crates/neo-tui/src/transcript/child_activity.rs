use std::time::Duration;

use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentLifecycleState, AgentProfile, AgentRole,
    AgentRunMode, AgentSnapshot, AgentToolActivityPhase, AgentToolFileChange,
    AgentToolFileOperation, AgentToolFileStatus, AgentToolOutputPreview,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::primitive::theme::TuiTheme;
use crate::primitive::{Line, Span, Style, clip_plain_to_width, visible_width};

use super::tool_renderers::hard_wrap_line;

pub const MAX_CHILD_TOOL_ROWS: usize = 4;
const THINKING_PREVIEW_LINES: usize = 2;
const TOOL_OUTPUT_PREVIEW_LINES: usize = 2;
const FINAL_TEXT_CHARS: usize = 110;

pub struct ChildActivityView<'a> {
    pub tools: Vec<ChildToolRow<'a>>,
    pub thinking: Option<String>,
    pub body_text: Option<String>,
    pub final_text: Option<String>,
    pub final_is_error: bool,
}

pub struct ChildToolRow<'a> {
    pub name: &'a str,
    pub summary: Option<&'a str>,
    pub phase: AgentToolActivityPhase,
    pub output: Option<&'a AgentToolOutputPreview>,
    pub files: &'a [AgentToolFileChange],
}

#[must_use]
pub fn role_label(role: AgentRole) -> &'static str {
    AgentProfile::for_role(role).display_label
}

#[must_use]
pub fn role_badge_style(role: AgentRole, theme: &TuiTheme) -> Style {
    let color = match role {
        AgentRole::Coder => theme.status_warn,
        AgentRole::Explorer => theme.shell_mode,
        AgentRole::Planner => theme.brand,
        AgentRole::Reviewer => theme.status_ok,
    };
    Style::default().fg(color)
}

#[must_use]
pub fn format_elapsed(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {}s", seconds / 60, seconds % 60)
    }
}

#[must_use]
pub fn format_token_count(tokens: usize) -> String {
    if tokens >= 1_000 {
        // usize -> f64 is lossy for values above 2^53; token counts well under
        // that bound are safely represented and the precision loss is acceptable.
        #[allow(clippy::cast_precision_loss)]
        let scaled = tokens as f64 / 1_000.0;
        format!("{scaled:.1}k")
    } else {
        tokens.to_string()
    }
}

#[must_use]
pub fn format_cache_token_usage(snapshot: &AgentSnapshot) -> Option<String> {
    let read = snapshot.cache_read_token_count;
    let write = snapshot.cache_write_token_count;
    match (read, write) {
        (0, 0) => None,
        (read, 0) => Some(format!("cache {} read", format_token_count(read))),
        (0, write) => Some(format!("cache {} write", format_token_count(write))),
        (read, write) => Some(format!(
            "cache {} read / {} write",
            format_token_count(read),
            format_token_count(write)
        )),
    }
}

#[must_use]
pub fn can_detach(snapshot: &AgentSnapshot) -> bool {
    snapshot.state == AgentLifecycleState::Running
        && snapshot.mode == AgentRunMode::Foreground
        && !snapshot.detached_from_foreground
}

#[must_use]
pub fn display_elapsed(snapshot: &AgentSnapshot, now_ms: Option<u64>) -> Duration {
    if let (Some(started), None, Some(now)) =
        (snapshot.started_at_ms, snapshot.terminal_at_ms, now_ms)
    {
        return Duration::from_millis(now.saturating_sub(started));
    }
    snapshot.elapsed
}

#[must_use]
pub fn child_activity_view(
    snapshot: &AgentSnapshot,
    max_tool_rows: usize,
) -> ChildActivityView<'_> {
    let thinking = combined_text_activity(&snapshot.activity, true);
    let latest_body =
        latest_text_activity(&snapshot.activity, false).or_else(|| snapshot.latest_text.clone());
    let final_text = snapshot
        .outcome
        .as_ref()
        .map(|outcome| outcome.summary.clone())
        .or_else(|| {
            snapshot
                .state
                .is_terminal()
                .then(|| latest_body.clone())
                .flatten()
        });
    let body_text = if snapshot.state.is_terminal() {
        latest_body.filter(|text| {
            final_text
                .as_ref()
                .is_none_or(|final_text| !body_is_redundant_with_final(text, final_text))
        })
    } else {
        latest_body
    };
    let tools = visible_tool_rows(&snapshot.activity, max_tool_rows);
    ChildActivityView {
        tools,
        thinking,
        body_text,
        final_text,
        final_is_error: snapshot
            .outcome
            .as_ref()
            .is_some_and(|outcome| outcome.is_error)
            || matches!(
                snapshot.state,
                AgentLifecycleState::Failed | AgentLifecycleState::TimedOut
            ),
    }
}

const fn child_tool_verb(phase: AgentToolActivityPhase) -> &'static str {
    match phase {
        AgentToolActivityPhase::Queued { .. } => "Queued",
        AgentToolActivityPhase::Ongoing => "Using",
        AgentToolActivityPhase::Done => "Used",
        AgentToolActivityPhase::Failed => "Failed",
    }
}

fn child_tool_phase_style(phase: AgentToolActivityPhase, theme: &TuiTheme) -> Style {
    let color = match phase {
        AgentToolActivityPhase::Queued { .. } => theme.status_pending,
        AgentToolActivityPhase::Ongoing => theme.text_primary,
        AgentToolActivityPhase::Done => theme.status_ok,
        AgentToolActivityPhase::Failed => theme.status_error,
    };
    Style::default().fg(color)
}

pub(super) struct ChildToolStatus<'a> {
    pub name: &'a str,
    pub summary: Option<&'a str>,
    pub phase: AgentToolActivityPhase,
    pub inline_files: Option<&'a [AgentToolFileChange]>,
    pub verb_override: Option<&'a str>,
    pub now_ms: u64,
    pub max_width: usize,
    pub theme: &'a TuiTheme,
}

pub(super) fn child_tool_status_spans(status: ChildToolStatus<'_>) -> Vec<Span> {
    let ChildToolStatus {
        name,
        summary,
        phase,
        inline_files,
        verb_override,
        now_ms,
        max_width,
        theme,
    } = status;
    let verb = verb_override.unwrap_or_else(|| child_tool_verb(phase));
    let preserve_shell_summary = matches!(name, "Bash" | "Terminal")
        && summary.is_some_and(|summary| !summary.trim().is_empty());
    let verb_style = if verb_override.is_some() {
        Style::default().fg(theme.status_pending)
    } else {
        child_tool_phase_style(phase, theme)
    };
    let mut spans = vec![
        Span::styled(verb, verb_style),
        Span::styled(" ", Style::default().fg(theme.text_muted)),
        Span::styled(name, Style::default().fg(theme.brand).bold()),
    ];
    if matches!(name, "Edit" | "Write")
        && let Some(files) = inline_files.filter(|files| !files.is_empty())
    {
        spans.extend(inline_file_spans(files, phase, theme));
    } else if let Some(summary) =
        bounded_status_summary(verb, name, summary, phase, now_ms, max_width)
    {
        spans.push(Span::styled(
            format!(" ({summary})"),
            Style::default().fg(theme.text_muted),
        ));
    }
    if let AgentToolActivityPhase::Queued {
        position: Some(position),
        queued_at_ms,
    } = phase
    {
        let waiting_s = now_ms.saturating_sub(queued_at_ms) / 1_000;
        spans.push(Span::styled(
            format!(" · #{position} · waiting {waiting_s}s"),
            Style::default().fg(theme.text_muted),
        ));
    }
    if preserve_shell_summary {
        spans
    } else {
        compact_styled_chars(spans, max_width)
    }
}

fn compact_styled_chars(spans: Vec<Span>, max_chars: usize) -> Vec<Span> {
    if spans
        .iter()
        .map(|span| span.text().chars().count())
        .sum::<usize>()
        <= max_chars
    {
        return spans;
    }
    let mut remaining = max_chars.saturating_sub(3);
    let mut compact = Vec::new();
    let mut ellipsis_style = Style::default();
    for span in spans {
        ellipsis_style = span.style();
        if remaining == 0 {
            break;
        }
        let count = span.text().chars().count();
        if count <= remaining {
            compact.push(span);
            remaining -= count;
        } else {
            compact.push(Span::styled(
                span.text().chars().take(remaining).collect::<String>(),
                span.style(),
            ));
            break;
        }
    }
    compact.push(Span::styled("...", ellipsis_style));
    compact
}

fn bounded_status_summary(
    verb: &str,
    name: &str,
    summary: Option<&str>,
    phase: AgentToolActivityPhase,
    now_ms: u64,
    max_width: usize,
) -> Option<String> {
    let summary = summary
        .filter(|value| !value.trim().is_empty())
        .map(one_line)?;
    if !matches!(name, "Bash" | "Terminal") {
        return Some(summary);
    }
    let queue_suffix = if let AgentToolActivityPhase::Queued {
        position: Some(position),
        queued_at_ms,
    } = phase
    {
        let waiting_s = now_ms.saturating_sub(queued_at_ms) / 1_000;
        format!(" · #{position} · waiting {waiting_s}s")
    } else {
        String::new()
    };
    let fixed_width = visible_width(verb)
        + 1
        + visible_width(name)
        + visible_width(" ()")
        + visible_width(&queue_suffix);
    let summary_width = max_width.saturating_sub(fixed_width);
    (summary_width > 0).then(|| compact_middle_width(&summary, summary_width))
}

fn inline_file_spans(
    files: &[AgentToolFileChange],
    phase: AgentToolActivityPhase,
    theme: &TuiTheme,
) -> Vec<Span> {
    let muted = Style::default().fg(theme.text_muted);
    let mut spans = Vec::new();
    if files.len() > 1 {
        spans.push(Span::styled(format!(" · {} files", files.len()), muted));
    }
    let file = representative_file(files);
    spans.push(Span::styled(" · ", muted));
    spans.extend(inline_file_marker_spans(file, theme));
    spans.push(Span::styled(" ", muted));
    spans.push(Span::styled(
        file.path.clone(),
        Style::default().fg(theme.text_primary),
    ));

    if files.len() == 1 {
        append_single_file_stats(&mut spans, file, theme);
    } else if phase == AgentToolActivityPhase::Done
        && let Some((added, removed)) = aggregate_diff_stats(files)
    {
        spans.push(Span::styled(" · total", muted));
        spans.push(Span::styled(
            format!(" +{added}"),
            Style::default().fg(theme.diff_added),
        ));
        spans.push(Span::styled(
            format!(" -{removed}"),
            Style::default().fg(theme.diff_removed),
        ));
    }
    append_file_message(&mut spans, file, theme);
    spans
}

fn representative_file(files: &[AgentToolFileChange]) -> &AgentToolFileChange {
    for status in [
        AgentToolFileStatus::Failed,
        AgentToolFileStatus::CommittedUnsynced,
        AgentToolFileStatus::Pending,
    ] {
        if let Some(file) = files.iter().find(|file| file.status == status) {
            return file;
        }
    }
    &files[0]
}

fn aggregate_diff_stats(files: &[AgentToolFileChange]) -> Option<(usize, usize)> {
    if files
        .iter()
        .any(|file| file.status == AgentToolFileStatus::Pending)
    {
        return None;
    }
    files
        .iter()
        .try_fold((0usize, 0usize), |(added, removed), file| {
            Some((
                added.checked_add(file.added?)?,
                removed.checked_add(file.removed?)?,
            ))
        })
}

fn append_single_file_stats(spans: &mut Vec<Span>, file: &AgentToolFileChange, theme: &TuiTheme) {
    if file.status == AgentToolFileStatus::Pending {
        return;
    }
    let muted = Style::default().fg(theme.text_muted);
    match file.operation {
        Some(AgentToolFileOperation::Edited) => {
            if let (Some(added), Some(removed)) = (file.added, file.removed) {
                spans.push(Span::styled(" ·", muted));
                spans.push(Span::styled(
                    format!(" +{added}"),
                    Style::default().fg(theme.diff_added),
                ));
                spans.push(Span::styled(
                    format!(" -{removed}"),
                    Style::default().fg(theme.diff_removed),
                ));
            }
        }
        Some(AgentToolFileOperation::Created | AgentToolFileOperation::Overwritten) => {
            if let Some(lines) = file.line_count {
                spans.push(Span::styled(format!(" · {lines} lines"), muted));
            }
        }
        None => {}
    }
}

fn inline_file_marker_spans(file: &AgentToolFileChange, theme: &TuiTheme) -> Vec<Span> {
    let operation = file_operation_marker(file.operation);
    let (text, style) = match file.status {
        AgentToolFileStatus::Pending => ("…".to_owned(), theme.status_pending),
        AgentToolFileStatus::Committed => (
            operation.unwrap_or("•").to_owned(),
            file_operation_color(file.operation, theme),
        ),
        AgentToolFileStatus::CommittedUnsynced => (
            operation.map_or_else(|| "!".to_owned(), |operation| format!("! {operation}")),
            theme.status_warn,
        ),
        AgentToolFileStatus::Failed => (operation.unwrap_or("✗").to_owned(), theme.status_error),
        AgentToolFileStatus::NotAttempted => (
            operation.map_or_else(|| "–".to_owned(), |operation| format!("– {operation}")),
            theme.text_muted,
        ),
    };
    vec![Span::styled(text, Style::default().fg(style))]
}

const fn file_operation_marker(operation: Option<AgentToolFileOperation>) -> Option<&'static str> {
    match operation {
        Some(AgentToolFileOperation::Edited | AgentToolFileOperation::Overwritten) => Some("M"),
        Some(AgentToolFileOperation::Created) => Some("C"),
        None => None,
    }
}

const fn file_operation_color(
    operation: Option<AgentToolFileOperation>,
    theme: &TuiTheme,
) -> crate::primitive::Color {
    match operation {
        Some(AgentToolFileOperation::Created) => theme.diff_added,
        Some(AgentToolFileOperation::Edited | AgentToolFileOperation::Overwritten) | None => {
            theme.diff_hunk
        }
    }
}

fn append_file_message(spans: &mut Vec<Span>, file: &AgentToolFileChange, theme: &TuiTheme) {
    let Some(message) = file
        .message
        .as_deref()
        .filter(|message| !message.trim().is_empty())
    else {
        return;
    };
    spans.push(Span::styled(" · ", Style::default().fg(theme.text_muted)));
    let color = if file.status == AgentToolFileStatus::Failed {
        theme.status_error
    } else {
        theme.status_warn
    };
    spans.push(Span::styled(one_line(message), Style::default().fg(color)));
}

fn compact_middle_width(text: &str, max_width: usize) -> String {
    const SEPARATOR: &str = " … ";
    let width = visible_width(text);
    if width <= max_width {
        return text.to_owned();
    }
    let separator_width = visible_width(SEPARATOR);
    if max_width <= separator_width {
        return clip_plain_to_width(text, max_width);
    }
    let content_width = max_width - separator_width;
    let head_width = content_width / 2;
    let tail_width = content_width - head_width;
    format!(
        "{}{SEPARATOR}{}",
        clip_plain_to_width(text, head_width),
        clip_plain_tail_to_width(text, tail_width)
    )
}

fn clip_plain_tail_to_width(text: &str, max_width: usize) -> String {
    let mut kept = Vec::new();
    let mut width = 0;
    for grapheme in text.graphemes(true).rev() {
        let grapheme_width = visible_width(grapheme);
        if width + grapheme_width > max_width {
            break;
        }
        kept.push(grapheme);
        width += grapheme_width;
    }
    kept.into_iter().rev().collect()
}

pub fn render_child_tool_row(
    row: &ChildToolRow<'_>,
    width: usize,
    indent: &str,
    theme: &TuiTheme,
    now_ms: Option<u64>,
) -> Vec<Line> {
    let marker = match row.phase {
        AgentToolActivityPhase::Failed => "✗",
        AgentToolActivityPhase::Done
        | AgentToolActivityPhase::Ongoing
        | AgentToolActivityPhase::Queued { .. } => "•",
    };
    let marker_style = child_tool_phase_style(row.phase, theme);
    let status_width = width.saturating_sub(visible_width(indent) + visible_width(marker) + 1);
    let status = child_tool_status_spans(ChildToolStatus {
        name: row.name,
        summary: row.summary,
        phase: row.phase,
        inline_files: None,
        verb_override: None,
        now_ms: now_ms.unwrap_or(0),
        max_width: status_width,
        theme,
    });
    let muted = Style::default().fg(theme.text_muted);
    let mut header = vec![
        Span::styled(indent.to_owned(), muted),
        Span::styled(marker, marker_style),
        Span::styled(" ", muted),
    ];
    header.extend(status);
    let mut lines = vec![Line::from_spans(header).truncate_to_width(width)];
    lines.extend(render_child_file_rows(row.files, width, indent, theme));
    if let Some(output) = row.output {
        lines.extend(render_output_preview(output, width, indent, theme));
    }
    lines
}

fn render_child_file_rows(
    files: &[AgentToolFileChange],
    width: usize,
    indent: &str,
    theme: &TuiTheme,
) -> Vec<Line> {
    let first_prefix = format!("{indent}    ");
    let continuation_prefix = format!("{first_prefix}  ");
    let content_width = width
        .saturating_sub(visible_width(&continuation_prefix))
        .max(1);
    let mut lines = Vec::new();
    for file in files {
        let file_line = Line::from_spans(child_file_spans(file, theme));
        for (index, row) in hard_wrap_line(&file_line, content_width)
            .into_iter()
            .enumerate()
        {
            let prefix = if index == 0 {
                &first_prefix
            } else {
                &continuation_prefix
            };
            lines.push(
                Line::from_spans(
                    vec![Span::styled(
                        prefix.clone(),
                        Style::default().fg(theme.text_muted),
                    )]
                    .into_iter()
                    .chain(row.into_spans())
                    .collect(),
                )
                .truncate_to_width(width),
            );
        }
    }
    lines
}

fn child_file_spans(file: &AgentToolFileChange, theme: &TuiTheme) -> Vec<Span> {
    let operation = file_operation_marker(file.operation);
    let marker = match file.status {
        AgentToolFileStatus::Pending => "…".to_owned(),
        AgentToolFileStatus::Committed => operation.unwrap_or("").to_owned(),
        AgentToolFileStatus::CommittedUnsynced => {
            operation.map_or_else(|| "!".to_owned(), |operation| format!("! {operation}"))
        }
        AgentToolFileStatus::Failed => {
            operation.map_or_else(|| "✗".to_owned(), |operation| format!("✗ {operation}"))
        }
        AgentToolFileStatus::NotAttempted => {
            operation.map_or_else(|| "–".to_owned(), |operation| format!("– {operation}"))
        }
    };
    let marker_color = match file.status {
        AgentToolFileStatus::Pending => theme.status_pending,
        AgentToolFileStatus::Committed => file_operation_color(file.operation, theme),
        AgentToolFileStatus::CommittedUnsynced => theme.status_warn,
        AgentToolFileStatus::Failed => theme.status_error,
        AgentToolFileStatus::NotAttempted => theme.text_muted,
    };
    let muted = Style::default().fg(theme.text_muted);
    let mut spans = vec![
        Span::styled(marker, Style::default().fg(marker_color)),
        Span::styled(" ", muted),
        Span::styled(file.path.clone(), Style::default().fg(theme.text_primary)),
    ];
    if file.status != AgentToolFileStatus::Pending {
        match file.operation {
            Some(AgentToolFileOperation::Edited) => {
                if let (Some(added), Some(removed)) = (file.added, file.removed) {
                    spans.push(Span::styled("  ", muted));
                    spans.push(Span::styled(
                        format!("+{added}"),
                        Style::default().fg(theme.diff_added),
                    ));
                    spans.push(Span::styled(" ", muted));
                    spans.push(Span::styled(
                        format!("-{removed}"),
                        Style::default().fg(theme.diff_removed),
                    ));
                }
            }
            Some(AgentToolFileOperation::Created | AgentToolFileOperation::Overwritten) => {
                if let Some(lines) = file.line_count {
                    spans.push(Span::styled(format!("  {lines} lines"), muted));
                }
            }
            None => {}
        }
    }
    append_file_message(&mut spans, file, theme);
    spans
}

pub fn render_child_thinking(
    text: &str,
    width: usize,
    indent: &str,
    theme: &TuiTheme,
) -> Vec<Line> {
    let muted = Style::default().fg(theme.text_muted);
    let mut lines = vec![
        Line::from_spans(vec![
            Span::styled(indent.to_owned(), muted),
            Span::styled("◌ thinking".to_owned(), muted),
        ])
        .truncate_to_width(width),
    ];
    lines.extend(
        tail_non_empty_lines(text, THINKING_PREVIEW_LINES)
            .into_iter()
            .map(|line| {
                Line::from_spans(vec![
                    Span::styled(indent.to_owned(), muted),
                    Span::styled(
                        format!("  {}", compact_chars(&one_line(&line), FINAL_TEXT_CHARS)),
                        muted,
                    ),
                ])
                .truncate_to_width(width)
            }),
    );
    lines
}

pub fn render_child_body(text: &str, width: usize, indent: &str, theme: &TuiTheme) -> Option<Line> {
    let compact = compact_chars(&one_line(text), FINAL_TEXT_CHARS);
    let muted = Style::default().fg(theme.text_muted);
    let primary = Style::default().fg(theme.text_primary);
    (!compact.is_empty()).then(|| {
        Line::from_spans(vec![
            Span::styled(indent.to_owned(), muted),
            Span::styled("│ ".to_owned(), muted),
            Span::styled(compact, primary),
        ])
        .truncate_to_width(width)
    })
}

pub fn render_child_final(
    text: &str,
    is_error: bool,
    width: usize,
    indent: &str,
    theme: &TuiTheme,
) -> Line {
    let muted = Style::default().fg(theme.text_muted);
    let color = if is_error {
        theme.status_error
    } else {
        theme.text_primary
    };
    Line::from_spans(vec![
        Span::styled(indent.to_owned(), muted),
        Span::styled("└ ".to_owned(), muted),
        Span::styled(
            compact_chars(&one_line(text), FINAL_TEXT_CHARS),
            Style::default().fg(color),
        ),
    ])
    .truncate_to_width(width)
}

#[must_use]
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Returns true when the streaming body line would duplicate the start of the
/// final summary, so only the final (`└`) line needs to be shown.
fn body_is_redundant_with_final(body: &str, final_text: &str) -> bool {
    let body_norm = comparable_child_text(body);
    let final_norm = comparable_child_text(final_text);
    body_norm == final_norm || final_norm.starts_with(&body_norm)
}

fn comparable_child_text(text: &str) -> String {
    // Normalize markdown emphasis/inline-code markers into whitespace so that
    // formatting differences such as `**word` vs `** word` do not prevent
    // detecting duplicate body/final content.
    let cleaned = one_line(text)
        .replace("**", " ")
        .replace('*', " ")
        .replace("__", " ")
        .replace('_', " ")
        .replace("~~", " ")
        .replace('`', " ");
    let mut normalized = String::new();
    let mut previous: Option<char> = None;
    for ch in cleaned.chars() {
        if ch == '#' {
            normalized.push(' ');
            normalized.push('#');
            normalized.push(' ');
        } else {
            if let Some(previous) = previous
                && ((previous.is_ascii_alphabetic() && ch.is_ascii_digit())
                    || (previous.is_ascii_digit() && ch.is_ascii_alphabetic()))
            {
                normalized.push(' ');
            }
            normalized.push(ch);
        }
        previous = Some(ch);
    }
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[must_use]
pub fn compact_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    format!(
        "{}...",
        text.chars()
            .take(max_chars.saturating_sub(3))
            .collect::<String>()
    )
}

fn tool_row(entry: &AgentActivityEntry) -> Option<ChildToolRow<'_>> {
    match &entry.kind {
        AgentActivityKind::Tool {
            name,
            summary,
            phase,
            output,
            files,
            ..
        } => Some(ChildToolRow {
            name,
            summary: summary.as_deref(),
            phase: *phase,
            output: output.as_ref(),
            files,
        }),
        AgentActivityKind::Text { .. } => None,
    }
}

fn visible_tool_rows(
    activity: &[AgentActivityEntry],
    max_tool_rows: usize,
) -> Vec<ChildToolRow<'_>> {
    if max_tool_rows == 0 {
        return Vec::new();
    }
    let tool_rows = activity.iter().filter_map(tool_row).collect::<Vec<_>>();
    if tool_rows.len() <= max_tool_rows {
        return tool_rows;
    }

    let mut keep = vec![false; tool_rows.len()];
    for (index, row) in tool_rows.iter().enumerate().rev() {
        if matches!(
            row.phase,
            AgentToolActivityPhase::Ongoing | AgentToolActivityPhase::Queued { .. }
        ) {
            keep[index] = true;
        }
    }

    let kept = keep.iter().filter(|value| **value).count();
    if kept > max_tool_rows {
        let mut remaining = max_tool_rows;
        for index in (0..keep.len()).rev() {
            if keep[index] {
                if remaining == 0 {
                    keep[index] = false;
                } else {
                    remaining -= 1;
                }
            }
        }
    } else {
        let mut remaining = max_tool_rows - kept;
        for index in (0..tool_rows.len()).rev() {
            if keep[index] {
                continue;
            }
            if remaining == 0 {
                break;
            }
            keep[index] = true;
            remaining -= 1;
        }
    }

    tool_rows
        .into_iter()
        .enumerate()
        .filter_map(|(index, row)| keep[index].then_some(row))
        .collect()
}

fn latest_text_activity(activity: &[AgentActivityEntry], thinking: bool) -> Option<String> {
    activity
        .iter()
        .rev()
        .filter_map(|entry| match &entry.kind {
            AgentActivityKind::Text {
                text,
                thinking: entry_thinking,
            } if *entry_thinking == thinking => Some(text.trim()),
            _ => None,
        })
        .find(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

fn combined_text_activity(activity: &[AgentActivityEntry], thinking: bool) -> Option<String> {
    let text = activity
        .iter()
        .filter_map(|entry| match &entry.kind {
            AgentActivityKind::Text {
                text,
                thinking: entry_thinking,
            } if *entry_thinking == thinking => Some(text.trim()),
            _ => None,
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

fn render_output_preview(
    output: &AgentToolOutputPreview,
    width: usize,
    indent: &str,
    theme: &TuiTheme,
) -> Vec<Line> {
    let preview_indent = format!("{indent}    ");
    let muted = Style::default().fg(theme.text_muted);
    tail_non_empty_lines(&output.text, TOOL_OUTPUT_PREVIEW_LINES)
        .into_iter()
        .map(|line| {
            let color = if output.is_error {
                theme.status_error
            } else {
                theme.text_muted
            };
            Line::from_spans(vec![
                Span::styled(preview_indent.clone(), muted),
                Span::styled(line, Style::default().fg(color)),
            ])
            .truncate_to_width(width)
        })
        .collect()
}

fn tail_non_empty_lines(text: &str, limit: usize) -> Vec<String> {
    let mut lines = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let start = lines.len().saturating_sub(limit);
    lines.drain(0..start);
    lines
}
