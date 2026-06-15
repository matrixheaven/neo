//! Custom differential renderer for inline terminal output.
//!
//! This is a 1:1 Rust port of pi-tui's `TUI.doRender()` / `fullRender()` /
//! `positionHardwareCursor()` algorithm (see `docs/pi/packages/tui/src/tui.ts`).
//! It renders content as `Vec<String>` (each string = one terminal line with
//! embedded ANSI codes), diffs against the previous frame, and writes only
//! changed lines.
//!
//! When content grows past the screen bottom, `\r\n` pushes old lines into the
//! terminal's native scrollback buffer — no alternate screen needed.
//!
//! ## Coordinate System
//!
//! `hardware_cursor_row` is ALWAYS a **screen row**: 0 = top of visible screen,
//! `height-1` = bottom. Content rows are converted to screen rows via
//! `content_row - viewport_top`.

use std::io::{Stdout, Write, stdout};

use crossterm::{
    event::{
        DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, size},
};

/// Cursor position for prompt editing (row, col) in the rendered content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPos {
    pub row: usize,
    pub col: usize,
}

/// A zero-width cursor marker embedded in rendered output.
/// The renderer finds this marker, strips it, and positions the hardware cursor.
pub const CURSOR_MARKER: &str = "\x1b_pi:c\x07";

pub struct InlineRenderer {
    previous_lines: Vec<String>,
    /// Content row index of the top of the visible viewport.
    viewport_top: usize,
    previous_viewport_top: usize,
    /// Current hardware cursor position in **screen row** coordinates.
    /// 0 = top of screen, height-1 = bottom.
    hardware_cursor_row: usize,
    previous_width: u16,
    previous_height: u16,
    /// Whether this is the first render (no diff, just output everything).
    first_render: bool,
    /// Track terminal's working area (max lines ever rendered). Mirrors
    /// pi-tui's `maxLinesRendered`: grows but doesn't shrink unless cleared.
    max_lines_rendered: usize,
    /// Logical end-of-content row (mirrors pi-tui's `cursorRow`).
    cursor_row: usize,
}

impl InlineRenderer {
    /// Enable raw mode + bracketed paste + keyboard enhancement.
    /// Does NOT enter alternate screen or enable mouse capture.
    pub fn enter() -> std::io::Result<Self> {
        enable_raw_mode()?;
        execute!(
            stdout(),
            EnableBracketedPaste,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        Ok(Self {
            previous_lines: Vec::new(),
            viewport_top: 0,
            previous_viewport_top: 0,
            hardware_cursor_row: 0,
            previous_width: 0,
            previous_height: 0,
            first_render: true,
            max_lines_rendered: 0,
            cursor_row: 0,
        })
    }

    /// Restore terminal state.
    pub fn leave(&mut self) {
        let mut output = stdout();
        // Move cursor to the end of the content to prevent overwriting on exit.
        // 1:1 port of pi-tui's `stop()`.
        if !self.previous_lines.is_empty() {
            let target_row = self.previous_lines.len(); // Line after the last content
            let line_diff = target_row as isize - self.hardware_cursor_row as isize;
            if line_diff > 0 {
                let _ = write!(output, "\x1b[{line_diff}B");
            } else if line_diff < 0 {
                let _ = write!(output, "\x1b[{}A", (-line_diff));
            }
            let _ = write!(output, "\r\n");
        }
        let _ = output.flush();

        let _ = execute!(output, PopKeyboardEnhancementFlags, DisableBracketedPaste,);
        let _ = disable_raw_mode();
    }

    /// Suspend (Ctrl+Z): leave terminal, then re-enter.
    pub fn suspend_prepare(&mut self) {
        self.leave();
    }

    /// Re-enter after suspend.
    pub fn suspend_resume(&mut self) -> std::io::Result<()> {
        enable_raw_mode()?;
        execute!(
            stdout(),
            EnableBracketedPaste,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        // Force full redraw after resume
        self.first_render = true;
        self.previous_lines.clear();
        self.viewport_top = 0;
        self.previous_viewport_top = 0;
        self.hardware_cursor_row = 0;
        self.max_lines_rendered = 0;
        self.cursor_row = 0;
        Ok(())
    }

    /// Render a frame. `new_lines` contains all content lines (with ANSI codes).
    /// `cursor` is the optional prompt cursor position in the rendered content.
    ///
    /// This is a 1:1 port of pi-tui's `TUI.doRender()`.
    pub fn render(
        &mut self,
        new_lines: Vec<String>,
        cursor: Option<CursorPos>,
    ) -> std::io::Result<()> {
        let (width, height_u16) = size()?;
        if width == 0 || height_u16 == 0 {
            return Ok(());
        }
        let height = height_u16 as usize;

        let width_changed = self.previous_width != 0 && self.previous_width != width;
        let height_changed = self.previous_height != 0 && self.previous_height != height_u16;

        // The previous buffer length is how many content rows the old viewport
        // covered. On a height change we recompute the previous viewport top so
        // the bottom stays anchored.
        let previous_buffer_length = if self.previous_height > 0 {
            self.previous_viewport_top + usize::from(self.previous_height)
        } else {
            height
        };
        let mut prev_viewport_top = if height_changed {
            previous_buffer_length.saturating_sub(height)
        } else {
            self.previous_viewport_top
        };
        let mut viewport_top = prev_viewport_top;
        let mut hardware_cursor_row = self.hardware_cursor_row;

        // Helper: line diff (in screen rows) from the current cursor to a
        // target content row. Mirrors pi-tui's `computeLineDiff` closure; kept
        // as a free function so the loop body can mutate the viewport state.
        let compute_line_diff =
            |target_row: usize, hwc: usize, prev_vt: usize, vt: usize| -> isize {
                let current_screen_row = hwc as isize - prev_vt as isize;
                let target_screen_row = target_row as isize - vt as isize;
                target_screen_row - current_screen_row
            };

        let cursor_pos = cursor;
        let new_lines_ref = &new_lines;

        let mut output = stdout();

        // First render - just output everything without clearing (assumes clean screen)
        if self.previous_lines.is_empty() && !width_changed && !height_changed {
            self.full_render(
                &mut output,
                false,
                new_lines_ref,
                height,
                height_u16,
                width,
                cursor_pos,
            )?;
            self.first_render = false;
            return Ok(());
        }

        // First render flag (e.g. after suspend_resume) → full redraw without clear.
        if self.first_render {
            self.full_render(
                &mut output,
                false,
                new_lines_ref,
                height,
                height_u16,
                width,
                cursor_pos,
            )?;
            self.first_render = false;
            return Ok(());
        }

        // Width changes always need a full re-render because wrapping changes.
        if width_changed {
            self.full_render(
                &mut output,
                true,
                new_lines_ref,
                height,
                height_u16,
                width,
                cursor_pos,
            )?;
            return Ok(());
        }

        // Height changes need a full re-render to keep the viewport aligned.
        if height_changed {
            self.full_render(
                &mut output,
                true,
                new_lines_ref,
                height,
                height_u16,
                width,
                cursor_pos,
            )?;
            return Ok(());
        }

        // Find first and last changed lines
        let mut first_changed: i64 = -1;
        let mut last_changed: i64 = -1;
        let max_lines = new_lines.len().max(self.previous_lines.len());
        for i in 0..max_lines {
            let old_line = self.previous_lines.get(i).map(String::as_str).unwrap_or("");
            let new_line = new_lines.get(i).map(String::as_str).unwrap_or("");
            if old_line != new_line {
                if first_changed == -1 {
                    first_changed = i as i64;
                }
                last_changed = i as i64;
            }
        }
        let appended_lines = new_lines.len() > self.previous_lines.len();
        if appended_lines {
            if first_changed == -1 {
                first_changed = self.previous_lines.len() as i64;
            }
            last_changed = (new_lines.len() - 1) as i64;
        }
        let append_start = appended_lines
            && first_changed == self.previous_lines.len() as i64
            && first_changed > 0;

        // No changes - but still need to update hardware cursor position if it moved
        if first_changed == -1 {
            self.position_hardware_cursor(&mut output, cursor_pos, new_lines.len())?;
            self.previous_viewport_top = prev_viewport_top;
            self.previous_height = height_u16;
            self.previous_lines = new_lines;
            return Ok(());
        }

        let first_changed_u = first_changed as usize;
        let last_changed_u = last_changed as usize;

        // All changes are in deleted lines (nothing to render, just clear)
        if first_changed_u >= new_lines.len() {
            if self.previous_lines.len() > new_lines.len() {
                let mut buffer = String::new();
                buffer.push_str("\x1b[?2026h");
                // Move to end of new content (clamp to 0 for empty content)
                let target_row = new_lines.len().saturating_sub(1);
                if target_row < prev_viewport_top {
                    self.full_render(
                        &mut output,
                        true,
                        new_lines_ref,
                        height,
                        height_u16,
                        width,
                        cursor_pos,
                    )?;
                    return Ok(());
                }
                let line_diff = compute_line_diff(
                    target_row,
                    hardware_cursor_row,
                    prev_viewport_top,
                    viewport_top,
                );
                if line_diff > 0 {
                    buffer.push_str(&format!("\x1b[{line_diff}B"));
                } else if line_diff < 0 {
                    buffer.push_str(&format!("\x1b[{}A", (-line_diff)));
                }
                buffer.push('\r');
                // Clear extra lines without scrolling
                let extra_lines = self.previous_lines.len() - new_lines.len();
                if extra_lines > height {
                    self.full_render(
                        &mut output,
                        true,
                        new_lines_ref,
                        height,
                        height_u16,
                        width,
                        cursor_pos,
                    )?;
                    return Ok(());
                }
                if extra_lines > 0 {
                    buffer.push_str("\x1b[1B");
                }
                for i in 0..extra_lines {
                    buffer.push_str("\r\x1b[2K");
                    if i < extra_lines - 1 {
                        buffer.push_str("\x1b[1B");
                    }
                }
                if extra_lines > 0 {
                    buffer.push_str(&format!("\x1b[{extra_lines}A"));
                }
                buffer.push_str("\x1b[?2026l");
                let _ = output.write_all(buffer.as_bytes());
                let _ = output.flush();
                self.cursor_row = target_row;
                self.hardware_cursor_row = target_row;
            }
            self.position_hardware_cursor(&mut output, cursor_pos, new_lines.len())?;
            self.previous_lines = new_lines;
            self.previous_width = width;
            self.previous_height = height_u16;
            self.previous_viewport_top = prev_viewport_top;
            return Ok(());
        }

        // Differential rendering can only touch what was actually visible.
        // If the first changed line is above the previous viewport, full redraw.
        if first_changed_u < prev_viewport_top {
            self.full_render(
                &mut output,
                true,
                new_lines_ref,
                height,
                height_u16,
                width,
                cursor_pos,
            )?;
            return Ok(());
        }

        // Render from first changed line to end
        let mut buffer = String::with_capacity(4096);
        buffer.push_str("\x1b[?2026h"); // Begin synchronized output
        let prev_viewport_bottom = prev_viewport_top + height - 1;
        let move_target_row = if append_start {
            first_changed_u.saturating_sub(1)
        } else {
            first_changed_u
        };
        if move_target_row > prev_viewport_bottom {
            let current_screen_row = (hardware_cursor_row as isize - prev_viewport_top as isize)
                .clamp(0, (height - 1) as isize) as usize;
            let move_to_bottom = height - 1 - current_screen_row;
            if move_to_bottom > 0 {
                buffer.push_str(&format!("\x1b[{move_to_bottom}B"));
            }
            let scroll = move_target_row - prev_viewport_bottom;
            for _ in 0..scroll {
                buffer.push_str("\r\n");
            }
            prev_viewport_top += scroll;
            viewport_top += scroll;
            hardware_cursor_row = move_target_row;
        }

        // Move cursor to first changed line (use hardware_cursor_row for actual position)
        let line_diff = compute_line_diff(
            move_target_row,
            hardware_cursor_row,
            prev_viewport_top,
            viewport_top,
        );
        if line_diff > 0 {
            buffer.push_str(&format!("\x1b[{line_diff}B")); // Move down
        } else if line_diff < 0 {
            buffer.push_str(&format!("\x1b[{}A", (-line_diff))); // Move up
        }

        if append_start {
            buffer.push_str("\r\n"); // Move to column 0 on a fresh line
        } else {
            buffer.push('\r'); // Move to column 0
        }

        // Only render changed lines (first_changed to last_changed), not all lines to end.
        let render_end = last_changed_u.min(new_lines.len().saturating_sub(1));
        for i in first_changed_u..=render_end {
            if i > first_changed_u {
                buffer.push_str("\r\n");
            }
            buffer.push_str("\x1b[2K"); // Clear current line
            buffer.push_str(&new_lines[i]);
        }

        // Track where cursor ended up after rendering
        let mut final_cursor_row = render_end;

        // If we had more lines before, clear them and move cursor back
        if self.previous_lines.len() > new_lines.len() {
            // Move to end of new content first if we stopped before it
            if render_end + 1 < new_lines.len() {
                let move_down = new_lines.len() - 1 - render_end;
                buffer.push_str(&format!("\x1b[{move_down}B"));
                final_cursor_row = new_lines.len() - 1;
            }
            let extra_lines = self.previous_lines.len() - new_lines.len();
            for _ in new_lines.len()..self.previous_lines.len() {
                buffer.push_str("\r\n\x1b[2K");
            }
            // Move cursor back to end of new content
            buffer.push_str(&format!("\x1b[{extra_lines}A"));
        }

        buffer.push_str("\x1b[?2026l"); // End synchronized output

        // Write entire buffer at once
        let _ = output.write_all(buffer.as_bytes());
        let _ = output.flush();

        // Track cursor position for next render
        self.cursor_row = new_lines.len().saturating_sub(1);
        self.hardware_cursor_row = final_cursor_row;
        self.max_lines_rendered = self.max_lines_rendered.max(new_lines.len());
        self.previous_viewport_top =
            prev_viewport_top.max(final_cursor_row.saturating_sub(height - 1));

        // Position hardware cursor for IME
        self.position_hardware_cursor(&mut output, cursor_pos, new_lines.len())?;

        self.previous_lines = new_lines;
        self.previous_width = width;
        self.previous_height = height_u16;
        Ok(())
    }

    /// Full redraw: output all lines from scratch.
    /// 1:1 port of pi-tui's `fullRender()`.
    fn full_render(
        &mut self,
        output: &mut Stdout,
        clear: bool,
        new_lines: &[String],
        height: usize,
        height_u16: u16,
        width: u16,
        cursor_pos: Option<CursorPos>,
    ) -> std::io::Result<()> {
        let mut buffer = String::with_capacity(8192);
        buffer.push_str("\x1b[?2026h");
        if clear {
            buffer.push_str("\x1b[2J\x1b[H\x1b[3J"); // Clear screen, home, clear scrollback
        }
        for (i, line) in new_lines.iter().enumerate() {
            if i > 0 {
                buffer.push_str("\r\n");
            }
            buffer.push_str(line);
        }
        buffer.push_str("\x1b[?2026l");
        let _ = output.write_all(buffer.as_bytes());
        let _ = output.flush();

        self.cursor_row = new_lines.len().saturating_sub(1);
        self.hardware_cursor_row = self.cursor_row;
        if clear {
            self.max_lines_rendered = new_lines.len();
        } else {
            self.max_lines_rendered = self.max_lines_rendered.max(new_lines.len());
        }
        let buffer_length = height.max(new_lines.len());
        self.previous_viewport_top = buffer_length.saturating_sub(height);
        self.position_hardware_cursor(output, cursor_pos, new_lines.len())?;
        self.previous_lines = new_lines.to_vec();
        self.previous_width = width;
        self.previous_height = height_u16;
        Ok(())
    }

    /// Position the hardware cursor for IME candidate window.
    /// 1:1 port of pi-tui's `positionHardwareCursor`.
    fn position_hardware_cursor(
        &mut self,
        output: &mut Stdout,
        cursor_pos: Option<CursorPos>,
        total_lines: usize,
    ) -> std::io::Result<()> {
        if cursor_pos.is_none() || total_lines == 0 {
            let _ = write!(output, "\x1b[?25l"); // Hide cursor
            let _ = output.flush();
            return Ok(());
        }
        let cursor_pos = cursor_pos.unwrap();

        // Clamp cursor position to valid range
        let target_row = cursor_pos.row.min(total_lines - 1);
        let target_col = cursor_pos.col;

        // Move cursor from current position to target
        let row_delta = target_row as isize - self.hardware_cursor_row as isize;
        let mut buffer = String::new();
        if row_delta > 0 {
            buffer.push_str(&format!("\x1b[{row_delta}B")); // Move down
        } else if row_delta < 0 {
            buffer.push_str(&format!("\x1b[{}A", (-row_delta))); // Move up
        }
        // Move to absolute column (1-indexed)
        buffer.push_str(&format!("\x1b[{}G", target_col + 1));
        // Show cursor (pi-tui shows it when a cursor position exists)
        buffer.push_str("\x1b[?25h");

        if !buffer.is_empty() {
            let _ = output.write_all(buffer.as_bytes());
            let _ = output.flush();
        }

        self.hardware_cursor_row = target_row;
        Ok(())
    }
}

impl Drop for InlineRenderer {
    fn drop(&mut self) {
        self.leave();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_pos_is_copy() {
        let pos = CursorPos { row: 1, col: 2 };
        let pos2 = pos;
        assert_eq!(pos, pos2);
    }
}
