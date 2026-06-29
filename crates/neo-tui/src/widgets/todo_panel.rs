use crate::primitive::theme::TuiTheme;
use crate::primitive::wrap_width;
use crate::primitive::{Style, paint, truncate_width};
use std::collections::BTreeSet;

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TodoHiddenCounts {
    pub done: usize,
    pub in_progress: usize,
    pub pending: usize,
}

impl TodoHiddenCounts {
    fn add(&mut self, status: TodoDisplayStatus) {
        match status {
            TodoDisplayStatus::Pending => self.pending += 1,
            TodoDisplayStatus::InProgress => self.in_progress += 1,
            TodoDisplayStatus::Done => self.done += 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleTodos {
    pub indices: Vec<usize>,
    pub hidden: usize,
    pub hidden_counts: TodoHiddenCounts,
}

/// Smart truncation algorithm matching Kimi's collapsed todo selector.
///
/// 1. Include ALL `in_progress` items (capped at `max_visible`).
/// 2. If slots remain: balance latest done items with earliest pending items.
/// 3. Re-sort to original order and count hidden statuses.
#[must_use]
pub fn select_visible_todos(todos: &[TodoDisplayItem], max_visible: usize) -> VisibleTodos {
    if todos.is_empty() || max_visible == 0 {
        return visible_todos(Vec::new(), todos);
    }
    if todos.len() <= max_visible {
        return VisibleTodos {
            indices: (0..todos.len()).collect(),
            hidden: 0,
            hidden_counts: TodoHiddenCounts::default(),
        };
    }

    let mut selected: Vec<usize> = Vec::new();
    let mut in_progress = Vec::new();
    let mut pending = Vec::new();
    let mut done = Vec::new();

    for (index, todo) in todos.iter().enumerate() {
        match todo.status {
            TodoDisplayStatus::Pending => pending.push(index),
            TodoDisplayStatus::InProgress => in_progress.push(index),
            TodoDisplayStatus::Done => done.push(index),
        }
    }

    for index in in_progress {
        if selected.len() >= max_visible {
            break;
        }
        selected.push(index);
    }

    let slots = max_visible.saturating_sub(selected.len());
    if slots > 0 {
        if pending.is_empty() {
            selected.extend(done.iter().rev().take(slots));
        } else if done.is_empty() {
            selected.extend(pending.iter().take(slots));
        } else {
            if let Some(&latest_done) = done.last() {
                selected.push(latest_done);
            }

            let pending_slots = max_visible.saturating_sub(selected.len());
            selected.extend(pending.iter().take(pending_slots));

            if selected.len() < max_visible {
                let selected_set: BTreeSet<usize> = selected.iter().copied().collect();
                selected.extend(
                    done.iter()
                        .rev()
                        .copied()
                        .filter(|index| !selected_set.contains(index))
                        .take(max_visible - selected.len()),
                );
            }
        }
    }

    selected.sort_unstable();
    visible_todos(selected, todos)
}

fn visible_todos(indices: Vec<usize>, todos: &[TodoDisplayItem]) -> VisibleTodos {
    let selected: BTreeSet<usize> = indices.iter().copied().collect();
    let mut hidden_counts = TodoHiddenCounts::default();

    for (index, todo) in todos.iter().enumerate() {
        if !selected.contains(&index) {
            hidden_counts.add(todo.status);
        }
    }

    VisibleTodos {
        indices,
        hidden: todos.len().saturating_sub(selected.len()),
        hidden_counts,
    }
}

pub struct TodoPanel<'a> {
    todos: &'a [TodoDisplayItem],
    theme: TuiTheme,
    expanded: bool,
}

impl<'a> TodoPanel<'a> {
    #[must_use]
    pub fn new(todos: &'a [TodoDisplayItem]) -> Self {
        Self {
            todos,
            theme: TuiTheme::default(),
            expanded: false,
        }
    }

    #[must_use]
    pub const fn with_theme(mut self, theme: TuiTheme) -> Self {
        self.theme = theme;
        self
    }

    #[must_use]
    pub const fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    /// Compute the rendered height of the panel (including border) for a
    /// given terminal width.
    #[must_use]
    pub fn height(&self, width: u16) -> u16 {
        if self.todos.is_empty() {
            return 0;
        }
        let visible = if self.expanded {
            VisibleTodos {
                indices: (0..self.todos.len()).collect(),
                hidden: 0,
                hidden_counts: TodoHiddenCounts::default(),
            }
        } else {
            select_visible_todos(self.todos, MAX_VISIBLE_TODOS)
        };
        let inner_width = usize::from(width.saturating_sub(6).max(1));
        let item_lines: usize = visible
            .indices
            .iter()
            .map(|&i| wrap_width(&self.todos[i].title, inner_width).len().max(1))
            .sum();
        let has_footer = if self.expanded {
            self.todos.len() > MAX_VISIBLE_TODOS
        } else {
            visible.hidden > 0
        };
        let total = 2 + item_lines + usize::from(has_footer);
        u16::try_from(total).unwrap_or(u16::MAX)
    }

    #[must_use]
    pub fn render(&self, width: usize) -> Vec<String> {
        if self.todos.is_empty() {
            return Vec::new();
        }

        let visible = if self.expanded {
            VisibleTodos {
                indices: (0..self.todos.len()).collect(),
                hidden: 0,
                hidden_counts: TodoHiddenCounts::default(),
            }
        } else {
            select_visible_todos(self.todos, MAX_VISIBLE_TODOS)
        };
        let inner_width = width.saturating_sub(6).max(1);
        let mut lines = vec![
            paint(
                &"\u{2500}".repeat(width),
                Style::default().fg(self.theme.text_muted),
            ),
            paint("  Todo", Style::default().fg(self.theme.brand).bold()),
        ];

        for &index in &visible.indices {
            lines.extend(render_item(&self.todos[index], inner_width, self.theme));
        }

        if self.expanded && self.todos.len() > MAX_VISIBLE_TODOS {
            lines.push(paint(
                &format!("  all {} items \u{b7} ctrl+t to collapse", self.todos.len()),
                Style::default().fg(self.theme.text_muted),
            ));
        } else if visible.hidden > 0 {
            let hidden_counts = format_hidden_counts(visible.hidden_counts);
            let distribution = if hidden_counts.is_empty() {
                String::new()
            } else {
                format!(" ({hidden_counts})")
            };
            lines.push(paint(
                &format!(
                    "  \u{2026} +{} more{} \u{b7} ctrl+t to expand",
                    visible.hidden, distribution
                ),
                Style::default().fg(self.theme.text_muted),
            ));
        }

        lines
            .into_iter()
            .map(|line| truncate_width(&line, width, "", false))
            .collect()
    }
}

fn format_hidden_counts(counts: TodoHiddenCounts) -> String {
    let mut parts = Vec::new();
    if counts.done > 0 {
        parts.push(format!("{} done", counts.done));
    }
    if counts.in_progress > 0 {
        parts.push(format!("{} in progress", counts.in_progress));
    }
    if counts.pending > 0 {
        parts.push(format!("{} pending", counts.pending));
    }
    parts.join(" \u{b7} ")
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
    fn selector_returns_all_items_when_count_fits() {
        let todos = vec![
            item("a", TodoDisplayStatus::Done),
            item("b", TodoDisplayStatus::InProgress),
            item("c", TodoDisplayStatus::Pending),
        ];

        let visible = select_visible_todos(&todos, 5);

        assert_eq!(visible.indices, vec![0, 1, 2]);
        assert_eq!(visible.hidden, 0);
        assert_eq!(visible.hidden_counts.done, 0);
        assert_eq!(visible.hidden_counts.in_progress, 0);
        assert_eq!(visible.hidden_counts.pending, 0);
    }

    #[test]
    fn selector_shows_latest_done_active_and_earliest_pending() {
        let todos = vec![
            item("d1", TodoDisplayStatus::Done),
            item("d2", TodoDisplayStatus::Done),
            item("d3", TodoDisplayStatus::Done),
            item("ip", TodoDisplayStatus::InProgress),
            item("p1", TodoDisplayStatus::Pending),
            item("p2", TodoDisplayStatus::Pending),
            item("p3", TodoDisplayStatus::Pending),
            item("p4", TodoDisplayStatus::Pending),
            item("p5", TodoDisplayStatus::Pending),
        ];

        let visible = select_visible_todos(&todos, 5);
        let titles: Vec<&str> = visible
            .indices
            .iter()
            .map(|&index| todos[index].title.as_str())
            .collect();

        assert_eq!(visible.indices, vec![2, 3, 4, 5, 6]);
        assert_eq!(titles, vec!["d3", "ip", "p1", "p2", "p3"]);
        assert_eq!(visible.hidden, 4);
    }

    #[test]
    fn selector_expands_done_when_pending_has_too_few_items() {
        let todos = vec![
            item("d1", TodoDisplayStatus::Done),
            item("d2", TodoDisplayStatus::Done),
            item("d3", TodoDisplayStatus::Done),
            item("d4", TodoDisplayStatus::Done),
            item("d5", TodoDisplayStatus::Done),
            item("ip", TodoDisplayStatus::InProgress),
            item("p1", TodoDisplayStatus::Pending),
        ];

        let visible = select_visible_todos(&todos, 5);

        assert_eq!(visible.indices, vec![2, 3, 4, 5, 6]);
    }

    #[test]
    fn selector_all_pending_shows_first_five() {
        let todos: Vec<TodoDisplayItem> = (0..8)
            .map(|i| item(&format!("p{i}"), TodoDisplayStatus::Pending))
            .collect();

        let visible = select_visible_todos(&todos, 5);

        assert_eq!(visible.indices, vec![0, 1, 2, 3, 4]);
        assert_eq!(visible.hidden, 3);
        assert_eq!(visible.hidden_counts.pending, 3);
    }

    #[test]
    fn selector_all_done_shows_last_five() {
        let todos: Vec<TodoDisplayItem> = (0..8)
            .map(|i| item(&format!("d{i}"), TodoDisplayStatus::Done))
            .collect();

        let visible = select_visible_todos(&todos, 5);

        assert_eq!(visible.indices, vec![3, 4, 5, 6, 7]);
        assert_eq!(visible.hidden, 3);
        assert_eq!(visible.hidden_counts.done, 3);
    }

    #[test]
    fn selector_mixed_done_pending_without_active_keeps_one_recent_done() {
        let todos = vec![
            item("d1", TodoDisplayStatus::Done),
            item("d2", TodoDisplayStatus::Done),
            item("d3", TodoDisplayStatus::Done),
            item("p1", TodoDisplayStatus::Pending),
            item("p2", TodoDisplayStatus::Pending),
            item("p3", TodoDisplayStatus::Pending),
            item("p4", TodoDisplayStatus::Pending),
            item("p5", TodoDisplayStatus::Pending),
        ];

        let visible = select_visible_todos(&todos, 5);

        assert_eq!(visible.indices, vec![2, 3, 4, 5, 6]);
    }

    #[test]
    fn selector_hidden_counts_reflect_hidden_items() {
        let todos = vec![
            item("ip0", TodoDisplayStatus::InProgress),
            item("ip1", TodoDisplayStatus::InProgress),
            item("ip2", TodoDisplayStatus::InProgress),
            item("ip3", TodoDisplayStatus::InProgress),
            item("ip4", TodoDisplayStatus::InProgress),
            item("ip5", TodoDisplayStatus::InProgress),
            item("d0", TodoDisplayStatus::Done),
            item("d1", TodoDisplayStatus::Done),
            item("d2", TodoDisplayStatus::Done),
            item("p0", TodoDisplayStatus::Pending),
            item("p1", TodoDisplayStatus::Pending),
            item("p2", TodoDisplayStatus::Pending),
        ];

        let visible = select_visible_todos(&todos, 5);

        assert_eq!(visible.indices, vec![0, 1, 2, 3, 4]);
        assert_eq!(visible.hidden, 7);
        assert_eq!(visible.hidden_counts.done, 3);
        assert_eq!(visible.hidden_counts.in_progress, 1);
        assert_eq!(visible.hidden_counts.pending, 3);
    }

    #[test]
    fn selector_max_visible_zero_hides_all_items_with_counts() {
        let todos = vec![
            item("done", TodoDisplayStatus::Done),
            item("active", TodoDisplayStatus::InProgress),
            item("pending", TodoDisplayStatus::Pending),
            item("pending 2", TodoDisplayStatus::Pending),
        ];

        let visible = select_visible_todos(&todos, 0);

        assert_eq!(visible.indices, Vec::<usize>::new());
        assert_eq!(visible.hidden, todos.len());
        assert_eq!(visible.hidden_counts.done, 1);
        assert_eq!(visible.hidden_counts.in_progress, 1);
        assert_eq!(visible.hidden_counts.pending, 2);
    }

    #[test]
    fn selector_empty_todos_returns_empty_visible_state() {
        let todos: Vec<TodoDisplayItem> = Vec::new();

        let visible = select_visible_todos(&todos, 5);

        assert_eq!(visible.indices, Vec::<usize>::new());
        assert_eq!(visible.hidden, 0);
        assert_eq!(visible.hidden_counts.done, 0);
        assert_eq!(visible.hidden_counts.in_progress, 0);
        assert_eq!(visible.hidden_counts.pending, 0);
    }

    #[test]
    fn selector_types_are_exported_from_widgets_surface() {
        let counts = crate::widgets::TodoHiddenCounts::default();
        let visible = crate::widgets::VisibleTodos {
            indices: Vec::new(),
            hidden: 0,
            hidden_counts: counts,
        };

        assert_eq!(visible.hidden_counts, counts);
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
            .map(|line| crate::primitive::strip_ansi(line))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(plain.contains("Todo"));
        assert!(plain.contains("\u{2713} latest done task"));
        assert!(plain.contains("\u{25CF} active task"));
        assert!(plain.contains("\u{25CB} pending one"));
        assert!(plain.contains("\u{2026} +1 more"));
    }

    #[test]
    fn collapsed_footer_advertises_ctrl_t_and_hidden_distribution() {
        let todos = vec![
            item("ip0", TodoDisplayStatus::InProgress),
            item("ip1", TodoDisplayStatus::InProgress),
            item("ip2", TodoDisplayStatus::InProgress),
            item("ip3", TodoDisplayStatus::InProgress),
            item("ip4", TodoDisplayStatus::InProgress),
            item("ip5", TodoDisplayStatus::InProgress),
            item("d0", TodoDisplayStatus::Done),
            item("d1", TodoDisplayStatus::Done),
            item("d2", TodoDisplayStatus::Done),
            item("p0", TodoDisplayStatus::Pending),
            item("p1", TodoDisplayStatus::Pending),
            item("p2", TodoDisplayStatus::Pending),
        ];

        let lines = TodoPanel::new(&todos).render(80);
        let plain = lines
            .iter()
            .map(|line| crate::primitive::strip_ansi(line))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(plain.contains(
            "\u{2026} +7 more (3 done \u{b7} 1 in progress \u{b7} 3 pending) \u{b7} ctrl+t to expand"
        ));
    }

    #[test]
    fn expanded_panel_renders_all_items_and_collapse_hint() {
        let todos: Vec<TodoDisplayItem> = (0..7)
            .map(|i| item(&format!("task-{i}"), TodoDisplayStatus::Pending))
            .collect();

        let lines = TodoPanel::new(&todos).expanded(true).render(80);
        let plain = lines
            .iter()
            .map(|line| crate::primitive::strip_ansi(line))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(plain.contains("task-0"));
        assert!(plain.contains("task-6"));
        assert!(plain.contains("all 7 items \u{b7} ctrl+t to collapse"));
        assert!(!plain.contains("+2 more"));
    }

    #[test]
    fn todo_panel_height_matches_rendered_lines() {
        let mut todos: Vec<TodoDisplayItem> = (0..7)
            .map(|i| item(&format!("task-{i}"), TodoDisplayStatus::Pending))
            .collect();
        todos[0] = item(
            "task-0 has a deliberately long title that wraps at a narrow width",
            TodoDisplayStatus::Pending,
        );
        let width = 40;

        let collapsed = TodoPanel::new(&todos);
        assert_eq!(
            usize::from(collapsed.height(width)),
            collapsed.render(usize::from(width)).len()
        );

        let expanded = TodoPanel::new(&todos).expanded(true);
        assert_eq!(
            usize::from(expanded.height(width)),
            expanded.render(usize::from(width)).len()
        );
    }
}
