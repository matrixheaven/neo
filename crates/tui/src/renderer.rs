//! Custom differential renderer for inline terminal output.
//!
//! This is a Rust port of pi-tui's `doRender()` algorithm. It renders content
//! as `Vec<String>` (each string = one terminal line with embedded ANSI codes),
//! diffs against the previous frame, and writes only changed lines.
//!
//! When content grows past the screen bottom, `\r\n` pushes old lines into the
//! terminal's native scrollback buffer — no alternate screen needed.

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
    viewport_top: usize,
    hardware_cursor_row: usize,
    previous_width: u16,
    previous_height: u16,
    /// Whether this is the first render (no diff, just output everything).
    first_render: bool,
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
            hardware_cursor_row: 0,
            previous_width: 0,
            previous_height: 0,
            first_render: true,
        })
    }

    /// Restore terminal state.
    pub fn leave(&mut self) {
        let mut output = stdout();
        // Move cursor to end of content and write newline for shell prompt
        let _ = self.move_cursor(&mut output, self.previous_lines.len().saturating_sub(1));
        let _ = write!(output, "\r\n");
        let _ = output.flush();

        let _ = execute!(
            output,
            PopKeyboardEnhancementFlags,
            DisableBracketedPaste,
        );
        let _ = disable_raw_mode();
    }

    /// Suspend (Ctrl+Z): leave terminal, then re-enter.
    /// The actual SIGTSTP signal must be sent by the caller (interactive.rs)
    /// because neo-tui doesn't depend on rustix.
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
        Ok(())
    }

    /// Render a frame. `new_lines` contains all content lines (with ANSI codes).
    /// `cursor` is the optional prompt cursor position in the rendered content.
    pub fn render(
        &mut self,
        new_lines: Vec<String>,
        cursor: Option<CursorPos>,
    ) -> std::io::Result<()> {
        let (width, height) = size()?;
        if width == 0 || height == 0 {
            return Ok(());
        }

        let height = height as usize;
        let width_changed = self.previous_width != 0 && self.previous_width != width;
        let height_changed = self.previous_height != 0 && self.previous_height != height as u16;

        let mut output = stdout();

        // First render or size change → full redraw
        if self.first_render || width_changed || height_changed {
            self.full_render(&mut output, &new_lines, height, cursor)?;
            self.first_render = false;
            self.previous_width = width;
            self.previous_height = height as u16;
            return Ok(());
        }

        // Find first and last changed lines
        let mut first_changed: isize = -1;
        let mut last_changed: isize = -1;
        let max_lines = new_lines.len().max(self.previous_lines.len());
        for i in 0..max_lines {
            let old = self.previous_lines.get(i).map(String::as_str).unwrap_or("");
            let new = new_lines.get(i).map(String::as_str).unwrap_or("");
            if old != new {
                if first_changed == -1 {
                    first_changed = i as isize;
                }
                last_changed = i as isize;
            }
        }

        // Content grew: mark all new lines as changed
        let appended = new_lines.len() > self.previous_lines.len();
        if appended && first_changed == -1 {
            first_changed = self.previous_lines.len() as isize;
        }
        if appended {
            last_changed = new_lines.len() as isize - 1;
        }

        // No changes — just reposition cursor
        if first_changed == -1 {
            self.position_cursor(&mut output, cursor, new_lines.len(), height)?;
            self.previous_lines = new_lines;
            return Ok(());
        }

        let first_changed = first_changed as usize;
        let last_changed = last_changed as usize;

        // Begin synchronized output
        let mut buf = String::with_capacity(4096);
        buf.push_str("\x1b[?2026h");

        // If content extends past the viewport, scroll old lines into scrollback
        let mut viewport_top = self.viewport_top;
        let prev_viewport_top = viewport_top;
        let prev_viewport_bottom = prev_viewport_top + height;

        if last_changed >= prev_viewport_bottom {
            // Content grew past the viewport — scroll terminal
            let move_target = if first_changed == self.previous_lines.len() && first_changed > 0 {
                first_changed - 1
            } else {
                first_changed
            };

            if move_target >= prev_viewport_bottom {
                // Move cursor to bottom of viewport
                let current_screen_row = self.hardware_cursor_row.saturating_sub(prev_viewport_top);
                let move_down = height.saturating_sub(1).saturating_sub(current_screen_row);
                if move_down > 0 {
                    buf.push_str(&format!("\x1b[{move_down}B"));
                }
                // Scroll by emitting \r\n
                let scroll = move_target.saturating_sub(prev_viewport_bottom) + 1;
                for _ in 0..scroll {
                    buf.push_str("\r\n");
                }
                viewport_top += scroll;
            }
        }

        // Move cursor to first changed line
        let target_screen_row = first_changed.saturating_sub(viewport_top);
        let current_screen_row = self
            .hardware_cursor_row
            .saturating_sub(prev_viewport_top);
        let row_diff = target_screen_row as isize - current_screen_row as isize;
        if row_diff > 0 {
            buf.push_str(&format!("\x1b[{row_diff}B"));
        } else if row_diff < 0 {
            buf.push_str(&format!("\x1b[{}A", -row_diff));
        }
        buf.push('\r'); // Return to column 0

        // Rewrite changed lines
        let render_end = last_changed.min(new_lines.len().saturating_sub(1));
        for i in first_changed..=render_end {
            if i > first_changed {
                buf.push_str("\r\n");
            }
            buf.push_str("\x1b[2K"); // Clear line
            if let Some(line) = new_lines.get(i) {
                buf.push_str(line);
            }
        }

        // If content shrank, clear extra lines
        if new_lines.len() < self.previous_lines.len() {
            let extra = self.previous_lines.len() - new_lines.len();
            for _ in 0..extra {
                buf.push_str("\r\n\x1b[2K");
            }
            // Move back up
            buf.push_str(&format!("\x1b[{extra}A"));
        }

        buf.push_str("\x1b[?2026l"); // End synchronized output

        // Write entire buffer at once
        output.write_all(buf.as_bytes())?;
        output.flush()?;

        // Update state
        self.hardware_cursor_row = render_end;
        self.viewport_top = viewport_top;
        self.previous_lines = new_lines;

        // Position cursor for prompt
        self.position_cursor(&mut output, cursor, self.previous_lines.len(), height)?;

        self.previous_width = width;
        self.previous_height = height as u16;

        Ok(())
    }

    /// Full redraw: output all lines from scratch.
    fn full_render(
        &mut self,
        output: &mut Stdout,
        lines: &[String],
        height: usize,
        cursor: Option<CursorPos>,
    ) -> std::io::Result<()> {
        let mut buf = String::with_capacity(8192);
        buf.push_str("\x1b[?2026h");

        // Output all lines
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                buf.push_str("\r\n");
            }
            buf.push_str(line);
        }

        buf.push_str("\x1b[?2026l");

        output.write_all(buf.as_bytes())?;
        output.flush()?;

        // Update state
        self.viewport_top = lines.len().saturating_sub(height);
        self.hardware_cursor_row = lines.len().saturating_sub(1);
        self.previous_lines = lines.to_vec();

        // Position cursor
        self.position_cursor(output, cursor, lines.len(), height)?;

        Ok(())
    }

    /// Move the hardware cursor to a specific content row.
    fn move_cursor(&mut self, output: &mut Stdout, target_row: usize) -> std::io::Result<()> {
        let row_diff = target_row as isize - self.hardware_cursor_row as isize;
        if row_diff > 0 {
            write!(output, "\x1b[{row_diff}B")?;
        } else if row_diff < 0 {
            write!(output, "\x1b[{}A", -row_diff)?;
        }
        self.hardware_cursor_row = target_row;
        Ok(())
    }

    /// Position the hardware cursor at the prompt cursor location.
    fn position_cursor(
        &mut self,
        output: &mut Stdout,
        cursor: Option<CursorPos>,
        total_lines: usize,
        _height: usize,
    ) -> std::io::Result<()> {
        let Some(cursor) = cursor else {
            // Hide cursor
            write!(output, "\x1b[?25l")?;
            output.flush()?;
            return Ok(());
        };

        // Show cursor
        write!(output, "\x1b[?25h")?;

        // Clamp cursor row
        let target_row = cursor.row.min(total_lines.saturating_sub(1));
        let screen_row = target_row.saturating_sub(self.viewport_top);

        let row_diff = screen_row as isize - self.hardware_cursor_row as isize;
        if row_diff > 0 {
            write!(output, "\x1b[{row_diff}B")?;
        } else if row_diff < 0 {
            write!(output, "\x1b[{}A", -row_diff)?;
        }
        // Move to cursor column (1-indexed)
        write!(output, "\x1b[{}G", cursor.col + 1)?;

        output.flush()?;
        self.hardware_cursor_row = screen_row;
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
