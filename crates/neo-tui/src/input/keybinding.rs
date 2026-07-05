use std::collections::{BTreeMap, BTreeSet};

use super::key_id::KeyId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KeybindingAction {
    EditorCursorUp,
    EditorCursorDown,
    EditorCursorLeft,
    EditorCursorRight,
    EditorCursorWordLeft,
    EditorCursorWordRight,
    EditorCursorLineStart,
    EditorCursorLineEnd,
    EditorPageUp,
    EditorPageDown,
    EditorDeleteCharBackward,
    EditorDeleteCharForward,
    EditorDeleteWordBackward,
    EditorDeleteWordForward,
    EditorDeleteToLineStart,
    EditorDeleteToLineEnd,
    EditorYank,
    EditorUndo,
    InputNewLine,
    InputSubmit,
    InputTab,
    InputCopy,
    /// Steer the running turn with the current composer text at the next
    /// natural break point, or queue a follow-up if no turn is active.
    /// Default: Ctrl+S (requires `stty -ixon` to disable XON/XOFF flow control
    /// in terminals that swallow Ctrl+S by default).
    PromptSteer,
    /// Dequeue the next queued follow-up back into the composer for editing.
    /// Default: Alt+Up.
    EditNextQueuedMessage,
    TranscriptSelectionStart,
    TranscriptSelectionClear,
    TranscriptSelectionExtendUp,
    TranscriptSelectionExtendDown,
    TranscriptSelectionExtendPageUp,
    TranscriptSelectionExtendPageDown,
    TranscriptCopySelection,
    ToolOutputToggle,
    TodoPanelToggle,
    PasteImage,
    AppClear,
    AppExit,
    AppSuspend,
    PromptCompletionToggle,
    CommandPaletteOpen,
    SessionPickerOpen,
    SessionPickerToggleScope,
    SessionFork,
    ModelPickerOpen,
    TogglePlanMode,
    CycleDevelopmentMode,
    SelectUp,
    SelectDown,
    SelectPageUp,
    SelectPageDown,
    SelectConfirm,
    SelectCancel,
}

const KEYBINDING_ACTION_IDS: &[(KeybindingAction, &str)] = &[
    (KeybindingAction::EditorCursorUp, "tui.editor.cursorUp"),
    (KeybindingAction::EditorCursorDown, "tui.editor.cursorDown"),
    (KeybindingAction::EditorCursorLeft, "tui.editor.cursorLeft"),
    (
        KeybindingAction::EditorCursorRight,
        "tui.editor.cursorRight",
    ),
    (
        KeybindingAction::EditorCursorWordLeft,
        "tui.editor.cursorWordLeft",
    ),
    (
        KeybindingAction::EditorCursorWordRight,
        "tui.editor.cursorWordRight",
    ),
    (
        KeybindingAction::EditorCursorLineStart,
        "tui.editor.cursorLineStart",
    ),
    (
        KeybindingAction::EditorCursorLineEnd,
        "tui.editor.cursorLineEnd",
    ),
    (KeybindingAction::EditorPageUp, "tui.editor.pageUp"),
    (KeybindingAction::EditorPageDown, "tui.editor.pageDown"),
    (
        KeybindingAction::EditorDeleteCharBackward,
        "tui.editor.deleteCharBackward",
    ),
    (
        KeybindingAction::EditorDeleteCharForward,
        "tui.editor.deleteCharForward",
    ),
    (
        KeybindingAction::EditorDeleteWordBackward,
        "tui.editor.deleteWordBackward",
    ),
    (
        KeybindingAction::EditorDeleteWordForward,
        "tui.editor.deleteWordForward",
    ),
    (
        KeybindingAction::EditorDeleteToLineStart,
        "tui.editor.deleteToLineStart",
    ),
    (
        KeybindingAction::EditorDeleteToLineEnd,
        "tui.editor.deleteToLineEnd",
    ),
    (KeybindingAction::EditorYank, "tui.editor.yank"),
    (KeybindingAction::EditorUndo, "tui.editor.undo"),
    (KeybindingAction::InputNewLine, "tui.input.newLine"),
    (KeybindingAction::InputSubmit, "tui.input.submit"),
    (KeybindingAction::InputTab, "tui.input.tab"),
    (KeybindingAction::InputCopy, "tui.input.copy"),
    (KeybindingAction::PromptSteer, "tui.input.steer"),
    (
        KeybindingAction::EditNextQueuedMessage,
        "tui.input.editNextQueuedMessage",
    ),
    (
        KeybindingAction::TranscriptSelectionStart,
        "tui.transcript.selection.start",
    ),
    (
        KeybindingAction::TranscriptSelectionClear,
        "tui.transcript.selection.clear",
    ),
    (
        KeybindingAction::TranscriptSelectionExtendUp,
        "tui.transcript.selection.extendUp",
    ),
    (
        KeybindingAction::TranscriptSelectionExtendDown,
        "tui.transcript.selection.extendDown",
    ),
    (
        KeybindingAction::TranscriptSelectionExtendPageUp,
        "tui.transcript.selection.extendPageUp",
    ),
    (
        KeybindingAction::TranscriptSelectionExtendPageDown,
        "tui.transcript.selection.extendPageDown",
    ),
    (
        KeybindingAction::TranscriptCopySelection,
        "tui.transcript.copySelection",
    ),
    (KeybindingAction::ToolOutputToggle, "tui.tool.toggleOutput"),
    (KeybindingAction::TodoPanelToggle, "tui.todo.toggle"),
    (KeybindingAction::PasteImage, "tui.input.pasteImage"),
    (KeybindingAction::AppClear, "app.clear"),
    (KeybindingAction::AppExit, "app.exit"),
    (KeybindingAction::AppSuspend, "app.suspend"),
    (
        KeybindingAction::PromptCompletionToggle,
        "tui.promptCompletion.toggle",
    ),
    (KeybindingAction::CommandPaletteOpen, "tui.command.open"),
    (KeybindingAction::SessionPickerOpen, "tui.session.open"),
    (
        KeybindingAction::SessionPickerToggleScope,
        "tui.session.toggle_scope",
    ),
    (KeybindingAction::SessionFork, "tui.session.fork"),
    (KeybindingAction::ModelPickerOpen, "tui.model.open"),
    (KeybindingAction::TogglePlanMode, "tui.plan.toggle"),
    (
        KeybindingAction::CycleDevelopmentMode,
        "tui.developmentMode.cycle",
    ),
    (KeybindingAction::SelectUp, "tui.select.up"),
    (KeybindingAction::SelectDown, "tui.select.down"),
    (KeybindingAction::SelectPageUp, "tui.select.pageUp"),
    (KeybindingAction::SelectPageDown, "tui.select.pageDown"),
    (KeybindingAction::SelectConfirm, "tui.select.confirm"),
    (KeybindingAction::SelectCancel, "tui.select.cancel"),
];

impl KeybindingAction {
    #[must_use]
    pub fn id(self) -> &'static str {
        KEYBINDING_ACTION_IDS
            .iter()
            .find_map(|(action, id)| (*action == self).then_some(*id))
            .expect("keybinding action id table must cover every action")
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        KEYBINDING_ACTION_IDS
            .iter()
            .find_map(|(action, action_id)| (*action_id == id).then_some(*action))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingDefinition {
    pub action: KeybindingAction,
    pub default_keys: Vec<KeyId>,
    pub description: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingConflict {
    pub key: KeyId,
    pub actions: Vec<KeybindingAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingsManager {
    definitions: BTreeMap<KeybindingAction, KeybindingDefinition>,
    user_bindings: BTreeMap<KeybindingAction, Vec<KeyId>>,
    resolved: BTreeMap<KeybindingAction, Vec<KeyId>>,
    conflicts: Vec<KeybindingConflict>,
}

impl Default for KeybindingsManager {
    fn default() -> Self {
        Self::new(default_keybinding_definitions(), [])
    }
}

impl KeybindingsManager {
    #[must_use]
    pub fn new(
        definitions: impl IntoIterator<Item = KeybindingDefinition>,
        user_bindings: impl IntoIterator<Item = (KeybindingAction, Vec<KeyId>)>,
    ) -> Self {
        let definitions = definitions
            .into_iter()
            .map(|definition| (definition.action, definition))
            .collect();
        let mut manager = Self {
            definitions,
            user_bindings: user_bindings.into_iter().collect(),
            resolved: BTreeMap::new(),
            conflicts: Vec::new(),
        };
        manager.rebuild();
        manager
    }

    #[must_use]
    pub fn matches(&self, key: &KeyId, action: KeybindingAction) -> bool {
        self.resolved
            .get(&action)
            .is_some_and(|keys| keys.iter().any(|candidate| candidate == key))
    }

    #[must_use]
    pub fn matching_actions(&self, key: &KeyId) -> Vec<KeybindingAction> {
        self.resolved
            .iter()
            .filter_map(|(action, keys)| {
                keys.iter()
                    .any(|candidate| candidate == key)
                    .then_some(*action)
            })
            .collect()
    }

    #[must_use]
    pub fn keys(&self, action: KeybindingAction) -> Vec<KeyId> {
        self.resolved.get(&action).cloned().unwrap_or_default()
    }

    #[must_use]
    pub fn conflicts(&self) -> Vec<KeybindingConflict> {
        self.conflicts.clone()
    }

    pub fn set_user_bindings(
        &mut self,
        bindings: impl IntoIterator<Item = (KeybindingAction, Vec<KeyId>)>,
    ) {
        self.user_bindings = bindings.into_iter().collect();
        self.rebuild();
    }

    fn rebuild(&mut self) {
        self.resolved.clear();
        self.conflicts.clear();

        let mut user_claims: BTreeMap<KeyId, BTreeSet<KeybindingAction>> = BTreeMap::new();
        for (action, keys) in &self.user_bindings {
            if !self.definitions.contains_key(action) {
                continue;
            }
            for key in unique_keys(keys) {
                user_claims.entry(key).or_default().insert(*action);
            }
        }

        self.conflicts = user_claims
            .into_iter()
            .filter_map(|(key, actions)| {
                if actions.len() > 1 {
                    Some(KeybindingConflict {
                        key,
                        actions: actions.into_iter().collect(),
                    })
                } else {
                    None
                }
            })
            .collect();

        for (action, definition) in &self.definitions {
            let keys = self
                .user_bindings
                .get(action)
                .map_or_else(|| definition.default_keys.clone(), |keys| unique_keys(keys));
            self.resolved.insert(*action, keys);
        }
    }
}

fn default_keybinding_definitions() -> Vec<KeybindingDefinition> {
    let mut definitions = Vec::new();
    definitions.extend(editor_keybinding_definitions());
    definitions.extend(input_keybinding_definitions());
    definitions.extend(transcript_keybinding_definitions());
    definitions.extend(app_keybinding_definitions());
    definitions.extend(picker_keybinding_definitions());
    definitions
}

fn editor_keybinding_definitions() -> Vec<KeybindingDefinition> {
    use KeybindingAction as Action;

    vec![
        definition(Action::EditorCursorUp, &["up"], "Move cursor up"),
        definition(Action::EditorCursorDown, &["down"], "Move cursor down"),
        definition(
            Action::EditorCursorLeft,
            &["left", "ctrl+b"],
            "Move cursor left",
        ),
        definition(
            Action::EditorCursorRight,
            &["right", "ctrl+f"],
            "Move cursor right",
        ),
        definition(
            Action::EditorCursorWordLeft,
            &["alt+left", "ctrl+left", "alt+b"],
            "Move cursor word left",
        ),
        definition(
            Action::EditorCursorWordRight,
            &["alt+right", "ctrl+right", "alt+f"],
            "Move cursor word right",
        ),
        definition(
            Action::EditorCursorLineStart,
            &["home", "ctrl+a"],
            "Move to line start",
        ),
        definition(
            Action::EditorCursorLineEnd,
            &["end", "ctrl+e"],
            "Move to line end",
        ),
        definition(Action::EditorPageUp, &["pageup"], "Page up"),
        definition(Action::EditorPageDown, &["pagedown"], "Page down"),
        definition(
            Action::EditorDeleteCharBackward,
            &["backspace"],
            "Delete character backward",
        ),
        definition(
            Action::EditorDeleteCharForward,
            &["delete", "ctrl+d"],
            "Delete character forward",
        ),
        definition(
            Action::EditorDeleteWordBackward,
            &["ctrl+w", "alt+backspace"],
            "Delete word backward",
        ),
        definition(
            Action::EditorDeleteWordForward,
            &["alt+d", "alt+delete"],
            "Delete word forward",
        ),
        definition(
            Action::EditorDeleteToLineStart,
            &["ctrl+u"],
            "Delete to line start",
        ),
        definition(
            Action::EditorDeleteToLineEnd,
            &["ctrl+k"],
            "Delete to line end",
        ),
        definition(Action::EditorYank, &["ctrl+y"], "Yank"),
        definition(Action::EditorUndo, &["ctrl+-", "ctrl+_"], "Undo"),
    ]
}

fn input_keybinding_definitions() -> Vec<KeybindingDefinition> {
    use KeybindingAction as Action;

    vec![
        definition(
            Action::InputNewLine,
            &["alt+enter", "ctrl+enter", "ctrl+j"],
            "Insert newline",
        ),
        definition(Action::InputSubmit, &["enter"], "Submit input"),
        definition(Action::InputTab, &["tab"], "Tab"),
        definition(Action::InputCopy, &["ctrl+c"], "Copy selection"),
        definition(
            Action::PromptSteer,
            &["ctrl+s"],
            "Steer the running turn or queue a follow-up",
        ),
        definition(
            Action::EditNextQueuedMessage,
            &["alt+up"],
            "Edit the next queued follow-up message",
        ),
        definition(
            Action::CycleDevelopmentMode,
            &["shift+tab"],
            "Cycle normal/plan/goal mode",
        ),
        definition(
            Action::TodoPanelToggle,
            &["ctrl+t"],
            "Expand or collapse the todo panel",
        ),
        #[cfg(target_os = "windows")]
        definition(Action::PasteImage, &["alt+v"], "Paste image from clipboard"),
        #[cfg(not(target_os = "windows"))]
        definition(
            Action::PasteImage,
            &["ctrl+v"],
            "Paste image from clipboard",
        ),
    ]
}

fn transcript_keybinding_definitions() -> Vec<KeybindingDefinition> {
    use KeybindingAction as Action;

    vec![
        definition(
            Action::TranscriptSelectionStart,
            &["ctrl+space"],
            "Select transcript item",
        ),
        definition(
            Action::TranscriptSelectionClear,
            &["ctrl+shift+space"],
            "Clear transcript selection",
        ),
        definition(
            Action::TranscriptSelectionExtendUp,
            &["shift+up"],
            "Extend transcript selection up",
        ),
        definition(
            Action::TranscriptSelectionExtendDown,
            &["shift+down"],
            "Extend transcript selection down",
        ),
        definition(
            Action::TranscriptSelectionExtendPageUp,
            &["shift+pageup"],
            "Extend transcript selection page up",
        ),
        definition(
            Action::TranscriptSelectionExtendPageDown,
            &["shift+pagedown"],
            "Extend transcript selection page down",
        ),
        definition(
            Action::TranscriptCopySelection,
            &["ctrl+c"],
            "Copy transcript selection",
        ),
        definition(Action::ToolOutputToggle, &["ctrl+o"], "Toggle tool output"),
    ]
}

fn app_keybinding_definitions() -> Vec<KeybindingDefinition> {
    use KeybindingAction as Action;

    vec![
        definition(Action::AppClear, &["ctrl+c"], "Clear editor"),
        definition(Action::AppExit, &["ctrl+d"], "Exit when prompt is empty"),
        definition(Action::AppSuspend, &["ctrl+z"], "Suspend to background"),
    ]
}

fn picker_keybinding_definitions() -> Vec<KeybindingDefinition> {
    use KeybindingAction as Action;

    vec![
        definition(
            Action::PromptCompletionToggle,
            &["ctrl+p"],
            "Toggle prompt completion",
        ),
        definition(Action::CommandPaletteOpen, &[], "Open command palette"),
        definition(Action::SessionPickerOpen, &["ctrl+r"], "Open sessions"),
        definition(
            Action::SessionPickerToggleScope,
            &["ctrl+a"],
            "Toggle session scope",
        ),
        definition(Action::SessionFork, &["ctrl+n"], "Fork selected session"),
        definition(Action::ModelPickerOpen, &[], "Open models"),
        definition(Action::TogglePlanMode, &[], "Toggle plan mode"),
        definition(Action::SelectUp, &["up"], "Move selection up"),
        definition(Action::SelectDown, &["down"], "Move selection down"),
        definition(Action::SelectPageUp, &["pageup"], "Selection page up"),
        definition(Action::SelectPageDown, &["pagedown"], "Selection page down"),
        definition(Action::SelectConfirm, &["enter"], "Confirm selection"),
        definition(
            Action::SelectCancel,
            &["escape", "ctrl+c"],
            "Cancel selection",
        ),
    ]
}

fn definition(
    action: KeybindingAction,
    keys: &[&str],
    description: &'static str,
) -> KeybindingDefinition {
    KeybindingDefinition {
        action,
        default_keys: keys
            .iter()
            .map(|key| KeyId::new(*key).expect("default keybinding must be valid"))
            .collect(),
        description,
    }
}

fn unique_keys(keys: &[KeyId]) -> Vec<KeyId> {
    let mut seen = BTreeSet::new();
    let mut unique = Vec::new();
    for key in keys {
        if seen.insert(key.clone()) {
            unique.push(key.clone());
        }
    }
    unique
}
