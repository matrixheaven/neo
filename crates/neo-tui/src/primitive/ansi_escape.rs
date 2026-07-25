//! ANSI escape sequence builders and parsers.

use super::color::Color;
use super::style::{RESET, Style};

/// Convert a `Color` to an ANSI foreground escape sequence.
#[must_use]
pub fn fg_to_ansi(color: Color) -> String {
    ansi_color_sequence(color, 39, 30, 90, 38)
}

/// Convert a `Color` to an ANSI background escape sequence.
#[must_use]
pub fn bg_to_ansi(color: Color) -> String {
    ansi_color_sequence(color, 49, 40, 100, 48)
}

fn ansi_color_sequence(
    color: Color,
    reset: u8,
    normal_base: u8,
    bright_base: u8,
    dynamic: u8,
) -> String {
    if let Some((base, offset)) = named_color_slot(color, normal_base, bright_base) {
        return ansi_indexed_slot(base, offset);
    }
    match color {
        Color::Reset => format!("\x1b[{reset}m"),
        Color::Rgb(r, g, b) => format!("\x1b[{dynamic};2;{r};{g};{b}m"),
        Color::Indexed(n) => format!("\x1b[{dynamic};5;{n}m"),
        Color::Black
        | Color::Red
        | Color::Green
        | Color::Yellow
        | Color::Blue
        | Color::Magenta
        | Color::Cyan
        | Color::Gray
        | Color::DarkGray
        | Color::LightRed
        | Color::LightGreen
        | Color::LightYellow
        | Color::LightBlue
        | Color::LightMagenta
        | Color::LightCyan
        | Color::White => unreachable!("named colors are handled before dynamic colors"),
    }
}

fn named_color_slot(color: Color, normal_base: u8, bright_base: u8) -> Option<(u8, u8)> {
    let slot = match color {
        Color::Black => (normal_base, 0),
        Color::Red => (normal_base, 1),
        Color::Green => (normal_base, 2),
        Color::Yellow => (normal_base, 3),
        Color::Blue => (normal_base, 4),
        Color::Magenta => (normal_base, 5),
        Color::Cyan => (normal_base, 6),
        Color::Gray | Color::DarkGray => (bright_base, 0),
        Color::LightRed => (bright_base, 1),
        Color::LightGreen => (bright_base, 2),
        Color::LightYellow => (bright_base, 3),
        Color::LightBlue => (bright_base, 4),
        Color::LightMagenta => (bright_base, 5),
        Color::LightCyan => (bright_base, 6),
        Color::White => (bright_base, 7),
        Color::Reset | Color::Rgb(_, _, _) | Color::Indexed(_) => return None,
    };
    Some(slot)
}

fn ansi_indexed_slot(base: u8, offset: u8) -> String {
    format!("\x1b[{}m", base + offset)
}

/// Convert a `Style` to ANSI escape sequences (fg + bg + modifiers).
#[must_use]
pub fn style_to_ansi(style: Style) -> String {
    if style.is_empty() {
        return String::new();
    }
    let mut buf = String::new();
    if let Some(color) = style.fg
        && color != Color::Reset
    {
        buf.push_str(&fg_to_ansi(color));
    }
    if let Some(color) = style.bg
        && color != Color::Reset
    {
        buf.push_str(&bg_to_ansi(color));
    }
    if style.bold {
        buf.push_str("\x1b[1m");
    }
    if style.dim {
        buf.push_str("\x1b[2m");
    }
    if style.italic {
        buf.push_str("\x1b[3m");
    }
    if style.underline {
        buf.push_str("\x1b[4m");
    }
    if style.blink {
        buf.push_str("\x1b[5m");
    }
    if style.reversed {
        buf.push_str("\x1b[7m");
    }
    if style.crossed_out {
        buf.push_str("\x1b[9m");
    }
    buf
}

/// Apply a style to text: prefix with ANSI codes, suffix with RESET.
#[must_use]
pub fn paint(text: &str, style: Style) -> String {
    let ansi = style_to_ansi(style);
    if ansi.is_empty() {
        text.to_owned()
    } else {
        format!("{ansi}{text}{RESET}")
    }
}

/// Classification of ANSI string sequences introduced by `ESC <kind>` or a C1 control byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StringKind {
    Osc,
    Dcs,
    Sos,
    Apc,
    Pm,
}

/// State for the incremental ANSI control parser.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum AnsiParseState {
    /// Normal text; control characters are stripped or preserved.
    #[default]
    Ground,
    /// Saw `ESC` and are examining the next byte.
    Esc,
    /// Inside a `CSI` sequence (`ESC [` or C1 `CSI`).
    Csi,
    /// Inside a string sequence (OSC, DCS, SOS, APC, PM).
    String(StringKind),
    /// Inside a string sequence and saw `ESC`; the next byte determines `ST`.
    StringEsc(StringKind),
    /// Saw one of the `ESC ( ) * + - . /` introducers and need one more byte.
    SingleChar,
}

/// Incremental ANSI escape sequence parser.
///
/// `AnsiParser` can be fed input in arbitrary chunks. Sequences split across
/// chunk boundaries are held in parser state until they terminate or the parser
/// is finalized. Finalizing discards any pending control state so a trailing
/// partial sequence does not leak into later output.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AnsiParser {
    state: AnsiParseState,
}

impl AnsiParser {
    /// Create a new parser in the ground state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset the parser, discarding any pending control state.
    pub fn reset(&mut self) {
        self.state = AnsiParseState::Ground;
    }

    /// Finalize parsing, discarding any pending control state.
    pub fn finalize(&mut self) {
        self.reset();
    }

    /// Consume `input` and append visible characters to `output`.
    ///
    /// Newlines and tabs are preserved. CSI, OSC, DCS, APC, PM, SOS, C1
    /// controls, and other control characters are stripped. Sequences split
    /// across calls are held in parser state.
    ///
    /// Returns the number of visible characters appended to `output`.
    pub fn consume(&mut self, input: &str, output: &mut String) -> usize {
        let mut appended = 0;
        for ch in input.chars() {
            match self.state {
                AnsiParseState::Ground => {
                    if ch == '\x1b' {
                        self.state = AnsiParseState::Esc;
                    } else if ch == '\u{009b}' {
                        self.state = AnsiParseState::Csi;
                    } else if ch == '\u{009d}' {
                        self.state = AnsiParseState::String(StringKind::Osc);
                    } else if ch == '\u{0090}' {
                        self.state = AnsiParseState::String(StringKind::Dcs);
                    } else if ch == '\u{0098}' {
                        self.state = AnsiParseState::String(StringKind::Sos);
                    } else if ch == '\u{009f}' {
                        self.state = AnsiParseState::String(StringKind::Apc);
                    } else if ch == '\u{009e}' {
                        self.state = AnsiParseState::String(StringKind::Pm);
                    } else if ch == '\u{009c}' {
                        // C1 ST in ground state has no effect.
                    } else if ch.is_control() && !matches!(ch, '\n' | '\t') {
                        // Drop C0 controls (except newline and tab).
                    } else {
                        output.push(ch);
                        appended += 1;
                    }
                }
                AnsiParseState::Esc => match ch {
                    '[' => self.state = AnsiParseState::Csi,
                    ']' => self.state = AnsiParseState::String(StringKind::Osc),
                    'P' => self.state = AnsiParseState::String(StringKind::Dcs),
                    'X' => self.state = AnsiParseState::String(StringKind::Sos),
                    '_' => self.state = AnsiParseState::String(StringKind::Apc),
                    '^' => self.state = AnsiParseState::String(StringKind::Pm),
                    '(' | ')' | '*' | '+' | '-' | '.' | '/' => {
                        self.state = AnsiParseState::SingleChar;
                    }
                    '\u{009b}' => self.state = AnsiParseState::Csi,
                    _ => self.state = AnsiParseState::Ground,
                },
                AnsiParseState::Csi => {
                    if ('\x40'..='\x7e').contains(&ch) {
                        self.state = AnsiParseState::Ground;
                    }
                    // Otherwise remain in CSI until the final byte arrives.
                }
                AnsiParseState::String(kind) => {
                    if ch == '\x07' || ch == '\x18' || ch == '\x1a' || ch == '\u{009c}' {
                        self.state = AnsiParseState::Ground;
                    } else if ch == '\x1b' {
                        self.state = AnsiParseState::StringEsc(kind);
                    }
                }
                AnsiParseState::StringEsc(kind) => {
                    if ch == '\\' {
                        self.state = AnsiParseState::Ground;
                    } else {
                        // ESC was not followed by backslash, so it is part of
                        // the string and this byte is processed as string content.
                        self.state = AnsiParseState::String(kind);
                        if ch == '\x07' || ch == '\x18' || ch == '\x1a' || ch == '\u{009c}' {
                            self.state = AnsiParseState::Ground;
                        } else if ch == '\x1b' {
                            self.state = AnsiParseState::StringEsc(kind);
                        }
                    }
                }
                AnsiParseState::SingleChar => {
                    self.state = AnsiParseState::Ground;
                }
            }
        }
        appended
    }
}

/// If `s` starts with an ANSI escape sequence at byte `start`, return that sequence.
/// Mirrors the set of sequences handled by `strip_ansi`.
pub(crate) fn next_sequence(s: &str, start: usize) -> Option<&str> {
    let tail = s.get(start..)?;
    let mut chars = tail.chars().peekable();
    if chars.next()? != '\x1b' {
        return None;
    }
    match chars.peek() {
        Some('[') => {
            chars.next();
            let mut consumed = 2;
            for c in chars.by_ref() {
                consumed += c.len_utf8();
                if ('\x40'..='\x7e').contains(&c) {
                    return Some(&tail[..consumed]);
                }
            }
            Some(tail)
        }
        Some(']' | '_' | 'P' | '^' | 'X') => {
            chars.next();
            let mut consumed = 2;
            loop {
                match chars.next() {
                    None => return Some(tail),
                    Some(c) => {
                        consumed += c.len_utf8();
                        if c == '\x07'
                            || c == '\x18'
                            || c == '\x1a'
                            || c == '\u{009c}'
                            || (c == '\x1b' && chars.peek() == Some(&'\\'))
                        {
                            if c == '\x1b' {
                                let _ = chars.next();
                                consumed += 1;
                            }
                            return Some(&tail[..consumed]);
                        }
                    }
                }
            }
        }
        Some('(' | ')' | '*' | '+' | '-' | '.' | '/') => {
            chars.next();
            match chars.next() {
                None => Some(tail),
                Some(c) => {
                    let consumed = 2 + c.len_utf8();
                    Some(&tail[..consumed])
                }
            }
        }
        _ => match chars.next() {
            None => Some(tail),
            Some(c) => {
                let consumed = 1 + c.len_utf8();
                Some(&tail[..consumed])
            }
        },
    }
}

/// Strip ANSI escape sequences and unsafe terminal controls from visible text.
/// Newlines and tabs remain available to callers that own text layout.
#[must_use]
pub fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut parser = AnsiParser::new();
    parser.consume(s, &mut result);
    parser.finalize();
    result
}

#[cfg(test)]
mod tests {
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
}
