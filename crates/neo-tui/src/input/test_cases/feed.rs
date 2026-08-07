use super::*;

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
