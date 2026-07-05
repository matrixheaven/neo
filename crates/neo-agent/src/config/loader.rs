use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::{env, fs};

use anyhow::Context;
use neo_agent_core::BackgroundTaskManager;
use neo_agent_core::multi_agent::MultiAgentRuntime;
use neo_agent_core::{PermissionMode, QueueMode, ToolExecutionMode};
use neo_tui::input::{KeyId, KeybindingAction, KeybindingsManager};
use neo_tui::notify::NotificationMode;

use super::types::{
    FileConfig, FileRuntimeConfig, FileTuiConfig, default_runtime_compaction_keep_recent_messages,
    default_runtime_compaction_max_estimated_tokens,
};
use super::{
    AppConfig, ConfigOverrides, Defaults, ProviderConfig, RuntimeCompactionConfig, RuntimeConfig,
    TuiConfig, default_config_path, expand_user_path, neo_home,
};
use crate::{themes, trust};

const DEFAULT_MODEL: &str = "gpt-4.1";
const DEFAULT_PROVIDER: &str = "openai";
const DEFAULT_MODE: &str = "interactive";

impl AppConfig {
    #[allow(clippy::too_many_lines)]
    pub fn load(overrides: ConfigOverrides) -> anyhow::Result<Self> {
        // There is exactly one config file: `~/.neo/config.toml` (or wherever
        // `NEO_HOME` points). There is no project-local config anymore —
        // providers/models/settings/skills/prompts/themes all live under the
        // single neo home and are shared across every workspace.
        let config_path = overrides
            .config_path
            .or_else(default_config_path)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "could not resolve Neo home directory: set NEO_HOME, or HOME on Unix / USERPROFILE on Windows"
                )
            })?;
        // `project_dir` is the *workspace identity* (used for trust keying,
        // session bucketing, git status, `@file` sandboxing). It is NOT a config
        // location. Default to the current working directory.
        let project_dir = overrides.project_dir.map_or_else(env::current_dir, Ok)?;

        let file_config = read_file_config(&config_path)?;
        let config_file_exists = config_path.exists();
        let (project_trusted, project_trust) =
            resolve_project_trust_state(&project_dir, overrides.yolo, overrides.trust_store)?;
        anyhow::ensure!(
            !(overrides.yolo && overrides.auto),
            "--yolo and --auto cannot be used together"
        );

        let default_model = file_config
            .default_model
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        let default_provider = file_config
            .default_provider
            .unwrap_or_else(|| DEFAULT_PROVIDER.to_owned());
        let providers = file_config.providers.unwrap_or_default();
        let models = file_config.models.unwrap_or_default();
        let api_key_env = file_config
            .api_key_env
            .or_else(|| provider_api_key_env(&providers, &default_provider));
        let model_scope = file_config.model_scope.unwrap_or_default();
        let prompt_templates = file_config.prompt_templates.unwrap_or_default();
        let system_prompt_file = file_config.system_prompt_file.map(expand_user_path);
        let extra_skill_dirs = file_config.extra_skill_dirs.unwrap_or_default();
        let skill_path = file_config.skill_path;
        let sessions_dir = match file_config.sessions_dir {
            Some(path) => expand_user_path(path),
            None => neo_home()
                .map(|home| home.join("sessions"))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "could not resolve Neo home directory: sessions_dir defaults to ~/.neo/sessions, but NEO_HOME or the platform home directory (HOME on Unix, USERPROFILE on Windows) is not set"
                    )
                })?,
        };
        let permission_mode = if overrides.yolo {
            PermissionMode::Yolo
        } else if overrides.auto {
            PermissionMode::Auto
        } else {
            file_config.permission_mode.unwrap_or_default()
        };
        let runtime = runtime_from_file(file_config.runtime);
        validate_runtime_config(&runtime)?;
        let tui = tui_from_file(file_config.tui);
        validate_tui_config(&tui)?;
        let theme = themes::resolve_theme()?;
        let mcp = file_config.mcp.unwrap_or_default();
        let mode = file_config
            .defaults
            .and_then(|defaults| defaults.mode)
            .unwrap_or_else(|| DEFAULT_MODE.to_owned());

        Ok(Self {
            default_model,
            default_provider,
            api_key_env,
            providers,
            models,
            model_scope,
            sessions_dir,
            permission_mode,
            live_permission_mode: Arc::new(RwLock::new(permission_mode)),
            defaults: Defaults { mode },
            runtime,
            background_tasks: BackgroundTaskManager::new(),
            multi_agent: MultiAgentRuntime::new(),
            tui,
            theme,
            mcp,
            prompt_templates,
            system_prompt_file,
            extra_skill_dirs,
            skill_path,
            project_trusted,
            project_trust,
            project_dir,
            config_file_exists,
            config_path,
        })
    }
}

fn resolve_project_trust_state(
    project_dir: &Path,
    yolo: bool,
    trust_store: Option<trust::ProjectTrustStore>,
) -> anyhow::Result<(bool, trust::ProjectTrustState)> {
    let project_dir = project_dir.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize project dir {}",
            project_dir.display()
        )
    })?;

    if yolo {
        return Ok((false, trust::ProjectTrustState::NotRequired));
    }

    let inputs = trust::collect_project_trust_inputs(&project_dir)?;
    if inputs.detected.is_empty() && inputs.parent_candidates.is_empty() {
        return Ok((true, trust::ProjectTrustState::NotRequired));
    }

    let store = trust_store.map_or_else(trust::ProjectTrustStore::from_home, Ok)?;
    match trust::resolve_project_trust_decision(&project_dir, false, &store)? {
        trust::ProjectTrustDecision::Trusted { source } => Ok((
            true,
            trust::ProjectTrustState::Trusted {
                target: source.target(&project_dir),
            },
        )),
        trust::ProjectTrustDecision::Untrusted { source } => Ok((
            false,
            trust::ProjectTrustState::Untrusted {
                target: source.target(&project_dir),
            },
        )),
        trust::ProjectTrustDecision::Unknown { inputs } => {
            Ok((false, trust::ProjectTrustState::Unknown { inputs }))
        }
    }
}

fn provider_api_key_env(
    providers: &BTreeMap<String, ProviderConfig>,
    provider_id: &str,
) -> Option<String> {
    providers
        .get(provider_id)
        .and_then(|provider| provider.api_key_env.clone())
}

fn runtime_from_file(runtime: Option<FileRuntimeConfig>) -> RuntimeConfig {
    let Some(runtime) = runtime else {
        return RuntimeConfig::default();
    };
    RuntimeConfig {
        temperature: runtime.temperature,
        max_tokens: runtime.max_tokens,
        reasoning_effort: runtime.reasoning_effort,
        replay_reasoning: runtime.replay_reasoning.unwrap_or(true),
        steering_queue_mode: runtime.steering_queue_mode.unwrap_or(QueueMode::All),
        follow_up_queue_mode: runtime.follow_up_queue_mode.unwrap_or(QueueMode::All),
        tool_execution_mode: runtime
            .tool_execution_mode
            .unwrap_or(ToolExecutionMode::Parallel),
        compaction: runtime
            .compaction
            .map(|compaction| RuntimeCompactionConfig {
                enabled: compaction.enabled.unwrap_or(true),
                max_estimated_tokens: compaction
                    .max_estimated_tokens
                    .unwrap_or_else(default_runtime_compaction_max_estimated_tokens),
                keep_recent_messages: compaction
                    .keep_recent_messages
                    .unwrap_or_else(default_runtime_compaction_keep_recent_messages),
                trigger_ratio: compaction.trigger_ratio.unwrap_or(0.85),
                reserved_context_tokens: compaction.reserved_context_tokens.unwrap_or(50_000),
                max_recent_messages: compaction.max_recent_messages.unwrap_or(4),
                micro_enabled: compaction.micro_enabled.unwrap_or(true),
                micro_keep_recent: compaction.micro_keep_recent.unwrap_or(20),
                max_rounds: compaction.max_rounds.unwrap_or(5),
                max_retry_attempts: compaction.max_retry_attempts.unwrap_or(5),
            }),
    }
}

fn tui_from_file(tui: Option<FileTuiConfig>) -> TuiConfig {
    let Some(tui) = tui else {
        return TuiConfig::default();
    };
    TuiConfig {
        image_protocol: tui.image_protocol.unwrap_or_default(),
        fetch_remote_images: tui.fetch_remote_images.unwrap_or(false),
        keybindings: tui.keybindings.unwrap_or_default(),
        completion_notification: tui.completion_notification.unwrap_or_default(),
        question_notification: tui.question_notification.unwrap_or(NotificationMode::None),
    }
}

fn validate_runtime_config(config: &RuntimeConfig) -> anyhow::Result<()> {
    if let Some(temperature) = config.temperature {
        anyhow::ensure!(
            temperature.is_finite() && temperature >= 0.0,
            "runtime.temperature must be a finite non-negative number"
        );
    }
    if let Some(max_tokens) = config.max_tokens {
        anyhow::ensure!(max_tokens > 0, "runtime.max_tokens must be greater than 0");
    }
    if let Some(compaction) = &config.compaction
        && compaction.enabled
    {
        anyhow::ensure!(
            compaction.max_estimated_tokens > 0,
            "runtime.compaction.max_estimated_tokens must be greater than 0"
        );
        anyhow::ensure!(
            compaction.keep_recent_messages > 0,
            "runtime.compaction.keep_recent_messages must be greater than 0"
        );
    }
    Ok(())
}

fn validate_tui_config(config: &TuiConfig) -> anyhow::Result<()> {
    let default_manager = KeybindingsManager::default();
    let mut manager = KeybindingsManager::default();
    let overrides = config.keybinding_overrides()?;
    for (_action, keys) in &overrides {
        for key in keys {
            anyhow::ensure!(
                !key.is_text_insertion_key(),
                "tui.keybindings key {key} is reserved for prompt text insertion"
            );
        }
    }
    manager.set_user_bindings(overrides.iter().cloned());
    anyhow::ensure!(
        manager.conflicts().is_empty(),
        "tui.keybindings contains conflicting key assignments"
    );
    validate_tui_context_conflicts(&default_manager, &manager, &overrides)?;
    Ok(())
}

fn validate_tui_context_conflicts(
    default_manager: &KeybindingsManager,
    manager: &KeybindingsManager,
    overrides: &[(KeybindingAction, Vec<KeyId>)],
) -> anyhow::Result<()> {
    for (action, keys) in overrides {
        for context in [TUI_EDITING_ACTIONS, TUI_OVERLAY_ACTIONS] {
            if !context.contains(action) {
                continue;
            }
            for key in keys {
                let current_actions = context_actions_for_key(manager, context, key);
                if current_actions.len() <= 1 {
                    continue;
                }
                let default_actions = context_actions_for_key(default_manager, context, key);
                if current_actions != default_actions {
                    let action_ids = current_actions
                        .iter()
                        .map(|action| action.id())
                        .collect::<Vec<_>>()
                        .join(", ");
                    anyhow::bail!(
                        "tui.keybindings key {key} conflicts within a TUI input context: {action_ids}"
                    );
                }
            }
        }
    }
    Ok(())
}

fn context_actions_for_key(
    manager: &KeybindingsManager,
    context: &[KeybindingAction],
    key: &KeyId,
) -> BTreeSet<KeybindingAction> {
    context
        .iter()
        .filter(|action| {
            manager
                .keys(**action)
                .iter()
                .any(|candidate| candidate == key)
        })
        .copied()
        .collect()
}

const TUI_EDITING_ACTIONS: &[KeybindingAction] = &[
    KeybindingAction::InputSubmit,
    KeybindingAction::InputNewLine,
    KeybindingAction::TranscriptCopySelection,
    KeybindingAction::AppClear,
    KeybindingAction::AppExit,
    KeybindingAction::AppSuspend,
    KeybindingAction::InputCopy,
    KeybindingAction::TranscriptSelectionStart,
    KeybindingAction::TranscriptSelectionClear,
    KeybindingAction::TranscriptSelectionExtendUp,
    KeybindingAction::TranscriptSelectionExtendDown,
    KeybindingAction::TranscriptSelectionExtendPageUp,
    KeybindingAction::TranscriptSelectionExtendPageDown,
    KeybindingAction::PromptCompletionToggle,
    KeybindingAction::CommandPaletteOpen,
    KeybindingAction::SessionPickerOpen,
    KeybindingAction::SessionFork,
    KeybindingAction::ModelPickerOpen,
    KeybindingAction::EditorCursorLeft,
    KeybindingAction::EditorCursorRight,
    KeybindingAction::EditorCursorWordLeft,
    KeybindingAction::EditorCursorWordRight,
    KeybindingAction::EditorCursorLineStart,
    KeybindingAction::EditorCursorLineEnd,
    KeybindingAction::EditorDeleteCharBackward,
    KeybindingAction::EditorDeleteCharForward,
    KeybindingAction::EditorDeleteWordBackward,
    KeybindingAction::EditorDeleteWordForward,
    KeybindingAction::EditorDeleteToLineStart,
    KeybindingAction::EditorDeleteToLineEnd,
    KeybindingAction::EditorYank,
    KeybindingAction::EditorUndo,
    KeybindingAction::InputTab,
    KeybindingAction::SelectCancel,
];

const TUI_OVERLAY_ACTIONS: &[KeybindingAction] = &[
    KeybindingAction::SelectConfirm,
    KeybindingAction::SelectCancel,
    KeybindingAction::SessionFork,
    KeybindingAction::SessionPickerToggleScope,
    KeybindingAction::SelectUp,
    KeybindingAction::SelectDown,
    KeybindingAction::SelectPageUp,
    KeybindingAction::SelectPageDown,
];

impl TuiConfig {
    pub fn keybinding_overrides(&self) -> anyhow::Result<Vec<(KeybindingAction, Vec<KeyId>)>> {
        self.keybindings
            .iter()
            .map(|(action_id, keys)| {
                let action = KeybindingAction::from_id(action_id)
                    .with_context(|| format!("unsupported TUI keybinding action: {action_id}"))?;
                let keys = keys
                    .iter()
                    .map(|key| KeyId::new(key).map_err(|err| anyhow::anyhow!(err.to_string())))
                    .collect::<anyhow::Result<Vec<_>>>()?;
                Ok((action, keys))
            })
            .collect()
    }
}

pub(crate) fn read_file_config(path: &Path) -> anyhow::Result<FileConfig> {
    if !path.exists() {
        return Ok(FileConfig::default());
    }

    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    toml::from_str(&content).with_context(|| format!("failed to parse config {}", path.display()))
}

pub(crate) fn write_file_config(path: &Path, config: &FileConfig) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let content = toml::to_string_pretty(config)?;
    fs::write(path, content).with_context(|| format!("failed to write config {}", path.display()))
}
