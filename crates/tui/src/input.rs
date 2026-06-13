use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};

const MOUSE_WHEEL_SCROLL_ROWS: usize = 3;

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
            Event::Mouse(mouse_event) => Self::from_mouse_event_kind(mouse_event.kind),
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
            Event::Mouse(mouse_event) => Self::from_mouse_event_kind(mouse_event.kind),
            Event::Resize(columns, rows) => Some(Self::Resize {
                columns: *columns,
                rows: *rows,
            }),
            _ => None,
        }
    }

    #[must_use]
    pub fn from_key_event(event: KeyEvent) -> Option<Self> {
        if event.kind != KeyEventKind::Press {
            return None;
        }

        match (event.code, event.modifiers) {
            (KeyCode::Char('c' | 'C'), KeyModifiers::CONTROL) => Some(Self::Interrupt),
            (KeyCode::Char(character), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                Some(Self::Insert(character))
            }
            (KeyCode::Backspace, _) => Some(Self::Backspace),
            (KeyCode::Delete, _) => Some(Self::Delete),
            (KeyCode::Left, _) => Some(Self::MoveLeft),
            (KeyCode::Right, _) => Some(Self::MoveRight),
            (KeyCode::Home, _) => Some(Self::MoveHome),
            (KeyCode::End, _) => Some(Self::MoveEnd),
            (KeyCode::Enter, KeyModifiers::SHIFT) => Some(Self::NewLine),
            (KeyCode::Enter, _) => Some(Self::Submit),
            (KeyCode::Esc, _) => Some(Self::Cancel),
            _ => None,
        }
    }

    #[must_use]
    pub fn from_key_event_with_keybindings(
        event: KeyEvent,
        keybindings: &KeybindingsManager,
    ) -> Option<Self> {
        if event.kind != KeyEventKind::Press {
            return None;
        }

        if matches!(event.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
            && let KeyCode::Char(character) = event.code
        {
            return Some(Self::Insert(character));
        }

        let key = KeyId::from_key_event(event)?;
        if keybindings.matching_actions(&key).is_empty() {
            return None;
        }
        Some(Self::Key(key))
    }

    fn from_mouse_event_kind(kind: MouseEventKind) -> Option<Self> {
        match kind {
            MouseEventKind::ScrollUp => Some(Self::ScrollUp(MOUSE_WHEEL_SCROLL_ROWS)),
            MouseEventKind::ScrollDown => Some(Self::ScrollDown(MOUSE_WHEEL_SCROLL_ROWS)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct InputParser {
    keybindings: Option<KeybindingsManager>,
    paste_buffer: Option<String>,
    pending_escape: String,
}

impl InputParser {
    #[must_use]
    pub fn new() -> Self {
        Self {
            keybindings: None,
            paste_buffer: None,
            pending_escape: String::new(),
        }
    }

    #[must_use]
    pub fn with_keybindings(keybindings: KeybindingsManager) -> Self {
        Self {
            keybindings: Some(keybindings),
            paste_buffer: None,
            pending_escape: String::new(),
        }
    }

    #[must_use]
    pub fn feed_crossterm_event(&mut self, event: &Event) -> Vec<InputEvent> {
        match event {
            Event::Paste(text) => vec![InputEvent::Paste(text.clone())],
            Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                self.feed_key_event(*key_event)
            }
            Event::Mouse(mouse_event) => InputEvent::from_mouse_event_kind(mouse_event.kind)
                .into_iter()
                .collect(),
            Event::Resize(columns, rows) => vec![InputEvent::Resize {
                columns: *columns,
                rows: *rows,
            }],
            _ => Vec::new(),
        }
    }

    fn feed_key_event(&mut self, event: KeyEvent) -> Vec<InputEvent> {
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
        if BRACKETED_PASTE_START.starts_with(&self.pending_escape)
            || BRACKETED_PASTE_END.starts_with(&self.pending_escape)
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
}

fn raw_sequence_character(event: KeyEvent) -> Option<char> {
    if event.modifiers != KeyModifiers::NONE {
        return None;
    }

    match event.code {
        KeyCode::Char(character) => Some(character),
        _ => None,
    }
}

const BRACKETED_PASTE_START: &str = "\x1b[200~";
const BRACKETED_PASTE_END: &str = "\x1b[201~";

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
        if event.kind != KeyEventKind::Press {
            return None;
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
    TranscriptSelectionStart,
    TranscriptSelectionClear,
    TranscriptSelectionExtendUp,
    TranscriptSelectionExtendDown,
    TranscriptSelectionExtendPageUp,
    TranscriptSelectionExtendPageDown,
    TranscriptCopySelection,
    AppClear,
    AppExit,
    AppSuspend,
    CommandPaletteOpen,
    SessionPickerOpen,
    SessionFork,
    ModelPickerOpen,
    SelectUp,
    SelectDown,
    SelectPageUp,
    SelectPageDown,
    SelectConfirm,
    SelectCancel,
}

impl KeybindingAction {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::EditorCursorUp => "tui.editor.cursorUp",
            Self::EditorCursorDown => "tui.editor.cursorDown",
            Self::EditorCursorLeft => "tui.editor.cursorLeft",
            Self::EditorCursorRight => "tui.editor.cursorRight",
            Self::EditorCursorWordLeft => "tui.editor.cursorWordLeft",
            Self::EditorCursorWordRight => "tui.editor.cursorWordRight",
            Self::EditorCursorLineStart => "tui.editor.cursorLineStart",
            Self::EditorCursorLineEnd => "tui.editor.cursorLineEnd",
            Self::EditorPageUp => "tui.editor.pageUp",
            Self::EditorPageDown => "tui.editor.pageDown",
            Self::EditorDeleteCharBackward => "tui.editor.deleteCharBackward",
            Self::EditorDeleteCharForward => "tui.editor.deleteCharForward",
            Self::EditorDeleteWordBackward => "tui.editor.deleteWordBackward",
            Self::EditorDeleteWordForward => "tui.editor.deleteWordForward",
            Self::EditorDeleteToLineStart => "tui.editor.deleteToLineStart",
            Self::EditorDeleteToLineEnd => "tui.editor.deleteToLineEnd",
            Self::EditorYank => "tui.editor.yank",
            Self::EditorUndo => "tui.editor.undo",
            Self::InputNewLine => "tui.input.newLine",
            Self::InputSubmit => "tui.input.submit",
            Self::InputTab => "tui.input.tab",
            Self::InputCopy => "tui.input.copy",
            Self::TranscriptSelectionStart => "tui.transcript.selection.start",
            Self::TranscriptSelectionClear => "tui.transcript.selection.clear",
            Self::TranscriptSelectionExtendUp => "tui.transcript.selection.extendUp",
            Self::TranscriptSelectionExtendDown => "tui.transcript.selection.extendDown",
            Self::TranscriptSelectionExtendPageUp => "tui.transcript.selection.extendPageUp",
            Self::TranscriptSelectionExtendPageDown => "tui.transcript.selection.extendPageDown",
            Self::TranscriptCopySelection => "tui.transcript.copySelection",
            Self::AppClear => "app.clear",
            Self::AppExit => "app.exit",
            Self::AppSuspend => "app.suspend",
            Self::CommandPaletteOpen => "tui.command.open",
            Self::SessionPickerOpen => "tui.session.open",
            Self::SessionFork => "tui.session.fork",
            Self::ModelPickerOpen => "tui.model.open",
            Self::SelectUp => "tui.select.up",
            Self::SelectDown => "tui.select.down",
            Self::SelectPageUp => "tui.select.pageUp",
            Self::SelectPageDown => "tui.select.pageDown",
            Self::SelectConfirm => "tui.select.confirm",
            Self::SelectCancel => "tui.select.cancel",
        }
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "tui.editor.cursorUp" => Self::EditorCursorUp,
            "tui.editor.cursorDown" => Self::EditorCursorDown,
            "tui.editor.cursorLeft" => Self::EditorCursorLeft,
            "tui.editor.cursorRight" => Self::EditorCursorRight,
            "tui.editor.cursorWordLeft" => Self::EditorCursorWordLeft,
            "tui.editor.cursorWordRight" => Self::EditorCursorWordRight,
            "tui.editor.cursorLineStart" => Self::EditorCursorLineStart,
            "tui.editor.cursorLineEnd" => Self::EditorCursorLineEnd,
            "tui.editor.pageUp" => Self::EditorPageUp,
            "tui.editor.pageDown" => Self::EditorPageDown,
            "tui.editor.deleteCharBackward" => Self::EditorDeleteCharBackward,
            "tui.editor.deleteCharForward" => Self::EditorDeleteCharForward,
            "tui.editor.deleteWordBackward" => Self::EditorDeleteWordBackward,
            "tui.editor.deleteWordForward" => Self::EditorDeleteWordForward,
            "tui.editor.deleteToLineStart" => Self::EditorDeleteToLineStart,
            "tui.editor.deleteToLineEnd" => Self::EditorDeleteToLineEnd,
            "tui.editor.yank" => Self::EditorYank,
            "tui.editor.undo" => Self::EditorUndo,
            "tui.input.newLine" => Self::InputNewLine,
            "tui.input.submit" => Self::InputSubmit,
            "tui.input.tab" => Self::InputTab,
            "tui.input.copy" => Self::InputCopy,
            "tui.transcript.selection.start" => Self::TranscriptSelectionStart,
            "tui.transcript.selection.clear" => Self::TranscriptSelectionClear,
            "tui.transcript.selection.extendUp" => Self::TranscriptSelectionExtendUp,
            "tui.transcript.selection.extendDown" => Self::TranscriptSelectionExtendDown,
            "tui.transcript.selection.extendPageUp" => Self::TranscriptSelectionExtendPageUp,
            "tui.transcript.selection.extendPageDown" => Self::TranscriptSelectionExtendPageDown,
            "tui.transcript.copySelection" => Self::TranscriptCopySelection,
            "app.clear" => Self::AppClear,
            "app.exit" => Self::AppExit,
            "app.suspend" => Self::AppSuspend,
            "tui.command.open" => Self::CommandPaletteOpen,
            "tui.session.open" => Self::SessionPickerOpen,
            "tui.session.fork" => Self::SessionFork,
            "tui.model.open" => Self::ModelPickerOpen,
            "tui.select.up" => Self::SelectUp,
            "tui.select.down" => Self::SelectDown,
            "tui.select.pageUp" => Self::SelectPageUp,
            "tui.select.pageDown" => Self::SelectPageDown,
            "tui.select.confirm" => Self::SelectConfirm,
            "tui.select.cancel" => Self::SelectCancel,
            _ => return None,
        })
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
        definition(Action::InputNewLine, &["shift+enter"], "Insert newline"),
        definition(Action::InputSubmit, &["enter"], "Submit input"),
        definition(Action::InputTab, &["tab"], "Tab"),
        definition(Action::InputCopy, &["ctrl+c"], "Copy selection"),
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
        definition(Action::SessionFork, &["ctrl+n"], "Fork selected session"),
        definition(Action::ModelPickerOpen, &["ctrl+o"], "Open models"),
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
    match code {
        KeyCode::Backspace => Some("backspace".into()),
        KeyCode::Enter => Some("enter".into()),
        KeyCode::Left => Some("left".into()),
        KeyCode::Right => Some("right".into()),
        KeyCode::Up => Some("up".into()),
        KeyCode::Down => Some("down".into()),
        KeyCode::Home => Some("home".into()),
        KeyCode::End => Some("end".into()),
        KeyCode::PageUp => Some("pageup".into()),
        KeyCode::PageDown => Some("pagedown".into()),
        KeyCode::Tab | KeyCode::BackTab => Some("tab".into()),
        KeyCode::Delete => Some("delete".into()),
        KeyCode::Insert => Some("insert".into()),
        KeyCode::Esc => Some("escape".into()),
        KeyCode::Char(' ') => Some("space".into()),
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
