use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, RwLock},
};

use neo_agent_core::multi_agent::MultiAgentRuntime;
use neo_agent_core::{
    BackgroundTaskManager, DEFAULT_FIRST_EVENT_TIMEOUT_SECS, DEFAULT_STREAM_IDLE_TIMEOUT_SECS,
    ShellLimits, ShellRuntime, WorkspaceAccessPolicy,
};
use neo_agent_core::{PermissionMode, QueueMode, ToolExecutionMode};
use neo_tui::notify::NotificationMode;
use neo_tui::terminal_image::ImageProtocolPreference;
use serde::{Deserialize, Serialize};

use crate::{cli::Cli, themes::ResolvedTheme, trust};

mod atomic_file;
mod loader;
mod matching;
pub(crate) mod mutations;
mod paths;
mod types;

pub(crate) use crate::themes::ThemeResolution;
pub(crate) use matching::scoped_models;
#[allow(unused_imports)]
pub(crate) use paths::{
    default_config_path, expand_user_path, expand_user_path_with_home, global_prompts_dir,
    neo_home, user_home, workspace_sessions_dir,
};

// Re-export config types for callers that access them via `crate::config::*`.
pub(crate) use loader::update_file_config;
#[cfg(test)]
pub(crate) use loader::{
    config_process_lock_is_available, read_file_config, update_file_config_with_lock_hook,
    update_file_config_with_writer,
};
pub(crate) use types::FileConfig;
pub use types::{McpConfig, McpServerConfig, McpTransport, ModelConfig, ProviderConfig};

#[derive(Debug, Clone, Default)]
pub struct ConfigOverrides {
    pub config_path: Option<PathBuf>,
    pub yolo: bool,
    pub auto: bool,
    pub(crate) trust_store: Option<trust::ProjectTrustStore>,
    pub(crate) project_dir: Option<PathBuf>,
}

impl ConfigOverrides {
    pub fn from_cli(cli: &Cli) -> Self {
        Self {
            config_path: cli.config.clone(),
            yolo: cli.yolo,
            auto: cli.auto,
            trust_store: None,
            project_dir: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub default_model: String,
    pub default_provider: String,
    pub providers: BTreeMap<String, ProviderConfig>,
    /// Models defined inline in config.toml `[models.<alias>]`.
    pub models: BTreeMap<String, ModelConfig>,
    #[serde(skip)]
    pub model_scope: Vec<String>,
    pub sessions_dir: PathBuf,
    pub permission_mode: PermissionMode,
    /// Shared live permission state for the interactive TUI. Updated by
    /// `/ask`, `/auto`, `/yolo` (and `/permissions`) even while a turn is
    /// running, and read at every tool-call approval so the active turn honors
    /// the latest mode without needing to be cancelled. Seeded from
    /// `permission_mode` at construction.
    #[serde(skip)]
    pub live_permission_mode: Arc<RwLock<PermissionMode>>,
    #[serde(skip)]
    pub workspace_policy: Arc<RwLock<Option<WorkspaceAccessPolicy>>>,
    pub defaults: Defaults,
    pub runtime: RuntimeConfig,
    /// Shared background task registry for the interactive session.
    ///
    /// Model-initiated Bash background jobs, background `AskUser` questions, and
    /// user shell-mode Ctrl+B detach all need to land in the same task list so
    /// `TaskList`/`TaskOutput` can observe them on later turns.
    #[serde(skip)]
    pub background_tasks: BackgroundTaskManager,
    /// Session-shared durable workflow owner.
    #[serde(skip)]
    pub workflow_runtime: neo_agent_core::workflow::WorkflowRuntime,
    /// Session-shared trusted definition registry (discovery/save only).
    #[serde(skip)]
    pub workflow_definitions: neo_agent_core::workflow::WorkflowDefinitionRegistry,
    /// Session-shared resolver for live workflow child dispatch dependencies.
    #[serde(skip)]
    pub workflow_dispatch_resolver: neo_agent_core::runtime::WorkflowDispatchResolver,
    /// Shared multi-agent runtime for Delegate/DelegateSwarm tasks in this app session.
    #[serde(skip)]
    pub multi_agent: MultiAgentRuntime,
    pub tui: TuiConfig,
    #[serde(skip)]
    pub theme: ResolvedTheme,
    /// Provenance of startup theme selection, including the bounded legacy
    /// fallback marker and the diagnostic for an unusable explicit id.
    #[serde(skip)]
    pub theme_resolution: ThemeResolution,
    pub mcp: McpConfig,
    #[serde(skip)]
    pub prompt_templates: Vec<String>,
    #[serde(skip)]
    pub system_prompt_file: Option<PathBuf>,
    #[serde(skip)]
    pub extra_skill_dirs: Vec<String>,
    #[serde(skip)]
    pub skill_path: Vec<String>,
    #[serde(skip)]
    pub project_trusted: bool,
    #[serde(skip)]
    pub project_trust: trust::ProjectTrustState,
    pub project_dir: PathBuf,

    /// Whether the configuration was loaded from an existing config file. When
    /// false, the application is using hard-coded defaults and should indicate
    /// to the user that no providers or models are configured.
    #[serde(skip)]
    pub config_file_exists: bool,

    #[serde(skip)]
    pub config_path: PathBuf,
}

impl AppConfig {
    pub(crate) fn inherit_live_state(&mut self, current: &Self) {
        self.permission_mode = current.permission_mode;
        self.live_permission_mode = Arc::clone(&current.live_permission_mode);
        self.workspace_policy = Arc::clone(&current.workspace_policy);
        self.background_tasks = current.background_tasks.clone();
        self.workflow_runtime = current.workflow_runtime.clone();
        self.runtime.workflow = current.workflow_runtime.limits();
        self.workflow_definitions = current.workflow_definitions.clone();
        self.workflow_dispatch_resolver = current.workflow_dispatch_resolver.clone();
        self.multi_agent = current.multi_agent.clone();
        self.runtime.shell = current.runtime.shell;
        self.runtime.shell_runtime = current.runtime.shell_runtime.clone();
    }

    /// The canonical `provider/model` display label for the configured default
    /// model. This is the single source of truth for label formatting.
    ///
    /// `default_model` stores the model alias. If that alias exists in
    /// `[models.*]`, the label is derived from the referenced provider/model.
    /// Otherwise built-in bare model ids such as `gpt-4.1` are prefixed with
    /// `default_provider`, while already-qualified values are used as-is.
    #[must_use]
    pub fn default_model_label(&self) -> String {
        if !self.config_file_exists {
            return "No configured providers/models".to_owned();
        }
        if let Some(model) = self.models.get(&self.default_model) {
            return format!("{}/{}", model.provider, model.model);
        }
        if self.default_model.contains('/') {
            self.default_model.clone()
        } else {
            format!("{}/{}", self.default_provider, self.default_model)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Defaults {
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub reasoning: neo_ai::ReasoningSelection,
    pub replay_reasoning: bool,
    pub steering_queue_mode: QueueMode,
    pub follow_up_queue_mode: QueueMode,
    pub tool_execution_mode: ToolExecutionMode,
    pub retry: RuntimeRetryConfig,
    pub compaction: Option<RuntimeCompactionConfig>,
    pub shell: ShellLimits,
    pub workflow: neo_agent_core::workflow::WorkflowLimits,
    #[serde(skip)]
    pub shell_runtime: ShellRuntime,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            temperature: None,
            max_tokens: None,
            reasoning: neo_ai::ReasoningSelection::Off,
            replay_reasoning: true,
            steering_queue_mode: QueueMode::All,
            follow_up_queue_mode: QueueMode::All,
            tool_execution_mode: ToolExecutionMode::Parallel,
            retry: RuntimeRetryConfig::default(),
            compaction: Some(RuntimeCompactionConfig::default()),
            shell: ShellLimits::default(),
            workflow: neo_agent_core::workflow::WorkflowLimits::default(),
            shell_runtime: ShellRuntime::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeRetryConfig {
    pub max_retries: u32,
    pub first_event_timeout_secs: u64,
    pub stream_idle_timeout_secs: u64,
}

impl Default for RuntimeRetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 5,
            first_event_timeout_secs: DEFAULT_FIRST_EVENT_TIMEOUT_SECS,
            stream_idle_timeout_secs: DEFAULT_STREAM_IDLE_TIMEOUT_SECS,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeCompactionConfig {
    pub enabled: bool,
    pub max_estimated_tokens: usize,
    pub keep_recent_messages: usize,
    pub trigger_ratio: f64,
    pub reserved_context_tokens: usize,
    pub max_recent_messages: usize,
    pub micro_enabled: bool,
    pub micro_keep_recent: usize,
    #[serde(default = "default_snip_enabled")]
    pub snip_enabled: bool,
    #[serde(default = "default_snip_min_tokens")]
    pub snip_min_tokens: usize,
    #[serde(default = "default_snip_keep_recent")]
    pub snip_keep_recent: usize,
    #[serde(default = "default_snip_trigger_ratio")]
    pub snip_trigger_ratio: f64,
    pub max_rounds: usize,
    pub max_retry_attempts: u32,
}

/// Snip rewrites old tool results in the model input and therefore breaks the
/// provider prefix cache once per rewritten result; keep it opt-in so paid
/// providers keep the append-only prefix by default.
fn default_snip_enabled() -> bool {
    false
}

fn default_snip_min_tokens() -> usize {
    1_000
}

fn default_snip_keep_recent() -> usize {
    16
}

fn default_snip_trigger_ratio() -> f64 {
    0.6
}

impl Default for RuntimeCompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_estimated_tokens: types::default_runtime_compaction_max_estimated_tokens(),
            keep_recent_messages: types::default_runtime_compaction_keep_recent_messages(),
            trigger_ratio: 0.85,
            reserved_context_tokens: 50_000,
            max_recent_messages: 4,
            micro_enabled: false,
            micro_keep_recent: 20,
            snip_enabled: false,
            snip_min_tokens: 1_000,
            snip_keep_recent: 16,
            snip_trigger_ratio: 0.6,
            max_rounds: 5,
            max_retry_attempts: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
    #[serde(default)]
    pub image_protocol: ImageProtocolPreference,
    #[serde(default)]
    pub keybindings: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub completion_notification: NotificationMode,
    #[serde(default)]
    pub question_notification: NotificationMode,
    /// Explicit startup theme id from the persisted config, kept as the raw
    /// logical id string so an invalid id still resolves to the built-in
    /// default with a visible diagnostic instead of silently re-entering
    /// sorted-first discovery.
    #[serde(default)]
    pub theme: Option<String>,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            image_protocol: ImageProtocolPreference::default(),
            keybindings: BTreeMap::new(),
            completion_notification: NotificationMode::Bell,
            question_notification: NotificationMode::None,
            theme: None,
        }
    }
}
#[cfg(test)]
mod test_cases {

    use std::{fs, path::PathBuf};

    use tempfile::TempDir;

    use crate::config::{AppConfig, ConfigOverrides};

    use crate::trust::ProjectTrustStore;

    fn temp_project_config(content: &str) -> (TempDir, PathBuf, PathBuf) {
        let temp = TempDir::new().expect("temp dir");
        let config_path = temp.path().join("config.toml");
        fs::write(&config_path, content).expect("write config");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project");
        (temp, config_path, project_dir)
    }

    fn load_config(config_path: PathBuf, project_dir: PathBuf) -> AppConfig {
        AppConfig::load(ConfigOverrides {
            config_path: Some(config_path),
            yolo: false,
            auto: false,
            trust_store: None,
            project_dir: Some(project_dir),
        })
        .expect("load config")
    }

    fn load_config_with_store(
        config_path: PathBuf,
        project_dir: PathBuf,
        store: ProjectTrustStore,
    ) -> AppConfig {
        AppConfig::load(ConfigOverrides {
            config_path: Some(config_path),
            yolo: false,
            auto: false,
            trust_store: Some(store),
            project_dir: Some(project_dir),
        })
        .expect("load config")
    }

    fn config_with_theme(content: &str, theme: &str) -> String {
        format!("{content}\n[tui]\ntheme = {theme:?}\n")
    }

    #[path = "loader.rs"]
    mod loader;
    #[path = "paths.rs"]
    mod paths;
    #[path = "providers.rs"]
    mod providers;
    #[path = "runtime.rs"]
    mod runtime;
    #[path = "tui.rs"]
    mod tui;
    #[path = "workflow.rs"]
    mod workflow;
}
