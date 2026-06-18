//! Custom differential renderer for inline terminal output.
//!
//! This implements Neo's single-buffer terminal rendering algorithm.
//! It renders content as `Vec<String>` (each string = one terminal line with
//! embedded ANSI codes), diffs against the previous frame, and writes only
//! changed lines.
//!
//! When content grows past the screen bottom, `\r\n` pushes old lines into the
//! terminal's native scrollback buffer — no alternate screen needed.
//!
//! ## Coordinate System
//!
//! `hardware_cursor_row` is the rendered content row currently occupied by the
//! terminal cursor. Content rows are converted to screen rows via
//! `content_row - viewport_top` when computing cursor movement.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{Write, stdout};
use std::path::PathBuf;
use std::time::SystemTime;

use crossterm::{
    event::{
        DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, size},
};

use crate::ansi::visible_width;

const KITTY_SEQUENCE_PREFIX: &str = "\x1b_G";
const SEGMENT_RESET: &str = "\x1b[0m\x1b]8;;\x07";

/// Cursor position for prompt editing (row, col) in the rendered content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPos {
    pub row: usize,
    pub col: usize,
}

/// A zero-width cursor marker embedded in rendered output.
/// The renderer finds this marker, strips it, and positions the hardware cursor.
pub const CURSOR_MARKER: &str = "\x1b_pi:c\x07";

fn debug_log_enabled() -> bool {
    env::var("NEO_TUI_DEBUG").is_ok_and(|v| v == "1")
}

fn is_termux_session() -> bool {
    env::var("TERMUX_VERSION").is_ok()
}

const fn height_change_requires_clear(height_changed: bool, is_termux: bool) -> bool {
    height_changed && !is_termux
}

fn write_output_log(label: &str, buffer: &str) -> std::io::Result<()> {
    let dir = PathBuf::from("/tmp/neo-tui-debug");
    fs::create_dir_all(&dir)?;
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = dir.join(format!("output-{label}-{timestamp}.log"));
    let mut file = fs::File::create(&path)?;
    file.write_all(buffer.as_bytes())?;
    file.flush()
}

fn write_debug_log(
    label: &str,
    width: u16,
    height: usize,
    new_lines: &[String],
    previous_lines: &[String],
    extra: Option<&str>,
) -> std::io::Result<()> {
    let dir = PathBuf::from("/tmp/neo-tui-debug");
    fs::create_dir_all(&dir)?;
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = dir.join(format!("{label}-{timestamp}.log"));
    let mut file = fs::File::create(&path)?;
    writeln!(file, "[{label}] width={width} height={height}")?;
    writeln!(
        file,
        "new_lines.len()={} previous_lines.len()={}",
        new_lines.len(),
        previous_lines.len()
    )?;
    if let Some(text) = extra {
        writeln!(file, "{text}")?;
    }
    writeln!(file, "=== new_lines ===")?;
    for (i, line) in new_lines.iter().enumerate() {
        let vw = visible_width(line);
        writeln!(file, "[{i}] (w={vw}) {line}")?;
    }
    writeln!(file, "=== previous_lines ===")?;
    for (i, line) in previous_lines.iter().enumerate() {
        let vw = visible_width(line);
        writeln!(file, "[{i}] (w={vw}) {line}")?;
    }
    file.flush()
}

fn write_width_crash_log(
    width: u16,
    new_lines: &[String],
    offender_idx: usize,
) -> std::io::Result<PathBuf> {
    let dir = PathBuf::from("/tmp/neo-tui-debug");
    fs::create_dir_all(&dir)?;
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = dir.join(format!("width-crash-{timestamp}.log"));
    let mut file = fs::File::create(&path)?;
    writeln!(file, "Terminal width: {width}")?;
    writeln!(file, "Offending line index: {offender_idx}")?;
    writeln!(
        file,
        "Offending line visible width: {}",
        visible_width(&new_lines[offender_idx])
    )?;
    writeln!(file, "=== All rendered lines ===")?;
    for (i, line) in new_lines.iter().enumerate() {
        let vw = visible_width(line);
        writeln!(file, "[{i}] (w={vw}) {line}")?;
    }
    file.flush()?;
    Ok(path)
}

fn check_line_widths(width: u16, new_lines: &[String]) -> std::io::Result<()> {
    for (i, line) in new_lines.iter().enumerate() {
        if visible_width(line) > usize::from(width) {
            let path = write_width_crash_log(width, new_lines, i)?;
            return Err(std::io::Error::other(format!(
                "rendered line {i} exceeds terminal width ({} > {width}). crash log: {}",
                visible_width(line),
                path.display()
            )));
        }
    }
    Ok(())
}

pub struct TuiRenderer {
    previous_lines: Vec<String>,
    previous_kitty_image_ids: BTreeSet<u32>,
    /// Content row index of the top of the visible viewport.
    viewport_top: usize,
    previous_viewport_top: usize,
    /// Rendered content row currently occupied by the terminal cursor.
    hardware_cursor_row: usize,
    previous_width: u16,
    previous_height: u16,
    /// Whether this is the first render (no diff, just output everything).
    first_render: bool,
    /// Track terminal's working area (max lines ever rendered).
    /// Grows but doesn't shrink unless the renderer takes a clear path.
    max_lines_rendered: usize,
    /// Logical end-of-content row.
    cursor_row: usize,
    /// Defaults to off; when enabled, a shrink below the historical high-water
    /// mark takes the full clear path.
    clear_on_shrink: bool,
    show_hardware_cursor: bool,
}

impl TuiRenderer {
    /// Enable raw mode + bracketed paste + keyboard enhancement.
    /// Does NOT enter alternate screen or enable mouse capture.
    pub fn enter() -> std::io::Result<Self> {
        enable_raw_mode()?;
        execute!(
            stdout(),
            EnableBracketedPaste,
            PushKeyboardEnhancementFlags(
                // Match Codex: only disambiguate, report event types, and report
                // alternate keys. REPORT_ALL_KEYS_AS_ESCAPE_CODES is omitted
                // because it can cause terminals to send Enter as `CSI 13 u`
                // and may drop the shift modifier on Shift+Enter.
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS,
            )
        )?;
        Ok(Self {
            previous_lines: Vec::new(),
            previous_kitty_image_ids: BTreeSet::new(),
            viewport_top: 0,
            previous_viewport_top: 0,
            hardware_cursor_row: 0,
            previous_width: 0,
            previous_height: 0,
            first_render: true,
            max_lines_rendered: 0,
            cursor_row: 0,
            clear_on_shrink: false,
            show_hardware_cursor: env::var("PI_HARDWARE_CURSOR").is_ok_and(|value| value == "1"),
        })
    }

    /// Restore terminal state.
    pub fn leave(&mut self) {
        let mut output = stdout();
        // Move cursor to the end of the content to prevent overwriting on exit.
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
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS,
            )
        )?;
        // Force full redraw after resume
        self.first_render = true;
        self.previous_lines.clear();
        self.previous_kitty_image_ids.clear();
        self.viewport_top = 0;
        self.previous_viewport_top = 0;
        self.hardware_cursor_row = 0;
        self.max_lines_rendered = 0;
        self.cursor_row = 0;
        self.previous_kitty_image_ids.clear();
        Ok(())
    }

    /// Force the next `render()` call down the full clear render path.
    pub fn force_clear(&mut self) {
        // Set width_changed=true by making previous_width nonzero but different.
        // The render() method checks `previous_width != 0 && previous_width != width`.
        // We use 1 as a sentinel that will never match any real terminal width.
        self.previous_lines.clear();
        self.previous_width = 1; // sentinel — will always differ from real width
        self.previous_height = 0;
        self.previous_viewport_top = 0;
        self.viewport_top = 0;
        self.hardware_cursor_row = 0;
        self.max_lines_rendered = 0;
        self.cursor_row = 0;
        // Do NOT set first_render — we want the width_changed path which does
        // full_render(true), not first_render which does full_render(false).
        self.first_render = false;
    }

    /// Render a frame. `new_lines` contains all content lines (with ANSI codes).
    /// `cursor` is the optional prompt cursor position in the rendered content.
    ///
    /// Render a complete frame using single-buffer diffing.
    pub fn render(
        &mut self,
        new_lines: Vec<String>,
        cursor: Option<CursorPos>,
    ) -> std::io::Result<()> {
        let mut output = stdout();
        let (width, height_u16) = size()?;
        if width == 0 || height_u16 == 0 {
            return Ok(());
        }
        self.render_to_with_size(&mut output, width, height_u16, new_lines, cursor)
    }

    fn render_to_with_size(
        &mut self,
        output: &mut dyn Write,
        width: u16,
        height_u16: u16,
        mut new_lines: Vec<String>,
        cursor: Option<CursorPos>,
    ) -> std::io::Result<()> {
        let height = height_u16 as usize;

        if debug_log_enabled() {
            let _ = write_debug_log(
                "render-start",
                width,
                height,
                &new_lines,
                &self.previous_lines,
                Some(&format!(
                    "previous_width={} previous_height={} previous_viewport_top={} viewport_top={} hardware_cursor_row={} first_render={} clear_on_shrink={}",
                    self.previous_width,
                    self.previous_height,
                    self.previous_viewport_top,
                    self.viewport_top,
                    self.hardware_cursor_row,
                    self.first_render,
                    self.clear_on_shrink
                )),
            );
        }

        check_line_widths(width, &new_lines)?;

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
        // target content row. Kept as a free function so the loop body can
        // mutate the viewport state.
        let compute_line_diff =
            |target_row: usize, hwc: usize, prev_vt: usize, vt: usize| -> isize {
                let current_screen_row = hwc as isize - prev_vt as isize;
                let target_screen_row = target_row as isize - vt as isize;
                target_screen_row - current_screen_row
            };

        let marker_cursor_pos = extract_cursor_position(&mut new_lines, height);
        let cursor_pos = marker_cursor_pos.or(cursor);
        let new_lines = apply_line_resets(new_lines);
        let new_lines_ref = &new_lines;

        // First render - just output everything without clearing (assumes clean screen)
        if self.previous_lines.is_empty() && !width_changed && !height_changed {
            self.full_render(
                output,
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
                output,
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
                output,
                true,
                new_lines_ref,
                height,
                height_u16,
                width,
                cursor_pos,
            )?;
            return Ok(());
        }

        // Height changes normally need a full re-render to keep the viewport aligned.
        // Termux changes height when the software keyboard opens/closes, so pi
        // keeps the diff path there to avoid replaying history on every toggle.
        if height_change_requires_clear(height_changed, is_termux_session()) {
            self.full_render(
                output,
                true,
                new_lines_ref,
                height,
                height_u16,
                width,
                cursor_pos,
            )?;
            return Ok(());
        }

        // Content shrank below the historical high-water mark. We only force a
        // viewport redraw when explicitly opted in (e.g. a closed overlay).
        if self.clear_on_shrink && new_lines.len() < self.max_lines_rendered {
            self.full_render(
                output,
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
        if first_changed != -1 {
            let (expanded_first, expanded_last) = self.expand_changed_range_for_kitty_images(
                first_changed as usize,
                last_changed as usize,
                &new_lines,
            );
            first_changed = expanded_first as i64;
            last_changed = expanded_last as i64;
        }
        let append_start = appended_lines
            && first_changed == self.previous_lines.len() as i64
            && first_changed > 0;

        // No changes - but still need to update hardware cursor position if it moved
        if first_changed == -1 {
            self.position_hardware_cursor(output, cursor_pos, new_lines.len())?;
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
                buffer.push_str(&self.delete_changed_kitty_images(first_changed_u, last_changed_u));
                // Move to end of new content (clamp to 0 for empty content)
                let target_row = new_lines.len().saturating_sub(1);
                if target_row < prev_viewport_top {
                    self.full_render(
                        output,
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
                        output,
                        true,
                        new_lines_ref,
                        height,
                        height_u16,
                        width,
                        cursor_pos,
                    )?;
                    return Ok(());
                }
                let clear_start_offset = if new_lines.is_empty() { 0 } else { 1 };
                if extra_lines > 0 && clear_start_offset > 0 {
                    buffer.push_str(&format!("\x1b[{clear_start_offset}B"));
                }
                for i in 0..extra_lines {
                    buffer.push_str("\r\x1b[2K");
                    if i < extra_lines - 1 {
                        buffer.push_str("\x1b[1B");
                    }
                }
                let move_back = extra_lines.saturating_sub(1) + clear_start_offset;
                if move_back > 0 {
                    buffer.push_str(&format!("\x1b[{move_back}A"));
                }
                buffer.push_str("\x1b[?2026l");
                if debug_log_enabled() {
                    let _ = write_output_log("deleted-lines", &buffer);
                }
                let _ = output.write_all(buffer.as_bytes());
                let _ = output.flush();
                self.cursor_row = target_row;
                self.hardware_cursor_row = target_row;
            }
            self.position_hardware_cursor(output, cursor_pos, new_lines.len())?;
            self.previous_lines = new_lines;
            self.previous_kitty_image_ids = collect_kitty_image_ids(&self.previous_lines);
            self.previous_width = width;
            self.previous_height = height_u16;
            self.previous_viewport_top = prev_viewport_top;
            return Ok(());
        }

        // Differential rendering can only touch what was actually visible.
        // If the first changed line is above the previous viewport, use a full
        // clear render.
        if first_changed_u < prev_viewport_top {
            self.full_render(
                output,
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
        buffer.push_str(&self.delete_changed_kitty_images(first_changed_u, last_changed_u));
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

        let render_end = last_changed_u.min(new_lines.len().saturating_sub(1));
        let mut i = first_changed_u;
        while i <= render_end {
            if i > first_changed_u {
                buffer.push_str("\r\n");
            }
            let line = &new_lines[i];
            let image_reserved_rows = if is_image_line(line) {
                get_kitty_image_reserved_rows(&new_lines, i, render_end)
            } else {
                1
            };
            if image_reserved_rows > 1 {
                let image_start_screen_row = i as isize - viewport_top as isize;
                if image_start_screen_row < 0
                    || image_start_screen_row as usize + image_reserved_rows > height
                {
                    self.full_render(
                        output,
                        true,
                        new_lines_ref,
                        height,
                        height_u16,
                        width,
                        cursor_pos,
                    )?;
                    return Ok(());
                }
                buffer.push_str("\x1b[2K");
                for _ in 1..image_reserved_rows {
                    buffer.push_str("\r\n\x1b[2K");
                }
                buffer.push_str(&format!("\x1b[{}A", image_reserved_rows - 1));
                buffer.push_str(line);
                buffer.push_str(&format!("\x1b[{}B", image_reserved_rows - 1));
                i += image_reserved_rows;
                continue;
            }
            buffer.push_str("\x1b[2K"); // Clear current line
            buffer.push_str(line);
            i += 1;
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

        if debug_log_enabled() {
            let _ = write_output_log("diff-render", &buffer);
        }

        if debug_log_enabled() {
            let _ = write_debug_log(
                "diff-render",
                width,
                height,
                &new_lines,
                &self.previous_lines,
                Some(&format!(
                    "first_changed={first_changed_u} last_changed={last_changed_u} append_start={append_start} prev_viewport_top={prev_viewport_top} viewport_top={viewport_top} hardware_cursor_row={hardware_cursor_row} move_target_row={move_target_row} render_end={render_end} final_cursor_row={final_cursor_row}"
                )),
            );
        }

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
        self.position_hardware_cursor(output, cursor_pos, new_lines.len())?;

        self.previous_lines = new_lines;
        self.previous_kitty_image_ids = collect_kitty_image_ids(&self.previous_lines);
        self.previous_width = width;
        self.previous_height = height_u16;
        Ok(())
    }

    /// Full render: optionally clear screen/scrollback, then write the full
    /// rendered frame.
    fn full_render(
        &mut self,
        output: &mut dyn Write,
        clear: bool,
        new_lines: &[String],
        height: usize,
        height_u16: u16,
        width: u16,
        cursor_pos: Option<CursorPos>,
    ) -> std::io::Result<()> {
        if debug_log_enabled() {
            let _ = write_debug_log(
                &format!("full-render-{clear}"),
                width,
                height,
                new_lines,
                &self.previous_lines,
                None,
            );
        }
        let mut buffer = String::with_capacity(8192);
        buffer.push_str("\x1b[?2026h");
        if clear {
            buffer.push_str(&delete_kitty_images(&self.previous_kitty_image_ids));
            buffer.push_str("\x1b[2J\x1b[H\x1b[3J");
        }
        let mut i = 0;
        while i < new_lines.len() {
            if i > 0 {
                buffer.push_str("\r\n");
            }
            let line = &new_lines[i];
            let image_reserved_rows = if is_image_line(line) {
                get_kitty_image_reserved_rows(new_lines, i, new_lines.len().saturating_sub(1))
            } else {
                1
            };
            if image_reserved_rows > 1 && image_reserved_rows <= height {
                for _ in 1..image_reserved_rows {
                    buffer.push_str("\r\n");
                }
                buffer.push_str(&format!("\x1b[{}A", image_reserved_rows - 1));
                buffer.push_str(line);
                buffer.push_str(&format!("\x1b[{}B", image_reserved_rows - 1));
                i += image_reserved_rows;
                continue;
            }
            buffer.push_str(line);
            i += 1;
        }
        buffer.push_str("\x1b[?2026l");
        if debug_log_enabled() {
            let _ = write_output_log(&format!("full-render-{clear}"), &buffer);
        }
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
        self.previous_kitty_image_ids = collect_kitty_image_ids(&self.previous_lines);
        self.previous_width = width;
        self.previous_height = height_u16;
        Ok(())
    }

    /// Position the hardware cursor for IME candidate windows.
    fn position_hardware_cursor(
        &mut self,
        output: &mut dyn Write,
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
        if self.show_hardware_cursor {
            buffer.push_str("\x1b[?25h");
        } else {
            buffer.push_str("\x1b[?25l");
        }

        if !buffer.is_empty() {
            let _ = output.write_all(buffer.as_bytes());
            let _ = output.flush();
        }

        self.hardware_cursor_row = target_row;
        Ok(())
    }
}

fn apply_line_resets(mut lines: Vec<String>) -> Vec<String> {
    for line in &mut lines {
        if !is_image_line(line) {
            normalize_terminal_output(line);
            line.push_str(SEGMENT_RESET);
        }
    }
    lines
}

fn normalize_terminal_output(line: &mut String) {
    if line.contains('\n') || line.contains('\r') {
        *line = line.replace(['\n', '\r'], "");
    }
}

fn extract_cursor_position(lines: &mut [String], height: usize) -> Option<CursorPos> {
    let viewport_top = lines.len().saturating_sub(height);
    for row in (viewport_top..lines.len()).rev() {
        let Some(marker_index) = lines[row].find(CURSOR_MARKER) else {
            continue;
        };
        let before_marker = &lines[row][..marker_index];
        let col = visible_width(before_marker);
        let after_marker = marker_index + CURSOR_MARKER.len();
        lines[row].replace_range(marker_index..after_marker, "");
        return Some(CursorPos { row, col });
    }
    None
}

fn is_image_line(line: &str) -> bool {
    line.contains(KITTY_SEQUENCE_PREFIX) || line.contains("\x1b]1337;File=")
}

fn collect_kitty_image_ids(lines: &[String]) -> BTreeSet<u32> {
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

fn get_kitty_image_reserved_rows(lines: &[String], index: usize, max_index: usize) -> usize {
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

fn delete_kitty_images(ids: &BTreeSet<u32>) -> String {
    let mut buffer = String::new();
    for id in ids {
        buffer.push_str(&format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\"));
    }
    buffer
}

impl TuiRenderer {
    fn expand_changed_range_for_kitty_images(
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

    fn delete_changed_kitty_images(&self, first_changed: usize, last_changed: usize) -> String {
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

impl Drop for TuiRenderer {
    fn drop(&mut self) {
        self.leave();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_renderer(previous_lines: Vec<String>) -> TuiRenderer {
        let line_count = previous_lines.len();
        TuiRenderer {
            previous_lines,
            previous_kitty_image_ids: BTreeSet::new(),
            viewport_top: 0,
            previous_viewport_top: 0,
            hardware_cursor_row: line_count.saturating_sub(1),
            previous_width: if line_count == 0 { 0 } else { 80 },
            previous_height: if line_count == 0 { 0 } else { 24 },
            first_render: line_count == 0,
            max_lines_rendered: line_count,
            cursor_row: line_count.saturating_sub(1),
            clear_on_shrink: false,
            show_hardware_cursor: false,
        }
    }

    #[test]
    fn cursor_pos_is_copy() {
        let pos = CursorPos { row: 1, col: 2 };
        let pos2 = pos;
        assert_eq!(pos, pos2);
    }

    #[test]
    fn shrink_uses_pi_full_clear_when_enabled() {
        let mut renderer = test_renderer(vec![
            "line0".to_owned(),
            "line1".to_owned(),
            "line2".to_owned(),
        ]);
        renderer.clear_on_shrink = true;
        let mut buf: Vec<u8> = Vec::new();
        renderer
            .render_to_with_size(
                &mut buf,
                80,
                24,
                vec!["line0".to_owned(), "line1".to_owned()],
                None,
            )
            .unwrap();
        let output = String::from_utf8_lossy(&buf);
        assert!(output.contains("\x1b[2J\x1b[H\x1b[3J"));
        assert_eq!(
            renderer.max_lines_rendered, 2,
            "high-water mark should reset to new line count"
        );

        // A second frame of the same size must not force another clear.
        buf.clear();
        renderer
            .render_to_with_size(
                &mut buf,
                80,
                24,
                vec!["line0".to_owned(), "line1".to_owned()],
                None,
            )
            .unwrap();
        let output2 = String::from_utf8_lossy(&buf);
        assert!(
            !output2.contains("\x1b[2J"),
            "same small content should not clear again: {output2:?}"
        );
        assert_eq!(
            renderer.max_lines_rendered, 2,
            "high-water mark should stay at current line count"
        );
    }

    #[test]
    fn shrink_to_empty_uses_pi_full_clear_when_enabled() {
        let mut renderer = test_renderer(vec!["line0".to_owned(), "line1".to_owned()]);
        renderer.clear_on_shrink = true;
        let mut buf = Vec::new();
        renderer
            .render_to_with_size(&mut buf, 80, 24, Vec::new(), None)
            .unwrap();
        let output = String::from_utf8_lossy(&buf);
        assert!(output.contains("\x1b[2J\x1b[H\x1b[3J"));
        assert_eq!(renderer.max_lines_rendered, 0);
    }

    #[test]
    fn shrink_does_not_clear_by_default() {
        let mut renderer = test_renderer(vec![
            "line0".to_owned(),
            "line1".to_owned(),
            "line2".to_owned(),
        ]);
        let mut buf: Vec<u8> = Vec::new();
        renderer
            .render_to_with_size(
                &mut buf,
                80,
                24,
                vec!["line0".to_owned(), "line1".to_owned()],
                None,
            )
            .unwrap();
        let output = String::from_utf8_lossy(&buf);
        assert!(
            !output.contains("\x1b[2J"),
            "default shrink should not clear screen: {output:?}"
        );
        assert!(
            !output.contains("\x1b[3J"),
            "default shrink should not wipe scrollback: {output:?}"
        );
        // Differential rendering for a deleted trailing line only emits cursor
        // moves/clears; unchanged lines are assumed to already be on screen.
        assert!(
            output.contains("\x1b[2K"),
            "default shrink should clear the obsolete line: {output:?}"
        );
    }

    #[test]
    fn first_render_of_tall_content_outputs_full_frame_without_clear() {
        let mut renderer = test_renderer(Vec::new());
        let new_lines: Vec<String> = (0..100).map(|i| format!("line{i:03}")).collect();
        let mut buf: Vec<u8> = Vec::new();
        renderer
            .render_to_with_size(&mut buf, 80, 24, new_lines, None)
            .unwrap();
        let output = String::from_utf8_lossy(&buf);
        assert!(output.contains("line000"));
        assert!(
            output.contains("line099"),
            "first render should emit the whole frame: {output:?}"
        );
        assert_eq!(
            output.matches("\r\n").count(),
            99,
            "should emit the whole frame: {output:?}"
        );
        assert_eq!(renderer.previous_viewport_top, 76);
    }

    #[test]
    fn full_render_clears_screen_and_scrollback_when_first_changed_is_above_viewport() {
        let previous_lines: Vec<String> = (0..100).map(|i| format!("old{i:03}")).collect();
        let mut renderer = test_renderer(previous_lines.clone());
        renderer.previous_viewport_top = 50;
        renderer.hardware_cursor_row = 99;
        renderer.cursor_row = 99;
        let new_lines: Vec<String> = (0..100).map(|i| format!("new{i:03}")).collect();
        let mut buf: Vec<u8> = Vec::new();
        renderer
            .render_to_with_size(&mut buf, 80, 24, new_lines, None)
            .unwrap();
        let output = String::from_utf8_lossy(&buf);
        assert!(output.contains("\x1b[2J\x1b[H\x1b[3J"));
        assert!(output.contains("new000"));
        assert!(
            output.contains("new099"),
            "full redraw should emit the whole frame: {output:?}"
        );
        assert!(
            !output.contains("old050"),
            "previous content should be overwritten: {output:?}"
        );
        assert_eq!(
            output.matches("\r\n").count(),
            99,
            "should emit the whole frame: {output:?}"
        );
        assert_eq!(renderer.previous_viewport_top, 76);
    }

    #[test]
    fn full_render_deletes_previous_kitty_images_before_clearing() {
        let mut renderer = test_renderer(vec!["\x1b_Gi=42,r=1;payload\x1b\\".to_owned()]);
        renderer.previous_kitty_image_ids = collect_kitty_image_ids(&renderer.previous_lines);
        renderer.previous_width = 80;
        renderer.previous_height = 24;
        renderer.first_render = false;

        let mut buf = Vec::new();
        renderer
            .render_to_with_size(&mut buf, 120, 24, vec!["plain".to_owned()], None)
            .unwrap();
        let output = String::from_utf8_lossy(&buf);

        assert!(output.contains("\x1b_Ga=d,d=I,i=42,q=2\x1b\\"));
        assert!(output.contains("\x1b[2J\x1b[H\x1b[3J"));
    }

    #[test]
    fn renderer_extracts_visible_cursor_marker() {
        let mut renderer = test_renderer(Vec::new());
        let mut buf = Vec::new();
        renderer
            .render_to_with_size(
                &mut buf,
                80,
                10,
                vec![format!("prompt {CURSOR_MARKER}text")],
                None,
            )
            .unwrap();

        let output = String::from_utf8_lossy(&buf);
        assert!(!output.contains(CURSOR_MARKER));
        assert!(
            output.contains("\x1b[8G"),
            "cursor col should follow prompt: {output:?}"
        );
    }

    #[test]
    fn diff_render_skips_reserved_kitty_image_rows() {
        let mut renderer = test_renderer(vec![
            "\x1b_Gi=7,r=3;payload\x1b\\".to_owned(),
            "".to_owned(),
            "".to_owned(),
        ]);
        renderer.previous_kitty_image_ids = collect_kitty_image_ids(&renderer.previous_lines);
        let mut buf = Vec::new();
        renderer
            .render_to_with_size(
                &mut buf,
                80,
                24,
                vec![
                    "\x1b_Gi=8,r=3;payload\x1b\\".to_owned(),
                    "".to_owned(),
                    "".to_owned(),
                ],
                None,
            )
            .unwrap();

        let output = String::from_utf8_lossy(&buf);
        assert!(output.contains("\x1b_Ga=d,d=I,i=7,q=2\x1b\\"));
        assert_eq!(
            output.matches("\x1b[2K").count(),
            3,
            "image block should be pre-cleared once per reserved row: {output:?}"
        );
    }

    #[test]
    fn termux_height_change_uses_diff_path() {
        assert!(!height_change_requires_clear(true, true));
        assert!(height_change_requires_clear(true, false));
        assert!(!height_change_requires_clear(false, false));
    }

    #[test]
    fn cursor_position_hides_hardware_cursor_by_default() {
        let mut renderer = test_renderer(vec!["hello".to_owned()]);
        renderer.hardware_cursor_row = 0;
        let mut buf = Vec::new();

        renderer
            .position_hardware_cursor(&mut buf, Some(CursorPos { row: 0, col: 3 }), 1)
            .unwrap();
        let output = String::from_utf8_lossy(&buf);

        assert!(output.contains("\x1b[4G"));
        assert!(output.contains("\x1b[?25l"));
        assert!(!output.contains("\x1b[?25h"));
    }

    #[test]
    fn diff_render_redraws_tail_when_middle_rows_are_inserted() {
        let mut renderer = test_renderer(vec![
            "intro".to_owned(),
            "```bash".to_owned(),
            "\x1b[2m│  >\x1b[0m".to_owned(),
        ]);
        renderer.previous_height = 10;
        let new_lines = vec![
            "intro".to_owned(),
            "```bash".to_owned(),
            "cargo run -p neo-agent -- models list".to_owned(),
            "```".to_owned(),
            "\x1b[2m│  >\x1b[0m".to_owned(),
        ];

        let mut buf = Vec::new();
        renderer
            .render_to_with_size(&mut buf, 80, 10, new_lines, None)
            .unwrap();
        let output = String::from_utf8_lossy(&buf);

        assert!(
            output.contains("cargo run -p neo-agent -- models list"),
            "inserted code row should render: {output:?}"
        );
        assert!(
            output.contains("```"),
            "closing fence should render: {output:?}"
        );
        assert!(
            !output.contains("\x1b[2J"),
            "middle insertion must not clear the whole visible screen: {output:?}"
        );
        assert!(
            !output.contains("\x1b[3J"),
            "viewport redraw must preserve user scrollback: {output:?}"
        );
    }
}
