use std::time::Duration;

use neo_agent_core::multi_agent::{
    AgentLifecycleState, AgentRunMode, AgentSnapshot, AgentToolActivityPhase,
};

use crate::primitive::theme::TuiTheme;
use crate::primitive::{Component, Finalization, Line, Span, Style};
use crate::transcript::{
    can_detach, child_activity_view, display_elapsed, format_elapsed, format_token_count, one_line,
    role_label,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegateGroupComponent {
    turn: u32,
    agents: Vec<AgentSnapshot>,
    now_ms: Option<u64>,
}

impl DelegateGroupComponent {
    #[must_use]
    pub fn new(turn: u32, agents: Vec<AgentSnapshot>) -> Self {
        Self {
            turn,
            agents,
            now_ms: None,
        }
    }

    #[must_use]
    pub const fn turn(&self) -> u32 {
        self.turn
    }

    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.agents.iter().any(|agent| agent.id.as_str() == id)
    }

    pub fn upsert(&mut self, snapshot: AgentSnapshot) {
        if let Some(existing) = self.agents.iter_mut().find(|agent| agent.id == snapshot.id) {
            *existing = snapshot;
        } else {
            self.agents.push(snapshot);
        }
    }

    pub fn on_render_tick(&mut self, now_ms: u64) -> bool {
        if self.agents.iter().all(|agent| agent.state.is_terminal()) {
            return false;
        }
        if self.now_ms == Some(now_ms) {
            return false;
        }
        self.now_ms = Some(now_ms);
        true
    }

    #[must_use]
    pub fn render_with_theme(&self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        let mut lines = vec![self.header(width, theme)];
        for (index, agent) in self.agents.iter().enumerate() {
            let last = index + 1 == self.agents.len();
            lines.extend(self.render_agent(agent, last, width, theme));
        }
        if self.agents.iter().any(can_detach) {
            lines.push(Line::styled(
                "  Press Ctrl+B to run in background",
                Style::default().fg(theme.text_muted),
            ));
        }
        lines
    }

    fn header(&self, width: usize, theme: &TuiTheme) -> Line {
        let all_terminal = self.agents.iter().all(|agent| agent.state.is_terminal());
        let marker = if all_terminal { "•" } else { "●" };
        let marker_color = if all_terminal {
            theme.status_ok
        } else {
            theme.text_primary
        };
        let total = self.agents.len();
        let elapsed = self.max_elapsed();
        let label = if all_terminal {
            format!("{total} agents finished")
        } else {
            let running = self
                .agents
                .iter()
                .filter(|agent| {
                    agent.state == AgentLifecycleState::Running
                        && !(agent.detached_from_foreground
                            && agent.mode == AgentRunMode::Background)
                })
                .count();
            let waiting = self
                .agents
                .iter()
                .filter(|agent| agent.state == AgentLifecycleState::Queued)
                .count();
            let backgrounded = self
                .agents
                .iter()
                .filter(|agent| {
                    agent.detached_from_foreground && agent.state == AgentLifecycleState::Running
                })
                .count();
            let mut parts = Vec::new();
            if running > 0 {
                parts.push(format!("{running} running"));
            }
            if waiting > 0 {
                parts.push(format!("{waiting} waiting"));
            }
            if backgrounded > 0 {
                parts.push(format!("{backgrounded} backgrounded"));
            }
            if parts.is_empty() {
                format!("Running {total} agents")
            } else {
                format!("Running {total} agents ({})", parts.join(", "))
            }
        };
        let tools = self
            .agents
            .iter()
            .map(|agent| agent.tool_count)
            .sum::<usize>();
        let tokens = self
            .agents
            .iter()
            .map(|agent| agent.token_count)
            .sum::<usize>();
        let tail = if all_terminal {
            format!(
                " · {tools} tools · {} · {} tok",
                format_elapsed(elapsed.as_secs()),
                format_token_count(tokens)
            )
        } else {
            format!(" · {}", format_elapsed(elapsed.as_secs()))
        };
        Line::from_spans(vec![
            Span::styled(marker, Style::default().fg(marker_color)),
            Span::raw(" "),
            Span::styled(label, Style::default().fg(theme.brand)),
            Span::styled(tail, Style::default().fg(theme.text_muted)),
        ])
        .truncate_to_width(width)
    }

    fn render_agent(
        &self,
        agent: &AgentSnapshot,
        is_last: bool,
        width: usize,
        theme: &TuiTheme,
    ) -> Vec<Line> {
        let branch = if is_last { "└─" } else { "├─" };
        let continuation = if is_last { "   " } else { "│  " };
        let activity = latest_activity(agent).unwrap_or_else(|| fallback_activity(agent));
        let mut lines = vec![
            Line::from_spans(vec![
                Span::raw(format!("  {branch} ")),
                Span::styled(
                    format!("{} · {}", role_label(agent.role), agent.display_title()),
                    Style::default().fg(theme.text_primary),
                ),
                Span::styled(
                    format_stats(agent, self.now_ms),
                    Style::default().fg(theme.text_muted),
                ),
            ])
            .truncate_to_width(width),
        ];
        if !agent.state.is_terminal() {
            lines.push(
                Line::styled(
                    format!("  {continuation}    {activity}"),
                    Style::default().fg(theme.text_muted),
                )
                .truncate_to_width(width),
            );
        } else if matches!(
            agent.state,
            AgentLifecycleState::Failed | AgentLifecycleState::TimedOut
        ) {
            lines.push(
                Line::styled(
                    format!("  {continuation}    Error: {activity}"),
                    Style::default().fg(theme.status_error),
                )
                .truncate_to_width(width),
            );
        }
        lines
    }

    fn max_elapsed(&self) -> Duration {
        self.agents
            .iter()
            .map(|agent| display_elapsed(agent, self.now_ms))
            .max()
            .unwrap_or_default()
    }
}

impl Component for DelegateGroupComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.render_with_theme(width, &TuiTheme::default())
    }

    fn finalization(&self) -> Finalization {
        if self.agents.iter().all(|agent| agent.state.is_terminal()) {
            Finalization::Finalized
        } else {
            Finalization::Live
        }
    }
}

fn format_stats(agent: &AgentSnapshot, now_ms: Option<u64>) -> String {
    let elapsed = display_elapsed(agent, now_ms);
    let mut parts = Vec::new();
    if agent.tool_count > 0 {
        parts.push(format!("{} tools", agent.tool_count));
    }
    if !elapsed.is_zero() {
        parts.push(format_elapsed(elapsed.as_secs()));
    }
    if agent.token_count > 0 {
        parts.push(format!("{} tok", format_token_count(agent.token_count)));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" · {}", parts.join(" · "))
    }
}

fn latest_activity(agent: &AgentSnapshot) -> Option<String> {
    let view = child_activity_view(agent, 1);
    if let Some(tool) = view.tools.last() {
        let verb = if tool.phase == AgentToolActivityPhase::Ongoing {
            "Using"
        } else {
            "Used"
        };
        return Some(match tool.summary {
            Some(summary) if !summary.trim().is_empty() => {
                format!("{verb} {} ({})", tool.name, one_line(summary))
            }
            _ => format!("{verb} {}", tool.name),
        });
    }
    view.final_text.map(|text| one_line(&text))
}

fn fallback_activity(agent: &AgentSnapshot) -> String {
    match agent.state {
        AgentLifecycleState::Queued => "Waiting...".to_owned(),
        AgentLifecycleState::Running => "Running...".to_owned(),
        AgentLifecycleState::Completed => "Completed".to_owned(),
        AgentLifecycleState::Failed => "Failed".to_owned(),
        AgentLifecycleState::Cancelled => "Cancelled".to_owned(),
        AgentLifecycleState::TimedOut => "Timed out".to_owned(),
    }
}
