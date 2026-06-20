mod ask_user;
mod background_tasks;
mod bash;
mod edit;
pub mod extensions;
mod find;
mod glob;
mod goal;
mod grep;
mod list;
mod mcp;
mod plan_mode;
mod process_supervisor;
mod read;
mod sessions;
mod skills_manager;
mod terminal;
mod todo;
mod write;

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::{Component, Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

use neo_ai::ToolSpec;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::PermissionPolicy;
use crate::TodoEventData;
use crate::goal::GoalManager;

pub const DEFAULT_BASH_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub use mcp::*;
pub use process_supervisor::{ProcessKind, ProcessSupervisor};

// Re-export AskUser tool types for external use (TUI / CLI layer).
pub use ask_user::{
    AskUserInput, AskUserOptionInput, AskUserQuestionInput, AskUserTool, PendingQuestion,
    QuestionResponse,
};
pub use background_tasks::{
    BackgroundTaskKind, BackgroundTaskManager, BackgroundTaskSnapshot, BackgroundTaskStatus,
    CommandOutput, ManagedBackgroundCommand, TaskListTool, TaskOutputTool, TaskStopTool,
    cap_output_details, cap_plain_output, format_collected_answers, output_from_buffers,
};
// Re-export Todo tool types.
pub use todo::{TodoInput, TodoItem, TodoStatus, TodoTool};
// Re-export Goal tool types.
pub use goal::{GetGoalStatusTool, StartGoalTool, UpdateGoalStatusTool};
// Re-export session tool types.
pub use sessions::SummarizeSessionsTool;
// Re-export skill-manager tool types.
pub use skills_manager::{CreateSkillTool, ListSkillsTool, MoveSkillTool};

pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + 'a>>;

/// Callback invoked by tools to stream intermediate output while executing.
///
/// The callback receives a `partial_content` string (e.g. the latest line(s)
/// from stdout/stderr). The runtime wires this to emit
/// [`crate::AgentEvent::ToolExecutionUpdate`] so the TUI can display live
/// output in the tool card body.
///
/// This is intentionally a simple boxed closure rather than a channel so that
/// tools don't need to know about `AgentEvent` — they just push text.
pub type ToolUpdateCallback = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("unknown tool: {name}")]
    UnknownTool { name: String },
    #[error("permission denied for {operation}")]
    PermissionDenied { operation: &'static str },
    #[error("path is outside workspace: {path}")]
    PathOutsideWorkspace { path: PathBuf },
    #[error("invalid input for {tool}: {message}")]
    InvalidInput { tool: String, message: String },
    #[error("command timed out after {timeout_ms} ms")]
    CommandTimedOut { timeout_ms: u64 },
    #[error("tool execution cancelled")]
    Cancelled,
    #[error("mcp error from {server_id}/{tool_name}: {message}")]
    Mcp {
        server_id: String,
        tool_name: String,
        message: String,
    },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("regex error: {0}")]
    Regex(#[from] regex::Error),
}

#[derive(Clone)]
pub struct ToolContext {
    pub cwd: PathBuf,
    pub permissions: PermissionPolicy,
    pub bash_timeout: Duration,
    pub max_output_bytes: usize,
    pub cancel_token: CancellationToken,
    pub process_supervisor: ProcessSupervisor,
    pub background_tasks: BackgroundTaskManager,
    /// Optional callback for streaming intermediate tool output (e.g. bash
    /// stdout lines). Set by the runtime so tools can emit live updates.
    pub tool_update: Option<ToolUpdateCallback>,
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("cwd", &self.cwd)
            .field("permissions", &self.permissions)
            .field("bash_timeout", &self.bash_timeout)
            .field("max_output_bytes", &self.max_output_bytes)
            .field("cancel_token", &self.cancel_token)
            .field("process_supervisor", &self.process_supervisor)
            .field("background_tasks", &self.background_tasks)
            .field("tool_update", &self.tool_update.is_some())
            .finish()
    }
}

impl ToolContext {
    pub fn new(workspace_root: impl AsRef<Path>) -> Result<Self, ToolError> {
        let cwd = workspace_root.as_ref().canonicalize()?;
        Ok(Self {
            cwd,
            permissions: PermissionPolicy::default(),
            bash_timeout: DEFAULT_BASH_TIMEOUT,
            max_output_bytes: 64 * 1024,
            cancel_token: CancellationToken::new(),
            process_supervisor: ProcessSupervisor::default(),
            background_tasks: BackgroundTaskManager::new(),
            tool_update: None,
        })
    }

    #[must_use]
    pub fn with_permission_policy(mut self, permissions: PermissionPolicy) -> Self {
        self.permissions = permissions;
        self
    }

    #[must_use]
    pub fn with_bash_timeout(mut self, timeout: Duration) -> Self {
        self.bash_timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_cancel_token(mut self, cancel_token: CancellationToken) -> Self {
        self.cancel_token = cancel_token;
        self
    }

    #[must_use]
    pub fn with_process_supervisor(mut self, process_supervisor: ProcessSupervisor) -> Self {
        self.process_supervisor = process_supervisor;
        self
    }

    #[must_use]
    pub fn with_background_tasks(mut self, background_tasks: BackgroundTaskManager) -> Self {
        self.background_tasks = background_tasks;
        self
    }

    #[must_use]
    pub fn with_tool_update(mut self, callback: ToolUpdateCallback) -> Self {
        self.tool_update = Some(callback);
        self
    }

    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        &self.cwd
    }

    /// Push intermediate output through the tool update callback (if set).
    /// Tools call this to stream live progress, e.g. bash stdout lines.
    pub fn emit_update(&self, partial: &str) {
        if let Some(callback) = &self.tool_update {
            callback(partial);
        }
    }

    pub fn ensure_file_read_allowed(&self) -> Result<(), ToolError> {
        if self.permissions.can_read_files() {
            Ok(())
        } else {
            Err(ToolError::PermissionDenied {
                operation: "file_read",
            })
        }
    }

    pub fn ensure_file_write_allowed(&self) -> Result<(), ToolError> {
        if self.permissions.can_write_files() {
            Ok(())
        } else {
            Err(ToolError::PermissionDenied {
                operation: "file_write",
            })
        }
    }

    pub fn ensure_shell_allowed(&self) -> Result<(), ToolError> {
        if self.permissions.can_run_shell() {
            Ok(())
        } else {
            Err(ToolError::PermissionDenied { operation: "shell" })
        }
    }

    pub fn resolve_workspace_path(&self, path: &Path) -> Result<PathBuf, ToolError> {
        let candidate = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        };
        let normalized = normalize_inside_workspace(&self.cwd, &candidate)?;
        if normalized.exists() {
            let canonical = normalized.canonicalize()?;
            if canonical.starts_with(&self.cwd) {
                Ok(canonical)
            } else {
                Err(ToolError::PathOutsideWorkspace { path: canonical })
            }
        } else {
            Ok(normalized)
        }
    }

    pub fn resolve_parent_for_write(&self, path: &Path) -> Result<PathBuf, ToolError> {
        let candidate = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        };
        let parent = candidate.parent().unwrap_or(&self.cwd);
        let resolved_parent = normalize_inside_workspace(&self.cwd, parent)?;
        let file_name = candidate
            .file_name()
            .ok_or_else(|| ToolError::PathOutsideWorkspace {
                path: candidate.clone(),
            })?;
        Ok(resolved_parent.join(file_name))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, JsonSchema)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    pub details: Option<serde_json::Value>,
    pub terminate: bool,
}

impl ToolResult {
    #[must_use]
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            details: None,
            terminate: false,
        }
    }

    #[must_use]
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            details: None,
            terminate: false,
        }
    }

    #[must_use]
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    #[must_use]
    pub const fn terminate(mut self) -> Self {
        self.terminate = true;
        self
    }
}

pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> serde_json::Value;
    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a>;

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_owned(),
            description: self.description().to_owned(),
            input_schema: self.input_schema(),
        }
    }
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_builtin_tools() -> Self {
        Self::with_builtin_tools_and_todos(Arc::new(Mutex::new(Vec::new())))
    }

    #[must_use]
    pub fn with_builtin_tools_and_todos(todos: Arc<Mutex<Vec<TodoEventData>>>) -> Self {
        let mut registry = Self::new();
        registry.register(read::ReadTool);
        registry.register(list::ListTool);
        registry.register(grep::GrepTool);
        registry.register(find::FindTool);
        registry.register(glob::GlobTool);
        registry.register(self::todo::TodoTool::with_state(todos));
        registry.register(write::WriteTool);
        registry.register(edit::EditTool);
        registry.register(bash::BashTool);
        registry.register(background_tasks::TaskListTool);
        registry.register(background_tasks::TaskOutputTool);
        registry.register(background_tasks::TaskStopTool);
        registry.register(terminal::TerminalTool);
        registry.register(plan_mode::EnterPlanModeTool);
        registry.register(plan_mode::ExitPlanModeTool);
        registry
    }

    pub fn register<T>(&mut self, tool: T)
    where
        T: Tool + 'static,
    {
        self.tools.insert(tool.name().to_owned(), Box::new(tool));
    }

    pub fn register_goal_tools(&mut self, manager: Arc<GoalManager>) {
        self.register(goal::StartGoalTool::new(Arc::clone(&manager)));
        self.register(goal::UpdateGoalStatusTool::new(Arc::clone(&manager)));
        self.register(goal::GetGoalStatusTool::new(manager));
    }

    pub fn retain_named(&mut self, names: &BTreeSet<String>) {
        self.tools.retain(|name, _| names.contains(name));
    }

    pub fn remove_named(&mut self, names: &BTreeSet<String>) {
        self.tools.retain(|name, _| !names.contains(name));
    }

    #[must_use]
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|tool| tool.spec()).collect()
    }

    pub async fn run(
        &self,
        name: &str,
        ctx: &ToolContext,
        input: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let tool = self.tools.get(name).ok_or_else(|| ToolError::UnknownTool {
            name: name.to_owned(),
        })?;
        tool.execute(ctx, input).await
    }
}

fn parse_input<T>(tool: &str, input: serde_json::Value) -> Result<T, ToolError>
where
    T: DeserializeOwned,
{
    serde_json::from_value(input).map_err(|err| ToolError::InvalidInput {
        tool: tool.to_owned(),
        message: err.to_string(),
    })
}

fn schema<T>() -> serde_json::Value
where
    T: JsonSchema,
{
    neo_ai::tool_schema::schema_for::<T>()
}

fn normalize_inside_workspace(workspace_root: &Path, path: &Path) -> Result<PathBuf, ToolError> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
        }
    }

    if normalized.starts_with(workspace_root) {
        Ok(normalized)
    } else {
        Err(ToolError::PathOutsideWorkspace {
            path: path.to_path_buf(),
        })
    }
}

fn cap_output(content: &str, max_bytes: usize) -> (String, bool) {
    if content.len() <= max_bytes {
        return (format!("{content}\ntruncated: false"), false);
    }
    let mut capped = String::new();
    for character in content.chars() {
        let next_len = capped.len() + character.len_utf8();
        if next_len > max_bytes {
            break;
        }
        capped.push(character);
    }
    (format!("{capped}\ntruncated: true"), true)
}
