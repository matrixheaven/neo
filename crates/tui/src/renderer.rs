//! Custom differential renderer for inline terminal output.
//!
//! This is a Rust port of pi-tui's `doRender()` algorithm. It renders content
//! as `Vec<String>` (each string = one terminal line with embedded ANSI codes),
//! diffs against the previous frame, and writes only changed lines.
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
    /// Current hardware cursor position in **screen row** coordinates.
    /// 0 = top of screen, height-1 = bottom.
    hardware_cursor_row: usize,
    previous_width: u16,
    previous_height: u16,
    /// Whether this is the first render (no diff, just output everything).
    first_render: bool,
}

/// Result of computing what terminal operations are needed for a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderDiff {
    /// Number of `\r\n` to emit to scroll content into scrollback.
    scroll_amount: usize,
    /// First content row that changed.
    first_changed: usize,
    /// Last content row that changed.
    last_changed: usize,
    /// Whether to do a full redraw instead of incremental.
    full_redraw: bool,
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
        // Move cursor to the last content row on screen, then write \r\n
        let last_screen_row = self
            .previous_lines
            .len()
            .saturating_sub(1)
            .saturating_sub(self.viewport_top);
        let row_diff = last_screen_row as isize - self.hardware_cursor_row as isize;
        if row_diff > 0 {
            let _ = write!(output, "\x1b[{row_diff}B");
        } else if row_diff < 0 {
            let _ = write!(output, "\x1b[{}A", -row_diff);
        }
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
        let (width, height_u16) = size()?;
        if width == 0 || height_u16 == 0 {
            return Ok(());
        }
        let height = height_u16 as usize;

        let width_changed = self.previous_width != 0 && self.previous_width != width;
        let height_changed = self.previous_height != 0 && self.previous_height != height_u16;

        let mut output = stdout();

        // First render or size change → full redraw
        if self.first_render || width_changed || height_changed {
            self.full_render(&mut output, &new_lines, height, cursor)?;
            self.first_render = false;
            self.previous_width = width;
            self.previous_height = height_u16;
            return Ok(());
        }

        // Compute what operations are needed
        let diff = compute_render_diff(
            &self.previous_lines,
            &new_lines,
            self.viewport_top,
            self.hardware_cursor_row,
            height,
        );

        // No changes — just reposition cursor
        if diff.first_changed == diff.last_changed
            && diff.first_changed == usize::MAX
            && diff.scroll_amount == 0
        {
            self.position_cursor(&mut output, cursor, new_lines.len())?;
            self.previous_lines = new_lines;
            return Ok(());
        }

        // Build the terminal output buffer
        let mut buf = String::with_capacity(4096);
        buf.push_str("\x1b[?2026h"); // Begin synchronized output

        let mut viewport_top = self.viewport_top;
        let hw_cursor = self.hardware_cursor_row; // screen row

        // Handle scrolling: if content grew past viewport, emit \r\n
        if diff.scroll_amount > 0 {
            // Move cursor to bottom of screen first
            let move_down = height.saturating_sub(1).saturating_sub(hw_cursor);
            if move_down > 0 {
                buf.push_str(&format!("\x1b[{move_down}B"));
            }
            // Emit \r\n to push old lines into scrollback
            for _ in 0..diff.scroll_amount {
                buf.push_str("\r\n");
            }
            viewport_top += diff.scroll_amount;
        }

        // Move cursor to the screen position of first_changed
        let target_screen_row = diff.first_changed.saturating_sub(viewport_top);
        // Current cursor position: after scroll we're at the bottom row
        let current_screen_row = if diff.scroll_amount > 0 {
            height.saturating_sub(1)
        } else {
            hw_cursor
        };
        let row_diff = target_screen_row as isize - current_screen_row as isize;
        if row_diff > 0 {
            buf.push_str(&format!("\x1b[{row_diff}B"));
        } else if row_diff < 0 {
            buf.push_str(&format!("\x1b[{}A", -row_diff));
        }
        buf.push('\r'); // Return to column 0

        // Rewrite changed lines
        let render_end = diff.last_changed.min(new_lines.len().saturating_sub(1));
        for i in diff.first_changed..=render_end {
            if i > diff.first_changed {
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
            // Move back up to end of content
            buf.push_str(&format!("\x1b[{extra}A"));
        }

        buf.push_str("\x1b[?2026l"); // End synchronized output

        // Write entire buffer at once
        output.write_all(buf.as_bytes())?;
        let _ = output.flush();

        // Update state — hardware_cursor_row is screen row of last written line
        self.hardware_cursor_row = render_end.saturating_sub(viewport_top);
        self.viewport_top = viewport_top;
        self.previous_lines = new_lines;

        // Position cursor for prompt
        self.position_cursor(&mut output, cursor, self.previous_lines.len())?;

        self.previous_width = width;
        self.previous_height = height_u16;

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
        let _ = output.flush();

        // Update state
        self.viewport_top = lines.len().saturating_sub(height);
        // hardware_cursor_row = screen row of last content line
        let last_content_row = lines.len().saturating_sub(1);
        self.hardware_cursor_row = last_content_row.saturating_sub(self.viewport_top);
        self.previous_lines = lines.to_vec();

        // Position cursor
        self.position_cursor(output, cursor, lines.len())?;

        Ok(())
    }

    /// Position the hardware cursor at the prompt cursor location.
    /// `cursor.row` is a content row; we convert to screen row internally.
    fn position_cursor(
        &mut self,
        output: &mut Stdout,
        cursor: Option<CursorPos>,
        total_lines: usize,
    ) -> std::io::Result<()> {
        let Some(cursor) = cursor else {
            // Hide cursor
            write!(output, "\x1b[?25l")?;
            output.flush()?;
            return Ok(());
        };

        // Show cursor
        write!(output, "\x1b[?25h")?;

        // Clamp cursor row and convert content row → screen row
        let target_content_row = cursor.row.min(total_lines.saturating_sub(1));
        let target_screen_row = target_content_row.saturating_sub(self.viewport_top);

        let row_diff = target_screen_row as isize - self.hardware_cursor_row as isize;
        if row_diff > 0 {
            write!(output, "\x1b[{row_diff}B")?;
        } else if row_diff < 0 {
            write!(output, "\x1b[{}A", -row_diff)?;
        }
        // Move to cursor column (1-indexed)
        write!(output, "\x1b[{}G", cursor.col + 1)?;

        output.flush()?;
        self.hardware_cursor_row = target_screen_row;
        Ok(())
    }
}

/// Pure function: compute what terminal operations are needed for this frame.
/// This is extracted from `render()` so it can be unit tested.
fn compute_render_diff(
    prev_lines: &[String],
    new_lines: &[String],
    viewport_top: usize,
    _hardware_cursor_row: usize,
    height: usize,
) -> RenderDiff {
    // Find first and last changed lines
    let mut first_changed: usize = usize::MAX;
    let mut last_changed: usize = 0;
    let max_lines = new_lines.len().max(prev_lines.len());
    for i in 0..max_lines {
        let old = prev_lines.get(i).map(String::as_str).unwrap_or("");
        let new = new_lines.get(i).map(String::as_str).unwrap_or("");
        if old != new {
            if first_changed == usize::MAX {
                first_changed = i;
            }
            last_changed = i;
        }
    }

    // Content grew: mark all new lines as changed
    let appended = new_lines.len() > prev_lines.len();
    if appended && first_changed == usize::MAX {
        first_changed = prev_lines.len();
    }
    if appended {
        last_changed = new_lines.len() - 1;
    }

    // No changes at all
    if first_changed == usize::MAX {
        return RenderDiff {
            scroll_amount: 0,
            first_changed: usize::MAX,
            last_changed: usize::MAX,
            full_redraw: false,
        };
    }

    // Determine scroll amount: if new content extends past viewport bottom
    let viewport_bottom = viewport_top + height;
    let scroll_amount = if last_changed >= viewport_bottom {
        last_changed.saturating_sub(viewport_bottom) + 1
    } else {
        0
    };

    RenderDiff {
        scroll_amount,
        first_changed,
        last_changed,
        full_redraw: false,
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

    #[test]
    fn compute_diff_no_changes() {
        let prev = vec!["a".to_owned(), "b".to_owned()];
        let new = vec!["a".to_owned(), "b".to_owned()];
        let diff = compute_render_diff(&prev, &new, 0, 0, 24);
        assert_eq!(diff.first_changed, usize::MAX);
        assert_eq!(diff.scroll_amount, 0);
    }

    #[test]
    fn compute_diff_single_line_change() {
        let prev = vec!["a".to_owned(), "b".to_owned()];
        let new = vec!["a".to_owned(), "B".to_owned()];
        let diff = compute_render_diff(&prev, &new, 0, 0, 24);
        assert_eq!(diff.first_changed, 1);
        assert_eq!(diff.last_changed, 1);
        assert_eq!(diff.scroll_amount, 0);
    }

    #[test]
    fn compute_diff_content_grew() {
        let prev = vec!["a".to_owned()];
        let new = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let diff = compute_render_diff(&prev, &new, 0, 0, 24);
        assert_eq!(diff.first_changed, 1);
        assert_eq!(diff.last_changed, 2);
    }

    #[test]
    fn compute_diff_scroll_when_exceeds_viewport() {
        let prev = vec!["line0".to_owned()];
        let new: Vec<String> = (0..30).map(|i| format!("line{i}")).collect();
        let diff = compute_render_diff(&prev, &new, 0, 0, 10);
        assert!(diff.scroll_amount > 0);
        // last_changed (29) >= viewport_bottom (0+10=10)
        assert_eq!(diff.scroll_amount, 29 - 10 + 1); // = 20
    }

    #[test]
    fn compute_diff_no_scroll_within_viewport() {
        let prev = vec!["a".to_owned()];
        let new = vec!["a".to_owned(), "b".to_owned()];
        let diff = compute_render_diff(&prev, &new, 0, 0, 24);
        assert_eq!(diff.scroll_amount, 0);
    }

    #[test]
    fn compute_diff_content_shrunk() {
        let prev = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let new = vec!["a".to_owned()];
        let diff = compute_render_diff(&prev, &new, 0, 0, 24);
        assert_eq!(diff.first_changed, 1);
        assert_eq!(diff.last_changed, 2);
    }

    #[test]
    fn compute_diff_with_viewport_top_nonzero() {
        // Content already scrolled: viewport_top = 5
        let prev: Vec<String> = (0..10).map(|i| format!("line{i}")).collect();
        let new: Vec<String> = (0..10)
            .map(|i| if i == 7 { "CHANGED".to_owned() } else { format!("line{i}") })
            .collect();
        let diff = compute_render_diff(&prev, &new, 5, 0, 24);
        assert_eq!(diff.first_changed, 7);
        assert_eq!(diff.last_changed, 7);
        assert_eq!(diff.scroll_amount, 0); // 7 < 5+24
    }
}
