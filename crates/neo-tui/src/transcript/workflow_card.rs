use crate::primitive::theme::TuiTheme;
use crate::primitive::{Color, Component, Finalization, Line, Span, Style};
use crate::transcript::format_elapsed;
use neo_agent_core::workflow::{WorkflowSnapshot, WorkflowState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowCardComponent {
    snapshot: WorkflowSnapshot,
    max_projection_sequence: Option<u64>,
    now_ms: Option<u64>,
}

impl WorkflowCardComponent {
    #[must_use]
    pub fn new(snapshot: WorkflowSnapshot) -> Self {
        let max_projection_sequence = snapshot.projection_sequence;
        Self {
            snapshot,
            max_projection_sequence,
            now_ms: None,
        }
    }

    pub(crate) fn accepts_projection(&self, incoming: &WorkflowSnapshot) -> bool {
        if incoming.recovery_failure {
            return incoming.state.is_terminal() && self.snapshot != *incoming;
        }
        if self.snapshot.recovery_failure {
            return incoming.projection_sequence.is_some_and(|sequence| {
                self.max_projection_sequence
                    .is_none_or(|watermark| sequence > watermark)
            });
        }
        match (
            self.snapshot.projection_sequence,
            incoming.projection_sequence,
        ) {
            (Some(current), Some(incoming)) => incoming > current,
            (None, Some(_)) => true,
            (Some(_), None) => false,
            (None, None) => !self.snapshot.state.is_terminal() || incoming.state.is_terminal(),
        }
    }

    pub fn update(&mut self, snapshot: WorkflowSnapshot) -> bool {
        if self.snapshot == snapshot {
            return false;
        }
        if let Some(sequence) = snapshot.projection_sequence
            && self
                .max_projection_sequence
                .is_none_or(|watermark| sequence > watermark)
        {
            self.max_projection_sequence = Some(sequence);
        }
        self.snapshot = snapshot;
        self.now_ms = None;
        true
    }

    pub fn interrupt(&mut self) -> bool {
        if self.snapshot.state != WorkflowState::Running {
            return false;
        }
        self.snapshot.state = WorkflowState::Failed;
        self.snapshot.terminal_reason = Some("interrupted when terminal exited".to_owned());
        true
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.snapshot.id.0
    }

    #[must_use]
    pub const fn snapshot(&self) -> &WorkflowSnapshot {
        &self.snapshot
    }

    pub fn on_render_tick(&mut self, now_ms: u64) -> bool {
        if !self.has_ticking_elapsed() || self.now_ms == Some(now_ms) {
            return false;
        }
        self.now_ms = Some(now_ms);
        true
    }

    #[must_use]
    pub fn has_ticking_elapsed(&self) -> bool {
        self.snapshot.state == WorkflowState::Running
    }

    #[must_use]
    pub fn render_with_theme(&self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        let brand = Style::default().fg(theme.brand);
        let primary = Style::default().fg(theme.text_primary);
        let muted = Style::default().fg(theme.text_muted);
        let mut lines = Vec::new();

        let status_label = match self.snapshot.state {
            WorkflowState::Running => "running",
            WorkflowState::Completed => "completed",
            WorkflowState::Failed => "failed",
            WorkflowState::Paused => "paused",
            WorkflowState::Cancelled => "cancelled",
            WorkflowState::ResourceLimited => "resource limited",
        };

        lines.push(
            Line::from_spans(vec![
                Span::styled("\u{25b8} Workflow  ", brand),
                Span::styled(self.snapshot.title.as_str(), primary),
                Span::raw("  "),
                Span::styled(
                    status_label,
                    workflow_state_style(self.snapshot.state, theme),
                ),
            ])
            .truncate_to_width(width),
        );
        lines.push(Line::styled(
            "\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}",
            brand,
        ));

        let elapsed_ms = self.snapshot.started_at_ms.map(|started| {
            let end = if self.snapshot.state == WorkflowState::Running {
                self.now_ms.or(self.snapshot.updated_at_ms)
            } else {
                self.snapshot.updated_at_ms
            }
            .unwrap_or(started);
            end.saturating_sub(started)
        });
        let mut stats = Vec::new();
        if let Some(phase) = self.snapshot.current_phase.as_deref() {
            stats.push(format!("phase {phase}"));
        }
        if let Some(elapsed_ms) = elapsed_ms {
            stats.push(format_elapsed(elapsed_ms / 1_000));
        }
        stats.push(format!("{} invocations", self.snapshot.invocation_count));
        if self.snapshot.failure_count > 0 {
            stats.push(format!("{} failures", self.snapshot.failure_count));
        }
        if let Some(usage) = self.snapshot.actual_usage {
            let total = u64::from(usage.input_tokens) + u64::from(usage.output_tokens);
            stats.push(format!("{total} tokens"));
        }
        lines
            .push(Line::styled(format!("  {}", stats.join(" · ")), muted).truncate_to_width(width));

        for (label, summary) in [
            ("Log", self.snapshot.latest_log_summary.as_deref()),
            ("Report", self.snapshot.latest_report_summary.as_deref()),
            ("Reason", self.snapshot.terminal_reason.as_deref()),
        ] {
            if let Some(summary) = summary {
                lines.push(
                    Line::from_spans(vec![
                        Span::styled(format!("  {label}  "), muted),
                        Span::styled(summary, primary),
                    ])
                    .truncate_to_width(width),
                );
            }
        }
        if let Some(controls) = workflow_controls(self.snapshot.state) {
            lines.push(
                Line::styled(format!("  Controls  {controls}"), muted).truncate_to_width(width),
            );
        }

        lines
    }
}

fn workflow_state_style(state: WorkflowState, theme: &TuiTheme) -> Style {
    Style::default().fg(workflow_state_color(state, theme))
}

fn workflow_state_color(state: WorkflowState, theme: &TuiTheme) -> Color {
    match state {
        WorkflowState::Completed => theme.status_ok,
        WorkflowState::Failed => theme.status_error,
        WorkflowState::Running => theme.status_warn,
        WorkflowState::Paused => theme.status_warn,
        WorkflowState::Cancelled => theme.status_error,
        WorkflowState::ResourceLimited => theme.status_error,
    }
}

fn workflow_controls(state: WorkflowState) -> Option<&'static str> {
    match state {
        WorkflowState::Running => Some("TaskPause · TaskStop"),
        WorkflowState::Paused => Some("TaskResume · TaskStop"),
        WorkflowState::Completed
        | WorkflowState::Failed
        | WorkflowState::Cancelled
        | WorkflowState::ResourceLimited => None,
    }
}

impl Component for WorkflowCardComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.render_with_theme(width, &TuiTheme::default())
    }

    fn finalization(&self) -> Finalization {
        match self.snapshot.state {
            WorkflowState::Running | WorkflowState::Paused => Finalization::Live,
            WorkflowState::Completed
            | WorkflowState::Failed
            | WorkflowState::Cancelled
            | WorkflowState::ResourceLimited => Finalization::Finalized,
        }
    }
}
