use neo_agent_core::multi_agent::{
    AgentActivityKind, AgentLifecycleState, AgentSnapshot, AgentToolActivityPhase,
};

use crate::primitive::theme::TuiTheme;
use crate::primitive::{Line, Span, Style};

use super::child_activity::instruction_header;
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
    now_ms: Option<u64>,
    theme: &TuiTheme,
) -> Option<WorkflowSummaryRender> {
    if agents.is_empty() {
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
    for (index, agent) in agents.iter().enumerate() {
        let is_last = index + 1 == agents.len();
        lines.push(render_agent_row(
            agent,
            if is_last { "└─ " } else { "├─ " },
            "",
            width,
            now_ms,
            theme,
        ));
    }
    Some(WorkflowSummaryRender { lines })
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
pub(super) fn render_agent_row(
    agent: &AgentSnapshot,
    branch: &str,
    identity_prefix: &str,
    width: usize,
    now_ms: Option<u64>,
    theme: &TuiTheme,
) -> Line {
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
    if let Some(suffix) = elapsed_suffix
        && line.visible_width() + Line::raw(suffix.as_str()).visible_width() <= width
    {
        let mut spans = line.into_spans();
        spans.push(Span::styled(suffix, Style::default().fg(theme.text_muted)));
        line = Line::from_spans(spans);
    }
    line.truncate_to_width(width)
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
    if let Some(AgentActivityKind::Instruction { outcome, .. }) =
        agent.activity.last().map(|entry| &entry.kind)
    {
        return Some(instruction_header(*outcome).to_owned());
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
