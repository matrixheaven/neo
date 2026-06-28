use crate::primitive::{Color, Style, paint, truncate_width, visible_width};
use crate::primitive::theme::TuiTheme;

use super::{
    state::{TaskBrowserFilter, TaskBrowserState},
    view::{TaskBrowserItem, TaskBrowserStatus},
};

const MIN_WIDE_WIDTH: usize = 72;
const WIDE_GAP: usize = 1;
const FOOTER: &str = " ↑↓ select   Enter/O output   S stop   R refresh   Tab filter   Q/Esc close";
const COMPACT_FOOTER: &str = " Q/Esc close   Tab filter   S stop   R refresh";

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
        if height < 6 {
            return self.render_tiny(width, height);
        }
        if width >= MIN_WIDE_WIDTH {
            self.render_wide(width, height)
        } else {
            self.render_narrow(width, height)
        }
    }

    fn render_wide(&self, width: usize, height: usize) -> Vec<String> {
        let header_height = 2usize;
        let footer_height = 1usize;
        let content_height = height.saturating_sub(header_height + footer_height).max(3);
        let left_width = (width / 3).clamp(30, 42);
        let right_width = width.saturating_sub(left_width + WIDE_GAP);
        let detail_height = (content_height / 2).max(4).min(content_height);
        let preview_height = content_height.saturating_sub(detail_height).max(3);

        let left = self.tasks_pane(left_width, content_height);
        let mut right = self.detail_pane(right_width, detail_height);
        right.extend(self.preview_pane(right_width, preview_height));
        right.truncate(content_height);
        while right.len() < content_height {
            right.push(" ".repeat(right_width));
        }

        let mut lines = Vec::with_capacity(height);
        lines.push(self.header(width));
        lines.push(String::new());
        for row in 0..content_height {
            lines.push(fit_line(
                &format!("{}{}{}", left[row], " ".repeat(WIDE_GAP), right[row]),
                width,
            ));
        }
        lines.push(self.footer(width));
        pad_height(lines, height)
    }

    fn render_narrow(&self, width: usize, height: usize) -> Vec<String> {
        let footer_height = 1usize;
        let content_height = height.saturating_sub(2 + footer_height).max(3);
        let list_height = (content_height / 2).max(4).min(content_height);
        let detail_height = content_height.saturating_sub(list_height);

        let mut lines = Vec::with_capacity(height);
        lines.push(self.header(width));
        lines.push(String::new());
        lines.extend(self.tasks_pane(width, list_height));
        if detail_height > 0 {
            lines.extend(self.detail_pane(width, detail_height.max(3)));
        }
        lines.push(self.footer(width));
        pad_height(lines, height)
    }

    fn render_tiny(&self, width: usize, height: usize) -> Vec<String> {
        let mut lines = Vec::with_capacity(height);
        lines.push(self.header(width));
        if height > 2 {
            lines.push(truncate_width(
                "Tasks: use a taller terminal",
                width,
                "...",
                false,
            ));
        }
        if height > 1 {
            lines.push(self.footer(width));
        }
        pad_height(lines, height)
    }

    fn header(&self, width: usize) -> String {
        let visible = self.state.visible_items();
        let running = visible
            .iter()
            .filter(|item| item.status == TaskBrowserStatus::Running)
            .count();
        let waiting = visible
            .iter()
            .filter(|item| item.status == TaskBrowserStatus::Waiting)
            .count();
        let completed = visible
            .iter()
            .filter(|item| item.status == TaskBrowserStatus::Completed)
            .count();
        let interrupted = visible
            .iter()
            .filter(|item| item.status.is_interrupted())
            .count();
        let mut header = format!(" TASK BROWSER  filter={}", self.state.filter().label());
        if running > 0 {
            header.push_str(&format!("  {running} running"));
        }
        if waiting > 0 {
            header.push_str(&format!("  {waiting} waiting"));
        }
        if completed > 0 {
            header.push_str(&format!("  {completed} completed"));
        }
        if interrupted > 0 {
            header.push_str(&format!("  {interrupted} interrupted"));
        }
        header.push_str(&format!("  {} total", visible.len()));
        truncate_width(&header, width, "...", false)
    }

    fn footer(&self, width: usize) -> String {
        if let Some(task_id) = self.state.stop_confirmation_task_id() {
            return truncate_width(
                &format!(" Stop {task_id}?  Enter confirm   Esc cancel"),
                width,
                "...",
                false,
            );
        }
        if let Some(message) = self.state.footer_message() {
            return truncate_width(&format!(" {message}"), width, "...", false);
        }
        let footer = if width < visible_width(FOOTER) {
            COMPACT_FOOTER
        } else {
            FOOTER
        };
        truncate_width(footer, width, "...", false)
    }

    fn tasks_pane(&self, width: usize, height: usize) -> Vec<String> {
        let title = format!(" Tasks [{}] ", self.state.filter().pane_label());
        let visible = self.state.visible_items();
        let mut body = Vec::new();
        if visible.is_empty() {
            let empty = match self.state.filter() {
                TaskBrowserFilter::All => "No background tasks in this session.",
                TaskBrowserFilter::Active => "No active tasks. Tab = show all.",
            };
            body.extend(wrap_words(empty, width.saturating_sub(4)));
        } else {
            let max_rows = height.saturating_sub(2);
            let start = self
                .state
                .list_scroll()
                .min(visible.len().saturating_sub(1));
            for item in visible.into_iter().skip(start).take(max_rows) {
                body.push(self.task_row(item, width.saturating_sub(4)));
            }
        }
        pane(&title, width, height, body, self.theme.overlay_border)
    }

    fn detail_pane(&self, width: usize, height: usize) -> Vec<String> {
        let body = self.state.selected_item().map_or_else(
            || vec!["Select a task from the list.".to_owned()],
            |item| item.detail_lines.clone(),
        );
        pane(" Detail ", width, height, body, self.theme.overlay_border)
    }

    fn preview_pane(&self, width: usize, height: usize) -> Vec<String> {
        let body = self.state.selected_item().map_or_else(
            || vec!["No task selected.".to_owned()],
            |item| {
                if item.preview_lines.is_empty() {
                    vec!["No output yet.".to_owned()]
                } else {
                    item.preview_lines
                        .iter()
                        .skip(self.state.output_scroll())
                        .cloned()
                        .collect()
                }
            },
        );
        pane(
            " Preview Output ",
            width,
            height,
            body,
            self.theme.overlay_border,
        )
    }

    fn task_row(&self, item: &TaskBrowserItem, width: usize) -> String {
        let pointer = if self.state.selected_task_id() == Some(item.id.as_str()) {
            ">"
        } else {
            " "
        };
        let raw = format!(
            "{pointer} {} {}  {:<9} {}",
            item.status.marker(),
            item.id,
            item.status.label(),
            item.title
        );
        truncate_width(&raw, width, "...", false)
    }
}

fn pane(title: &str, width: usize, height: usize, body: Vec<String>, color: Color) -> Vec<String> {
    if width < 2 || height == 0 {
        return Vec::new();
    }
    if height == 1 {
        return vec![truncate_width(title.trim(), width, "...", false)];
    }

    let inner = width.saturating_sub(2);
    let border_style = Style::default().fg(color);
    let mut lines = Vec::with_capacity(height);
    lines.push(paint(&titled_top(title, inner), border_style));

    let content_rows = height.saturating_sub(2);
    for row in 0..content_rows {
        let raw = body.get(row).map_or("", String::as_str);
        lines.push(side_line(raw, inner, border_style));
    }
    lines.push(paint(&format!("└{}┘", "─".repeat(inner)), border_style));
    lines
}

fn titled_top(title: &str, inner: usize) -> String {
    let title_width = visible_width(title);
    if title_width >= inner {
        return format!("┌{}┐", truncate_width(title, inner, "", false));
    }
    format!("┌{title}{}┐", "─".repeat(inner - title_width))
}

fn side_line(raw: &str, inner: usize, border_style: Style) -> String {
    let content_width = inner.saturating_sub(2);
    let text = truncate_width(raw, content_width, "...", false);
    let padding = " ".repeat(content_width.saturating_sub(visible_width(&text)));
    format!(
        "{} {}{} {}",
        paint("│", border_style),
        text,
        padding,
        paint("│", border_style)
    )
}

fn wrap_words(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let next_width = if current.is_empty() {
            visible_width(word)
        } else {
            visible_width(&current) + 1 + visible_width(word)
        };
        if next_width > width && !current.is_empty() {
            lines.push(current);
            current = word.to_owned();
        } else if current.is_empty() {
            current = word.to_owned();
        } else {
            current.push(' ');
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn fit_line(line: &str, width: usize) -> String {
    truncate_width(line, width, "...", false)
}

fn pad_height(mut lines: Vec<String>, height: usize) -> Vec<String> {
    lines.truncate(height);
    while lines.len() < height {
        lines.push(String::new());
    }
    lines
}
