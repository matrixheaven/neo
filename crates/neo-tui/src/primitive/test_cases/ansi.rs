use super::*;

#[test]
fn foreground_and_background_named_colors_use_matching_ansi_slots() {
    let cases = [
        (Color::Reset, "\x1b[39m", "\x1b[49m"),
        (Color::Black, "\x1b[30m", "\x1b[40m"),
        (Color::Red, "\x1b[31m", "\x1b[41m"),
        (Color::Green, "\x1b[32m", "\x1b[42m"),
        (Color::Yellow, "\x1b[33m", "\x1b[43m"),
        (Color::Blue, "\x1b[34m", "\x1b[44m"),
        (Color::Magenta, "\x1b[35m", "\x1b[45m"),
        (Color::Cyan, "\x1b[36m", "\x1b[46m"),
        (Color::Gray, "\x1b[90m", "\x1b[100m"),
        (Color::DarkGray, "\x1b[90m", "\x1b[100m"),
        (Color::LightRed, "\x1b[91m", "\x1b[101m"),
        (Color::LightGreen, "\x1b[92m", "\x1b[102m"),
        (Color::LightYellow, "\x1b[93m", "\x1b[103m"),
        (Color::LightBlue, "\x1b[94m", "\x1b[104m"),
        (Color::LightMagenta, "\x1b[95m", "\x1b[105m"),
        (Color::LightCyan, "\x1b[96m", "\x1b[106m"),
        (Color::White, "\x1b[97m", "\x1b[107m"),
    ];

    for (color, expected_fg, expected_bg) in cases {
        assert_eq!(fg_to_ansi(color), expected_fg);
        assert_eq!(bg_to_ansi(color), expected_bg);
    }
}

#[test]
fn dynamic_colors_use_foreground_and_background_prefixes() {
    assert_eq!(fg_to_ansi(Color::Rgb(1, 2, 3)), "\x1b[38;2;1;2;3m");
    assert_eq!(bg_to_ansi(Color::Rgb(1, 2, 3)), "\x1b[48;2;1;2;3m");
    assert_eq!(fg_to_ansi(Color::Indexed(42)), "\x1b[38;5;42m");
    assert_eq!(bg_to_ansi(Color::Indexed(42)), "\x1b[48;5;42m");
}

#[test]
fn style_to_ansi_combines() {
    let style = Style::default().fg(Color::Red).bold();
    let ansi = style_to_ansi(style);
    assert!(ansi.contains("\x1b[31m"));
    assert!(ansi.contains("\x1b[1m"));
}

#[test]
fn empty_style_produces_nothing() {
    assert!(style_to_ansi(Style::default()).is_empty());
}

#[test]
fn paint_wraps_with_reset() {
    let styled = paint("hello", Style::default().fg(Color::Blue));
    assert!(styled.starts_with("\x1b[34m"));
    assert!(styled.ends_with(RESET));
}

#[test]
fn strip_ansi_removes_cursor_marker() {
    assert_eq!(strip_ansi(crate::screen_output::CURSOR_MARKER), "");
}

#[test]
fn strip_ansi_removes_dcs_pm_sos_apc_with_st() {
    assert_eq!(strip_ansi("\x1bPpayload\x1b\\"), "");
    assert_eq!(strip_ansi("\x1b^payload\x1b\\"), "");
    assert_eq!(strip_ansi("\x1bXpayload\x1b\\"), "");
    assert_eq!(strip_ansi("\x1b_payload\x1b\\"), "");
}

#[test]
fn strip_ansi_string_sequences_cancel_on_can_sub() {
    assert_eq!(strip_ansi("\x1b]osc\x18visible"), "visible");
    assert_eq!(strip_ansi("\x1b_apc\x1avisible"), "visible");
}

#[test]
fn strip_ansi_string_sequences_terminate_on_c1_st() {
    assert_eq!(strip_ansi("\x1b]osc\u{009c}visible"), "visible");
    assert_eq!(strip_ansi("\x1bPpayload\u{009c}visible"), "visible");
    assert_eq!(strip_ansi("\x1b^payload\u{009c}visible"), "visible");
    assert_eq!(strip_ansi("\x1bXpayload\u{009c}visible"), "visible");
    assert_eq!(strip_ansi("\x1b_payload\u{009c}visible"), "visible");
}

#[test]
fn strip_ansi_empty_string() {
    assert_eq!(strip_ansi(""), "");
}

#[test]
fn strip_ansi_no_ansi_preserved() {
    assert_eq!(strip_ansi("hello 世界"), "hello 世界");
}

#[test]
fn strip_ansi_trailing_esc_removed() {
    assert_eq!(strip_ansi("text\x1b"), "text");
}

#[test]
fn strip_ansi_unknown_two_char_sequence_removed() {
    assert_eq!(strip_ansi("a\x1bDb"), "ab");
}

#[test]
fn strip_ansi_multibyte_after_esc_does_not_panic() {
    // ESC followed by a multi-byte codepoint is not a valid ANSI sequence,
    // but the parser must not panic on a non-char-boundary slice.
    assert_eq!(strip_ansi("a\x1b中b"), "ab");
}

#[test]
fn strip_ansi_osc_terminated_by_bel() {
    assert_eq!(strip_ansi("\x1b]0;title\x07visible"), "visible");
}

#[test]
fn parser_consumes_split_csi_across_chunks() {
    let mut parser = AnsiParser::new();
    let mut out = String::new();
    parser.consume("\x1b[3", &mut out);
    assert_eq!(out, "");
    parser.consume("1mred\x1b[0m", &mut out);
    assert_eq!(out, "red");
    parser.finalize();
}

#[test]
fn parser_consumes_split_osc_across_chunks() {
    let mut parser = AnsiParser::new();
    let mut out = String::new();
    parser.consume("\x1b]0;ti", &mut out);
    assert_eq!(out, "");
    parser.consume("tle\x07hello", &mut out);
    assert_eq!(out, "hello");
    parser.finalize();
}

#[test]
fn parser_consumes_split_dcs_apc_pm_sos_across_chunks() {
    for (intro, kind) in [('P', "DCS"), ('_', "APC"), ('^', "PM"), ('X', "SOS")] {
        let mut parser = AnsiParser::new();
        let mut out = String::new();
        parser.consume(&format!("\x1b{intro}pay"), &mut out);
        assert_eq!(out, "", "{kind} partial should hold");
        parser.consume("load\x1b\\visible", &mut out);
        assert_eq!(out, "visible", "{kind} termination failed");
        parser.finalize();
    }
}

#[test]
fn parser_finalizes_away_trailing_partial_sequence() {
    let mut parser = AnsiParser::new();
    let mut out = String::new();
    parser.consume("text\x1b[", &mut out);
    parser.finalize();
    assert_eq!(out, "text");
}

#[test]
fn parser_resets_pending_control_state() {
    let mut parser = AnsiParser::new();
    let mut out = String::new();
    parser.consume("\x1b[", &mut out);
    parser.reset();
    parser.consume("visible", &mut out);
    assert_eq!(out, "visible");
}
