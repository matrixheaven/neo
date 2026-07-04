use std::time::Duration;

use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentLifecycleState, AgentProfile, AgentRole,
    AgentRunMode, AgentSnapshot, AgentToolActivityPhase, AgentToolOutputPreview,
};

use crate::primitive::theme::TuiTheme;
use crate::primitive::{Line, Span, Style};

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
                .is_none_or(|final_text| !same_child_final_body(text, final_text))
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

pub fn render_child_tool_row(
    row: &ChildToolRow<'_>,
    width: usize,
    indent: &str,
    theme: &TuiTheme,
) -> Vec<Line> {
    let marker = match row.phase {
        AgentToolActivityPhase::Failed => "✗",
        AgentToolActivityPhase::Done | AgentToolActivityPhase::Ongoing => "•",
    };
    let marker_style = match row.phase {
        AgentToolActivityPhase::Failed => Style::default().fg(theme.status_error),
        AgentToolActivityPhase::Done => Style::default().fg(theme.status_ok),
        AgentToolActivityPhase::Ongoing => Style::default().fg(theme.text_primary),
    };
    let verb = match row.phase {
        AgentToolActivityPhase::Ongoing => "Using",
        AgentToolActivityPhase::Done | AgentToolActivityPhase::Failed => "Used",
    };
    let suffix = row
        .summary
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("  {}", one_line(value)))
        .unwrap_or_default();
    let muted = Style::default().fg(theme.text_muted);
    let mut lines = vec![
        Line::from_spans(vec![
            Span::styled(indent.to_owned(), muted),
            Span::styled(marker, marker_style),
            Span::raw(format!(" {verb} ")),
            Span::styled(row.name.to_owned(), Style::default().fg(theme.brand)),
            Span::styled(suffix, muted),
        ])
        .truncate_to_width(width),
    ];
    if let Some(output) = row.output {
        lines.extend(render_output_preview(output, width, indent, theme));
    }
    lines
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

fn same_child_final_body(body: &str, final_text: &str) -> bool {
    comparable_child_text(body) == comparable_child_text(final_text)
}

fn comparable_child_text(text: &str) -> String {
    let mut normalized = String::new();
    let mut previous: Option<char> = None;
    for ch in one_line(text).chars() {
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
            ..
        } => Some(ChildToolRow {
            name,
            summary: summary.as_deref(),
            phase: *phase,
            output: output.as_ref(),
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
        if row.phase == AgentToolActivityPhase::Ongoing {
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
