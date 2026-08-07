use std::fmt::Write as _;

use crate::primitive::theme::TuiTheme;
use crate::primitive::{
    Color, Style, pad_to_width, paint, truncate_width, visible_width, wrap_text,
};

use super::{
    state::{TaskBrowserFilter, TaskBrowserFocus, TaskBrowserState},
    view::{TaskBrowserItem, TaskBrowserStatus},
};

/// Below this content width the browser must render at most two lines per task
/// row; at or above it, one line per task row.
const MEDIUM_MIN_COLUMNS: usize = 70;
/// At or above this content width the browser splits into list + inspector.
const WIDE_MIN_COLUMNS: usize = 100;
const MIN_TASK_LIST_COLUMNS: usize = 30;
const MAX_TASK_LIST_COLUMNS: usize = 42;

/// Which full-page or split-page surface the general browser shows.
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
}

/// Single-source geometry for the general task browser frame.
///
/// Every breakpoint decision and rectangle in `render_browser` comes from this
/// one value so pointer hit testing can reuse the exact same arithmetic.
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
}

impl BrowserLayout {
    fn new(width: usize, height: usize, state: &TaskBrowserState) -> Self {
        let content_height = height.saturating_sub(2).max(1);
        let list_width = (width / 3).clamp(MIN_TASK_LIST_COLUMNS, MAX_TASK_LIST_COLUMNS);
        let page = if width >= WIDE_MIN_COLUMNS {
            BrowserPage::Split
        } else if !state.task_details_open() {
            BrowserPage::List
        } else if state.focus() == TaskBrowserFocus::Output {
            BrowserPage::Output
        } else {
            BrowserPage::Details
        };
        Self {
            width,
            content_top: 1,
            content_height,
            footer_top: height.saturating_sub(1),
            page,
            list_width,
            right_left: list_width + 1,
            list_row_height: if width < MEDIUM_MIN_COLUMNS { 2 } else { 1 },
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

    fn render_workflow(&self, width: usize, height: usize) -> Vec<String> {
        if height < 4 {
            return pad_height(
                vec![self.workflow_header(width), self.workflow_footer(width)],
                height,
            );
        }
        let content_height = height.saturating_sub(2);
        let item = self
            .state
            .workflow_item()
            .expect("workflow view requires item");
        let mut lines = vec![self.workflow_header(width)];
        if width >= 100 {
            let top = content_height.saturating_sub(5).max(3);
            let steps_width = (width / 3).clamp(24, 42);
            let agents_width = width.saturating_sub(steps_width + 1);
            lines.extend(join_columns(
                &[
                    self.steps_pane(steps_width, top),
                    self.children_pane(agents_width, top),
                ],
                &[steps_width, agents_width],
                top,
            ));
            lines.extend(self.details_pane(width, content_height.saturating_sub(top), item));
        } else if width >= 70 {
            let steps_height = (content_height / 3).max(3);
            let agents_height = (content_height / 3).max(3);
            lines.extend(self.steps_pane(width, steps_height));
            lines.extend(self.children_pane(width, agents_height));
            lines.extend(self.details_pane(
                width,
                content_height.saturating_sub(steps_height + agents_height),
                item,
            ));
        } else {
            let navigation_height = content_height.saturating_sub(4).max(2);
            if self.state.focus() == TaskBrowserFocus::Steps {
                lines.extend(self.steps_pane(width, navigation_height));
            } else {
                lines.extend(self.children_pane(width, navigation_height));
            }
            lines.extend(self.details_pane(
                width,
                content_height.saturating_sub(navigation_height),
                item,
            ));
        }
        lines.push(self.workflow_footer(width));
        pad_height(lines, height)
    }

    fn workflow_header(&self, width: usize) -> String {
        let item = self
            .state
            .workflow_item()
            .expect("workflow view requires item");
        let workflow = item.workflow.as_ref().expect("workflow item carries meta");
        let mut header = format!(
            " {}  {}  {}  {}",
            workflow.display_name,
            item.status.label(),
            format_elapsed(workflow.elapsed_ms),
            workflow.purpose
        );
        if workflow.pending_user.is_some() {
            header.push_str("  Needs input");
        }
        truncate_width(&header, width, "...", false)
    }

    fn workflow_footer(&self, width: usize) -> String {
        if let Some(task_id) = self.state.stop_confirmation_task_id() {
            return truncate_width(
                &format!(" Stop {task_id}?  Enter confirm  Esc back"),
                width,
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
                width,
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
            return truncate_width(help, width, "...", false);
        }
        let workflow = self
            .state
            .workflow_item()
            .and_then(|item| item.workflow.as_ref());
        let mut footer = " Tab switch  Enter details  P pause/resume  X stop  Esc back".to_owned();
        if workflow.is_some_and(|value| value.inline_unsaved) {
            footer.push_str("  S save");
        }
        truncate_width(&footer, width, "...", false)
    }

    fn steps_pane(&self, width: usize, height: usize) -> Vec<String> {
        let selected = self.state.selected_workflow_step();
        let body = self
            .state
            .workflow_item()
            .and_then(|item| item.workflow.as_ref())
            .map(|workflow| {
                workflow
                    .steps
                    .iter()
                    .map(|step| {
                        let pointer = if selected == Some(step) { ">" } else { " " };
                        format!(
                            "{pointer} {} {}  {}/{}/{}",
                            step.state.marker(),
                            step.title,
                            step.done_count,
                            step.working_count,
                            step.queued_count,
                        )
                    })
                    .collect()
            })
            .unwrap_or_else(|| vec!["No steps yet.".to_owned()]);
        pane(" Steps ", width, height, &body, self.theme.overlay_border)
    }

    fn children_pane(&self, width: usize, height: usize) -> Vec<String> {
        let selected = self.state.selected_workflow_child();
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
                        let pointer = if selected == Some(child) { ">" } else { " " };
                        let role = child
                            .role
                            .as_deref()
                            .map_or(String::new(), |role| format!(" [{role}]"));
                        let activity = child
                            .latest_activity
                            .as_deref()
                            .map_or(String::new(), |value| format!("  {value}"));
                        format!(
                            "{pointer} {} {}{}  {}{}",
                            child.state.marker(),
                            child.title,
                            role,
                            child.elapsed,
                            activity
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if body.is_empty() {
            body.push("No agents in this step.".to_owned());
        }
        pane(" Agents ", width, height, &body, self.theme.overlay_border)
    }

    fn details_pane(&self, width: usize, height: usize, item: &TaskBrowserItem) -> Vec<String> {
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
        if self.state.child_details_open()
            && let Some(child) = self.state.selected_workflow_child()
        {
            let mut body = vec![
                format!("Agent: {}", child.title),
                format!("State: {}", child.state.marker()),
            ];
            if let Some(usage) = &child.actual_usage {
                body.push(format!("Usage: {usage}"));
            }
            body.push(format!("Elapsed: {}", child.elapsed));
            if let Some(role) = &child.role {
                body.push(format!("Role: {role}"));
            }
            if let Some(activity) = &child.latest_activity {
                body.push(format!("Activity: {activity}"));
            }
            if let Some(summary) = &child.terminal_summary {
                body.push(format!("Result: {summary}"));
            }
            return pane(" Details ", width, height, &body, self.theme.overlay_border);
        }
        pane(
            " Details ",
            width,
            height,
            &item.detail_lines,
            self.theme.overlay_border,
        )
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
                        self.inspector(right_width, layout.content_height),
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
    /// Latest output preview section. Not boxed; follows the selection even
    /// when `task_details_open` is false.
    fn inspector(&self, width: usize, height: usize) -> Vec<String> {
        let Some(item) = self.state.selected_item() else {
            return vec![pad_to_width("No task selected.", width)];
        };
        let mut lines = Self::identity_rows(item, width);
        let remaining = height.saturating_sub(4);
        let details_rows = remaining / 2;
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
            Self::wrapped_details(item, width)
                .into_iter()
                .take(details_rows),
        );
        let output_all = Self::wrapped_output(item, width);
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
