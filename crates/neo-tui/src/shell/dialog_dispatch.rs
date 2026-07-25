use super::overlay::OverlayKind;

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
        OverlayKind::ModelSelector(state) => {
            let _ = state.handle_input(input);
        }
        OverlayKind::TabbedModelSelector(state) => {
            let _ = state.handle_input(input);
        }
        _ => return false,
    }
    true
}

fn handle_provider_choice_dialog_selection(kind: &mut OverlayKind, input: &InputEvent) -> bool {
    match kind {
        OverlayKind::ProviderManager(state) => {
            let _ = state.handle_input(input);
        }
        OverlayKind::McpManager(state) => {
            let _ = state.handle_input(input);
        }
        OverlayKind::WorkspaceManager(state) => {
            let _ = state.handle_input(input);
        }
        OverlayKind::ChoicePicker(state) => {
            let _ = state.handle_input(input);
        }
        _ => return false,
    }
    true
}

fn handle_input_dialog_selection(kind: &mut OverlayKind, input: InputEvent) {
    match kind {
        OverlayKind::ApiKeyInput(state) => {
            let _ = state.handle_input(&input);
        }
        OverlayKind::ConfirmDialog(state) => {
            let _ = state.handle_input(&input);
        }
        OverlayKind::CustomEndpointWizard(state) => {
            let _ = state.handle_input(&input);
        }
        OverlayKind::CustomRegistryImport(state) => {
            let _ = state.handle_input(input);
        }
        OverlayKind::McpAddForm(state) => {
            let _ = state.handle_input(input);
        }
        _ => {}
    }
}
