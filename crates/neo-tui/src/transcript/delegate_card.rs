use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentLifecycleState, AgentRunMode, AgentSnapshot,
};

use crate::primitive::theme::TuiTheme;
use crate::primitive::{Component, Expandable, Finalization, Line, Style};

const MAX_SINGLE_AGENT_ACTIVITY_ROWS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegateCardComponent {
    snapshot: AgentSnapshot,
    expanded: bool,
}

impl DelegateCardComponent {
    #[must_use]
    pub fn new(snapshot: AgentSnapshot) -> Self {
        Self {
            snapshot,
            expanded: false,
        }
    }

    pub fn update(&mut self, snapshot: AgentSnapshot) {
        self.snapshot = snapshot;
    }

    #[must_use]
    pub fn id(&self) -> &str {
        self.snapshot.id.as_str()
    }

    #[must_use]
    pub fn render_with_theme(&self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        let accent = Style::default().fg(status_color(self.snapshot.state, theme));
        let muted = Style::default().fg(theme.text_muted);
        let primary = Style::default().fg(theme.text_primary);
        let mut lines = Vec::new();

        let header = Line::from_spans(vec![
            Span::styled(status_marker(self.snapshot.state), accent),
            Span::raw(" "),
            Span::styled(self.snapshot.display_name.as_str(), accent),
            Span::styled(
                format!(
                    " Agent {} ({}) · {} tools · {} · {} tok",
                    state_label(self.snapshot.state),
                    short_task_title(&self.snapshot.task),
                    self.snapshot.tool_count,
                    format_elapsed(self.snapshot.elapsed.as_secs()),
                    format_token_count(self.snapshot.token_count)
                ),
                primary,
            ),
        ])
        .truncate_to_width(width);
        lines.push(header);

        if self.snapshot.state == AgentLifecycleState::Running
            && self.snapshot.mode == AgentRunMode::Foreground
        {
            lines.push(Line::styled("  Press Ctrl+B to run in background", muted));
        }

        for activity in recent_activity(&self.snapshot.activity) {
            lines.push(render_activity(activity, width, theme));
        }

        if self.snapshot.activity.is_empty()
            && let Some(text) = &self.snapshot.latest_text
        {
            lines.push(Line::styled(format!("  \u{25cc} {text}"), muted).truncate_to_width(width));
        }

        if let Some(outcome) = &self.snapshot.outcome {
            let outcome_style = if outcome.is_error {
                Style::default().fg(theme.status_error)
            } else {
                Style::default().fg(theme.status_ok)
            };
            lines.push(
                Line::styled(format!("  \u{2514} {}", outcome.summary), outcome_style)
                    .truncate_to_width(width),
            );
        }

        lines
    }
}

impl Expandable for DelegateCardComponent {
    fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }
}

impl Component for DelegateCardComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.render_with_theme(width, &TuiTheme::default())
    }

    fn finalization(&self) -> Finalization {
        match self.snapshot.state {
            AgentLifecycleState::Completed
            | AgentLifecycleState::Failed
            | AgentLifecycleState::Cancelled => Finalization::Finalized,
            AgentLifecycleState::Queued | AgentLifecycleState::Running => Finalization::Live,
        }
    }
}

use crate::primitive::Span;

fn status_color(state: AgentLifecycleState, theme: &TuiTheme) -> crate::primitive::Color {
    match state {
        AgentLifecycleState::Completed => theme.status_ok,
        AgentLifecycleState::Failed => theme.status_error,
        AgentLifecycleState::Cancelled => theme.status_warn,
        AgentLifecycleState::Queued | AgentLifecycleState::Running => theme.brand,
    }
}

fn status_marker(state: AgentLifecycleState) -> &'static str {
    match state {
        AgentLifecycleState::Running => "\u{25cf}",
        AgentLifecycleState::Completed => "\u{2713}",
        AgentLifecycleState::Failed => "\u{2717}",
        AgentLifecycleState::Queued | AgentLifecycleState::Cancelled => "\u{25cc}",
    }
}

fn state_label(state: AgentLifecycleState) -> &'static str {
    match state {
        AgentLifecycleState::Queued => "Queued",
        AgentLifecycleState::Running => "Running",
        AgentLifecycleState::Completed => "Completed",
        AgentLifecycleState::Failed => "Failed",
        AgentLifecycleState::Cancelled => "Cancelled",
    }
}

fn format_elapsed(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {}s", seconds / 60, seconds % 60)
    }
}

fn format_token_count(tokens: usize) -> String {
    if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

fn recent_activity(activity: &[AgentActivityEntry]) -> &[AgentActivityEntry] {
    if activity.len() <= MAX_SINGLE_AGENT_ACTIVITY_ROWS {
        activity
    } else {
        &activity[activity.len() - MAX_SINGLE_AGENT_ACTIVITY_ROWS..]
    }
}

fn render_activity(activity: &AgentActivityEntry, width: usize, theme: &TuiTheme) -> Line {
    match &activity.kind {
        AgentActivityKind::Tool {
            name,
            summary,
            failed,
            ..
        } => {
            let marker = if *failed { "\u{2717}" } else { "\u{2022}" };
            let marker_style = if *failed {
                Style::default().fg(theme.status_error)
            } else {
                Style::default().fg(theme.status_ok)
            };
            let summary = summary
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(|value| format!(" ({value})"))
                .unwrap_or_default();
            Line::from_spans(vec![
                Span::raw("  "),
                Span::styled(marker, marker_style),
                Span::raw(" Used "),
                Span::styled(name.as_str(), Style::default().fg(theme.brand)),
                Span::styled(summary, Style::default().fg(theme.text_muted)),
            ])
            .truncate_to_width(width)
        }
        AgentActivityKind::Text { text, thinking } => {
            let marker = if *thinking { "\u{25cc}" } else { "\u{2514}" };
            Line::styled(
                format!("  {marker} {}", compact_display_line(text)),
                Style::default().fg(theme.text_muted),
            )
            .truncate_to_width(width)
        }
    }
}

fn short_task_title(task: &str) -> String {
    let first_line = task
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(task);
    let sentence = first_line
        .split(". ")
        .next()
        .unwrap_or(first_line)
        .trim()
        .trim_end_matches('.');
    compact_to_chars(sentence, 64)
}

fn compact_display_line(text: &str) -> String {
    compact_to_chars(&text.split_whitespace().collect::<Vec<_>>().join(" "), 110)
}

fn compact_to_chars(text: &str, max_chars: usize) -> String {
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
