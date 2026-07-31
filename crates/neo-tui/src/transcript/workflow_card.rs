use crate::primitive::theme::TuiTheme;
use crate::primitive::{Color, Component, Finalization, Line, Span, Style};
use crate::transcript::format_elapsed;
use crate::transcript::{ToolCallComponent, ToolCallState};
use neo_agent_core::multi_agent::{
    AgentProgressSnapshot, AgentSnapshot, SwarmAggregate, SwarmChildProgress, SwarmSnapshot,
    apply_agent_progress, apply_swarm_child_progress,
};
use neo_agent_core::workflow::{WorkflowExecutionOrigin, WorkflowSnapshot, WorkflowState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowCardComponent {
    snapshot: WorkflowSnapshot,
    max_projection_sequence: Option<u64>,
    now_ms: Option<u64>,
    direct_tools: Vec<ToolCallComponent>,
    delegates: Vec<AgentSnapshot>,
    swarms: Vec<SwarmSnapshot>,
}

impl WorkflowCardComponent {
    #[must_use]
    pub fn new(snapshot: WorkflowSnapshot) -> Self {
        let max_projection_sequence = snapshot.projection_sequence;
        Self {
            snapshot,
            max_projection_sequence,
            now_ms: None,
            direct_tools: Vec::new(),
            delegates: Vec::new(),
            swarms: Vec::new(),
        }
    }

    pub(crate) fn upsert_direct_tool(
        &mut self,
        state: ToolCallState,
        workflow_origin: WorkflowExecutionOrigin,
    ) -> Result<bool, ()> {
        if let Some(tool) = self
            .direct_tools
            .iter_mut()
            .find(|tool| tool.id() == state.id)
        {
            if !tool.accepts_workflow_origin(&workflow_origin) {
                return Err(());
            }
            let attached = tool.attach_workflow_origin(workflow_origin);
            return Ok(attached | tool.update_call_state(state.name, state.arguments, state.status));
        }
        let mut tool = ToolCallComponent::new(state);
        tool.attach_workflow_origin(workflow_origin);
        self.direct_tools.push(tool);
        Ok(true)
    }

    pub(crate) fn mutate_direct_tool(
        &mut self,
        id: &str,
        mutate: impl FnOnce(&mut ToolCallComponent) -> bool,
    ) -> bool {
        self.direct_tools
            .iter_mut()
            .find(|tool| tool.id() == id)
            .is_some_and(mutate)
    }

    pub(crate) fn absorb_direct_tool(
        &mut self,
        workflow_origin: &WorkflowExecutionOrigin,
    ) -> Result<bool, ()> {
        self.validate_direct_tool_origin(workflow_origin)?;
        let Some(invocation_id) = workflow_origin.invocation_id.as_deref() else {
            return Ok(false);
        };
        let Some(index) = self
            .direct_tools
            .iter()
            .position(|tool| tool.id() == invocation_id)
        else {
            return Ok(false);
        };
        self.direct_tools.remove(index);
        Ok(true)
    }

    pub(crate) fn validate_direct_tool_origin(
        &self,
        workflow_origin: &WorkflowExecutionOrigin,
    ) -> Result<(), ()> {
        let Some(invocation_id) = workflow_origin.invocation_id.as_deref() else {
            return Ok(());
        };
        let Some(tool) = self
            .direct_tools
            .iter()
            .find(|tool| tool.id() == invocation_id)
        else {
            return Ok(());
        };
        tool.accepts_workflow_origin(workflow_origin)
            .then_some(())
            .ok_or(())
    }

    pub(crate) fn upsert_delegate(&mut self, snapshot: AgentSnapshot) -> bool {
        if let Some(current) = self
            .delegates
            .iter_mut()
            .find(|current| current.id == snapshot.id)
        {
            let merged = super::store::merge_delegate_snapshot(current, snapshot);
            if *current == merged {
                return false;
            }
            *current = merged;
            return true;
        }
        self.delegates.push(snapshot);
        true
    }

    pub(crate) fn upsert_delegate_progress(&mut self, progress: &AgentProgressSnapshot) -> bool {
        let Some(snapshot) = self.delegates.iter_mut().find(|snapshot| {
            snapshot.id == progress.agent_id && snapshot.run_count == progress.run_count
        }) else {
            return false;
        };
        if snapshot.state.is_terminal() && !progress.state.is_terminal() {
            return false;
        }
        let previous = snapshot.clone();
        apply_agent_progress(snapshot, progress) && *snapshot != previous
    }

    pub(crate) fn upsert_swarm(&mut self, snapshot: SwarmSnapshot) -> bool {
        if let Some(current) = self
            .swarms
            .iter_mut()
            .find(|current| current.swarm_id == snapshot.swarm_id)
        {
            let merged = super::store::merge_swarm_snapshot(current, snapshot);
            if *current == merged {
                return false;
            }
            *current = merged;
            return true;
        }
        self.swarms.push(snapshot);
        true
    }

    pub(crate) fn upsert_swarm_progress(
        &mut self,
        swarm_id: &str,
        state: neo_agent_core::multi_agent::AgentLifecycleState,
        aggregate: SwarmAggregate,
        child_progress: &SwarmChildProgress,
    ) -> bool {
        let Some(snapshot) = self
            .swarms
            .iter_mut()
            .find(|snapshot| snapshot.swarm_id == swarm_id)
        else {
            return false;
        };
        if super::store::swarm_snapshot_is_terminal(snapshot) {
            return false;
        }
        let previous = snapshot.clone();
        apply_swarm_child_progress(snapshot, child_progress, aggregate, state);
        *snapshot != previous
    }

    #[must_use]
    pub fn direct_tools(&self) -> &[ToolCallComponent] {
        &self.direct_tools
    }

    #[must_use]
    pub fn delegates(&self) -> &[AgentSnapshot] {
        &self.delegates
    }

    #[must_use]
    pub fn swarms(&self) -> &[SwarmSnapshot] {
        &self.swarms
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
        if self.snapshot.state.is_terminal() {
            return false;
        }
        // Every non-terminal workflow state (running, queued, pausing,
        // awaiting user, paused) converges to one terminal presentation on
        // interrupt or exit.
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

        let status_label = workflow_state_label(self.snapshot.state);

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
            let end = if matches!(
                self.snapshot.state,
                WorkflowState::Running
                    | WorkflowState::Queued
                    | WorkflowState::Pausing
                    | WorkflowState::AwaitingUser
            ) {
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

    /// One terminal status for native history once progressive transition
    /// facts were emitted. The full card remains available through explicit
    /// review.
    #[must_use]
    pub fn terminal_summary(&self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        let brand = Style::default().fg(theme.brand);
        let primary = Style::default().fg(theme.text_primary);
        let muted = Style::default().fg(theme.text_muted);
        let mut lines = vec![
            Line::from_spans(vec![
                Span::styled("\u{25b8} Workflow  ", brand),
                Span::styled(self.snapshot.title.as_str(), primary),
                Span::raw("  "),
                Span::styled(
                    workflow_state_label(self.snapshot.state),
                    workflow_state_style(self.snapshot.state, theme),
                ),
            ])
            .truncate_to_width(width),
        ];
        if let Some(reason) = self.snapshot.terminal_reason.as_deref() {
            lines.push(Line::styled(format!("  Reason  {reason}"), muted).truncate_to_width(width));
        }
        lines
    }
}

#[must_use]
pub(super) fn workflow_state_label(state: WorkflowState) -> &'static str {
    match state {
        WorkflowState::Running => "running",
        WorkflowState::Queued => "queued",
        WorkflowState::Pausing => "finishing work",
        WorkflowState::AwaitingUser => "awaiting user",
        WorkflowState::Completed => "completed",
        WorkflowState::Failed => "failed",
        WorkflowState::Paused => "paused",
        WorkflowState::Cancelled => "cancelled",
        WorkflowState::ResourceLimited => "resource limited",
    }
}

fn workflow_state_style(state: WorkflowState, theme: &TuiTheme) -> Style {
    Style::default().fg(workflow_state_color(state, theme))
}

fn workflow_state_color(state: WorkflowState, theme: &TuiTheme) -> Color {
    match state {
        WorkflowState::Completed => theme.status_ok,
        WorkflowState::Failed => theme.status_error,
        WorkflowState::Running
        | WorkflowState::Queued
        | WorkflowState::Pausing
        | WorkflowState::AwaitingUser => theme.status_warn,
        WorkflowState::Paused => theme.status_warn,
        WorkflowState::Cancelled => theme.status_error,
        WorkflowState::ResourceLimited => theme.status_error,
    }
}

fn workflow_controls(state: WorkflowState) -> Option<&'static str> {
    match state {
        WorkflowState::Running | WorkflowState::Queued | WorkflowState::Pausing => {
            Some("TaskPause · TaskStop")
        }
        WorkflowState::Paused | WorkflowState::AwaitingUser => Some("TaskResume · TaskStop"),
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
            WorkflowState::Running
            | WorkflowState::Queued
            | WorkflowState::Pausing
            | WorkflowState::AwaitingUser
            | WorkflowState::Paused => Finalization::Live,
            WorkflowState::Completed
            | WorkflowState::Failed
            | WorkflowState::Cancelled
            | WorkflowState::ResourceLimited => Finalization::Finalized,
        }
    }
}
