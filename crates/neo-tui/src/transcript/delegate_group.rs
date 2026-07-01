use std::time::Duration;

use neo_agent_core::multi_agent::{AgentLifecycleState, AgentRunMode, AgentSnapshot};

use crate::primitive::theme::TuiTheme;
use crate::primitive::{Component, Finalization, Line, Span, Style};
use crate::transcript::{
    MAX_CHILD_TOOL_ROWS, can_detach, child_activity_view, display_elapsed,
    format_cache_token_usage, format_elapsed, format_token_count, render_child_body,
    render_child_final, render_child_thinking, render_child_tool_row, role_label,
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
            Span::styled(" Delegate group · ", Style::default().fg(theme.text_muted)),
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
        let state_style = Style::default().fg(if agent.state.is_terminal() {
            theme.text_muted
        } else {
            theme.text_primary
        });
        let muted = Style::default().fg(theme.text_muted);
        let primary = Style::default().fg(theme.text_primary);
        let mut lines = vec![
            Line::from_spans(vec![
                Span::raw(format!("  {branch} ")),
                Span::styled(agent.display_name.as_str(), state_style),
                Span::raw("  "),
                Span::styled(format!("[{}]", role_label(agent.role)), muted),
                Span::styled(
                    format!(
                        "  {}{}",
                        agent.display_title(),
                        format_stats(agent, self.now_ms)
                    ),
                    primary,
                ),
            ])
            .truncate_to_width(width),
        ];

        let indent = format!("  {continuation}    ");
        let view = child_activity_view(agent, MAX_CHILD_TOOL_ROWS);
        for row in &view.tools {
            lines.extend(render_child_tool_row(row, width, &indent, theme));
        }
        if let Some(thinking) = view.thinking.as_deref() {
            lines.extend(render_child_thinking(thinking, width, &indent, theme));
        }
        if let Some(body) = view.body_text.as_deref()
            && let Some(line) = render_child_body(body, width, &indent, theme)
        {
            lines.push(line);
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

        if lines.len() == 1 {
            lines.push(
                Line::styled(
                    format!("{indent}◌ {}", fallback_activity(agent)),
                    Style::default().fg(theme.text_muted),
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
    if let Some(cache) = format_cache_token_usage(agent) {
        parts.push(cache);
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" · {}", parts.join(" · "))
    }
}

fn fallback_activity(agent: &AgentSnapshot) -> String {
    match agent.state {
        AgentLifecycleState::Queued => "Waiting for scheduler slot".to_owned(),
        AgentLifecycleState::Running => "Running...".to_owned(),
        AgentLifecycleState::Completed => "Completed".to_owned(),
        AgentLifecycleState::Failed => "Failed".to_owned(),
        AgentLifecycleState::Cancelled => "Cancelled".to_owned(),
        AgentLifecycleState::TimedOut => "Timed out".to_owned(),
    }
}
