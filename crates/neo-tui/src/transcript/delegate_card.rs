use neo_agent_core::multi_agent::{AgentLifecycleState, AgentSnapshot, AgentTerminalReason};

use crate::primitive::theme::TuiTheme;
use crate::primitive::{Component, Expandable, Finalization, Line, Span, Style};
use crate::transcript::{
    MAX_CHILD_TOOL_ROWS, can_detach, child_activity_view, display_elapsed, format_elapsed,
    format_token_count, render_child_final, render_child_thinking, render_child_tool_row,
    role_label,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegateCardComponent {
    turn: Option<u32>,
    snapshot: AgentSnapshot,
    expanded: bool,
    now_ms: Option<u64>,
}

impl DelegateCardComponent {
    #[must_use]
    pub fn new(snapshot: AgentSnapshot) -> Self {
        Self::new_with_turn(None, snapshot)
    }

    #[must_use]
    fn new_with_turn(turn: Option<u32>, snapshot: AgentSnapshot) -> Self {
        Self {
            turn,
            snapshot,
            expanded: false,
            now_ms: None,
        }
    }

    #[must_use]
    pub fn with_turn(turn: u32, snapshot: AgentSnapshot) -> Self {
        Self::new_with_turn(Some(turn), snapshot)
    }

    pub fn update(&mut self, snapshot: AgentSnapshot) {
        self.snapshot = snapshot;
    }

    #[must_use]
    pub fn id(&self) -> &str {
        self.snapshot.id.as_str()
    }

    #[must_use]
    pub const fn turn(&self) -> Option<u32> {
        self.turn
    }

    #[must_use]
    pub const fn snapshot(&self) -> &AgentSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub fn into_snapshot(self) -> AgentSnapshot {
        self.snapshot
    }

    pub fn on_render_tick(&mut self, now_ms: u64) -> bool {
        if self.snapshot.state.is_terminal() {
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
        let phase = display_phase(&self.snapshot);
        let accent = Style::default().fg(status_color(phase, theme));
        let muted = Style::default().fg(theme.text_muted);
        let primary = Style::default().fg(theme.text_primary);
        let elapsed = display_elapsed(&self.snapshot, self.now_ms);
        let mut lines = Vec::new();

        lines.push(
            Line::from_spans(vec![
                Span::styled(status_marker(phase), accent),
                Span::raw(" "),
                Span::styled(self.snapshot.display_name.as_str(), accent),
                Span::styled(
                    format!(
                        " {} Agent {} ({}) · {} tools · {} · {} tok",
                        role_label(self.snapshot.role),
                        state_label(phase),
                        self.snapshot.task_title,
                        self.snapshot.tool_count,
                        format_elapsed(elapsed.as_secs()),
                        format_token_count(self.snapshot.token_count)
                    ),
                    primary,
                ),
            ])
            .truncate_to_width(width),
        );

        if can_detach(&self.snapshot) {
            lines.push(Line::styled("  Press Ctrl+B to run in background", muted));
        }

        let activity = child_activity_view(&self.snapshot, MAX_CHILD_TOOL_ROWS);
        for tool in &activity.tools {
            lines.extend(render_child_tool_row(tool, width, "  ", theme));
        }
        if let Some(thinking) = activity.thinking.as_deref() {
            lines.extend(render_child_thinking(thinking, width, "  ", theme));
        }
        if let Some(final_text) = activity.final_text.as_deref() {
            lines.push(render_child_final(
                final_text,
                activity.final_is_error,
                width,
                "  ",
                theme,
            ));
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
        if self.snapshot.state.is_terminal() {
            Finalization::Finalized
        } else {
            Finalization::Live
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DelegateDisplayPhase {
    Queued,
    Running,
    Backgrounded,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    Lost,
    Killed,
}

fn display_phase(snapshot: &AgentSnapshot) -> DelegateDisplayPhase {
    if snapshot.detached_from_foreground && snapshot.state == AgentLifecycleState::Running {
        return DelegateDisplayPhase::Backgrounded;
    }
    match snapshot.terminal_reason {
        Some(AgentTerminalReason::Lost) => DelegateDisplayPhase::Lost,
        Some(AgentTerminalReason::Killed) => DelegateDisplayPhase::Killed,
        _ => match snapshot.state {
            AgentLifecycleState::Queued => DelegateDisplayPhase::Queued,
            AgentLifecycleState::Running => DelegateDisplayPhase::Running,
            AgentLifecycleState::Completed => DelegateDisplayPhase::Completed,
            AgentLifecycleState::Failed => DelegateDisplayPhase::Failed,
            AgentLifecycleState::Cancelled => DelegateDisplayPhase::Cancelled,
            AgentLifecycleState::TimedOut => DelegateDisplayPhase::TimedOut,
        },
    }
}

fn status_color(phase: DelegateDisplayPhase, theme: &TuiTheme) -> crate::primitive::Color {
    match phase {
        DelegateDisplayPhase::Completed => theme.status_ok,
        DelegateDisplayPhase::Failed
        | DelegateDisplayPhase::TimedOut
        | DelegateDisplayPhase::Lost
        | DelegateDisplayPhase::Killed => theme.status_error,
        DelegateDisplayPhase::Cancelled => theme.status_warn,
        DelegateDisplayPhase::Queued
        | DelegateDisplayPhase::Running
        | DelegateDisplayPhase::Backgrounded => theme.brand,
    }
}

fn status_marker(phase: DelegateDisplayPhase) -> &'static str {
    match phase {
        DelegateDisplayPhase::Running | DelegateDisplayPhase::Backgrounded => "●",
        DelegateDisplayPhase::Completed => "✓",
        DelegateDisplayPhase::Failed
        | DelegateDisplayPhase::TimedOut
        | DelegateDisplayPhase::Lost
        | DelegateDisplayPhase::Killed => "✗",
        DelegateDisplayPhase::Queued | DelegateDisplayPhase::Cancelled => "◌",
    }
}

fn state_label(phase: DelegateDisplayPhase) -> &'static str {
    match phase {
        DelegateDisplayPhase::Queued => "Queued",
        DelegateDisplayPhase::Running => "Running",
        DelegateDisplayPhase::Backgrounded => "Backgrounded",
        DelegateDisplayPhase::Completed => "Completed",
        DelegateDisplayPhase::Failed => "Failed",
        DelegateDisplayPhase::Cancelled => "Cancelled",
        DelegateDisplayPhase::TimedOut => "Timed Out",
        DelegateDisplayPhase::Lost => "Lost",
        DelegateDisplayPhase::Killed => "Killed",
    }
}
