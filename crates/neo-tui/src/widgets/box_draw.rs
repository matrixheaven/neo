use crate::ansi::{RESET, Style, clip_visible_to_width, paint, visible_width};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoxSpec {
    pub top_left: char,
    pub top_right: char,
    pub bottom_left: char,
    pub bottom_right: char,
    pub horizontal: char,
    pub vertical: char,
}

pub const ROUNDED: BoxSpec = BoxSpec {
    top_left: '╭',
    top_right: '╮',
    bottom_left: '╰',
    bottom_right: '╯',
    horizontal: '─',
    vertical: '│',
};

pub(crate) fn repeat_char(ch: char, n: usize) -> String {
    std::iter::repeat_n(ch, n).collect()
}

fn clip_to_width(text: &str, max_width: usize) -> String {
    clip_visible_to_width(text, max_width)
}

#[must_use]
pub fn top_border(width: usize, style: Style) -> String {
    if width < 2 {
        return String::new();
    }
    let inner = width - 2;
    format!(
        "{}{}{}",
        paint(&ROUNDED.top_left.to_string(), style),
        paint(&repeat_char(ROUNDED.horizontal, inner), style),
        paint(&ROUNDED.top_right.to_string(), style)
    )
}

#[must_use]
pub fn bottom_border(width: usize, style: Style) -> String {
    if width < 2 {
        return String::new();
    }
    let inner = width - 2;
    format!(
        "{}{}{}",
        paint(&ROUNDED.bottom_left.to_string(), style),
        paint(&repeat_char(ROUNDED.horizontal, inner), style),
        paint(&ROUNDED.bottom_right.to_string(), style)
    )
}

/// Content row: left border + content (may contain ANSI styles) + padding + right border.
/// `width` is the full visible row width including both borders.
#[must_use]
pub fn content_line(content: &str, width: usize, border_style: Style) -> String {
    if width < 2 {
        return String::new();
    }
    let inner = width - 2;
    let clipped = clip_to_width(content, inner);
    let pad = inner.saturating_sub(visible_width(&clipped));
    format!(
        "{}{}{}{}{}",
        paint(&ROUNDED.vertical.to_string(), border_style),
        clipped,
        RESET,
        " ".repeat(pad),
        paint(&ROUNDED.vertical.to_string(), border_style)
    )
}

/// Row with only left/right borders (no top/bottom), used for dropdown lists.
#[must_use]
pub fn side_bordered_line(content: &str, width: usize, border_style: Style) -> String {
    content_line(content, width, border_style)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi::Style;

    #[test]
    fn rounded_box_width_exact() {
        let w = 40;
        let style = Style::default();
        assert_eq!(visible_width(&top_border(w, style)), w);
        assert_eq!(visible_width(&bottom_border(w, style)), w);
        assert_eq!(visible_width(&content_line("hello", w, style)), w);
    }

    #[test]
    fn content_line_pads_with_ansi_content() {
        let w = 20;
        let style = Style::default();
        let red = crate::ansi::Style::default().fg(crate::ansi::Color::Red);
        let content = format!("{}{}", crate::ansi::paint("hi", red), "x");
        let line = content_line(&content, w, style);
        assert_eq!(visible_width(&line), w);
    }

    #[test]
    fn narrow_width_returns_empty() {
        let style = Style::default();
        assert!(top_border(0, style).is_empty());
        assert!(top_border(1, style).is_empty());
        assert!(bottom_border(0, style).is_empty());
        assert!(bottom_border(1, style).is_empty());
        assert!(content_line("x", 0, style).is_empty());
        assert!(content_line("x", 1, style).is_empty());
    }

    #[test]
    fn width_two_has_only_borders() {
        let style = Style::default();
        assert_eq!(visible_width(&content_line("", 2, style)), 2);
        assert_eq!(visible_width(&content_line("a", 2, style)), 2);
    }

    #[test]
    fn empty_content_is_padded() {
        let w = 12;
        let style = Style::default();
        let line = content_line("", w, style);
        assert_eq!(visible_width(&line), w);
        // Reset is always appended after the (empty) clipped content.
        assert_eq!(line, format!("│{RESET}          │"));
    }

    #[test]
    fn unicode_and_fullwidth_content() {
        let w = 12;
        let style = Style::default();
        let line = content_line("中文", w, style);
        assert_eq!(visible_width(&line), w);
        // Inner width is 10; "中文" is 4 columns, so 6 spaces of padding.
        assert!(line.contains("中文"));
        assert_eq!(line.matches(' ').count(), 6);
    }

    #[test]
    fn content_longer_than_inner_is_clipped() {
        let w = 10;
        let style = Style::default();
        let line = content_line("hello world", w, style);
        assert_eq!(visible_width(&line), w);
        assert!(line.contains("hello wo"));
        assert!(!line.contains("rld"));
    }

    #[test]
    fn unterminated_ansi_is_reset_before_padding() {
        let w = 20;
        let style = Style::default();
        let content = "\x1b[31mhi";
        let line = content_line(content, w, style);
        assert_eq!(visible_width(&line), w);
        // The reset sequence must appear before the padding spaces / right border.
        assert!(line.contains("\x1b[31mhi\x1b[0m "));
        assert!(line.ends_with('│'));
    }
}
