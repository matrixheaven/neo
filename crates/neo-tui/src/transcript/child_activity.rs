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
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
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
    let start = snapshot.activity.len().saturating_sub(max_tool_rows);
    let activity_window = &snapshot.activity[start..];
    let final_text = snapshot
        .outcome
        .as_ref()
        .map(|outcome| outcome.summary.clone())
        .or_else(|| latest_text_activity(activity_window, false))
        .or_else(|| snapshot.latest_text.clone());
    let thinking = latest_text_activity(activity_window, true);
    let tool_rows = activity_window
        .iter()
        .filter_map(tool_row)
        .collect::<Vec<_>>();
    let start = tool_rows.len().saturating_sub(max_tool_rows);
    let tools = tool_rows.into_iter().skip(start).collect::<Vec<_>>();
    ChildActivityView {
        tools,
        thinking,
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
        .map(|value| format!(" ({})", one_line(value)))
        .unwrap_or_default();
    let mut lines = vec![
        Line::from_spans(vec![
            Span::raw(indent.to_owned()),
            Span::styled(marker, marker_style),
            Span::raw(format!(" {verb} ")),
            Span::styled(row.name.to_owned(), Style::default().fg(theme.brand)),
            Span::styled(suffix, Style::default().fg(theme.text_muted)),
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
    let preview = tail_non_empty_lines(text, THINKING_PREVIEW_LINES).join(" ");
    if preview.is_empty() {
        return Vec::new();
    }
    vec![
        Line::styled(
            format!("{indent}◌ {}", compact_chars(&preview, FINAL_TEXT_CHARS)),
            Style::default().fg(theme.text_muted),
        )
        .truncate_to_width(width),
    ]
}

pub fn render_child_final(
    text: &str,
    is_error: bool,
    width: usize,
    indent: &str,
    theme: &TuiTheme,
) -> Line {
    let color = if is_error {
        theme.status_error
    } else {
        theme.text_primary
    };
    Line::styled(
        format!(
            "{indent}└ {}",
            compact_chars(&one_line(text), FINAL_TEXT_CHARS)
        ),
        Style::default().fg(color),
    )
    .truncate_to_width(width)
}

#[must_use]
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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

fn latest_text_activity(activity: &[AgentActivityEntry], thinking: bool) -> Option<String> {
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
        .join(" ");
    (!text.is_empty()).then_some(text)
}

fn render_output_preview(
    output: &AgentToolOutputPreview,
    width: usize,
    indent: &str,
    theme: &TuiTheme,
) -> Vec<Line> {
    let preview_indent = format!("{indent}    ");
    tail_non_empty_lines(&output.text, TOOL_OUTPUT_PREVIEW_LINES)
        .into_iter()
        .map(|line| {
            let color = if output.is_error {
                theme.status_error
            } else {
                theme.text_muted
            };
            Line::styled(
                format!("{preview_indent}{line}"),
                Style::default().fg(color),
            )
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
