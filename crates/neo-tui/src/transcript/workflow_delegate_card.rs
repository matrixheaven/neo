use neo_agent_core::multi_agent::{
    AgentActivityKind, AgentLifecycleState, AgentSnapshot, AgentToolActivityPhase,
};

use crate::primitive::theme::TuiTheme;
use crate::primitive::{Line, Span, Style};

use super::{display_elapsed, format_elapsed, one_line, role_badge_style, role_label};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct AgentCounts {
    running: usize,
    queued: usize,
    failed: usize,
    completed: usize,
    stopped: usize,
}

impl AgentCounts {
    #[must_use]
    pub(super) fn total(self) -> usize {
        self.running + self.queued + self.failed + self.completed + self.stopped
    }

    #[must_use]
    pub(super) fn text(self) -> String {
        let mut parts = Vec::new();
        for (count, label) in [
            (self.running, "running"),
            (self.queued, "queued"),
            (self.failed, "failed"),
            (self.completed, "done"),
            (self.stopped, "stopped"),
        ] {
            if count > 0 {
                parts.push(format!("{count} {label}"));
            }
        }
        if parts.is_empty() {
            "0 agents".to_owned()
        } else {
            parts.join(" · ")
        }
    }
}

pub(super) struct WorkflowSummaryRender {
    pub(super) lines: Vec<Line>,
    pub(super) has_visible_animation: bool,
}

#[must_use]
pub(super) fn count_agents<'a>(agents: impl IntoIterator<Item = &'a AgentSnapshot>) -> AgentCounts {
    let mut counts = AgentCounts::default();
    for agent in agents {
        match agent.state {
            AgentLifecycleState::Running => counts.running += 1,
            AgentLifecycleState::Queued => counts.queued += 1,
            AgentLifecycleState::Completed => counts.completed += 1,
            AgentLifecycleState::Failed | AgentLifecycleState::TimedOut => counts.failed += 1,
            AgentLifecycleState::Cancelled | AgentLifecycleState::Interrupted => {
                counts.stopped += 1;
            }
        }
    }
    counts
}

#[must_use]
pub(super) fn render_workflow_delegate_card(
    agents: &[AgentSnapshot],
    width: usize,
    max_rows: usize,
    now_ms: Option<u64>,
    theme: &TuiTheme,
) -> Option<WorkflowSummaryRender> {
    if agents.is_empty() || max_rows == 0 {
        return None;
    }
    let counts = count_agents(agents);
    let mut lines = vec![render_summary_header(
        "Workflow Delegates",
        None,
        counts,
        width,
        theme,
    )];
    if max_rows == 1 {
        return Some(WorkflowSummaryRender {
            lines,
            has_visible_animation: false,
        });
    }

    let agent_refs = agents.iter().collect::<Vec<_>>();
    let content_rows = max_rows - 1;
    let pressured = content_rows < agent_refs.len();
    let visible_rows = if pressured {
        content_rows.saturating_sub(1)
    } else {
        content_rows
    };
    let indexes = selected_agent_indices(&agent_refs, visible_rows, pressured);
    let omitted = agents.len().saturating_sub(indexes.len());
    let visible_count = indexes.len();
    let mut has_visible_animation = false;
    for (visible_index, agent_index) in indexes.into_iter().enumerate() {
        let is_last = visible_index + 1 == visible_count && omitted == 0;
        let (line, animated) = render_agent_row(
            agent_refs[agent_index],
            if is_last { "└─ " } else { "├─ " },
            "",
            width,
            now_ms,
            theme,
        );
        lines.push(line);
        has_visible_animation |= animated;
    }
    if omitted > 0 && lines.len() < max_rows {
        lines.push(
            Line::styled(
                format!("└─ … {omitted} agents omitted"),
                Style::default().fg(theme.text_muted),
            )
            .truncate_to_width(width),
        );
    }

    Some(WorkflowSummaryRender {
        lines,
        has_visible_animation,
    })
}

#[must_use]
pub(super) fn render_summary_header(
    title: &str,
    detail: Option<&str>,
    counts: AgentCounts,
    width: usize,
    theme: &TuiTheme,
) -> Line {
    let (marker, color) = summary_marker(counts, theme);
    let mut spans = vec![
        Span::styled(format!("{marker} "), Style::default().fg(color)),
        Span::styled(title, Style::default().fg(theme.text_primary).bold()),
    ];
    if let Some(detail) = detail.filter(|detail| !detail.trim().is_empty()) {
        spans.push(Span::styled(
            format!(" · {}", one_line(detail)),
            Style::default().fg(theme.text_muted),
        ));
    }
    spans.push(Span::styled(
        format!(" · {}", counts.text()),
        Style::default().fg(theme.text_muted),
    ));
    Line::from_spans(spans).truncate_to_width(width)
}

#[must_use]
pub(super) fn selected_agent_indices(
    agents: &[&AgentSnapshot],
    max_rows: usize,
    pressured: bool,
) -> Vec<usize> {
    if max_rows == 0 {
        return Vec::new();
    }
    if !pressured || max_rows >= agents.len() {
        return (0..agents.len()).collect();
    }

    let mut indexes = Vec::with_capacity(max_rows);
    for states in [
        &[AgentLifecycleState::Failed, AgentLifecycleState::TimedOut][..],
        &[AgentLifecycleState::Running][..],
        &[AgentLifecycleState::Queued][..],
    ] {
        for (index, agent) in agents.iter().enumerate() {
            if states.contains(&agent.state) {
                indexes.push(index);
                if indexes.len() == max_rows {
                    return indexes;
                }
            }
        }
    }
    for (index, agent) in agents.iter().enumerate().rev() {
        if agent.state == AgentLifecycleState::Completed {
            indexes.push(index);
            if indexes.len() == max_rows {
                return indexes;
            }
        }
    }
    for (index, agent) in agents.iter().enumerate() {
        if matches!(
            agent.state,
            AgentLifecycleState::Cancelled | AgentLifecycleState::Interrupted
        ) {
            indexes.push(index);
            if indexes.len() == max_rows {
                break;
            }
        }
    }
    indexes
}

#[must_use]
pub(super) fn render_agent_row(
    agent: &AgentSnapshot,
    branch: &str,
    identity_prefix: &str,
    width: usize,
    now_ms: Option<u64>,
    theme: &TuiTheme,
) -> (Line, bool) {
    let (marker, marker_color) = agent_marker(agent.state, theme);
    let activity = agent_activity(agent);
    let mut spans = vec![
        Span::styled(branch, Style::default().fg(theme.text_muted)),
        Span::styled(identity_prefix, Style::default().fg(theme.text_muted)),
        Span::styled(format!("{marker} "), Style::default().fg(marker_color)),
        Span::styled(short_agent_id(agent), Style::default().fg(theme.text_muted)),
        Span::raw(" "),
        Span::styled(
            agent.display_name.as_str(),
            Style::default().fg(theme.text_primary),
        ),
        Span::styled(
            format!(" [{}]", role_label(agent.role)),
            role_badge_style(agent.role, theme),
        ),
        Span::styled(
            format!(" {}", agent_state_label(agent.state)),
            Style::default().fg(marker_color),
        ),
    ];
    if let Some(activity) = activity {
        spans.push(Span::styled(
            format!(" · {activity}"),
            Style::default().fg(theme.text_primary),
        ));
    }

    let mut line = Line::from_spans(spans);
    let elapsed = display_elapsed(agent, now_ms);
    let elapsed_suffix =
        (!elapsed.is_zero()).then(|| format!(" · {}", format_elapsed(elapsed.as_secs())));
    let animated = agent.state == AgentLifecycleState::Running
        && elapsed_suffix.as_ref().is_some_and(|suffix| {
            line.visible_width() + Line::raw(suffix.as_str()).visible_width() <= width
        });
    if let Some(suffix) = elapsed_suffix
        && line.visible_width() + Line::raw(suffix.as_str()).visible_width() <= width
    {
        let mut spans = line.into_spans();
        spans.push(Span::styled(suffix, Style::default().fg(theme.text_muted)));
        line = Line::from_spans(spans);
    }
    (line.truncate_to_width(width), animated)
}

fn summary_marker(
    counts: AgentCounts,
    theme: &TuiTheme,
) -> (&'static str, crate::primitive::Color) {
    if counts.failed > 0 {
        ("✗", theme.status_error)
    } else if counts.running > 0 {
        ("●", theme.brand)
    } else if counts.queued > 0 {
        ("◌", theme.status_pending)
    } else if counts.stopped > 0 {
        ("■", theme.status_warn)
    } else {
        ("✓", theme.status_ok)
    }
}

fn agent_marker(
    state: AgentLifecycleState,
    theme: &TuiTheme,
) -> (&'static str, crate::primitive::Color) {
    let (marker, color) = match state {
        AgentLifecycleState::Queued => ("◌", theme.status_pending),
        AgentLifecycleState::Running => ("●", theme.brand),
        AgentLifecycleState::Completed => ("✓", theme.status_ok),
        AgentLifecycleState::Failed | AgentLifecycleState::TimedOut => ("✗", theme.status_error),
        AgentLifecycleState::Cancelled | AgentLifecycleState::Interrupted => {
            ("■", theme.status_warn)
        }
    };
    (marker, color)
}

fn short_agent_id(agent: &AgentSnapshot) -> String {
    let raw = agent
        .id
        .as_str()
        .strip_prefix("agent_")
        .unwrap_or(agent.id.as_str());
    let suffix = raw
        .chars()
        .rev()
        .take(8)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("#{suffix}")
}

fn agent_activity(agent: &AgentSnapshot) -> Option<String> {
    if agent.state.is_terminal() {
        return agent
            .outcome
            .as_ref()
            .map(|outcome| one_line(&outcome.summary))
            .filter(|summary| !summary.is_empty())
            .or_else(|| agent.latest_text.as_deref().map(one_line))
            .filter(|summary| !summary.is_empty());
    }
    let tool = agent
        .activity
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            AgentActivityKind::Tool {
                name,
                summary,
                phase: AgentToolActivityPhase::Queued { .. } | AgentToolActivityPhase::Ongoing,
                ..
            } => Some((name.as_str(), summary.as_deref())),
            _ => None,
        });
    if let Some((name, summary)) = tool {
        return Some(
            match summary.map(one_line).filter(|summary| !summary.is_empty()) {
                Some(summary) => format!("{name} {summary}"),
                None => name.to_owned(),
            },
        );
    }
    agent
        .latest_text
        .as_deref()
        .map(one_line)
        .filter(|summary| !summary.is_empty())
}

fn agent_state_label(state: AgentLifecycleState) -> &'static str {
    match state {
        AgentLifecycleState::Queued => "queued",
        AgentLifecycleState::Running => "running",
        AgentLifecycleState::Completed => "completed",
        AgentLifecycleState::Failed => "failed",
        AgentLifecycleState::Cancelled => "cancelled",
        AgentLifecycleState::TimedOut => "timed out",
        AgentLifecycleState::Interrupted => "interrupted",
    }
}
