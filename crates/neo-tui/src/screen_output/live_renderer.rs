use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::Write;

use crate::primitive::visible_width;

use super::kitty_image::{collect_kitty_image_ids, delete_kitty_images};
use super::types::CursorPos;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveRenderer {
    width: u16,
    height: u16,
    previous_lines: Vec<String>,
    previous_cursor: Option<CursorPos>,
    hardware_cursor_row: usize,
    previous_kitty_image_ids: BTreeSet<u32>,
    full_redraw_pending: bool,
    fresh_anchor_pending: bool,
}

impl LiveRenderer {
    #[must_use]
    pub const fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            previous_lines: Vec::new(),
            previous_cursor: None,
            hardware_cursor_row: 0,
            previous_kitty_image_ids: BTreeSet::new(),
            full_redraw_pending: false,
            fresh_anchor_pending: false,
        }
    }

    pub(crate) fn resize(&mut self, width: u16, height: u16) {
        if self.width == width && self.height == height {
            return;
        }
        let width_changed = self.width != width;
        let height_changed = self.height != height;
        let anchor_is_recoverable = !height_changed
            && self.previous_lines.len() <= usize::from(height)
            && self.previous_kitty_image_ids.is_empty()
            && (!width_changed
                || self
                    .previous_lines
                    .iter()
                    .all(|line| visible_width(line) < usize::from(width)));
        self.width = width;
        self.height = height;
        if !self.previous_lines.is_empty() && (!anchor_is_recoverable || self.fresh_anchor_pending)
        {
            self.full_redraw_pending = false;
            self.fresh_anchor_pending = true;
        } else {
            self.full_redraw_pending = true;
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the renderer validates, diffs, and commits one transactional live frame"
    )]
    pub fn render_to(
        &mut self,
        output: &mut dyn Write,
        lines: Vec<String>,
        cursor: Option<CursorPos>,
    ) -> std::io::Result<()> {
        if lines.len() > usize::from(self.height) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "live frame has {} rows but terminal height is {}",
                    lines.len(),
                    self.height
                ),
            ));
        }
        if let Some((index, line_width)) = lines.iter().enumerate().find_map(|(index, line)| {
            let line_width = visible_width(line);
            (line_width > usize::from(self.width)).then_some((index, line_width))
        }) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "live row {index} width {line_width} exceeds terminal width {}",
                    self.width
                ),
            ));
        }
        if !self.full_redraw_pending
            && !self.fresh_anchor_pending
            && self.previous_lines == lines
            && self.previous_cursor == cursor
        {
            return Ok(());
        }

        let previous_lines = if self.full_redraw_pending || self.fresh_anchor_pending {
            &[][..]
        } else {
            self.previous_lines.as_slice()
        };
        let first_changed = previous_lines
            .iter()
            .zip(&lines)
            .position(|(previous, next)| previous != next)
            .unwrap_or_else(|| previous_lines.len().min(lines.len()));
        let render_rows = if previous_lines.len() == lines.len() {
            previous_lines
                .iter()
                .zip(&lines)
                .rposition(|(previous, next)| previous != next)
                .map_or(first_changed, |last_changed| last_changed + 1)
        } else {
            previous_lines.len().max(lines.len())
        };
        let append_start = first_changed == previous_lines.len()
            && first_changed > 0
            && lines.len() > previous_lines.len();
        let kitty_image_ids = collect_kitty_image_ids(&lines);
        let mut bytes = String::new();
        let mut hardware_cursor_row = self.hardware_cursor_row;
        if self.fresh_anchor_pending {
            bytes.push_str(&delete_kitty_images(&self.previous_kitty_image_ids));
            bytes.push_str("\r\n");
            hardware_cursor_row = 0;
        } else if self.full_redraw_pending {
            bytes.push_str(&delete_kitty_images(&self.previous_kitty_image_ids));
            push_vertical_move(&mut bytes, hardware_cursor_row, 0);
            bytes.push_str("\r\x1b[J");
            hardware_cursor_row = 0;
        } else {
            bytes.push_str(&delete_kitty_images(
                &self
                    .previous_kitty_image_ids
                    .difference(&kitty_image_ids)
                    .copied()
                    .collect(),
            ));
        }

        if first_changed < render_rows {
            let move_target = if append_start {
                first_changed - 1
            } else {
                first_changed
            };
            push_vertical_move(&mut bytes, hardware_cursor_row, move_target);
            hardware_cursor_row = move_target;
            if append_start {
                bytes.push_str("\r\n");
                hardware_cursor_row = hardware_cursor_row.saturating_add(1);
            }
            for row in first_changed..render_rows {
                if row > first_changed {
                    bytes.push_str("\r\n");
                    hardware_cursor_row = hardware_cursor_row.saturating_add(1);
                }
                bytes.push_str("\r\x1b[2K");
                if let Some(line) = lines.get(row) {
                    bytes.push_str(line);
                }
            }
        }

        let target_row =
            cursor.map_or_else(|| lines.len().saturating_sub(1), |position| position.row);
        push_vertical_move(&mut bytes, hardware_cursor_row, target_row);
        hardware_cursor_row = target_row;
        if let Some(cursor) = cursor {
            bytes.push('\r');
            if cursor.col > 0 {
                let _ = write!(bytes, "\x1b[{}C", cursor.col);
            }
        }
        bytes.push_str(if cursor.is_some() {
            "\x1b[?25h"
        } else {
            "\x1b[?25l"
        });

        output.write_all(bytes.as_bytes())?;
        output.flush()?;
        self.previous_lines = lines;
        self.previous_cursor = cursor;
        self.hardware_cursor_row = hardware_cursor_row;
        self.previous_kitty_image_ids = kitty_image_ids;
        self.full_redraw_pending = false;
        self.fresh_anchor_pending = false;
        Ok(())
    }

    pub(crate) fn clear_for_history_redraw(&mut self) -> String {
        let mut output = String::new();
        output.push_str(&delete_kitty_images(&self.previous_kitty_image_ids));
        if self.fresh_anchor_pending {
            output.push_str("\r\n");
        } else {
            push_vertical_move(&mut output, self.hardware_cursor_row, 0);
            output.push_str("\r\x1b[J");
        }
        self.previous_lines.clear();
        self.previous_cursor = None;
        self.hardware_cursor_row = 0;
        self.previous_kitty_image_ids.clear();
        self.full_redraw_pending = false;
        self.fresh_anchor_pending = false;
        output
    }

    pub(crate) fn reset(&mut self) {
        self.previous_lines.clear();
        self.previous_cursor = None;
        self.hardware_cursor_row = 0;
        self.previous_kitty_image_ids.clear();
        self.full_redraw_pending = false;
        self.fresh_anchor_pending = false;
    }
}

fn push_vertical_move(output: &mut String, from: usize, to: usize) {
    if to > from {
        let _ = write!(output, "\x1b[{}B", to - from);
    } else if from > to {
        let _ = write!(output, "\x1b[{}A", from - to);
    }
}
