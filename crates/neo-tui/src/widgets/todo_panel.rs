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
#[path = "test_cases/todo.rs"]
mod todo;
