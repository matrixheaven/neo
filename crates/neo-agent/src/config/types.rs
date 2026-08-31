use std::collections::BTreeMap;
use std::path::PathBuf;

use neo_agent_core::{PermissionMode, QueueMode, ToolExecutionMode};
use neo_ai::ReasoningEffort;
use neo_tui::notify::NotificationMode;
use neo_tui::terminal_image::ImageProtocolPreference;
use serde::{Deserialize, Serialize};

pub(crate) fn deserialize_string_or_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{Deserialize, SeqAccess, Visitor};
    struct StringOrVec;

    impl<'de> Visitor<'de> for StringOrVec {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or a list of strings")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(vec![value.to_owned()])
        }

        fn visit_seq<A>(self, seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            Vec::<String>::deserialize(serde::de::value::SeqAccessDeserializer::new(seq))
        }
    }

    deserializer.deserialize_any(StringOrVec)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Human-readable display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<neo_ai::ApiType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Inline API key stored directly in config.toml.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Environment variable name that holds the API key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
}

/// A model definition in `config.toml` `[models.<alias>]`.
///
/// Each model references a provider by id and specifies the actual model ID
/// sent to the API, context window, and capabilities.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Provider id — must match a key in `[providers.<id>]`.
    pub provider: String,
    /// Actual model ID sent to the provider API.
    pub model: String,
    /// Maximum context window in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_tokens: Option<u32>,
    /// Maximum output tokens (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// Capability tags: `"streaming"`, `"tools"`, `"images"`, `"reasoning"`.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Typed reasoning capability; capability tags are only display/fallback input.
    #[serde(default)]
    pub reasoning: neo_ai::ReasoningCapability,
    /// Human-readable display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

/// Transport mechanism for an MCP server connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    Stdio,
    Http,
    Sse,
}

impl McpTransport {
    /// Returns the string representation used in config files.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
            Self::Sse => "sse",
        }
    }
}

impl std::fmt::Display for McpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub id: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub transport: McpTransport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enabled_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_timeout_ms: Option<u64>,
}

const fn default_enabled() -> bool {
    true
}

pub(super) const fn default_runtime_compaction_max_estimated_tokens() -> usize {
    32_000
}

pub(super) const fn default_runtime_compaction_keep_recent_messages() -> usize {
    20
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct FileConfig {
    pub(crate) default_model: Option<String>,
    pub(crate) default_provider: Option<String>,
    pub(crate) providers: Option<BTreeMap<String, ProviderConfig>>,
    /// Models defined inline via `[models.<alias>]` tables.
    pub(crate) models: Option<BTreeMap<String, ModelConfig>>,
    pub(crate) model_scope: Option<Vec<String>>,
    pub(crate) prompt_templates: Option<Vec<String>>,
    pub(crate) system_prompt_file: Option<PathBuf>,
    pub(crate) extra_skill_dirs: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_string_or_vec")]
    pub(crate) skill_path: Vec<String>,
    pub(crate) sessions_dir: Option<PathBuf>,
    pub(crate) permission_mode: Option<PermissionMode>,
    pub(crate) defaults: Option<FileDefaults>,
    pub(crate) runtime: Option<FileRuntimeConfig>,
    pub(crate) tui: Option<FileTuiConfig>,
    pub(crate) webui: Option<FileWebUiConfig>,
    pub(crate) mcp: Option<McpConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct FileDefaults {
    pub(crate) mode: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct FileRuntimeConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reasoning: Option<neo_ai::ReasoningSelection>,
    /// Auxiliary-call reasoning intensity (`"auto"`, `"off"`, or a pinned
    /// effort value); `None` behaves as `auto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) aux_reasoning: Option<neo_ai::AuxReasoning>,
    // Legacy config migration input only; runtime uses `reasoning`.
    #[serde(default, skip_serializing)]
    pub(crate) reasoning_effort: Option<ReasoningEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) replay_reasoning: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) steering_queue_mode: Option<QueueMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) follow_up_queue_mode: Option<QueueMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tool_execution_mode: Option<ToolExecutionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) retry: Option<FileRuntimeRetryConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) compaction: Option<FileRuntimeCompactionConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) shell: Option<FileRuntimeShellConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) workflow: Option<FileRuntimeWorkflowConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileRuntimeWorkflowConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) lua_source_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) manifest_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) lua_vm_memory_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pause_hook_interval: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_uninterrupted_instructions: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) journal_record_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) journal_total_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) artifact_record_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) artifact_total_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) global_storage_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pending_record_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) task_output_page_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_active_vms: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_active_workers: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_active_executors: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) swarm_concurrency: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct FileRuntimeRetryConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_retries: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) first_event_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) stream_idle_timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileRuntimeShellConfig {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "max_active_commands"
    )]
    pub(crate) active_commands: Option<usize>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "max_command_parallelism"
    )]
    pub(crate) command_parallelism: Option<usize>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "max_command_descendant_processes"
    )]
    pub(crate) command_descendant_processes: Option<usize>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "max_command_memory_percent"
    )]
    pub(crate) command_memory_percent: Option<u8>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "max_output_bytes"
    )]
    pub(crate) output_bytes: Option<usize>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "max_background_log_bytes"
    )]
    pub(crate) background_log_bytes: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct FileRuntimeCompactionConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_estimated_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) keep_recent_messages: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) trigger_ratio: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reserved_context_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_recent_messages: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_rounds: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_retry_attempts: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct FileTuiConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) image_protocol: Option<ImageProtocolPreference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) keybindings: Option<BTreeMap<String, Vec<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) completion_notification: Option<NotificationMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) question_notification: Option<NotificationMode>,
    /// Explicit startup theme: a logical theme id (relative path under
    /// `$NEO_HOME/themes/`), never an absolute path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) theme: Option<String>,
}

/// WebUI display preferences persisted under the `[webui]` table. All fields
/// optional so an unset section falls back to WebUI defaults. WebUI is a
/// browser-rendered surface; these values drive its CSS variables and code
/// rendering (separate from the TUI `[tui].theme` file id).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileWebUiConfig {
    /// Interface theme: `"system"` (follow OS), `"light"`, or `"dark"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) theme: Option<String>,
    /// Base UI font size in px.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ui_font_size: Option<u32>,
    /// Code block font size in px.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) code_font_size: Option<u32>,
    /// Code token palette: `"auto"` (follow interface theme), `"light"`, or `"dark"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) code_theme: Option<String>,
    /// Show line numbers in code blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) show_line_numbers: Option<bool>,
    /// Wrap long lines in code blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) word_wrap: Option<bool>,
}

/// Resolved WebUI appearance with defaults applied. Used as the settings
/// write model and mirrored to the web protocol snapshot/`bootstrap`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiAppearance {
    pub theme: String,
    pub ui_font_size: u32,
    pub code_font_size: u32,
    pub code_theme: String,
    pub show_line_numbers: bool,
    pub word_wrap: bool,
}

impl Default for WebUiAppearance {
    fn default() -> Self {
        Self {
            theme: "system".to_owned(),
            ui_font_size: 14,
            code_font_size: 12,
            code_theme: "auto".to_owned(),
            show_line_numbers: true,
            word_wrap: true,
        }
    }
}

impl WebUiAppearance {
    /// Merge a partial `[webui]` table over the defaults.
    #[must_use]
    pub fn from_file(file: Option<&FileWebUiConfig>) -> Self {
        let defaults = Self::default();
        let Some(file) = file else {
            return defaults;
        };
        Self {
            theme: file.theme.clone().unwrap_or(defaults.theme),
            ui_font_size: file.ui_font_size.unwrap_or(defaults.ui_font_size),
            code_font_size: file.code_font_size.unwrap_or(defaults.code_font_size),
            code_theme: file.code_theme.clone().unwrap_or(defaults.code_theme),
            show_line_numbers: file.show_line_numbers.unwrap_or(defaults.show_line_numbers),
            word_wrap: file.word_wrap.unwrap_or(defaults.word_wrap),
        }
    }
}
