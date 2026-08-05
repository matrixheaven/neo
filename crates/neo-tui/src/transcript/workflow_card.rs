use crate::primitive::theme::TuiTheme;
use crate::primitive::{Color, Component, Expandable, Finalization, Line, Span, Style};
use crate::shell::ToolStatusKind;
use crate::transcript::format_elapsed;
use crate::transcript::tool_renderers::tool_header_spans_with_elapsed;
use crate::transcript::{ToolCallComponent, ToolCallState};
use neo_agent_core::multi_agent::{
    AgentProgressSnapshot, AgentSnapshot, SwarmAggregate, SwarmChildProgress, SwarmSnapshot,
    apply_agent_progress, apply_swarm_child_progress,
};
use neo_agent_core::session::ToolOutputStore;
use neo_agent_core::workflow::{WorkflowExecutionOrigin, WorkflowSnapshot, WorkflowState};

use super::store::ExpandedOutputCache;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowCardComponent {
    snapshot: WorkflowSnapshot,
    max_projection_sequence: Option<u64>,
    now_ms: Option<u64>,
    direct_tools: Vec<ToolCallComponent>,
    delegates: Vec<AgentSnapshot>,
    swarms: Vec<SwarmSnapshot>,
    /// Typed tool ID of the one expanded direct tool. Entry-local view state:
    /// toggled by typed tool ID, never persisted, one expansion at a time.
    expanded_tool_id: Option<String>,
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
            expanded_tool_id: None,
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

    /// Toggle inline expansion for one direct tool by its typed tool ID.
    ///
    /// Expanding a tool collapses any previously expanded one; toggling the
    /// same ID again restores the one-line row.
    pub(crate) fn toggle_direct_tool_expansion(&mut self, tool_id: &str) -> bool {
        if !self.direct_tools.iter().any(|tool| tool.id() == tool_id) {
            return false;
        }
        self.expanded_tool_id = if self.expanded_tool_id.as_deref() == Some(tool_id) {
            None
        } else {
            Some(tool_id.to_owned())
        };
        true
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
        let mut cache = ExpandedOutputCache::default();
        super::workflow_group::render_workflow_group(self, width, theme, None, 0, &mut cache)
            .into_lines()
    }

    /// Store-aware group render: expanded direct tools read their bounded
    /// visible complete-output range through `output_store`, with the derived
    /// wrap rows cached per width in `output_cache`.
    #[must_use]
    pub(crate) fn render_with_output(
        &self,
        width: usize,
        theme: &TuiTheme,
        output_store: Option<&ToolOutputStore>,
        viewport_rows: usize,
        output_cache: &mut ExpandedOutputCache,
    ) -> Vec<Line> {
        super::workflow_group::render_workflow_group(
            self,
            width,
            theme,
            output_store,
            viewport_rows,
            output_cache,
        )
        .into_lines()
    }

    #[must_use]
    pub(crate) const fn now_ms(&self) -> Option<u64> {
        self.now_ms
    }

    /// Render every structural row of the main card: header, state action
    /// lines, every direct tool (one line each; the expanded tool renders its
    /// command/arguments/details and visible complete-output range inline),
    /// the report, stats, running action lines, and the log line. No
    /// terminal-height budget applies.
    #[must_use]
    pub(crate) fn render_main_with_theme(
        &self,
        width: usize,
        theme: &TuiTheme,
        output_store: Option<&ToolOutputStore>,
        viewport_rows: usize,
        output_cache: &mut ExpandedOutputCache,
    ) -> Vec<Line> {
        let mut lines = vec![self.header_line(width, theme)];

        let action_lines = self.actionable_lines(width, theme);
        let (state_action_lines, running_action_lines) =
            if self.snapshot.state == WorkflowState::Running {
                (None, action_lines)
            } else {
                (action_lines, None)
            };
        if let Some(action_lines) = state_action_lines {
            lines.extend(action_lines);
        }

        let (actionable_tool_indexes, completed_tool_indexes): (Vec<_>, Vec<_>) =
            self.direct_tool_indexes().into_iter().partition(|index| {
                matches!(
                    self.direct_tools[*index].status(),
                    ToolStatusKind::Pending
                        | ToolStatusKind::Queued
                        | ToolStatusKind::Running
                        | ToolStatusKind::Failed
                )
            });

        for index in actionable_tool_indexes
            .iter()
            .copied()
            .chain(completed_tool_indexes)
        {
            lines.push(self.direct_tool_line(index, width, theme));
            if self.expanded_tool_id.as_deref() == Some(self.direct_tools[index].id()) {
                lines.extend(self.expanded_direct_tool_rows(
                    index,
                    width,
                    theme,
                    output_store,
                    viewport_rows,
                    output_cache,
                ));
            }
        }

        if let Some(report) = self.snapshot.latest_report_summary.as_deref() {
            lines.push(self.summary_line("Report", report, width, theme));
        }
        lines.push(self.stats_line(width, theme));
        if let Some(action_lines) = running_action_lines {
            lines.extend(action_lines);
        }
        if let Some(log) = self.snapshot.latest_log_summary.as_deref() {
            lines.push(self.summary_line("Log", log, width, theme));
        }
        lines
    }

    /// Inline expansion rows for one direct tool: the tool's own
    /// `ToolCallComponent` body (command/arguments, details, bounded live
    /// preview) immediately beneath its one-line row, followed by the visible
    /// complete-output range when the session output store is available.
    fn expanded_direct_tool_rows(
        &self,
        index: usize,
        width: usize,
        theme: &TuiTheme,
        output_store: Option<&ToolOutputStore>,
        viewport_rows: usize,
        output_cache: &mut ExpandedOutputCache,
    ) -> Vec<Line> {
        let mut tool = self.direct_tools[index].clone();
        tool.set_expanded(true);
        let mut rows = tool.render_with_theme(width, theme);
        if rows.len() > 1 {
            // The one-line row above already shows the header; drop the
            // duplicated header row from the ToolCallComponent render.
            rows.remove(0);
        } else {
            rows.clear();
        }
        if let Some(store) = output_store {
            rows.extend(tool.render_complete_output_range(
                width,
                theme,
                store,
                output_cache,
                u64::try_from(viewport_rows).unwrap_or(u64::MAX),
            ));
        }
        rows
    }

    fn header_line(&self, width: usize, theme: &TuiTheme) -> Line {
        let spans = vec![
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

    fn actionable_lines(&self, width: usize, theme: &TuiTheme) -> Option<Vec<Line>> {
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
        Some(self.summary_lines(label, &text, width, theme))
    }

    fn summary_line(&self, label: &str, summary: &str, width: usize, theme: &TuiTheme) -> Line {
        self.summary_lines(label, summary, width, theme)
            .into_iter()
            .next()
            .unwrap_or_else(|| Line::styled("", Style::default().fg(theme.text_primary)))
    }

    fn summary_lines(
        &self,
        label: &str,
        summary: &str,
        width: usize,
        theme: &TuiTheme,
    ) -> Vec<Line> {
        let continuation_prefix = format!("│ {:width$}", "", width = label.len() + 2);
        summary
            .split('\n')
            .enumerate()
            .map(|(index, summary)| {
                let prefix = if index == 0 {
                    format!("│ {label}  ")
                } else {
                    continuation_prefix.clone()
                };
                Line::from_spans(vec![
                    Span::styled(prefix, Style::default().fg(theme.text_muted)),
                    Span::styled(
                        summary.trim_end_matches('\r'),
                        Style::default().fg(theme.text_primary),
                    ),
                ])
                .truncate_to_width(width)
            })
            .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use neo_agent_core::workflow::WorkflowId;

    fn snapshot(state: WorkflowState) -> WorkflowSnapshot {
        WorkflowSnapshot {
            id: WorkflowId("wf-priority".to_owned()),
            title: "Priority workflow".to_owned(),
            state,
            current_phase: Some("verify".to_owned()),
            projection_sequence: Some(1),
            recovery_failure: false,
            started_at_ms: Some(1_000),
            updated_at_ms: Some(2_000),
            invocation_count: 4,
            failure_count: 1,
            actual_usage: None,
            latest_log_summary: Some("latest log".to_owned()),
            latest_report_summary: Some("latest report".to_owned()),
            terminal_reason: state
                .is_terminal()
                .then(|| "bounded terminal reason".to_owned()),
            display_name: "Priority workflow".to_owned(),
            purpose: "Verify compact row priority".to_owned(),
        }
    }

    fn tool(id: &str, name: &str, status: ToolStatusKind) -> ToolCallComponent {
        ToolCallComponent::new(ToolCallState {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments: None,
            result: None,
            details: None,
            status,
            exit_code: None,
        })
    }

    fn card(state: WorkflowState) -> WorkflowCardComponent {
        let mut card = WorkflowCardComponent::new(snapshot(state));
        card.direct_tools = vec![
            tool("running", "RunningTool", ToolStatusKind::Running),
            tool("queued", "QueuedTool", ToolStatusKind::Queued),
            tool("failed", "FailedTool", ToolStatusKind::Failed),
            tool("completed", "CompletedTool", ToolStatusKind::Succeeded),
        ];
        card
    }

    fn render_text(card: &WorkflowCardComponent) -> String {
        let mut cache = ExpandedOutputCache::default();
        card.render_main_with_theme(120, &TuiTheme::default(), None, 0, &mut cache)
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn multiline_reason_keeps_continuation_fields_on_the_card_column() {
        let mut failed = card(WorkflowState::Failed);
        failed.snapshot.terminal_reason = Some(
            "workflow failed: agent_id: agent-1\nname: Archimedes\nstatus: failed\nrun_index: 1\nsummary_scope: current_run\ncontext_mode: inherit"
                .to_owned(),
        );

        let rendered = render_text(&failed);
        let lines = rendered.lines().collect::<Vec<_>>();
        assert_eq!(lines[1], "│ Reason  workflow failed: agent_id: agent-1");
        assert_eq!(lines[2], "│         name: Archimedes");
        assert_eq!(lines[3], "│         status: failed");
        assert_eq!(lines[4], "│         run_index: 1");
        assert_eq!(lines[5], "│         summary_scope: current_run");
        assert_eq!(lines[6], "│         context_mode: inherit");
        assert_eq!(lines[7], "│ ● Using RunningTool", "{rendered}");
        assert!(
            rendered.contains("phase verify") && rendered.contains("latest report"),
            "{rendered}"
        );
    }
}
