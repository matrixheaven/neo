use std::{
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};

use futures::{FutureExt, future::BoxFuture};
use neo_ai::{ModelSpec, ReasoningSelection, ToolSpec};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::instructions::{InstructionInheritance, InstructionRegistry};
use crate::multi_agent::MultiAgentRuntime;
use crate::permissions::{ApprovalRuleStore, SessionApprovalKey};
use crate::tools::{BackgroundTaskManager, ShellRuntime};
use crate::workspace_policy::WorkspaceAccessPolicy;
use crate::{
    AgentMessage, AgentToolCall, ApprovalRequest, ApprovalResponse, PermissionMode, PlanMode,
    TodoEventData, ToolResult,
};

pub type ContextAppendTransform = Arc<dyn Fn(&[AgentMessage]) -> Vec<AgentMessage> + Send + Sync>;
pub type BeforeToolCallHook = Arc<dyn Fn(&AgentToolCall) -> Option<ToolResult> + Send + Sync>;
pub type AsyncBeforeToolCallHook = Arc<
    dyn Fn(AgentToolCall, CancellationToken) -> BoxFuture<'static, Option<ToolResult>>
        + Send
        + Sync,
>;
pub type AfterToolCallHook = Arc<dyn Fn(&AgentToolCall, ToolResult) -> ToolResult + Send + Sync>;
pub type AsyncAfterToolCallHook = Arc<
    dyn Fn(AgentToolCall, ToolResult, CancellationToken) -> BoxFuture<'static, ToolResult>
        + Send
        + Sync,
>;
pub type ApprovalHandler = Arc<dyn Fn(&ApprovalRequest) -> ApprovalResponse + Send + Sync>;
pub type AsyncApprovalHandler =
    Arc<dyn Fn(ApprovalRequest) -> BoxFuture<'static, ApprovalResponse> + Send + Sync>;

pub const DEFAULT_FIRST_EVENT_TIMEOUT_SECS: u64 = 60;
pub const DEFAULT_STREAM_IDLE_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum QueueMode {
    All,
    OneAtATime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ToolExecutionMode {
    Sequential,
    Parallel,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentConfig {
    pub model: ModelSpec,
    pub workspace_root: Option<PathBuf>,
    #[serde(skip)]
    #[schemars(skip)]
    pub workspace_policy: Arc<RwLock<Option<WorkspaceAccessPolicy>>>,
    pub system_prompt: Option<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub max_retries: u32,
    pub first_event_timeout_secs: u64,
    pub stream_idle_timeout_secs: u64,
    pub reasoning: ReasoningSelection,
    pub replay_reasoning: bool,
    pub tools: Vec<ToolSpec>,
    pub steering_queue_mode: QueueMode,
    pub follow_up_queue_mode: QueueMode,
    pub tool_execution_mode: ToolExecutionMode,
    pub permission_mode: PermissionMode,
    /// Shared live permission state. Updated by the TUI when the user runs
    /// `/ask`, `/auto`, `/yolo` (or opens `/permissions`) even mid-turn, and
    /// read by `permission_preparation_for_mode` at every tool call so a
    /// running turn immediately honors the new posture.
    ///
    /// `permission_mode` above is kept for serialization/snapshots and initial
    /// state; live evaluation must go through [`AgentConfig::current_permission_mode`].
    #[serde(skip)]
    #[schemars(skip)]
    pub live_permission_mode: Arc<RwLock<PermissionMode>>,
    pub compaction: Option<CompactionSettings>,
    /// Runtime-observed context overflow point.
    /// Set when provider reports overflow; used to cap effective max.
    #[serde(skip)]
    #[schemars(skip)]
    pub observed_max_context_tokens: Arc<Mutex<Option<usize>>>,
    #[serde(skip)]
    #[schemars(skip)]
    pub context_append_transform: Option<ContextAppendTransform>,
    #[serde(skip)]
    #[schemars(skip)]
    pub before_tool_call: Option<BeforeToolCallHook>,
    #[serde(skip)]
    #[schemars(skip)]
    pub async_before_tool_call: Option<AsyncBeforeToolCallHook>,
    #[serde(skip)]
    #[schemars(skip)]
    pub after_tool_call: Option<AfterToolCallHook>,
    #[serde(skip)]
    #[schemars(skip)]
    pub async_after_tool_call: Option<AsyncAfterToolCallHook>,
    #[serde(skip)]
    #[schemars(skip)]
    pub approval_handler: Option<ApprovalHandler>,
    #[serde(skip)]
    #[schemars(skip)]
    pub async_approval_handler: Option<AsyncApprovalHandler>,
    /// Shared plan-mode state. Checked before every tool call via
    /// [`check_plan_mode_guard`]. Updated when the model calls
    /// `EnterPlanMode` / `ExitPlanMode`.
    #[serde(skip)]
    #[schemars(skip)]
    pub plan_mode: Arc<RwLock<PlanMode>>,
    /// True while the TUI is in AI-assisted goal authoring mode.
    #[serde(skip)]
    #[schemars(skip)]
    pub goal_mode_authoring: bool,
    /// Narrow reusable approval grants for this session. Keyed by
    /// [`SessionApprovalKey`] (exact canonical command + cwd, exact file
    /// write/edit path), never by tool name, so approving one command never
    /// approves a different command on the same tool.
    #[serde(skip)]
    #[schemars(skip)]
    pub session_approvals: Arc<Mutex<std::collections::HashSet<SessionApprovalKey>>>,
    /// Persistent prefix approval rules (Layer 2). Loaded from
    /// `~/.neo/approval_rules.json` on startup; a prefix match auto-approves
    /// any shell command whose argv starts with the rule prefix.
    #[serde(skip)]
    #[schemars(skip)]
    pub prefix_approval_rules: Arc<Mutex<ApprovalRuleStore>>,
    /// Home directory for persistent state (e.g. `~/.neo`, approval rules).
    pub home_dir: Option<PathBuf>,
    /// Session directory for this turn. Used to store agent-scoped plans, goals,
    /// tasks, and image blobs scoped to the active session.
    #[serde(skip)]
    #[schemars(skip)]
    pub session_directory: Option<PathBuf>,
    /// Runtime agent id for agent-scoped session artifacts.
    #[serde(skip)]
    #[schemars(skip)]
    pub agent_id: Option<String>,
    /// Shared todo list state. Used by `TodoTool` read mode and kept in sync
    /// with replayed/runtime `TodoUpdated` events.
    #[serde(skip)]
    #[schemars(skip)]
    pub todos: Arc<Mutex<Vec<TodoEventData>>>,
    /// Shared background task manager for Bash and `AskUserQuestion` background tasks.
    #[serde(skip)]
    #[schemars(skip)]
    pub background_tasks: BackgroundTaskManager,
    #[serde(skip)]
    #[schemars(skip)]
    pub shell_runtime: ShellRuntime,
    #[serde(skip)]
    #[schemars(skip)]
    pub workflow_capability: crate::workflow::WorkflowCapability,
    /// Live dependencies used by workflow-hosted tool calls. Every invocation
    /// snapshots this resolver before awaiting permission or execution.
    #[serde(skip)]
    #[schemars(skip)]
    pub workflow_dispatch_resolver: super::workflow_dispatch::WorkflowDispatchResolver,
    /// Shared manual `/compact` request. `Some(instruction)` means a manual
    /// compaction was requested with an optional custom instruction; `None`
    /// means no request is pending. Set by the TUI and taken by runtime compaction.
    #[serde(skip)]
    #[schemars(skip)]
    pub manual_compact_request: Arc<std::sync::Mutex<Option<String>>>,
    /// Shared multi-agent runtime for Delegate and `DelegateSwarm` tools.
    #[serde(skip)]
    #[schemars(skip)]
    pub multi_agent: MultiAgentRuntime,
    /// Session-shared instruction registry handle. Only the `Arc` handle is
    /// cloned into child agent configs: source/revision caches are shared
    /// session-wide while model visibility stays agent-local. Never
    /// serialized, and never a process-global.
    #[serde(skip)]
    #[schemars(skip)]
    pub instruction_registry: Option<Arc<InstructionRegistry>>,
    /// Session-shared record of currently blocked instruction scopes, keyed
    /// per agent id inside each entry. While a scope is blocked, read-only
    /// diagnosis may proceed but mutation/execution batches touching the
    /// scope stay blocked until the source fingerprint changes. Shared
    /// across per-turn runtime reconstruction; agent visibility stays local.
    #[serde(skip)]
    #[schemars(skip)]
    pub blocked_instruction_scopes: Arc<Mutex<Vec<BlockedInstructionScope>>>,
    /// How a child agent spawned from this config inherits instruction
    /// visibility. Set from the delegate context mode at child creation and
    /// immutable once cloned into the child config.
    #[serde(skip, default = "default_instruction_inheritance")]
    #[schemars(skip)]
    pub instruction_inheritance: InstructionInheritance,
    /// Cached token estimate for `tools`. Computed once on first access;
    /// reset to default on clone (the value is unchanged because `tools`
    /// is cloned by value, but re-computation is cheap relative to the
    /// avoidance of repeated `input_schema.to_string()` calls).
    #[serde(skip)]
    #[schemars(skip)]
    pub cached_tool_spec_tokens: std::sync::OnceLock<usize>,
}

/// Default instruction inheritance for configs that never spawn children:
/// the conservative global/workspace baseline (no parent scope seeding).
const fn default_instruction_inheritance() -> InstructionInheritance {
    InstructionInheritance::Summary
}

/// One currently blocked instruction scope for one agent. Registered when a
/// preflight reconciliation returns `Block`; consulted while the blocked
/// epoch's fingerprint stays current so mutation/execution batches touching
/// the scope keep blocking while read-only diagnosis proceeds. Entries are
/// dropped when a fresh reconciliation of a covered target shows the failure
/// fingerprint changed (fixed or re-resolved).
#[derive(Debug, Clone)]
pub struct BlockedInstructionScope {
    /// Agent whose model visibility owns this blocked state.
    pub agent_id: String,
    /// Fingerprint hash of the blocked reconciliation (failure identity).
    pub fingerprint: String,
    /// Canonical directories governed by the blocked reconciliation: probe
    /// targets plus the epoch's scope directories.
    pub directories: Vec<PathBuf>,
    /// Display-safe failure data shown in blocked tool results.
    pub failure: crate::instructions::InstructionFailure,
    /// Generation of the blocked epoch (already visible to the model).
    pub generation: u64,
}

impl AgentConfig {
    #[must_use]
    pub fn for_model(model: ModelSpec) -> Self {
        Self {
            model,
            workspace_root: None,
            workspace_policy: Arc::new(RwLock::new(None)),
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            max_retries: 5,
            first_event_timeout_secs: DEFAULT_FIRST_EVENT_TIMEOUT_SECS,
            stream_idle_timeout_secs: DEFAULT_STREAM_IDLE_TIMEOUT_SECS,
            reasoning: ReasoningSelection::Off,
            replay_reasoning: true,
            tools: Vec::new(),
            steering_queue_mode: QueueMode::All,
            follow_up_queue_mode: QueueMode::All,
            tool_execution_mode: ToolExecutionMode::Parallel,
            permission_mode: PermissionMode::default(),
            live_permission_mode: Arc::new(RwLock::new(PermissionMode::default())),
            compaction: None,
            observed_max_context_tokens: Arc::new(Mutex::new(None)),
            context_append_transform: None,
            before_tool_call: None,
            async_before_tool_call: None,
            after_tool_call: None,
            async_after_tool_call: None,
            approval_handler: None,
            async_approval_handler: None,
            plan_mode: Arc::new(RwLock::new(PlanMode::default())),
            goal_mode_authoring: false,
            session_approvals: Arc::new(Mutex::new(std::collections::HashSet::new())),
            prefix_approval_rules: Arc::new(Mutex::new(ApprovalRuleStore::default())),
            home_dir: None,
            session_directory: None,
            agent_id: None,
            todos: Arc::new(Mutex::new(Vec::new())),
            background_tasks: BackgroundTaskManager::new(),
            shell_runtime: ShellRuntime::default(),
            workflow_capability: crate::workflow::WorkflowCapability::default(),
            workflow_dispatch_resolver: super::workflow_dispatch::WorkflowDispatchResolver::default(
            ),
            manual_compact_request: Arc::new(std::sync::Mutex::new(None)),
            multi_agent: MultiAgentRuntime::new(),
            instruction_registry: None,
            blocked_instruction_scopes: Arc::new(Mutex::new(Vec::new())),
            instruction_inheritance: default_instruction_inheritance(),
            cached_tool_spec_tokens: std::sync::OnceLock::new(),
        }
    }

    #[must_use]
    pub fn with_system_prompt(mut self, system_prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(system_prompt.into());
        self
    }

    #[must_use]
    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = tools;
        self
    }

    #[must_use]
    pub const fn with_queue_modes(mut self, steering: QueueMode, follow_up: QueueMode) -> Self {
        self.steering_queue_mode = steering;
        self.follow_up_queue_mode = follow_up;
        self
    }

    #[must_use]
    pub const fn with_tool_execution_mode(mut self, mode: ToolExecutionMode) -> Self {
        self.tool_execution_mode = mode;
        self
    }

    #[must_use]
    pub fn with_permission_mode(mut self, mode: PermissionMode) -> Self {
        self.permission_mode = mode;
        if let Ok(mut live) = self.live_permission_mode.write() {
            *live = mode;
        }
        self
    }

    /// Replace the shared live permission state. The static `permission_mode`
    /// is seeded from the live value so they stay consistent at attachment time.
    #[must_use]
    pub fn with_live_permission_mode(
        mut self,
        live_permission_mode: Arc<RwLock<PermissionMode>>,
    ) -> Self {
        if let Ok(mode) = live_permission_mode.read().map(|guard| *guard) {
            self.permission_mode = mode;
        }
        self.live_permission_mode = live_permission_mode;
        self
    }

    pub fn with_workspace_root(
        mut self,
        workspace_root: impl Into<PathBuf>,
    ) -> Result<Self, std::io::Error> {
        self.workspace_root = Some(workspace_root.into().canonicalize()?);
        Ok(self)
    }

    #[must_use]
    pub fn with_workspace_policy(
        mut self,
        workspace_policy: Arc<RwLock<Option<WorkspaceAccessPolicy>>>,
    ) -> Self {
        self.workspace_policy = workspace_policy;
        self
    }

    #[must_use]
    pub fn with_shell_runtime(mut self, shell_runtime: ShellRuntime) -> Self {
        self.shell_runtime = shell_runtime;
        self
    }

    #[must_use]
    pub const fn with_compaction(mut self, settings: CompactionSettings) -> Self {
        self.compaction = Some(settings);
        self
    }

    #[must_use]
    pub fn with_context_append_transform(
        mut self,
        transform: impl Fn(&[AgentMessage]) -> Vec<AgentMessage> + Send + Sync + 'static,
    ) -> Self {
        self.context_append_transform = Some(Arc::new(transform));
        self
    }

    #[must_use]
    pub fn with_before_tool_call(
        mut self,
        hook: impl Fn(&AgentToolCall) -> Option<ToolResult> + Send + Sync + 'static,
    ) -> Self {
        self.before_tool_call = Some(Arc::new(hook));
        self
    }

    #[must_use]
    pub fn with_async_before_tool_call<F, Fut>(mut self, hook: F) -> Self
    where
        F: Fn(AgentToolCall, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<ToolResult>> + Send + 'static,
    {
        self.async_before_tool_call = Some(Arc::new(move |call, cancel_token| {
            hook(call, cancel_token).boxed()
        }));
        self
    }

    #[must_use]
    pub fn with_after_tool_call(
        mut self,
        hook: impl Fn(&AgentToolCall, ToolResult) -> ToolResult + Send + Sync + 'static,
    ) -> Self {
        self.after_tool_call = Some(Arc::new(hook));
        self
    }

    #[must_use]
    pub fn with_async_after_tool_call<F, Fut>(mut self, hook: F) -> Self
    where
        F: Fn(AgentToolCall, ToolResult, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ToolResult> + Send + 'static,
    {
        self.async_after_tool_call = Some(Arc::new(move |call, result, cancel_token| {
            hook(call, result, cancel_token).boxed()
        }));
        self
    }

    #[must_use]
    pub fn with_approval_handler(
        mut self,
        handler: impl Fn(&ApprovalRequest) -> ApprovalResponse + Send + Sync + 'static,
    ) -> Self {
        self.approval_handler = Some(Arc::new(handler));
        self
    }

    #[must_use]
    pub fn with_async_approval_handler<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(ApprovalRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ApprovalResponse> + Send + 'static,
    {
        self.async_approval_handler = Some(Arc::new(move |request| handler(request).boxed()));
        self
    }

    /// Set the home directory used for plan file creation.
    #[must_use]
    pub fn with_home_dir(mut self, home_dir: impl Into<PathBuf>) -> Self {
        self.home_dir = Some(home_dir.into());
        self
    }

    /// Set the session directory used for plan files and image blobs.
    #[must_use]
    pub fn with_session_directory(mut self, session_directory: impl Into<PathBuf>) -> Self {
        let session_directory = session_directory.into();
        self.multi_agent = self
            .multi_agent
            .clone()
            .with_session_directory(session_directory.clone());
        self.session_directory = Some(session_directory);
        self
    }

    /// Set the runtime agent id used for agent-scoped session artifacts.
    #[must_use]
    pub fn with_agent_id(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = Some(agent_id.into());
        self
    }

    /// Path to the persistent prefix-approval-rules file, when a home dir is set.
    #[must_use]
    pub fn approval_rules_path(&self) -> Option<PathBuf> {
        self.home_dir
            .as_ref()
            .map(|home| home.join("approval_rules.json"))
    }

    /// Load persistent Layer-2 prefix rules from `<home>/approval_rules.json`.
    /// Missing or malformed file is treated as an empty rule set (no error).
    pub fn load_prefix_approval_rules(&mut self) {
        let Some(path) = self.approval_rules_path() else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        match serde_json::from_str::<ApprovalRuleStore>(&text) {
            Ok(store) => {
                if let Ok(mut guard) = self.prefix_approval_rules.lock() {
                    *guard = store;
                }
            }
            Err(err) => {
                if let Ok(mut guard) = self.prefix_approval_rules.lock() {
                    *guard = ApprovalRuleStore::default();
                }
                tracing::warn!(path = %path.display(), error = %err, "ignoring malformed approval rules");
            }
        }
    }

    /// Persist the current Layer-2 prefix rules to `<home>/approval_rules.json`.
    /// Returns the path written, or `None` if no home dir is configured.
    pub fn save_prefix_approval_rules(&self) -> anyhow::Result<Option<PathBuf>> {
        let Some(path) = self.approval_rules_path() else {
            return Ok(None);
        };
        let store = self
            .prefix_approval_rules
            .lock()
            .map_err(|_| anyhow::anyhow!("approval rule store lock poisoned"))?
            .clone();
        let text = serde_json::to_string_pretty(&store)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                tracing::warn!(path = %parent.display(), %error, "failed to create approval rules directory");
                error
            })?;
        }
        std::fs::write(&path, text).map_err(|error| {
            tracing::warn!(path = %path.display(), %error, "failed to write approval rules");
            error
        })?;
        Ok(Some(path))
    }

    /// Replace the shared plan-mode state. Useful when constructing from a
    /// pre-existing state (e.g. after replay).
    #[must_use]
    pub fn with_plan_mode(mut self, plan_mode: Arc<RwLock<PlanMode>>) -> Self {
        self.plan_mode = plan_mode;
        self
    }

    #[must_use]
    pub const fn with_goal_mode_authoring(mut self, active: bool) -> Self {
        self.goal_mode_authoring = active;
        self
    }

    /// Replace the shared todo list state.
    #[must_use]
    pub fn with_todos(mut self, todos: Arc<Mutex<Vec<TodoEventData>>>) -> Self {
        self.todos = todos;
        self
    }

    /// Replace the shared background task manager.
    #[must_use]
    pub fn with_background_tasks(mut self, background_tasks: BackgroundTaskManager) -> Self {
        self.background_tasks = background_tasks;
        self
    }

    /// Replace the shared one-shot workflow launch capability.
    #[must_use]
    pub fn with_workflow_capability(
        mut self,
        workflow_capability: crate::workflow::WorkflowCapability,
    ) -> Self {
        self.workflow_capability = workflow_capability;
        self
    }

    /// Replace the session-shared workflow dispatch resolver.
    #[must_use]
    pub fn with_workflow_dispatch_resolver(
        mut self,
        resolver: super::workflow_dispatch::WorkflowDispatchResolver,
    ) -> Self {
        self.workflow_dispatch_resolver = resolver;
        self
    }

    /// Replace the shared multi-agent runtime.
    #[must_use]
    pub fn with_multi_agent(mut self, multi_agent: MultiAgentRuntime) -> Self {
        self.multi_agent = if let Some(session_directory) = &self.session_directory {
            multi_agent.with_session_directory(session_directory.clone())
        } else {
            multi_agent
        };
        self
    }
}

/// Safety ratio: observed overflow point × this = safe effective max.
const OVERFLOW_SAFETY_RATIO: f64 = 0.85;

/// Effective max context tokens, considering observed overflow.
///
/// Returns `min(configured, observed × 0.85)`.
#[must_use]
pub fn effective_max_context_tokens(config: &AgentConfig) -> usize {
    let configured = config.model.capabilities.max_context_tokens.unwrap_or(0) as usize;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let observed = config
        .observed_max_context_tokens
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .map(|v| ((v as f64) * OVERFLOW_SAFETY_RATIO) as usize);

    match (configured, observed) {
        (0, Some(o)) => o,
        (c, Some(o)) => c.min(o),
        (c, None) => c,
    }
}

/// Record an observed context overflow point.
///
/// Only updates if the new value (× 0.85) is smaller than the current
/// observation — never increases the effective max.
pub fn observe_context_overflow(config: &AgentConfig, estimated_tokens: usize) {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let safe = ((estimated_tokens as f64) * OVERFLOW_SAFETY_RATIO) as usize;
    let mut guard = config
        .observed_max_context_tokens
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match *guard {
        Some(current) if safe < current => *guard = Some(safe),
        None => *guard = Some(safe),
        _ => {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CompactionSettings {
    pub enabled: bool,
    pub max_estimated_tokens: usize,
    pub keep_recent_messages: usize,
    /// Fraction of `max_context_tokens` at which auto-compaction triggers.
    pub trigger_ratio: f64,
    /// Reserved headroom in tokens that forces compaction when
    /// `used + reserved >= max_context_tokens`.
    pub reserved_context_tokens: usize,
    /// Maximum recent messages to keep during auto-compaction.
    pub max_recent_messages: usize,
    /// Whether experimental micro compaction (old tool-result truncation) is on.
    pub micro_enabled: bool,
    /// Number of recent messages exempt from micro compaction.
    pub micro_keep_recent: usize,
    /// Maximum compaction rounds per invocation.
    pub max_rounds: usize,
    /// Maximum retry attempts for empty/truncated summaries.
    pub max_retry_attempts: u32,
}

impl CompactionSettings {
    #[must_use]
    pub const fn new(max_estimated_tokens: usize, keep_recent_messages: usize) -> Self {
        Self {
            enabled: true,
            max_estimated_tokens,
            keep_recent_messages,
            trigger_ratio: 0.85,
            reserved_context_tokens: 50_000,
            max_recent_messages: 4,
            micro_enabled: false,
            micro_keep_recent: 20,
            max_rounds: 5,
            max_retry_attempts: 5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neo_ai::{ApiKind, ModelCapabilities, ModelSpec, ProviderId};

    /// Test helper — constructs a minimal `AgentConfig` via `for_model`.
    /// `AgentConfig` has NO Default impl (closure/handler fields).
    fn test_config() -> AgentConfig {
        let spec = ModelSpec {
            provider: ProviderId("test".to_owned()),
            model: "test-model".to_owned(),
            api: ApiKind::OpenAi,
            capabilities: ModelCapabilities {
                max_context_tokens: Some(200_000),
                ..ModelCapabilities::chat()
            },
        };
        AgentConfig::for_model(spec)
    }

    #[test]
    fn effective_max_uses_observed_when_smaller() {
        let mut config = test_config();
        config.model.capabilities.max_context_tokens = Some(200_000);
        *config.observed_max_context_tokens.lock().unwrap() = Some(100_000);

        let effective = effective_max_context_tokens(&config);
        // observed (100k * 0.85 = 85k) < configured (200k) → use 85k
        assert_eq!(effective, 85_000);
    }

    #[test]
    fn effective_max_uses_configured_when_no_observation() {
        let config = test_config();
        // observed is None → use configured 200k
        let effective = effective_max_context_tokens(&config);
        assert_eq!(effective, 200_000);
    }

    #[test]
    fn observe_context_overflow_only_updates_smaller() {
        let mut config = test_config();
        config.model.capabilities.max_context_tokens = Some(200_000);

        observe_context_overflow(&config, 180_000);
        // 180k * 0.85 = 153k
        assert_eq!(
            *config.observed_max_context_tokens.lock().unwrap(),
            Some(153_000)
        );

        // Second overflow at 220k → 220k * 0.85 = 187k > 153k → should NOT update
        observe_context_overflow(&config, 220_000);
        assert_eq!(
            *config.observed_max_context_tokens.lock().unwrap(),
            Some(153_000)
        );

        // Third overflow at 100k → 100k * 0.85 = 85k < 153k → should update
        observe_context_overflow(&config, 100_000);
        assert_eq!(
            *config.observed_max_context_tokens.lock().unwrap(),
            Some(85_000)
        );
    }

    #[test]
    fn compaction_settings_disable_micro_by_default() {
        let settings = CompactionSettings::new(100_000, 4);

        assert!(!settings.micro_enabled);
    }

    #[test]
    fn malformed_approval_rules_clear_stale_typed_state() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("approval_rules.json"), b"not json").unwrap();
        let mut config = test_config().with_home_dir(temp.path());
        config
            .prefix_approval_rules
            .lock()
            .unwrap()
            .prefix_rules
            .push(crate::permissions::PrefixApprovalRule {
                prefix: vec!["cargo".to_owned()],
                label: "cargo".to_owned(),
            });

        config.load_prefix_approval_rules();

        assert!(
            config
                .prefix_approval_rules
                .lock()
                .unwrap()
                .prefix_rules
                .is_empty()
        );
    }
}
