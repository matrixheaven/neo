use super::*;

#[test]
fn cursor_position_report_is_internal_and_chunk_safe() {
    let mut parser = InputParser::new();
    // CPR split across chunks must never surface as prompt input.
    assert!(parser.feed_bytes(b"\x1b[12;").is_empty());
    assert!(parser.feed_bytes(b"34R").is_empty());
    assert_eq!(parser.take_cursor_position(), Some((33, 11)));
    assert_eq!(parser.take_cursor_position(), None);
    assert_eq!(parser.feed_bytes(b"x"), vec![InputEvent::Insert('x')]);
}

#[test]
fn cursor_position_report_rejects_zero_based_wire_coordinates() {
    let mut parser = InputParser::new();
    assert!(parser.feed_bytes(b"\x1b[0;4R").is_empty());
    assert_eq!(parser.take_cursor_position(), None);
}
