use neo_agent_core::multi_agent::{
    AgentActivityKind, AgentLifecycleState, AgentSnapshot, AgentToolActivityPhase,
    SwarmEstimatorPhase, SwarmProgressEstimator, SwarmSnapshot,
};

use crate::primitive::theme::TuiTheme;
use crate::primitive::{Color, Component, Expandable, Finalization, Line, Span, Style};
use crate::transcript::{
    MAX_CHILD_TOOL_ROWS, child_activity_view, compact_chars, display_elapsed,
    format_cache_token_usage, format_elapsed, format_token_count, one_line, render_child_body,
    render_child_final, render_child_thinking, render_child_tool_row, role_badge_style, role_label,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwarmCardComponent {
    snapshot: SwarmSnapshot,
    expanded: bool,
    estimator: SwarmProgressEstimator,
    now_ms: Option<u64>,
}

impl SwarmCardComponent {
    #[must_use]
    pub fn new(snapshot: SwarmSnapshot) -> Self {
        let mut component = Self {
            snapshot,
            expanded: false,
            estimator: SwarmProgressEstimator::default(),
            now_ms: None,
        };
        component.sync_estimator_from_snapshot(snapshot_time_ms(&component.snapshot));
        component
    }

    pub fn update(&mut self, snapshot: SwarmSnapshot) {
        self.snapshot = snapshot;
        self.sync_estimator_from_snapshot(snapshot_time_ms(&self.snapshot));
    }

    pub fn on_render_tick(&mut self, now_ms: u64) -> bool {
        self.now_ms = Some(now_ms);
        self.sync_estimator_from_snapshot(now_ms);
        self.estimator.has_pending_catchup()
            || self
                .snapshot
                .children
                .iter()
                .any(|child| !child.agent.state.is_terminal())
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
        let child_progress = self.child_progresses();
        let progress = aggregate_progress(&child_progress);
        let mut lines = Vec::new();
        let total = self.snapshot.children.len();
        let running = self
            .snapshot
            .children
            .iter()
            .filter(|child| matches!(child.agent.state, AgentLifecycleState::Running))
            .count();
        let completed = self
            .snapshot
            .children
            .iter()
            .filter(|child| matches!(child.agent.state, AgentLifecycleState::Completed))
            .count();
        let queued = self
            .snapshot
            .children
            .iter()
            .filter(|child| matches!(child.agent.state, AgentLifecycleState::Queued))
            .count();

        lines.push(
            Line::from_spans(vec![
                Span::styled(marker(self.snapshot.state), brand),
                Span::styled(" DelegateSwarm · ", muted),
                Span::styled(state_label(self.snapshot.state), brand),
                Span::styled(" · ", muted),
                Span::styled(self.snapshot.description.as_str(), primary),
                Span::styled(
                    format!(" · {total} agents · {running} run · {completed} done · {queued} wait · progress ["),
                    muted,
                ),
                Span::styled(
                    compact_progress_meter(progress, 18),
                    Style::default().fg(theme.status_warn),
                ),
                Span::styled(
                    format!(
                        "] {:.0}% · bayes estimate · max {}",
                        progress * 100.0,
                        self.snapshot.max_concurrency,
                    ),
                    muted,
                ),
            ])
            .truncate_to_width(width),
        );
        lines.push(
            Line::styled(format!("│ {}", self.snapshot.swarm_id), muted).truncate_to_width(width),
        );

        let mut children = self
            .snapshot
            .children
            .iter()
            .zip(child_progress.iter().copied())
            .collect::<Vec<_>>();
        children.sort_by_key(|(child, _)| child.item_index);
        let last_child_index = children.len().saturating_sub(1);

        for (index, (child, progress)) in children.into_iter().enumerate() {
            let state_style = Style::default().fg(agent_status_color(child.agent.state, theme));
            let elapsed = display_elapsed(&child.agent, self.now_ms);
            let branch = if index == last_child_index {
                "└─"
            } else {
                "├─"
            };
            lines.push(
                Line::from_spans(vec![
                    Span::styled(format!("{branch} "), muted),
                    Span::styled(child.agent.display_name.as_str(), state_style),
                    Span::raw("  "),
                    Span::styled(
                        format!("[{}]", role_label(child.agent.role)),
                        role_badge_style(child.agent.role, theme),
                    ),
                    Span::raw(" "),
                    Span::styled(marker(child.agent.state), state_style),
                    Span::raw(" ["),
                    progress_bar_line(progress, child.agent.state, theme),
                    Span::raw("] "),
                    Span::styled(
                        format!(
                            " {} · {} tools · {} · {} · {}",
                            state_label(child.agent.state),
                            child.agent.tool_count,
                            format_elapsed(elapsed.as_secs()),
                            child_token_stats(&child.agent),
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
                "● Orchestrating...",
                Style::default().fg(theme.status_warn),
            ));
        } else if any_suspended {
            lines.push(
                Line::from_spans(vec![
                    Span::styled(
                        "● Suspended (rate-limit) ",
                        Style::default().fg(theme.status_warn),
                    ),
                    Span::styled("━".repeat(10), Style::default().fg(theme.status_warn)),
                ])
                .truncate_to_width(width),
            );
        } else {
            lines.push(
                Line::from_spans(vec![
                    Span::styled(
                        format!("● Working... {:.0}% ", progress * 100.0),
                        Style::default().fg(theme.status_warn),
                    ),
                    progress_meter(progress, theme),
                ])
                .truncate_to_width(width),
            );
        }

        if self.expanded {
            for (index, child) in self.snapshot.children.iter().enumerate() {
                let state_style = Style::default().fg(agent_status_color(child.agent.state, theme));
                let elapsed = display_elapsed(&child.agent, self.now_ms);
                let branch = if index + 1 == self.snapshot.children.len() {
                    "└─"
                } else {
                    "├─"
                };
                let continuation = if index + 1 == self.snapshot.children.len() {
                    "   "
                } else {
                    "│  "
                };
                lines.push(
                    Line::from_spans(vec![
                        Span::raw(format!("  {branch} ")),
                        Span::styled(child.agent.display_name.as_str(), state_style),
                        Span::raw("  "),
                        Span::styled(
                            format!("[{}]", role_label(child.agent.role)),
                            role_badge_style(child.agent.role, theme),
                        ),
                        Span::styled(
                            format!(
                                "  {} · {} · {} tools · {}",
                                state_label(child.agent.state),
                                format_elapsed(elapsed.as_secs()),
                                child.agent.tool_count,
                                child_token_stats(&child.agent),
                            ),
                            primary,
                        ),
                    ])
                    .truncate_to_width(width),
                );

                let indent = format!("  {continuation} ");
                let view = child_activity_view(&child.agent, MAX_CHILD_TOOL_ROWS);
                for tool in &view.tools {
                    lines.extend(render_child_tool_row(tool, width, &indent, theme));
                }
                if let Some(thinking) = view.thinking.as_deref() {
                    lines.extend(render_child_thinking(thinking, width, &indent, theme));
                }
                if let Some(body) = view
                    .body_text
                    .as_deref()
                    .and_then(|text| render_child_body(text, width, &indent, theme))
                {
                    lines.push(body);
                }
                if let Some(final_text) = view.final_text.as_deref() {
                    lines.push(render_child_final(
                        final_text,
                        view.final_is_error,
                        width,
                        &indent,
                        theme,
                    ));
                }
            }
        }

        lines
    }

    fn child_progresses(&self) -> Vec<f32> {
        let now_ms = self
            .now_ms
            .unwrap_or_else(|| snapshot_time_ms(&self.snapshot));
        let mut estimator = self.estimator.clone();
        self.snapshot
            .children
            .iter()
            .map(|child| {
                estimator
                    .estimate(
                        child.agent.id.as_str(),
                        estimator_phase(child.agent.state),
                        1.0,
                        now_ms,
                    )
                    .progress
            })
            .collect()
    }

    fn sync_estimator_from_snapshot(&mut self, now_ms: u64) {
        for child in &self.snapshot.children {
            let id = child.agent.id.as_str();
            self.estimator.ensure_member(id, now_ms);
            if child.agent.state == AgentLifecycleState::Running {
                self.estimator
                    .mark_started(id, child.agent.started_at_ms.unwrap_or(now_ms));
            }
            if child.agent.state != AgentLifecycleState::Queued {
                for tool_id in child_tool_ids(&child.agent) {
                    self.estimator.record_tool_call(id, tool_id, now_ms);
                }
            }
            match child.agent.state {
                AgentLifecycleState::Completed => self.estimator.mark_completed(id, now_ms),
                AgentLifecycleState::Failed | AgentLifecycleState::TimedOut => {
                    self.estimator.mark_failed(id, now_ms);
                }
                AgentLifecycleState::Cancelled => self.estimator.mark_cancelled(id, now_ms),
                AgentLifecycleState::Queued | AgentLifecycleState::Running => {}
            }
        }
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
        if self
            .snapshot
            .children
            .iter()
            .all(|child| child.agent.state.is_terminal())
        {
            Finalization::Finalized
        } else {
            Finalization::Live
        }
    }
}

fn progress_bar_line(progress: f32, state: AgentLifecycleState, theme: &TuiTheme) -> Span {
    Span::styled(
        progress_bar_text(progress, state),
        Style::default().fg(agent_status_color(state, theme)),
    )
}

fn progress_bar_text(progress: f32, state: AgentLifecycleState) -> String {
    const WIDTH: usize = 8;
    let progress = if state.is_terminal() {
        1.0
    } else {
        progress.clamp(0.0, 0.95)
    };
    let filled = (progress * WIDTH as f32).floor() as usize;
    format!(
        "{}{}",
        "■".repeat(filled),
        "·".repeat(WIDTH.saturating_sub(filled))
    )
}

fn compact_progress_meter(progress: f32, width: usize) -> String {
    let width = width.max(1);
    let filled = ((progress.clamp(0.0, 1.0) * width as f32).round() as usize).min(width);
    format!("{}{}", "■".repeat(filled), "·".repeat(width - filled))
}

fn child_token_stats(agent: &AgentSnapshot) -> String {
    let mut parts = vec![format!("{} tok", format_token_count(agent.token_count))];
    if let Some(cache) = format_cache_token_usage(agent) {
        parts.push(cache);
    }
    parts.join(" · ")
}

fn child_activity_summary(agent: &AgentSnapshot, fallback_item: &str) -> String {
    if agent.state == AgentLifecycleState::Queued {
        if !agent.task_title.is_empty() {
            return compact_chars(&one_line(&agent.task_title), 96);
        }
        return compact_chars(&one_line(fallback_item), 96);
    }
    if let Some((name, summary)) = agent
        .activity
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            AgentActivityKind::Tool {
                name,
                summary,
                phase,
                ..
            } if *phase == AgentToolActivityPhase::Ongoing => {
                Some((name.as_str(), summary.as_deref()))
            }
            AgentActivityKind::Tool { .. } | AgentActivityKind::Text { .. } => None,
        })
    {
        return compact_chars(&format_tool_summary("Using", name, summary), 96);
    }
    if let Some((name, summary)) = agent
        .activity
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            AgentActivityKind::Tool { name, summary, .. } => {
                Some((name.as_str(), summary.as_deref()))
            }
            AgentActivityKind::Text { .. } => None,
        })
    {
        return compact_chars(&format_tool_summary("Used", name, summary), 96);
    }
    let view = child_activity_view(agent, 1);
    if let Some(tool) = view.tools.last() {
        let verb = if tool.phase == AgentToolActivityPhase::Ongoing {
            "Using"
        } else {
            "Used"
        };
        return compact_chars(&format_tool_summary(verb, tool.name, tool.summary), 96);
    }
    if let Some(final_text) = view.final_text {
        return compact_chars(&one_line(&final_text), 96);
    }
    if let Some(text) = &agent.latest_text
        && !text.trim().is_empty()
    {
        return compact_chars(&one_line(text), 96);
    }
    if !agent.task_title.is_empty() {
        return compact_chars(&one_line(&agent.task_title), 96);
    }
    compact_chars(&one_line(fallback_item), 96)
}

fn format_tool_summary(verb: &str, name: &str, summary: Option<&str>) -> String {
    match summary {
        Some(summary) if !summary.trim().is_empty() => {
            format!("{verb} {name} ({})", one_line(summary))
        }
        _ => format!("{verb} {name}"),
    }
}

fn progress_meter(progress: f32, theme: &TuiTheme) -> Span {
    let width = 30usize;
    let filled = ((progress.clamp(0.0, 1.0) * width as f32).round() as usize).min(width);
    let text = format!(
        "{}{}",
        "━".repeat(filled),
        "┄".repeat(width.saturating_sub(filled))
    );
    Span::styled(text, Style::default().fg(theme.status_warn))
}

fn aggregate_progress(child_progress: &[f32]) -> f32 {
    if child_progress.is_empty() {
        return 1.0;
    }
    child_progress.iter().sum::<f32>() / child_progress.len() as f32
}

fn child_tool_ids(agent: &AgentSnapshot) -> impl Iterator<Item = &str> {
    agent.activity.iter().filter_map(|entry| match &entry.kind {
        AgentActivityKind::Tool { id, .. } => Some(id.as_str()),
        AgentActivityKind::Text { .. } => None,
    })
}

fn estimator_phase(state: AgentLifecycleState) -> SwarmEstimatorPhase {
    match state {
        AgentLifecycleState::Queued => SwarmEstimatorPhase::Queued,
        AgentLifecycleState::Running => SwarmEstimatorPhase::Running,
        AgentLifecycleState::Completed => SwarmEstimatorPhase::Completed,
        AgentLifecycleState::Failed => SwarmEstimatorPhase::Failed,
        AgentLifecycleState::Cancelled => SwarmEstimatorPhase::Cancelled,
        AgentLifecycleState::TimedOut => SwarmEstimatorPhase::TimedOut,
    }
}

fn snapshot_time_ms(snapshot: &SwarmSnapshot) -> u64 {
    snapshot
        .children
        .iter()
        .map(|child| child.agent.updated_at_ms.max(child.agent.created_at_ms))
        .max()
        .unwrap_or(0)
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
        Span::styled(
            format!(" · max concurrency {max_concurrency}"),
            Style::default().fg(theme.text_muted),
        ),
        Span::styled(
            format!(" · {queued} queued"),
            Style::default().fg(theme.text_muted),
        ),
    ])
}

fn agent_status_color(state: AgentLifecycleState, theme: &TuiTheme) -> Color {
    match state {
        AgentLifecycleState::Completed => theme.status_ok,
        AgentLifecycleState::Failed | AgentLifecycleState::TimedOut => theme.status_error,
        AgentLifecycleState::Cancelled => theme.status_warn,
        AgentLifecycleState::Queued | AgentLifecycleState::Running => theme.brand,
    }
}

fn marker(state: AgentLifecycleState) -> &'static str {
    match state {
        AgentLifecycleState::Queued => "◌",
        AgentLifecycleState::Running => "●",
        AgentLifecycleState::Completed => "✓",
        AgentLifecycleState::Failed | AgentLifecycleState::TimedOut => "✗",
        AgentLifecycleState::Cancelled => "◌",
    }
}

fn state_label(state: AgentLifecycleState) -> &'static str {
    match state {
        AgentLifecycleState::Queued => "queued",
        AgentLifecycleState::Running => "running",
        AgentLifecycleState::Completed => "done",
        AgentLifecycleState::Failed => "failed",
        AgentLifecycleState::Cancelled => "cancelled",
        AgentLifecycleState::TimedOut => "timed out",
    }
}
