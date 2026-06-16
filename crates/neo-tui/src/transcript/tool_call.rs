use crate::ToolStatusKind;
use crate::ansi::Style;
use crate::app::TuiTheme;
use crate::core::{Component, Expandable, Finalization, Line};

use super::tool_renderers::{render_tool_body_themed, tool_header_spans};

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

#[derive(Debug, Clone)]
pub struct ToolCallComponent {
    state: ToolCallState,
    expanded: bool,
    progress_lines: Vec<String>,
    live_output: Vec<String>,
    dropped_live_output_lines: usize,
    live_output_chars: usize,
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
        }
    }

    pub fn update_call(&mut self, arguments: Option<String>) {
        self.state.arguments = arguments;
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

    /// Borrow the underlying tool state (for grouping/rendering snapshots).
    #[must_use]
    pub const fn state(&self) -> &ToolCallState {
        &self.state
    }

    /// Consume into the underlying state (used when draining finalized cards).
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
    /// + tool name + key arg + chip) and the body as muted preview lines.
    #[must_use]
    pub fn render_with_theme(&mut self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        let header_spans = tool_header_spans(&self.state, theme);
        let mut rows = vec![Line::from_spans(header_spans)];
        rows.extend(render_tool_body_themed(
            &self.state,
            self.expanded,
            width,
            theme,
        ));
        if self.state.status == ToolStatusKind::Running {
            let live_style = Style::default().fg(theme.muted);
            rows.extend(
                self.progress_lines
                    .iter()
                    .map(|line| Line::styled(format!("  {line}"), live_style)),
            );
            if self.dropped_live_output_lines > 0 {
                rows.push(Line::styled(
                    format!("  ... ({} earlier lines)", self.dropped_live_output_lines),
                    Style::default().fg(theme.muted),
                ));
            }
            rows.extend(
                self.live_output
                    .iter()
                    .map(|line| Line::styled(format!("  {line}"), live_style)),
            );
        }
        rows
    }
}
