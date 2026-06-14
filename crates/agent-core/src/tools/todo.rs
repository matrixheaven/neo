use std::sync::{Arc, Mutex};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Tool, ToolContext, ToolResult, parse_input, schema};
use crate::TodoEventData;

/// A single todo item tracked by the model.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TodoItem {
    /// Short, human-readable description of the task.
    pub title: String,
    /// Current status of the task.
    #[serde(rename = "status")]
    pub status: TodoStatus,
}

/// Lifecycle status of a todo item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Not yet started — rendered as `○`.
    Pending,
    /// Actively being worked on — rendered as `●`.
    InProgress,
    /// Finished — rendered as `✓`.
    Done,
}

impl TodoStatus {
    /// Returns the single-character glyph used in the formatted output.
    #[must_use]
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Pending => "\u{25CB}", // ○
            Self::InProgress => "\u{25CF}", // ●
            Self::Done => "\u{2713}", // ✓
        }
    }

    /// Returns the serialisable string key matching `#[serde(rename_all)]`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Done => "done",
        }
    }
}

impl From<&TodoItem> for TodoEventData {
    fn from(item: &TodoItem) -> Self {
        Self {
            title: item.title.clone(),
            status: item.status.as_str().to_owned(),
        }
    }
}

/// Input payload for [`TodoTool`].
///
/// The model always sends the **full** todo list. An empty array clears the
/// list; a non-empty array replaces it entirely.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TodoInput {
    /// The complete set of todos. Replace the entire list each call.
    pub todos: Vec<TodoItem>,
}

/// Format a slice of todos into the human-readable display string.
///
/// ```text
/// ○ Pending task title
/// ● In-progress task title
/// ✓ Completed task title
/// ```
fn format_todos(todos: &[TodoItem]) -> String {
    if todos.is_empty() {
        return "(todo list cleared)".to_owned();
    }
    let mut out = String::new();
    for item in todos {
        out.push_str(item.status.glyph());
        out.push(' ');
        out.push_str(&item.title);
        out.push('\n');
    }
    // Remove trailing newline for a clean single-block result.
    out.trim_end_matches('\n').to_owned()
}

/// Tool that manages a structured todo list.
///
/// Holds shared state (`Arc<Mutex<Vec<TodoEventData>>>`) so that the runtime
/// can read the latest todos after execution and emit `AgentEvent::TodoUpdated`
/// for persistence. The structured data is also returned in
/// [`ToolResult::details`] as a JSON bridge.
pub struct TodoTool {
    state: Arc<Mutex<Vec<TodoEventData>>>,
}

impl Default for TodoTool {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl TodoTool {
    /// Create a new `TodoTool` with its own internal state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a `TodoTool` that shares the given state Arc.
    ///
    /// Use this when the caller (e.g. the runtime) also holds a clone of the
    /// same Arc so it can read current todos directly.
    #[must_use]
    pub fn with_state(state: Arc<Mutex<Vec<TodoEventData>>>) -> Self {
        Self { state }
    }

    /// Read the current todos from shared state (for testing / external queries).
    #[must_use]
    pub fn current_todos(&self) -> Vec<TodoEventData> {
        self.state.lock().map_or_else(|_| Vec::new(), |guard| guard.clone())
    }
}

impl Tool for TodoTool {
    fn name(&self) -> &'static str {
        "todo"
    }

    fn description(&self) -> &'static str {
        "Manage your task list for multi-step work. Provide the full list of \
         todos every time you call this tool — the list is replaced entirely. \
         Use an empty array to clear all todos. Statuses: `pending` (\u{25CB}), \
         `in_progress` (\u{25CF}), `done` (\u{2713}). Call this whenever you \
         start or complete a task so the user can see your progress."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<TodoInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> super::ToolFuture<'a> {
        Box::pin(async move {
            let input: TodoInput = parse_input(self.name(), input)?;
            let formatted = format_todos(&input.todos);

            // Convert to event data for persistence.
            let event_todos: Vec<TodoEventData> =
                input.todos.iter().map(TodoEventData::from).collect();

            // Update shared state.
            if let Ok(mut state) = self.state.lock() {
                (*state).clone_from(&event_todos);
            }

            // Stream the formatted list for live TUI display.
            ctx.emit_update(&formatted);

            // Return structured data in details so the runtime can emit
            // AgentEvent::TodoUpdated.
            Ok(ToolResult::ok(formatted).with_details(json!({
                "todos": event_todos,
            })))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PermissionPolicy, ToolContext};
    use serde_json::json;
    use std::sync::{Arc, Mutex};

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
        assert_eq!(format_todos(&[]), "(todo list cleared)");
    }

    #[test]
    fn format_single_pending() {
        let todos = vec![TodoItem {
            title: "Read files".into(),
            status: TodoStatus::Pending,
        }];
        assert_eq!(format_todos(&todos), "\u{25CB} Read files");
    }

    #[test]
    fn format_single_in_progress() {
        let todos = vec![TodoItem {
            title: "Write code".into(),
            status: TodoStatus::InProgress,
        }];
        assert_eq!(format_todos(&todos), "\u{25CF} Write code");
    }

    #[test]
    fn format_single_done() {
        let todos = vec![TodoItem {
            title: "Run tests".into(),
            status: TodoStatus::Done,
        }];
        assert_eq!(format_todos(&todos), "\u{2713} Run tests");
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
            "\u{2713} Plan\n\u{25CF} Implement\n\u{25CB} Document"
        );
    }

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
        assert_eq!(input.todos.len(), 3);
        assert_eq!(input.todos[0].status, TodoStatus::Pending);
        assert_eq!(input.todos[1].status, TodoStatus::InProgress);
        assert_eq!(input.todos[2].status, TodoStatus::Done);
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

    #[tokio::test]
    async fn execute_formats_and_returns() {
        let tool = TodoTool::new();
        let ctx = ToolContext::new(std::env::current_dir().unwrap())
            .unwrap()
            .with_permission_policy(PermissionPolicy::allow_all());
        let input = json!({
            "todos": [
                { "title": "Step one", "status": "done" },
                { "title": "Step two", "status": "in_progress" }
            ]
        });
        let result = tool.execute(&ctx, input).await.expect("execute");
        assert!(!result.is_error);
        assert!(result.content.contains("\u{2713} Step one"));
        assert!(result.content.contains("\u{25CF} Step two"));
    }

    #[tokio::test]
    async fn execute_empty_array_clears() {
        let tool = TodoTool::new();
        let ctx = ToolContext::new(std::env::current_dir().unwrap())
            .unwrap()
            .with_permission_policy(PermissionPolicy::allow_all());
        let result = tool
            .execute(&ctx, json!({ "todos": [] }))
            .await
            .expect("execute");
        assert_eq!(result.content, "(todo list cleared)");
    }

    #[tokio::test]
    async fn execute_emits_update() {
        // Capture emitted updates via a shared buffer.
        let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_clone = Arc::clone(&captured);
        let callback: super::super::ToolUpdateCallback =
            Arc::new(move |partial: &str| {
                captured_clone
                    .lock()
                    .unwrap()
                    .push(partial.to_owned());
            });
        let ctx = ToolContext::new(std::env::current_dir().unwrap())
            .unwrap()
            .with_permission_policy(PermissionPolicy::allow_all())
            .with_tool_update(callback);

        let tool = TodoTool::new();
        let input = json!({
            "todos": [{ "title": "Task", "status": "pending" }]
        });
        let _ = tool.execute(&ctx, input).await.expect("execute");

        let updates = captured.lock().unwrap();
        assert_eq!(updates.len(), 1);
        assert!(updates[0].contains("\u{25CB} Task"));
    }

    #[tokio::test]
    async fn execute_invalid_input_is_error() {
        let tool = TodoTool::new();
        let ctx = ToolContext::new(std::env::current_dir().unwrap())
            .unwrap()
            .with_permission_policy(PermissionPolicy::allow_all());
        // Missing `todos` field.
        let result = tool.execute(&ctx, json!({})).await;
        assert!(result.is_err());
    }

    #[test]
    fn schema_has_todos_array() {
        let tool = TodoTool::new();
        let schema = tool.input_schema();
        let props = schema
            .get("properties")
            .expect("properties")
            .as_object()
            .unwrap();
        assert!(props.contains_key("todos"));
        // The top-level schema should declare `todos` as required.
        let required = schema.get("required").and_then(|v| v.as_array());
        assert!(required.is_some_and(|arr| {
            arr.iter().any(|v| v.as_str() == Some("todos"))
        }));
    }

    #[tokio::test]
    async fn execute_includes_structured_details() {
        let tool = TodoTool::new();
        let ctx = ToolContext::new(std::env::current_dir().unwrap())
            .unwrap()
            .with_permission_policy(PermissionPolicy::allow_all());
        let input = json!({
            "todos": [
                { "title": "Task A", "status": "done" },
                { "title": "Task B", "status": "pending" }
            ]
        });
        let result = tool.execute(&ctx, input).await.expect("execute");
        let details = result.details.expect("details should be present");
        let todos = details.get("todos").expect("todos in details");
        let parsed: Vec<TodoEventData> =
            serde_json::from_value(todos.clone()).expect("parse todos");
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
        let ctx = ToolContext::new(std::env::current_dir().unwrap())
            .unwrap()
            .with_permission_policy(PermissionPolicy::allow_all());
        let input = json!({
            "todos": [{ "title": "Shared task", "status": "in_progress" }]
        });
        let _ = tool.execute(&ctx, input).await.expect("execute");

        let state = shared.lock().unwrap();
        assert_eq!(state.len(), 1);
        assert_eq!(state[0].title, "Shared task");
        assert_eq!(state[0].status, "in_progress");
    }

    #[test]
    fn current_todos_reflects_state() {
        let shared: Arc<Mutex<Vec<TodoEventData>>> = Arc::new(Mutex::new(vec![
            TodoEventData {
                title: "X".into(),
                status: "done".into(),
            },
        ]));
        let tool = TodoTool::with_state(shared);
        let todos = tool.current_todos();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].title, "X");
    }
}
