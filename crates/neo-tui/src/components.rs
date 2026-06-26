use crate::ansi::{clip_visible_to_width, display_width, next_sequence};
use unicode_segmentation::UnicodeSegmentation;

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
