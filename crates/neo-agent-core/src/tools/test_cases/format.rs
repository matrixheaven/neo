use super::*;

#[test]
fn glyph_mapping() {
    assert_eq!(TodoStatus::Pending.glyph(), "\u{25CB}");
    assert_eq!(TodoStatus::InProgress.glyph(), "\u{25CF}");
    assert_eq!(TodoStatus::Done.glyph(), "\u{2713}");
}

#[test]
fn as_str_mapping() {
    assert_eq!(TodoStatus::Pending.as_str(), "pending");
    assert_eq!(TodoStatus::InProgress.as_str(), "in_progress");
    assert_eq!(TodoStatus::Done.as_str(), "done");
}

#[test]
fn format_empty_clears() {
    assert_eq!(format_todos(&[]), "Todo list is empty.");
}

#[test]
fn format_single_pending() {
    let todos = vec![TodoItem {
        title: "Read files".into(),
        status: TodoStatus::Pending,
    }];
    assert_eq!(
        format_todos(&todos),
        "Current todo list:\n  [pending] Read files"
    );
}

#[test]
fn format_single_in_progress() {
    let todos = vec![TodoItem {
        title: "Write code".into(),
        status: TodoStatus::InProgress,
    }];
    assert_eq!(
        format_todos(&todos),
        "Current todo list:\n  [in_progress] Write code"
    );
}

#[test]
fn format_single_done() {
    let todos = vec![TodoItem {
        title: "Run tests".into(),
        status: TodoStatus::Done,
    }];
    assert_eq!(
        format_todos(&todos),
        "Current todo list:\n  [done] Run tests"
    );
}

#[test]
fn format_mixed_statuses() {
    let todos = vec![
        TodoItem {
            title: "Plan".into(),
            status: TodoStatus::Done,
        },
        TodoItem {
            title: "Implement".into(),
            status: TodoStatus::InProgress,
        },
        TodoItem {
            title: "Document".into(),
            status: TodoStatus::Pending,
        },
    ];
    let result = format_todos(&todos);
    assert_eq!(
        result,
        "Current todo list:\n  [done] Plan\n  [in_progress] Implement\n  [pending] Document"
    );
}
