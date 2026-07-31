use crate::primitive::theme::TuiTheme;
use crate::primitive::{Color, Component, Finalization, Line, Span, Style};
use crate::shell::ToolStatusKind;
use crate::transcript::format_elapsed;
use crate::transcript::tool_renderers::tool_header_spans_with_elapsed;
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
        self.workflow_elapsed_ticks()
            || self.delegates.iter().any(|agent| {
                agent.state == neo_agent_core::multi_agent::AgentLifecycleState::Running
            })
            || self.swarms.iter().any(|swarm| {
                swarm.children.iter().any(|child| {
                    child.agent.state == neo_agent_core::multi_agent::AgentLifecycleState::Running
                })
            })
    }

    #[must_use]
    pub fn render_with_theme(&self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        super::workflow_group::render_workflow_group(self, width, usize::MAX, theme).into_lines()
    }

    /// Render the same complete workflow group used by explicit review.
    #[must_use]
    pub fn terminal_summary(&self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        self.render_with_theme(width, theme)
    }

    #[must_use]
    pub(crate) const fn now_ms(&self) -> Option<u64> {
        self.now_ms
    }

    #[must_use]
    pub(crate) fn render_main_with_theme(
        &self,
        width: usize,
        max_rows: usize,
        folded_child_counts: Option<&str>,
        theme: &TuiTheme,
    ) -> (Vec<Line>, bool) {
        if max_rows == 0 {
            return (Vec::new(), false);
        }
        let compact = max_rows == 1;
        let mut lines = vec![self.header_line(width, compact, folded_child_counts, theme)];
        if compact {
            return (lines, false);
        }

        let stats_line = self.stats_line(width, theme);
        let action_line = self.actionable_line(width, theme);
        let tool_indexes = self.direct_tool_indexes();
        let first_actionable_tool = tool_indexes.iter().copied().find(|index| {
            matches!(
                self.direct_tools[*index].status(),
                ToolStatusKind::Pending
                    | ToolStatusKind::Queued
                    | ToolStatusKind::Running
                    | ToolStatusKind::Failed
            )
        });
        let mut selected_tools = Vec::new();
        lines.push(stats_line.clone());
        if let Some(action_line) = action_line {
            lines.push(action_line);
        } else if let Some(index) = first_actionable_tool {
            lines.push(self.direct_tool_line(index, width, theme));
            selected_tools.push(index);
        }

        let report_line = self
            .snapshot
            .latest_report_summary
            .as_deref()
            .map(|report| self.summary_line("Report", report, width, theme));
        let remaining = max_rows.saturating_sub(lines.len());
        let reserve_report = usize::from(report_line.is_some() && remaining > 0);
        let unselected_tools = tool_indexes.len().saturating_sub(selected_tools.len());
        let tool_room_before_omission = remaining.saturating_sub(reserve_report);
        let reserve_omission = usize::from(unselected_tools > tool_room_before_omission);
        let tool_slots = tool_room_before_omission.saturating_sub(reserve_omission);
        let remaining_tool_indexes = tool_indexes
            .iter()
            .copied()
            .filter(|index| !selected_tools.contains(index))
            .take(tool_slots)
            .collect::<Vec<_>>();
        for index in remaining_tool_indexes {
            lines.push(self.direct_tool_line(index, width, theme));
            selected_tools.push(index);
        }
        if let Some(report_line) = report_line
            && lines.len() < max_rows
        {
            lines.push(report_line);
        }
        let omitted = self.direct_tools.len().saturating_sub(selected_tools.len());
        if omitted > 0 && lines.len() < max_rows {
            lines.push(
                Line::styled(
                    format!("│ … {omitted} direct tools omitted"),
                    Style::default().fg(theme.text_muted),
                )
                .truncate_to_width(width),
            );
        }
        if let Some(log) = self.snapshot.latest_log_summary.as_deref()
            && lines.len() < max_rows
        {
            lines.push(self.summary_line("Log", log, width, theme));
        }
        lines.truncate(max_rows);
        let has_visible_animation = self.workflow_elapsed_ticks()
            && lines.iter().any(|line| {
                self.elapsed_ms()
                    .is_some_and(|elapsed| line.text().contains(&format_elapsed(elapsed / 1_000)))
            });
        (lines, has_visible_animation)
    }

    fn header_line(
        &self,
        width: usize,
        compact: bool,
        folded_child_counts: Option<&str>,
        theme: &TuiTheme,
    ) -> Line {
        let mut spans = vec![
            Span::styled("▸ Workflow  ", Style::default().fg(theme.brand)),
            Span::styled(
                self.snapshot.title.as_str(),
                Style::default().fg(theme.text_primary),
            ),
            Span::styled(
                format!(" · {}", workflow_state_label(self.snapshot.state)),
                workflow_state_style(self.snapshot.state, theme),
            ),
        ];
        if compact {
            spans.push(Span::styled(
                format!(" · {} calls", self.snapshot.invocation_count),
                Style::default().fg(theme.text_muted),
            ));
            if self.snapshot.failure_count > 0 {
                spans.push(Span::styled(
                    format!(" · {} failed", self.snapshot.failure_count),
                    Style::default().fg(theme.status_error),
                ));
            }
        }
        if let Some(counts) = folded_child_counts {
            spans.push(Span::styled(
                format!(" · {counts}"),
                Style::default().fg(theme.text_muted),
            ));
        }
        Line::from_spans(spans).truncate_to_width(width)
    }

    fn stats_line(&self, width: usize, theme: &TuiTheme) -> Line {
        let mut stats = Vec::new();
        if let Some(phase) = self.snapshot.current_phase.as_deref() {
            stats.push(format!("phase {phase}"));
        }
        if let Some(elapsed_ms) = self.elapsed_ms() {
            stats.push(format_elapsed(elapsed_ms / 1_000));
        }
        stats.push(format!("{} invocations", self.snapshot.invocation_count));
        stats.push(format!("{} failed", self.snapshot.failure_count));
        if let Some(usage) = self.snapshot.actual_usage {
            let total = u64::from(usage.input_tokens) + u64::from(usage.output_tokens);
            stats.push(format!("{total} tokens"));
        }
        Line::styled(
            format!("│ {}", stats.join(" · ")),
            Style::default().fg(theme.text_muted),
        )
        .truncate_to_width(width)
    }

    fn actionable_line(&self, width: usize, theme: &TuiTheme) -> Option<Line> {
        let text = match self.snapshot.state {
            WorkflowState::Running => workflow_controls(self.snapshot.state)?.to_owned(),
            WorkflowState::Queued => format!(
                "waiting for a worker permit · {}",
                workflow_controls(self.snapshot.state)?
            ),
            WorkflowState::Pausing => format!(
                "current work is finishing · {}",
                workflow_controls(self.snapshot.state)?
            ),
            WorkflowState::AwaitingUser => format!(
                "awaiting user input · {}",
                workflow_controls(self.snapshot.state)?
            ),
            WorkflowState::Paused => format!(
                "paused at an invocation boundary · {}",
                workflow_controls(self.snapshot.state)?
            ),
            WorkflowState::Completed => self.snapshot.terminal_reason.clone()?,
            WorkflowState::Failed => self
                .snapshot
                .terminal_reason
                .clone()
                .unwrap_or_else(|| "workflow failed".to_owned()),
            WorkflowState::Cancelled => self
                .snapshot
                .terminal_reason
                .clone()
                .unwrap_or_else(|| "workflow was cancelled".to_owned()),
            WorkflowState::ResourceLimited => self
                .snapshot
                .terminal_reason
                .clone()
                .unwrap_or_else(|| "execution stopped at the machine-safety limit".to_owned()),
        };
        let label = if self.snapshot.state.is_terminal() {
            "Reason"
        } else {
            "Action"
        };
        Some(self.summary_line(label, &text, width, theme))
    }

    fn summary_line(&self, label: &str, summary: &str, width: usize, theme: &TuiTheme) -> Line {
        Line::from_spans(vec![
            Span::styled(
                format!("│ {label}  "),
                Style::default().fg(theme.text_muted),
            ),
            Span::styled(summary, Style::default().fg(theme.text_primary)),
        ])
        .truncate_to_width(width)
    }

    fn direct_tool_indexes(&self) -> Vec<usize> {
        let mut indexes = Vec::with_capacity(self.direct_tools.len());
        for statuses in [
            &[
                ToolStatusKind::Pending,
                ToolStatusKind::Queued,
                ToolStatusKind::Running,
            ][..],
            &[ToolStatusKind::Failed][..],
        ] {
            indexes.extend(
                self.direct_tools
                    .iter()
                    .enumerate()
                    .filter_map(|(index, tool)| statuses.contains(&tool.status()).then_some(index)),
            );
        }
        indexes.extend(
            self.direct_tools
                .iter()
                .enumerate()
                .rev()
                .filter_map(|(index, tool)| {
                    matches!(
                        tool.status(),
                        ToolStatusKind::Succeeded | ToolStatusKind::Cancelled
                    )
                    .then_some(index)
                }),
        );
        indexes
    }

    fn direct_tool_line(&self, index: usize, width: usize, theme: &TuiTheme) -> Line {
        let inner_width = width.saturating_sub(2);
        let mut spans = vec![Span::styled("│ ", Style::default().fg(theme.text_muted))];
        spans.extend(tool_header_spans_with_elapsed(
            self.direct_tools[index].state(),
            theme,
            None,
            inner_width,
            None,
        ));
        Line::from_spans(spans).truncate_to_width(width)
    }

    fn elapsed_ms(&self) -> Option<u64> {
        self.snapshot.started_at_ms.map(|started| {
            let end = if self.workflow_elapsed_ticks() {
                self.now_ms.or(self.snapshot.updated_at_ms)
            } else {
                self.snapshot.updated_at_ms
            }
            .unwrap_or(started);
            end.saturating_sub(started)
        })
    }

    fn workflow_elapsed_ticks(&self) -> bool {
        matches!(
            self.snapshot.state,
            WorkflowState::Running
                | WorkflowState::Queued
                | WorkflowState::Pausing
                | WorkflowState::AwaitingUser
        )
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
