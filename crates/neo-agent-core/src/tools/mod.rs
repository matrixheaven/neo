mod ask_user;
mod background_tasks;
mod bash;
mod delegate;
mod delegate_controls;
mod diff;
mod edit;
mod find;
mod glob;
mod goal;
mod grep;
mod list;
mod mcp;
mod mcp_manager;
mod multi_agent_format;
pub mod plan_mode;
mod process_supervisor;
mod read;
mod sessions;
mod shell_env;
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

use crate::{AgentEvent, WorkspaceAccessError, WorkspaceAccessPolicy};

#[allow(clippy::duration_suboptimal_units)]
pub const DEFAULT_BASH_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Format a shell failure message from `exit_code` and optional Unix `signal`.
///
/// Produces a precise, actionable line for tool results so the model does not
/// misdiagnose signal deaths (e.g. `SIGPIPE` from a closed pipe) as OOM or
/// timeout. On Windows `signal` is always `None`, so no Unix-specific wording
/// leaks across platforms.
#[must_use]
pub fn format_shell_failure(exit_code: Option<i32>, signal: Option<i32>) -> String {
    if let Some(code) = exit_code {
        return format!("Command failed with exit code: {code}.");
    }
    if let Some(sig) = signal {
        let (name, hint) = signal_name_and_hint(sig);
        return format!("Command terminated by signal {sig} ({name}){hint}.");
    }
    "Command terminated before returning an exit code.".to_owned()
}

/// Returns `(signal_name, human_readable_hint)` for common Unix signals.
///
/// Signal numbers 1–15 are identical on Linux and macOS; this function is only
/// called on Unix (callers guard with `#[cfg(unix)]` or pass `None` on Windows).
fn signal_name_and_hint(signal: i32) -> (&'static str, &'static str) {
    match signal {
        1 => ("SIGHUP", ""),
        2 => ("SIGINT", " — interrupted by user"),
        9 => (
            "SIGKILL",
            " — possibly killed by the OOM killer or manually",
        ),
        11 => ("SIGSEGV", " — process crashed (segmentation fault)"),
        13 => (
            "SIGPIPE",
            " — likely because a downstream command in a pipe exited early (e.g. grep found a match)",
        ),
        15 => ("SIGTERM", " — timed out or killed gracefully"),
        _ => ("unknown signal", ""),
    }
}

pub use mcp::{
    HttpConfig, HttpOAuthConfig, McpClient, McpError, McpResourceContent, McpResourceDefinition,
    McpResourceRead, McpToolDefinition, McpToolResponse, StdioConfig, build_authorization_manager,
    build_http_client, build_stdio_client,
    oauth::{McpOAuthIdentity, McpOAuthService, McpOAuthServiceConfig, McpOAuthTransportKind},
};
pub use mcp_manager::*;
pub use process_supervisor::ProcessSupervisor;
mod terminal_process;

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
    ShellExecutionRequest, ShellExecutionResult, ShellTermination, execute_model_bash_for_runtime,
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
    pub agent_id: Option<String>,
    pub session_directory: Option<PathBuf>,
    pub access: ToolAccess,
    workspace_policy: WorkspaceAccessPolicy,
    allowed_external_write_paths: BTreeSet<PathBuf>,
    pub bash_timeout: Duration,
    pub max_output_bytes: usize,
    pub cancel_token: CancellationToken,
    pub process_supervisor: ProcessSupervisor,
    pub background_tasks: BackgroundTaskManager,
    /// Shared multi-agent runtime for Delegate and `DelegateSwarm` tools.
    pub multi_agent: MultiAgentRuntime,
    /// Parent runtime config used to construct real child `AgentRuntime` instances.
    pub child_config: Option<AgentConfig>,
    /// Parent model client shared by child `AgentRuntime` instances.
    pub child_model: Option<Arc<dyn ModelClient>>,
    /// Parent tool registry shared by child `AgentRuntime` instances.
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
            .field("agent_id", &self.agent_id)
            .field("session_directory", &self.session_directory)
            .field("access", &self.access)
            .field("workspace_policy_roots", &self.workspace_policy.roots())
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
        let workspace_policy = WorkspaceAccessPolicy::new(&cwd).map_err(map_workspace_error)?;
        Ok(Self {
            cwd,
            agent_id: None,
            session_directory: None,
            access: ToolAccess::none(),
            workspace_policy,
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
    pub fn with_workspace_policy(mut self, workspace_policy: WorkspaceAccessPolicy) -> Self {
        self.workspace_policy = workspace_policy;
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
        self.background_tasks = self
            .agent_session_tasks_dir()
            .map_or(background_tasks.clone(), |tasks_dir| {
                background_tasks.with_persistence_dir(tasks_dir)
            });
        self
    }

    #[must_use]
    pub fn with_agent_session_context(
        mut self,
        session_directory: impl Into<PathBuf>,
        agent_id: impl Into<String>,
    ) -> Self {
        let session_directory = session_directory.into();
        let agent_id = agent_id.into();
        let tasks_dir = crate::session::agent_tasks_dir(&session_directory, &agent_id);
        self.background_tasks = self
            .background_tasks
            .clone()
            .with_persistence_dir(tasks_dir);
        self.session_directory = Some(session_directory);
        self.agent_id = Some(agent_id);
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
            callback(event.without_delegate_prior_messages());
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
        if self.is_allowed_external_write_path(path) {
            return Ok(normalize_path(path));
        }
        self.workspace_policy
            .resolve_read_path(path)
            .map_err(map_workspace_error)
    }

    pub fn resolve_parent_for_write(&self, path: &Path) -> Result<PathBuf, ToolError> {
        if self.is_allowed_external_write_path(path) {
            return Ok(normalize_path(path));
        }
        self.workspace_policy
            .resolve_write_path(path)
            .map_err(map_workspace_error)
    }

    fn is_allowed_external_write_path(&self, path: &Path) -> bool {
        self.allowed_external_write_paths
            .contains(&normalize_path(path))
    }

    fn agent_session_tasks_dir(&self) -> Option<PathBuf> {
        Some(crate::session::agent_tasks_dir(
            self.session_directory.as_deref()?,
            self.agent_id.as_deref()?,
        ))
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
    spec_cache: Mutex<Option<Vec<ToolSpec>>>,
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
        self.invalidate_specs();
    }

    pub fn register_goal_tools(&mut self, manager: Arc<GoalManager>) {
        self.register(goal::StartGoalTool::new(Arc::clone(&manager)));
        self.register(goal::ExitGoalModeTool::new(Arc::clone(&manager)));
        self.register(goal::UpdateGoalStatusTool::new(Arc::clone(&manager)));
        self.register(goal::GetGoalStatusTool::new(manager));
    }

    #[must_use]
    pub fn specs(&self) -> Vec<ToolSpec> {
        let mut cache = self.spec_cache.lock().expect("tool spec cache poisoned");
        cache
            .get_or_insert_with(|| self.tools.values().map(|tool| tool.spec()).collect())
            .clone()
    }

    fn invalidate_specs(&self) {
        *self.spec_cache.lock().expect("tool spec cache poisoned") = None;
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
    ///
    /// Role restrictions only govern the standard builtin toolset; any custom
    /// tool explicitly registered by the caller (e.g. a test probe) is passed
    /// through unchanged so subagent integrations can inject their own tools
    /// without having to extend every role allowlist.
    #[must_use]
    pub fn filtered_for_agent_role(&self, role: crate::multi_agent::AgentRole) -> Self {
        let profile = crate::multi_agent::AgentProfile::for_role(role);
        let mut filtered = Self::default();
        for (name, tool) in &self.tools {
            let allowed = profile.allowed_tools.contains(name.as_str());
            let is_builtin = is_builtin_tool_name(name);
            if allowed || !is_builtin {
                filtered.tools.insert(name.clone(), Arc::clone(tool));
            }
        }
        filtered
    }
}

/// Names of the standard builtin tools registered by
/// [`ToolRegistry::with_builtin_tools`] and its variants. Used by
/// [`ToolRegistry::filtered_for_agent_role`] to distinguish builtin tools
/// (which are subject to per-role allowlisting) from caller-registered custom
/// tools (which always pass through).
fn is_builtin_tool_name(name: &str) -> bool {
    use std::collections::BTreeSet;
    use std::sync::OnceLock;
    static NAMES: OnceLock<BTreeSet<&'static str>> = OnceLock::new();
    let names = NAMES.get_or_init(|| {
        [
            "Read",
            "List",
            "Grep",
            "Find",
            "Glob",
            "TodoList",
            "Write",
            "Edit",
            "Bash",
            "TaskList",
            "TaskOutput",
            "TaskStop",
            "Terminal",
            "EnterPlanMode",
            "ExitPlanMode",
            "Delegate",
            "DelegateSwarm",
            "ListDelegates",
            "WaitDelegate",
            "InterruptDelegate",
            "MessageDelegate",
            "RunWorkflow",
            "StartGoal",
            "ExitGoalMode",
            "UpdateGoalStatus",
            "GetGoalStatus",
            "AskUserQuestion",
            "ListSkills",
            "CreateSkill",
            "MoveSkill",
            "SummarizeSessions",
        ]
        .into_iter()
        .collect()
    });
    names.contains(name)
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

fn map_workspace_error(error: WorkspaceAccessError) -> ToolError {
    match error {
        WorkspaceAccessError::PathOutsideWorkspace { path }
        | WorkspaceAccessError::ReadDenied { path }
        | WorkspaceAccessError::WriteDenied { path } => ToolError::PathOutsideWorkspace { path },
        WorkspaceAccessError::Io(source) => ToolError::Io(source),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[tokio::test]
    async fn tool_context_agent_session_context_persists_main_task_output() {
        let workspace = tempfile::tempdir().expect("workspace");
        let session = tempfile::tempdir().expect("session");
        let ctx = ToolContext::new(workspace.path())
            .expect("tool context")
            .with_agent_session_context(session.path(), crate::session::MAIN_AGENT_ID);

        ctx.background_tasks
            .persist_task_output_for_test("bash-12345678", "hello\n")
            .await
            .expect("persist output");

        assert_eq!(
            tokio::fs::read_to_string(
                session
                    .path()
                    .join("agents")
                    .join("main")
                    .join("tasks")
                    .join("bash-12345678")
                    .join("output.log")
            )
            .await
            .expect("read output"),
            "hello\n"
        );
    }

    #[test]
    fn tool_context_resolve_workspace_path_uses_added_read_root() {
        let primary = tempfile::tempdir().expect("primary");
        let added = tempfile::tempdir().expect("added");
        let file = added.path().join("lib.rs");
        std::fs::write(&file, "pub fn lib() {}").expect("write");
        let policy = crate::WorkspaceAccessPolicy::with_roots(
            primary.path(),
            [crate::WorkspaceAccessRoot {
                path: added.path().canonicalize().expect("canonical added"),
                kind: crate::WorkspaceAccessRootKind::Added,
                read: true,
                write: false,
            }],
        )
        .expect("policy");
        let ctx = ToolContext::new(primary.path())
            .expect("context")
            .with_workspace_policy(policy);

        let resolved = ctx.resolve_workspace_path(&file).expect("resolve");

        assert_eq!(resolved, file.canonicalize().expect("canonical file"));
    }

    #[test]
    fn format_shell_failure_nonzero_exit_code() {
        assert_eq!(
            format_shell_failure(Some(1), None),
            "Command failed with exit code: 1."
        );
        assert_eq!(
            format_shell_failure(Some(127), None),
            "Command failed with exit code: 127."
        );
    }

    #[test]
    fn format_shell_failure_sigpipe_includes_hint() {
        let msg = format_shell_failure(None, Some(13));
        assert!(msg.contains("signal 13"), "{msg}");
        assert!(msg.contains("SIGPIPE"), "{msg}");
        assert!(msg.contains("pipe exited early"), "{msg}");
    }

    #[test]
    fn format_shell_failure_sigkill_includes_hint() {
        let msg = format_shell_failure(None, Some(9));
        assert!(msg.contains("signal 9"), "{msg}");
        assert!(msg.contains("SIGKILL"), "{msg}");
        assert!(msg.contains("OOM"), "{msg}");
    }

    #[test]
    fn format_shell_failure_unknown_signal() {
        let msg = format_shell_failure(None, Some(99));
        assert!(msg.contains("signal 99"), "{msg}");
        assert!(msg.contains("unknown signal"), "{msg}");
    }

    #[test]
    fn format_shell_failure_no_code_no_signal() {
        assert_eq!(
            format_shell_failure(None, None),
            "Command terminated before returning an exit code."
        );
    }

    struct CountingTool {
        name: &'static str,
        schema_calls: Arc<AtomicUsize>,
    }

    impl Tool for CountingTool {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &'static str {
            "count schema calls"
        }

        fn input_schema(&self) -> serde_json::Value {
            self.schema_calls.fetch_add(1, Ordering::SeqCst);
            serde_json::json!({ "type": "object" })
        }

        fn execute<'a>(
            &'a self,
            _ctx: &'a ToolContext,
            _input: serde_json::Value,
        ) -> ToolFuture<'a> {
            Box::pin(async { Ok(ToolResult::ok("ok")) })
        }
    }

    #[test]
    fn specs_are_cached_until_registry_mutates() {
        let first_calls = Arc::new(AtomicUsize::new(0));
        let second_calls = Arc::new(AtomicUsize::new(0));
        let mut registry = ToolRegistry::new();
        registry.register(CountingTool {
            name: "First",
            schema_calls: Arc::clone(&first_calls),
        });

        assert_eq!(registry.specs().len(), 1);
        assert_eq!(registry.specs().len(), 1);
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);

        registry.register(CountingTool {
            name: "Second",
            schema_calls: Arc::clone(&second_calls),
        });

        assert_eq!(registry.specs().len(), 2);
        assert_eq!(first_calls.load(Ordering::SeqCst), 2);
        assert_eq!(second_calls.load(Ordering::SeqCst), 1);
    }
}
