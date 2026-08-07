use super::*;

#[test]
fn feed_bytes_printable_chars_map_to_insert() {
    let cases: [(&str, &[u8], char); 3] = [
        ("cjk_character", "你".as_bytes(), '你'),
        ("space", b" ", ' '),
        ("fullwidth_symbol", "，".as_bytes(), '，'),
    ];
    for (name, bytes, expected) in cases {
        let mut parser = InputParser::with_keybindings(KeybindingsManager::default());
        let events = parser.feed_bytes(bytes);
        assert_eq!(events.len(), 1, "{name}: {events:?}");
        assert_eq!(events[0], InputEvent::Insert(expected), "{name}");
    }
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
