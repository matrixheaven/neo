use super::*;

#[test]
fn keybinding_action_ids_round_trip() {
    let actions = [
        KeybindingAction::EditorCursorUp,
        KeybindingAction::EditorCursorDown,
        KeybindingAction::EditorCursorLeft,
        KeybindingAction::EditorCursorRight,
        KeybindingAction::EditorCursorWordLeft,
        KeybindingAction::EditorCursorWordRight,
        KeybindingAction::EditorCursorLineStart,
        KeybindingAction::EditorCursorLineEnd,
        KeybindingAction::EditorPageUp,
        KeybindingAction::EditorPageDown,
        KeybindingAction::EditorDeleteCharBackward,
        KeybindingAction::EditorDeleteCharForward,
        KeybindingAction::EditorDeleteWordBackward,
        KeybindingAction::EditorDeleteWordForward,
        KeybindingAction::EditorDeleteToLineStart,
        KeybindingAction::EditorDeleteToLineEnd,
        KeybindingAction::EditorYank,
        KeybindingAction::EditorUndo,
        KeybindingAction::InputNewLine,
        KeybindingAction::InputSubmit,
        KeybindingAction::InputTab,
        KeybindingAction::InputCopy,
        KeybindingAction::TranscriptSelectionClear,
        KeybindingAction::TranscriptSelectionExtendUp,
        KeybindingAction::TranscriptSelectionExtendDown,
        KeybindingAction::TranscriptSelectionExtendPageUp,
        KeybindingAction::TranscriptSelectionExtendPageDown,
        KeybindingAction::TranscriptCopySelection,
        KeybindingAction::ToolOutputToggle,
        KeybindingAction::AppClear,
        KeybindingAction::AppExit,
        KeybindingAction::AppSuspend,
        KeybindingAction::PromptCompletionToggle,
        KeybindingAction::CommandPaletteOpen,
        KeybindingAction::SessionPickerOpen,
        KeybindingAction::SessionPickerToggleScope,
        KeybindingAction::SessionFork,
        KeybindingAction::ModelPickerOpen,
        KeybindingAction::TogglePlanMode,
        KeybindingAction::CycleDevelopmentMode,
        KeybindingAction::SelectUp,
        KeybindingAction::SelectDown,
        KeybindingAction::SelectPageUp,
        KeybindingAction::SelectPageDown,
        KeybindingAction::SelectConfirm,
        KeybindingAction::SelectCancel,
    ];

    for action in actions {
        assert_eq!(KeybindingAction::from_id(action.id()), Some(action));
    }
    assert_eq!(KeybindingAction::from_id("tui.unknown"), None);
}
