use super::*;

#[test]
fn parse_ctrl_c_legacy() {
    assert_eq!(parse_key("\x03"), Some("ctrl+c".to_owned()));
}

#[test]
fn parse_ctrl_v_legacy() {
    assert_eq!(parse_key("\x16"), Some("ctrl+v".to_owned()));
}

#[test]
fn parse_ctrl_v_kitty() {
    // CSI-u format for Ctrl+V: codepoint 118, modifier 5 (ctrl=4, 1-indexed=5)
    assert_eq!(parse_key("\x1b[118;5u"), Some("ctrl+v".to_owned()));
}

#[test]
fn matches_ctrl_v_legacy() {
    assert!(matches_key("\x16", "ctrl+v"));
}

#[test]
fn matches_ctrl_v_kitty() {
    assert!(matches_key("\x1b[118;5u", "ctrl+v"));
}

#[test]
fn parse_enter() {
    assert_eq!(parse_key("\r"), Some("enter".to_owned()));
}

#[test]
fn parse_escape() {
    assert_eq!(parse_key("\x1b"), Some("escape".to_owned()));
}

#[test]
fn parse_shift_tab() {
    assert_eq!(parse_key("\x1b[Z"), Some("shift+tab".to_owned()));
}

#[test]
fn parse_backspace() {
    assert_eq!(parse_key("\x7f"), Some("backspace".to_owned()));
}

#[test]
fn bracketed_paste_single_chunk() {
    let mut parser = RawInputParser::new();
    let events = parser.feed_bytes(b"\x1b[200~hello\x1b[201~");
    assert_eq!(events, vec![RawEvent::Paste("hello".to_owned())]);
}

#[test]
fn bracketed_paste_multi_chunk() {
    let mut parser = RawInputParser::new();
    let events = parser.feed_bytes(b"\x1b[200~hel");
    assert!(events.is_empty());
    let events = parser.feed_bytes(b"lo\x1b[201~");
    assert_eq!(events, vec![RawEvent::Paste("hello".to_owned())]);
}

#[test]
fn unterminated_bracketed_paste_is_bounded_and_recovers() {
    let mut parser = RawInputParser::new();
    assert!(parser.feed_bytes(b"\x1b[200~unfinished").is_empty());
    assert_eq!(
        parser.flush(),
        vec![RawEvent::Paste("unfinished".to_owned())]
    );
    assert_eq!(parser.feed_bytes(b"x"), vec![RawEvent::Key("x".to_owned())]);

    let mut parser = RawInputParser::new();
    assert!(parser.feed_bytes(b"\x1b[200~abc\x1b").is_empty());
    assert!(parser.feed_bytes(&[0xe4]).is_empty());
    assert_eq!(
        parser.flush(),
        vec![RawEvent::Paste("abc\x1b\u{fffd}".to_owned())]
    );

    let oversized = vec![b'a'; MAX_BRACKETED_PASTE_BYTES];
    let mut input = BRACKETED_PASTE_START.as_bytes().to_vec();
    input.extend_from_slice(&oversized);
    input.extend_from_slice(b"discarded-tail");
    assert!(parser.feed_bytes(&input).is_empty());
    let events = parser.feed_bytes(b"\x1b[201~y");
    assert_eq!(events.len(), 2);
    assert!(matches!(&events[0], RawEvent::Paste(text) if text.len() == MAX_BRACKETED_PASTE_BYTES));
    assert_eq!(events[1], RawEvent::Key("y".to_owned()));

    let mut parser = RawInputParser::new();
    let unicode = "你".repeat(MAX_BRACKETED_PASTE_BYTES / 3 + 1);
    let mut input = BRACKETED_PASTE_START.as_bytes().to_vec();
    input.extend_from_slice(unicode.as_bytes());
    assert!(parser.feed_bytes(&input).is_empty());
    let events = parser.flush();
    let expected_len = MAX_BRACKETED_PASTE_BYTES - MAX_BRACKETED_PASTE_BYTES % '你'.len_utf8();
    assert!(matches!(&events[0], RawEvent::Paste(text) if text.len() == expected_len));
    assert_eq!(parser.feed_bytes(b"z"), vec![RawEvent::Key("z".to_owned())]);
}

#[test]
fn key_after_paste() {
    let mut parser = RawInputParser::new();
    parser.feed_bytes(b"\x1b[200~paste\x1b[201~");
    let events = parser.feed_bytes(b"x");
    assert_eq!(events, vec![RawEvent::Key("x".to_owned())]);
}

#[test]
fn ctrl_c_then_ctrl_v_produces_two_events() {
    let mut parser = RawInputParser::new();
    let events = parser.feed_bytes(b"\x03\x16");
    assert_eq!(
        events,
        vec![
            RawEvent::Key("\x03".to_owned()),
            RawEvent::Key("\x16".to_owned()),
        ]
    );
}

#[test]
fn arrow_up_sequence() {
    let mut parser = RawInputParser::new();
    let events = parser.feed_bytes(b"\x1b[A");
    assert_eq!(events, vec![RawEvent::Key("\x1b[A".to_owned())]);
    assert_eq!(parse_key("\x1b[A"), Some("up".to_owned()));
}

#[test]
fn meta_arrow_up_sequence() {
    let mut parser = RawInputParser::new();
    let events = parser.feed_bytes(b"\x1b\x1b[A");
    assert_eq!(events, vec![RawEvent::Key("\x1b\x1b[A".to_owned())]);
    assert_eq!(parse_key("\x1b\x1b[A"), Some("alt+up".to_owned()));
    assert!(matches_key("\x1b\x1b[A", "alt+up"));
}

#[test]
fn meta_cjk_sequence_preserves_multibyte_character() {
    let mut parser = RawInputParser::new();
    let events = parser.feed_bytes("\x1b你".as_bytes());
    assert_eq!(events, vec![RawEvent::Key("\x1b你".to_owned())]);
}

#[test]
fn flush_lone_esc() {
    let mut parser = RawInputParser::new();
    let events = parser.feed_bytes(b"\x1b");
    assert!(events.is_empty());
    let events = parser.flush();
    assert_eq!(events, vec![RawEvent::Key("\x1b".to_owned())]);
}

#[test]
fn esc_enter_single_sequence() {
    let mut parser = RawInputParser::new();
    let events = parser.feed_bytes(b"\x1b\r");
    assert_eq!(events, vec![RawEvent::Key("\x1b\r".to_owned())]);
}

#[test]
fn decode_printable_kitty_a() {
    // CSI-u for plain 'a': codepoint 97, no modifiers
    assert_eq!(decode_printable_key("\x1b[97u"), Some('a'));
}

#[test]
fn decode_printable_kitty_shift_a() {
    // CSI-u for Shift+a (A): codepoint 97, shifted 65, modifier 2 (shift)
    assert_eq!(decode_printable_key("\x1b[97:65;2u"), Some('A'));
}

#[test]
fn decode_printable_rejects_ctrl() {
    // Ctrl+v should not be decoded as printable
    assert_eq!(decode_printable_key("\x1b[118;5u"), None);
}

#[test]
fn is_key_release_detection() {
    assert!(is_key_release("\x1b[97;5:3u"));
    assert!(!is_key_release("\x1b[97;5u"));
    assert!(!is_key_release("\x1b[200~some paste:3u"));
}

#[test]
fn parse_cjk_character() {
    // CJK character 你 (U+4F60, UTF-8: E4 BD A0)
    assert_eq!(parse_key("你"), Some("你".to_owned()));
}

#[test]
fn parse_emoji_character() {
    // Emoji 😀 (U+1F600)
    assert_eq!(parse_key("😀"), Some("😀".to_owned()));
}

#[test]
fn parse_fullwidth_symbol() {
    // Full-width comma （U+FF0C, UTF-8: EF BC 8C）
    assert_eq!(parse_key("，"), Some("，".to_owned()));
}

#[test]
fn feed_bytes_cjk_character() {
    let mut parser = RawInputParser::new();
    let events = parser.feed_bytes("你".as_bytes());
    assert_eq!(events.len(), 1);
    assert_eq!(events[0], RawEvent::Key("你".to_owned()));
}

#[test]
fn feed_bytes_multiple_cjk() {
    let mut parser = RawInputParser::new();
    let events = parser.feed_bytes("你好".as_bytes());
    assert_eq!(events.len(), 2);
    assert_eq!(events[0], RawEvent::Key("你".to_owned()));
    assert_eq!(events[1], RawEvent::Key("好".to_owned()));
}

#[test]
fn feed_bytes_space() {
    let mut parser = RawInputParser::new();
    let events = parser.feed_bytes(b" ");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0], RawEvent::Key(" ".to_owned()));
}

#[test]
fn sgr_mouse_no_button_motion_is_not_release() {
    // XTerm encodes motion with no button held as code 35 (motion bit 32
    // + button bits 3). It must not be classified as a release, which
    // would clear an established selection on plain mouse movement. It
    // parses as a drag: transient motion for queue coalescing, and a
    // no-op in the pane without a press anchor.
    let motion = parse_sgr_mouse("\x1b[<35;10;10M").expect("no-button motion parses");
    assert_ne!(motion.kind, MouseKind::Release);
    assert_eq!(motion.kind, MouseKind::Drag);
    assert_eq!(motion.button, MouseButton::Left);
    assert_eq!(motion.column, 9);
    assert_eq!(motion.row, 9);
    // Modifier variants of no-button motion stay motion, never release.
    assert_eq!(
        parse_sgr_mouse("\x1b[<39;10;10M").map(|event| event.kind),
        Some(MouseKind::Drag)
    );
    // Real releases and presses still classify correctly.
    assert_eq!(
        parse_sgr_mouse("\x1b[<3;10;10m").map(|event| event.kind),
        Some(MouseKind::Release)
    );
    assert_eq!(
        parse_sgr_mouse("\x1b[<0;10;10M").map(|event| event.kind),
        Some(MouseKind::Press)
    );
}
