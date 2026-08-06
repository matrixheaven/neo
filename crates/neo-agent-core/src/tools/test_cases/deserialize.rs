use super::*;
use serde_json::json;

#[test]
fn deserialize_snake_case_statuses() {
    let json = json!({
        "todos": [
            { "title": "a", "status": "pending" },
            { "title": "b", "status": "in_progress" },
            { "title": "c", "status": "done" }
        ]
    });
    let input: TodoInput = serde_json::from_value(json).expect("deserialize");
    let todos = input.todos.expect("todos");
    assert_eq!(todos.len(), 3);
    assert_eq!(todos[0].status, TodoStatus::Pending);
    assert_eq!(todos[1].status, TodoStatus::InProgress);
    assert_eq!(todos[2].status, TodoStatus::Done);
}

#[test]
fn deserialize_allows_read_mode_without_todos() {
    let input: TodoInput = serde_json::from_value(json!({})).expect("deserialize");
    assert!(input.todos.is_none());
}

#[test]
fn deserialize_rejects_invalid_status() {
    let json = json!({
        "todos": [{ "title": "x", "status": "completed" }]
    });
    assert!(serde_json::from_value::<TodoInput>(json).is_err());
}

#[test]
fn deserialize_rejects_unknown_field() {
    let json = json!({
        "todos": [{ "title": "x", "status": "done" }],
        "extra": true
    });
    assert!(serde_json::from_value::<TodoInput>(json).is_err());
}
