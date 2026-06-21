use std::sync::{Arc, Mutex};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Tool, ToolContext, ToolResult, parse_input, schema};
use crate::TodoEventData;

/// A single todo item tracked by the model.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TodoItem {
    /// Short, actionable title for the todo (e.g. "Read session-control.ts").
    #[schemars(description = "Short, actionable title for the todo.")]
    pub title: String,
    /// Current status of the task.
    #[serde(rename = "status")]
    #[schemars(description = "Current status of the todo: pending, in_progress, or done.")]
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
            Self::Pending => "\u{25CB}",    // ○
            Self::InProgress => "\u{25CF}", // ●
            Self::Done => "\u{2713}",       // ✓
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
/// list; a non-empty array replaces it entirely. Omit the field to query the
/// current list without changing it.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TodoInput {
    /// The complete set of todos. Omit to read the current list, pass an empty
    /// array to clear it, or pass a non-empty array to replace it entirely.
    #[schemars(
        description = "The updated todo list. Omit to read the current list without making changes. Pass an empty array to clear the list."
    )]
    pub todos: Option<Vec<TodoItem>>,
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
        return "Todo list is empty.".to_owned();
    }
    let mut out = String::new();
    out.push_str("Current todo list:\n");
    for item in todos {
        out.push_str("  [");
        out.push_str(item.status.as_str());
        out.push_str("] ");
        out.push_str(&item.title);
        out.push('\n');
    }
    // Remove trailing newline for a clean single-block result.
    out.trim_end_matches('\n').to_owned()
}

/// Tool that manages a structured todo list.
///
/// Holds shared state (`Arc<Mutex<Vec<TodoEventData>>>`) so read-mode calls can
/// return the latest list. Write-mode calls return the updated list in
/// [`ToolResult::details`], which the runtime turns into `AgentEvent::TodoUpdated`.
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
        self.state
            .lock()
            .map_or_else(|_| Vec::new(), |guard| guard.clone())
    }
}

impl Tool for TodoTool {
    fn name(&self) -> &'static str {
        "TodoList"
    }

    fn description(&self) -> &'static str {
        "Maintain a structured task list as you work through a multi-step task. \
         Use it proactively and often when progress tracking helps the current work, \
         especially in plan mode, long-running investigations, and implementation \
         tasks with several tool calls.\n\n\
         When to use:\n\
         - Multi-step tasks that span several tool calls.\n\
         - Tracking investigation progress across a large codebase search.\n\
         - Planning a sequence of edits before making them.\n\
         - After receiving new multi-step instructions, capture the requirements as todos.\n\
         - Before starting a tracked task, mark exactly one item as `in_progress`.\n\
         - Immediately after finishing a tracked task, mark it `done`; do not batch completions at the end.\n\n\
         When NOT to use:\n\
         - Single-shot answers that complete in one or two tool calls.\n\
         - Trivial requests where tracking adds no clarity.\n\
         - Purely conversational or informational replies.\n\n\
         How to use:\n\
         - Call with `todos: [...]` to replace the full list. Statuses: `pending`, `in_progress`, `done`.\n\
         - Call with no arguments to retrieve the current list without changing it.\n\
         - Call with `todos: []` to clear the list.\n\
         - Keep titles short and actionable (e.g. \"Read session-control.ts\", \"Add planMode flag to TurnManager\").\n\
         - When work is underway, keep exactly one task `in_progress`.\n\
         - Only mark a task `done` when it is fully accomplished.\n\
         - Never mark a task `done` if tests are failing, implementation is partial, unresolved errors remain, or required files/dependencies could not be found.\n\
         - If you encounter a blocker, keep the blocked task `in_progress` or add a new pending task describing what must be resolved."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<TodoInput>()
    }

    fn execute<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> super::ToolFuture<'a> {
        const WRITE_REMINDER: &str = "Ensure that you continue to use the todo list to track progress. Mark tasks done immediately after finishing them, and keep exactly one task in_progress when work is underway.";

        Box::pin(async move {
            let input: TodoInput = parse_input(self.name(), input)?;
            let Some(todos) = input.todos else {
                let current = self
                    .state
                    .lock()
                    .map_or_else(|_| Vec::new(), |guard| guard.clone());
                return Ok(ToolResult::ok(format_event_todos(&current)));
            };
            let formatted = if todos.is_empty() {
                "Todo list cleared.".to_owned()
            } else {
                format_todos(&todos)
            };

            // Convert to event data for persistence.
            let event_todos: Vec<TodoEventData> = todos.iter().map(TodoEventData::from).collect();

            // Update shared state.
            if let Ok(mut state) = self.state.lock() {
                (*state).clone_from(&event_todos);
            }

            // Stream the formatted list for live TUI display.
            ctx.emit_update(&formatted);

            // Build the final content, mirroring the kimi-code reference output:
            // cleared state gets a short confirmation; updates include the list
            // plus a reminder to keep using the list.
            let content = if todos.is_empty() {
                formatted.clone()
            } else {
                format!("Todo list updated.\n{formatted}\n\n{WRITE_REMINDER}")
            };

            // Return structured data in details so the runtime can emit
            // AgentEvent::TodoUpdated.
            Ok(ToolResult::ok(content).with_details(json!({
                "todos": event_todos,
            })))
        })
    }
}

fn format_event_todos(todos: &[TodoEventData]) -> String {
    if todos.is_empty() {
        return "Todo list is empty.".to_owned();
    }

    let mut out = String::from("Current todo list:\n");
    for item in todos {
        out.push_str("  [");
        out.push_str(&item.status);
        out.push_str("] ");
        out.push_str(&item.title);
        out.push('\n');
    }
    out.trim_end_matches('\n').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ToolAccess, ToolContext};
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

    #[tokio::test]
    async fn execute_formats_and_returns() {
        let tool = TodoTool::new();
        let ctx = ToolContext::new(std::env::current_dir().unwrap())
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
        let ctx = ToolContext::new(std::env::current_dir().unwrap())
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
        let ctx = ToolContext::new(std::env::current_dir().unwrap())
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
        let ctx = ToolContext::new(std::env::current_dir().unwrap())
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

    #[test]
    fn schema_has_optional_todos_array() {
        let tool = TodoTool::new();
        let schema = tool.input_schema();
        let props = schema
            .get("properties")
            .expect("properties")
            .as_object()
            .unwrap();
        assert!(props.contains_key("todos"));
        let required = schema.get("required").and_then(|v| v.as_array());
        assert!(!required.is_some_and(|arr| { arr.iter().any(|v| v.as_str() == Some("todos")) }));
    }

    #[test]
    fn description_contains_usage_guidance() {
        let tool = TodoTool::new();
        let description = tool.description();
        assert!(description.contains("When to use"));
        assert!(description.contains("When NOT to use"));
        assert!(description.contains("How to use"));
        assert!(description.contains("`in_progress`"));
    }

    #[test]
    fn schema_descriptions_are_present() {
        let tool = TodoTool::new();
        let schema = tool.input_schema();
        let props = schema
            .get("properties")
            .expect("properties")
            .as_object()
            .unwrap();
        let todos = props.get("todos").expect("todos schema");
        assert!(
            todos.get("description").is_some(),
            "todos field should have a description"
        );
        // The item schema is either inline or referenced via $ref in schemars.
        let items = todos.get("items").expect("todos items");
        let item_schema = if let Some(reference) = items.get("$ref").and_then(|v| v.as_str()) {
            let definitions = schema
                .get("$defs")
                .or_else(|| schema.get("definitions"))
                .expect("schema definitions");
            definitions
                .get(reference.split('/').next_back().expect("ref name"))
                .expect("resolved item schema")
        } else {
            items
        };
        assert!(
            item_schema.get("properties").is_some(),
            "item schema should expose properties"
        );
    }

    #[tokio::test]
    async fn execute_includes_structured_details() {
        let tool = TodoTool::new();
        let ctx = ToolContext::new(std::env::current_dir().unwrap())
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

    #[test]
    fn current_todos_reflects_state() {
        let shared: Arc<Mutex<Vec<TodoEventData>>> = Arc::new(Mutex::new(vec![TodoEventData {
            title: "X".into(),
            status: "done".into(),
        }]));
        let tool = TodoTool::with_state(shared);
        let todos = tool.current_todos();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].title, "X");
    }
}
