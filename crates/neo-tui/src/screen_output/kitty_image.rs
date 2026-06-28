//! Kitty image management helpers extracted from `frame_differ`.
//!
//! These functions handle detection, extraction, reservation, and deletion of
//! inline kitty/iTerm image sequences embedded in rendered output lines.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::primitive::visible_width;

use super::frame_differ::{TuiRenderer, ViewportState};

const KITTY_SEQUENCE_PREFIX: &str = "\x1b_G";

pub(super) fn is_image_line(line: &str) -> bool {
    line.contains(KITTY_SEQUENCE_PREFIX) || line.contains("\x1b]1337;File=")
}

pub(super) fn collect_kitty_image_ids(lines: &[String]) -> BTreeSet<u32> {
    lines
        .iter()
        .flat_map(|line| extract_kitty_image_ids(line))
        .collect()
}

fn extract_kitty_image_ids(line: &str) -> Vec<u32> {
    let mut ids = Vec::new();
    let mut rest = line;
    while let Some(sequence_start) = rest.find(KITTY_SEQUENCE_PREFIX) {
        rest = &rest[sequence_start + KITTY_SEQUENCE_PREFIX.len()..];
        let Some(params_end) = rest.find(';') else {
            break;
        };
        let params = &rest[..params_end];
        for param in params.split(',') {
            let Some((key, value)) = param.split_once('=') else {
                continue;
            };
            if key == "i"
                && let Ok(id) = value.parse::<u32>()
                && id > 0
            {
                ids.push(id);
            }
        }
        rest = &rest[params_end + 1..];
    }
    ids
}

fn extract_kitty_image_rows(line: &str) -> usize {
    let Some(sequence_start) = line.find(KITTY_SEQUENCE_PREFIX) else {
        return 1;
    };
    let params_start = sequence_start + KITTY_SEQUENCE_PREFIX.len();
    let Some(params_end) = line[params_start..].find(';') else {
        return 1;
    };
    let params = &line[params_start..params_start + params_end];
    for param in params.split(',') {
        let Some((key, value)) = param.split_once('=') else {
            continue;
        };
        if key == "r"
            && let Ok(rows) = value.parse::<usize>()
        {
            return rows.max(1);
        }
    }
    1
}

pub(super) fn get_kitty_image_reserved_rows(
    lines: &[String],
    index: usize,
    max_index: usize,
) -> usize {
    let rows = extract_kitty_image_rows(lines.get(index).map_or("", String::as_str));
    if rows <= 1 {
        return 1;
    }
    let max_rows = rows
        .min(max_index.saturating_sub(index) + 1)
        .min(lines.len() - index);
    let mut reserved_rows = 1;
    while reserved_rows < max_rows {
        let line = lines.get(index + reserved_rows).map_or("", String::as_str);
        if is_image_line(line) || visible_width(line) > 0 {
            break;
        }
        reserved_rows += 1;
    }
    reserved_rows
}

pub(super) fn delete_kitty_images(ids: &BTreeSet<u32>) -> String {
    let mut buffer = String::new();
    for id in ids {
        let _ = write!(buffer, "\x1b_Ga=d,d=I,i={id},q=2\x1b\\");
    }
    buffer
}

pub(super) fn reserved_render_rows(lines: &[String], index: usize, render_end: usize) -> usize {
    if is_image_line(&lines[index]) {
        get_kitty_image_reserved_rows(lines, index, render_end)
    } else {
        1
    }
}

pub(super) fn image_block_fits(
    index: usize,
    image_reserved_rows: usize,
    viewport: ViewportState,
    height: usize,
) -> bool {
    let image_start_screen_row = index.cast_signed() - viewport.top.cast_signed();
    image_start_screen_row >= 0
        && image_start_screen_row.cast_unsigned() + image_reserved_rows <= height
}

pub(super) fn push_image_block(buffer: &mut String, line: &str, image_reserved_rows: usize) {
    buffer.push_str("\x1b[2K");
    for _ in 1..image_reserved_rows {
        buffer.push_str("\r\n\x1b[2K");
    }
    let _ = write!(buffer, "\x1b[{}A", image_reserved_rows - 1);
    buffer.push_str(line);
    let _ = write!(buffer, "\x1b[{}B", image_reserved_rows - 1);
}

impl TuiRenderer {
    pub(super) fn expand_changed_range_for_kitty_images(
        &self,
        first_changed: usize,
        last_changed: usize,
        new_lines: &[String],
    ) -> (usize, usize) {
        let mut expanded_first = first_changed;
        let mut expanded_last = last_changed;

        for lines in [&self.previous_lines[..], new_lines] {
            for index in 0..lines.len() {
                if extract_kitty_image_ids(&lines[index]).is_empty() {
                    continue;
                }
                let block_end =
                    index + get_kitty_image_reserved_rows(lines, index, lines.len() - 1) - 1;
                if index >= first_changed || (index <= last_changed && block_end >= first_changed) {
                    expanded_first = expanded_first.min(index);
                    expanded_last = expanded_last.max(block_end);
                }
            }
        }

        (expanded_first, expanded_last)
    }

    pub(super) fn delete_changed_kitty_images(
        &self,
        first_changed: usize,
        last_changed: usize,
    ) -> String {
        if last_changed < first_changed {
            return String::new();
        }
        let mut ids = BTreeSet::new();
        let max_line = last_changed.min(self.previous_lines.len().saturating_sub(1));
        for index in first_changed..=max_line {
            for id in
                extract_kitty_image_ids(self.previous_lines.get(index).map_or("", String::as_str))
            {
                ids.insert(id);
            }
        }
        delete_kitty_images(&ids)
    }
}
