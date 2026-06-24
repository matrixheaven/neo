use crate::ansi::Style;
use crate::chrome::ToolStatusKind;
use crate::chrome::TuiTheme;
use crate::components::wrap_width;
use crate::core::{Component, Expandable, Finalization, Line};

use super::plan_box::PlanBoxComponent;
use super::tool_renderers::{
    is_file_write_tool, is_pending_or_running, render_streaming_preview, render_tool_body_themed,
    tool_header_spans,
};

use std::path::PathBuf;

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
pub struct ToolCallComponent {
    state: ToolCallState,
    expanded: bool,
    progress_lines: Vec<String>,
    live_output: Vec<String>,
    dropped_live_output_lines: usize,
    live_output_chars: usize,
    workspace_dir: Option<PathBuf>,
    streaming_started_at: Option<std::time::Instant>,
}

const MAX_PROGRESS_LINES: usize = 24;
const MAX_LIVE_OUTPUT_LINES: usize = 6;
const MAX_LIVE_OUTPUT_CHARS: usize = 50_000;

impl ToolCallComponent {
    #[must_use]
    pub fn new(state: ToolCallState) -> Self {
        Self {
            state,
            expanded: false,
            progress_lines: Vec::new(),
            live_output: Vec::new(),
            dropped_live_output_lines: 0,
            live_output_chars: 0,
            workspace_dir: None,
            streaming_started_at: None,
        }
    }

    pub fn update_call(&mut self, arguments: Option<String>) {
        if let Some(args) = &arguments {
            if !args.is_empty() && self.streaming_started_at.is_none() {
                self.streaming_started_at = Some(std::time::Instant::now());
            }
        }
        self.state.arguments = arguments;
    }

    pub fn update_call_state(
        &mut self,
        name: String,
        arguments: Option<String>,
        status: ToolStatusKind,
    ) {
        self.state.name = name;
        if arguments.is_some() {
            self.state.arguments = arguments;
        }
        self.state.status = status;
        if status == ToolStatusKind::Running && self.streaming_started_at.is_none() {
            self.streaming_started_at = Some(std::time::Instant::now());
        }
    }

    pub fn append_progress(&mut self, line: impl Into<String>) {
        self.progress_lines.push(line.into());
        if self.progress_lines.len() > MAX_PROGRESS_LINES {
            let extra = self.progress_lines.len() - MAX_PROGRESS_LINES;
            self.progress_lines.drain(..extra);
        }
    }

    pub fn append_live_output(&mut self, output: impl Into<String>) {
        for line in output.into().lines() {
            self.live_output_chars += line.chars().count();
            self.live_output.push(line.to_owned());
        }
        self.trim_live_output();
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
    ) {
        self.state.result = result;
        self.state.details = details;
        self.state.exit_code = exit_code;
        self.state.status = if is_error {
            ToolStatusKind::Failed
        } else {
            ToolStatusKind::Succeeded
        };
        self.progress_lines.clear();
        self.live_output.clear();
        self.dropped_live_output_lines = 0;
        self.live_output_chars = 0;
        self.streaming_started_at = None;
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

    pub fn set_workspace_dir(&mut self, workspace_dir: impl Into<PathBuf>) {
        self.workspace_dir = Some(workspace_dir.into());
    }

    /// Borrow the underlying tool state (for grouping/rendering snapshots).
    #[must_use]
    pub const fn state(&self) -> &ToolCallState {
        &self.state
    }

    /// Consume into the underlying state.
    #[must_use]
    pub fn into_state(self) -> ToolCallState {
        self.state
    }

    #[must_use]
    pub fn result(&self) -> Option<&str> {
        self.state.result.as_deref()
    }

    #[must_use]
    pub fn progress(&self) -> &[String] {
        &self.progress_lines
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
            ToolStatusKind::Pending | ToolStatusKind::Running => Finalization::Live,
        }
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
            ToolStatusKind::Pending | ToolStatusKind::Running => Finalization::Live,
        }
    }
}

impl ToolCallComponent {
    /// Theme-aware render. Builds the header as styled spans (status symbol
    /// + tool name + key arg + chip) and the body as weak preview lines.
    #[must_use]
    pub fn render_with_theme(&mut self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        let header_spans = tool_header_spans(&self.state, theme, self.workspace_dir.as_deref());
        let header_width = width.saturating_sub(2).max(1);
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
            let status = if self.state.status == ToolStatusKind::Failed {
                Some("Rejected".to_string())
            } else {
                None
            };
            let mut plan_box = PlanBoxComponent::new(plan_content, plan_path);
            if let Some(status) = status {
                plan_box = plan_box.with_status(status);
            }
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
            rows.extend(wrap_live_rows(&self.progress_lines, width, live_style));
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
            wrap_width(line, body_width)
                .into_iter()
                .map(move |segment| Line::styled(format!("{PREFIX}{segment}"), style))
        })
        .collect()
}
