//! Extracted: `/btw` sidecar panel — open/close, context inheritance, prompt submission.

use std::sync::Arc;

use anyhow::Result;

use neo_agent_core::AgentMessage;
use neo_tui::widgets::btw_panel::BtwPhase;

use super::InteractiveController;

impl InteractiveController {
    /// Cancel any running `/btw` sidecar and clear its receiver.
    pub(super) fn cancel_btw_sidecar(&mut self) {
        if let Some(runner) = self.btw_runner.take() {
            runner.cancel();
        }
        self.btw_receiver = None;
    }

    /// Open or focus the `/btw` sidecar panel.
    ///
    /// If `initial_prompt` is `Some`, a sidecar turn is started immediately using
    /// the sidecar runner's in-memory conversation. The main turn is never
    /// touched.
    pub(super) async fn open_btw_panel(&mut self, initial_prompt: Option<String>) {
        if self.tui.chrome().has_btw_panel() {
            if initial_prompt.is_none() {
                self.update_btw_panel_error("BTW sidecar is already open.");
                return;
            }
            if self
                .tui
                .chrome()
                .btw_panel_state()
                .is_some_and(|state| state.sidecar.phase == BtwPhase::Running)
            {
                self.update_btw_panel_error(
                    "Wait for /btw to finish before sending another question.",
                );
                return;
            }
        } else {
            let sidecar_id = uuid::Uuid::new_v4().to_string();
            let state = neo_tui::widgets::btw_panel::BtwPanelState::new(
                neo_tui::widgets::btw_panel::BtwSidecar::new(sidecar_id)
                    .with_parent_session_id(self.active_session_id.clone().unwrap_or_default()),
            );
            self.tui.chrome_mut().set_btw_panel_state(Some(state));
        }

        if self.btw_runner.is_none() {
            let Some(runner) = self.create_btw_runner().await else {
                return;
            };
            self.btw_runner = Some(runner);
        }

        if let Some(prompt) = initial_prompt {
            let Some(runner) = self.btw_runner.as_ref() else {
                return;
            };
            match runner.run(prompt).await {
                Ok(receiver) => {
                    self.btw_receiver = Some(receiver);
                }
                Err(error) => {
                    self.push_status(format!("BTW failed to start: {error}"));
                    self.update_btw_panel_error(&error.to_string());
                }
            }
        }
    }

    async fn create_btw_runner(&mut self) -> Option<crate::modes::btw::BtwRunner> {
        let Some(config) = self.local_config.clone() else {
            self.push_status("BTW requires a loaded config");
            return None;
        };

        let inherited_messages = self.load_btw_inherited_messages(&config).await;
        let model = self.resolve_btw_model(&config)?;
        let client = self.resolve_btw_client(&config, &model)?;

        Some(crate::modes::btw::BtwRunner::new(
            model,
            client,
            config,
            &inherited_messages,
        ))
    }

    async fn load_btw_inherited_messages(
        &self,
        config: &crate::config::AppConfig,
    ) -> Vec<AgentMessage> {
        if !self.session_messages.is_empty() {
            return self.session_messages.clone();
        }
        let Some(session_id) = self.active_session_id.as_ref() else {
            return Vec::new();
        };
        match crate::modes::sessions::session_path(session_id, config) {
            Ok(path) => {
                match neo_agent_core::session::JsonlSessionReader::replay_context(&path).await {
                    Ok(context) => context.messages().to_vec(),
                    Err(error) => {
                        tracing::warn!(?error, "failed to replay session for /btw context");
                        Vec::new()
                    }
                }
            }
            Err(error) => {
                tracing::warn!(?error, "failed to resolve session path for /btw context");
                Vec::new()
            }
        }
    }

    #[allow(clippy::unnecessary_wraps)]
    fn resolve_btw_model(
        &mut self,
        config: &crate::config::AppConfig,
    ) -> Option<neo_ai::ModelSpec> {
        #[cfg(test)]
        {
            let _ = self;
            Some(
                crate::modes::run::resolve_model(config).unwrap_or_else(|_| neo_ai::ModelSpec {
                    provider: neo_ai::ProviderId("test-provider".to_owned()),
                    model: "test-model".to_owned(),
                    api: neo_ai::ApiKind::Local,
                    capabilities: neo_ai::ModelCapabilities::tool_chat(),
                }),
            )
        }
        #[cfg(not(test))]
        match crate::modes::run::resolve_model(config) {
            Ok(model) => Some(model),
            Err(error) => {
                self.push_status(format!("BTW model unavailable: {error}"));
                self.update_btw_panel_error(&error.to_string());
                None
            }
        }
    }

    fn resolve_btw_client(
        &mut self,
        config: &crate::config::AppConfig,
        model: &neo_ai::ModelSpec,
    ) -> Option<Arc<dyn neo_ai::ModelClient>> {
        #[cfg(test)]
        if let Some(client) = self.btw_client.clone() {
            return Some(client);
        }
        match crate::modes::run::resolve_model_client(config, model) {
            Ok(client) => Some(client),
            Err(error) => {
                self.push_status(format!("BTW model client unavailable: {error}"));
                self.update_btw_panel_error(&error.to_string());
                None
            }
        }
    }

    fn update_btw_panel_error(&mut self, message: &str) {
        if let Some(state) = self.tui.chrome_mut().btw_panel_state_mut() {
            state.status_message = Some(message.to_owned());
        }
    }

    /// Drain any pending `/btw` sidecar events into the panel state.
    pub(super) fn drain_btw_sidecar(&mut self) {
        let Some(receiver) = &mut self.btw_receiver else {
            return;
        };
        while let Ok(event) = receiver.try_recv() {
            if let Some(state) = self.tui.chrome_mut().btw_panel_state_mut() {
                crate::modes::btw::update_btw_panel_state(state, event);
            }
        }
    }

    /// Send the current composer text to the `/btw` sidecar instead of the main
    /// turn. The main turn path is bypassed entirely.
    pub(super) async fn submit_btw_prompt(&mut self) -> Result<()> {
        // If a sidecar turn is already running, preserve the user's typed text
        // and show a busy notice instead of starting a second concurrent turn.
        if self
            .tui
            .chrome()
            .btw_panel_state()
            .is_some_and(|state| state.sidecar.phase == BtwPhase::Running)
        {
            if let Some(state) = self.tui.chrome_mut().btw_panel_state_mut() {
                state.status_message =
                    Some("Wait for /btw to finish before sending another question.".to_owned());
            }
            return Ok(());
        }

        let Some(prompt) = self.tui.chrome_mut().submit_prompt() else {
            return Ok(());
        };
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Ok(());
        }

        if self.btw_runner.is_none() {
            let Some(runner) = self.create_btw_runner().await else {
                return Ok(());
            };
            self.btw_runner = Some(runner);
        }

        let Some(runner) = self.btw_runner.as_ref() else {
            return Ok(());
        };
        match runner.run(prompt.to_owned()).await {
            Ok(receiver) => {
                self.btw_receiver = Some(receiver);
                self.drain_btw_sidecar();
            }
            Err(error) => {
                self.push_status(format!("BTW failed to start: {error}"));
                self.update_btw_panel_error(&error.to_string());
            }
        }
        Ok(())
    }
}
