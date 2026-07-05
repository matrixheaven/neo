pub mod key_id;
pub mod keybinding;
pub mod raw_input;

use std::time::{Duration, Instant};

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
    key_id.chars().count() == 1 && key_id.chars().next().is_some_and(|c| !c.is_control())
}

/// Max time between an ESC and the following Enter for the pair to be treated
/// as a single Shift+Enter newline. This covers terminals (e.g. Ghostty with
/// certain configs) that send `ESC CR` for Shift+Enter instead of a CSI-u
/// sequence. The window is intentionally short so a deliberate Esc followed by
/// Enter is not misinterpreted.
const ESC_ENTER_NEWLINE_WINDOW: Duration = Duration::from_millis(30);

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
        assert!(events.contains(&InputEvent::Insert('a')));
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
    fn raw_bracketed_paste_preserves_split_utf8() {
        let mut parser = InputParser::new();
        let bytes = "测试".as_bytes();
        assert!(parser.feed_bytes(b"\x1b[200~").is_empty());
        assert!(parser.feed_bytes(&bytes[..1]).is_empty());
        assert!(parser.feed_bytes(&bytes[1..4]).is_empty());
        let mut tail = bytes[4..].to_vec();
        tail.extend_from_slice(b"\x1b[201~");
        assert_eq!(
            parser.feed_bytes(&tail),
            vec![InputEvent::Paste("测试".into())]
        );
    }

    #[test]
    fn raw_alt_up_with_keybindings_produces_key_event() {
        let mut parser = InputParser::with_keybindings(KeybindingsManager::default());
        let events = parser.feed_bytes(b"\x1b\x1b[A");
        assert_eq!(
            events,
            vec![InputEvent::Key(KeyId::new("alt+up").expect("valid key"))]
        );
    }

    #[test]
    fn raw_csi_alt_up_with_keybindings_produces_key_event() {
        let mut parser = InputParser::with_keybindings(KeybindingsManager::default());
        let events = parser.feed_bytes(b"\x1b[1;3A");
        assert_eq!(
            events,
            vec![InputEvent::Key(KeyId::new("alt+up").expect("valid key"))]
        );
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
    fn raw_sgr_mouse_wheel_up_is_ignored() {
        let mut parser = InputParser::new();
        assert_eq!(
            parser.feed_bytes(b"\x1b[<64;20;10M"),
            Vec::<InputEvent>::new()
        );
    }

    #[test]
    fn raw_sgr_mouse_wheel_down_is_ignored() {
        let mut parser = InputParser::new();
        assert_eq!(
            parser.feed_bytes(b"\x1b[<65;20;10M"),
            Vec::<InputEvent>::new()
        );
    }

    #[test]
    fn raw_sgr_mouse_release_is_ignored() {
        let mut parser = InputParser::new();
        assert_eq!(
            parser.feed_bytes(b"\x1b[<64;20;10m"),
            Vec::<InputEvent>::new()
        );
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
    fn raw_plus_produces_insert() {
        let mut parser = InputParser::with_keybindings(KeybindingsManager::default());
        assert_eq!(parser.feed_bytes(b"+"), vec![InputEvent::Insert('+')]);
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
    fn feed_bytes_split_cjk_character_waits_for_complete_utf8() {
        let mut parser = InputParser::with_keybindings(KeybindingsManager::default());
        let bytes = "测".as_bytes();
        assert!(parser.feed_bytes(&bytes[..1]).is_empty());
        assert_eq!(
            parser.feed_bytes(&bytes[1..]),
            vec![InputEvent::Insert('测')]
        );
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
