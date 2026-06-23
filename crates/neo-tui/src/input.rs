use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    time::{Duration, Instant},
};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

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

impl InputEvent {
    #[must_use]
    pub fn from_crossterm_event(event: &Event) -> Option<Self> {
        match event {
            Event::Paste(text) => Some(Self::Paste(text.clone())),
            Event::Key(key_event) => Self::from_key_event(*key_event),
            Event::Resize(columns, rows) => Some(Self::Resize {
                columns: *columns,
                rows: *rows,
            }),
            _ => None,
        }
    }

    #[must_use]
    pub fn from_crossterm_event_with_keybindings(
        event: &Event,
        keybindings: &KeybindingsManager,
    ) -> Option<Self> {
        match event {
            Event::Paste(text) => Some(Self::Paste(text.clone())),
            Event::Key(key_event) => Self::from_key_event_with_keybindings(*key_event, keybindings),
            Event::Resize(columns, rows) => Some(Self::Resize {
                columns: *columns,
                rows: *rows,
            }),
            _ => None,
        }
    }

    #[must_use]
    pub fn from_key_event(event: KeyEvent) -> Option<Self> {
        if !is_key_activation(event.kind) {
            return None;
        }

        match (event.code, event.modifiers) {
            (KeyCode::Char('c' | 'C'), KeyModifiers::CONTROL)
                if event.kind == KeyEventKind::Press =>
            {
                Some(Self::Interrupt)
            }
            (KeyCode::Char(character), KeyModifiers::NONE | KeyModifiers::SHIFT)
                if !character.is_control() =>
            {
                Some(Self::Insert(character))
            }
            (KeyCode::Backspace, _) => Some(Self::Backspace),
            (KeyCode::Delete, _) => Some(Self::Delete),
            (KeyCode::Left, _) => Some(Self::MoveLeft),
            (KeyCode::Right, _) => Some(Self::MoveRight),
            (KeyCode::Home, _) => Some(Self::MoveHome),
            (KeyCode::End, _) => Some(Self::MoveEnd),
            (KeyCode::Enter, modifiers)
                if event.kind == KeyEventKind::Press && modifiers.contains(KeyModifiers::ALT) =>
            {
                Some(Self::NewLine)
            }
            (KeyCode::Char('j' | 'J'), KeyModifiers::CONTROL)
                if event.kind == KeyEventKind::Press =>
            {
                Some(Self::NewLine)
            }
            (KeyCode::Enter, _) if event.kind == KeyEventKind::Press => Some(Self::Submit),
            (KeyCode::Esc, _) if event.kind == KeyEventKind::Press => Some(Self::Cancel),
            _ => None,
        }
    }

    #[must_use]
    pub fn from_key_event_with_keybindings(
        event: KeyEvent,
        keybindings: &KeybindingsManager,
    ) -> Option<Self> {
        if !is_key_activation(event.kind) {
            return None;
        }

        // Explicit intercept for newline keys — works regardless of keybinding
        // configuration and survives crossterm parsing quirks.
        if event.kind == KeyEventKind::Press
            && event.code == KeyCode::Enter
            && event.modifiers.contains(KeyModifiers::ALT)
        {
            return Some(Self::NewLine);
        }
        if event.kind == KeyEventKind::Press
            && event.code == KeyCode::Enter
            && event.modifiers.contains(KeyModifiers::SHIFT)
        {
            return Some(Self::NewLine);
        }
        if event.kind == KeyEventKind::Press
            && matches!(event.code, KeyCode::Char('j' | 'J'))
            && event.modifiers.contains(KeyModifiers::CONTROL)
        {
            return Some(Self::NewLine);
        }

        if matches!(event.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
            && let KeyCode::Char(character) = event.code
            && !character.is_control()
        {
            return Some(Self::Insert(character));
        }

        let key = KeyId::from_key_event(event)?;
        let actions = keybindings.matching_actions(&key);
        if actions.is_empty() || actions_are_repeat_blocked(event.kind, &actions) {
            return None;
        }
        Some(Self::Key(key))
    }
}

#[derive(Debug, Clone, Default)]
pub struct InputParser {
    keybindings: Option<KeybindingsManager>,
    paste_buffer: Option<String>,
    pending_escape: String,
    /// Some terminals send ESC followed by CR for Shift+Enter. We buffer a lone
    /// ESC briefly so that a subsequent Enter can be converted into a newline.
    pending_esc: Option<(Instant, KeyEvent)>,
}

impl InputParser {
    #[must_use]
    pub fn new() -> Self {
        Self {
            keybindings: None,
            paste_buffer: None,
            pending_escape: String::new(),
            pending_esc: None,
        }
    }

    #[must_use]
    pub fn with_keybindings(keybindings: KeybindingsManager) -> Self {
        Self {
            keybindings: Some(keybindings),
            paste_buffer: None,
            pending_escape: String::new(),
            pending_esc: None,
        }
    }

    #[must_use]
    pub fn feed_crossterm_event(&mut self, event: &Event) -> Vec<InputEvent> {
        match event {
            Event::Paste(text) => vec![InputEvent::Paste(text.clone())],
            Event::Key(key_event) if is_key_activation(key_event.kind) => {
                self.feed_key_event(*key_event)
            }
            Event::Resize(columns, rows) => vec![InputEvent::Resize {
                columns: *columns,
                rows: *rows,
            }],
            _ => Vec::new(),
        }
    }

    /// Flush any buffered input that has exceeded its recognition window.
    ///
    /// Call this after an input poll timeout so a lone ESC is still reported as
    /// `Cancel` even when no subsequent key arrives.
    #[must_use]
    pub fn flush_timeout(&mut self) -> Vec<InputEvent> {
        if let Some((esc_time, _)) = self.pending_esc
            && esc_time.elapsed() > ESC_ENTER_NEWLINE_WINDOW
        {
            self.pending_esc = None;
            return vec![InputEvent::Cancel];
        }
        Vec::new()
    }

    fn feed_key_event(&mut self, event: KeyEvent) -> Vec<InputEvent> {
        // Handle a buffered ESC that may be the first half of an ESC-CR Shift+Enter.
        if let Some((esc_time, _)) = self.pending_esc.take() {
            if event.code == KeyCode::Enter
                && event.modifiers == KeyModifiers::NONE
                && esc_time.elapsed() <= ESC_ENTER_NEWLINE_WINDOW
            {
                return vec![InputEvent::NewLine];
            }
            if event.code == KeyCode::Char('[')
                && matches!(event.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
            {
                "\x1b[".clone_into(&mut self.pending_escape);
                return Vec::new();
            }

            let mut output = vec![InputEvent::Cancel];
            output.extend(self.feed_key_event(event));
            return output;
        }

        if event.code == KeyCode::Esc
            && event.modifiers == KeyModifiers::NONE
            && event.kind == KeyEventKind::Press
        {
            self.pending_esc = Some((Instant::now(), event));
            return Vec::new();
        }

        if let Some(output) = self.feed_pending_escape(event) {
            return output;
        }

        if let Some(paste_buffer) = &mut self.paste_buffer {
            match event.code {
                KeyCode::Char(character)
                    if matches!(event.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) =>
                {
                    paste_buffer.push(character);
                }
                KeyCode::Enter => paste_buffer.push('\n'),
                KeyCode::Tab => paste_buffer.push('\t'),
                _ => {}
            }
            return Vec::new();
        }

        self.map_key_event(event).into_iter().collect()
    }

    fn feed_pending_escape(&mut self, event: KeyEvent) -> Option<Vec<InputEvent>> {
        let Some(character) = raw_sequence_character(event) else {
            return self.flush_pending_escape();
        };

        if self.pending_escape.is_empty() && character != '\x1b' {
            return None;
        }

        self.pending_escape.push(character);

        if self.pending_escape == BRACKETED_PASTE_START {
            self.pending_escape.clear();
            self.paste_buffer = Some(String::new());
            return Some(Vec::new());
        }
        if self.pending_escape == BRACKETED_PASTE_END {
            self.pending_escape.clear();
            return Some(vec![InputEvent::Paste(
                self.paste_buffer.take().unwrap_or_default(),
            )]);
        }
        if self.pending_escape == SHIFT_TAB_SEQUENCE {
            self.pending_escape.clear();
            let key = KeyId::new("shift+tab").expect("shift+tab key id is valid");
            return Some(self.map_key_id(key).into_iter().collect());
        }
        if BRACKETED_PASTE_START.starts_with(&self.pending_escape)
            || BRACKETED_PASTE_END.starts_with(&self.pending_escape)
            || SHIFT_TAB_SEQUENCE.starts_with(&self.pending_escape)
        {
            return Some(Vec::new());
        }

        self.flush_pending_escape()
    }

    fn flush_pending_escape(&mut self) -> Option<Vec<InputEvent>> {
        if self.pending_escape.is_empty() {
            return None;
        }

        let pending = std::mem::take(&mut self.pending_escape);
        if let Some(paste_buffer) = &mut self.paste_buffer {
            paste_buffer.push_str(&pending);
            return Some(Vec::new());
        }

        Some(
            pending
                .chars()
                .filter_map(|character| {
                    self.map_key_event(KeyEvent::new_with_kind(
                        KeyCode::Char(character),
                        KeyModifiers::NONE,
                        KeyEventKind::Press,
                    ))
                })
                .collect(),
        )
    }

    fn map_key_event(&self, event: KeyEvent) -> Option<InputEvent> {
        self.keybindings.as_ref().map_or_else(
            || InputEvent::from_key_event(event),
            |keybindings| InputEvent::from_key_event_with_keybindings(event, keybindings),
        )
    }

    fn map_key_id(&self, key: KeyId) -> Option<InputEvent> {
        let keybindings = self.keybindings.as_ref()?;
        (!keybindings.matching_actions(&key).is_empty()).then_some(InputEvent::Key(key))
    }
}

fn raw_sequence_character(event: KeyEvent) -> Option<char> {
    if !matches!(event.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) {
        return None;
    }

    match event.code {
        KeyCode::Char(character) => Some(character),
        _ => None,
    }
}

const BRACKETED_PASTE_START: &str = "\x1b[200~";
const BRACKETED_PASTE_END: &str = "\x1b[201~";
const SHIFT_TAB_SEQUENCE: &str = "\x1b[Z";

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

    #[must_use]
    pub fn from_key_event(event: KeyEvent) -> Option<Self> {
        if !is_key_activation(event.kind) {
            return None;
        }

        if event.modifiers == KeyModifiers::NONE
            && let KeyCode::Char(character) = event.code
            && let Some(key) = key_id_from_ascii_control(character)
        {
            return Some(key);
        }

        let base = key_base(event.code)?;
        let mut parts = Vec::new();
        if event.modifiers.contains(KeyModifiers::CONTROL) {
            parts.push("ctrl");
        }
        if event.modifiers.contains(KeyModifiers::ALT) {
            parts.push("alt");
        }
        if event.modifiers.contains(KeyModifiers::SHIFT) {
            parts.push("shift");
        }
        parts.push(base.as_str());
        Self::new(parts.join("+")).ok()
    }
}

const fn is_key_activation(kind: KeyEventKind) -> bool {
    matches!(kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

fn actions_are_repeat_blocked(kind: KeyEventKind, actions: &[KeybindingAction]) -> bool {
    kind == KeyEventKind::Repeat && actions.iter().any(|action| action.blocks_repeat())
}

fn key_id_from_ascii_control(character: char) -> Option<KeyId> {
    let code = character as u32;
    let base = match code {
        0 => "space".to_owned(),
        1..=26 => char::from(b'a' + u8::try_from(code).ok()? - 1).to_string(),
        28 => "\\".to_owned(),
        29 => "]".to_owned(),
        30 => "^".to_owned(),
        31 => "_".to_owned(),
        _ => return None,
    };
    KeyId::new(format!("ctrl+{base}")).ok()
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

    const fn blocks_repeat(self) -> bool {
        !matches!(
            self,
            Self::EditorCursorUp
                | Self::EditorCursorDown
                | Self::EditorCursorLeft
                | Self::EditorCursorRight
                | Self::EditorCursorWordLeft
                | Self::EditorCursorWordRight
                | Self::EditorCursorLineStart
                | Self::EditorCursorLineEnd
                | Self::EditorPageUp
                | Self::EditorPageDown
                | Self::EditorDeleteCharBackward
                | Self::EditorDeleteCharForward
                | Self::EditorDeleteWordBackward
                | Self::EditorDeleteWordForward
                | Self::EditorDeleteToLineStart
                | Self::EditorDeleteToLineEnd
                | Self::SelectUp
                | Self::SelectDown
                | Self::SelectPageUp
                | Self::SelectPageDown
                | Self::TranscriptSelectionExtendUp
                | Self::TranscriptSelectionExtendDown
                | Self::TranscriptSelectionExtendPageUp
                | Self::TranscriptSelectionExtendPageDown
        )
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

fn key_base(code: KeyCode) -> Option<String> {
    key_base_name(code)
        .map(str::to_owned)
        .or_else(|| dynamic_key_base(code))
}

fn key_base_name(code: KeyCode) -> Option<&'static str> {
    navigation_key_base_name(code)
        .or_else(|| edit_key_base_name(code))
        .or_else(|| named_char_key_base_name(code))
}

fn navigation_key_base_name(code: KeyCode) -> Option<&'static str> {
    arrow_key_base_name(code).or_else(|| page_key_base_name(code))
}

fn arrow_key_base_name(code: KeyCode) -> Option<&'static str> {
    match code {
        KeyCode::Left => Some("left"),
        KeyCode::Right => Some("right"),
        KeyCode::Up => Some("up"),
        KeyCode::Down => Some("down"),
        _ => None,
    }
}

fn page_key_base_name(code: KeyCode) -> Option<&'static str> {
    match code {
        KeyCode::Home => Some("home"),
        KeyCode::End => Some("end"),
        KeyCode::PageUp => Some("pageup"),
        KeyCode::PageDown => Some("pagedown"),
        _ => None,
    }
}

fn edit_key_base_name(code: KeyCode) -> Option<&'static str> {
    match code {
        KeyCode::Backspace => Some("backspace"),
        KeyCode::Enter => Some("enter"),
        KeyCode::Tab => Some("tab"),
        KeyCode::BackTab => Some("shift+tab"),
        KeyCode::Delete => Some("delete"),
        KeyCode::Insert => Some("insert"),
        KeyCode::Esc => Some("escape"),
        _ => None,
    }
}

fn named_char_key_base_name(code: KeyCode) -> Option<&'static str> {
    match code {
        KeyCode::Char(' ') => Some("space"),
        _ => None,
    }
}

fn dynamic_key_base(code: KeyCode) -> Option<String> {
    match code {
        KeyCode::Char(character) => Some(character.to_lowercase().collect()),
        KeyCode::F(number) if (1..=12).contains(&number) => Some(format!("f{number}")),
        _ => None,
    }
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

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new_with_kind(code, modifiers, KeyEventKind::Press)
    }

    #[test]
    fn esc_then_enter_becomes_newline() {
        let mut parser = InputParser::new();
        assert!(
            parser
                .feed_key_event(key(KeyCode::Esc, KeyModifiers::NONE))
                .is_empty()
        );
        assert_eq!(
            parser.feed_key_event(key(KeyCode::Enter, KeyModifiers::NONE)),
            vec![InputEvent::NewLine]
        );
    }

    #[test]
    fn esc_alone_is_buffered_and_flushed_after_timeout() {
        let mut parser = InputParser::new();
        assert!(
            parser
                .feed_key_event(key(KeyCode::Esc, KeyModifiers::NONE))
                .is_empty()
        );
        std::thread::sleep(ESC_ENTER_NEWLINE_WINDOW + Duration::from_millis(20));
        assert_eq!(parser.flush_timeout(), vec![InputEvent::Cancel]);
    }

    #[test]
    fn esc_then_letter_does_not_swallow_letter() {
        let mut parser = InputParser::new();
        assert!(
            parser
                .feed_key_event(key(KeyCode::Esc, KeyModifiers::NONE))
                .is_empty()
        );
        assert_eq!(
            parser.feed_key_event(key(KeyCode::Char('a'), KeyModifiers::NONE)),
            vec![InputEvent::Cancel, InputEvent::Insert('a')]
        );
    }

    #[test]
    fn esc_bracket_z_becomes_shift_tab_keybinding() {
        let mut parser = InputParser::with_keybindings(KeybindingsManager::default());
        assert!(
            parser
                .feed_key_event(key(KeyCode::Esc, KeyModifiers::NONE))
                .is_empty()
        );
        assert!(
            parser
                .feed_key_event(key(KeyCode::Char('['), KeyModifiers::NONE))
                .is_empty()
        );
        assert_eq!(
            parser.feed_key_event(key(KeyCode::Char('Z'), KeyModifiers::SHIFT)),
            vec![InputEvent::Key(KeyId::new("shift+tab").expect("valid key"))]
        );
    }

    #[test]
    fn bracketed_paste_still_works() {
        let mut parser = InputParser::new();
        for c in "\x1b[200~".chars() {
            assert!(
                parser
                    .feed_key_event(key(KeyCode::Char(c), KeyModifiers::NONE))
                    .is_empty()
            );
        }
        assert!(
            parser
                .feed_key_event(key(KeyCode::Char('h'), KeyModifiers::NONE))
                .is_empty()
        );
        assert!(
            parser
                .feed_key_event(key(KeyCode::Char('i'), KeyModifiers::NONE))
                .is_empty()
        );
        for c in "\x1b[201".chars() {
            assert!(
                parser
                    .feed_key_event(key(KeyCode::Char(c), KeyModifiers::NONE))
                    .is_empty()
            );
        }
        assert_eq!(
            parser.feed_key_event(key(KeyCode::Char('~'), KeyModifiers::NONE)),
            vec![InputEvent::Paste("hi".into())]
        );
        assert_eq!(
            parser.feed_key_event(key(KeyCode::Char('x'), KeyModifiers::NONE)),
            vec![InputEvent::Insert('x')]
        );
    }

    #[test]
    fn shift_enter_produces_newline() {
        let mut parser = InputParser::with_keybindings(KeybindingsManager::default());
        assert_eq!(
            parser.feed_key_event(key(KeyCode::Enter, KeyModifiers::SHIFT)),
            vec![InputEvent::NewLine]
        );
    }

    #[test]
    fn alt_enter_produces_newline() {
        let mut parser = InputParser::new();
        assert_eq!(
            parser.feed_key_event(key(KeyCode::Enter, KeyModifiers::ALT)),
            vec![InputEvent::NewLine]
        );
    }

    #[test]
    fn ctrl_j_produces_newline() {
        let mut parser = InputParser::new();
        assert_eq!(
            parser.feed_key_event(key(KeyCode::Char('j'), KeyModifiers::CONTROL)),
            vec![InputEvent::NewLine]
        );
    }

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

    #[test]
    fn key_base_names_special_keys_and_characters() {
        let cases = [
            (KeyCode::Backspace, Some("backspace")),
            (KeyCode::Enter, Some("enter")),
            (KeyCode::Tab, Some("tab")),
            (KeyCode::BackTab, Some("shift+tab")),
            (KeyCode::Esc, Some("escape")),
            (KeyCode::Char(' '), Some("space")),
            (KeyCode::Char('A'), Some("a")),
            (KeyCode::F(12), Some("f12")),
            (KeyCode::F(13), None),
        ];

        for (code, expected) in cases {
            assert_eq!(key_base(code), expected.map(str::to_owned));
        }
    }
}
