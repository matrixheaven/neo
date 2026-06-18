//! ANSI escape code helpers for the custom diff renderer.
//! Self-contained ANSI utilities.

use unicode_width::UnicodeWidthChar;

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

    /// Is this the default (empty) style?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Style::default()
    }
}

/// Convert a `Color` to an ANSI foreground escape sequence.
#[must_use]
pub fn fg_to_ansi(color: Color) -> String {
    match color {
        Color::Reset => "\x1b[39m".to_owned(),
        Color::Black => "\x1b[30m".to_owned(),
        Color::Red => "\x1b[31m".to_owned(),
        Color::Green => "\x1b[32m".to_owned(),
        Color::Yellow => "\x1b[33m".to_owned(),
        Color::Blue => "\x1b[34m".to_owned(),
        Color::Magenta => "\x1b[35m".to_owned(),
        Color::Cyan => "\x1b[36m".to_owned(),
        Color::Gray => "\x1b[90m".to_owned(),
        Color::DarkGray => "\x1b[90m".to_owned(),
        Color::LightRed => "\x1b[91m".to_owned(),
        Color::LightGreen => "\x1b[92m".to_owned(),
        Color::LightYellow => "\x1b[93m".to_owned(),
        Color::LightBlue => "\x1b[94m".to_owned(),
        Color::LightMagenta => "\x1b[95m".to_owned(),
        Color::LightCyan => "\x1b[96m".to_owned(),
        Color::White => "\x1b[97m".to_owned(),
        Color::Rgb(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m"),
        Color::Indexed(n) => format!("\x1b[38;5;{n}m"),
    }
}

/// Convert a `Color` to an ANSI background escape sequence.
#[must_use]
pub fn bg_to_ansi(color: Color) -> String {
    match color {
        Color::Reset => "\x1b[49m".to_owned(),
        Color::Black => "\x1b[40m".to_owned(),
        Color::Red => "\x1b[41m".to_owned(),
        Color::Green => "\x1b[42m".to_owned(),
        Color::Yellow => "\x1b[43m".to_owned(),
        Color::Blue => "\x1b[44m".to_owned(),
        Color::Magenta => "\x1b[45m".to_owned(),
        Color::Cyan => "\x1b[46m".to_owned(),
        Color::Gray => "\x1b[100m".to_owned(),
        Color::DarkGray => "\x1b[100m".to_owned(),
        Color::LightRed => "\x1b[101m".to_owned(),
        Color::LightGreen => "\x1b[102m".to_owned(),
        Color::LightYellow => "\x1b[103m".to_owned(),
        Color::LightBlue => "\x1b[104m".to_owned(),
        Color::LightMagenta => "\x1b[105m".to_owned(),
        Color::LightCyan => "\x1b[106m".to_owned(),
        Color::White => "\x1b[107m".to_owned(),
        Color::Rgb(r, g, b) => format!("\x1b[48;2;{r};{g};{b}m"),
        Color::Indexed(n) => format!("\x1b[48;5;{n}m"),
    }
}

/// Convert a `Style` to ANSI escape sequences (fg + bg + modifiers).
#[must_use]
pub fn style_to_ansi(style: Style) -> String {
    if style.is_empty() {
        return String::new();
    }
    let mut buf = String::new();
    if let Some(color) = style.fg {
        if color != Color::Reset {
            buf.push_str(&fg_to_ansi(color));
        }
    }
    if let Some(color) = style.bg {
        if color != Color::Reset {
            buf.push_str(&bg_to_ansi(color));
        }
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
    strip_ansi(s).chars().map(|c| c.width().unwrap_or(0)).sum()
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
            let word_width: usize = word.chars().map(|c| c.width().unwrap_or(0)).sum();

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
                for c in word.chars() {
                    let cw = c.width().unwrap_or(0);
                    if line_w + cw > width && !line.is_empty() {
                        result.push(std::mem::take(&mut line));
                        line_w = 0;
                    }
                    line.push(c);
                    line_w += cw;
                }
                if !line.is_empty() {
                    current = line;
                    current_width = line_w;
                }
                continue;
            }

            if current_width == 0 {
                current = word.to_owned();
                current_width = word_width;
            } else if current_width + 1 + word_width <= width {
                current.push(' ');
                current.push_str(word);
                current_width += 1 + word_width;
            } else {
                result.push(std::mem::take(&mut current));
                current = word.to_owned();
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
    let w: usize = stripped.chars().map(|c| c.width().unwrap_or(0)).sum();
    if w <= width {
        return text.to_owned();
    }
    let mut result = String::new();
    let mut current = 0usize;
    for c in stripped.chars() {
        let cw = c.width().unwrap_or(0);
        if current + cw > width.saturating_sub(1) {
            break;
        }
        result.push(c);
        current += cw;
    }
    if width > 0 {
        result.push('…');
    }
    result
}

/// A styled line of text (content + style).
#[derive(Debug, Clone, Default)]
pub struct StyledLine {
    pub text: String,
    pub style: Style,
}

impl StyledLine {
    #[must_use]
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    /// Convert to a single ANSI-styled string.
    #[must_use]
    pub fn to_ansi(&self) -> String {
        paint(&self.text, self.style)
    }
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
        assert_eq!(strip_ansi(crate::pi_tui::CURSOR_MARKER), "");
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
        let line = format!("> {}hello", crate::pi_tui::CURSOR_MARKER);
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
