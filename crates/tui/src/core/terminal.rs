use std::io::{self, Write};

use crate::renderer::CursorPos;

use super::Line;

#[derive(Debug, Clone)]
pub struct TerminalRenderer {
    width: usize,
    height: usize,
    committed_rows: Vec<Line>,
    live_rows: Vec<Line>,
    cursor: Option<CursorPos>,
    live_region_needs_leading_newline: bool,
}

impl TerminalRenderer {
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            committed_rows: Vec::new(),
            live_rows: Vec::new(),
            cursor: None,
            live_region_needs_leading_newline: false,
        }
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        let previous_live_len = self.live_rows.len();
        self.width = width;
        self.height = height;
        self.live_rows = self.clamp_live_rows(&self.live_rows);
        self.cursor =
            self.clamp_cursor_for_rows(self.cursor, previous_live_len, self.live_rows.len());
    }

    pub fn commit_rows(&mut self, rows: &[Line]) {
        self.committed_rows.extend_from_slice(rows);
    }

    pub fn render_live_region(&mut self, rows: &[Line], cursor: Option<CursorPos>) {
        self.live_rows = self.clamp_live_rows(rows);
        self.cursor = self.clamp_cursor_for_rows(cursor, rows.len(), self.live_rows.len());
        self.live_region_needs_leading_newline = false;
    }

    #[must_use]
    pub fn commit_buffer(&self, rows: &[Line]) -> String {
        let mut buffer = String::new();
        for row in rows {
            buffer.push_str("\r\n");
            buffer.push_str(&row.to_ansi());
        }
        buffer
    }

    pub fn write_commit<W: Write>(&mut self, writer: &mut W, rows: &[Line]) -> io::Result<()> {
        let buffer = self.commit_buffer(rows);
        writer.write_all(buffer.as_bytes())?;
        writer.flush()?;
        self.commit_rows(rows);
        self.live_region_needs_leading_newline |= !rows.is_empty();
        Ok(())
    }

    #[must_use]
    pub fn live_region_buffer(&self, rows: &[Line], cursor: Option<CursorPos>) -> String {
        let live_rows = self.clamp_live_rows(rows);
        let clear_count = self.live_rows.len().max(live_rows.len());
        let mut buffer = String::new();
        buffer.push_str("\x1b[?2026h");
        if self.live_region_needs_leading_newline && clear_count > 0 {
            buffer.push_str("\r\n");
        }
        for index in 0..clear_count {
            if index > 0 {
                buffer.push_str("\r\n");
            }
            buffer.push_str("\x1b[2K");
            if let Some(row) = live_rows.get(index) {
                buffer.push_str(&row.to_ansi());
            }
        }
        if let Some(cursor) = self.clamp_cursor_for_rows(cursor, rows.len(), live_rows.len()) {
            let target_row = cursor.row.min(live_rows.len().saturating_sub(1));
            if clear_count > 0 && target_row + 1 < clear_count {
                buffer.push_str(&format!("\x1b[{}A", clear_count - target_row - 1));
            }
            buffer.push_str(&format!("\r\x1b[{}G", cursor.col + 1));
        }
        buffer.push_str("\x1b[?2026l");
        buffer
    }

    pub fn write_live_region<W: Write>(
        &mut self,
        writer: &mut W,
        rows: &[Line],
        cursor: Option<CursorPos>,
    ) -> io::Result<()> {
        let buffer = self.live_region_buffer(rows, cursor);
        writer.write_all(buffer.as_bytes())?;
        writer.flush()?;
        self.render_live_region(rows, cursor);
        Ok(())
    }

    #[must_use]
    pub fn committed_rows(&self) -> &[Line] {
        &self.committed_rows
    }

    #[must_use]
    pub fn live_rows(&self) -> &[Line] {
        &self.live_rows
    }

    #[must_use]
    pub fn cursor(&self) -> Option<CursorPos> {
        self.cursor
    }

    #[must_use]
    pub fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    fn clamp_width(&self, row: &Line) -> Line {
        if row.visible_width() <= self.width {
            row.clone()
        } else {
            row.truncate_to_width(self.width)
        }
    }

    fn clamp_live_rows(&self, rows: &[Line]) -> Vec<Line> {
        let start = rows.len().saturating_sub(self.height);
        rows[start..]
            .iter()
            .map(|row| self.clamp_width(row))
            .collect()
    }

    fn clamp_cursor_for_rows(
        &self,
        cursor: Option<CursorPos>,
        source_row_count: usize,
        rendered_row_count: usize,
    ) -> Option<CursorPos> {
        let cursor = cursor?;
        let dropped_rows = source_row_count.saturating_sub(rendered_row_count);
        if cursor.row < dropped_rows {
            return None;
        }
        let row = cursor
            .row
            .saturating_sub(dropped_rows)
            .min(rendered_row_count.saturating_sub(1));
        let col = cursor.col.min(self.width.saturating_sub(1));
        Some(CursorPos { row, col })
    }
}
