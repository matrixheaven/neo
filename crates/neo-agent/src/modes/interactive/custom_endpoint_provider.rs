use std::time::Duration;

use neo_tui::dialogs::{
    CustomEndpointAuthDraft, CustomEndpointFetchedModel, CustomEndpointModelDraft,
    CustomEndpointProviderDraft, CustomEndpointWizardAction, CustomEndpointWizardOptions,
};

use super::InteractiveController;

pub(super) struct PendingCustomEndpointFetch {
    pub(super) overlay_id: neo_tui::shell::OverlayId,
    draft_key: CustomEndpointFetchKey,
    working_label: String,
    pub(super) handle: tokio::task::JoinHandle<anyhow::Result<Vec<CustomEndpointFetchedModel>>>,
}

pub(super) struct PendingCustomEndpointTest {
    pub(super) overlay_id: neo_tui::shell::OverlayId,
    draft_key: CustomEndpointTestKey,
    working_label: String,
    pub(super) handle: tokio::task::JoinHandle<Result<(), String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CustomEndpointFetchKey {
    provider_id: String,
    api_type: neo_ai::ApiType,
    base_url: String,
    auth: CustomEndpointAuthDraft,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CustomEndpointTestKey {
    api_type: neo_ai::ApiType,
    base_url: String,
    auth: CustomEndpointAuthDraft,
}

impl InteractiveController {
    pub(super) fn handle_custom_endpoint_choice_item(&mut self, id: &str) -> bool {
        if id != "custom-endpoint" {
            return false;
        }

        let theme = self.tui.chrome().theme();
        self.tui
            .chrome_mut()
            .open_custom_endpoint_wizard(CustomEndpointWizardOptions { theme });
        true
    }

    pub(super) fn handle_custom_endpoint_wizard_action(&mut self) -> bool {
        let Some(action) = self.tui.chrome_mut().take_custom_endpoint_wizard_action() else {
            return false;
        };

        match action {
            CustomEndpointWizardAction::FetchModels => self.start_custom_endpoint_fetch(),
            CustomEndpointWizardAction::TestConnection(draft) => {
                self.start_custom_endpoint_test(draft);
            }
            CustomEndpointWizardAction::Save(draft) => self.save_custom_endpoint_provider(draft),
            CustomEndpointWizardAction::Cancelled => {
                self.abort_pending_custom_endpoint_fetch_for_focused_overlay();
                self.abort_pending_custom_endpoint_test_for_focused_overlay();
                self.tui.chrome_mut().close_focused_overlay();
            }
        }
        true
    }

    fn start_custom_endpoint_fetch(&mut self) {
        if let Some(pending) = &self.pending_custom_endpoint_fetch {
            if self.custom_endpoint_fetch_still_matches(pending) {
                self.push_status("Fetch from /models is already running");
                return;
            }
            self.abort_pending_custom_endpoint_fetch();
        }
        let Some(overlay_id) = self.tui.chrome().focused_overlay_id() else {
            self.push_status("Custom endpoint wizard is no longer open");
            return;
        };
        let Some(draft) = self.tui.chrome().current_custom_endpoint_provider_draft() else {
            self.push_status("Custom endpoint wizard is no longer open");
            return;
        };
        if !matches!(
            draft.api_type,
            neo_ai::ApiType::OpenAi | neo_ai::ApiType::OpenAiResponse
        ) {
            self.push_status(
                "Fetch from /models is only available for OpenAI-compatible protocols",
            );
            return;
        }
        let bearer_token = match bearer_token_for_auth(&draft.auth) {
            Ok(token) => token,
            Err(error) => {
                self.push_status(format!("Error: Failed to fetch /models: {error}"));
                return;
            }
        };

        let working_label = "Fetching /models...".to_owned();
        self.tui
            .chrome_mut()
            .set_custom_working_label(Some(working_label.clone()));
        let draft_key = CustomEndpointFetchKey::from_draft(&draft);
        let base_url = draft.base_url;
        self.pending_custom_endpoint_fetch = Some(PendingCustomEndpointFetch {
            overlay_id,
            draft_key,
            working_label,
            handle: tokio::spawn(async move {
                fetch_openai_family_models(base_url, bearer_token).await
            }),
        });
    }

    pub(super) async fn poll_pending_custom_endpoint_fetch(&mut self) -> bool {
        let Some(pending) = self.pending_custom_endpoint_fetch.take() else {
            return false;
        };
        if !pending.handle.is_finished() {
            if self.custom_endpoint_fetch_still_matches(&pending) {
                self.pending_custom_endpoint_fetch = Some(pending);
                return false;
            }
            pending.handle.abort();
            self.clear_custom_endpoint_working_label(&pending.working_label);
            self.push_status("Custom endpoint wizard changed before /models returned");
            return true;
        }

        self.clear_custom_endpoint_working_label(&pending.working_label);
        if !self.custom_endpoint_fetch_still_matches(&pending) {
            self.push_status("Custom endpoint wizard changed before /models returned");
            return true;
        }
        match pending.handle.await {
            Ok(Ok(models)) => {
                if models.is_empty() {
                    self.push_status("No models returned from /models");
                } else if !self
                    .tui
                    .chrome_mut()
                    .apply_custom_endpoint_fetched_models(models)
                {
                    self.push_status("Custom endpoint wizard is no longer open");
                }
            }
            Ok(Err(error)) => {
                self.push_status(format!("Error: Failed to fetch /models: {error}"));
            }
            Err(join_error) => {
                self.push_status(format!("Error: Failed to fetch /models: {join_error}"));
            }
        }
        true
    }

    #[allow(clippy::needless_pass_by_value)]
    fn start_custom_endpoint_test(&mut self, draft: CustomEndpointProviderDraft) {
        if let Some(pending) = &self.pending_custom_endpoint_test {
            if self.custom_endpoint_test_still_matches(pending) {
                self.push_status("Connection test is already running");
                return;
            }
            self.abort_pending_custom_endpoint_test();
        }
        let Some(overlay_id) = self.tui.chrome().focused_overlay_id() else {
            self.push_status("Custom endpoint wizard is no longer open");
            return;
        };
        let Some(model) = draft.models.first().cloned() else {
            self.push_status("Add a model before testing connection");
            return;
        };
        match draft.api_type {
            neo_ai::ApiType::OpenAi | neo_ai::ApiType::OpenAiResponse => {
                let token = match bearer_token_for_auth(&draft.auth) {
                    Ok(token) => token,
                    Err(error) => {
                        let _ = self
                            .tui
                            .chrome_mut()
                            .apply_custom_endpoint_test_result(Err(error.to_string()));
                        return;
                    }
                };
                let working_label = format!("Testing {}...", model.alias);
                self.tui
                    .chrome_mut()
                    .set_custom_working_label(Some(working_label.clone()));
                let base_url = draft.base_url.clone();
                let draft_key = CustomEndpointTestKey::from_draft(&draft);
                self.pending_custom_endpoint_test = Some(PendingCustomEndpointTest {
                    overlay_id,
                    draft_key,
                    working_label,
                    handle: tokio::spawn(async move {
                        fetch_openai_family_models(base_url, token)
                            .await
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    }),
                });
            }
            neo_ai::ApiType::Anthropic | neo_ai::ApiType::Google => {
                let _ = self.tui.chrome_mut().apply_custom_endpoint_test_result(Err(
                    "provider protocol does not expose /models in this wizard".to_owned(),
                ));
            }
        }
    }

    pub(super) async fn poll_pending_custom_endpoint_test(&mut self) -> bool {
        let Some(pending) = self.pending_custom_endpoint_test.take() else {
            return false;
        };
        if !pending.handle.is_finished() {
            if self.custom_endpoint_test_still_matches(&pending) {
                self.pending_custom_endpoint_test = Some(pending);
                return false;
            }
            pending.handle.abort();
            self.clear_custom_endpoint_working_label(&pending.working_label);
            self.push_status("Custom endpoint wizard changed before test returned");
            return true;
        }

        self.clear_custom_endpoint_working_label(&pending.working_label);
        if !self.custom_endpoint_test_still_matches(&pending) {
            self.push_status("Custom endpoint wizard changed before test returned");
            return true;
        }
        let result = match pending.handle.await {
            Ok(result) => result,
            Err(join_error) => Err(join_error.to_string()),
        };
        if !self
            .tui
            .chrome_mut()
            .apply_custom_endpoint_test_result(result)
        {
            self.push_status("Custom endpoint wizard is no longer open");
        }
        true
    }

    fn custom_endpoint_fetch_still_matches(&self, pending: &PendingCustomEndpointFetch) -> bool {
        if self.tui.chrome().focused_overlay_id() != Some(pending.overlay_id) {
            return false;
        }
        self.tui
            .chrome()
            .current_custom_endpoint_provider_draft()
            .is_some_and(|draft| CustomEndpointFetchKey::from_draft(&draft) == pending.draft_key)
    }

    fn custom_endpoint_test_still_matches(&self, pending: &PendingCustomEndpointTest) -> bool {
        if self.tui.chrome().focused_overlay_id() != Some(pending.overlay_id) {
            return false;
        }
        self.tui
            .chrome()
            .current_custom_endpoint_provider_draft()
            .is_some_and(|draft| CustomEndpointTestKey::from_draft(&draft) == pending.draft_key)
    }

    fn abort_pending_custom_endpoint_fetch(&mut self) {
        if let Some(pending) = self.pending_custom_endpoint_fetch.take() {
            pending.handle.abort();
            self.clear_custom_endpoint_working_label(&pending.working_label);
        }
    }

    fn abort_pending_custom_endpoint_test(&mut self) {
        if let Some(pending) = self.pending_custom_endpoint_test.take() {
            pending.handle.abort();
            self.clear_custom_endpoint_working_label(&pending.working_label);
        }
    }

    fn clear_custom_endpoint_working_label(&mut self, label: &str) {
        if self.tui.chrome().working_label().as_deref() == Some(label) {
            let next = self.next_custom_endpoint_working_label();
            self.tui.chrome_mut().set_custom_working_label(next);
        }
    }

    fn next_custom_endpoint_working_label(&self) -> Option<String> {
        self.pending_custom_endpoint_test
            .as_ref()
            .map(|pending| pending.working_label.clone())
            .or_else(|| {
                self.pending_custom_endpoint_fetch
                    .as_ref()
                    .map(|pending| pending.working_label.clone())
            })
    }

    fn abort_pending_custom_endpoint_fetch_for_focused_overlay(&mut self) {
        let focused = self.tui.chrome().focused_overlay_id();
        let should_abort = self
            .pending_custom_endpoint_fetch
            .as_ref()
            .is_some_and(|pending| Some(pending.overlay_id) == focused);
        if should_abort {
            self.abort_pending_custom_endpoint_fetch();
        }
    }

    fn abort_pending_custom_endpoint_test_for_focused_overlay(&mut self) {
        let focused = self.tui.chrome().focused_overlay_id();
        let should_abort = self
            .pending_custom_endpoint_test
            .as_ref()
            .is_some_and(|pending| Some(pending.overlay_id) == focused);
        if should_abort {
            self.abort_pending_custom_endpoint_test();
        }
    }

    #[allow(clippy::needless_pass_by_value)]
    fn save_custom_endpoint_provider(&mut self, draft: CustomEndpointProviderDraft) {
        let Some(config_path) = self.config_path() else {
            self.push_status("No config available");
            return;
        };

        let provider_id = draft.provider_id.clone();
        let models = draft
            .models
            .iter()
            .map(|model| {
                (
                    model.alias.clone(),
                    model_config_from_draft(&provider_id, model),
                )
            })
            .collect::<Vec<_>>();
        let provider_config = provider_config_from_draft(&draft);

        match crate::config::mutations::add_custom_endpoint_provider(
            &config_path,
            &provider_id,
            provider_config,
            models,
            None,
        ) {
            Ok(message) => {
                self.abort_pending_custom_endpoint_fetch_for_focused_overlay();
                self.abort_pending_custom_endpoint_test_for_focused_overlay();
                self.tui.chrome_mut().close_focused_overlay();
                self.push_status(message);
                self.refresh_config();
            }
            Err(error) => {
                self.push_status(format!("Error: Failed to add custom endpoint: {error}"));
            }
        }
    }
}

impl CustomEndpointFetchKey {
    fn from_draft(draft: &CustomEndpointProviderDraft) -> Self {
        Self {
            provider_id: draft.provider_id.clone(),
            api_type: draft.api_type,
            base_url: draft.base_url.clone(),
            auth: draft.auth.clone(),
        }
    }
}

impl CustomEndpointTestKey {
    fn from_draft(draft: &CustomEndpointProviderDraft) -> Self {
        Self {
            api_type: draft.api_type,
            base_url: draft.base_url.clone(),
            auth: draft.auth.clone(),
        }
    }
}

#[derive(serde::Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<OpenAiModelObject>,
}

#[derive(serde::Deserialize)]
struct OpenAiModelObject {
    id: String,
    #[serde(default)]
    created: Option<u64>,
    #[serde(default)]
    owned_by: Option<String>,
}

fn parse_openai_models_response(body: &str) -> anyhow::Result<Vec<CustomEndpointFetchedModel>> {
    let response: OpenAiModelsResponse = serde_json::from_str(body)?;
    Ok(response
        .data
        .into_iter()
        .filter(|model| !model.id.trim().is_empty())
        .map(|model| CustomEndpointFetchedModel {
            id: model.id,
            owned_by: model.owned_by,
            created: model.created,
        })
        .collect())
}

async fn fetch_openai_family_models(
    base_url: String,
    bearer_token: String,
) -> anyhow::Result<Vec<CustomEndpointFetchedModel>> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?
        .get(url)
        .bearer_auth(bearer_token)
        .send()
        .await?
        .error_for_status()?;
    let body = response.text().await?;
    parse_openai_models_response(&body)
}

fn bearer_token_for_auth(auth: &CustomEndpointAuthDraft) -> anyhow::Result<String> {
    match auth {
        CustomEndpointAuthDraft::EnvVar(name) => std::env::var(name)
            .map_err(|_| anyhow::anyhow!("environment variable {name} is not set")),
        CustomEndpointAuthDraft::InlineSecret(secret) => Ok(secret.clone()),
        CustomEndpointAuthDraft::LocalPlaceholder => Ok("local".to_owned()),
    }
}

fn provider_config_from_draft(
    draft: &CustomEndpointProviderDraft,
) -> crate::config::ProviderConfig {
    let mut config = crate::config::ProviderConfig {
        display_name: Some(draft.display_name.trim().to_owned()),
        provider_type: Some(draft.api_type),
        base_url: Some(draft.base_url.trim().to_owned()),
        api_key: None,
        api_key_env: None,
    };

    match &draft.auth {
        CustomEndpointAuthDraft::EnvVar(value) => {
            config.api_key_env = Some(value.trim().to_owned());
        }
        CustomEndpointAuthDraft::InlineSecret(value) => {
            config.api_key = Some(value.clone());
        }
        CustomEndpointAuthDraft::LocalPlaceholder => {
            config.api_key = Some("local".to_owned());
        }
    }

    config
}

fn model_config_from_draft(
    provider_id: &str,
    draft: &CustomEndpointModelDraft,
) -> crate::config::ModelConfig {
    let mut capabilities = Vec::new();
    if draft.streaming {
        capabilities.push("streaming".to_owned());
    }
    if draft.tools {
        capabilities.push("tools".to_owned());
    }
    if draft.images {
        capabilities.push("images".to_owned());
    }
    if draft.embeddings {
        capabilities.push("embeddings".to_owned());
    }
    if draft.reasoning.supports_reasoning() {
        capabilities.push("reasoning".to_owned());
    }

    crate::config::ModelConfig {
        provider: provider_id.to_owned(),
        model: draft.model_id.clone(),
        max_context_tokens: draft.max_context_tokens,
        max_output_tokens: draft.max_output_tokens,
        capabilities,
        reasoning: draft.reasoning.clone(),
        display_name: draft.display_name.clone(),
    }
}

#[cfg(test)]
mod tests {
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
            api_key_env: None,
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
            workflow_capability: neo_agent_core::workflow::WorkflowCapability::default(),
            multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
            tui: TuiConfig::default(),
            theme: crate::themes::ResolvedTheme::default(),
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
        let written =
            fs::read_to_string(temp.path().join(".neo/config.toml")).expect("read config");
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
        controller.pending_custom_endpoint_fetch =
            Some(pending_fetch_for_current_wizard(&controller));

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
        controller.pending_custom_endpoint_fetch =
            Some(pending_fetch_for_current_wizard(&controller));
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
        controller.pending_custom_endpoint_fetch =
            Some(pending_fetch_for_current_wizard(&controller));
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
        controller.pending_custom_endpoint_fetch =
            Some(pending_fetch_for_current_wizard(&controller));
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
}
