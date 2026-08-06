use std::sync::{Arc, Mutex};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Tool, ToolContext, ToolResult, parse_input, schema};
use crate::TodoEventData;

/// A single todo item tracked by the model.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TodoItem {
    #[schemars(
        description = "Short, actionable title for the todo. Example: \"Read session-control.ts\"."
    )]
    pub title: String,
    #[serde(rename = "status")]
    #[schemars(
        description = "Current status of the todo. Must be one of: `pending`, `in_progress`, `done`."
    )]
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
    #[schemars(
        description = "The updated todo list. Each item must be an object with `title` (string) and `status` (`pending`, `in_progress`, or `done`). Omit to read the current list without making changes. Pass an empty array to clear the list."
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
#[path = "test_cases/todo_execute.rs"]
mod todo_execute;

#[cfg(test)]
#[path = "test_cases/format.rs"]
mod format;

#[cfg(test)]
#[path = "test_cases/deserialize.rs"]
mod deserialize;

#[cfg(test)]
#[path = "test_cases/todo_schema.rs"]
mod todo_schema;
