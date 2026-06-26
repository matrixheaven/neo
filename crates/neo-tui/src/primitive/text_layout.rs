//! Text measurement, wrapping, truncation, and clipping utilities.

use super::ansi_escape::{next_sequence, strip_ansi};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Visible width of a string (ANSI escapes skipped, unicode-width aware).
#[must_use]
pub fn visible_width(text: &str) -> usize {
    let mut width = 0;
    let mut index = 0;
    while index < text.len() {
        if let Some(sequence) = next_sequence(text, index) {
            index += sequence.len();
            continue;
        }

        let Some(grapheme) = text[index..].graphemes(true).next() else {
            break;
        };
        width += display_width(grapheme);
        index += grapheme.len();
    }
    width
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

#[must_use]
pub fn truncate_width(text: &str, max_width: usize, ellipsis: &str, pad: bool) -> String {
    if max_width == 0 {
        return String::new();
    }

    let text_width = visible_width(text);
    if text_width <= max_width {
        if pad {
            let mut padded = text.to_string();
            padded.push_str(&" ".repeat(max_width - text_width));
            return padded;
        }
        return text.to_string();
    }

    let ellipsis_width = visible_width(ellipsis);
    if ellipsis_width >= max_width {
        let clipped = clip_width(ellipsis, max_width);
        if pad {
            let clipped_width = visible_width(&clipped);
            return format!("{clipped}{}", " ".repeat(max_width - clipped_width));
        }
        return clipped;
    }

    let prefix_width = max_width - ellipsis_width;
    let prefix = clip_width(text, prefix_width);
    let mut truncated = format!("{prefix}{ellipsis}");
    if pad {
        let truncated_width = visible_width(&truncated);
        truncated.push_str(&" ".repeat(max_width - truncated_width));
    }
    truncated
}

#[must_use]
pub fn wrap_width(text: &str, max_width: usize) -> Vec<String> {
    wrap_width_with_indices(text, max_width)
        .into_iter()
        .map(|(_, line)| line)
        .collect()
}

/// Wrap `text` to `max_width` display columns and return each wrapped segment
/// with the char index in `text` where that segment starts.
#[must_use]
pub fn wrap_width_with_indices(text: &str, max_width: usize) -> Vec<(usize, String)> {
    if max_width == 0 {
        return vec![(0, String::new())];
    }

    let mut result = Vec::new();
    let mut char_index = 0;

    for logical_line in text.split('\n') {
        if logical_line.is_empty() {
            result.push((char_index, String::new()));
        } else {
            let mut current = String::new();
            let mut current_width = 0;
            let mut active_sgr = String::new();
            let mut byte_index = 0;
            let mut segment_start = char_index;

            while byte_index < logical_line.len() {
                if let Some(sequence) = next_sequence(logical_line, byte_index) {
                    current.push_str(sequence);
                    update_active_sgr(sequence, &mut active_sgr);
                    byte_index += sequence.len();
                    continue;
                }

                let Some(grapheme) = logical_line[byte_index..].graphemes(true).next() else {
                    break;
                };

                let grapheme_width = display_width(grapheme);
                if current_width > 0 && current_width + grapheme_width > max_width {
                    result.push((segment_start, std::mem::take(&mut current)));
                    segment_start = char_index;
                    current.push_str(&active_sgr);
                    current_width = 0;
                }

                current.push_str(grapheme);
                current_width += grapheme_width;
                byte_index += grapheme.len();
                char_index += grapheme.chars().count();
            }

            if !current.is_empty() {
                result.push((segment_start, current));
            }
        }
        char_index += 1; // for the '\n' separator
    }

    if result.is_empty() {
        result.push((0, String::new()));
    }
    result
}

pub(crate) fn update_active_sgr(sequence: &str, active_sgr: &mut String) {
    if !sequence.starts_with("\x1b[") || !sequence.ends_with('m') {
        return;
    }

    let action = sgr_style_action(sequence);
    if action.resets {
        active_sgr.clear();
    }
    if action.sets_style {
        active_sgr.push_str(sequence);
    }
}

struct SgrStyleAction {
    resets: bool,
    sets_style: bool,
}

fn sgr_style_action(sequence: &str) -> SgrStyleAction {
    let Some(parameters) = sequence
        .strip_prefix("\x1b[")
        .and_then(|sequence| sequence.strip_suffix('m'))
    else {
        return SgrStyleAction {
            resets: false,
            sets_style: false,
        };
    };

    let mut action = SgrStyleAction {
        resets: parameters.is_empty(),
        sets_style: false,
    };

    for parameter in parameters.split(';') {
        if parameter == "0" {
            action.resets = true;
        } else if !parameter.is_empty() {
            action.sets_style = true;
        }
    }

    action
}

fn clip_width(text: &str, max_width: usize) -> String {
    clip_visible_to_width(text, max_width)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn visible_width_ignores_cursor_marker() {
        let line = format!("> {}hello", crate::screen_output::CURSOR_MARKER);
        assert_eq!(visible_width(&line), "> hello".chars().count());
    }

    #[test]
    fn visible_width_ignores_dcs_with_st() {
        assert_eq!(visible_width("\x1bP\x1b\\hello"), 5);
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
}
