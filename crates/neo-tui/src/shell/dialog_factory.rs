use crate::tasks_browser::TaskBrowserState;

use super::command_palette::{CommandPaletteState, CommandSpec};
use super::overlay::{Overlay, OverlayId, OverlayKind};
use super::pickers::{ModelPickerState, PickerItem, PromptCompletionPrefix, PromptCompletionState};
use super::session_picker::{SessionPickerItem, SessionPickerScope, SessionPickerState};
use super::state::NeoChromeState;

impl NeoChromeState {
    pub fn open_command_palette(
        &mut self,
        commands: impl IntoIterator<Item = CommandSpec>,
    ) -> OverlayId {
        self.push_overlay(Overlay::new(
            "commands",
            OverlayKind::CommandPalette(CommandPaletteState::new(commands)),
        ))
    }

    #[must_use]
    pub fn selected_command(&self) -> Option<CommandSpec> {
        let OverlayKind::CommandPalette(palette) = &self.focused_overlay()?.kind else {
            return None;
        };
        palette.confirm()
    }

    pub fn confirm_command_palette(&mut self) -> Option<CommandSpec> {
        let id = self.focused_overlay;
        let selected = self.selected_command()?;
        if let Some(id) = id {
            let _ = self.close_overlay(id);
        }
        Some(selected)
    }

    pub fn open_session_picker(
        &mut self,
        current_session_id: &str,
        scope: SessionPickerScope,
        items: impl IntoIterator<Item = SessionPickerItem>,
    ) -> OverlayId {
        self.push_overlay(Overlay::new(
            "sessions",
            OverlayKind::SessionPicker(SessionPickerState::new(
                items,
                current_session_id,
                scope,
                4,
            )),
        ))
    }

    #[must_use]
    pub fn selected_session(&self) -> Option<SessionPickerItem> {
        let OverlayKind::SessionPicker(picker) = &self.focused_overlay()?.kind else {
            return None;
        };
        picker.confirm()
    }

    pub fn confirm_session_picker(&mut self) -> Option<SessionPickerItem> {
        let id = self.focused_overlay;
        let selected = self.selected_session()?;
        if let Some(id) = id {
            let _ = self.close_overlay(id);
        }
        Some(selected)
    }

    /// Render the focused overlay as ANSI lines, if any.
    #[must_use]
    pub fn render_focused_overlay(&self, width: usize) -> Option<Vec<String>> {
        self.focused_overlay()?
            .render_standalone_lines(width, &self.theme)
    }

    pub fn open_model_picker(&mut self, items: impl IntoIterator<Item = PickerItem>) -> OverlayId {
        self.push_overlay(Overlay::new(
            "models",
            OverlayKind::ModelPicker(ModelPickerState::new(items)),
        ))
    }

    pub fn open_prompt_completion_picker(
        &mut self,
        prefix: PromptCompletionPrefix,
        items: impl IntoIterator<Item = PickerItem>,
    ) -> OverlayId {
        self.push_overlay(Overlay::new(
            "prompt-completion",
            OverlayKind::PromptCompletion(PromptCompletionState::new(prefix, items)),
        ))
    }

    #[must_use]
    pub fn selected_prompt_completion(&self) -> Option<PickerItem> {
        let OverlayKind::PromptCompletion(completions) = &self.focused_overlay()?.kind else {
            return None;
        };
        completions.selected_item()
    }

    #[must_use]
    pub fn selected_prompt_completion_with_prefix(
        &self,
    ) -> Option<(PromptCompletionPrefix, PickerItem)> {
        let OverlayKind::PromptCompletion(completions) = &self.focused_overlay()?.kind else {
            return None;
        };
        Some((completions.prefix().clone(), completions.selected_item()?))
    }

    pub fn confirm_prompt_completion(&mut self) -> Option<PickerItem> {
        let item = self.selected_prompt_completion()?;
        self.confirm_prompt_completion_with_replacement(&item.value)
    }

    pub fn confirm_prompt_completion_with_replacement(
        &mut self,
        replacement: &str,
    ) -> Option<PickerItem> {
        let id = self.focused_overlay;
        let (prefix, item) = {
            let OverlayKind::PromptCompletion(completions) = &self.focused_overlay()?.kind else {
                return None;
            };
            (completions.prefix().clone(), completions.confirm()?)
        };
        self.prompt
            .replace_completion_prefix(&prefix, replacement)?;
        if let Some(id) = id {
            let _ = self.close_overlay(id);
        }
        Some(item)
    }

    #[must_use]
    pub fn selected_model(&self) -> Option<PickerItem> {
        let OverlayKind::ModelPicker(picker) = &self.focused_overlay()?.kind else {
            return None;
        };
        picker.confirm()
    }

    pub fn confirm_model_picker(&mut self) -> Option<PickerItem> {
        let id = self.focused_overlay;
        let selected = self.selected_model()?;
        if let Some(id) = id {
            let _ = self.close_overlay(id);
        }
        Some(selected)
    }

    // -- Rich Neo dialog overlays ---------------------------------------

    pub fn open_tabbed_model_selector(
        &mut self,
        opts: crate::dialogs::TabbedModelSelectorOptions,
    ) -> OverlayId {
        let state = crate::dialogs::TabbedModelSelectorState::new(opts);
        self.push_overlay(Overlay::new(
            "models",
            OverlayKind::TabbedModelSelector(state),
        ))
    }

    pub fn open_provider_manager(
        &mut self,
        opts: &crate::dialogs::ProviderManagerOptions,
    ) -> OverlayId {
        // Try to find existing provider manager overlay and update in place
        let existing_id =
            self.find_overlay_by_kind(|kind| matches!(kind, OverlayKind::ProviderManager(_)));
        if let Some(id) = existing_id {
            if let Some(overlay) = self.overlays.iter_mut().find(|o| o.id == id)
                && let OverlayKind::ProviderManager(state) = &mut overlay.kind
            {
                state.set_options(opts);
            }
            self.focus_overlay(id);
            return id;
        }
        // No existing overlay — create new one
        let state = crate::dialogs::ProviderManagerState::new(opts);
        self.push_overlay(Overlay::new(
            "providers",
            OverlayKind::ProviderManager(state),
        ))
    }

    pub fn open_mcp_manager(&mut self, opts: &crate::dialogs::McpManagerOptions) -> OverlayId {
        // Try to find existing MCP manager overlay and update in place
        let existing_id =
            self.find_overlay_by_kind(|kind| matches!(kind, OverlayKind::McpManager(_)));
        if let Some(id) = existing_id {
            if let Some(overlay) = self.overlays.iter_mut().find(|o| o.id == id)
                && let OverlayKind::McpManager(state) = &mut overlay.kind
            {
                state.set_options(opts);
            }
            self.focus_overlay(id);
            return id;
        }
        // No existing overlay — create new one
        let state = crate::dialogs::McpManagerState::new(opts);
        self.push_overlay(Overlay::new("mcp", OverlayKind::McpManager(state)))
    }

    pub fn open_workspace_manager(
        &mut self,
        opts: &crate::dialogs::WorkspaceManagerOptions,
    ) -> OverlayId {
        let existing_id =
            self.find_overlay_by_kind(|kind| matches!(kind, OverlayKind::WorkspaceManager(_)));
        if let Some(id) = existing_id {
            if let Some(overlay) = self.overlays.iter_mut().find(|overlay| overlay.id == id)
                && let OverlayKind::WorkspaceManager(state) = &mut overlay.kind
            {
                state.set_options(opts);
            }
            self.focus_overlay(id);
            return id;
        }

        let state = crate::dialogs::WorkspaceManagerState::new(opts);
        self.push_overlay(Overlay::new(
            "workspace-access",
            OverlayKind::WorkspaceManager(state),
        ))
    }

    pub fn open_choice_picker(&mut self, opts: crate::dialogs::ChoicePickerOptions) -> OverlayId {
        let state = crate::dialogs::ChoicePickerState::new(opts);
        self.push_overlay(Overlay::new("choice", OverlayKind::ChoicePicker(state)))
    }

    pub fn open_confirm_dialog(&mut self, opts: crate::dialogs::ConfirmDialogOptions) -> OverlayId {
        let state = crate::dialogs::ConfirmDialogState::new(opts);
        self.push_overlay(Overlay::new("confirm", OverlayKind::ConfirmDialog(state)))
    }

    pub fn open_api_key_input(&mut self, opts: crate::dialogs::ApiKeyInputOptions) -> OverlayId {
        let state = crate::dialogs::ApiKeyInputState::new(opts, self.theme);
        self.push_overlay(Overlay::new("api-key", OverlayKind::ApiKeyInput(state)))
    }

    pub fn open_text_input(&mut self, opts: crate::dialogs::TextInputOptions) -> OverlayId {
        let state = crate::dialogs::TextInputState::new(opts, self.theme);
        self.push_overlay(Overlay::new("text-input", OverlayKind::TextInput(state)))
    }

    pub fn open_custom_registry_import(
        &mut self,
        opts: crate::dialogs::CustomRegistryImportOptions,
    ) -> OverlayId {
        let state = crate::dialogs::CustomRegistryImportState::new(opts, self.theme);
        self.push_overlay(Overlay::new(
            "registry",
            OverlayKind::CustomRegistryImport(state),
        ))
    }

    pub fn open_mcp_add_form(&mut self, opts: crate::dialogs::McpAddFormOptions) -> OverlayId {
        let state = crate::dialogs::McpAddFormState::new(opts, self.theme);
        self.push_overlay(Overlay::new("mcp-add", OverlayKind::McpAddForm(state)))
    }

    pub fn open_trust_dialog(&mut self, data: crate::dialogs::TrustDialogData) -> OverlayId {
        let state = crate::dialogs::TrustDialogState::new(data, self.theme);
        self.push_overlay(Overlay::new("trust", OverlayKind::TrustDialog(state)))
    }

    pub fn open_help_panel(
        &mut self,
        commands: Vec<crate::dialogs::HelpPanelCommand>,
    ) -> OverlayId {
        let state = crate::dialogs::HelpPanelState::new(crate::dialogs::HelpPanelOptions {
            commands,
            theme: self.theme,
        });
        self.push_overlay(Overlay::new("help", OverlayKind::HelpPanel(state)))
    }

    pub fn push_task_browser_overlay(&mut self, state: TaskBrowserState) -> OverlayId {
        if let Some(overlay) = self
            .overlays
            .iter_mut()
            .find(|overlay| matches!(overlay.kind, OverlayKind::TaskBrowser(_)))
        {
            overlay.kind = OverlayKind::TaskBrowser(state);
            let id = overlay.id;
            self.focus_overlay(id);
            return id;
        }
        self.push_overlay(Overlay::new("tasks", OverlayKind::TaskBrowser(state)))
    }

    #[must_use]
    pub fn take_trust_dialog_result(&mut self) -> Option<crate::dialogs::TrustDialogResult> {
        let id = self.focused_overlay?;
        let overlay = self.overlays.iter_mut().find(|overlay| overlay.id == id)?;
        let OverlayKind::TrustDialog(state) = &mut overlay.kind else {
            return None;
        };
        state.take_result()
    }

    /// Render the focused overlay (if any) into ANSI lines at the given width.
    #[must_use]
    pub fn focused_overlay_lines(&self, width: usize) -> Vec<String> {
        self.focused_overlay()
            .map_or_else(Vec::new, |overlay| overlay.render_lines(width, &self.theme))
    }

    #[must_use]
    pub fn render_focused_full_screen_overlay(
        &self,
        width: usize,
        height: usize,
    ) -> Option<Vec<String>> {
        self.focused_overlay()?
            .render_full_screen_lines(width, height, &self.theme)
    }

    /// Height in terminal lines the focused overlay wants to occupy.
    #[must_use]
    pub fn focused_overlay_height(&self) -> u16 {
        self.focused_overlay().map_or(0, Overlay::height)
    }

    /// Check if the focused overlay is one of the rich dialog types that
    /// handles its own keyboard input via `handle_input`.
    #[must_use]
    pub fn focused_overlay_is_rich_dialog(&self) -> bool {
        let Some(overlay) = self.focused_overlay() else {
            return false;
        };
        matches!(
            overlay.kind,
            OverlayKind::ModelSelector(_)
                | OverlayKind::TabbedModelSelector(_)
                | OverlayKind::ProviderManager(_)
                | OverlayKind::McpManager(_)
                | OverlayKind::WorkspaceManager(_)
                | OverlayKind::ConfirmDialog(_)
                | OverlayKind::McpAddForm(_)
                | OverlayKind::ChoicePicker(_)
                | OverlayKind::ApiKeyInput(_)
                | OverlayKind::TextInput(_)
                | OverlayKind::CustomRegistryImport(_)
                | OverlayKind::QuestionDialog(_)
                | OverlayKind::TrustDialog(_)
                | OverlayKind::HelpPanel(_)
                | OverlayKind::TaskBrowser(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_manager_overlay_is_rich_dialog() {
        let mut chrome = NeoChromeState::new("title", "session", "model", "/tmp");
        let id = chrome.open_workspace_manager(&crate::dialogs::WorkspaceManagerOptions {
            trusted: true,
            rows: Vec::new(),
            theme: crate::primitive::theme::TuiTheme::default(),
        });

        assert_eq!(chrome.focused_overlay_id(), Some(id));
        assert!(chrome.focused_overlay_is_rich_dialog());
    }

    #[test]
    fn workspace_manager_overlay_blocks_prompt_and_renders_empty_state() {
        let mut chrome = NeoChromeState::new("title", "session", "model", "/tmp");
        chrome.open_workspace_manager(&crate::dialogs::WorkspaceManagerOptions {
            trusted: true,
            rows: Vec::new(),
            theme: crate::primitive::theme::TuiTheme::default(),
        });

        assert!(chrome.focused_overlay_blocks_prompt());
        let visible = chrome
            .focused_overlay_lines(80)
            .into_iter()
            .map(|line| crate::primitive::strip_ansi(&line))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            visible.contains("No additional workspaces configured."),
            "{visible}"
        );
        assert!(visible.contains("+ Add workspace directory"), "{visible}");
    }
}
