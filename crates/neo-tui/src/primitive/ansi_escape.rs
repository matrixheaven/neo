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
    let mut index = 0;
    while index < s.len() {
        if let Some(seq) = next_sequence(s, index) {
            index += seq.len();
        } else {
            let ch = s[index..].chars().next().unwrap();
            if !ch.is_control() || matches!(ch, '\n' | '\t') {
                result.push(ch);
            }
            index += ch.len_utf8();
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_foreground() {
        assert_eq!(fg_to_ansi(Color::Rgb(255, 0, 0)), "\x1b[38;2;255;0;0m");
    }

    #[test]
    fn named_colors() {
        assert_eq!(fg_to_ansi(Color::Green), "\x1b[32m");
        assert_eq!(fg_to_ansi(Color::White), "\x1b[97m");
    }

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
}
