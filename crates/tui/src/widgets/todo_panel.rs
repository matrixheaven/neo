use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::TuiTheme;

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

/// Smart truncation algorithm matching kimi-code's `todo-panel.ts`.
///
/// 1. Include ALL in_progress items (capped at `max_visible`).
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
    if selected.len() < max_visible {
        if let Some(done_idx) = todos
            .iter()
            .enumerate()
            .rev()
            .find(|(_, t)| t.status == TodoDisplayStatus::Done)
            .map(|(i, _)| i)
        {
            if !selected.contains(&done_idx) {
                selected.push(done_idx);
            }
        }
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
            .map(|&i| {
                crate::wrap_width(&self.todos[i].title, inner_width)
                    .len()
                    .max(1)
            })
            .sum();
        let hidden = self.todos.len() > visible.len();
        let total = 2 + 1 + item_lines + usize::from(hidden);
        u16::try_from(total).unwrap_or(u16::MAX)
    }
}

impl Widget for TodoPanel<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if self.todos.is_empty() || area.width < 6 || area.height < 3 {
            return;
        }

        let theme = self.theme;
        let block = Block::default()
            .title(Line::from(vec![Span::styled(
                " Todo ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.surface_border));
        let inner = block.inner(area);
        block.render(area, buf);

        let visible = select_visible_todos(self.todos, MAX_VISIBLE_TODOS);
        let text_width = usize::from(inner.width.saturating_sub(4).max(1));
        let mut y = inner.y;

        for &idx in &visible {
            if y >= inner.bottom() {
                break;
            }
            let item = &self.todos[idx];

            let (symbol, symbol_style) = match item.status {
                TodoDisplayStatus::Pending => ("○", Style::default().fg(theme.muted)),
                TodoDisplayStatus::InProgress => (
                    "●",
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                TodoDisplayStatus::Done => ("✓", Style::default().fg(theme.success)),
            };

            let title_style = match item.status {
                TodoDisplayStatus::Pending => Style::default().fg(theme.notice),
                TodoDisplayStatus::InProgress => Style::default()
                    .fg(theme.header)
                    .add_modifier(Modifier::BOLD),
                TodoDisplayStatus::Done => Style::default()
                    .fg(theme.muted)
                    .add_modifier(Modifier::CROSSED_OUT),
            };

            let wrapped = crate::wrap_width(&item.title, text_width);
            for (line_idx, title_line) in wrapped.iter().enumerate() {
                if y >= inner.bottom() {
                    break;
                }
                let line = if line_idx == 0 {
                    Line::from(vec![
                        Span::raw(" "),
                        Span::styled(symbol, symbol_style),
                        Span::raw(" "),
                        Span::styled(title_line.as_str(), title_style),
                    ])
                } else {
                    Line::from(Span::styled(format!("    {title_line}"), title_style))
                };
                Paragraph::new(line).render(
                    Rect {
                        x: inner.x,
                        y,
                        width: inner.width,
                        height: 1,
                    },
                    buf,
                );
                y = y.saturating_add(1);
            }
        }

        // Truncation indicator.
        let hidden_count = self.todos.len().saturating_sub(visible.len());
        if hidden_count > 0 && y < inner.bottom() {
            let line = Line::from(Span::styled(
                format!(" \u{2026} +{hidden_count} more"),
                Style::default().fg(theme.muted),
            ));
            Paragraph::new(line).render(
                Rect {
                    x: inner.x,
                    y,
                    width: inner.width,
                    height: 1,
                },
                buf,
            );
        }
    }
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
}
