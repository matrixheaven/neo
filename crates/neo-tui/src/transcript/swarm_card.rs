use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentLifecycleState, AgentSnapshot, SwarmProgressInput,
    SwarmSnapshot, estimate_swarm_progress,
};

use crate::primitive::theme::TuiTheme;
use crate::primitive::{Color, Component, Expandable, Finalization, Line, Span, Style};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwarmCardComponent {
    snapshot: SwarmSnapshot,
    expanded: bool,
}

impl SwarmCardComponent {
    #[must_use]
    pub fn new(snapshot: SwarmSnapshot) -> Self {
        Self {
            snapshot,
            expanded: false,
        }
    }

    pub fn update(&mut self, snapshot: SwarmSnapshot) {
        self.snapshot = snapshot;
    }

    #[must_use]
    pub fn snapshot(&self) -> &SwarmSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub fn swarm_id(&self) -> &str {
        &self.snapshot.swarm_id
    }

    #[must_use]
    pub fn render_with_theme(&self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        let brand = Style::default().fg(theme.brand);
        let muted = Style::default().fg(theme.text_muted);
        let primary = Style::default().fg(theme.text_primary);
        let mut lines = Vec::new();

        let progress = self.estimate_progress();
        lines.push(
            Line::from_spans(vec![
                Span::styled("\u{2500} Agent Swarm \u{2500} ", brand),
                Span::styled(self.snapshot.description.as_str(), primary),
                Span::styled(
                    format!(
                        " \u{2500} {} \u{2500} {:.0}%",
                        swarm_status_label(&self.snapshot),
                        progress * 100.0,
                    ),
                    swarm_status_style(&self.snapshot, theme),
                ),
            ])
            .truncate_to_width(width),
        );
        lines.push(Line::styled(
            "\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}",
            brand,
        ));

        for child in &self.snapshot.children {
            let state_style = Style::default().fg(agent_status_color(child.agent.state, theme));
            lines.push(
                Line::from_spans(vec![
                    Span::styled(format!("{:03} ", child.item_index + 1), muted),
                    Span::raw("["),
                    progress_bar_line(child.agent.state, theme),
                    Span::raw("] "),
                    Span::styled(marker(child.agent.state), state_style),
                    Span::raw(" "),
                    Span::styled(child.agent.display_name.as_str(), state_style),
                    Span::styled(
                        format!(
                            " {} · {} tools · {} · {} tok · {}",
                            state_label(child.agent.state),
                            child.agent.tool_count,
                            format_elapsed(child.agent.elapsed.as_secs()),
                            format_token_count(child.agent.token_count),
                            child_activity_summary(&child.agent, &child.item),
                        ),
                        primary,
                    ),
                ])
                .truncate_to_width(width),
            );
        }

        lines.push(render_scheduling_summary(&self.snapshot, theme).truncate_to_width(width));

        let all_queued = self
            .snapshot
            .children
            .iter()
            .all(|child| matches!(child.agent.state, AgentLifecycleState::Queued));

        let any_suspended = self
            .snapshot
            .children
            .iter()
            .any(|child| child.agent.latest_text.as_deref() == Some("suspended"));

        lines.push(Line::raw(""));
        if all_queued {
            lines.push(Line::styled(
                "\u{25cf} Orchestrating...",
                Style::default().fg(theme.status_warn),
            ));
        } else if any_suspended {
            lines.push(
                Line::from_spans(vec![
                    Span::styled("\u{25cf} Suspended (rate-limit) ", Style::default().fg(theme.status_warn)),
                    Span::styled(
                        "\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}",
                        Style::default().fg(theme.status_warn),
                    ),
                ])
                .truncate_to_width(width),
            );
        } else {
            lines.push(
                Line::from_spans(vec![
                    Span::styled(
                        format!("\u{25cf} Working... {:.0}% ", progress * 100.0),
                        Style::default().fg(theme.status_warn),
                    ),
                    progress_meter(progress, theme),
                ])
                .truncate_to_width(width),
            );
        }

        if self.expanded {
            for child in &self.snapshot.children {
                let state_style = Style::default().fg(agent_status_color(child.agent.state, theme));
                lines.push(
                    Line::from_spans(vec![
                        Span::raw("  "),
                        Span::styled(marker(child.agent.state), state_style),
                        Span::raw(" "),
                        Span::styled(child.agent.display_name.as_str(), state_style),
                        Span::styled(
                            format!(
                                " {} · {} tools · {} · {} tok",
                                state_label(child.agent.state),
                                child.agent.tool_count,
                                format_elapsed(child.agent.elapsed.as_secs()),
                                format_token_count(child.agent.token_count),
                            ),
                            primary,
                        ),
                    ])
                    .truncate_to_width(width),
                );

                for activity in child.agent.activity.iter().rev().take(2).rev() {
                    lines.push(render_child_activity(activity, width, theme));
                }
                if should_render_latest_text(&child.agent)
                    && let Some(text) = &child.agent.latest_text
                {
                    lines.push(
                        Line::styled(format!("    \u{25cc} {text}"), muted)
                            .truncate_to_width(width),
                    );
                }
                if let Some(outcome) = &child.agent.outcome {
                    let outcome_style = if outcome.is_error {
                        Style::default().fg(theme.status_error)
                    } else {
                        Style::default().fg(theme.status_ok)
                    };
                    lines.push(
                        Line::styled(format!("    \u{2514} {}", outcome.summary), outcome_style)
                            .truncate_to_width(width),
                    );
                }
            }
        }

        lines
    }

    fn estimate_progress(&self) -> f32 {
        let total = self.snapshot.children.len();
        let completed = self
            .snapshot
            .children
            .iter()
            .filter(|c| matches!(c.agent.state, AgentLifecycleState::Completed))
            .count();
        let failed = self
            .snapshot
            .children
            .iter()
            .filter(|c| matches!(c.agent.state, AgentLifecycleState::Failed))
            .count();
        let running = self
            .snapshot
            .children
            .iter()
            .filter(|c| matches!(c.agent.state, AgentLifecycleState::Running))
            .count();
        let queued = self
            .snapshot
            .children
            .iter()
            .filter(|c| matches!(c.agent.state, AgentLifecycleState::Queued))
            .count();
        let suspended = self
            .snapshot
            .children
            .iter()
            .filter(|c| c.agent.latest_text.as_deref() == Some("suspended"))
            .count();
        let completed_durations: Vec<_> = self
            .snapshot
            .children
            .iter()
            .filter(|c| {
                matches!(
                    c.agent.state,
                    AgentLifecycleState::Completed | AgentLifecycleState::Failed
                )
            })
            .map(|c| c.agent.elapsed)
            .filter(|duration| !duration.is_zero())
            .collect();
        let median_completed_duration = median_duration(completed_durations);
        let longest_running_duration = self
            .snapshot
            .children
            .iter()
            .filter(|c| matches!(c.agent.state, AgentLifecycleState::Running))
            .map(|c| c.agent.elapsed)
            .max()
            .unwrap_or_default();
        estimate_swarm_progress(SwarmProgressInput {
            total,
            completed,
            failed,
            running,
            queued,
            suspended,
            median_completed_duration,
            longest_running_duration,
        })
    }
}

impl Expandable for SwarmCardComponent {
    fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }
}

impl Component for SwarmCardComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.render_with_theme(width, &TuiTheme::default())
    }

    fn finalization(&self) -> Finalization {
        if self.snapshot.children.iter().all(|child| {
            matches!(
                child.agent.state,
                AgentLifecycleState::Completed
                    | AgentLifecycleState::Failed
                    | AgentLifecycleState::Cancelled
            )
        }) {
            Finalization::Finalized
        } else {
            Finalization::Live
        }
    }
}

fn progress_bar_line(state: AgentLifecycleState, theme: &TuiTheme) -> Span {
    Span::styled(
        progress_bar_text(state),
        Style::default().fg(agent_status_color(state, theme)),
    )
}

fn progress_bar_text(state: AgentLifecycleState) -> &'static str {
    match state {
        AgentLifecycleState::Queued => {
            "\u{00b7}\u{00b7}\u{00b7}\u{00b7}\u{00b7}\u{00b7}\u{00b7}\u{00b7}\u{00b7}\u{00b7}"
        }
        AgentLifecycleState::Running => {
            "\u{25a0}\u{25a0}\u{25a0}\u{00b7}\u{00b7}\u{00b7}\u{00b7}\u{00b7}\u{00b7}\u{00b7}"
        }
        AgentLifecycleState::Completed => {
            "\u{25a0}\u{25a0}\u{25a0}\u{25a0}\u{25a0}\u{25a0}\u{25a0}\u{25a0}\u{25a0}\u{25a0}"
        }
        AgentLifecycleState::Failed | AgentLifecycleState::Cancelled => {
            "\u{2715}\u{2715}\u{2715}\u{00b7}\u{00b7}\u{00b7}\u{00b7}\u{00b7}\u{00b7}\u{00b7}"
        }
    }
}

fn child_activity_summary(agent: &AgentSnapshot, fallback_item: &str) -> String {
    if let Some(outcome) = &agent.outcome
        && !outcome.summary.trim().is_empty()
    {
        return compact_to_chars(&one_line(&outcome.summary), 96);
    }
    if let Some(activity) = agent.activity.last()
        && let Some(text) = activity_summary(activity)
    {
        return compact_to_chars(&text, 96);
    }
    if let Some(text) = &agent.latest_text
        && !text.trim().is_empty()
    {
        return compact_to_chars(&one_line(text), 96);
    }
    compact_to_chars(&one_line(fallback_item), 96)
}

fn activity_summary(activity: &AgentActivityEntry) -> Option<String> {
    match &activity.kind {
        AgentActivityKind::Tool {
            name,
            summary,
            failed,
            ..
        } => {
            let verb = if *failed { "Failed" } else { "Used" };
            Some(match summary {
                Some(summary) if !summary.trim().is_empty() => {
                    format!("{verb} {name} ({})", one_line(summary))
                }
                _ => format!("{verb} {name}"),
            })
        }
        AgentActivityKind::Text { text, .. } => (!text.trim().is_empty()).then(|| one_line(text)),
    }
}

fn render_child_activity(activity: &AgentActivityEntry, width: usize, theme: &TuiTheme) -> Line {
    let muted = Style::default().fg(theme.text_muted);
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
            let suffix = summary
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(|value| format!(" ({})", one_line(value)))
                .unwrap_or_default();
            Line::from_spans(vec![
                Span::raw("    "),
                Span::styled(marker, marker_style),
                Span::raw(" Used "),
                Span::styled(name.as_str(), Style::default().fg(theme.brand)),
                Span::styled(suffix, muted),
            ])
            .truncate_to_width(width)
        }
        AgentActivityKind::Text { text, thinking } => {
            let marker = if *thinking { "\u{25cc}" } else { "\u{2514}" };
            Line::styled(format!("    {marker} {}", one_line(text)), muted).truncate_to_width(width)
        }
    }
}

fn should_render_latest_text(agent: &AgentSnapshot) -> bool {
    let Some(latest_text) = &agent.latest_text else {
        return false;
    };
    let latest = one_line(latest_text);
    !agent.activity.iter().any(|activity| match &activity.kind {
        AgentActivityKind::Text { text, .. } => one_line(text) == latest,
        _ => false,
    })
}

fn progress_meter(progress: f32, theme: &TuiTheme) -> Span {
    let width = 30usize;
    let filled = ((progress.clamp(0.0, 1.0) * width as f32).round() as usize).min(width);
    let text = format!(
        "{}{}",
        "\u{2501}".repeat(filled),
        "\u{2504}".repeat(width.saturating_sub(filled))
    );
    Span::styled(text, Style::default().fg(theme.status_warn))
}

fn render_scheduling_summary(snapshot: &SwarmSnapshot, theme: &TuiTheme) -> Line {
    let total = snapshot.children.len();
    let running = snapshot
        .children
        .iter()
        .filter(|child| matches!(child.agent.state, AgentLifecycleState::Running))
        .count();
    let queued = snapshot
        .children
        .iter()
        .filter(|child| matches!(child.agent.state, AgentLifecycleState::Queued))
        .count();
    let max_concurrency = snapshot.max_concurrency.max(1).min(total.max(1));
    Line::from_spans(vec![
        Span::styled("Scheduling: ", Style::default().fg(theme.text_muted)),
        Span::styled(
            format!("{running}/{total} running"),
            Style::default().fg(theme.text_primary),
        ),
        Span::styled(" · max concurrency ", Style::default().fg(theme.text_muted)),
        Span::styled(
            max_concurrency.to_string(),
            Style::default().fg(theme.text_primary),
        ),
        Span::styled(" · ", Style::default().fg(theme.text_muted)),
        Span::styled(
            format!("{queued} queued"),
            Style::default().fg(if queued > 0 {
                theme.status_warn
            } else {
                theme.text_primary
            }),
        ),
    ])
}

fn median_duration(mut durations: Vec<std::time::Duration>) -> Option<std::time::Duration> {
    if durations.is_empty() {
        return None;
    }
    durations.sort_unstable();
    durations.get(durations.len() / 2).copied()
}

fn swarm_status_label(snapshot: &SwarmSnapshot) -> &'static str {
    if snapshot
        .children
        .iter()
        .any(|child| child.agent.latest_text.as_deref() == Some("suspended"))
    {
        "Suspended"
    } else if snapshot
        .children
        .iter()
        .any(|child| matches!(child.agent.state, AgentLifecycleState::Failed))
    {
        "Failed"
    } else if snapshot
        .children
        .iter()
        .all(|child| matches!(child.agent.state, AgentLifecycleState::Completed))
    {
        "Completed"
    } else if snapshot
        .children
        .iter()
        .all(|child| matches!(child.agent.state, AgentLifecycleState::Queued))
    {
        "Queued"
    } else {
        "Running"
    }
}

fn swarm_status_style(snapshot: &SwarmSnapshot, theme: &TuiTheme) -> Style {
    let color = match swarm_status_label(snapshot) {
        "Completed" => theme.status_ok,
        "Failed" => theme.status_error,
        "Suspended" | "Queued" => theme.status_warn,
        _ => theme.brand,
    };
    Style::default().fg(color)
}

fn agent_status_color(state: AgentLifecycleState, theme: &TuiTheme) -> Color {
    match state {
        AgentLifecycleState::Completed => theme.status_ok,
        AgentLifecycleState::Failed => theme.status_error,
        AgentLifecycleState::Cancelled => theme.status_warn,
        AgentLifecycleState::Queued => theme.text_muted,
        AgentLifecycleState::Running => theme.brand,
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

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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

fn marker(state: AgentLifecycleState) -> &'static str {
    match state {
        AgentLifecycleState::Running => "\u{25cf}",
        AgentLifecycleState::Completed => "\u{2713}",
        AgentLifecycleState::Failed => "\u{2717}",
        AgentLifecycleState::Queued | AgentLifecycleState::Cancelled => "\u{25cc}",
    }
}

fn state_label(state: AgentLifecycleState) -> &'static str {
    match state {
        AgentLifecycleState::Queued => "queued",
        AgentLifecycleState::Running => "running",
        AgentLifecycleState::Completed => "done",
        AgentLifecycleState::Failed => "failed",
        AgentLifecycleState::Cancelled => "cancelled",
    }
}
