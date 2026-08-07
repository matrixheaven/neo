pub mod key_id;
pub mod keybinding;
pub mod raw_input;

use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub use crate::transcript::{DocumentPoint, MouseEvent, MouseKind};
pub use key_id::{KeyId, KeyIdError};
pub use keybinding::{
    KeybindingAction, KeybindingConflict, KeybindingDefinition, KeybindingsManager,
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
    Mouse(MouseEvent),
    Resize { columns: u16, rows: u16 },
    Cancel,
    Interrupt,
}

impl InputEvent {
    /// Whether the event is a wheel scroll (up or down). Wheel navigation
    /// keeps its transcript-wide behavior even while dialogs are focused.
    #[must_use]
    pub fn is_wheel(&self) -> bool {
        matches!(self, InputEvent::Mouse(event) if event.is_wheel())
    }
}

#[derive(Debug, Clone, Default)]
pub struct InputParser {
    keybindings: Option<KeybindingsManager>,
    /// Raw stdin byte parser for the `feed_bytes` path.
    raw_parser: RawInputParser,
    /// Pending ESC timestamp for the raw input path (no `KeyEvent` available).
    raw_pending_esc: Option<Instant>,
    /// Zero-based CPR observations queued as terminal protocol state.
    cursor_positions: VecDeque<(u16, u16)>,
}

impl InputParser {
    #[must_use]
    pub fn new() -> Self {
        Self {
            keybindings: None,
            raw_parser: RawInputParser::new(),
            raw_pending_esc: None,
            cursor_positions: VecDeque::new(),
        }
    }

    #[must_use]
    pub fn with_keybindings(keybindings: KeybindingsManager) -> Self {
        Self {
            keybindings: Some(keybindings),
            raw_parser: RawInputParser::new(),
            raw_pending_esc: None,
            cursor_positions: VecDeque::new(),
        }
    }

    /// Take the next zero-based CPR observation, if any.
    pub fn take_cursor_position(&mut self) -> Option<(u16, u16)> {
        self.cursor_positions.pop_front()
    }

    /// Discard cursor reports that predate a new terminal geometry probe.
    pub fn discard_cursor_positions(&mut self) {
        self.cursor_positions.clear();
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
            RawEvent::CursorPosition { column, row } => {
                self.cursor_positions.push_back((column, row));
                Vec::new()
            }
        }
    }

    /// Convert a complete ANSI sequence string into [`InputEvent`] values.
    fn convert_key_sequence(&mut self, seq: &str) -> Vec<InputEvent> {
        // Skip key release events
        if is_key_release(seq) {
            return Vec::new();
        }

        if let Some(mouse) = raw_input::parse_sgr_mouse(seq) {
            return vec![InputEvent::Mouse(mouse)];
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
        if matches_key(seq, "ctrl+enter") {
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
    key_id.chars().count() == 1 && key_id.chars().next().is_some_and(|c| !c.is_control())
}

/// Max time between an ESC and the following Enter for the pair to be treated
/// as a single Shift+Enter newline. This covers terminals (e.g. Ghostty with
/// certain configs) that send `ESC CR` for Shift+Enter instead of a CSI-u
/// sequence. The window is intentionally short so a deliberate Esc followed by
/// Enter is not misinterpreted.
const ESC_ENTER_NEWLINE_WINDOW: Duration = Duration::from_millis(30);

#[cfg(test)]
#[path = "test_cases/raw.rs"]
mod raw;

#[cfg(test)]
#[path = "test_cases/feed.rs"]
mod feed;

#[cfg(test)]
#[path = "test_cases/cursor.rs"]
mod cursor;

#[cfg(test)]
#[path = "test_cases/action_ids.rs"]
mod action_ids;
