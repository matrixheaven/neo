//! Custom endpoint provider behavior (moved from `custom_endpoint_provider.rs`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use neo_agent_core::{AgentEvent, PermissionMode};
use neo_ai::{ReasoningCapability, ReasoningEffort};
use neo_tui::dialogs::{
    CustomEndpointAuthDraft, CustomEndpointModelDraft, CustomEndpointModelSource,
    CustomEndpointProviderDraft, CustomEndpointWizardOptions,
};

use super::*;
use crate::config::{AppConfig, Defaults, McpConfig, RuntimeConfig, TuiConfig};

fn test_config(project_dir: &Path, sessions_dir: PathBuf) -> AppConfig {
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

fn test_controller(temp: &tempfile::TempDir) -> InteractiveController {
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(test_config(temp.path(), sessions_dir));
    controller
}

fn open_wizard(controller: &mut InteractiveController) -> neo_tui::shell::OverlayId {
    let theme = controller.tui.chrome().theme();
    controller
        .tui
        .chrome_mut()
        .open_custom_endpoint_wizard(CustomEndpointWizardOptions { theme })
}

#[allow(clippy::duration_suboptimal_units)]
fn pending_fetch_for_current_wizard(
    controller: &InteractiveController,
) -> PendingCustomEndpointFetch {
    let overlay_id = controller
        .tui
        .chrome()
        .focused_overlay_id()
        .expect("focused overlay");
    let draft = controller
        .tui
        .chrome()
        .current_custom_endpoint_provider_draft()
        .expect("wizard draft");
    PendingCustomEndpointFetch {
        overlay_id,
        draft_key: CustomEndpointFetchKey::from_draft(&draft),
        working_label: "Fetching /models...".to_owned(),
        handle: tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(Vec::new())
        }),
    }
}

#[allow(clippy::duration_suboptimal_units)]
fn pending_test_for_current_wizard(
    controller: &InteractiveController,
    working_label: &str,
) -> PendingCustomEndpointTest {
    let overlay_id = controller
        .tui
        .chrome()
        .focused_overlay_id()
        .expect("focused overlay");
    let draft = controller
        .tui
        .chrome()
        .current_custom_endpoint_provider_draft()
        .expect("wizard draft");
    PendingCustomEndpointTest {
        overlay_id,
        draft_key: CustomEndpointTestKey::from_draft(&draft),
        working_label: working_label.to_owned(),
        handle: tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(())
        }),
    }
}

fn finished_test_for_current_wizard(
    controller: &InteractiveController,
    working_label: &str,
) -> PendingCustomEndpointTest {
    let overlay_id = controller
        .tui
        .chrome()
        .focused_overlay_id()
        .expect("focused overlay");
    let draft = controller
        .tui
        .chrome()
        .current_custom_endpoint_provider_draft()
        .expect("wizard draft");
    PendingCustomEndpointTest {
        overlay_id,
        draft_key: CustomEndpointTestKey::from_draft(&draft),
        working_label: working_label.to_owned(),
        handle: tokio::spawn(async { Ok(()) }),
    }
}

#[test]
fn custom_endpoint_model_conversion_adds_reasoning_capability_tag() {
    let draft = CustomEndpointModelDraft {
        source: CustomEndpointModelSource::Manual,
        model_id: "reasoner-large".to_owned(),
        alias: "acme/reasoner-large".to_owned(),
        display_name: Some("Reasoner Large".to_owned()),
        max_context_tokens: Some(128_000),
        max_output_tokens: Some(16_000),
        streaming: true,
        tools: true,
        images: true,
        embeddings: true,
        reasoning: ReasoningCapability::Effort {
            values: vec![ReasoningEffort::low(), ReasoningEffort::high()],
            disable_supported: true,
        },
    };

    let config = model_config_from_draft("acme", &draft);

    assert_eq!(config.provider, "acme");
    assert_eq!(config.model, "reasoner-large");
    assert_eq!(
        config.capabilities,
        vec!["streaming", "tools", "images", "embeddings", "reasoning"]
    );
    assert_eq!(config.reasoning, draft.reasoning);
}

#[test]
fn parses_openai_family_model_list_as_id_discovery() {
    let body = r#"
{
  "object": "list",
  "data": [
    {
      "id": "qwen2.5-coder-32b-instruct",
      "object": "model",
      "created": 1700000000,
      "owned_by": "acme",
      "context_length": 131072
    }
  ]
}
"#;

    let models = super::parse_openai_models_response(body).expect("parse models");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "qwen2.5-coder-32b-instruct");
    assert_eq!(models[0].owned_by.as_deref(), Some("acme"));
    assert_eq!(models[0].created, Some(1_700_000_000));
}

#[tokio::test]
async fn custom_endpoint_save_writes_config_and_closes_wizard() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let mut controller = test_controller(&temp);
    open_wizard(&mut controller);

    controller.save_custom_endpoint_provider(CustomEndpointProviderDraft {
        display_name: "Acme Gateway".to_owned(),
        provider_id: "acme".to_owned(),
        api_type: neo_ai::ApiType::OpenAi,
        base_url: "https://gateway.example.com/v1".to_owned(),
        auth: CustomEndpointAuthDraft::EnvVar("ACME_API_KEY".to_owned()),
        models: vec![CustomEndpointModelDraft {
            source: CustomEndpointModelSource::Manual,
            model_id: "reasoner".to_owned(),
            alias: "acme/reasoner".to_owned(),
            display_name: Some("Reasoner".to_owned()),
            max_context_tokens: Some(128_000),
            max_output_tokens: Some(8_192),
            streaming: true,
            tools: true,
            images: false,
            embeddings: false,
            reasoning: ReasoningCapability::Toggle {
                disable_supported: true,
            },
        }],
    });

    assert!(controller.tui.chrome().focused_overlay().is_none());
    let written = fs::read_to_string(temp.path().join(".neo/config.toml")).expect("read config");
    assert!(written.contains("[providers.acme]"), "{written}");
    assert!(
        written.contains("display_name = \"Acme Gateway\""),
        "{written}"
    );
    assert!(written.contains("[models.\"acme/reasoner\"]"), "{written}");
    let config = controller.local_config.as_ref().expect("refreshed config");
    let provider = config.providers.get("acme").expect("provider");
    assert_eq!(provider.provider_type, Some(neo_ai::ApiType::OpenAi));
    assert_eq!(provider.display_name.as_deref(), Some("Acme Gateway"));
    assert_eq!(
        provider.base_url.as_deref(),
        Some("https://gateway.example.com/v1")
    );
    assert_eq!(provider.api_key_env.as_deref(), Some("ACME_API_KEY"));

    let model = config.models.get("acme/reasoner").expect("model");
    assert_eq!(model.provider, "acme");
    assert_eq!(model.model, "reasoner");
    assert_eq!(model.capabilities, vec!["streaming", "tools", "reasoning"]);
    assert_eq!(
        model.reasoning,
        ReasoningCapability::Toggle {
            disable_supported: true,
        }
    );
}

#[tokio::test]
async fn custom_endpoint_duplicate_fetch_keeps_existing_pending_request() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let mut controller = test_controller(&temp);
    open_wizard(&mut controller);
    controller.pending_custom_endpoint_fetch = Some(pending_fetch_for_current_wizard(&controller));

    controller.start_custom_endpoint_fetch();

    assert!(controller.pending_custom_endpoint_fetch.is_some());
    assert!(
        controller
            .pending_custom_endpoint_fetch
            .as_ref()
            .is_some_and(|pending| !pending.handle.is_finished())
    );
    controller.abort_pending_custom_endpoint_fetch();
}

#[tokio::test]
async fn custom_endpoint_abandoned_fetch_is_aborted_on_poll() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let mut controller = test_controller(&temp);
    let overlay_id = open_wizard(&mut controller);
    controller.pending_custom_endpoint_fetch = Some(pending_fetch_for_current_wizard(&controller));
    controller.tui.chrome_mut().close_overlay(overlay_id);

    assert!(controller.poll_pending_custom_endpoint_fetch().await);

    assert!(controller.pending_custom_endpoint_fetch.is_none());
}

#[test]
fn custom_endpoint_test_key_ignores_unprobed_model_metadata() {
    let mut first = CustomEndpointProviderDraft {
        display_name: "Acme".to_owned(),
        provider_id: "acme".to_owned(),
        api_type: neo_ai::ApiType::OpenAi,
        base_url: "https://gateway.example.com/v1".to_owned(),
        auth: CustomEndpointAuthDraft::LocalPlaceholder,
        models: vec![CustomEndpointModelDraft {
            source: CustomEndpointModelSource::Manual,
            model_id: "reasoner".to_owned(),
            alias: "acme/reasoner".to_owned(),
            display_name: Some("Reasoner".to_owned()),
            max_context_tokens: Some(128_000),
            max_output_tokens: Some(8_192),
            streaming: true,
            tools: true,
            images: false,
            embeddings: false,
            reasoning: ReasoningCapability::None,
        }],
    };
    let mut second = first.clone();
    second.display_name = "Renamed".to_owned();
    second.models[0].display_name = Some("Renamed model".to_owned());
    second.models[0].tools = false;

    assert_eq!(
        CustomEndpointTestKey::from_draft(&first),
        CustomEndpointTestKey::from_draft(&second)
    );

    first.base_url = "https://other.example.com/v1".to_owned();
    assert_ne!(
        CustomEndpointTestKey::from_draft(&first),
        CustomEndpointTestKey::from_draft(&second)
    );
}

#[tokio::test]
async fn custom_endpoint_abandoned_test_is_aborted_on_poll() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let mut controller = test_controller(&temp);
    let overlay_id = open_wizard(&mut controller);
    controller.pending_custom_endpoint_test = Some(pending_test_for_current_wizard(
        &controller,
        "Testing acme/reasoner...",
    ));
    controller.tui.chrome_mut().close_overlay(overlay_id);

    assert!(controller.poll_pending_custom_endpoint_test().await);

    assert!(controller.pending_custom_endpoint_test.is_none());
}

#[tokio::test]
async fn custom_endpoint_fetch_abort_keeps_unrelated_test_working_label() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let mut controller = test_controller(&temp);
    open_wizard(&mut controller);
    controller.pending_custom_endpoint_fetch = Some(pending_fetch_for_current_wizard(&controller));
    controller.pending_custom_endpoint_test = Some(pending_test_for_current_wizard(
        &controller,
        "Testing acme/reasoner...",
    ));
    controller
        .tui
        .chrome_mut()
        .set_custom_working_label(Some("Testing acme/reasoner...".to_owned()));

    controller.abort_pending_custom_endpoint_fetch();

    assert_eq!(
        controller.tui.chrome().working_label().as_deref(),
        Some("Testing acme/reasoner...")
    );
    controller.abort_pending_custom_endpoint_test();
}

#[tokio::test]
async fn custom_endpoint_finished_test_restores_pending_fetch_working_label() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let mut controller = test_controller(&temp);
    open_wizard(&mut controller);
    controller.pending_custom_endpoint_fetch = Some(pending_fetch_for_current_wizard(&controller));
    controller.pending_custom_endpoint_test = Some(finished_test_for_current_wizard(
        &controller,
        "Testing acme/reasoner...",
    ));
    controller
        .tui
        .chrome_mut()
        .set_custom_working_label(Some("Testing acme/reasoner...".to_owned()));
    while controller
        .pending_custom_endpoint_test
        .as_ref()
        .is_some_and(|pending| !pending.handle.is_finished())
    {
        tokio::task::yield_now().await;
    }

    controller.poll_pending_custom_endpoint_test().await;

    assert_eq!(
        controller.tui.chrome().working_label().as_deref(),
        Some("Fetching /models...")
    );
    controller.abort_pending_custom_endpoint_fetch();
}
