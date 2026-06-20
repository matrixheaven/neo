use crate::chrome::TuiTheme;
use crate::components::wrap_width;
use crate::{
    ansi::{Style, paint},
    components::truncate_width,
};

/// Maximum number of todo items visible without truncation.
pub const MAX_VISIBLE_TODOS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoDisplayStatus {
    Pending,
    InProgress,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoDisplayItem {
    pub title: String,
    pub status: TodoDisplayStatus,
}

impl TodoDisplayItem {
    #[must_use]
    pub fn new(title: impl Into<String>, status: TodoDisplayStatus) -> Self {
        Self {
            title: title.into(),
            status,
        }
    }
}

/// Smart truncation algorithm matching Neo's `todo-panel.ts`.
///
/// 1. Include ALL `in_progress` items (capped at `max_visible`).
/// 2. If slots remain: include 1 latest done item.
/// 3. Fill remaining with earliest pending items.
/// 4. Re-sort to original order.
///
/// Returns the **indices** of visible items.
#[must_use]
pub fn select_visible_todos(todos: &[TodoDisplayItem], max_visible: usize) -> Vec<usize> {
    if todos.is_empty() || max_visible == 0 {
        return Vec::new();
    }
    if todos.len() <= max_visible {
        return (0..todos.len()).collect();
    }

    let mut selected: Vec<usize> = Vec::new();

    // 1. All in_progress (capped).
    for (i, todo) in todos.iter().enumerate() {
        if selected.len() >= max_visible {
            break;
        }
        if todo.status == TodoDisplayStatus::InProgress {
            selected.push(i);
        }
    }

    // 2. One latest done.
    if selected.len() < max_visible
        && let Some(done_idx) = todos
            .iter()
            .enumerate()
            .rev()
            .find(|(_, t)| t.status == TodoDisplayStatus::Done)
            .map(|(i, _)| i)
        && !selected.contains(&done_idx)
    {
        selected.push(done_idx);
    }

    // 3. Earliest pending to fill.
    for (i, todo) in todos.iter().enumerate() {
        if selected.len() >= max_visible {
            break;
        }
        if todo.status == TodoDisplayStatus::Pending && !selected.contains(&i) {
            selected.push(i);
        }
    }

    // 4. Re-sort.
    selected.sort_unstable();
    selected
}

pub struct TodoPanel<'a> {
    todos: &'a [TodoDisplayItem],
    theme: TuiTheme,
}

impl<'a> TodoPanel<'a> {
    #[must_use]
    pub fn new(todos: &'a [TodoDisplayItem]) -> Self {
        Self {
            todos,
            theme: TuiTheme::default(),
        }
    }

    #[must_use]
    pub const fn with_theme(mut self, theme: TuiTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Compute the rendered height of the panel (including border) for a
    /// given terminal width.
    #[must_use]
    pub fn height(&self, width: u16) -> u16 {
        if self.todos.is_empty() {
            return 0;
        }
        let visible = select_visible_todos(self.todos, MAX_VISIBLE_TODOS);
        let inner_width = usize::from(width.saturating_sub(6).max(1));
        let item_lines: usize = visible
            .iter()
            .map(|&i| wrap_width(&self.todos[i].title, inner_width).len().max(1))
            .sum();
        let hidden = self.todos.len() > visible.len();
        let total = 2 + 1 + item_lines + usize::from(hidden);
        u16::try_from(total).unwrap_or(u16::MAX)
    }

    #[must_use]
    pub fn render(&self, width: usize) -> Vec<String> {
        if self.todos.is_empty() {
            return Vec::new();
        }

        let visible = select_visible_todos(self.todos, MAX_VISIBLE_TODOS);
        let inner_width = width.saturating_sub(6).max(1);
        let mut lines = vec![
            paint(
                &"\u{2500}".repeat(width),
                Style::default().fg(self.theme.text_muted),
            ),
            paint("  Todo", Style::default().fg(self.theme.brand).bold()),
        ];

        for &index in &visible {
            lines.extend(render_item(&self.todos[index], inner_width, self.theme));
        }

        let hidden = self.todos.len().saturating_sub(visible.len());
        if hidden > 0 {
            lines.push(paint(
                &format!("  \u{2026} +{hidden} more"),
                Style::default().fg(self.theme.text_muted),
            ));
        }

        lines
            .into_iter()
            .map(|line| truncate_width(&line, width, "", false))
            .collect()
    }
}

fn render_item(item: &TodoDisplayItem, inner_width: usize, theme: TuiTheme) -> Vec<String> {
    let marker = match item.status {
        TodoDisplayStatus::Pending => "\u{25CB}",
        TodoDisplayStatus::InProgress => "\u{25CF}",
        TodoDisplayStatus::Done => "\u{2713}",
    };
    let marker_style = match item.status {
        TodoDisplayStatus::Pending => Style::default().fg(theme.text_muted),
        TodoDisplayStatus::InProgress => Style::default().fg(theme.brand).bold(),
        TodoDisplayStatus::Done => Style::default().fg(theme.status_ok),
    };
    let title_style = match item.status {
        TodoDisplayStatus::Pending => Style::default().fg(theme.text_primary),
        TodoDisplayStatus::InProgress => Style::default().fg(theme.text_primary).bold(),
        TodoDisplayStatus::Done => Style::default().fg(theme.text_muted).crossed_out(),
    };

    let wrapped = wrap_width(&item.title, inner_width);
    if wrapped.is_empty() {
        return vec![format!("  {} ", paint(marker, marker_style))];
    }

    let mut rows = Vec::with_capacity(wrapped.len());
    for (line_index, line) in wrapped.into_iter().enumerate() {
        if line_index == 0 {
            rows.push(format!(
                "  {} {}",
                paint(marker, marker_style),
                paint(&line, title_style)
            ));
        } else {
            rows.push(format!("    {}", paint(&line, title_style)));
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str, status: TodoDisplayStatus) -> TodoDisplayItem {
        TodoDisplayItem::new(title, status)
    }

    #[test]
    fn no_truncation_when_under_limit() {
        let todos = vec![
            item("a", TodoDisplayStatus::Pending),
            item("b", TodoDisplayStatus::InProgress),
            item("c", TodoDisplayStatus::Done),
        ];
        let visible = select_visible_todos(&todos, 5);
        assert_eq!(visible, vec![0, 1, 2]);
    }

    #[test]
    fn prioritises_in_progress() {
        let todos = vec![
            item("a", TodoDisplayStatus::Pending),
            item("b", TodoDisplayStatus::Pending),
            item("c", TodoDisplayStatus::InProgress),
            item("d", TodoDisplayStatus::Pending),
            item("e", TodoDisplayStatus::Done),
            item("f", TodoDisplayStatus::Done),
            item("g", TodoDisplayStatus::Pending),
        ];
        let visible = select_visible_todos(&todos, 5);
        // c (in_progress), e or f (latest done), then earliest pending a, b, d
        assert!(visible.contains(&2)); // in_progress
        assert_eq!(visible.len(), 5);
        // Should be sorted
        assert_eq!(visible, vec![0, 1, 2, 3, 5]); // a, b, c, d, f(latest done)
    }

    #[test]
    fn all_done_returns_all_when_under_limit() {
        let todos = vec![
            item("a", TodoDisplayStatus::Done),
            item("b", TodoDisplayStatus::Done),
        ];
        let visible = select_visible_todos(&todos, 5);
        assert_eq!(visible, vec![0, 1]);
    }

    #[test]
    fn all_pending_truncates_earliest() {
        let todos: Vec<TodoDisplayItem> = (0..7)
            .map(|i| item(&format!("task-{i}"), TodoDisplayStatus::Pending))
            .collect();
        let visible = select_visible_todos(&todos, 5);
        assert_eq!(visible.len(), 5);
        assert_eq!(visible, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn empty_input_returns_empty() {
        let todos: Vec<TodoDisplayItem> = vec![];
        let visible = select_visible_todos(&todos, 5);
        assert!(visible.is_empty());
    }

    #[test]
    fn max_visible_zero_returns_empty() {
        let todos = vec![item("a", TodoDisplayStatus::InProgress)];
        let visible = select_visible_todos(&todos, 0);
        assert!(visible.is_empty());
    }

    #[test]
    fn exactly_max_visible() {
        let todos: Vec<TodoDisplayItem> = (0..5)
            .map(|i| item(&format!("t{i}"), TodoDisplayStatus::Pending))
            .collect();
        let visible = select_visible_todos(&todos, 5);
        assert_eq!(visible.len(), 5);
        assert_eq!(visible, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn render_outputs_header_status_rows_and_hidden_count() {
        let todos = vec![
            item("old done task", TodoDisplayStatus::Done),
            item("active task", TodoDisplayStatus::InProgress),
            item("pending one", TodoDisplayStatus::Pending),
            item("pending two", TodoDisplayStatus::Pending),
            item("pending three", TodoDisplayStatus::Pending),
            item("latest done task", TodoDisplayStatus::Done),
        ];

        let lines = TodoPanel::new(&todos).render(40);
        let plain = lines
            .iter()
            .map(|line| crate::ansi::strip_ansi(line))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(plain.contains("Todo"));
        assert!(plain.contains("\u{2713} latest done task"));
        assert!(plain.contains("\u{25CF} active task"));
        assert!(plain.contains("\u{25CB} pending one"));
        assert!(plain.contains("\u{2026} +1 more"));
    }
}
