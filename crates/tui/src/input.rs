use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    Insert(char),
    Backspace,
    Delete,
    MoveLeft,
    MoveRight,
    MoveHome,
    MoveEnd,
    Submit,
    NewLine,
    Cancel,
    Interrupt,
}

impl InputEvent {
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
}

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
            Self::SelectUp => "tui.select.up",
            Self::SelectDown => "tui.select.down",
            Self::SelectPageUp => "tui.select.pageUp",
            Self::SelectPageDown => "tui.select.pageDown",
            Self::SelectConfirm => "tui.select.confirm",
            Self::SelectCancel => "tui.select.cancel",
        }
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
        definition(Action::EditorUndo, &["ctrl+-"], "Undo"),
        definition(Action::InputNewLine, &["shift+enter"], "Insert newline"),
        definition(Action::InputSubmit, &["enter"], "Submit input"),
        definition(Action::InputTab, &["tab"], "Tab"),
        definition(Action::InputCopy, &["ctrl+c"], "Copy selection"),
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
