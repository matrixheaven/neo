use std::fmt::Write as _;

use crate::primitive::theme::TuiTheme;
use crate::primitive::{Color, Style, paint, truncate_width, visible_width};

use super::{
    state::{TaskBrowserFocus, TaskBrowserState},
    view::TaskBrowserItem,
};

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
        let content_height = height.saturating_sub(2).max(1);
        let mut lines = vec![self.browser_header(width)];
        if self.state.task_details_open() && width >= 70 {
            let tasks_width = (width / 3).clamp(24, 42);
            let details_width = width.saturating_sub(tasks_width + 1);
            let details_height = content_height / 2;
            let mut details = self.task_details_pane(details_width, details_height);
            details.extend(
                self.task_output_pane(details_width, content_height.saturating_sub(details_height)),
            );
            lines.extend(join_columns(
                &[self.tasks_pane(tasks_width, content_height), details],
                &[tasks_width, details_width],
                content_height,
            ));
        } else {
            lines.extend(self.tasks_pane(width, content_height));
        }
        let footer = if self.state.task_details_open() {
            " O switch output  PgUp/PgDn scroll output  X stop  Esc back"
        } else {
            " Tab filter  Enter details  O output  X stop  Esc close"
        };
        lines.push(truncate_width(footer, width, "...", false));
        pad_height(lines, height)
    }

    fn browser_header(&self, width: usize) -> String {
        let mut header = format!(" TASK BROWSER  filter={}", self.state.filter().label());
        let count = self.state.visible_items().len();
        let _ = write!(header, "  {count} tasks");
        truncate_width(&header, width, "...", false)
    }

    fn tasks_pane(&self, width: usize, height: usize) -> Vec<String> {
        let body = self
            .state
            .visible_items()
            .into_iter()
            .map(|item| self.task_row(item, width.saturating_sub(4)))
            .collect::<Vec<_>>();
        let body = if body.is_empty() {
            vec!["No tasks.".to_owned()]
        } else {
            body
        };
        pane(" Tasks ", width, height, &body, self.theme.overlay_border)
    }

    fn task_details_pane(&self, width: usize, height: usize) -> Vec<String> {
        let body = self
            .state
            .selected_item()
            .map(|item| item.detail_lines.clone())
            .unwrap_or_else(|| vec!["No task selected.".to_owned()]);
        pane(" Details ", width, height, &body, self.theme.overlay_border)
    }

    fn task_output_pane(&self, width: usize, height: usize) -> Vec<String> {
        let body = self
            .state
            .selected_item()
            .map(|item| {
                item.preview_lines
                    .iter()
                    .skip(self.state.output_scroll())
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        pane(" Output ", width, height, &body, self.theme.overlay_border)
    }

    fn task_row(&self, item: &TaskBrowserItem, width: usize) -> String {
        let pointer = if self.state.selected_task_id() == Some(item.id.as_str()) {
            ">"
        } else {
            " "
        };
        let label = item.human_handle.as_deref().unwrap_or(item.id.as_str());
        truncate_width(
            &format!(
                "{pointer} {} {}  {}",
                item.status.marker(),
                label,
                item.title
            ),
            width,
            "...",
            false,
        )
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
