use super::*;
use crate::ToolAccess;
use crate::ToolContext;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex;

#[tokio::test]
async fn execute_formats_and_returns() {
    let tool = TodoTool::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_access(ToolAccess::all());
    let input = json!({
        "todos": [
            { "title": "Step one", "status": "done" },
            { "title": "Step two", "status": "in_progress" }
        ]
    });
    let result = tool.execute(&ctx, input).await.expect("execute");
    assert!(!result.is_error);
    assert!(result.content.contains("Todo list updated."));
    assert!(result.content.contains("[done] Step one"));
    assert!(result.content.contains("[in_progress] Step two"));
    assert!(result.content.contains("keep exactly one task in_progress"));
}

#[tokio::test]
async fn execute_empty_array_clears() {
    let tool = TodoTool::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_access(ToolAccess::all());
    let result = tool
        .execute(&ctx, json!({ "todos": [] }))
        .await
        .expect("execute");
    assert_eq!(result.content, "Todo list cleared.");
    let details = result.details.expect("clear details");
    assert_eq!(details.get("todos"), Some(&json!([])));
}

#[tokio::test]
async fn execute_emits_update() {
    // Capture emitted updates via a shared buffer.
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = Arc::clone(&captured);
    let callback: super::super::ToolUpdateCallback = Arc::new(move |partial: &str| {
        captured_clone.lock().unwrap().push(partial.to_owned());
    });
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_access(ToolAccess::all())
        .with_tool_update(callback);

    let tool = TodoTool::new();
    let input = json!({
        "todos": [{ "title": "Task", "status": "pending" }]
    });
    let _ = tool.execute(&ctx, input).await.expect("execute");

    let updates = captured.lock().unwrap();
    assert_eq!(updates.len(), 1);
    assert!(updates[0].contains("[pending] Task"));
}

#[tokio::test]
async fn execute_read_mode_returns_current_list_without_details_or_update() {
    let shared: Arc<Mutex<Vec<TodoEventData>>> = Arc::new(Mutex::new(vec![
        TodoEventData {
            title: "Read code".to_owned(),
            status: "in_progress".to_owned(),
        },
        TodoEventData {
            title: "Write tests".to_owned(),
            status: "pending".to_owned(),
        },
    ]));
    let tool = TodoTool::with_state(Arc::clone(&shared));
    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = Arc::clone(&captured);
    let callback: super::super::ToolUpdateCallback = Arc::new(move |partial: &str| {
        captured_clone.lock().unwrap().push(partial.to_owned());
    });
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_access(ToolAccess::all())
        .with_tool_update(callback);

    let result = tool.execute(&ctx, json!({})).await.expect("execute");

    assert_eq!(
        result.content,
        "Current todo list:\n  [in_progress] Read code\n  [pending] Write tests"
    );
    assert!(result.details.is_none());
    assert!(captured.lock().unwrap().is_empty());
    assert_eq!(shared.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn execute_includes_structured_details() {
    let tool = TodoTool::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_access(ToolAccess::all());
    let input = json!({
        "todos": [
            { "title": "Task A", "status": "done" },
            { "title": "Task B", "status": "pending" }
        ]
    });
    let result = tool.execute(&ctx, input).await.expect("execute");
    let details = result.details.expect("details should be present");
    let todos = details.get("todos").expect("todos in details");
    let parsed: Vec<TodoEventData> = serde_json::from_value(todos.clone()).expect("parse todos");
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].title, "Task A");
    assert_eq!(parsed[0].status, "done");
    assert_eq!(parsed[1].title, "Task B");
    assert_eq!(parsed[1].status, "pending");
}

#[tokio::test]
async fn execute_updates_shared_state() {
    let shared: Arc<Mutex<Vec<TodoEventData>>> = Arc::new(Mutex::new(Vec::new()));
    let tool = TodoTool::with_state(Arc::clone(&shared));
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_access(ToolAccess::all());
    let input = json!({
        "todos": [{ "title": "Shared task", "status": "in_progress" }]
    });
    let _ = tool.execute(&ctx, input).await.expect("execute");

    let state = shared.lock().unwrap();
    assert_eq!(state.len(), 1);
    assert_eq!(state[0].title, "Shared task");
    assert_eq!(state[0].status, "in_progress");
}
