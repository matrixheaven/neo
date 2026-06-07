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
