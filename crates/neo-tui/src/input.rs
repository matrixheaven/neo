mod raw_input;

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    time::{Duration, Instant},
};

pub use raw_input::{
    RawEvent, RawInputParser, decode_printable_key, is_key_release, is_key_repeat,
    is_kitty_protocol_active, matches_key, parse_key, set_kitty_protocol_active,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    Insert(char),
    Paste(String),
    Key(KeyId),
    Action(KeybindingAction),
    Backspace,
    Delete,
    MoveLeft,
    MoveRight,
    MoveHome,
    MoveEnd,
    Submit,
    NewLine,
    ScrollUp(usize),
    ScrollDown(usize),
    Resize { columns: u16, rows: u16 },
    Cancel,
    Interrupt,
}

#[derive(Debug, Clone, Default)]
pub struct InputParser {
    keybindings: Option<KeybindingsManager>,
    /// Raw stdin byte parser for the `feed_bytes` path.
    raw_parser: RawInputParser,
    /// Pending ESC timestamp for the raw input path (no `KeyEvent` available).
    raw_pending_esc: Option<Instant>,
}

impl InputParser {
    #[must_use]
    pub fn new() -> Self {
        Self {
            keybindings: None,
            raw_parser: RawInputParser::new(),
            raw_pending_esc: None,
        }
    }

    #[must_use]
    pub fn with_keybindings(keybindings: KeybindingsManager) -> Self {
        Self {
            keybindings: Some(keybindings),
            raw_parser: RawInputParser::new(),
            raw_pending_esc: None,
        }
    }

    /// Feed raw stdin bytes through the raw input parser.
    ///
    /// This is the primary entry point for the raw-stdin event loop. It
    /// buffers bytes into complete ANSI sequences, handles bracketed paste,
    /// and converts each sequence into [`InputEvent`] values.
    #[must_use]
    pub fn feed_bytes(&mut self, data: &[u8]) -> Vec<InputEvent> {
        let raw_events = self.raw_parser.feed_bytes(data);
        raw_events
            .into_iter()
            .flat_map(|ev| self.convert_raw_event(ev))
            .collect()
    }

    /// Flush any buffered input that has exceeded its recognition window.
    ///
    /// Call this after an input poll timeout so a lone ESC is still reported as
    /// `Cancel` even when no subsequent key arrives.
    #[must_use]
    pub fn flush_timeout(&mut self) -> Vec<InputEvent> {
        let mut events = Vec::new();

        // Flush the raw-path pending ESC
        if let Some(esc_time) = self.raw_pending_esc
            && esc_time.elapsed() > ESC_ENTER_NEWLINE_WINDOW
        {
            self.raw_pending_esc = None;
            events.push(InputEvent::Cancel);
        }

        // Flush incomplete sequences from the raw parser
        for raw_event in self.raw_parser.flush() {
            events.extend(self.convert_raw_event(raw_event));
        }

        events
    }

    /// Convert a [`RawEvent`] into zero or more [`InputEvent`] values.
    fn convert_raw_event(&mut self, event: RawEvent) -> Vec<InputEvent> {
        match event {
            RawEvent::Paste(text) => vec![InputEvent::Paste(text)],
            RawEvent::Key(seq) => self.convert_key_sequence(&seq),
        }
    }

    /// Convert a complete ANSI sequence string into [`InputEvent`] values.
    fn convert_key_sequence(&mut self, seq: &str) -> Vec<InputEvent> {
        // Skip key release events
        if is_key_release(seq) {
            return Vec::new();
        }

        // Try printable key first (for text insertion)
        if let Some(ch) = decode_printable_key(seq) {
            return vec![InputEvent::Insert(ch)];
        }

        // Check explicit newline keys before parse_key to handle ambiguous
        // cases like \n (which parse_key returns as "enter")
        if matches_key(seq, "ctrl+j") {
            return vec![InputEvent::NewLine];
        }
        if matches_key(seq, "shift+enter") {
            return vec![InputEvent::NewLine];
        }
        if matches_key(seq, "alt+enter") {
            return vec![InputEvent::NewLine];
        }

        // Parse the key id
        let Some(key_id) = parse_key(seq) else {
            return Vec::new();
        };

        // Handle ESC+Enter newline detection for the raw path
        if let Some(esc_time) = self.raw_pending_esc.take() {
            if key_id == "enter" && esc_time.elapsed() <= ESC_ENTER_NEWLINE_WINDOW {
                return vec![InputEvent::NewLine];
            }
            // ESC followed by something else — emit Cancel then process
            let mut events = vec![InputEvent::Cancel];
            events.extend(self.map_raw_key_id(&key_id));
            return events;
        }

        if key_id == "escape" {
            self.raw_pending_esc = Some(Instant::now());
            return Vec::new();
        }

        self.map_raw_key_id(&key_id).into_iter().collect()
    }

    /// Map a parsed key id string to an [`InputEvent`] using the active
    /// keybindings (or direct mapping when no keybindings are configured).
    fn map_raw_key_id(&self, key_id: &str) -> Option<InputEvent> {
        // Plain printable characters (no modifiers) produce Insert, matching
        // the raw path behavior. This must be checked before keybinding
        // matching so that typing a letter inserts text.
        if is_plain_printable_key_id(key_id) {
            let ch = key_id.chars().next().expect("checked non-empty");
            return Some(InputEvent::Insert(ch));
        }

        // Named printable keys that should insert text
        if key_id == "space" {
            return Some(InputEvent::Insert(' '));
        }

        // With keybindings, convert to KeyId and check
        if let Some(keybindings) = &self.keybindings {
            let key = KeyId::new(key_id).ok()?;
            let actions = keybindings.matching_actions(&key);
            if actions.is_empty() {
                return None;
            }
            return Some(InputEvent::Key(key));
        }

        // Without keybindings, map directly
        match key_id {
            "ctrl+c" => Some(InputEvent::Interrupt),
            "space" => Some(InputEvent::Insert(' ')),
            "enter" => Some(InputEvent::Submit),
            "backspace" => Some(InputEvent::Backspace),
            "delete" => Some(InputEvent::Delete),
            "left" => Some(InputEvent::MoveLeft),
            "right" => Some(InputEvent::MoveRight),
            "home" => Some(InputEvent::MoveHome),
            "end" => Some(InputEvent::MoveEnd),
            "escape" => Some(InputEvent::Cancel),
            _ => KeyId::new(key_id).ok().map(InputEvent::Key),
        }
    }
}

/// Check if a key id represents a plain printable character with no modifiers.
/// Such keys should produce `InputEvent::Insert(char)` rather than a key event.
fn is_plain_printable_key_id(key_id: &str) -> bool {
    !key_id.contains('+')
        && key_id.chars().count() == 1
        && key_id.chars().next().is_some_and(|c| !c.is_control())
}

/// Max time between an ESC and the following Enter for the pair to be treated
/// as a single Shift+Enter newline. This covers terminals (e.g. Ghostty with
/// certain configs) that send `ESC CR` for Shift+Enter instead of a CSI-u
/// sequence. The window is intentionally short so a deliberate Esc followed by
/// Enter is not misinterpreted.
const ESC_ENTER_NEWLINE_WINDOW: Duration = Duration::from_millis(30);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyId(String);

impl KeyId {
    pub fn new(value: impl Into<String>) -> Result<Self, KeyIdError> {
        let value = value.into();
        let normalized = normalize_key_id(&value).ok_or_else(|| KeyIdError {
            value: value.clone(),
        })?;
        Ok(Self(normalized))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_text_insertion_key(&self) -> bool {
        let mut parts = self.0.split('+').collect::<Vec<_>>();
        let Some(base) = parts.pop() else {
            return false;
        };
        let has_action_modifier = parts
            .iter()
            .any(|modifier| matches!(*modifier, "ctrl" | "alt" | "super"));
        !has_action_modifier && (base == "space" || base.chars().count() == 1)
    }
}

impl fmt::Display for KeyId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyIdError {
    value: String,
}

impl fmt::Display for KeyIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid key id: {}", self.value)
    }
}

impl std::error::Error for KeyIdError {}

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
    /// Pop the most recent queued follow-up back into the composer for editing.
    /// Default: Alt+Up.
    EditLastQueuedMessage,
    TranscriptSelectionStart,
    TranscriptSelectionClear,
    TranscriptSelectionExtendUp,
    TranscriptSelectionExtendDown,
    TranscriptSelectionExtendPageUp,
    TranscriptSelectionExtendPageDown,
    TranscriptCopySelection,
    ToolOutputToggle,
    PasteImage,
    AppClear,
    AppExit,
    AppSuspend,
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
        KeybindingAction::EditLastQueuedMessage,
        "tui.input.editLastQueuedMessage",
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
    (KeybindingAction::PasteImage, "tui.input.pasteImage"),
    (KeybindingAction::AppClear, "app.clear"),
    (KeybindingAction::AppExit, "app.exit"),
    (KeybindingAction::AppSuspend, "app.suspend"),
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
            &["alt+enter", "ctrl+j"],
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
            Action::EditLastQueuedMessage,
            &["alt+up"],
            "Edit the last queued follow-up message",
        ),
        definition(
            Action::CycleDevelopmentMode,
            &["shift+tab"],
            "Cycle normal/plan/goal mode",
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
            Action::CommandPaletteOpen,
            &["ctrl+p"],
            "Open command palette",
        ),
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

fn normalize_key_id(value: &str) -> Option<String> {
    let mut parts = value
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let base = parts.pop()?.to_ascii_lowercase();
    let base = match base.as_str() {
        "esc" => "escape".to_string(),
        "return" => "enter".to_string(),
        "pageup" => "pageup".to_string(),
        "pagedown" => "pagedown".to_string(),
        _ => base,
    };

    if !is_valid_base_key(&base) {
        return None;
    }

    let mut modifiers = Vec::new();
    for part in parts {
        let modifier = part.to_ascii_lowercase();
        if !matches!(modifier.as_str(), "ctrl" | "alt" | "shift" | "super") {
            return None;
        }
        if !modifiers.contains(&modifier) {
            modifiers.push(modifier);
        }
    }
    modifiers.push(base);
    Some(modifiers.join("+"))
}

fn is_valid_base_key(base: &str) -> bool {
    matches!(
        base,
        "escape"
            | "enter"
            | "tab"
            | "space"
            | "backspace"
            | "delete"
            | "insert"
            | "clear"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
            | "up"
            | "down"
            | "left"
            | "right"
            | "f1"
            | "f2"
            | "f3"
            | "f4"
            | "f5"
            | "f6"
            | "f7"
            | "f8"
            | "f9"
            | "f10"
            | "f11"
            | "f12"
            | "`"
            | "-"
            | "="
            | "["
            | "]"
            | "\\"
            | ";"
            | "'"
            | ","
            | "."
            | "/"
            | "!"
            | "@"
            | "#"
            | "$"
            | "%"
            | "^"
            | "&"
            | "*"
            | "("
            | ")"
            | "_"
            | "|"
            | "~"
            | "{"
            | "}"
            | ":"
            | "<"
            | ">"
            | "?"
    ) || base.chars().count() == 1
}

#[cfg(test)]
mod tests {
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
            KeybindingAction::TranscriptSelectionStart,
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

    // ======================================================================
    // Raw input (feed_bytes) tests
    // ======================================================================

    #[test]
    fn raw_ctrl_c_produces_interrupt() {
        let mut parser = InputParser::new();
        assert_eq!(parser.feed_bytes(b"\x03"), vec![InputEvent::Interrupt]);
    }

    #[test]
    fn raw_ctrl_v_legacy_produces_key_event() {
        // Without keybindings, ctrl+v maps to KeyId
        let mut parser = InputParser::new();
        let events = parser.feed_bytes(b"\x16");
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            InputEvent::Key(ref k) if k.as_str() == "ctrl+v"
        ));
    }

    #[test]
    fn raw_ctrl_v_kitty_produces_key_event() {
        // CSI-u format for Ctrl+V
        let mut parser = InputParser::new();
        let events = parser.feed_bytes(b"\x1b[118;5u");
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            InputEvent::Key(ref k) if k.as_str() == "ctrl+v"
        ));
    }

    #[test]
    fn raw_ctrl_v_with_keybindings() {
        let mut parser = InputParser::with_keybindings(KeybindingsManager::default());
        let events = parser.feed_bytes(b"\x16");
        // ctrl+v maps to PasteImage action
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            InputEvent::Key(ref k) if k.as_str() == "ctrl+v"
        ));
    }

    #[test]
    fn raw_enter_produces_submit() {
        let mut parser = InputParser::new();
        assert_eq!(parser.feed_bytes(b"\r"), vec![InputEvent::Submit]);
    }

    #[test]
    fn raw_esc_then_enter_becomes_newline() {
        let mut parser = InputParser::new();
        assert!(parser.feed_bytes(b"\x1b").is_empty());
        assert_eq!(parser.feed_bytes(b"\r"), vec![InputEvent::NewLine]);
    }

    #[test]
    fn raw_esc_alone_flushed_after_timeout() {
        let mut parser = InputParser::new();
        assert!(parser.feed_bytes(b"\x1b").is_empty());
        // RawInputParser buffers the lone ESC; flush forces it out
        let events = parser.flush_timeout();
        // The first flush_timeout emits the ESC, starting the pending_esc timer
        assert!(events.is_empty() || events == vec![InputEvent::Cancel]);
        if events.is_empty() {
            // ESC was flushed from raw_parser, now pending_esc is set
            std::thread::sleep(ESC_ENTER_NEWLINE_WINDOW + Duration::from_millis(20));
            assert_eq!(parser.flush_timeout(), vec![InputEvent::Cancel]);
        }
    }

    #[test]
    fn raw_esc_then_letter_does_not_swallow_letter() {
        let mut parser = InputParser::new();
        // ESC + 'a' arrives as a single meta-key sequence \x1ba
        let events = parser.feed_bytes(b"\x1b");
        assert!(events.is_empty());
        // Flush to get the ESC out
        let _ = parser.flush_timeout();
        // Now feed 'a' — but pending_esc might or might not be set depending on timing
        // The raw path handles this: ESC is converted to Cancel, then 'a' is Insert
        let events = parser.feed_bytes(b"a");
        // Should get Insert('a') at minimum
        assert!(events.iter().any(|e| *e == InputEvent::Insert('a')));
    }

    #[test]
    fn raw_shift_tab_single_sequence() {
        let mut parser = InputParser::with_keybindings(KeybindingsManager::default());
        let events = parser.feed_bytes(b"\x1b[Z");
        assert_eq!(
            events,
            vec![InputEvent::Key(KeyId::new("shift+tab").expect("valid key"))]
        );
    }

    #[test]
    fn raw_bracketed_paste_single_chunk() {
        let mut parser = InputParser::new();
        let events = parser.feed_bytes(b"\x1b[200~hi\x1b[201~");
        assert_eq!(events, vec![InputEvent::Paste("hi".into())]);
    }

    #[test]
    fn raw_bracketed_paste_then_key() {
        let mut parser = InputParser::new();
        let _ = parser.feed_bytes(b"\x1b[200~paste\x1b[201~");
        assert_eq!(parser.feed_bytes(b"x"), vec![InputEvent::Insert('x')]);
    }

    #[test]
    fn raw_ctrl_j_produces_newline() {
        let mut parser = InputParser::new();
        assert_eq!(parser.feed_bytes(b"\x0a"), vec![InputEvent::NewLine]);
    }

    #[test]
    fn raw_shift_enter_kitty_csi_u() {
        let mut parser = InputParser::new();
        // CSI-u for Shift+Enter: codepoint 13, modifier 2 (shift)
        assert_eq!(parser.feed_bytes(b"\x1b[13;2u"), vec![InputEvent::NewLine]);
    }

    #[test]
    fn raw_alt_enter_legacy() {
        let mut parser = InputParser::new();
        // ESC + CR = alt+enter in legacy mode
        assert_eq!(parser.feed_bytes(b"\x1b\r"), vec![InputEvent::NewLine]);
    }

    #[test]
    fn raw_backspace() {
        let mut parser = InputParser::new();
        assert_eq!(parser.feed_bytes(b"\x7f"), vec![InputEvent::Backspace]);
    }

    #[test]
    fn raw_arrow_keys() {
        let mut parser = InputParser::with_keybindings(KeybindingsManager::default());
        let events = parser.feed_bytes(b"\x1b[A");
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            InputEvent::Key(ref k) if k.as_str() == "up"
        ));

        let events = parser.feed_bytes(b"\x1b[B");
        assert!(matches!(
            events[0],
            InputEvent::Key(ref k) if k.as_str() == "down"
        ));
    }

    #[test]
    fn raw_printable_char() {
        let mut parser = InputParser::new();
        assert_eq!(parser.feed_bytes(b"a"), vec![InputEvent::Insert('a')]);
    }

    #[test]
    fn raw_multiple_chars() {
        let mut parser = InputParser::new();
        let events = parser.feed_bytes(b"abc");
        assert_eq!(
            events,
            vec![
                InputEvent::Insert('a'),
                InputEvent::Insert('b'),
                InputEvent::Insert('c'),
            ]
        );
    }

    #[test]
    fn raw_kitty_printable_dedup() {
        let mut parser = InputParser::new();
        // When Kitty protocol is active, pressing 'a' sends both CSI-u and plain 'a'
        // The plain 'a' should be deduplicated
        let events = parser.feed_bytes(b"\x1b[97ua");
        assert_eq!(events, vec![InputEvent::Insert('a')]);
    }

    #[test]
    fn raw_ctrl_c_with_keybindings_matches_copy() {
        let mut parser = InputParser::with_keybindings(KeybindingsManager::default());
        let events = parser.feed_bytes(b"\x03");
        // With keybindings, ctrl+c matches KeyId("ctrl+c")
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            InputEvent::Key(ref k) if k.as_str() == "ctrl+c"
        ));
    }

    #[test]
    fn feed_bytes_cjk_character_produces_insert() {
        let mut parser = InputParser::with_keybindings(KeybindingsManager::default());
        let events = parser.feed_bytes("你".as_bytes());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], InputEvent::Insert('你'));
    }

    #[test]
    fn feed_bytes_space_produces_insert() {
        let mut parser = InputParser::with_keybindings(KeybindingsManager::default());
        let events = parser.feed_bytes(b" ");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], InputEvent::Insert(' '));
    }

    #[test]
    fn feed_bytes_fullwidth_symbol_produces_insert() {
        let mut parser = InputParser::with_keybindings(KeybindingsManager::default());
        let events = parser.feed_bytes("，".as_bytes());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], InputEvent::Insert('，'));
    }
}
