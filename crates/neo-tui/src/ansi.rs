//! ANSI escape code helpers for the custom diff renderer.
//! Self-contained ANSI utilities.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Reset all attributes to terminal default.
pub const RESET: &str = "\x1b[0m";

/// A color value for rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Reset,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Gray,
    DarkGray,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightMagenta,
    LightCyan,
    White,
    Rgb(u8, u8, u8),
    Indexed(u8),
}

/// A rectangular region used for layout calculations.
///
/// A simple rectangle used for layout calculations.
/// from external dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    #[must_use]
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    #[must_use]
    pub const fn bottom(&self) -> u16 {
        self.y + self.height
    }

    #[must_use]
    pub const fn right(&self) -> u16 {
        self.x + self.width
    }

    #[must_use]
    pub const fn area(&self) -> u32 {
        self.width as u32 * self.height as u32
    }
}

/// A text style for rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub blink: bool,
    pub reversed: bool,
    pub crossed_out: bool,
}

impl Style {
    #[must_use]
    pub fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    #[must_use]
    pub fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    #[must_use]
    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    #[must_use]
    pub fn dim(mut self) -> Self {
        self.dim = true;
        self
    }

    #[must_use]
    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    #[must_use]
    pub fn underline(mut self) -> Self {
        self.underline = true;
        self
    }

    #[must_use]
    pub fn crossed_out(mut self) -> Self {
        self.crossed_out = true;
        self
    }

    /// Is this the default (empty) style?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Style::default()
    }
}

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

/// Strip ANSI escape sequences from a string and return the visible text.
#[must_use]
pub fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut index = 0;
    while index < s.len() {
        if let Some(seq) = next_sequence(s, index) {
            index += seq.len();
        } else {
            let ch = s[index..].chars().next().unwrap();
            result.push(ch);
            index += ch.len_utf8();
        }
    }
    result
}

/// Visible width of a string (ANSI escapes stripped, unicode-width aware).
#[must_use]
pub fn visible_width(s: &str) -> usize {
    display_width(&strip_ansi(s))
}

#[must_use]
pub(crate) fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

#[must_use]
pub(crate) fn clip_plain_to_width(text: &str, max_width: usize) -> String {
    let mut clipped = String::new();
    let mut width = 0;
    for grapheme in text.graphemes(true) {
        let grapheme_width = display_width(grapheme);
        if width + grapheme_width > max_width {
            break;
        }
        clipped.push_str(grapheme);
        width += grapheme_width;
    }
    clipped
}

#[must_use]
pub(crate) fn clip_visible_to_width(text: &str, max_width: usize) -> String {
    let mut clipped = String::new();
    let mut width = 0;
    let mut index = 0;
    while index < text.len() {
        if let Some(sequence) = next_sequence(text, index) {
            clipped.push_str(sequence);
            index += sequence.len();
            continue;
        }

        let Some(grapheme) = text[index..].graphemes(true).next() else {
            break;
        };
        let grapheme_width = display_width(grapheme);
        if width + grapheme_width > max_width {
            break;
        }
        clipped.push_str(grapheme);
        width += grapheme_width;
        index += grapheme.len();
    }
    clipped
}

/// Wrap plain text (no ANSI) to a maximum visible width.
/// Uses word-wrapping by default, but hard-wraps words that exceed `width`.
#[must_use]
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_owned()];
    }
    let mut result = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            result.push(String::new());
            continue;
        }
        let mut current = String::new();
        let mut current_width = 0usize;
        for word in paragraph.split(' ') {
            let word_width = display_width(word);

            // Hard-wrap words that are wider than the available width
            if word_width > width {
                // Push any accumulated content first
                if !current.is_empty() {
                    result.push(std::mem::take(&mut current));
                    current_width = 0;
                }
                // Hard-wrap the long word
                let mut line = String::new();
                let mut line_w = 0usize;
                for grapheme in word.graphemes(true) {
                    let grapheme_width = display_width(grapheme);
                    if line_w + grapheme_width > width && !line.is_empty() {
                        result.push(std::mem::take(&mut line));
                        line_w = 0;
                    }
                    line.push_str(grapheme);
                    line_w += grapheme_width;
                }
                if !line.is_empty() {
                    current = line;
                    current_width = line_w;
                }
                continue;
            }

            if current_width == 0 {
                word.clone_into(&mut current);
                current_width = word_width;
            } else if current_width + 1 + word_width <= width {
                current.push(' ');
                current.push_str(word);
                current_width += 1 + word_width;
            } else {
                result.push(std::mem::take(&mut current));
                word.clone_into(&mut current);
                current_width = word_width;
            }
        }
        if !current.is_empty() || result.is_empty() {
            result.push(current);
        }
    }
    result
}

/// Pad a string with spaces to fill exactly `width` visible columns.
#[must_use]
pub fn pad_to_width(text: &str, width: usize) -> String {
    let vis = visible_width(text);
    if vis >= width {
        text.to_owned()
    } else {
        format!("{text}{}", " ".repeat(width - vis))
    }
}

/// Truncate a string to `width` visible columns, appending "…" if truncated.
#[must_use]
pub fn truncate_to_width(text: &str, width: usize) -> String {
    let stripped = strip_ansi(text);
    let w = display_width(&stripped);
    if w <= width {
        return text.to_owned();
    }
    let mut result = clip_plain_to_width(&stripped, width.saturating_sub(1));
    if width > 0 {
        result.push('…');
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
    fn visible_width_strips_ansi() {
        assert_eq!(visible_width("\x1b[31mhello\x1b[0m"), 5);
    }

    #[test]
    fn visible_width_treats_emoji_presentation_as_one_display_unit() {
        assert_eq!(visible_width("⚠️"), 2);
        assert_eq!(visible_width("a⚠️b"), 4);
        assert_eq!(clip_plain_to_width("a⚠️b", 3), "a⚠️");
        assert_eq!(clip_plain_to_width("a⚠️b", 2), "a");
    }

    #[test]
    fn wrap_text_basic() {
        assert_eq!(wrap_text("hello world", 5), vec!["hello", "world"]);
    }

    #[test]
    fn wrap_text_preserves_empty_lines() {
        assert_eq!(wrap_text("a\n\nb", 80), vec!["a", "", "b"]);
    }

    #[test]
    fn pad_to_width_adds_spaces() {
        assert_eq!(pad_to_width("hi", 5), "hi   ");
    }

    #[test]
    fn truncate_adds_ellipsis() {
        assert_eq!(truncate_to_width("hello world", 8), "hello w…");
    }

    #[test]
    fn strip_ansi_removes_cursor_marker() {
        assert_eq!(strip_ansi(crate::terminal::CURSOR_MARKER), "");
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
    fn visible_width_ignores_cursor_marker() {
        let line = format!("> {}hello", crate::terminal::CURSOR_MARKER);
        assert_eq!(visible_width(&line), "> hello".chars().count());
    }

    #[test]
    fn visible_width_ignores_dcs_with_st() {
        assert_eq!(visible_width("\x1bP\x1b\\hello"), 5);
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
