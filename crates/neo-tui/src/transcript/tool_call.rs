use crate::primitive::Style;
use crate::primitive::theme::TuiTheme;
use crate::primitive::wrap_width;
use crate::primitive::{Component, Expandable, Finalization, Line, Span, strip_ansi};
use crate::shell::ToolStatusKind;
use crate::token_estimate::format_elapsed;

use super::plan_box::PlanBoxComponent;
use super::tool_renderers::{
    is_file_write_tool, is_pending_or_running, render_streaming_preview, render_tool_body_themed,
    tool_header_spans_with_elapsed,
};

use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallState {
    pub id: String,
    pub name: String,
    pub arguments: Option<String>,
    pub result: Option<String>,
    pub details: Option<serde_json::Value>,
    pub status: ToolStatusKind,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QueueDisplayState {
    position: usize,
    waiting_ms: u64,
    observed_at: Instant,
}

impl QueueDisplayState {
    fn elapsed_ms(&self) -> u64 {
        self.waiting_ms.saturating_add(
            u64::try_from(self.observed_at.elapsed().as_millis()).unwrap_or(u64::MAX),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallComponent {
    state: ToolCallState,
    expanded: bool,
    live_output: Vec<String>,
    dropped_live_output_lines: usize,
    live_output_chars: usize,
    workspace_dir: Option<PathBuf>,
    streaming_started_at: Option<Instant>,
    queue: Option<QueueDisplayState>,
}

const MAX_LIVE_OUTPUT_LINES: usize = 6;
const MAX_LIVE_OUTPUT_CHARS: usize = 50_000;

impl ToolCallComponent {
    #[must_use]
    pub fn new(state: ToolCallState) -> Self {
        let streaming_started_at =
            matches!(state.status, ToolStatusKind::Running).then(Instant::now);
        Self {
            state,
            expanded: false,
            live_output: Vec::new(),
            dropped_live_output_lines: 0,
            live_output_chars: 0,
            workspace_dir: None,
            streaming_started_at,
            queue: None,
        }
    }

    pub fn update_call(&mut self, arguments: Option<String>) -> bool {
        let mut changed = self.state.arguments != arguments;
        if let Some(args) = &arguments
            && !args.is_empty()
            && self.state.name != "Sleep"
            && self.streaming_started_at.is_none()
        {
            self.streaming_started_at = Some(Instant::now());
            changed = true;
        }
        if !changed {
            return false;
        }
        self.state.arguments = arguments;
        true
    }

    pub fn update_call_state(
        &mut self,
        name: String,
        arguments: Option<String>,
        status: ToolStatusKind,
    ) -> bool {
        let mut changed = self.state.name != name || self.state.status != status;
        if arguments.is_some() && self.state.arguments != arguments {
            changed = true;
        }
        if status == ToolStatusKind::Running && self.streaming_started_at.is_none() {
            changed = true;
        }
        if status != ToolStatusKind::Queued && self.queue.is_some() {
            changed = true;
        }
        if !changed {
            return false;
        }
        self.state.name = name;
        if arguments.is_some() {
            self.state.arguments = arguments;
        }
        self.state.status = status;
        if status != ToolStatusKind::Queued {
            self.queue = None;
        }
        if status == ToolStatusKind::Running && self.streaming_started_at.is_none() {
            self.streaming_started_at = Some(Instant::now());
        }
        true
    }

    /// Mark this tool as admission-queued and refresh its live wait baseline.
    ///
    /// Queue updates after the tool has left `Queued` (Started/Finished) are ignored.
    pub fn set_queued(&mut self, position: usize, waiting_ms: u64) -> bool {
        if self.state.status != ToolStatusKind::Queued {
            return false;
        }
        if self
            .queue
            .as_ref()
            .is_some_and(|current| current.position == position && current.waiting_ms == waiting_ms)
        {
            return false;
        }
        self.queue = Some(QueueDisplayState {
            position,
            waiting_ms,
            observed_at: Instant::now(),
        });
        true
    }

    pub fn append_live_output(&mut self, output: impl Into<String>) -> bool {
        let output = output.into();
        if output.is_empty() {
            return false;
        }
        for line in output.lines() {
            self.live_output_chars += line.chars().count();
            self.live_output.push(line.to_owned());
        }
        self.trim_live_output();
        true
    }

    fn trim_live_output(&mut self) {
        while self.live_output.len() > MAX_LIVE_OUTPUT_LINES
            || self.live_output_chars > MAX_LIVE_OUTPUT_CHARS
        {
            let Some(line) = self.live_output.first() else {
                self.live_output_chars = 0;
                break;
            };
            self.live_output_chars = self.live_output_chars.saturating_sub(line.chars().count());
            self.live_output.remove(0);
            self.dropped_live_output_lines += 1;
        }
    }

    pub fn set_result(
        &mut self,
        result: Option<String>,
        details: Option<serde_json::Value>,
        is_error: bool,
        exit_code: Option<i32>,
    ) -> bool {
        let status = if is_error {
            ToolStatusKind::Failed
        } else {
            ToolStatusKind::Succeeded
        };
        let changed = self.state.result != result
            || self.state.details != details
            || self.state.exit_code != exit_code
            || self.state.status != status
            || !self.live_output.is_empty()
            || self.dropped_live_output_lines != 0
            || self.live_output_chars != 0
            || self.streaming_started_at.is_some()
            || self.queue.is_some();
        if !changed {
            return false;
        }
        self.state.result = result;
        self.state.details = details;
        self.state.exit_code = exit_code;
        self.state.status = status;
        self.live_output.clear();
        self.dropped_live_output_lines = 0;
        self.live_output_chars = 0;
        self.streaming_started_at = None;
        self.queue = None;
        true
    }

    pub fn set_terminal_status(&mut self, status: ToolStatusKind, result: Option<String>) -> bool {
        let changed = self.state.result != result
            || self.state.details.is_some()
            || self.state.exit_code.is_some()
            || self.state.status != status
            || !self.live_output.is_empty()
            || self.dropped_live_output_lines != 0
            || self.live_output_chars != 0
            || self.streaming_started_at.is_some()
            || self.queue.is_some();
        if !changed {
            return false;
        }
        self.state.result = result;
        self.state.details = None;
        self.state.exit_code = None;
        self.state.status = status;
        self.live_output.clear();
        self.dropped_live_output_lines = 0;
        self.live_output_chars = 0;
        self.streaming_started_at = None;
        self.queue = None;
        true
    }

    #[must_use]
    pub const fn status(&self) -> ToolStatusKind {
        self.state.status
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.state.id
    }

    /// The tool name (e.g. "Read", "Bash").
    #[must_use]
    pub fn name(&self) -> &str {
        &self.state.name
    }

    #[must_use]
    pub fn arguments(&self) -> Option<&str> {
        self.state.arguments.as_deref()
    }

    pub fn set_workspace_dir(&mut self, workspace_dir: impl Into<PathBuf>) -> bool {
        let workspace_dir = workspace_dir.into();
        if self.workspace_dir.as_ref() == Some(&workspace_dir) {
            return false;
        }
        self.workspace_dir = Some(workspace_dir);
        true
    }

    /// Borrow the underlying tool state (for grouping/rendering snapshots).
    #[must_use]
    pub const fn state(&self) -> &ToolCallState {
        &self.state
    }

    #[must_use]
    pub fn result(&self) -> Option<&str> {
        self.state.result.as_deref()
    }

    #[must_use]
    pub fn has_live_rows(&self) -> bool {
        self.dropped_live_output_lines > 0 || !self.live_output.is_empty()
    }

    #[must_use]
    pub const fn is_expanded(&self) -> bool {
        self.expanded
    }

    #[must_use]
    pub const fn finalization(&self) -> Finalization {
        match self.state.status {
            ToolStatusKind::Succeeded | ToolStatusKind::Failed | ToolStatusKind::Cancelled => {
                Finalization::Finalized
            }
            ToolStatusKind::Pending | ToolStatusKind::Queued | ToolStatusKind::Running => {
                Finalization::Live
            }
        }
    }

    #[must_use]
    pub fn has_visible_animation(&self) -> bool {
        if self.state.status == ToolStatusKind::Queued {
            return true;
        }
        if self.state.name == "Sleep" {
            return self.state.status == ToolStatusKind::Running
                && self.streaming_started_at.is_some();
        }
        is_pending_or_running(self.state.status)
            && (is_file_write_tool(&self.state.name) || self.state.name == "WaitDelegate")
            && self.streaming_started_at.is_some()
    }
}

impl Expandable for ToolCallComponent {
    fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }
}

impl Component for ToolCallComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.render_with_theme(width, &TuiTheme::default())
    }

    fn finalization(&self) -> Finalization {
        match self.state.status {
            ToolStatusKind::Succeeded | ToolStatusKind::Failed | ToolStatusKind::Cancelled => {
                Finalization::Finalized
            }
            ToolStatusKind::Pending | ToolStatusKind::Queued | ToolStatusKind::Running => {
                Finalization::Live
            }
        }
    }
}

impl ToolCallComponent {
    /// Theme-aware render. Builds the header as styled spans (status symbol
    /// + tool name + key arg + chip) and the body as weak preview lines.
    #[must_use]
    pub fn render_with_theme(&mut self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        let header_width = width.saturating_sub(2).max(1);
        let mut header_spans = if self.state.name == "ExitPlanMode" {
            crate::transcript::tool_renderers::exit_plan_mode_header_spans(&self.state, theme)
        } else {
            tool_header_spans_with_elapsed(
                &self.state,
                theme,
                self.workspace_dir.as_deref(),
                header_width,
                self.streaming_started_at
                    .map(|started| started.elapsed().as_secs()),
            )
        };
        // While Write/Edit is streaming, show a token count chip in the header
        // instead of a separate progress line in the body.
        if is_pending_or_running(self.state.status)
            && is_file_write_tool(&self.state.name)
            && let Some(started_at) = self.streaming_started_at
        {
            let tokens = crate::transcript::tool_renderers::estimate_tool_tokens(
                self.state.arguments.as_deref().unwrap_or(""),
            );
            let elapsed = started_at.elapsed().as_secs();
            let chip = format!(
                " · ~{} tok · {}m",
                crate::transcript::tool_renderers::format_tool_token_count(tokens),
                elapsed
            );
            header_spans.push(Span::styled(chip, Style::default().fg(theme.text_muted)));
        }
        if self.state.status == ToolStatusKind::Queued
            && let Some(queue) = &self.queue
        {
            let chip = format!(
                " · #{} · waiting {}",
                queue.position,
                format_elapsed(queue.elapsed_ms() / 1000)
            );
            header_spans.push(Span::styled(chip, Style::default().fg(theme.text_muted)));
        }
        let mut rows = vec![Line::from_spans(header_spans).truncate_to_width(header_width)];

        // For ExitPlanMode, render a PlanBox from the tool result details.
        if self.state.name == "ExitPlanMode"
            && let Some(details) = &self.state.details
            && let Some(plan_content) = details.get("plan_content").and_then(|v| v.as_str())
        {
            let plan_path = details
                .get("plan_path")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string);
            let plan_box = PlanBoxComponent::new(plan_content, plan_path);
            rows.extend(plan_box.render(width, theme));
        }

        if is_pending_or_running(self.state.status) && is_file_write_tool(&self.state.name) {
            rows.extend(render_streaming_preview(
                &self.state,
                self.expanded,
                width,
                theme,
                self.streaming_started_at,
            ));
        } else {
            rows.extend(render_tool_body_themed(
                &self.state,
                self.expanded,
                width,
                theme,
            ));
        }
        if self.state.status == ToolStatusKind::Running {
            let live_style = Style::default().fg(theme.text_muted);
            if self.dropped_live_output_lines > 0 {
                rows.push(Line::styled(
                    format!("  ... ({} earlier lines)", self.dropped_live_output_lines),
                    Style::default().fg(theme.text_muted),
                ));
            }
            rows.extend(wrap_live_rows(&self.live_output, width, live_style));
        }
        rows
    }
}

fn wrap_live_rows(lines: &[String], width: usize, style: Style) -> Vec<Line> {
    const PREFIX: &str = "  ";
    let body_width = width.saturating_sub(PREFIX.len()).max(1);
    lines
        .iter()
        .flat_map(|line| {
            wrap_width(&strip_ansi(line), body_width)
                .into_iter()
                .map(move |segment| Line::styled(format!("{PREFIX}{segment}"), style))
        })
        .collect()
}
