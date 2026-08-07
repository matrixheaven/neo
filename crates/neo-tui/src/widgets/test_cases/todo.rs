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
