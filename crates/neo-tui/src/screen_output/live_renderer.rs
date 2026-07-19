use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::Write;

use crate::primitive::visible_width;

use super::kitty_image::{collect_kitty_image_ids, delete_kitty_images};
use super::types::CursorPos;

/// Bounded live-frame diff renderer.
///
/// Geometry ownership lives in [`super::InlineTerminal`]. This type only diffs
/// row contents and emits absolute CUP sequences within a supplied origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveRenderer {
    width: u16,
    height: u16,
    previous_lines: Vec<String>,
    previous_cursor: Option<CursorPos>,
    previous_kitty_image_ids: BTreeSet<u32>,
    full_redraw_pending: bool,
}

impl LiveRenderer {
    #[must_use]
    pub const fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            previous_lines: Vec::new(),
            previous_cursor: None,
            previous_kitty_image_ids: BTreeSet::new(),
            full_redraw_pending: false,
        }
    }

    pub(crate) fn resize(&mut self, width: u16, height: u16) {
        if self.width == width && self.height == height {
            return;
        }
        self.width = width;
        self.height = height;
        // Width/height changes invalidate cached rows. Absolute CUP redraws the
        // live viewport; do not emit CRLF to establish a relative anchor.
        if !self.previous_lines.is_empty() {
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
        origin_row: u16,
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
        let origin = usize::from(origin_row);
        if origin.saturating_add(lines.len()) > usize::from(self.height) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "live frame origin {origin_row} with {} rows exceeds terminal height {}",
                    lines.len(),
                    self.height
                ),
            ));
        }
        if !self.full_redraw_pending
            && self.previous_lines == lines
            && self.previous_cursor == cursor
        {
            return Ok(());
        }

        // Retain the prior line count for absolute clears even when the diff
        // baseline is forced empty by a full redraw.
        let previous_line_count = self.previous_lines.len();
        let previous_lines = if self.full_redraw_pending {
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
        let kitty_image_ids = collect_kitty_image_ids(&lines);
        let mut bytes = String::new();

        if self.full_redraw_pending {
            bytes.push_str(&delete_kitty_images(&self.previous_kitty_image_ids));
            // Clear every previously live-owned row at the absolute origin.
            let clear_rows = previous_line_count.max(lines.len()).max(1);
            for row in 0..clear_rows {
                let screen_row = origin.saturating_add(row);
                if screen_row >= usize::from(self.height) {
                    break;
                }
                push_absolute_move(&mut bytes, screen_row, 0);
                bytes.push_str("\x1b[2K");
            }
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
            for row in first_changed..render_rows {
                let screen_row = origin.saturating_add(row);
                push_absolute_move(&mut bytes, screen_row, 0);
                bytes.push_str("\x1b[2K");
                if let Some(line) = lines.get(row) {
                    bytes.push_str(line);
                }
            }
        }

        let target_row =
            cursor.map_or_else(|| lines.len().saturating_sub(1), |position| position.row);
        let target_col = cursor.map_or(0, |position| position.col);
        let absolute_row = origin.saturating_add(target_row);
        push_absolute_move(&mut bytes, absolute_row, target_col);
        bytes.push_str(if cursor.is_some() {
            "\x1b[?25h"
        } else {
            "\x1b[?25l"
        });

        output.write_all(bytes.as_bytes())?;
        output.flush()?;
        self.previous_lines = lines;
        self.previous_cursor = cursor;
        self.previous_kitty_image_ids = kitty_image_ids;
        self.full_redraw_pending = false;
        Ok(())
    }

    /// Emit absolute clears for the previously drawn live rows at `origin_row`.
    pub(crate) fn clear_at_origin(&mut self, origin_row: u16) -> String {
        let mut output = String::new();
        output.push_str(&delete_kitty_images(&self.previous_kitty_image_ids));
        let origin = usize::from(origin_row);
        let clear_rows = self.previous_lines.len();
        for row in 0..clear_rows {
            let screen_row = origin.saturating_add(row);
            if screen_row >= usize::from(self.height) {
                break;
            }
            push_absolute_move(&mut output, screen_row, 0);
            output.push_str("\x1b[2K");
        }
        self.previous_lines.clear();
        self.previous_cursor = None;
        self.previous_kitty_image_ids.clear();
        self.full_redraw_pending = false;
        output
    }

    pub(crate) fn reset(&mut self) {
        self.previous_lines.clear();
        self.previous_cursor = None;
        self.previous_kitty_image_ids.clear();
        self.full_redraw_pending = false;
    }

    #[must_use]
    pub(crate) fn previous_line_count(&self) -> usize {
        self.previous_lines.len()
    }
}

/// Emit CUP using one-based ANSI coordinates from zero-based geometry.
fn push_absolute_move(output: &mut String, row: usize, col: usize) {
    let ansi_row = row.saturating_add(1);
    let ansi_col = col.saturating_add(1);
    let _ = write!(output, "\x1b[{ansi_row};{ansi_col}H");
}
