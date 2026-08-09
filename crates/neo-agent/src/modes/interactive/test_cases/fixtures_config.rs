//! Interactive test fixtures: `AppConfig` builders and model stubs (moved from `mod.rs`).

use crate::config::{Defaults, McpConfig, ModelConfig, RuntimeConfig, TuiConfig};

use super::super::*;
use super::fixtures_sessions::*;

pub fn test_config(project_dir: &Path, sessions_dir: PathBuf) -> AppConfig {
    AppConfig {
        default_model: "gpt-4.1".to_owned(),
        default_provider: "openai".to_owned(),
        providers: BTreeMap::new(),
        models: BTreeMap::new(),
        model_scope: Vec::new(),
        sessions_dir,
        permission_mode: PermissionMode::default(),
        live_permission_mode: Arc::new(RwLock::new(PermissionMode::default())),
        workspace_policy: Arc::new(RwLock::new(None)),
        defaults: Defaults {
            mode: "interactive".to_owned(),
        },
        runtime: RuntimeConfig::default(),
        background_tasks: neo_agent_core::BackgroundTaskManager::new(),
        workflow_runtime: neo_agent_core::workflow::WorkflowRuntime::new(
            neo_agent_core::workflow::WorkflowLimits::default(),
        ),
        workflow_definitions: neo_agent_core::workflow::WorkflowDefinitionRegistry::empty(),
        workflow_dispatch_resolver: neo_agent_core::runtime::WorkflowDispatchResolver::default(),
        multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
        tui: TuiConfig::default(),
        theme: crate::themes::ResolvedTheme::default(),
        theme_resolution: crate::themes::ThemeResolution::Default,
        mcp: McpConfig::default(),
        prompt_templates: Vec::new(),
        system_prompt_file: None,
        extra_skill_dirs: Vec::new(),
        skill_path: Vec::new(),
        project_trusted: true,
        project_trust: crate::trust::ProjectTrustState::NotRequired,
        project_dir: project_dir.to_path_buf(),
        config_path: project_dir.join(".neo/config.toml"),
        config_file_exists: true,
    }
}

pub fn test_config_with_models(
    project_dir: &Path,
    sessions_dir: PathBuf,
    models: BTreeMap<String, ModelConfig>,
) -> AppConfig {
    let mut config = test_config(project_dir, sessions_dir);
    config.models = models;
    config
}

pub fn selected_model_local_config() -> AppConfig {
    test_config_with_models(
        &test_workspace_root(),
        test_workspace_root().join(".neo/sessions"),
        BTreeMap::from([
            (
                "openai/gpt-4.1".to_owned(),
                ModelConfig {
                    provider: "openai".to_owned(),
                    model: "gpt-4.1".to_owned(),
                    display_name: Some("Responses".into()),
                    ..ModelConfig::default()
                },
            ),
            (
                "anthropic/claude-sonnet-4-5".to_owned(),
                ModelConfig {
                    provider: "anthropic".to_owned(),
                    model: "claude-sonnet-4-5".to_owned(),
                    display_name: Some("Messages · ctx 200000".into()),
                    max_context_tokens: Some(200_000),
                    ..ModelConfig::default()
                },
            ),
        ]),
    )
}

pub fn demo_named_workflow_config(temp: &tempfile::TempDir, mode: PermissionMode) -> AppConfig {
    use neo_agent_core::workflow::{
        BuiltinWorkflowDefinition, WorkflowDefinitionRegistry, WorkflowDefinitionRegistryConfig,
        WorkflowLimits, source_sha256_hex,
    };

    let project_dir = temp.path().join("workspace");
    let neo_home = temp.path().join("neo_home");
    std::fs::create_dir_all(&project_dir).expect("workspace");
    std::fs::create_dir_all(&neo_home).expect("neo home");

    let script = "return { ok = true }\n";
    let source_sha = source_sha256_hex(script.as_bytes());
    let manifest = format!(
        r#"
name = "demo"
display_name = "Demo"
description = "named slash fixture"
source_sha256 = "{source_sha}"

[[phases]]
id = "run"
description = "execute"

[output_schema]
type = "object"
additionalProperties = false
required = ["ok"]

[output_schema.properties.ok]
type = "boolean"

[input_schema]
type = "object"
required = ["topic"]

[input_schema.properties.topic]
type = "string"
"#
    );
    let mut config = test_config(&project_dir, temp.path().join("sessions"));
    config.permission_mode = mode;
    config.live_permission_mode = std::sync::Arc::new(std::sync::RwLock::new(mode));
    config.workflow_definitions =
        WorkflowDefinitionRegistry::new(WorkflowDefinitionRegistryConfig {
            neo_home,
            workspace: project_dir.clone(),
            project_trusted: true,
            limits: WorkflowLimits::default(),
            builtins: vec![BuiltinWorkflowDefinition {
                name: "demo".to_owned(),
                manifest_bytes: manifest.into_bytes(),
                source_bytes: script.as_bytes().to_vec(),
            }],
        });
    config
        .workflow_runtime
        .bind_runner(|_handle, _metadata, _session_dir| async move { Ok(()) })
        .expect("bind runner");
    config
}

pub fn btw_test_config(project_dir: &std::path::Path) -> crate::config::AppConfig {
    test_config(project_dir, project_dir.join(".neo/sessions"))
}

pub fn btw_fake_client(answer: &str) -> Arc<dyn neo_ai::ModelClient> {
    use neo_ai::{AiStreamEvent, StopReason};
    Arc::new(neo_ai::providers::fake::FakeModelClient::new(vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "msg-1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: answer.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]))
}

pub fn chat_message_text(message: &neo_ai::ChatMessage) -> String {
    let content = match message {
        neo_ai::ChatMessage::System { content }
        | neo_ai::ChatMessage::User { content }
        | neo_ai::ChatMessage::Assistant { content, .. }
        | neo_ai::ChatMessage::ToolResult { content, .. } => content,
    };
    content
        .iter()
        .filter_map(|part| match part {
            neo_ai::ContentPart::Text { text } => Some(text.as_str()),
            neo_ai::ContentPart::Thinking { .. }
            | neo_ai::ContentPart::Image { .. }
            | neo_ai::ContentPart::Video { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("")
}
