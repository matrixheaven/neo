use std::fmt::Write as _;

use crossterm::event::MouseButton;

use crate::input::{MouseEvent, MouseKind};
use crate::primitive::theme::TuiTheme;
use crate::primitive::{
    Color, Style, pad_to_width, paint, truncate_width, visible_width, wrap_text,
};
use crate::transcript::{CHROME_GUTTER, frame_content_width};

use super::{
    state::{TaskBrowserAction, TaskBrowserFilter, TaskBrowserFocus, TaskBrowserState},
    view::{
        TaskBrowserItem, TaskBrowserStatus, TaskBrowserWorkflowChild, TaskBrowserWorkflowRowState,
        TaskBrowserWorkflowStep,
    },
};

/// Below this content width the browser must render at most two lines per task
/// row; at or above it, one line per task row.
const MEDIUM_MIN_COLUMNS: usize = 70;
/// At or above this content width the browser splits into list + inspector.
const WIDE_MIN_COLUMNS: usize = 100;
const MIN_TASK_LIST_COLUMNS: usize = 30;
const MAX_TASK_LIST_COLUMNS: usize = 42;

/// Which full-page or split-page surface the browser shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserPage {
    /// List and inspector side by side (width >= 100, always).
    Split,
    /// Single full-width task list (details closed).
    List,
    /// Single full-width details page (details open, Tasks focus).
    Details,
    /// Single full-width latest-output page (details open, Output focus).
    Output,
    /// Workflow: Steps and Agents side by side with a lower selected-agent
    /// preview (width >= 100).
    WorkflowWide,
    /// Workflow: stacked summary, Steps, Agents, and compact preview
    /// (width 70-99).
    WorkflowStacked,
    /// Workflow: stable header plus a `[STEPS] [AGENTS]` tab selector and one
    /// active navigation page (width < 70).
    WorkflowTabs,
}

/// Single-source geometry for the general task browser frame.
///
/// Every breakpoint decision and rectangle in `render_browser` and
/// `render_workflow` comes from this one value so pointer hit testing can
/// reuse the exact same arithmetic.
struct BrowserLayout {
    width: usize,
    content_top: usize,
    content_height: usize,
    footer_top: usize,
    page: BrowserPage,
    list_width: usize,
    right_left: usize,
    /// Screen rows occupied by one task row (2 below `MEDIUM_MIN_COLUMNS`,
    /// 1 at or above). Hit testing maps screen rows to task indices with it.
    list_row_height: usize,
    /// Workflow: first content row of the Steps section (absolute screen row).
    steps_top: usize,
    /// Workflow: first content row of the Agents section (stacked only).
    agents_top: usize,
    /// Workflow: first content row of the lower selected-agent preview.
    preview_top: usize,
    /// Split page: absolute screen row where the inspector's LATEST OUTPUT
    /// divider starts (`content_top + 3 + details_rows`). Other pages never
    /// render the inspector and default to `content_top`.
    inspector_output_top: usize,
}

impl BrowserLayout {
    fn new(width: usize, height: usize, state: &TaskBrowserState) -> Self {
        let workflow = state.workflow_item().is_some();
        let content_top = if workflow { 3 } else { 1 };
        let content_height = height.saturating_sub(content_top + 1).max(1);
        let list_width = (width / 3).clamp(MIN_TASK_LIST_COLUMNS, MAX_TASK_LIST_COLUMNS);
        let page = if workflow {
            if width >= WIDE_MIN_COLUMNS {
                BrowserPage::WorkflowWide
            } else if width >= MEDIUM_MIN_COLUMNS {
                BrowserPage::WorkflowStacked
            } else {
                BrowserPage::WorkflowTabs
            }
        } else if width >= WIDE_MIN_COLUMNS {
            BrowserPage::Split
        } else if !state.task_details_open() {
            BrowserPage::List
        } else if state.focus() == TaskBrowserFocus::Output {
            BrowserPage::Output
        } else {
            BrowserPage::Details
        };
        let (steps_top, agents_top, preview_top) = match page {
            BrowserPage::WorkflowWide => {
                // The lower preview keeps at most ~1/3 of the content so the
                // Steps/Agents split keeps at least ~2/3; on very short
                // terminals it shrinks to fit.
                let preview_height = content_height / 3;
                let preview_top = content_top + content_height - preview_height;
                (content_top, content_top, preview_top)
            }
            BrowserPage::WorkflowStacked => {
                // Fixed header/footer and the navigation sections outrank the
                // preview: it receives only the remainder and may be empty on
                // short terminals.
                let steps_height = (content_height / 3).max(3).min(content_height);
                let agents_height = (content_height / 3)
                    .max(3)
                    .min(content_height.saturating_sub(steps_height));
                let steps_top = content_top;
                let agents_top = steps_top + steps_height;
                let preview_top = agents_top + agents_height;
                (steps_top, agents_top, preview_top)
            }
            _ => (content_top, content_top, content_top),
        };
        // The inspector splits the remaining rows under the two identity rows
        // and the DETAILS divider in half; the LATEST OUTPUT divider starts
        // one row after the Details section.
        let inspector_output_top = if page == BrowserPage::Split {
            content_top + 3 + content_height.saturating_sub(4) / 2
        } else {
            content_top
        };
        Self {
            width,
            content_top,
            content_height,
            footer_top: height.saturating_sub(1),
            page,
            list_width,
            right_left: list_width + 1,
            list_row_height: if width < MEDIUM_MIN_COLUMNS { 2 } else { 1 },
            steps_top,
            agents_top,
            preview_top,
            inspector_output_top,
        }
    }
}

pub struct TaskBrowserRenderer<'a> {
    state: &'a TaskBrowserState,
    theme: TuiTheme,
}

impl<'a> TaskBrowserRenderer<'a> {
    #[must_use]
    pub const fn new(state: &'a TaskBrowserState, theme: TuiTheme) -> Self {
        Self { state, theme }
    }

    #[must_use]
    pub fn render(&self, width: usize, height: usize) -> Vec<String> {
        if width == 0 || height == 0 {
            return Vec::new();
        }
        if self.state.workflow_item().is_some() {
            self.render_workflow(width, height)
        } else {
            self.render_browser(width, height)
        }
    }

    /// Map one terminal-space mouse event to a Task Browser action using the
    /// exact same `BrowserLayout` geometry the renderer uses.
    ///
    /// Only actions this browser owns are produced; no rectangle math leaks
    /// to callers. `terminal_width` is the full terminal width; the chrome
    /// gutter and the unused last column are stripped here, so callers pass
    /// raw terminal coordinates. Drag and release events map to `None` and
    /// stay consumed by the browser (they never reach the transcript).
    #[must_use]
    pub fn pointer_action(
        &self,
        terminal_width: usize,
        terminal_height: usize,
        mouse: &MouseEvent,
    ) -> Option<TaskBrowserAction> {
        if terminal_width == 0 || terminal_height == 0 {
            return None;
        }
        let content_width = frame_content_width(terminal_width);
        let column = usize::from(mouse.column).checked_sub(CHROME_GUTTER)?;
        let row = usize::from(mouse.row);
        if column >= content_width || row >= terminal_height {
            return None;
        }
        let layout = BrowserLayout::new(content_width, terminal_height, self.state);
        match mouse.kind {
            MouseKind::Drag | MouseKind::Release => None,
            MouseKind::Press if mouse.button == MouseButton::Left => {
                self.pointer_press(&layout, column, row)
            }
            MouseKind::ScrollUp => self.pointer_wheel(&layout, column, row, -1),
            MouseKind::ScrollDown => self.pointer_wheel(&layout, column, row, 1),
            MouseKind::Press => None,
        }
    }

    /// Left-button press: select the row under the pointer. Header rows, pane
    /// borders, dividers, blank rows below the last item, and the footer are
    /// no-ops. The index is translated to a stable ID or key by the state
    /// handler; row numbers never survive a refresh.
    fn pointer_press(
        &self,
        layout: &BrowserLayout,
        column: usize,
        row: usize,
    ) -> Option<TaskBrowserAction> {
        if row < layout.content_top || row >= layout.footer_top {
            return None;
        }
        match layout.page {
            BrowserPage::Split | BrowserPage::List => {
                if layout.page == BrowserPage::Split && column >= layout.list_width {
                    return None; // inspector column
                }
                if row < layout.content_top + 1 {
                    return None; // pane top border
                }
                if row + 1 >= layout.footer_top {
                    return None; // pane bottom border
                }
                let index = (row - layout.content_top - 1) / layout.list_row_height;
                (index < self.state.visible_items().len())
                    .then_some(TaskBrowserAction::SelectTaskRow(index))
            }
            BrowserPage::Details | BrowserPage::Output => None,
            BrowserPage::WorkflowWide => {
                if row + 1 < layout.preview_top && column < layout.list_width {
                    let index = row.checked_sub(layout.content_top + 1)?;
                    (index < self.step_count())
                        .then_some(TaskBrowserAction::SelectWorkflowStepRow(index))
                } else if row + 1 < layout.preview_top && column >= layout.right_left {
                    let index = row.checked_sub(layout.content_top + 1)?;
                    (index < self.agent_count())
                        .then_some(TaskBrowserAction::SelectWorkflowAgentRow(index))
                } else {
                    None
                }
            }
            BrowserPage::WorkflowStacked => {
                if row > layout.steps_top && row + 1 < layout.agents_top {
                    let index = row - layout.steps_top - 1;
                    (index < self.step_count())
                        .then_some(TaskBrowserAction::SelectWorkflowStepRow(index))
                } else if row > layout.agents_top && row + 1 < layout.preview_top {
                    let index = row - layout.agents_top - 1;
                    (index < self.agent_count())
                        .then_some(TaskBrowserAction::SelectWorkflowAgentRow(index))
                } else {
                    None
                }
            }
            BrowserPage::WorkflowTabs => {
                if row < layout.content_top + 2 {
                    return None; // tab selector and pane top border
                }
                if row + 1 >= layout.footer_top {
                    return None; // pane bottom border
                }
                let index = row - layout.content_top - 2;
                if self.state.focus() == TaskBrowserFocus::Steps {
                    (index < self.step_count())
                        .then_some(TaskBrowserAction::SelectWorkflowStepRow(index))
                } else {
                    (index < self.agent_count())
                        .then_some(TaskBrowserAction::SelectWorkflowAgentRow(index))
                }
            }
        }
    }

    /// Pointer wheel: move the selection of the pane under the pointer. The
    /// wheel never reaches the transcript while the browser is open.
    fn pointer_wheel(
        &self,
        layout: &BrowserLayout,
        column: usize,
        row: usize,
        delta: isize,
    ) -> Option<TaskBrowserAction> {
        match layout.page {
            BrowserPage::Split => {
                if column < layout.list_width
                    && row >= layout.content_top
                    && row < layout.footer_top
                {
                    Some(TaskBrowserAction::MoveTaskSelection(delta))
                } else if row >= layout.content_top && row < layout.footer_top {
                    // Inspector: only the LATEST OUTPUT section scrolls; the
                    // identity and Details rows keep the wheel inert.
                    (row >= layout.inspector_output_top)
                        .then_some(TaskBrowserAction::MoveOutputScroll(delta))
                } else {
                    None
                }
            }
            BrowserPage::List => (row >= layout.content_top && row < layout.footer_top)
                .then_some(TaskBrowserAction::MoveTaskSelection(delta)),
            BrowserPage::Details => None,
            BrowserPage::Output => (row >= layout.content_top && row < layout.footer_top)
                .then_some(TaskBrowserAction::MoveOutputScroll(delta)),
            BrowserPage::WorkflowWide => {
                if row < layout.preview_top && column < layout.list_width {
                    Some(TaskBrowserAction::MoveWorkflowStepSelection(delta))
                } else if row < layout.preview_top && column >= layout.right_left {
                    Some(TaskBrowserAction::MoveWorkflowAgentSelection(delta))
                } else {
                    None
                }
            }
            BrowserPage::WorkflowStacked => {
                if row >= layout.steps_top && row < layout.agents_top {
                    Some(TaskBrowserAction::MoveWorkflowStepSelection(delta))
                } else if row >= layout.agents_top && row < layout.preview_top {
                    Some(TaskBrowserAction::MoveWorkflowAgentSelection(delta))
                } else {
                    None
                }
            }
            BrowserPage::WorkflowTabs => {
                if row > layout.content_top && row < layout.footer_top {
                    if self.state.focus() == TaskBrowserFocus::Steps {
                        Some(TaskBrowserAction::MoveWorkflowStepSelection(delta))
                    } else {
                        Some(TaskBrowserAction::MoveWorkflowAgentSelection(delta))
                    }
                } else {
                    None
                }
            }
        }
    }

    fn step_count(&self) -> usize {
        self.state
            .workflow_item()
            .and_then(|item| item.workflow.as_ref())
            .map_or(0, |workflow| workflow.steps.len())
    }

    fn agent_count(&self) -> usize {
        self.state
            .workflow_item()
            .and_then(|item| item.workflow.as_ref())
            .map_or(0, |workflow| workflow.child_page.items.len())
    }

    fn render_workflow(&self, width: usize, height: usize) -> Vec<String> {
        let layout = BrowserLayout::new(width, height, self.state);
        if height < 4 {
            let mut lines = self.workflow_header(width);
            lines.push(self.workflow_footer(&layout));
            return pad_height(lines, height);
        }
        // Drafts and Agent Details replace the whole workspace with one
        // full-width page; everything else renders the responsive layout.
        let body = if self.state.save_draft().is_some() || self.state.answer_draft().is_some() {
            self.draft_page(width, height - 1)
        } else if self.state.child_details_open() {
            self.agent_details_page(width, height - 1)
        } else {
            match layout.page {
                BrowserPage::WorkflowWide => self.workflow_wide(&layout),
                BrowserPage::WorkflowStacked => self.workflow_stacked(&layout),
                BrowserPage::WorkflowTabs => self.workflow_tabs(&layout),
                _ => Vec::new(),
            }
        };
        let mut lines = vec![String::new(); height];
        for (index, line) in body.into_iter().take(height - 1).enumerate() {
            lines[index] = line;
        }
        lines[height - 1] = self.workflow_footer(&layout);
        lines
    }

    /// Stable Workflow identity rows: display name with right-aligned status
    /// and elapsed, the purpose, and observed child counts with a
    /// right-aligned `NEEDS INPUT` marker when a request is pending.
    fn workflow_header(&self, width: usize) -> Vec<String> {
        let item = self
            .state
            .workflow_item()
            .expect("workflow view requires item");
        let workflow = item.workflow.as_ref().expect("workflow item carries meta");
        let right = format!(
            "{} · {}",
            item.status.label().to_uppercase(),
            format_elapsed(workflow.elapsed_ms)
        );
        let mut identity = format!(" WORKFLOW / {} ", workflow.display_name);
        if visible_width(&identity) + 1 + visible_width(&right) <= width {
            identity = format!(
                "{}{}",
                pad_to_width(&identity, width - visible_width(&right)),
                right
            );
        } else {
            // Keep the right status; truncate only the left identity.
            let left_width = width.saturating_sub(visible_width(&right) + 1);
            identity = format!(
                "{}{}",
                truncate_width(&identity, left_width, "...", false),
                right
            );
        }
        let mut counts = format!(
            " {} done · {} working · {} queued",
            workflow
                .steps
                .iter()
                .map(|step| step.done_count)
                .sum::<u64>(),
            workflow
                .steps
                .iter()
                .map(|step| step.working_count)
                .sum::<u64>(),
            workflow
                .steps
                .iter()
                .map(|step| step.queued_count)
                .sum::<u64>(),
        );
        if workflow.pending_user.is_some() {
            let needs = paint("NEEDS INPUT", Style::default().fg(self.theme.status_warn));
            if visible_width(&counts) + 1 + visible_width(&needs) <= width {
                counts = format!(
                    "{}{}",
                    pad_to_width(&counts, width - visible_width(&needs)),
                    needs
                );
            } else {
                let left_width = width.saturating_sub(visible_width(&needs) + 1);
                counts = format!(
                    "{}{}",
                    truncate_width(&counts, left_width, "...", false),
                    needs
                );
            }
        }
        vec![
            truncate_width(&identity, width, "...", false),
            truncate_width(&format!(" {}", workflow.purpose), width, "...", false),
            truncate_width(&counts, width, "...", false),
        ]
    }

    fn workflow_footer(&self, layout: &BrowserLayout) -> String {
        if let Some(task_id) = self.state.stop_confirmation_task_id() {
            return truncate_width(
                &format!(" Stop {task_id}?  Enter confirm  Esc back"),
                layout.width,
                "...",
                false,
            );
        }
        if let Some(draft) = self.state.save_draft() {
            return truncate_width(
                if draft.replacement.is_some() {
                    " Enter replace  Esc cancel"
                } else {
                    " Tab destination  Enter save  Esc back"
                },
                layout.width,
                "...",
                false,
            );
        }
        if let Some(draft) = self.state.answer_draft() {
            let help = if draft.form.structured_fallback {
                " Enter submit  Esc not now"
            } else if self.state.selected_answer_field().is_some_and(|field| {
                matches!(field.control, super::WorkflowAnswerControl::ObjectArray)
            }) {
                " Delete remove row  + add row  Left/Right select row  Up/Down fields  Enter submit  Esc later"
            } else {
                " Up/Down fields  Left/Right choose  Space toggle  Enter submit  Esc later"
            };
            return truncate_width(help, layout.width, "...", false);
        }
        let workflow = self
            .state
            .workflow_item()
            .and_then(|item| item.workflow.as_ref());
        let mut footer = match layout.page {
            BrowserPage::WorkflowTabs => " Tab switch  Enter open  Esc back",
            _ if self.state.child_details_open() => " PgUp/PgDn scroll  Esc back",
            _ => " Tab switch  Enter details  P pause/resume  X stop  Esc back",
        }
        .to_owned();
        if workflow.is_some_and(|value| value.inline_unsaved) {
            footer.push_str("  S save");
        }
        truncate_width(&footer, layout.width, "...", false)
    }

    fn steps_pane(&self, width: usize, height: usize) -> Vec<String> {
        let selected = self.state.selected_workflow_step();
        let body_width = width.saturating_sub(4);
        let body = self
            .state
            .workflow_item()
            .and_then(|item| item.workflow.as_ref())
            .map(|workflow| {
                workflow
                    .steps
                    .iter()
                    .enumerate()
                    .map(|(index, step)| {
                        let row = Self::step_row(step, index, body_width);
                        self.paint_task_row(
                            &row,
                            body_width,
                            step.state.marker(),
                            self.row_state_color(step.state),
                            selected == Some(step),
                        )
                    })
                    .collect()
            })
            .unwrap_or_else(|| vec!["No steps yet.".to_owned()]);
        let color = if self.state.focus() == TaskBrowserFocus::Steps {
            self.theme.brand
        } else {
            self.theme.overlay_border
        };
        pane(" Steps ", width, height, &body, color)
    }

    fn children_pane(&self, width: usize, height: usize) -> Vec<String> {
        let selected = self.state.selected_workflow_child();
        let body_width = width.saturating_sub(4);
        let mut body = self
            .state
            .workflow_item()
            .and_then(|item| item.workflow.as_ref())
            .map(|workflow| {
                workflow
                    .child_page
                    .items
                    .iter()
                    .map(|child| {
                        let row = Self::agent_row(child, body_width);
                        self.paint_task_row(
                            &row,
                            body_width,
                            child.state.marker(),
                            self.row_state_color(child.state),
                            selected == Some(child),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if body.is_empty() {
            body.push("No agents in this step.".to_owned());
        }
        let color = if self.state.focus() == TaskBrowserFocus::Agents {
            self.theme.brand
        } else {
            self.theme.overlay_border
        };
        pane(" Agents ", width, height, &body, color)
    }

    /// `{marker} {ordinal} {title}` with the observed child counts
    /// right-aligned; counts are dropped first, then the row truncates.
    fn step_row(step: &TaskBrowserWorkflowStep, ordinal: usize, width: usize) -> String {
        let mut text = format!("{} {} {}", step.state.marker(), ordinal + 1, step.title);
        let counts = format!(
            "{} · {} · {}",
            step.done_count, step.working_count, step.queued_count
        );
        if visible_width(&text) + 1 + visible_width(&counts) <= width {
            text = format!(
                "{}{}",
                pad_to_width(&text, width - visible_width(&counts)),
                counts
            );
        }
        truncate_width(&text, width, "...", false)
    }

    /// `{marker} {title}[ {role}]  {latest_activity}` with the elapsed time
    /// right-aligned; elapsed is dropped first, then the row truncates.
    fn agent_row(child: &TaskBrowserWorkflowChild, width: usize) -> String {
        let role = child
            .role
            .as_deref()
            .map_or(String::new(), |role| format!(" [{role}]"));
        let activity = child
            .latest_activity
            .as_deref()
            .map_or(String::new(), |value| format!("  {value}"));
        let mut text = format!(
            "{} {}{}{}",
            child.state.marker(),
            child.title,
            role,
            activity
        );
        if visible_width(&text) + 1 + visible_width(&child.elapsed) <= width {
            text = format!(
                "{}{}",
                pad_to_width(&text, width - visible_width(&child.elapsed)),
                child.elapsed
            );
        }
        truncate_width(&text, width, "...", false)
    }

    fn row_state_color(&self, state: TaskBrowserWorkflowRowState) -> Color {
        match state {
            TaskBrowserWorkflowRowState::Pending => self.theme.status_pending,
            TaskBrowserWorkflowRowState::Working | TaskBrowserWorkflowRowState::Recovering => {
                self.theme.status_warn
            }
            TaskBrowserWorkflowRowState::Completed => self.theme.status_ok,
            TaskBrowserWorkflowRowState::Failed => self.theme.status_error,
            TaskBrowserWorkflowRowState::Paused => self.theme.status_cancelled,
        }
    }

    /// Wide split: Steps and Agents side by side, lower selected-agent
    /// preview across the full width.
    fn workflow_wide(&self, layout: &BrowserLayout) -> Vec<String> {
        let mut lines = self.workflow_header(layout.width);
        let top_height = layout.preview_top.saturating_sub(layout.content_top);
        let agents_width = layout.width.saturating_sub(layout.list_width + 1);
        lines.extend(join_columns(
            &[
                self.steps_pane(layout.list_width, top_height),
                self.children_pane(agents_width, top_height),
            ],
            &[layout.list_width, agents_width],
            top_height,
        ));
        lines.extend(self.agent_preview(
            layout.width,
            layout.content_top + layout.content_height - layout.preview_top,
        ));
        lines
    }

    /// Medium stack: summary, Steps, Agents, and a compact selected-agent
    /// preview above the fixed footer.
    fn workflow_stacked(&self, layout: &BrowserLayout) -> Vec<String> {
        let mut lines = self.workflow_header(layout.width);
        let steps_height = layout.agents_top.saturating_sub(layout.steps_top);
        let agents_height = layout.preview_top.saturating_sub(layout.agents_top);
        let preview_height = layout.content_top + layout.content_height - layout.preview_top;
        lines.extend(self.steps_pane(layout.width, steps_height));
        lines.extend(self.children_pane(layout.width, agents_height));
        lines.extend(self.agent_preview(layout.width, preview_height));
        lines
    }

    /// Small tabs: stable header, `[STEPS] [AGENTS]` selector, and the single
    /// active navigation page above the footer.
    fn workflow_tabs(&self, layout: &BrowserLayout) -> Vec<String> {
        let mut lines = self.workflow_header(layout.width);
        lines.push(self.tab_selector());
        let pane_height = layout.content_height.saturating_sub(1);
        if self.state.focus() == TaskBrowserFocus::Steps {
            lines.extend(self.steps_pane(layout.width, pane_height));
        } else {
            lines.extend(self.children_pane(layout.width, pane_height));
        }
        lines
    }

    fn tab_selector(&self) -> String {
        if self.state.focus() == TaskBrowserFocus::Steps {
            format!(
                "{}  AGENTS",
                paint("[STEPS]", Style::default().fg(self.theme.brand))
            )
        } else {
            format!(
                "STEPS  {}",
                paint("[AGENTS]", Style::default().fg(self.theme.brand))
            )
        }
    }

    /// Lower selected-agent preview shared by the wide and stacked layouts:
    /// an identity divider, a `CURRENT ACTIVITY` divider, and wrapped
    /// activity (falling back to the terminal summary, then the state label).
    fn agent_preview(&self, width: usize, height: usize) -> Vec<String> {
        if height == 0 {
            return Vec::new();
        }
        let Some(child) = self.state.selected_workflow_child() else {
            let mut lines = vec![Self::divider(
                " SELECTED AGENT ",
                width,
                self.theme.overlay_border,
            )];
            lines.push(pad_to_width("No agent selected.", width));
            lines.resize(height, String::new());
            return lines;
        };
        let mut lines = vec![
            Self::divider(
                &format!(" SELECTED AGENT / {} ", child.title),
                width,
                self.theme.overlay_border,
            ),
            Self::divider(" CURRENT ACTIVITY ", width, self.theme.overlay_border),
        ];
        let activity = child
            .latest_activity
            .clone()
            .or_else(|| child.terminal_summary.clone())
            .unwrap_or_else(|| child.state.label().to_owned());
        for line in wrap_text(&activity, width.saturating_sub(2))
            .into_iter()
            .take(height.saturating_sub(2))
        {
            lines.push(format!(" {line}"));
        }
        lines.truncate(height);
        lines.resize(height, String::new());
        lines
    }

    /// Full-width Agent Details page: identity divider, meta line, wrapped
    /// scrollable current activity, then the terminal result, generated
    /// files, and actual usage sections when present.
    fn agent_details_page(&self, width: usize, height: usize) -> Vec<String> {
        let Some(child) = self.state.selected_workflow_child() else {
            return vec![pad_to_width("No agent selected.", width); height];
        };
        let mut lines = vec![Self::divider(
            &format!(" AGENT DETAILS / {} ", child.title),
            width,
            self.theme.overlay_border,
        )];
        let mut meta = format!(" {} {}", child.state.marker(), child.state.label());
        if let Some(role) = &child.role {
            let _ = write!(meta, " · {role}");
        }
        let _ = write!(meta, " · {}", child.elapsed);
        lines.push(truncate_width(&meta, width, "...", false));
        lines.push(Self::divider(
            " CURRENT ACTIVITY ",
            width,
            self.theme.overlay_border,
        ));

        let activity = child
            .latest_activity
            .clone()
            .or_else(|| child.terminal_summary.clone())
            .unwrap_or_else(|| child.state.label().to_owned());
        let wrapped = wrap_text(&activity, width.saturating_sub(2));

        let mut tail = Vec::new();
        if let Some(summary) = &child.terminal_summary {
            tail.push(Self::divider(
                " TERMINAL RESULT ",
                width,
                self.theme.overlay_border,
            ));
            tail.extend(
                wrap_text(summary, width.saturating_sub(2))
                    .into_iter()
                    .map(|line| format!(" {line}")),
            );
        }
        if !child.generated_files.is_empty() {
            tail.push(Self::divider(" FILES ", width, self.theme.overlay_border));
            tail.extend(child.generated_files.iter().map(|file| format!(" {file}")));
        }
        if let Some(usage) = &child.actual_usage {
            tail.push(Self::divider(
                " ACTUAL USAGE ",
                width,
                self.theme.overlay_border,
            ));
            tail.push(format!(" {usage}"));
        }

        // Slice the wrapped activity after wrapping; reserve one row for a
        // visible continuation indicator when rows remain below.
        let fixed = 3 + tail.len();
        let available = height.saturating_sub(fixed);
        let total = wrapped.len();
        let start = self.state.output_scroll().min(total.saturating_sub(1));
        let remaining = total.saturating_sub(start);
        let show = if remaining > available {
            available.saturating_sub(1)
        } else {
            available
        };
        for line in wrapped.iter().skip(start).take(show) {
            lines.push(format!(" {line}"));
        }
        if remaining > available {
            lines.push(paint(
                &pad_to_width(" more below (PgDn) ", width),
                Style::default().fg(self.theme.overlay_border),
            ));
        }
        lines.extend(tail);
        lines.truncate(height);
        lines.resize(height, String::new());
        lines
    }

    /// Full-width save/replacement or answer draft page. The exact draft
    /// strings are kept verbatim; only their placement (full-width inside the
    /// workflow frame) changed.
    fn draft_page(&self, width: usize, height: usize) -> Vec<String> {
        if let Some(draft) = self.state.save_draft() {
            if let Some(replacement) = &draft.replacement {
                return pane(
                    " Replace workflow? ",
                    width,
                    height,
                    &[
                        format!("Existing: {}", replacement.existing_display_name),
                        format!("New: {}", replacement.new_display_name),
                        format!("Location: {}", replacement.target_location),
                    ],
                    self.theme.overlay_border,
                );
            }
            return pane(
                " Save workflow ",
                width,
                height,
                &[
                    format!("Name: {}", draft.name),
                    format!("Save to: {}", draft.destination.label()),
                ],
                self.theme.overlay_border,
            );
        }
        if let Some(draft) = self.state.answer_draft() {
            let body = if draft.form.structured_fallback {
                let mut body = vec![draft.json_editor.clone()];
                body.extend(draft.field_errors.iter().cloned());
                body
            } else {
                draft.form.lines(
                    &draft.value,
                    &draft.field_errors,
                    draft.selected_field,
                    &draft.choice_indices,
                    &draft.branch_indices,
                )
            };
            return pane(" Answer ", width, height, &body, self.theme.overlay_border);
        }
        Vec::new()
    }

    fn render_browser(&self, width: usize, height: usize) -> Vec<String> {
        if height < 3 {
            return pad_height(
                vec![self.browser_header(width), self.browser_footer(width)],
                height,
            );
        }
        let layout = BrowserLayout::new(width, height, self.state);
        let mut lines = vec![String::new(); height];
        lines[0] = self.browser_header(width);
        let body = match layout.page {
            BrowserPage::Split => {
                let right_width = width.saturating_sub(layout.right_left);
                join_columns(
                    &[
                        self.tasks_pane(
                            layout.list_width,
                            layout.content_height,
                            layout.list_row_height,
                        ),
                        self.inspector(&layout, right_width, layout.content_height),
                    ],
                    &[layout.list_width, right_width],
                    layout.content_height,
                )
            }
            BrowserPage::List => {
                self.tasks_pane(width, layout.content_height, layout.list_row_height)
            }
            BrowserPage::Details => self.details_page(&layout),
            BrowserPage::Output => self.output_page(&layout),
            // The workflow pages are only reachable through `render_workflow`.
            BrowserPage::WorkflowWide
            | BrowserPage::WorkflowStacked
            | BrowserPage::WorkflowTabs => Vec::new(),
        };
        for (index, line) in body.into_iter().take(layout.content_height).enumerate() {
            lines[layout.content_top + index] = line;
        }
        lines[layout.footer_top] = self.browser_footer(width);
        lines
    }

    /// ` TASKS  ALL  ACTIVE  WORKFLOWS  {count} tasks ` with the active
    /// filter choice bracketed. Falls back to the visible list length when the
    /// snapshot carries no `total_matched`.
    fn browser_header(&self, width: usize) -> String {
        let count = self
            .state
            .snapshot()
            .total_matched
            .unwrap_or_else(|| self.state.visible_items().len());
        let choices = [
            TaskBrowserFilter::All,
            TaskBrowserFilter::Active,
            TaskBrowserFilter::Workflow,
        ]
        .into_iter()
        .map(|filter| {
            // The visible choice text is "WORKFLOWS"; `label()` is singular.
            let choice = match filter {
                TaskBrowserFilter::All => "ALL",
                TaskBrowserFilter::Active => "ACTIVE",
                TaskBrowserFilter::Workflow => "WORKFLOWS",
            };
            if filter == self.state.filter() {
                format!("[{choice}]")
            } else {
                choice.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("  ");
        let header = format!(" TASKS  {choices}  {count} tasks ");
        truncate_width(&header, width, "...", false)
    }

    fn browser_footer(&self, width: usize) -> String {
        let footer = if !self.state.task_details_open() {
            " Up/Down select  Tab filter  Enter open  O output  X stop  Esc close"
        } else if self.state.focus() == TaskBrowserFocus::Output {
            " O list  PgUp/PgDn scroll output  X stop  Esc back"
        } else {
            " O output  X stop  Esc back"
        };
        truncate_width(footer, width, "...", false)
    }

    fn tasks_pane(&self, width: usize, height: usize, row_height: usize) -> Vec<String> {
        let body = self
            .state
            .visible_items()
            .into_iter()
            .flat_map(|item| self.task_rows(item, width.saturating_sub(4), row_height))
            .collect::<Vec<_>>();
        let body = if body.is_empty() {
            vec!["No tasks.".to_owned()]
        } else {
            body
        };
        let color = if self.state.focus() == TaskBrowserFocus::Tasks {
            self.theme.brand
        } else {
            self.theme.overlay_border
        };
        pane(" Tasks ", width, height, &body, color)
    }

    /// Task rows with fixed priority: status marker, human handle/task ID,
    /// title, elapsed time. Elapsed is right-aligned only when it fits;
    /// anything below the truncation point is dropped before truncation.
    fn task_rows(&self, item: &TaskBrowserItem, width: usize, row_height: usize) -> Vec<String> {
        let selected = self.state.selected_task_id() == Some(item.id.as_str());
        let status_color = self.status_color(item.status);
        let marker = item.status.marker();
        let label = item.human_handle.as_deref().unwrap_or(item.id.as_str());
        let mut rows = Vec::new();
        if row_height > 1 {
            let first = truncate_width(&format!("{marker} {label}"), width, "...", false);
            let mut second = item.title.clone();
            if visible_width(&second) + 1 + visible_width(&item.elapsed) <= width {
                second = format!(
                    "{}{}",
                    pad_to_width(&second, width - visible_width(&item.elapsed)),
                    item.elapsed
                );
            }
            let second = truncate_width(&second, width, "...", false);
            rows.push(self.paint_task_row(&first, width, marker, status_color, selected));
            rows.push(self.paint_task_row(&second, width, "", status_color, selected));
        } else {
            let left = format!("{marker} {label}  {}", item.title);
            let mut text = left.clone();
            if visible_width(&left) + 1 + visible_width(&item.elapsed) <= width {
                text = format!(
                    "{}{}",
                    pad_to_width(&left, width - visible_width(&item.elapsed)),
                    item.elapsed
                );
            }
            let text = truncate_width(&text, width, "...", false);
            rows.push(self.paint_task_row(&text, width, marker, status_color, selected));
        }
        rows
    }

    /// Pad the plain row to the full row width first so the selection
    /// highlight has a stable width, then paint.
    fn paint_task_row(
        &self,
        plain: &str,
        width: usize,
        marker: &str,
        status_color: Color,
        selected: bool,
    ) -> String {
        let padded = pad_to_width(plain, width);
        if padded.len() < marker.len() {
            return padded; // too narrow to hold the marker; keep rendering panic-free
        }
        if selected {
            let marker_style = Style::default().fg(status_color).bg(self.theme.selected_bg);
            let rest_style = Style::default()
                .fg(self.theme.selected_fg)
                .bg(self.theme.selected_bg);
            format!(
                "{}{}",
                paint(marker, marker_style),
                paint(&padded[marker.len()..], rest_style)
            )
        } else {
            format!(
                "{}{}",
                paint(marker, Style::default().fg(status_color)),
                &padded[marker.len()..]
            )
        }
    }

    /// Wide-mode inspector column: identity rows, a Details section, and a
    /// Latest output preview section. Not boxed as one pane, but each section
    /// is fully bordered — `│`-ed content rows and a `└──┘` bottom — and
    /// follows the selection even when `task_details_open` is false. The
    /// section split comes from the single `BrowserLayout` geometry so hit
    /// testing and rendering agree.
    fn inspector(&self, layout: &BrowserLayout, width: usize, height: usize) -> Vec<String> {
        let Some(item) = self.state.selected_item() else {
            return vec![pad_to_width("No task selected.", width)];
        };
        let mut lines = Self::identity_rows(item, width);
        // Two identity rows, one divider and one bottom row per section:
        // 2 + 1 + details + 1 + 1 + output + 1 = height.
        let remaining = height.saturating_sub(6);
        // The LATEST OUTPUT divider starts at `inspector_output_top`; the
        // Details section is everything between it and the identity rows,
        // minus the section's bottom border row.
        let details_rows = layout
            .inspector_output_top
            .saturating_sub(layout.content_top + 3)
            .saturating_sub(1);
        let output_rows = remaining - details_rows;
        let details_color = if self.state.focus() == TaskBrowserFocus::Tasks {
            self.theme.brand
        } else {
            self.theme.overlay_border
        };
        let output_color = if self.state.focus() == TaskBrowserFocus::Output {
            self.theme.brand
        } else {
            self.theme.overlay_border
        };
        lines.push(Self::divider(" DETAILS ", width, details_color));
        lines.extend(
            Self::bordered_wrap(&item.detail_lines, width, details_color)
                .into_iter()
                .take(details_rows),
        );
        lines.push(Self::bottom(width, details_color));
        let output_all = Self::bordered_wrap(&item.preview_lines, width, output_color);
        let output_total = output_all.len();
        let skipped = output_all.iter().skip(self.state.output_scroll());
        let shown = skipped.len().min(output_rows);
        let mut output_title = " LATEST OUTPUT · Preview".to_owned();
        if output_total > shown {
            let _ = write!(output_title, " {shown}/{output_total}");
        }
        output_title.push(' ');
        lines.push(Self::divider(&output_title, width, output_color));
        lines.extend(
            output_all
                .into_iter()
                .skip(self.state.output_scroll())
                .take(output_rows),
        );
        lines.push(Self::bottom(width, output_color));
        lines
    }

    /// Medium/small full-width details page (details open, Tasks focus).
    fn details_page(&self, layout: &BrowserLayout) -> Vec<String> {
        let Some(item) = self.state.selected_item() else {
            return vec![pad_to_width("No task selected.", layout.width)];
        };
        let mut lines = Self::identity_rows(item, layout.width);
        lines.push(Self::divider(" DETAILS ", layout.width, self.theme.brand));
        let remaining = layout.content_height.saturating_sub(3);
        lines.extend(
            Self::wrapped_details(item, layout.width)
                .into_iter()
                .take(remaining),
        );
        lines
    }

    /// Medium/small full-width latest-output page (details open, Output focus).
    fn output_page(&self, layout: &BrowserLayout) -> Vec<String> {
        let Some(item) = self.state.selected_item() else {
            return vec![pad_to_width("No task selected.", layout.width)];
        };
        let output_all = Self::wrapped_output(item, layout.width);
        let output_total = output_all.len();
        let skipped = output_all.iter().skip(self.state.output_scroll());
        let shown = skipped.len().min(layout.content_height.saturating_sub(1));
        let mut title = " LATEST OUTPUT · Preview".to_owned();
        if output_total > shown {
            let _ = write!(title, " {shown}/{output_total}");
        }
        title.push(' ');
        let mut lines = vec![Self::divider(&title, layout.width, self.theme.brand)];
        lines.extend(
            output_all
                .into_iter()
                .skip(self.state.output_scroll())
                .take(layout.content_height.saturating_sub(1)),
        );
        lines
    }

    /// Two identity rows shared by the inspector and the details page:
    /// status + handle with right-aligned elapsed, then the truncated title.
    fn identity_rows(item: &TaskBrowserItem, width: usize) -> Vec<String> {
        let label = item.human_handle.as_deref().unwrap_or(item.id.as_str());
        let mut first = format!("{}  {label}", item.status.label());
        if visible_width(&first) + 1 + visible_width(&item.elapsed) <= width {
            first = format!(
                "{}{}",
                pad_to_width(&first, width - visible_width(&item.elapsed)),
                item.elapsed
            );
        }
        let first = truncate_width(&first, width, "...", false);
        let second = truncate_width(&item.title, width, "...", false);
        vec![first, second]
    }

    /// Wrap detail lines to the body width (indented one space). Wrapping
    /// never ellipsizes; viewport slicing happens after wrapping.
    fn wrapped_details(item: &TaskBrowserItem, width: usize) -> Vec<String> {
        item.detail_lines
            .iter()
            .flat_map(|line| wrap_text(line, width.saturating_sub(1)))
            .map(|line| format!(" {line}"))
            .collect()
    }

    /// Wrap preview lines to the body width (indented one space) so a bounded
    /// output region shows complete wrapped lines, never a hidden suffix.
    fn wrapped_output(item: &TaskBrowserItem, width: usize) -> Vec<String> {
        item.preview_lines
            .iter()
            .flat_map(|line| wrap_text(line, width.saturating_sub(1)))
            .map(|line| format!(" {line}"))
            .collect()
    }

    /// Wrap `source` lines to the bordered body width and render each wrapped
    /// line as a `pane()`-style content row — `│ {line}` padded to `width - 1`
    /// then `│`, exactly `width` columns wide. Wrapping never ellipsizes;
    /// viewport slicing happens after wrapping.
    fn bordered_wrap(source: &[String], width: usize, color: Color) -> Vec<String> {
        let body = width.saturating_sub(4);
        let style = Style::default().fg(color);
        source
            .iter()
            .flat_map(|line| wrap_text(line, body))
            .map(|line| {
                format!(
                    "{} {}{} {}",
                    paint("│", style),
                    line,
                    " ".repeat(body.saturating_sub(visible_width(&line))),
                    paint("│", style)
                )
            })
            .collect()
    }

    /// `└──...──┘` bottom border of a section, exactly `width` columns wide.
    fn bottom(width: usize, color: Color) -> String {
        paint(
            &format!("└{}┘", "─".repeat(width.saturating_sub(2))),
            Style::default().fg(color),
        )
    }

    /// `┌{title}─...─┐` divider painted with the section color, exactly
    /// `width` columns wide.
    fn divider(title: &str, width: usize, color: Color) -> String {
        let inner = width.saturating_sub(2);
        let title = truncate_width(title, inner, "", false);
        paint(
            &format!(
                "┌{title}{}┐",
                "─".repeat(inner.saturating_sub(visible_width(&title)))
            ),
            Style::default().fg(color),
        )
    }

    fn status_color(&self, status: TaskBrowserStatus) -> Color {
        match status {
            TaskBrowserStatus::Running => self.theme.status_warn,
            TaskBrowserStatus::Waiting => self.theme.status_pending,
            TaskBrowserStatus::Paused | TaskBrowserStatus::Cancelled => self.theme.status_cancelled,
            TaskBrowserStatus::Completed => self.theme.status_ok,
            TaskBrowserStatus::Failed
            | TaskBrowserStatus::TimedOut
            | TaskBrowserStatus::ResourceLimited
            | TaskBrowserStatus::ParentExited => self.theme.status_error,
        }
    }
}

fn format_elapsed(ms: u64) -> String {
    let seconds = ms / 1_000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn pane(title: &str, width: usize, height: usize, body: &[String], color: Color) -> Vec<String> {
    if height == 0 {
        return Vec::new();
    }
    if width < 3 {
        return vec![truncate_width(title, width, "", false); height];
    }
    if height == 1 {
        return vec![truncate_width(title.trim(), width, "...", false)];
    }
    let inner = width.saturating_sub(2);
    let style = Style::default().fg(color);
    let mut lines = vec![paint(&titled_top(title, inner), style)];
    for row in 0..height.saturating_sub(2) {
        let text = body.get(row).map_or("", String::as_str);
        let text = truncate_width(text, inner.saturating_sub(2), "...", false);
        lines.push(format!(
            "{} {}{} {}",
            paint("│", style),
            text,
            " ".repeat(inner.saturating_sub(2).saturating_sub(visible_width(&text))),
            paint("│", style)
        ));
    }
    lines.push(paint(&format!("└{}┘", "─".repeat(inner)), style));
    lines
}

fn titled_top(title: &str, inner: usize) -> String {
    let title = truncate_width(title, inner, "", false);
    format!(
        "┌{title}{}┐",
        "─".repeat(inner.saturating_sub(visible_width(&title)))
    )
}

fn join_columns(columns: &[Vec<String>], widths: &[usize], height: usize) -> Vec<String> {
    (0..height)
        .map(|row| {
            columns
                .iter()
                .zip(widths)
                .map(|(column, width)| {
                    truncate_width(
                        column.get(row).map_or("", String::as_str),
                        *width,
                        "...",
                        false,
                    )
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

fn pad_height(mut lines: Vec<String>, height: usize) -> Vec<String> {
    lines.truncate(height);
    while lines.len() < height {
        lines.push(String::new());
    }
    lines
}
