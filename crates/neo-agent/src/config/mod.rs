use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, RwLock},
};

use neo_agent_core::BackgroundTaskManager;
use neo_agent_core::multi_agent::MultiAgentRuntime;
use neo_agent_core::{PermissionMode, QueueMode, ToolExecutionMode};
use neo_ai::ReasoningEffort;
use neo_tui::notify::NotificationMode;
use neo_tui::terminal_image::ImageProtocolPreference;
use serde::{Deserialize, Serialize};

use crate::{cli::Cli, themes::ResolvedTheme, trust};

mod loader;
mod matching;
pub(crate) mod mutations;
mod paths;
mod types;

pub(crate) use matching::scoped_models;
#[allow(unused_imports)]
pub(crate) use paths::{
    default_config_path, expand_user_path, expand_user_path_with_home, global_prompts_dir,
    neo_home, user_home, workspace_sessions_dir,
};

// Re-export config types for callers that access them via `crate::config::*`.
pub(crate) use loader::{read_file_config, write_file_config};
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
    pub api_key_env: Option<String>,
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
    pub defaults: Defaults,
    pub runtime: RuntimeConfig,
    /// Shared background task registry for the interactive session.
    ///
    /// Model-initiated Bash background jobs, background `AskUser` questions, and
    /// user shell-mode Ctrl+B detach all need to land in the same task list so
    /// `TaskList`/`TaskOutput` can observe them on later turns.
    #[serde(skip)]
    pub background_tasks: BackgroundTaskManager,
    /// Shared multi-agent runtime for Delegate/DelegateSwarm tasks in this app session.
    #[serde(skip)]
    pub multi_agent: MultiAgentRuntime,
    pub tui: TuiConfig,
    #[serde(skip)]
    pub theme: ResolvedTheme,
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
    pub reasoning_effort: Option<ReasoningEffort>,
    pub replay_reasoning: bool,
    pub steering_queue_mode: QueueMode,
    pub follow_up_queue_mode: QueueMode,
    pub tool_execution_mode: ToolExecutionMode,
    pub compaction: Option<RuntimeCompactionConfig>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            temperature: None,
            max_tokens: None,
            reasoning_effort: None,
            replay_reasoning: true,
            steering_queue_mode: QueueMode::All,
            follow_up_queue_mode: QueueMode::All,
            tool_execution_mode: ToolExecutionMode::Parallel,
            compaction: None,
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
    pub max_rounds: usize,
    pub max_retry_attempts: u32,
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
            micro_enabled: true,
            micro_keep_recent: 20,
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
    pub fetch_remote_images: bool,
    #[serde(default)]
    pub keybindings: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub completion_notification: NotificationMode,
    #[serde(default)]
    pub question_notification: NotificationMode,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            image_protocol: ImageProtocolPreference::default(),
            fetch_remote_images: false,
            keybindings: BTreeMap::new(),
            completion_notification: NotificationMode::Bell,
            question_notification: NotificationMode::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use neo_agent_core::QueueMode;
    use neo_ai::{ApiKind, ModelCapabilities, ModelSpec, ProviderId};
    use tempfile::TempDir;

    use crate::config::{AppConfig, ConfigOverrides, PermissionMode, TuiConfig};
    use crate::trust::{ProjectTrustState, ProjectTrustStore};

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

    #[test]
    fn config_defaults_to_ask_permission_mode() {
        let (_temp, config_path, project_dir) = temp_project_config("");
        let config = load_config(config_path, project_dir);
        assert_eq!(config.permission_mode, PermissionMode::Ask);
    }

    #[test]
    fn no_config_file_shows_unconfigured_label() {
        let temp = TempDir::new().expect("temp dir");
        let config_path = temp.path().join("config.toml");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project");
        let config = AppConfig::load(ConfigOverrides {
            config_path: Some(config_path),
            yolo: false,
            auto: false,
            trust_store: None,
            project_dir: Some(project_dir),
        })
        .expect("load config without file");
        assert!(!config.config_file_exists);
        assert_eq!(
            config.default_model_label(),
            "No configured providers/models"
        );
    }

    #[test]
    fn config_defaults_follow_up_queue_to_all() {
        let (_temp, config_path, project_dir) = temp_project_config("");
        let config = load_config(config_path, project_dir);
        assert_eq!(config.runtime.follow_up_queue_mode, QueueMode::All);
    }

    #[test]
    fn config_loads_system_prompt_file_with_tilde_expansion() {
        let (_temp, config_path, project_dir) =
            temp_project_config("system_prompt_file = \"~/neo-system.md\"\n");
        let home = std::env::var_os("HOME").map(PathBuf::from).expect("home");

        let config = load_config(config_path, project_dir);

        assert_eq!(config.system_prompt_file, Some(home.join("neo-system.md")));
    }

    /// Regression: the model display label must never stitch the provider onto
    /// an already-qualified alias. When `default_model` is a `<provider>/<model>`
    /// alias, `default_model_label()` returns it as-is; otherwise it prefixes
    /// `default_provider/`. This avoids both
    /// `deepseek/minimax-.../MiniMax-M2` (stale provider) and
    /// `minimax-.../minimax-.../MiniMax-M2` (double prefix).
    #[test]
    fn default_model_label_never_double_prefixes() {
        let (_temp, config_path, project_dir) = temp_project_config(
            r#"
default_model = "minimax-cn-coding-plan/MiniMax-M2"
default_provider = "deepseek"

[providers.deepseek]
type = "openai"
base_url = "https://deepseek.example/v1"

[providers."minimax-cn-coding-plan"]
type = "anthropic"
base_url = "https://api.minimaxi.com/anthropic/v1"

[models."minimax-cn-coding-plan/MiniMax-M2"]
provider = "minimax-cn-coding-plan"
model = "MiniMax-M2"
"#,
        );
        let config = load_config(config_path, project_dir);
        // Alias is used as-is: no stale deepseek prefix, no double minimax prefix.
        assert_eq!(
            config.default_model_label(),
            "minimax-cn-coding-plan/MiniMax-M2"
        );
    }

    /// When `default_model` has no `/` (a plain model id), the label prefixes
    /// `default_provider/`.
    #[test]
    fn default_model_label_prefixes_provider_for_plain_model_id() {
        let (_temp, config_path, project_dir) = temp_project_config(
            r#"
default_model = "deepseek-v4-pro"
default_provider = "deepseek"

[providers.deepseek]
type = "openai"
base_url = "https://deepseek.example/v1"
"#,
        );
        let config = load_config(config_path, project_dir);
        assert_eq!(config.default_model_label(), "deepseek/deepseek-v4-pro");
    }

    #[test]
    fn default_model_label_resolves_unqualified_alias() {
        let (_temp, config_path, project_dir) = temp_project_config(
            r#"
default_model = "fast"
default_provider = "openai"

[providers.openai]
type = "openai_response"

[models.fast]
provider = "openai"
model = "gpt-4.1"
"#,
        );
        let config = load_config(config_path, project_dir);
        assert_eq!(config.default_model_label(), "openai/gpt-4.1");
    }

    #[test]
    fn tilde_expansion_uses_user_home_semantics() {
        let home = PathBuf::from("/home/alice");

        assert_eq!(
            super::expand_user_path_with_home(PathBuf::from("~/neo-sessions"), Some(&home)),
            PathBuf::from("/home/alice/neo-sessions")
        );
        assert_eq!(
            super::expand_user_path_with_home(PathBuf::from("relative/path"), Some(&home)),
            PathBuf::from("relative/path")
        );
    }

    #[test]
    fn config_loads_permission_mode_auto() {
        let (_temp, config_path, project_dir) = temp_project_config("permission_mode = \"auto\"\n");
        let config = load_config(config_path, project_dir);
        assert_eq!(config.permission_mode, PermissionMode::Auto);
    }

    #[test]
    fn cli_yolo_overrides_config_permission_mode() {
        let (_temp, config_path, project_dir) = temp_project_config("permission_mode = \"ask\"\n");
        let config = AppConfig::load(ConfigOverrides {
            config_path: Some(config_path),
            yolo: true,
            auto: false,
            trust_store: None,
            project_dir: Some(project_dir),
        })
        .expect("load config");
        assert_eq!(config.permission_mode, PermissionMode::Yolo);
    }

    #[test]
    fn cli_auto_overrides_config_permission_mode() {
        let (_temp, config_path, project_dir) = temp_project_config("permission_mode = \"ask\"\n");
        let config = AppConfig::load(ConfigOverrides {
            config_path: Some(config_path),
            yolo: false,
            auto: true,
            trust_store: None,
            project_dir: Some(project_dir),
        })
        .expect("load config");
        assert_eq!(config.permission_mode, PermissionMode::Auto);
    }

    #[test]
    fn scoped_models_matches_globs_against_qualified_and_model_ids() {
        let openai = ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: "gpt-4.1".to_owned(),
            api: ApiKind::OpenAiResponse,
            capabilities: ModelCapabilities::tool_chat(),
        };
        let anthropic = ModelSpec {
            provider: ProviderId("anthropic".to_owned()),
            model: "claude-sonnet-4".to_owned(),
            api: ApiKind::AnthropicMessages,
            capabilities: ModelCapabilities::tool_chat(),
        };

        let models = [openai, anthropic];
        let scoped = super::scoped_models(
            models.iter(),
            &["openai/gpt-*".to_owned(), "claude-??????-4:high".to_owned()],
        );

        assert_eq!(
            scoped
                .iter()
                .map(|model| format!("{}/{}", model.provider.0, model.model))
                .collect::<Vec<_>>(),
            vec!["openai/gpt-4.1", "anthropic/claude-sonnet-4"]
        );
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

    #[test]
    fn config_trust_is_not_required_for_directory_without_inputs() {
        let (_temp, config_path, project_dir) = temp_project_config("");
        let config = load_config(config_path, project_dir.clone());
        assert!(config.project_trusted);
        assert_eq!(config.project_trust, ProjectTrustState::NotRequired);
    }

    #[test]
    fn config_trust_is_unknown_when_inputs_exist_without_decision() {
        let (temp, config_path, project_dir) = temp_project_config("");
        fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");
        let store = ProjectTrustStore::new(temp.path().join("trust.json"));

        let config = load_config_with_store(config_path, project_dir.clone(), store);

        assert!(!config.project_trusted);
        assert!(matches!(
            config.project_trust,
            ProjectTrustState::Unknown { .. }
        ));
    }

    #[test]
    fn config_trust_is_trusted_when_store_approves_current_dir() {
        let (temp, config_path, project_dir) = temp_project_config("");
        fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");
        let store = ProjectTrustStore::new(temp.path().join("trust.json"));
        store.set(&project_dir, Some(true)).expect("approve");

        let config = load_config_with_store(config_path, project_dir.clone(), store);

        assert!(config.project_trusted);
        assert_eq!(
            config.project_trust,
            ProjectTrustState::Trusted {
                target: project_dir.canonicalize().expect("canonicalize"),
            }
        );
    }

    #[test]
    fn config_trust_is_untrusted_when_store_denies_current_dir() {
        let (temp, config_path, project_dir) = temp_project_config("");
        fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");
        let store = ProjectTrustStore::new(temp.path().join("trust.json"));
        store.set(&project_dir, Some(false)).expect("deny");

        let config = load_config_with_store(config_path, project_dir.clone(), store);

        assert!(!config.project_trusted);
        assert_eq!(
            config.project_trust,
            ProjectTrustState::Untrusted {
                target: project_dir.canonicalize().expect("canonicalize"),
            }
        );
    }

    #[test]
    fn tui_config_parses_notification_fields() {
        use neo_tui::notify::NotificationMode;

        let toml = r#"
            completion_notification = "all"
            question_notification = "bell"
        "#;
        let tui: TuiConfig = toml::from_str(toml).unwrap();
        assert_eq!(tui.completion_notification, NotificationMode::All);
        assert_eq!(tui.question_notification, NotificationMode::Bell);
    }

    #[test]
    fn tui_config_defaults_notification_fields() {
        use neo_tui::notify::NotificationMode;

        let tui = TuiConfig::default();
        assert_eq!(tui.completion_notification, NotificationMode::Bell);
        assert_eq!(tui.question_notification, NotificationMode::None);
    }

    #[test]
    fn config_yolo_sets_not_required_and_untrusted() {
        let (_temp, config_path, project_dir) = temp_project_config("");
        fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");

        let config = AppConfig::load(ConfigOverrides {
            config_path: Some(config_path),
            yolo: true,
            auto: false,
            trust_store: None,
            project_dir: Some(project_dir),
        })
        .expect("load config");

        assert!(!config.project_trusted);
        assert_eq!(config.project_trust, ProjectTrustState::NotRequired);
    }

    #[test]
    fn neo_home_prefers_neo_home_env() {
        temp_env::with_var("NEO_HOME", Some("/custom/neo"), || {
            assert_eq!(super::neo_home(), Some(PathBuf::from("/custom/neo")));
        });
    }

    #[test]
    #[cfg(not(windows))]
    fn neo_home_uses_home_on_unix() {
        temp_env::with_vars(
            [("NEO_HOME", None::<&str>), ("HOME", Some("/home/alice"))],
            || {
                assert_eq!(super::neo_home(), Some(PathBuf::from("/home/alice/.neo")));
                assert_eq!(super::user_home(), Some(PathBuf::from("/home/alice")));
            },
        );
    }

    #[test]
    #[cfg(windows)]
    fn neo_home_uses_userprofile_on_windows() {
        temp_env::with_vars(
            [
                ("NEO_HOME", None::<&str>),
                ("USERPROFILE", Some(r"C:\Users\Alice")),
                ("HOME", None::<&str>),
            ],
            || {
                assert_eq!(
                    super::neo_home(),
                    Some(PathBuf::from(r"C:\Users\Alice\.neo"))
                );
                assert_eq!(super::user_home(), Some(PathBuf::from(r"C:\Users\Alice")));
            },
        );
    }

    #[test]
    fn default_config_path_is_none_when_home_unresolvable() {
        temp_env::with_vars(
            [
                ("NEO_HOME", None::<&str>),
                ("HOME", None::<&str>),
                ("USERPROFILE", None::<&str>),
            ],
            || {
                assert!(super::default_config_path().is_none());
            },
        );
    }

    #[test]
    fn default_config_path_uses_neo_home() {
        temp_env::with_var("NEO_HOME", Some("/custom/neo"), || {
            assert_eq!(
                super::default_config_path(),
                Some(PathBuf::from("/custom/neo/config.toml"))
            );
        });
    }
}
