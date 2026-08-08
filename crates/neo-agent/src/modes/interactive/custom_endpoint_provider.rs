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
#[path = "test_cases/custom_endpoint_provider.rs"]
mod tests;
