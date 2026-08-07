use super::*;

fn parser() -> InputParser {
    InputParser::new()
}

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
fn raw_ctrl_enter_csi_u_produces_newline() {
    let mut parser = InputParser::with_keybindings(KeybindingsManager::default());
    assert_eq!(parser.feed_bytes(b"\x1b[13;5u"), vec![InputEvent::NewLine]);
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
fn sgr_mouse_parses_coordinates_buttons_modifiers_motion_and_release() {
    use crate::transcript::{MouseEvent, MouseKind};
    use crossterm::event::{KeyModifiers, MouseButton};

    let mouse = |kind, button, column, row, modifiers| {
        InputEvent::Mouse(MouseEvent {
            kind,
            button,
            column,
            row,
            modifiers,
        })
    };

    // Press: left button, one-based SGR coordinates become zero-based.
    assert_eq!(
        parser().feed_bytes(b"\x1b[<0;20;10M"),
        vec![mouse(
            MouseKind::Press,
            MouseButton::Left,
            19,
            9,
            KeyModifiers::NONE
        )]
    );
    // Right button.
    assert_eq!(
        parser().feed_bytes(b"\x1b[<2;1;1M"),
        vec![mouse(
            MouseKind::Press,
            MouseButton::Right,
            0,
            0,
            KeyModifiers::NONE
        )]
    );
    // Middle button.
    assert_eq!(
        parser().feed_bytes(b"\x1b[<1;5;3M"),
        vec![mouse(
            MouseKind::Press,
            MouseButton::Middle,
            4,
            2,
            KeyModifiers::NONE
        )]
    );
    // Modifier bits: 4 shift, 8 alt, 16 control, and combinations.
    assert_eq!(
        parser().feed_bytes(b"\x1b[<4;20;10M"),
        vec![mouse(
            MouseKind::Press,
            MouseButton::Left,
            19,
            9,
            KeyModifiers::SHIFT
        )]
    );
    assert_eq!(
        parser().feed_bytes(b"\x1b[<8;20;10M"),
        vec![mouse(
            MouseKind::Press,
            MouseButton::Left,
            19,
            9,
            KeyModifiers::ALT
        )]
    );
    assert_eq!(
        parser().feed_bytes(b"\x1b[<16;20;10M"),
        vec![mouse(
            MouseKind::Press,
            MouseButton::Left,
            19,
            9,
            KeyModifiers::CONTROL
        )]
    );
    assert_eq!(
        parser().feed_bytes(b"\x1b[<28;20;10M"),
        vec![mouse(
            MouseKind::Press,
            MouseButton::Left,
            19,
            9,
            KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL
        )]
    );
    // Motion bit (32) marks a drag with the button held.
    assert_eq!(
        parser().feed_bytes(b"\x1b[<32;20;10M"),
        vec![mouse(
            MouseKind::Drag,
            MouseButton::Left,
            19,
            9,
            KeyModifiers::NONE
        )]
    );
    // Drag with shift.
    assert_eq!(
        parser().feed_bytes(b"\x1b[<36;20;10M"),
        vec![mouse(
            MouseKind::Drag,
            MouseButton::Left,
            19,
            9,
            KeyModifiers::SHIFT
        )]
    );
    // Release: lowercase `m` suffix with button bits 3.
    assert_eq!(
        parser().feed_bytes(b"\x1b[<3;20;10m"),
        vec![mouse(
            MouseKind::Release,
            MouseButton::Left,
            19,
            9,
            KeyModifiers::NONE
        )]
    );
    // Release with shift modifier.
    assert_eq!(
        parser().feed_bytes(b"\x1b[<7;20;10m"),
        vec![mouse(
            MouseKind::Release,
            MouseButton::Left,
            19,
            9,
            KeyModifiers::SHIFT
        )]
    );
    // Wheel: 64 up, 65 down; 68/73 are wheel with shift/control.
    assert_eq!(
        parser().feed_bytes(b"\x1b[<64;20;10M"),
        vec![mouse(
            MouseKind::ScrollUp,
            MouseButton::Left,
            19,
            9,
            KeyModifiers::NONE
        )]
    );
    assert_eq!(
        parser().feed_bytes(b"\x1b[<65;20;10M"),
        vec![mouse(
            MouseKind::ScrollDown,
            MouseButton::Left,
            19,
            9,
            KeyModifiers::NONE
        )]
    );
    assert_eq!(
        parser().feed_bytes(b"\x1b[<68;20;10M"),
        vec![mouse(
            MouseKind::ScrollUp,
            MouseButton::Left,
            19,
            9,
            KeyModifiers::SHIFT
        )]
    );
    assert_eq!(
        parser().feed_bytes(b"\x1b[<81;20;10M"),
        vec![mouse(
            MouseKind::ScrollDown,
            MouseButton::Left,
            19,
            9,
            KeyModifiers::CONTROL
        )]
    );
    // The wheel's own release event does not re-scroll.
    assert_eq!(parser().feed_bytes(b"\x1b[<64;20;10m"), Vec::new());
    // Horizontal wheel (66/67) is not consumed as a scroll.
    assert_eq!(parser().feed_bytes(b"\x1b[<66;20;10M"), Vec::new());
    // Malformed sequences are ignored.
    assert_eq!(parser().feed_bytes(b"\x1b[<0;20M"), Vec::new());
    assert_eq!(parser().feed_bytes(b"\x1b[<0;20;10x"), Vec::new());
}

#[test]
fn raw_backspace() {
    let mut parser = InputParser::new();
    assert_eq!(parser.feed_bytes(b"\x7f"), vec![InputEvent::Backspace]);
}

#[test]
fn raw_ctrl_h_backspace_with_keybindings_deletes_backward() {
    let mut parser = InputParser::with_keybindings(KeybindingsManager::default());
    assert_eq!(
        parser.feed_bytes(b"\x08"),
        vec![InputEvent::Key(KeyId::new("backspace").expect("valid key"))]
    );
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
#[cfg(target_os = "windows")]
fn raw_legacy_uppercase_alt_v_with_keybindings_pastes_image_on_windows() {
    let mut parser = InputParser::with_keybindings(KeybindingsManager::default());
    assert_eq!(
        parser.feed_bytes(b"\x1bV"),
        vec![InputEvent::Key(KeyId::new("alt+v").expect("valid key"))]
    );
}

#[test]
#[cfg(not(target_os = "windows"))]
fn raw_legacy_uppercase_alt_v_without_default_keybinding_is_ignored() {
    let mut parser = InputParser::with_keybindings(KeybindingsManager::default());
    assert_eq!(parser.feed_bytes(b"\x1bV"), Vec::<InputEvent>::new());
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
