use super::overlay::OverlayKind;

use crate::dialogs::{
    ApiKeyInputState, ChoicePickerState, ConfirmDialogState, CustomEndpointWizardState,
    CustomRegistryImportState, McpAddFormState, McpManagerState, ModelSelectorState,
    ProviderManagerState, TabbedModelSelectorState, TextInputState, WorkspaceManagerState,
};
use crate::input::{InputEvent, KeybindingAction};

pub(super) fn handle_dialog_selection(kind: &mut OverlayKind, action: KeybindingAction) {
    let input = InputEvent::Action(action);
    if handle_selector_dialog_selection(kind, &input) {
        return;
    }
    handle_input_dialog_selection(kind, input);
}

fn handle_selector_dialog_selection(kind: &mut OverlayKind, input: &InputEvent) -> bool {
    if handle_model_dialog_selection(kind, input) {
        return true;
    }
    handle_provider_choice_dialog_selection(kind, input)
}

fn handle_model_dialog_selection(kind: &mut OverlayKind, input: &InputEvent) -> bool {
    match kind {
        OverlayKind::ModelSelector(state) => handle_input_ref(state, input),
        OverlayKind::TabbedModelSelector(state) => handle_input_ref(state, input),
        _ => return false,
    }
    true
}

fn handle_provider_choice_dialog_selection(kind: &mut OverlayKind, input: &InputEvent) -> bool {
    match kind {
        OverlayKind::ProviderManager(state) => handle_input_ref(state, input),
        OverlayKind::McpManager(state) => handle_input_ref(state, input),
        OverlayKind::WorkspaceManager(state) => handle_input_ref(state, input),
        OverlayKind::ChoicePicker(state) => handle_input_ref(state, input),
        _ => return false,
    }
    true
}

fn handle_input_dialog_selection(kind: &mut OverlayKind, input: InputEvent) {
    match kind {
        OverlayKind::ApiKeyInput(state) => handle_input_ref(state, &input),
        OverlayKind::ConfirmDialog(state) => handle_input_ref(state, &input),
        OverlayKind::CustomEndpointWizard(state) => handle_input_ref(state, &input),
        OverlayKind::CustomRegistryImport(state) => handle_input_owned(state, input),
        OverlayKind::McpAddForm(state) => handle_input_owned(state, input),
        _ => {}
    }
}

fn handle_input_ref<T: DialogInputRef>(state: &mut T, input: &InputEvent) {
    state.handle_dialog_input(input);
}

fn handle_input_owned<T: DialogInputOwned>(state: &mut T, input: InputEvent) {
    state.handle_dialog_input(input);
}

trait DialogInputRef {
    fn handle_dialog_input(&mut self, input: &InputEvent);
}

trait DialogInputOwned {
    fn handle_dialog_input(&mut self, input: InputEvent);
}

impl DialogInputRef for ModelSelectorState {
    fn handle_dialog_input(&mut self, input: &InputEvent) {
        let _ = self.handle_input(input);
    }
}

impl DialogInputRef for TabbedModelSelectorState {
    fn handle_dialog_input(&mut self, input: &InputEvent) {
        let _ = self.handle_input(input);
    }
}

impl DialogInputRef for ProviderManagerState {
    fn handle_dialog_input(&mut self, input: &InputEvent) {
        let _ = self.handle_input(input);
    }
}

impl DialogInputRef for McpManagerState {
    fn handle_dialog_input(&mut self, input: &InputEvent) {
        let _ = self.handle_input(input);
    }
}

impl DialogInputRef for WorkspaceManagerState {
    fn handle_dialog_input(&mut self, input: &InputEvent) {
        let _ = self.handle_input(input);
    }
}

impl DialogInputRef for ChoicePickerState {
    fn handle_dialog_input(&mut self, input: &InputEvent) {
        let _ = self.handle_input(input);
    }
}

impl DialogInputRef for ApiKeyInputState {
    fn handle_dialog_input(&mut self, input: &InputEvent) {
        let _ = self.handle_input(input);
    }
}

impl DialogInputRef for ConfirmDialogState {
    fn handle_dialog_input(&mut self, input: &InputEvent) {
        let _ = self.handle_input(input);
    }
}

impl DialogInputRef for TextInputState {
    fn handle_dialog_input(&mut self, input: &InputEvent) {
        let _ = self.handle_input(input);
    }
}

impl DialogInputRef for CustomEndpointWizardState {
    fn handle_dialog_input(&mut self, input: &InputEvent) {
        let _ = self.handle_input(input);
    }
}

impl DialogInputOwned for CustomRegistryImportState {
    fn handle_dialog_input(&mut self, input: InputEvent) {
        let _ = self.handle_input(input);
    }
}

impl DialogInputOwned for McpAddFormState {
    fn handle_dialog_input(&mut self, input: InputEvent) {
        let _ = self.handle_input(input);
    }
}
