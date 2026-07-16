use crate::primitive::theme::TuiTheme;
use crate::primitive::{Color, Component, Finalization, Line, Span, Style};
use neo_agent_core::workflow::{WorkflowSnapshot, WorkflowState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowCardComponent {
    snapshot: WorkflowSnapshot,
}

impl WorkflowCardComponent {
    #[must_use]
    pub fn new(snapshot: WorkflowSnapshot) -> Self {
        Self { snapshot }
    }

    pub fn update(&mut self, snapshot: WorkflowSnapshot) -> bool {
        if self.snapshot == snapshot {
            return false;
        }
        self.snapshot = snapshot;
        true
    }

    pub fn interrupt(&mut self) -> bool {
        if self.snapshot.state != WorkflowState::Running {
            return false;
        }
        self.snapshot.state = WorkflowState::Failed;
        for step in &mut self.snapshot.steps {
            if step.state == WorkflowState::Running {
                step.state = WorkflowState::Failed;
            }
        }
        true
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.snapshot.id.0
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

        for step in &self.snapshot.steps {
            let marker = match step.state {
                WorkflowState::Completed => "\u{2713}",
                WorkflowState::Failed => "\u{2717}",
                WorkflowState::Running => "\u{25cf}",
            };
            lines.push(
                Line::from_spans(vec![
                    Span::raw("  "),
                    Span::styled(marker, workflow_state_style(step.state, theme)),
                    Span::raw(" "),
                    Span::styled(step.name.as_str(), primary),
                ])
                .truncate_to_width(width),
            );
            if let Some(summary) = &step.summary {
                lines.push(
                    Line::styled(format!("    \u{2514} {summary}"), muted).truncate_to_width(width),
                );
            }
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
    }
}

impl Component for WorkflowCardComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.render_with_theme(width, &TuiTheme::default())
    }

    fn finalization(&self) -> Finalization {
        match self.snapshot.state {
            WorkflowState::Completed | WorkflowState::Failed => Finalization::Finalized,
            WorkflowState::Running => Finalization::Live,
        }
    }
}
