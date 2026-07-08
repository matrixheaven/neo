use anyhow::Result;

use super::{
    ContextWindow, InputResult, InteractiveController, PermissionMode, SelectedModel,
    dialog_result_may_close,
};

impl InteractiveController {
    /// Dispatch a rich dialog result after an input event was forwarded.
    pub(super) async fn process_rich_dialog_result(&mut self, result: InputResult) -> Result<()> {
        if !dialog_result_may_close(result) {
            return Ok(());
        }
        if self.process_model_dialog_result() {
            return Ok(());
        }
        if self.process_provider_dialog_result().await {
            return Ok(());
        }
        self.process_question_dialog_result().await
    }

    pub(super) fn process_model_dialog_result(&mut self) -> bool {
        if self
            .tui
            .chrome_mut()
            .tabbed_model_selector_result()
            .is_some()
        {
            self.apply_tabbed_model_selection();
        } else if self.tui.chrome_mut().model_selector_result().is_some() {
            self.apply_model_selector_result();
        } else {
            return false;
        }
        true
    }

    pub(super) async fn process_provider_dialog_result(&mut self) -> bool {
        if self.tui.chrome_mut().provider_manager_action().is_some() {
            self.handle_provider_manager_action();
        } else if self.tui.chrome_mut().workspace_manager_action().is_some() {
            self.handle_workspace_manager_action();
        } else if self.tui.chrome_mut().mcp_manager_action().is_some() {
            self.handle_mcp_manager_action().await;
        } else if self.tui.chrome_mut().confirm_dialog_result().is_some() {
            if let Some(result) = self.tui.chrome_mut().take_confirm_dialog_result() {
                self.handle_workspace_confirm_result(result);
            }
        } else if self.tui.chrome_mut().choice_picker_result().is_some() {
            self.handle_choice_picker_result().await;
        } else if self.tui.chrome_mut().text_input_result().is_some() {
            self.handle_text_input_result();
        } else if self.tui.chrome_mut().api_key_input_result().is_some() {
            self.handle_api_key_input_result();
        } else if self
            .tui
            .chrome_mut()
            .custom_registry_import_result()
            .is_some()
        {
            self.handle_custom_registry_import_result();
        } else if self.tui.chrome_mut().mcp_add_form_result().is_some() {
            self.handle_mcp_add_form_result().await;
        } else {
            return false;
        }
        true
    }

    pub(super) async fn process_question_dialog_result(&mut self) -> Result<()> {
        if let Some(result) = self.tui.chrome_mut().take_question_result() {
            self.resolve_question(&result.id, result.answers).await?;
        }
        Ok(())
    }

    /// Apply a model selection, updating the active model, context window,
    /// thinking state, and footer indicator.
    pub(super) fn apply_model_selection(&mut self, selection: &neo_tui::dialogs::ModelSelection) {
        self.tui
            .chrome_mut()
            .set_model_label(selection.alias.clone());
        let selected_model = SelectedModel::from_alias(
            &selection.alias,
            self.local_config.as_ref(),
            &self.model_items,
        )
        .map_or_else(
            |error| {
                tracing::warn!("failed to parse selected model: {error}");
                None
            },
            Some,
        );
        let max_ctx = selected_model
            .as_ref()
            .and_then(|model| model.max_context_tokens);
        self.tui
            .chrome_mut()
            .set_context_window(max_ctx.map(ContextWindow::new));
        self.active_model = selected_model;
        self.current_thinking = selection.thinking;
        self.tui
            .chrome_mut()
            .set_thinking_enabled(selection.thinking);
        if let Some(config) = self.local_config.as_mut() {
            config.runtime.reasoning_effort = if selection.thinking {
                Some(neo_ai::ReasoningEffort::High)
            } else {
                None
            };
            // Keep the in-memory config in sync so subsequent turns and the next
            // startup resolve the same model.
            config.default_model.clone_from(&selection.alias);
            if let Some(model) = &self.active_model {
                config.default_provider.clone_from(&model.provider);
            }
        }
        // Persist the selection to disk so the next session opens on the same
        // model the user chose, instead of reverting to a stale default.
        if let Some(config_path) = self.config_path()
            && let Err(error) =
                crate::config::mutations::set_default_model(&config_path, &selection.alias)
        {
            tracing::warn!("failed to persist default model: {error}");
        }
        let notice = if selection.thinking {
            format!("Switched to {} (thinking: on)", selection.alias)
        } else {
            format!("Switched to {}", selection.alias)
        };
        self.push_status(notice);
    }

    /// Apply the selection from the rich `TabbedModelSelector` dialog.
    pub(super) fn apply_tabbed_model_selection(&mut self) {
        let result = self
            .tui
            .chrome_mut()
            .tabbed_model_selector_result()
            .cloned();
        let Some(result) = result else {
            return;
        };
        self.tui.chrome_mut().close_focused_overlay();
        if let neo_tui::dialogs::ModelSelectorResult::Selected(selection) = result {
            self.apply_model_selection(&selection);
        }
    }

    /// Apply the selection from the flat `ModelSelector` dialog.
    pub(super) fn apply_model_selector_result(&mut self) {
        let result = self.tui.chrome_mut().model_selector_result().cloned();
        let Some(result) = result else {
            return;
        };
        self.tui.chrome_mut().close_focused_overlay();
        if let neo_tui::dialogs::ModelSelectorResult::Selected(selection) = result {
            self.apply_model_selection(&selection);
        }
    }

    /// Handle a `ProviderManager` action (Add / `DeleteSource` / Close).
    pub(super) fn handle_provider_manager_action(&mut self) {
        let action = self.tui.chrome_mut().provider_manager_action();
        let Some(action) = action else {
            return;
        };
        match action {
            neo_tui::dialogs::ProviderManagerAction::Close => {
                self.tui.chrome_mut().close_focused_overlay();
            }
            neo_tui::dialogs::ProviderManagerAction::Add => {
                self.tui.chrome_mut().close_focused_overlay();
                self.open_add_provider_picker();
            }
            neo_tui::dialogs::ProviderManagerAction::DeleteSource(ids) => {
                self.tui.chrome_mut().close_focused_overlay();
                self.delete_provider_sources(&ids);
            }
        }
    }

    pub(super) fn open_add_provider_picker(&mut self) {
        let theme = self.tui.chrome().theme();
        self.tui
            .chrome_mut()
            .open_choice_picker(neo_tui::dialogs::ChoicePickerOptions {
                title: "Add Provider".to_owned(),
                items: vec![
                    neo_tui::dialogs::ChoiceItem::new("known", "Known third-party provider")
                        .with_description("Import from models.dev catalog"),
                    neo_tui::dialogs::ChoiceItem::new("custom", "Custom registry (api.json)")
                        .with_description("Import from a custom registry URL"),
                ],
                initial_id: None,
                theme,
                page_size: 0,
                current_id: None,
            });
    }

    pub(super) fn delete_provider_sources(&mut self, ids: &[String]) {
        let Some(config_path) = self.config_path() else {
            return;
        };
        for id in ids {
            if let Err(error) = crate::config::mutations::remove_provider(&config_path, id) {
                self.push_status(format!("Failed to remove provider {id}: {error}"));
            }
        }
        self.push_status(format!("Removed {} provider(s)", ids.len()));
        self.refresh_config();
    }

    /// Handle a `ChoicePicker` result.
    pub(super) async fn handle_choice_picker_result(&mut self) {
        let Some(result) = self.tui.chrome_mut().choice_picker_result().cloned() else {
            return;
        };
        self.tui.chrome_mut().close_focused_overlay();
        match result {
            neo_tui::dialogs::ChoiceResult::Selected(item) => {
                self.handle_selected_choice_item(&item.id).await;
            }
            neo_tui::dialogs::ChoiceResult::Cancelled => {
                self.pending_interactive_workflow = None;
                self.pending_preflight = None;
            }
        }
    }

    pub(super) async fn handle_selected_choice_item(&mut self, id: &str) {
        if self.handle_preflight_choice_item(id).await {
            return;
        }
        if self.handle_permission_choice_item(id) {
            return;
        }
        if self.handle_catalog_choice_item(id) {
            return;
        }
        self.handle_builtin_choice_item(id);
    }

    pub(super) async fn handle_preflight_choice_item(&mut self, id: &str) -> bool {
        let Some(preflight) = self.pending_preflight.clone() else {
            return false;
        };
        let Some(action) = preflight.action_for_choice(id) else {
            return false;
        };
        let workflow = self.pending_interactive_workflow.take();
        self.pending_preflight = None;
        match action {
            super::PreflightAction::SwitchPermissionMode(mode) => {
                self.set_permission_mode(mode);
                self.start_pending_interactive_workflow_if_present(workflow, false)
                    .await;
            }
            super::PreflightAction::ContinueAutoBestEffort => {
                self.start_pending_interactive_workflow_if_present(workflow, true)
                    .await;
            }
            super::PreflightAction::Cancel => {
                self.push_status("Interactive workflow cancelled");
            }
        }
        true
    }

    async fn start_pending_interactive_workflow_if_present(
        &mut self,
        workflow: Option<super::PendingInteractiveWorkflow>,
        auto_mode_best_effort: bool,
    ) {
        let Some(workflow) = workflow else {
            return;
        };
        if let Err(error) = self
            .start_pending_interactive_workflow(workflow, auto_mode_best_effort)
            .await
        {
            self.push_status(format!("Failed to start interactive workflow: {error}"));
        }
    }

    async fn start_pending_interactive_workflow(
        &mut self,
        workflow: super::PendingInteractiveWorkflow,
        action_auto_mode_best_effort: bool,
    ) -> Result<()> {
        match workflow {
            super::PendingInteractiveWorkflow::Init { instruction } => {
                self.run_init_workflow(&instruction, action_auto_mode_best_effort)
                    .await
            }
            super::PendingInteractiveWorkflow::Skill {
                directives,
                generated_prompt,
            } => {
                self.start_skill_workflow_from_directives(
                    directives,
                    generated_prompt,
                    action_auto_mode_best_effort,
                )
                .await
            }
        }
    }

    async fn start_skill_workflow_from_directives(
        &mut self,
        directives: super::InlineSkillDirectives,
        generated_prompt: Option<String>,
        auto_mode_best_effort: bool,
    ) -> Result<()> {
        let skill_names = directives
            .invocations
            .iter()
            .map(|invocation| invocation.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let (stripped_prompt, _display_body) = self.activate_skill_directives(directives)?;
        let prompt = if let Some(generated_prompt) = generated_prompt {
            format!("Run the activated skill workflow for {skill_names}.\n\n{generated_prompt}")
        } else if stripped_prompt.trim().is_empty() {
            "Run the activated skill workflow.".to_owned()
        } else {
            stripped_prompt
        };
        let prompt = if auto_mode_best_effort {
            format!(
                "{}\n\n{}",
                super::interactive_preflight::auto_best_effort_note(),
                prompt
            )
        } else {
            prompt
        };
        self.start_generated_injection_turn_from_text(prompt, "skill", "/skill workflow")?;
        self.wait_for_active_turn().await?;
        self.start_pending_background_question_followups().await
    }

    pub(super) fn handle_builtin_choice_item(&mut self, id: &str) -> bool {
        if self.handle_mcp_choice_item(id) {
            return true;
        }
        match id {
            "known" => self.fetch_known_catalog(),
            "custom" => self.open_custom_registry_import(),
            _ => return false,
        }
        true
    }

    pub(super) fn handle_permission_choice_item(&mut self, id: &str) -> bool {
        match id {
            "permission:ask" => self.set_permission_mode(PermissionMode::Ask),
            "permission:auto" => self.set_permission_mode(PermissionMode::Auto),
            "permission:yolo" => self.set_permission_mode(PermissionMode::Yolo),
            _ => return false,
        }
        true
    }

    pub(super) fn handle_catalog_choice_item(&mut self, id: &str) -> bool {
        if let Some(provider_id) = id.strip_prefix("catalog:") {
            self.open_catalog_api_key_input(provider_id);
            return true;
        }
        if let Some(provider_id) = id.strip_prefix("custom-catalog:") {
            self.import_custom_catalog_provider(provider_id);
            return true;
        }
        false
    }

    /// Handle an API key input result.
    pub(super) fn handle_text_input_result(&mut self) {
        let Some(result) = self.tui.chrome_mut().text_input_result().cloned() else {
            return;
        };
        if self.handle_workspace_text_input_result(result.clone()) {
            return;
        }
        self.tui.chrome_mut().close_focused_overlay();
        match result {
            neo_tui::dialogs::TextInputResult::Submitted(_value) => {}
            neo_tui::dialogs::TextInputResult::Cancelled => {}
        }
    }
}
