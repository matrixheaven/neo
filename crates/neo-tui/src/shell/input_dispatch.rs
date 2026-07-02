use crate::dialogs::{QuestionDialogAction, QuestionResult};
use crate::input::{InputEvent, KeybindingAction};
use crate::primitive::InputResult;
use crate::primitive::theme::ChromeMode;

use super::approval::{self, ApprovalResult};
use super::overlay::{Overlay, OverlayKind};
use super::state::NeoChromeState;

impl NeoChromeState {
    /// Forward an input event to the focused rich dialog overlay.
    pub fn handle_focused_dialog_input(&mut self, input: InputEvent) -> InputResult {
        let Some(id) = self.focused_overlay else {
            return InputResult::Ignored;
        };
        let input = Self::translate_key_event_for_dialog(input);
        let Some(overlay) = self.overlays.iter_mut().find(|o| o.id == id) else {
            return InputResult::Ignored;
        };
        let mut close_overlay = false;
        let result = match &mut overlay.kind {
            OverlayKind::ModelSelector(state) => state.handle_input(&input),
            OverlayKind::TabbedModelSelector(state) => state.handle_input(&input),
            OverlayKind::ProviderManager(state) => state.handle_input(&input),
            OverlayKind::McpManager(state) => state.handle_input(&input),
            OverlayKind::ChoicePicker(state) => state.handle_input(&input),
            OverlayKind::ApiKeyInput(state) => state.handle_input(&input),
            OverlayKind::TextInput(state) => state.handle_input(&input),
            OverlayKind::CustomRegistryImport(state) => state.handle_input(input),
            OverlayKind::McpAddForm(state) => state.handle_input(input),
            OverlayKind::QuestionDialog(state) => {
                let result = state.handle_input(&input);
                if matches!(result, InputResult::Submitted | InputResult::Cancelled) {
                    if result == InputResult::Submitted {
                        self.pending_question_result = Some(QuestionResult {
                            id: state.id.clone(),
                            answers: state.compile_answers(),
                        });
                    }
                    close_overlay = true;
                }
                result
            }
            OverlayKind::TrustDialog(state) => state.handle_input(&input),
            OverlayKind::HelpPanel(state) => {
                let result = state.handle_input(&input);
                if matches!(result, InputResult::Submitted | InputResult::Cancelled) {
                    close_overlay = true;
                }
                result
            }
            _ => InputResult::Ignored,
        };
        if close_overlay {
            let _ = self.close_overlay(id);
        }
        result
    }

    /// Convenience result accessors for rich dialogs
    fn translate_key_event_for_dialog(input: InputEvent) -> InputEvent {
        match &input {
            InputEvent::Key(key) => match key.as_str() {
                "enter" => InputEvent::Submit,
                "escape" => InputEvent::Cancel,
                "up" => InputEvent::Action(KeybindingAction::SelectUp),
                "down" => InputEvent::Action(KeybindingAction::SelectDown),
                "pageup" => InputEvent::Action(KeybindingAction::SelectPageUp),
                "pagedown" => InputEvent::Action(KeybindingAction::SelectPageDown),
                "tab" => InputEvent::Insert('\t'),
                "backspace" => InputEvent::Backspace,
                "delete" => InputEvent::Delete,
                "left" => InputEvent::MoveLeft,
                "right" => InputEvent::MoveRight,
                "home" => InputEvent::MoveHome,
                "end" => InputEvent::MoveEnd,
                _ => input,
            },
            // The keybinding layer (see `keybinding_priority` /
            // `OVERLAY_ACTION_PRIORITY`) rewrites `enter`→`SelectConfirm` and
            // `escape`→`SelectCancel` before the dialog sees them. Normalize
            // those back to `Submit`/`Cancel` so text-input dialogs (API key,
            // custom registry import) that match on `Submit`/`Cancel` keep
            // working. Picker-style dialogs handle these actions directly too,
            // so they are unaffected.
            InputEvent::Action(KeybindingAction::SelectConfirm) => InputEvent::Submit,
            InputEvent::Action(KeybindingAction::SelectCancel) => InputEvent::Cancel,
            _ => input,
        }
    }

    #[must_use]
    pub fn model_selector_result(&self) -> Option<&crate::dialogs::ModelSelectorResult> {
        let OverlayKind::ModelSelector(state) = &self.focused_overlay()?.kind else {
            return None;
        };
        state.result()
    }

    #[must_use]
    pub fn tabbed_model_selector_result(&self) -> Option<&crate::dialogs::ModelSelectorResult> {
        let OverlayKind::TabbedModelSelector(state) = &self.focused_overlay()?.kind else {
            return None;
        };
        state.result()
    }

    #[must_use]
    pub fn provider_manager_action(&self) -> Option<crate::dialogs::ProviderManagerAction> {
        let OverlayKind::ProviderManager(state) = &self.focused_overlay()?.kind else {
            return None;
        };
        state.action()
    }

    #[must_use]
    pub fn mcp_manager_action(&self) -> Option<crate::dialogs::McpManagerAction> {
        let OverlayKind::McpManager(state) = &self.focused_overlay()?.kind else {
            return None;
        };
        state.action()
    }

    pub fn take_mcp_manager_action(&mut self) -> Option<crate::dialogs::McpManagerAction> {
        let OverlayKind::McpManager(state) = &mut self.focused_overlay_mut()?.kind else {
            return None;
        };
        state.take_action()
    }

    #[must_use]
    pub fn choice_picker_result(&self) -> Option<&crate::dialogs::ChoiceResult> {
        let OverlayKind::ChoicePicker(state) = &self.focused_overlay()?.kind else {
            return None;
        };
        state.result()
    }

    #[must_use]
    pub fn api_key_input_result(&self) -> Option<&crate::dialogs::ApiKeyInputResult> {
        let OverlayKind::ApiKeyInput(state) = &self.focused_overlay()?.kind else {
            return None;
        };
        state.result()
    }

    #[must_use]
    pub fn text_input_result(&self) -> Option<&crate::dialogs::TextInputResult> {
        let OverlayKind::TextInput(state) = &self.focused_overlay()?.kind else {
            return None;
        };
        state.result()
    }

    #[must_use]
    pub fn custom_registry_import_result(
        &self,
    ) -> Option<&crate::dialogs::CustomRegistryImportResult> {
        let OverlayKind::CustomRegistryImport(state) = &self.focused_overlay()?.kind else {
            return None;
        };
        state.result()
    }

    #[must_use]
    pub fn mcp_add_form_result(&self) -> Option<&crate::dialogs::McpAddFormResult> {
        let OverlayKind::McpAddForm(state) = &self.focused_overlay()?.kind else {
            return None;
        };
        state.result()
    }

    pub fn take_question_result(&mut self) -> Option<QuestionResult> {
        self.pending_question_result.take()
    }

    /// Process a crossterm key event in the question dialog.
    /// Returns the action produced (None if no question dialog is focused).
    #[must_use]
    pub fn handle_question_dialog_key(
        &mut self,
        event: crossterm::event::KeyEvent,
    ) -> Option<QuestionDialogAction> {
        let id = self.focused_overlay?;
        let action = {
            let overlay = self.overlays.iter_mut().find(|o| o.id == id)?;
            let OverlayKind::QuestionDialog(state) = &mut overlay.kind else {
                return None;
            };
            state.handle_key(event)
        };
        if matches!(
            action,
            QuestionDialogAction::Submit(_) | QuestionDialogAction::Cancel
        ) {
            self.close_overlay(id);
        }
        Some(action)
    }

    /// Confirm / submit the question dialog. Returns answers if all questions
    /// were answered.
    pub fn confirm_question(&mut self) -> Option<QuestionResult> {
        let id = self.focused_overlay?;
        let result = {
            let overlay = self.focused_overlay()?;
            let OverlayKind::QuestionDialog(state) = &overlay.kind else {
                return None;
            };
            if !state.is_complete() {
                return None;
            }
            QuestionResult {
                id: state.id.clone(),
                answers: state.compile_answers(),
            }
        };
        self.close_overlay(id);
        Some(result)
    }

    /// Cancel the question dialog. Returns the question id.
    pub fn cancel_question(&mut self) -> Option<String> {
        let id = self.focused_overlay?;
        let question_id = {
            let overlay = self.focused_overlay()?;
            let OverlayKind::QuestionDialog(state) = &overlay.kind else {
                return None;
            };
            state.id.clone()
        };
        self.close_overlay(id);
        Some(question_id)
    }

    pub fn move_overlay_selection_down(&mut self) {
        if let Some(approval) = self.pending_approvals.front_mut() {
            approval.move_down();
            self.mode = ChromeMode::Approval;
            return;
        }
        self.with_focused_overlay_mut(Overlay::move_selection_down);
    }

    pub fn move_overlay_selection_up(&mut self) {
        if let Some(approval) = self.pending_approvals.front_mut() {
            approval.move_up();
            self.mode = ChromeMode::Approval;
            return;
        }
        self.with_focused_overlay_mut(Overlay::move_selection_up);
    }

    pub fn handle_pending_approval_input(&mut self, input: InputEvent) -> Option<ApprovalResult> {
        let input = Self::translate_key_event_for_dialog(input);
        match input {
            InputEvent::Insert(character) => {
                if let Some(number) = approval::approval_number(character)
                    && let Some(result) = self.choose_approval_number(number)
                {
                    return Some(result);
                }
                if let Some(approval) = self.pending_approvals.front_mut() {
                    approval.insert_feedback(&character.to_string());
                }
                None
            }
            InputEvent::Paste(text) => {
                if let Some(approval) = self.pending_approvals.front_mut() {
                    approval.insert_feedback(&text);
                }
                None
            }
            InputEvent::Backspace | InputEvent::Delete => {
                if let Some(approval) = self.pending_approvals.front_mut() {
                    approval.backspace_feedback();
                }
                None
            }
            InputEvent::Action(KeybindingAction::SelectDown) => {
                self.move_overlay_selection_down();
                None
            }
            InputEvent::Action(KeybindingAction::SelectUp) => {
                self.move_overlay_selection_up();
                None
            }
            InputEvent::Submit
            | InputEvent::Action(KeybindingAction::SelectConfirm | KeybindingAction::InputSubmit) => {
                self.confirm_approval()
            }
            InputEvent::Cancel | InputEvent::Action(KeybindingAction::SelectCancel) => {
                self.deny_approval()
            }
            _ => None,
        }
    }

    pub fn move_overlay_selection_page_down(&mut self) {
        self.with_focused_overlay_mut(Overlay::move_selection_page_down);
    }

    pub fn move_overlay_selection_page_up(&mut self) {
        self.with_focused_overlay_mut(Overlay::move_selection_page_up);
    }

    fn with_focused_overlay_mut(&mut self, action: impl FnOnce(&mut Overlay)) {
        let Some(id) = self.focused_overlay else {
            return;
        };
        if let Some(overlay) = self.overlays.iter_mut().find(|overlay| overlay.id == id) {
            action(overlay);
        }
    }
}
