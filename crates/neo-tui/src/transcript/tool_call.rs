use crate::primitive::Style;
use crate::primitive::theme::TuiTheme;
use crate::primitive::wrap_width;
use crate::primitive::{Color, Component, Expandable, Finalization, Line, Span, strip_ansi};
use crate::shell::ToolStatusKind;
use crate::theme_preview::ThemePreviewRenderer;
use crate::token_estimate::format_elapsed;
use neo_agent_core::session::{ToolOutputRef, ToolOutputStore};
use neo_agent_core::workflow::WorkflowExecutionOrigin;

use super::live_output::LiveOutput;
use super::plan_box::PlanBoxComponent;
use super::shell_tool_presentation;
use super::store::{ExpandedOutputCache, ExpandedOutputRange};
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

/// The streamed live tail frozen at a terminal transition, so the rows the
/// user was watching stay in place instead of being swapped for the white
/// head preview (which read as a whole-block flash).
#[derive(Debug, Clone, PartialEq, Eq)]
struct FrozenTail {
    lines: Vec<String>,
    dropped: usize,
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
    live_output: LiveOutput,
    workspace_dir: Option<PathBuf>,
    streaming_started_at: Option<Instant>,
    queue: Option<QueueDisplayState>,
    workflow_origin: Option<WorkflowExecutionOrigin>,
    workflow_activity_route_error: bool,
    /// Typed complete-display-output artifact for this execution, when the
    /// runtime captured one. Presentation metadata only: the TUI resolves the
    /// artifact by this typed reference, never by inferring it from text,
    /// result JSON, or ids.
    output_ref: Option<ToolOutputRef>,
    /// Frozen streamed tail for Bash tools that reached a terminal state with
    /// live rows on screen (see [`FrozenTail`]).
    final_tail: Option<FrozenTail>,
    /// Whether the muted live tail was ever painted while Running. Only
    /// commands the user actually saw streaming freeze their tail on
    /// completion; instantly-delivered results keep the white preview.
    live_rendered: bool,
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
            live_output: LiveOutput::new(MAX_LIVE_OUTPUT_LINES, MAX_LIVE_OUTPUT_CHARS),
            workspace_dir: None,
            streaming_started_at,
            queue: None,
            workflow_origin: None,
            workflow_activity_route_error: false,
            output_ref: None,
            final_tail: None,
            live_rendered: false,
        }
    }

    /// Attach the typed output reference for this execution. A later `Some`
    /// wins; `None` never clears an already-attached reference.
    pub fn attach_output_ref(&mut self, output_ref: Option<ToolOutputRef>) -> bool {
        if output_ref.is_some() && self.output_ref != output_ref {
            self.output_ref = output_ref;
            return true;
        }
        false
    }

    #[must_use]
    pub const fn output_ref(&self) -> Option<&ToolOutputRef> {
        self.output_ref.as_ref()
    }

    pub(crate) fn attach_workflow_origin(
        &mut self,
        workflow_origin: WorkflowExecutionOrigin,
    ) -> bool {
        if self.workflow_origin.is_some() {
            return false;
        }
        self.workflow_origin = Some(workflow_origin);
        true
    }

    #[must_use]
    pub(crate) fn accepts_workflow_origin(
        &self,
        workflow_origin: &WorkflowExecutionOrigin,
    ) -> bool {
        self.workflow_origin.as_ref().is_none_or(|current| {
            current.run_id == workflow_origin.run_id
                && current.invocation_id == workflow_origin.invocation_id
        })
    }

    #[must_use]
    pub const fn workflow_origin(&self) -> Option<&WorkflowExecutionOrigin> {
        self.workflow_origin.as_ref()
    }

    pub fn update_call(&mut self, arguments: Option<String>) -> bool {
        if self.workflow_activity_route_error || tool_status_is_terminal(self.state.status) {
            return false;
        }
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
        if self.workflow_activity_route_error || tool_status_is_terminal(self.state.status) {
            return false;
        }
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
        if self.workflow_activity_route_error || self.state.status != ToolStatusKind::Queued {
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
        if self.workflow_activity_route_error || tool_status_is_terminal(self.state.status) {
            return false;
        }
        self.live_output.append(&output.into())
    }

    /// Retain structured Edit progress/prepared details on the live card.
    pub fn set_live_details(&mut self, details: serde_json::Value) -> bool {
        if self.workflow_activity_route_error || tool_status_is_terminal(self.state.status) {
            return false;
        }
        if self.state.details.as_ref() == Some(&details) {
            return false;
        }
        self.state.details = Some(details);
        true
    }

    /// Freeze the live tail for Bash tools that streamed output, so the
    /// terminal state keeps the same body rows instead of swapping to the
    /// white head preview. Only tails the user actually saw painted on screen
    /// are frozen; instantly-delivered results keep the white preview.
    fn capture_final_tail(&mut self) {
        let (lines, dropped) = self.live_output.finalize();
        if self.state.name == "Bash" && self.live_rendered && !lines.is_empty() {
            self.final_tail = Some(FrozenTail { lines, dropped });
        }
    }

    pub fn set_result(
        &mut self,
        result: Option<String>,
        details: Option<serde_json::Value>,
        is_error: bool,
        exit_code: Option<i32>,
    ) -> bool {
        if self.workflow_activity_route_error {
            return false;
        }
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
            || self.streaming_started_at.is_some()
            || self.queue.is_some();
        if !changed {
            return false;
        }
        self.state.result = result;
        self.state.details = details;
        self.state.exit_code = exit_code;
        self.state.status = status;
        self.capture_final_tail();
        self.streaming_started_at = None;
        self.queue = None;
        true
    }

    pub fn set_terminal_status(&mut self, status: ToolStatusKind, result: Option<String>) -> bool {
        if self.workflow_activity_route_error {
            return false;
        }
        self.set_terminal_status_unchecked(status, result)
    }

    pub(crate) fn set_workflow_activity_route_error(&mut self) -> bool {
        if self.workflow_activity_route_error {
            return false;
        }
        self.set_terminal_status_unchecked(
            ToolStatusKind::Failed,
            Some("Workflow activity stopped because its origin changed.".to_owned()),
        );
        self.workflow_activity_route_error = true;
        true
    }

    fn set_terminal_status_unchecked(
        &mut self,
        status: ToolStatusKind,
        result: Option<String>,
    ) -> bool {
        // Edit/Write interruptions retain last structured progress details.
        let clear_details = self.state.name != "Edit" && self.state.name != "Write";
        let changed = self.state.result != result
            || (clear_details && self.state.details.is_some())
            || self.state.exit_code.is_some()
            || self.state.status != status
            || !self.live_output.is_empty()
            || self.streaming_started_at.is_some()
            || self.queue.is_some();
        if !changed {
            return false;
        }
        self.state.result = result;
        if clear_details {
            self.state.details = None;
        }
        self.state.exit_code = None;
        self.state.status = status;
        self.capture_final_tail();
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
        self.live_output.dropped_lines() > 0 || !self.live_output.is_empty()
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

const fn tool_status_is_terminal(status: ToolStatusKind) -> bool {
    matches!(
        status,
        ToolStatusKind::Succeeded | ToolStatusKind::Failed | ToolStatusKind::Cancelled
    )
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
                &self.state.name,
                self.state.arguments.as_deref().unwrap_or(""),
            );
            let elapsed = started_at.elapsed().as_secs();
            let chip = format!(
                " · ~{} tok · {}",
                crate::token_estimate::format_token_count(tokens),
                format_elapsed(elapsed)
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

        // ThemeDraft preview results render a structured, non-interactive card:
        // name/status, color samples, a representative TUI sample rendered with
        // the draft theme, and contrast warnings. There is deliberately no
        // Apply action — application stays with the /theme command.
        if self.state.name == "ThemeDraft"
            && let Some(details) = &self.state.details
            && details.get("kind").and_then(serde_json::Value::as_str)
                == Some("theme_draft_preview")
        {
            rows.extend(render_theme_draft_preview_card(details, width, theme));
        }

        // Bash results that streamed live rows freeze those rows on completion
        // instead of swapping to the white head preview. When expanded, the
        // full white result preview takes over as before.
        let frozen = (!self.expanded)
            .then_some(self.final_tail.as_ref())
            .flatten();
        if is_pending_or_running(self.state.status) && is_file_write_tool(&self.state.name) {
            rows.extend(render_streaming_preview(
                &self.state,
                self.expanded,
                width,
                theme,
                self.streaming_started_at,
            ));
        } else if frozen.is_some() {
            rows.extend(
                shell_tool_presentation::render_body(
                    &self.state,
                    self.expanded,
                    width,
                    theme,
                    self.workspace_dir.as_deref(),
                    false,
                )
                .unwrap_or_else(|| {
                    render_tool_body_themed(&self.state, self.expanded, width, theme)
                }),
            );
        } else {
            rows.extend(
                shell_tool_presentation::render_body(
                    &self.state,
                    self.expanded,
                    width,
                    theme,
                    self.workspace_dir.as_deref(),
                    true,
                )
                .unwrap_or_else(|| {
                    render_tool_body_themed(&self.state, self.expanded, width, theme)
                }),
            );
        }
        if self.state.status == ToolStatusKind::Running {
            let live_style = Style::default().fg(theme.text_muted);
            if self.live_output.dropped_lines() > 0 {
                rows.push(Line::styled(
                    format!("  ... ({} earlier lines)", self.live_output.dropped_lines()),
                    Style::default().fg(theme.text_muted),
                ));
            }
            if !self.live_output.is_empty() {
                self.live_rendered = true;
            }
            rows.extend(wrap_live_rows(&self.live_output.tail(), width, live_style));
        } else if let Some(frozen) = frozen {
            // Freeze the streamed rows in place: identical content and style to
            // the last live frame, so the diff renderer repaints nothing here.
            let live_style = Style::default().fg(theme.text_muted);
            let visible = frozen.lines.len();
            let remaining = self
                .state
                .result
                .as_deref()
                .map_or(0, |result| result.lines().count())
                .saturating_sub(visible);
            if frozen.dropped > 0 {
                // The streaming frame already rendered an earlier-lines note in
                // this position; swap its text in place (single-row repaint).
                rows.push(Line::styled(
                    if remaining > 0 {
                        format!("  ... ({remaining} more lines, ctrl+o to expand)")
                    } else {
                        format!("  ... ({} earlier lines)", frozen.dropped)
                    },
                    live_style,
                ));
            }
            rows.extend(wrap_live_rows(&frozen.lines, width, live_style));
            if frozen.dropped == 0 && remaining > 0 {
                rows.push(Line::styled(
                    format!("  ... ({remaining} more lines, ctrl+o to expand)"),
                    live_style,
                ));
            }
        }
        rows
    }

    pub(super) fn render_projected_state(
        &self,
        state: ToolCallState,
        expanded: bool,
        width: usize,
        theme: &TuiTheme,
    ) -> Vec<Line> {
        let mut projected = self.clone();
        projected.state = state;
        projected.expanded = expanded;
        projected.render_with_theme(width, theme)
    }

    /// The bounded visible range of this execution's complete output
    /// artifact, wrapped to `width` and cached per width in `output_cache`.
    ///
    /// Reads through [`ToolOutputStore`] — never the six-line live preview —
    /// and appends honest footers for every state where the complete source
    /// is unavailable or partial: no typed reference, a missing artifact, an
    /// artifact that is still incomplete, or a file that continues beyond the
    /// visible range. A missing source is always shown as explicitly
    /// incomplete; it is never relabeled complete.
    pub fn render_complete_output_range(
        &self,
        width: usize,
        theme: &TuiTheme,
        output_store: &ToolOutputStore,
        output_cache: &mut ExpandedOutputCache,
        max_lines: u64,
    ) -> Vec<Line> {
        let muted = Style::default().fg(theme.text_muted);
        let range: Option<ExpandedOutputRange> = self.output_ref.as_ref().and_then(|output_ref| {
            output_cache.visible_range(output_store, output_ref, width, max_lines, theme)
        });
        let Some(range) = range else {
            let note = if self.output_ref.is_none() {
                "  … complete output not captured"
            } else {
                "  … complete output unavailable"
            };
            return vec![Line::styled(note, muted).truncate_to_width(width)];
        };
        let mut rows = range.rows;
        if !range.complete {
            rows.push(Line::styled("  … output incomplete", muted).truncate_to_width(width));
        } else if !range.read_all {
            let remaining = range.total_lines.saturating_sub(range.read_lines);
            rows.push(
                Line::styled(
                    format!("  … {remaining} lines remain in the output file"),
                    muted,
                )
                .truncate_to_width(width),
            );
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

/// Rows for a `theme_draft_preview` tool-result card.
///
/// Branches on the typed details `kind` (never on presentation labels). The
/// card is append-only transcript content: it renders the stored preview
/// payload, samples a few normalized colors as swatches, renders a compact
/// representative TUI surface with the draft theme via `ThemePreviewRenderer`,
/// and lists deterministic contrast warnings. No Apply action is offered.
fn render_theme_draft_preview_card(
    details: &serde_json::Value,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    const SAMPLE_ROWS: usize = 7;
    const PREFIX: &str = "  ";

    let body_width = width.saturating_sub(PREFIX.len()).max(1);
    let normalized = details
        .get("normalized_colors")
        .and_then(serde_json::Value::as_object);

    let mut rows = Vec::new();
    rows.extend(theme_draft_preview_header_lines(details, width, theme));
    if let Some(samples) = theme_draft_preview_color_samples(details, normalized, theme) {
        rows.push(samples.truncate_to_width(width));
    }

    // Representative TUI surface rendered with the draft theme. The renderer
    // never mutates runtime chrome; it is a pure presentation of the payload.
    if let Some(colors) = normalized {
        let draft_theme = theme_from_normalized_colors(colors);
        let preview = ThemePreviewRenderer::new(draft_theme, body_width, SAMPLE_ROWS, "draft");
        for row in preview.render() {
            rows.push(Line::styled(format!("{PREFIX}{row}"), Style::default()));
        }
    }
    rows.extend(theme_draft_preview_warning_lines(details, width, theme));
    rows
}

/// Status + name + draft id + destination + fingerprint lines.
fn theme_draft_preview_header_lines(
    details: &serde_json::Value,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    const PREFIX: &str = "  ";
    let muted = Style::default().fg(theme.text_muted);
    let display_name = details
        .get("display_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("theme");
    let draft_id = details
        .get("draft_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("draft");
    let candidate_id = details
        .get("candidate_theme_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let fingerprint = details
        .get("fingerprint")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    let mut header_spans = vec![
        Span::styled("●", Style::default().fg(theme.status_ok)),
        Span::raw(" "),
        Span::styled(display_name, Style::default().fg(theme.text_primary).bold()),
        Span::styled("  preview", Style::default().fg(theme.status_ok)),
        Span::styled(format!("  {draft_id}"), muted),
    ];
    if !candidate_id.is_empty() {
        header_spans.push(Span::styled(format!("  → {candidate_id}"), muted));
    }
    let mut rows = vec![Line::from_spans(header_spans).truncate_to_width(width)];
    if !fingerprint.is_empty() {
        rows.push(
            Line::styled(format!("{PREFIX}fingerprint {fingerprint}"), muted)
                .truncate_to_width(width),
        );
    }
    rows
}

/// One swatch line: overridden tokens first, then representative core tokens.
fn theme_draft_preview_color_samples(
    details: &serde_json::Value,
    normalized: Option<&serde_json::Map<String, serde_json::Value>>,
    theme: &TuiTheme,
) -> Option<Line> {
    const MAX_COLOR_SAMPLES: usize = 6;
    let muted = Style::default().fg(theme.text_muted);
    let overridden: Vec<&str> = details
        .get("overridden_tokens")
        .and_then(serde_json::Value::as_array)
        .map(|tokens| {
            tokens
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect()
        })
        .unwrap_or_default();
    let mut sample_tokens = overridden.clone();
    for token in [
        "brand",
        "text_primary",
        "text_muted",
        "status_ok",
        "status_error",
        "status_warn",
    ] {
        if !sample_tokens.contains(&token) {
            sample_tokens.push(token);
        }
    }
    sample_tokens.truncate(MAX_COLOR_SAMPLES);
    let mut sample_spans: Vec<Span> = Vec::new();
    for token in sample_tokens {
        let Some(hex) = normalized
            .and_then(|colors| colors.get(token))
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let Some(color) = color_from_canonical_string(hex) else {
            continue;
        };
        if !sample_spans.is_empty() {
            sample_spans.push(Span::raw("  "));
        }
        sample_spans.push(Span::styled("██", Style::default().bg(color)));
        sample_spans.push(Span::raw(" "));
        sample_spans.push(Span::styled(format!("{token} {hex}"), muted));
    }
    if sample_spans.is_empty() {
        None
    } else {
        Some(Line::from_spans(sample_spans))
    }
}

/// Deterministic contrast warnings from the preview, wrapped to the width.
fn theme_draft_preview_warning_lines(
    details: &serde_json::Value,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    const PREFIX: &str = "  ";
    let Some(warnings) = details
        .get("contrast_warnings")
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    let warn_style = Style::default().fg(theme.status_warn);
    let mut rows = Vec::new();
    for warning in warnings.iter().filter_map(serde_json::Value::as_str) {
        for segment in wrap_width(&format!("{PREFIX}⚠ {warning}"), width) {
            rows.push(Line::styled(segment, warn_style));
        }
    }
    rows
}

/// Parse a canonical theme color string (`#rrggbb` or the existing named set)
/// into a render color. Unknown values yield `None` and are skipped.
fn color_from_canonical_string(value: &str) -> Option<Color> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        if hex.len() != 6 || !hex.chars().all(|character| character.is_ascii_hexdigit()) {
            return None;
        }
        let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;
        return Some(Color::Rgb(red, green, blue));
    }
    match value.to_ascii_lowercase().as_str() {
        "reset" => Some(Color::Reset),
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "darkgray" | "dark_gray" | "dark-grey" => Some(Color::DarkGray),
        "lightred" | "light_red" | "light-red" => Some(Color::LightRed),
        "lightgreen" | "light_green" | "light-green" => Some(Color::LightGreen),
        "lightyellow" | "light_yellow" | "light-yellow" => Some(Color::LightYellow),
        "lightblue" | "light_blue" | "light-blue" => Some(Color::LightBlue),
        "lightmagenta" | "light_magenta" | "light-magenta" => Some(Color::LightMagenta),
        "lightcyan" | "light_cyan" | "light-cyan" => Some(Color::LightCyan),
        "white" => Some(Color::White),
        _ => None,
    }
}

/// Build a `TuiTheme` from the normalized canonical color payload. Tokens the
/// payload does not carry (or cannot parse) keep the built-in default value.
fn theme_from_normalized_colors(colors: &serde_json::Map<String, serde_json::Value>) -> TuiTheme {
    let mut theme = TuiTheme::default();
    for (token, value) in colors {
        let Some(color) = value.as_str().and_then(color_from_canonical_string) else {
            continue;
        };
        match token.as_str() {
            "text_primary" => theme.text_primary = color,
            "prompt" => theme.prompt = color,
            "brand" => theme.brand = color,
            "status_ok" => theme.status_ok = color,
            "status_error" => theme.status_error = color,
            "status_warn" => theme.status_warn = color,
            "text_muted" => theme.text_muted = color,
            "user_message" => theme.user_message = color,
            "diff_added" => theme.diff_added = color,
            "diff_removed" => theme.diff_removed = color,
            "diff_hunk" => theme.diff_hunk = color,
            "diff_context" => theme.diff_context = color,
            "selection_bg" => theme.selection_bg = color,
            "status_pending" => theme.status_pending = color,
            "status_cancelled" => theme.status_cancelled = color,
            "approval_border" => theme.approval_border = color,
            "selected_fg" => theme.selected_fg = color,
            "selected_bg" => theme.selected_bg = color,
            "overlay_border" => theme.overlay_border = color,
            "footer_permission_allow" => theme.footer_permission_allow = color,
            "footer_permission_ask" => theme.footer_permission_ask = color,
            "footer_permission_deny" => theme.footer_permission_deny = color,
            "footer_working" => theme.footer_working = color,
            "footer_context_ok" => theme.footer_context_ok = color,
            "footer_context_warn" => theme.footer_context_warn = color,
            "footer_context_critical" => theme.footer_context_critical = color,
            "shell_mode" => theme.shell_mode = color,
            _ => {}
        }
    }
    theme
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn queued_tool_can_start() {
        let mut component = ToolCallComponent::new(ToolCallState {
            id: "read-1".to_owned(),
            name: "Read".to_owned(),
            arguments: Some("{}".to_owned()),
            result: None,
            details: None,
            status: ToolStatusKind::Queued,
            exit_code: None,
        });

        assert!(component.update_call_state(
            "Read".to_owned(),
            Some("{\"path\":\"README.md\"}".to_owned()),
            ToolStatusKind::Running,
        ));
        assert_eq!(component.status(), ToolStatusKind::Running);
        assert_eq!(component.arguments(), Some("{\"path\":\"README.md\"}"));
    }

    #[test]
    fn file_write_streaming_chip_formats_elapsed_seconds() {
        let mut component = ToolCallComponent::new(ToolCallState {
            id: "edit-1".to_owned(),
            name: "Edit".to_owned(),
            arguments: Some("{}".to_owned()),
            result: None,
            details: None,
            status: ToolStatusKind::Running,
            exit_code: None,
        });
        component.streaming_started_at =
            Some(Instant::now().checked_sub(Duration::from_secs(65)).unwrap());

        let header = component.render(120)[0].text();

        assert!(header.contains("~0 tok · 1m 5s"), "{header}");
    }

    #[test]
    fn bash_with_streamed_output_freezes_tail_at_completion() {
        let mut component = ToolCallComponent::new(ToolCallState {
            id: "bash-1".to_owned(),
            name: "Bash".to_owned(),
            arguments: Some(r#"{"command":"cargo build"}"#.to_owned()),
            result: None,
            details: None,
            status: ToolStatusKind::Running,
            exit_code: None,
        });
        component.append_live_output("line one\nline two\nline three\n");
        component.append_live_output("line four\nline five\nline six\nline seven\n");
        // The muted tail must have been painted while Running for the freeze.
        component.render(120);
        assert!(component.set_result(
            Some(
                "line one\nline two\nline three\nline four\nline five\nline six\nline seven\nline eight\nline nine\n"
                    .to_owned()
            ),
            None,
            false,
            Some(0),
        ));

        let rendered = component.render(120);
        let joined: Vec<String> = rendered.iter().map(|line| line.text()).collect();
        let joined = joined.join("\n");

        // The streamed tail stays on screen (6 lines: line two..line seven).
        assert!(joined.contains("line seven"), "{joined}");
        assert!(joined.contains("line six"), "{joined}");
        // The white head preview must not replace the tail: lines that were
        // never streamed are hidden behind the truncation note.
        assert!(!joined.contains("line eight"), "{joined}");
        assert!(!joined.contains("line nine"), "{joined}");
        // The dropped first line is folded into the result-based note.
        assert!(!joined.contains("line one"), "{joined}");
        assert!(
            joined.contains("... (3 more lines, ctrl+o to expand)"),
            "{joined}"
        );
    }

    #[test]
    fn bash_unrendered_stream_keeps_white_head_preview() {
        let mut component = ToolCallComponent::new(ToolCallState {
            id: "bash-2".to_owned(),
            name: "Bash".to_owned(),
            arguments: Some(r#"{"command":"echo hi"}"#.to_owned()),
            result: None,
            details: None,
            status: ToolStatusKind::Running,
            exit_code: None,
        });
        // Output arrives and completes before any frame painted the tail: the
        // user never saw streaming, so the white head preview stays.
        component.append_live_output("line one\nline two\nline three\nline four\n");
        assert!(component.set_result(
            Some("line one\nline two\nline three\nline four\n".to_owned()),
            None,
            false,
            Some(0),
        ));

        let rendered = component.render(120);
        let joined: Vec<String> = rendered.iter().map(|line| line.text()).collect();
        let joined = joined.join("\n");

        assert!(joined.contains("line one"), "{joined}");
        assert!(joined.contains("line two"), "{joined}");
        assert!(joined.contains("line three"), "{joined}");
        assert!(
            joined.contains("... (1 more lines, ctrl+o to expand)"),
            "{joined}"
        );
    }

    #[test]
    fn frozen_bash_expands_to_full_result_preview() {
        let mut component = ToolCallComponent::new(ToolCallState {
            id: "bash-3".to_owned(),
            name: "Bash".to_owned(),
            arguments: Some(r#"{"command":"cargo build"}"#.to_owned()),
            result: None,
            details: None,
            status: ToolStatusKind::Running,
            exit_code: None,
        });
        component.append_live_output("line one\nline two\nline three\n");
        component.render(120);
        assert!(component.set_result(
            Some("line one\nline two\nline three\nline four\n".to_owned()),
            None,
            false,
            Some(0),
        ));
        component.set_expanded(true);

        let rendered = component.render(120);
        let joined: Vec<String> = rendered.iter().map(|line| line.text()).collect();
        let joined = joined.join("\n");

        // Ctrl+O still reveals the complete result as the white preview.
        assert!(joined.contains("line one"), "{joined}");
        assert!(joined.contains("line four"), "{joined}");
        assert!(!joined.contains("more lines, ctrl+o to expand"), "{joined}");
    }
}
