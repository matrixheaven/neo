mod ask_user;
mod background_tasks;
mod bash;
mod delegate;
mod delegate_controls;
mod diff;
mod edit;
pub mod extensions;
mod find;
mod glob;
mod goal;
mod grep;
mod list;
mod mcp;
mod mcp_manager;
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

use neo_ai::ModelClient;
use neo_ai::ToolSpec;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::TodoEventData;
use crate::ToolAccess;
use crate::goal::GoalManager;
use crate::multi_agent::MultiAgentRuntime;
use crate::runtime::AgentConfig;

use crate::AgentEvent;

pub const DEFAULT_BASH_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub use mcp::{
    HttpConfig, HttpOAuthConfig, McpClient, McpError, McpResourceContent, McpResourceDefinition,
    McpResourceRead, McpToolDefinition, McpToolResponse, StdioConfig, build_authorization_manager,
    build_http_client, build_stdio_client,
    oauth::{McpOAuthIdentity, McpOAuthService, McpOAuthServiceConfig, McpOAuthTransportKind},
};
pub use mcp_manager::*;
pub use process_supervisor::ProcessSupervisor;

// Re-export AskUser tool types for external use (TUI / CLI layer).
pub use ask_user::{
    AskUserInput, AskUserOptionInput, AskUserQuestionInput, AskUserTool, PendingQuestion,
    QuestionResponse,
};
pub use background_tasks::{
    BackgroundTaskKind, BackgroundTaskManager, BackgroundTaskSnapshot, BackgroundTaskStatus,
    CommandOutput, ManagedBackgroundCommand, TaskListTool, TaskOutputTool, TaskStopTool,
    cap_output_details, cap_plain_output, format_collected_answers, output_from_buffers,
    task_list_result,
};
pub use bash::{
    ShellExecutionRequest, ShellExecutionResult, execute_model_bash_for_runtime,
    execute_shell_command,
};
mod workflow;

pub use delegate_controls::{
    InterruptDelegateTool, ListDelegatesTool, MessageDelegateTool, WaitDelegateTool,
};
pub use workflow::RunWorkflowTool;
// Re-export Todo tool types.
pub use todo::{TodoInput, TodoItem, TodoStatus, TodoTool};
// Re-export Goal tool types.
pub use goal::{ExitGoalModeTool, GetGoalStatusTool, StartGoalTool, UpdateGoalStatusTool};
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

/// Callback for emitting structured `AgentEvent` values from tools during
/// execution (e.g. delegate/swarm lifecycle events). Set by the runtime via
/// `with_tool_event` so tools can emit normalized events without depending on
/// TUI types or holding a mutable emitter reference.
pub type ToolEventCallback = Arc<dyn Fn(AgentEvent) + Send + Sync>;

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
    pub access: ToolAccess,
    allowed_external_write_paths: BTreeSet<PathBuf>,
    pub bash_timeout: Duration,
    pub max_output_bytes: usize,
    pub cancel_token: CancellationToken,
    pub process_supervisor: ProcessSupervisor,
    pub background_tasks: BackgroundTaskManager,
    /// Shared multi-agent runtime for Delegate and DelegateSwarm tools.
    pub multi_agent: MultiAgentRuntime,
    /// Parent runtime config used to construct real child AgentRuntime instances.
    pub child_config: Option<AgentConfig>,
    /// Parent model client shared by child AgentRuntime instances.
    pub child_model: Option<Arc<dyn ModelClient>>,
    /// Parent tool registry shared by child AgentRuntime instances.
    pub child_tools: Option<Arc<ToolRegistry>>,
    /// Current parent turn for lifecycle events emitted by tools.
    pub current_turn: Option<u32>,
    /// Optional callback for streaming intermediate tool output (e.g. bash
    /// stdout lines). Set by the runtime so tools can emit live updates.
    pub tool_update: Option<ToolUpdateCallback>,
    /// Optional callback for emitting structured `AgentEvent` values from
    /// tools (e.g. delegate lifecycle events). Set by the runtime so delegate
    /// tools can emit `DelegateStarted`/`DelegateFinished` events.
    pub tool_event: Option<ToolEventCallback>,
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("cwd", &self.cwd)
            .field("access", &self.access)
            .field(
                "allowed_external_write_paths",
                &self.allowed_external_write_paths,
            )
            .field("bash_timeout", &self.bash_timeout)
            .field("max_output_bytes", &self.max_output_bytes)
            .field("cancel_token", &self.cancel_token)
            .field("process_supervisor", &self.process_supervisor)
            .field("background_tasks", &self.background_tasks)
            .field("multi_agent", &self.multi_agent)
            .field("child_config", &self.child_config.is_some())
            .field("child_model", &self.child_model.is_some())
            .field("child_tools", &self.child_tools.is_some())
            .field("current_turn", &self.current_turn)
            .field("tool_update", &self.tool_update.is_some())
            .field("tool_event", &self.tool_event.is_some())
            .finish()
    }
}

impl ToolContext {
    pub fn new(workspace_root: impl AsRef<Path>) -> Result<Self, ToolError> {
        let cwd = workspace_root.as_ref().canonicalize()?;
        Ok(Self {
            cwd,
            access: ToolAccess::none(),
            allowed_external_write_paths: BTreeSet::new(),
            bash_timeout: DEFAULT_BASH_TIMEOUT,
            max_output_bytes: 64 * 1024,
            cancel_token: CancellationToken::new(),
            process_supervisor: ProcessSupervisor::default(),
            background_tasks: BackgroundTaskManager::new(),
            multi_agent: MultiAgentRuntime::new(),
            child_config: None,
            child_model: None,
            child_tools: None,
            current_turn: None,
            tool_update: None,
            tool_event: None,
        })
    }

    #[must_use]
    pub const fn with_access(mut self, access: ToolAccess) -> Self {
        self.access = access;
        self
    }

    #[must_use]
    pub fn with_allowed_external_write_paths(
        mut self,
        paths: impl IntoIterator<Item = PathBuf>,
    ) -> Self {
        self.allowed_external_write_paths = paths
            .into_iter()
            .map(|path| normalize_path(&path))
            .collect();
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
    pub fn with_multi_agent(mut self, multi_agent: MultiAgentRuntime) -> Self {
        self.multi_agent = multi_agent;
        self
    }

    #[must_use]
    pub fn with_child_runtime(
        mut self,
        config: AgentConfig,
        model: Arc<dyn ModelClient>,
        tools: Arc<ToolRegistry>,
        current_turn: u32,
    ) -> Self {
        self.child_config = Some(config);
        self.child_model = Some(model);
        self.child_tools = Some(tools);
        self.current_turn = Some(current_turn);
        self
    }

    #[must_use]
    pub fn with_tool_update(mut self, callback: ToolUpdateCallback) -> Self {
        self.tool_update = Some(callback);
        self
    }

    #[must_use]
    pub fn with_tool_event(mut self, callback: ToolEventCallback) -> Self {
        self.tool_event = Some(callback);
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

    /// Emit a structured `AgentEvent` through the tool event callback (if set).
    /// Used by delegate/swarm tools to announce lifecycle transitions.
    pub fn emit_event(&self, event: AgentEvent) {
        if let Some(callback) = &self.tool_event {
            callback(event);
        }
    }

    pub fn ensure_file_read_allowed(&self) -> Result<(), ToolError> {
        if self.access.file_read {
            Ok(())
        } else {
            Err(ToolError::PermissionDenied {
                operation: "file_read",
            })
        }
    }

    pub fn ensure_file_write_allowed(&self) -> Result<(), ToolError> {
        if self.access.file_write {
            Ok(())
        } else {
            Err(ToolError::PermissionDenied {
                operation: "file_write",
            })
        }
    }

    pub fn ensure_shell_allowed(&self) -> Result<(), ToolError> {
        if self.access.shell {
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
        if self.is_allowed_external_write_path(&candidate) {
            return Ok(normalize_path(&candidate));
        }
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
        if self.is_allowed_external_write_path(&candidate) {
            return Ok(normalize_path(&candidate));
        }
        let parent = candidate.parent().unwrap_or(&self.cwd);
        let resolved_parent = normalize_inside_workspace(&self.cwd, parent)?;
        let file_name = candidate
            .file_name()
            .ok_or_else(|| ToolError::PathOutsideWorkspace {
                path: candidate.clone(),
            })?;
        Ok(resolved_parent.join(file_name))
    }

    fn is_allowed_external_write_path(&self, path: &Path) -> bool {
        self.allowed_external_write_paths
            .contains(&normalize_path(path))
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
    tools: BTreeMap<String, Arc<dyn Tool>>,
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
    pub fn with_builtin_child_tools() -> Self {
        let mut registry = Self::new();
        registry.register(read::ReadTool);
        registry.register(list::ListTool);
        registry.register(grep::GrepTool);
        registry.register(find::FindTool);
        registry.register(glob::GlobTool);
        registry.register(self::todo::TodoTool::with_state(Arc::new(Mutex::new(
            Vec::new(),
        ))));
        registry.register(write::WriteTool);
        registry.register(edit::EditTool);
        registry.register(bash::BashTool);
        registry.register(background_tasks::TaskListTool);
        registry.register(background_tasks::TaskOutputTool);
        registry.register(background_tasks::TaskStopTool);
        registry.register(terminal::TerminalTool);
        registry.register(plan_mode::EnterPlanModeTool);
        registry.register(plan_mode::ExitPlanModeTool);
        registry.register(workflow::RunWorkflowTool);
        registry
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
        registry.register(delegate::DelegateTool);
        registry.register(delegate::DelegateSwarmTool);
        registry.register(delegate_controls::ListDelegatesTool);
        registry.register(delegate_controls::WaitDelegateTool);
        registry.register(delegate_controls::InterruptDelegateTool);
        registry.register(delegate_controls::MessageDelegateTool);
        registry.register(workflow::RunWorkflowTool);
        registry
    }

    pub fn register<T>(&mut self, tool: T)
    where
        T: Tool + 'static,
    {
        self.tools.insert(tool.name().to_owned(), Arc::new(tool));
    }

    pub fn register_goal_tools(&mut self, manager: Arc<GoalManager>) {
        self.register(goal::StartGoalTool::new(Arc::clone(&manager)));
        self.register(goal::ExitGoalModeTool::new(Arc::clone(&manager)));
        self.register(goal::UpdateGoalStatusTool::new(Arc::clone(&manager)));
        self.register(goal::GetGoalStatusTool::new(manager));
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

    /// Return a new registry containing only the tools allowed for the given
    /// subagent role.
    #[must_use]
    pub fn filtered_for_agent_role(&self, role: crate::multi_agent::AgentRole) -> Self {
        let profile = crate::multi_agent::AgentProfile::for_role(role);
        let mut filtered = Self::default();
        for (name, tool) in &self.tools {
            if !is_standard_neo_tool_name(name) || profile.allowed_tools.contains(name.as_str()) {
                filtered.tools.insert(name.clone(), Arc::clone(tool));
            }
        }
        filtered
    }
}

fn is_standard_neo_tool_name(name: &str) -> bool {
    matches!(
        name,
        "Read"
            | "List"
            | "Grep"
            | "Find"
            | "Glob"
            | "Bash"
            | "Write"
            | "Edit"
            | "TodoList"
            | "Terminal"
            | "TaskList"
            | "TaskOutput"
            | "TaskStop"
            | "EnterPlanMode"
            | "ExitPlanMode"
            | "Delegate"
            | "DelegateSwarm"
            | "ListDelegates"
            | "WaitDelegate"
            | "InterruptDelegate"
            | "MessageDelegate"
            | "RunWorkflow"
    )
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
    let normalized = normalize_path(path);
    if normalized.starts_with(workspace_root) {
        Ok(normalized)
    } else {
        Err(ToolError::PathOutsideWorkspace {
            path: path.to_path_buf(),
        })
    }
}

pub(crate) fn normalize_path(path: &Path) -> PathBuf {
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
    normalized
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
