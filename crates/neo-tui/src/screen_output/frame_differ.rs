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
use std::fmt::Write as _;
use std::io::{Write, stdout};

use crossterm::{
    event::{
        DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute, queue,
    terminal::{disable_raw_mode, enable_raw_mode, size},
};

use crate::primitive::visible_width;
use crate::primitive::{truncate_width, wrap_width};
use crate::terminal_capabilities::TerminalCapabilities;

use super::debug_log::{check_line_widths, debug_log_enabled, write_debug_log, write_output_log};
use super::kitty_image::{
    collect_kitty_image_ids, delete_kitty_images, get_kitty_image_reserved_rows, image_block_fits,
    is_image_line, push_image_block, reserved_render_rows,
};

const SEGMENT_RESET: &str = "\x1b[0m\x1b]8;;\x07";
const SYNCHRONIZED_OUTPUT_END: &[u8] = b"\x1b[?2026l";

/// Cursor position for prompt editing (row, col) in the rendered content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPos {
    pub row: usize,
    pub col: usize,
}

/// A zero-width cursor marker embedded in rendered output.
/// The renderer finds this marker, strips it, and positions the hardware cursor.
pub const CURSOR_MARKER: &str = "\x1b_pi:c\x07";

fn is_termux_session() -> bool {
    env::var("TERMUX_VERSION").is_ok()
}

fn hardware_cursor_enabled_from_env_value(value: Option<&str>) -> bool {
    !matches!(
        value.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("0" | "false" | "off")
    )
}

const fn height_change_requires_clear(height_changed: bool, is_termux: bool) -> bool {
    height_changed && !is_termux
}

#[cfg(windows)]
pub(super) mod windows_input_mode {
    use std::io;

    const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;

    #[derive(Debug, Clone, Copy)]
    pub(super) struct WindowsInputModeGuard {
        original_mode: u32,
        changed: bool,
    }

    impl WindowsInputModeGuard {
        pub(super) const fn inactive() -> Self {
            Self {
                original_mode: 0,
                changed: false,
            }
        }

        pub(super) fn enter() -> io::Result<Self> {
            let stdin = io::stdin();
            let Ok(mode) = winapi_util::console::mode(&stdin) else {
                return Ok(Self::inactive());
            };
            let vt_mode = mode | ENABLE_VIRTUAL_TERMINAL_INPUT;
            if vt_mode == mode {
                return Ok(Self::inactive());
            }
            winapi_util::console::set_mode(&stdin, vt_mode)?;
            Ok(Self {
                original_mode: mode,
                changed: true,
            })
        }

        pub(super) fn restore(&mut self) {
            if !self.changed {
                return;
            }
            let stdin = io::stdin();
            let _ = winapi_util::console::set_mode(&stdin, self.original_mode);
            self.changed = false;
        }
    }
}

#[allow(clippy::struct_excessive_bools)]
pub struct TuiRenderer {
    pub(super) previous_lines: Vec<String>,
    pub(super) previous_kitty_image_ids: BTreeSet<u32>,
    /// Content row index of the top of the visible viewport.
    pub(super) viewport_top: usize,
    pub(super) previous_viewport_top: usize,
    /// Rendered content row currently occupied by the terminal cursor.
    pub(super) hardware_cursor_row: usize,
    pub(super) previous_width: u16,
    pub(super) previous_height: u16,
    /// A failed write leaves terminal contents unknown, so retry with a full redraw.
    pub(super) force_full_redraw: bool,
    /// Whether this is the first render (no diff, just output everything).
    pub(super) first_render: bool,
    /// Track terminal's working area (max lines ever rendered).
    /// Grows but doesn't shrink unless the renderer takes a clear path.
    pub(super) max_lines_rendered: usize,
    /// Logical end-of-content row.
    pub(super) cursor_row: usize,
    /// Defaults to off; when enabled, a shrink below the historical high-water
    /// mark takes the full clear path.
    pub(super) clear_on_shrink: bool,
    pub(super) show_hardware_cursor: bool,
    pub(super) capabilities: TerminalCapabilities,
    pub(super) debug_frame_id: u64,
    #[cfg(windows)]
    windows_input_mode: windows_input_mode::WindowsInputModeGuard,
}

#[derive(Clone, Copy)]
pub(super) struct RenderDimensions {
    pub(super) width: u16,
    pub(super) height: usize,
    pub(super) height_u16: u16,
}

impl RenderDimensions {
    const fn new(width: u16, height_u16: u16) -> Self {
        Self {
            width,
            height: height_u16 as usize,
            height_u16,
        }
    }
}

#[derive(Clone, Copy)]
struct RenderChangeFlags {
    width_changed: bool,
    height_changed: bool,
}

#[derive(Clone, Copy)]
pub(super) struct ViewportState {
    pub(super) previous_top: usize,
    pub(super) top: usize,
    pub(super) hardware_cursor_row: usize,
}

impl ViewportState {
    fn from_renderer(
        renderer: &TuiRenderer,
        dimensions: RenderDimensions,
        height_changed: bool,
    ) -> Self {
        let previous_buffer_length = renderer.previous_buffer_length(dimensions.height);
        let previous_top = if height_changed {
            previous_buffer_length.saturating_sub(dimensions.height)
        } else {
            renderer.previous_viewport_top
        };

        Self {
            previous_top,
            top: previous_top,
            hardware_cursor_row: renderer.hardware_cursor_row,
        }
    }

    fn line_diff(self, target_row: usize) -> isize {
        let current_screen_row =
            self.hardware_cursor_row.cast_signed() - self.previous_top.cast_signed();
        let target_screen_row = target_row.cast_signed() - self.top.cast_signed();
        target_screen_row - current_screen_row
    }

    fn scroll_to_row(&mut self, buffer: &mut String, target_row: usize, height: usize) {
        let previous_bottom = self.previous_top + height - 1;
        if target_row <= previous_bottom {
            return;
        }

        let current_screen_row = (self.hardware_cursor_row.cast_signed()
            - self.previous_top.cast_signed())
        .clamp(0, (height - 1).cast_signed())
        .cast_unsigned();
        let move_to_bottom = height - 1 - current_screen_row;
        if move_to_bottom > 0 {
            let _ = write!(buffer, "\x1b[{move_to_bottom}B");
        }

        let scroll = target_row - previous_bottom;
        for _ in 0..scroll {
            buffer.push_str("\r\n");
        }
        self.previous_top += scroll;
        self.top += scroll;
        self.hardware_cursor_row = target_row;
    }
}

#[derive(Clone, Copy)]
pub(super) struct ChangeRange {
    pub(super) first: usize,
    pub(super) last: usize,
    pub(super) append_start: bool,
}

impl ChangeRange {
    fn render_end(self, len: usize) -> usize {
        self.last.min(len.saturating_sub(1))
    }

    const fn move_target_row(self) -> usize {
        if self.append_start {
            self.first.saturating_sub(1)
        } else {
            self.first
        }
    }
}

enum ChangedLinesRender {
    Rendered { render_end: usize },
    NeedsFullRender,
}

pub(super) struct DiffRender {
    pub(super) buffer: String,
    pub(super) change_range: ChangeRange,
    pub(super) viewport: ViewportState,
    pub(super) move_target_row: usize,
    pub(super) render_end: usize,
    pub(super) final_cursor_row: usize,
}

struct ConstrainedFrameLines {
    lines: Vec<String>,
    row_starts: Vec<usize>,
}

impl ConstrainedFrameLines {
    fn map_cursor(&self, cursor: CursorPos) -> CursorPos {
        let Some(&row_start) = self.row_starts.get(cursor.row) else {
            return cursor;
        };
        CursorPos {
            row: row_start,
            col: cursor.col,
        }
    }
}

fn push_vertical_move(buffer: &mut String, line_diff: isize) {
    if line_diff > 0 {
        let _ = write!(buffer, "\x1b[{line_diff}B");
    } else if line_diff < 0 {
        let _ = write!(buffer, "\x1b[{}A", -line_diff);
    }
}

fn push_diff_start(buffer: &mut String, append_start: bool) {
    if append_start {
        buffer.push_str("\r\n");
    } else {
        buffer.push('\r');
    }
}

#[derive(Clone, Copy, Default)]
struct TerminalProtocolState {
    bracketed_paste: bool,
    kitty_keyboard: bool,
}

impl TerminalProtocolState {
    const fn for_capabilities(capabilities: TerminalCapabilities) -> Self {
        Self {
            bracketed_paste: capabilities.ansi.bracketed_paste,
            kitty_keyboard: capabilities.ansi.kitty_keyboard,
        }
    }
}

fn write_enter_output_with_state(
    output: &mut dyn Write,
    capabilities: TerminalCapabilities,
    state: &mut TerminalProtocolState,
) -> std::io::Result<()> {
    let mut output = output;
    if capabilities.ansi.bracketed_paste {
        queue!(&mut output, EnableBracketedPaste)?;
        state.bracketed_paste = true;
        output.flush()?;
    }
    if capabilities.ansi.kitty_keyboard {
        queue!(
            &mut output,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS,
            )
        )?;
        state.kitty_keyboard = true;
        output.flush()?;
    }
    Ok(())
}

#[cfg(test)]
fn write_enter_output(
    output: &mut dyn Write,
    capabilities: TerminalCapabilities,
) -> std::io::Result<()> {
    let mut state = TerminalProtocolState::default();
    write_enter_output_with_state(output, capabilities, &mut state)
}

fn write_leave_terminal_output_for_state(
    output: &mut dyn Write,
    state: TerminalProtocolState,
) -> std::io::Result<()> {
    let mut output = output;
    let mut result = Ok(());
    if state.kitty_keyboard
        && let Err(error) = execute!(&mut output, PopKeyboardEnhancementFlags)
    {
        result = Err(error);
    }
    if state.bracketed_paste
        && let Err(error) = execute!(&mut output, DisableBracketedPaste)
        && result.is_ok()
    {
        result = Err(error);
    }
    result
}

fn write_leave_terminal_output(
    output: &mut dyn Write,
    capabilities: TerminalCapabilities,
) -> std::io::Result<()> {
    write_leave_terminal_output_for_state(
        output,
        TerminalProtocolState::for_capabilities(capabilities),
    )
}

fn write_render_output(
    output: &mut dyn Write,
    buffer: &str,
    synchronized_output: bool,
) -> std::io::Result<()> {
    let result = output
        .write_all(buffer.as_bytes())
        .and_then(|()| output.flush());
    if let Err(error) = result {
        if synchronized_output {
            let _ = output.write_all(SYNCHRONIZED_OUTPUT_END);
            let _ = output.flush();
        }
        return Err(error);
    }
    Ok(())
}

struct RawModeGuard {
    active: bool,
}

impl RawModeGuard {
    fn enter() -> std::io::Result<Self> {
        enable_raw_mode()?;
        Ok(Self { active: true })
    }

    fn disarm(mut self) {
        self.active = false;
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = disable_raw_mode();
        }
    }
}

impl TuiRenderer {
    /// Enable raw mode + bracketed paste + keyboard enhancement.
    /// Does NOT enter alternate screen.
    pub fn enter(capabilities: TerminalCapabilities) -> std::io::Result<Self> {
        let raw_mode = RawModeGuard::enter()?;
        #[cfg(windows)]
        let mut windows_input_mode = windows_input_mode::WindowsInputModeGuard::enter()?;
        let mut output = stdout();
        let mut protocol_state = TerminalProtocolState::default();
        if let Err(error) =
            write_enter_output_with_state(&mut output, capabilities, &mut protocol_state)
        {
            let _ = write_leave_terminal_output_for_state(&mut output, protocol_state);
            #[cfg(windows)]
            windows_input_mode.restore();
            return Err(error);
        }
        raw_mode.disarm();
        Ok(Self {
            previous_lines: Vec::new(),
            previous_kitty_image_ids: BTreeSet::new(),
            viewport_top: 0,
            previous_viewport_top: 0,
            hardware_cursor_row: 0,
            previous_width: 0,
            previous_height: 0,
            force_full_redraw: false,
            first_render: true,
            max_lines_rendered: 0,
            cursor_row: 0,
            clear_on_shrink: false,
            show_hardware_cursor: hardware_cursor_enabled_from_env_value(
                env::var("NEO_HARDWARE_CURSOR").ok().as_deref(),
            ),
            capabilities,
            debug_frame_id: 0,
            #[cfg(windows)]
            windows_input_mode,
        })
    }

    /// Restore terminal state.
    pub fn leave(&mut self) {
        let mut output = stdout();
        self.write_leave_output(&mut output);
        let _ = output.flush();

        let _ = write_leave_terminal_output(&mut output, self.capabilities);
        #[cfg(windows)]
        self.windows_input_mode.restore();
        let _ = disable_raw_mode();
    }

    fn write_leave_output(&mut self, output: &mut dyn Write) {
        // Move cursor to the end of the content to prevent overwriting on exit.
        if !self.previous_lines.is_empty() {
            let target_row = self.previous_lines.len(); // Line after the last content
            let line_diff = target_row.cast_signed() - self.hardware_cursor_row.cast_signed();
            if line_diff > 0 {
                let _ = write!(output, "\x1b[{line_diff}B");
            } else if line_diff < 0 {
                let _ = write!(output, "\x1b[{}A", (-line_diff));
            }
            let _ = write!(output, "\r\n");
        }
        let _ = write!(output, "\x1b[?25h");
    }

    /// Suspend (Ctrl+Z): leave terminal, then re-enter.
    pub fn suspend_prepare(&mut self) {
        self.leave();
    }

    /// Re-enter after suspend.
    pub fn suspend_resume(&mut self) -> std::io::Result<()> {
        let raw_mode = RawModeGuard::enter()?;
        #[cfg(windows)]
        {
            self.windows_input_mode.restore();
            self.windows_input_mode = windows_input_mode::WindowsInputModeGuard::enter()?;
        }
        let mut output = stdout();
        let mut protocol_state = TerminalProtocolState::default();
        if let Err(error) =
            write_enter_output_with_state(&mut output, self.capabilities, &mut protocol_state)
        {
            let _ = write_leave_terminal_output_for_state(&mut output, protocol_state);
            #[cfg(windows)]
            self.windows_input_mode.restore();
            return Err(error);
        }
        raw_mode.disarm();
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
        new_lines: Vec<String>,
        cursor: Option<CursorPos>,
    ) -> std::io::Result<()> {
        match self.render_to_with_size_inner(output, width, height_u16, new_lines, cursor) {
            Ok(()) => {
                self.force_full_redraw = false;
                Ok(())
            }
            Err(error) => {
                self.force_full_redraw = true;
                Err(error)
            }
        }
    }

    fn render_to_with_size_inner(
        &mut self,
        output: &mut dyn Write,
        width: u16,
        height_u16: u16,
        mut new_lines: Vec<String>,
        cursor: Option<CursorPos>,
    ) -> std::io::Result<()> {
        let dimensions = RenderDimensions::new(width, height_u16);
        self.log_render_start(dimensions, &new_lines);

        let constrained = constrain_frame_lines(width, new_lines);
        let cursor = cursor.map(|cursor| constrained.map_cursor(cursor));
        new_lines = constrained.lines;
        check_line_widths(width, &new_lines)?;

        let width_changed = self.previous_width != 0 && self.previous_width != width;
        let height_changed = self.previous_height != 0 && self.previous_height != height_u16;
        let viewport = ViewportState::from_renderer(self, dimensions, height_changed);

        let marker_cursor_pos = extract_cursor_position(&mut new_lines, dimensions.height);
        let cursor_pos = marker_cursor_pos.or(cursor);
        let new_lines = apply_line_resets(new_lines);
        let new_lines_ref = &new_lines;

        if self.try_early_full_render(
            output,
            new_lines_ref,
            dimensions,
            cursor_pos,
            RenderChangeFlags {
                width_changed,
                height_changed,
            },
        )? {
            return Ok(());
        }

        // Content shrank below the historical high-water mark. We only force a
        // viewport redraw when explicitly opted in (e.g. a closed overlay).
        if self.clear_on_shrink && new_lines.len() < self.max_lines_rendered {
            self.full_render_with_dimensions(output, true, new_lines_ref, dimensions, cursor_pos)?;
            return Ok(());
        }

        let change_range = self.changed_range(&new_lines);

        // No changes - but still need to update hardware cursor position if it moved
        let Some(change_range) = change_range else {
            let hardware_cursor_row = self.position_hardware_cursor(
                output,
                cursor_pos,
                new_lines.len(),
                self.hardware_cursor_row,
            )?;
            self.previous_viewport_top = viewport.previous_top;
            self.previous_height = height_u16;
            self.previous_lines = new_lines;
            self.hardware_cursor_row = hardware_cursor_row;
            return Ok(());
        };

        // All changes are in deleted lines (nothing to render, just clear)
        if change_range.first >= new_lines.len() {
            if !self.render_deleted_tail(
                output,
                dimensions,
                new_lines_ref,
                change_range,
                viewport,
            )? {
                self.full_render_with_dimensions(
                    output,
                    true,
                    new_lines_ref,
                    dimensions,
                    cursor_pos,
                )?;
                return Ok(());
            }
            self.finish_deleted_tail(
                output,
                new_lines,
                cursor_pos,
                dimensions,
                viewport.previous_top,
            )?;
            return Ok(());
        }

        // Differential rendering can only touch what was actually visible.
        // If the first changed line is above the previous viewport, use a full
        // clear render.
        if change_range.first < viewport.previous_top {
            self.full_render_with_dimensions(output, true, new_lines_ref, dimensions, cursor_pos)?;
            return Ok(());
        }

        let Some(diff_render) =
            self.build_diff_render(&new_lines, change_range, viewport, dimensions)
        else {
            self.full_render_with_dimensions(output, true, new_lines_ref, dimensions, cursor_pos)?;
            return Ok(());
        };
        self.finish_diff_render(output, new_lines, cursor_pos, dimensions, &diff_render)
    }

    fn try_early_full_render(
        &mut self,
        output: &mut dyn Write,
        new_lines: &[String],
        dimensions: RenderDimensions,
        cursor_pos: Option<CursorPos>,
        changes: RenderChangeFlags,
    ) -> std::io::Result<bool> {
        let Some(clear) = self.early_full_render_clear(changes) else {
            return Ok(false);
        };
        self.full_render_with_dimensions(output, clear, new_lines, dimensions, cursor_pos)?;
        self.first_render = false;
        Ok(true)
    }

    fn full_render_with_dimensions(
        &mut self,
        output: &mut dyn Write,
        clear: bool,
        new_lines: &[String],
        dimensions: RenderDimensions,
        cursor_pos: Option<CursorPos>,
    ) -> std::io::Result<()> {
        self.full_render(
            output,
            clear,
            new_lines,
            dimensions.height,
            dimensions.height_u16,
            dimensions.width,
            cursor_pos,
        )
    }

    fn early_full_render_clear(&self, changes: RenderChangeFlags) -> Option<bool> {
        if self.force_full_redraw {
            return Some(true);
        }
        if self.previous_lines.is_empty() && !changes.width_changed && !changes.height_changed {
            return Some(false);
        }
        if self.first_render {
            return Some(false);
        }
        if changes.width_changed {
            return Some(true);
        }
        if height_change_requires_clear(changes.height_changed, is_termux_session()) {
            return Some(true);
        }
        None
    }

    fn build_diff_render(
        &self,
        new_lines: &[String],
        change_range: ChangeRange,
        mut viewport: ViewportState,
        dimensions: RenderDimensions,
    ) -> Option<DiffRender> {
        let mut buffer = String::with_capacity(4096);
        if self.capabilities.ansi.synchronized_output {
            buffer.push_str("\x1b[?2026h");
        }
        buffer.push_str(&self.delete_changed_kitty_images(change_range.first, change_range.last));
        let move_target_row = change_range.move_target_row();
        viewport.scroll_to_row(&mut buffer, move_target_row, dimensions.height);
        push_vertical_move(&mut buffer, viewport.line_diff(move_target_row));
        push_diff_start(&mut buffer, change_range.append_start);

        let ChangedLinesRender::Rendered { render_end } =
            render_changed_lines(&mut buffer, new_lines, change_range, viewport, dimensions)
        else {
            return None;
        };
        let final_cursor_row =
            self.push_removed_line_clears(&mut buffer, render_end, new_lines.len());
        if self.capabilities.ansi.synchronized_output {
            buffer.push_str("\x1b[?2026l");
        }

        Some(DiffRender {
            buffer,
            change_range,
            viewport,
            move_target_row,
            render_end,
            final_cursor_row,
        })
    }

    fn push_removed_line_clears(
        &self,
        buffer: &mut String,
        render_end: usize,
        new_len: usize,
    ) -> usize {
        if self.previous_lines.len() <= new_len {
            return render_end;
        }

        let mut final_cursor_row = render_end;
        if render_end + 1 < new_len {
            let move_down = new_len - 1 - render_end;
            let _ = write!(buffer, "\x1b[{move_down}B");
            final_cursor_row = new_len - 1;
        }
        let extra_lines = self.previous_lines.len() - new_len;
        for _ in new_len..self.previous_lines.len() {
            buffer.push_str("\r\n\x1b[2K");
        }
        let _ = write!(buffer, "\x1b[{extra_lines}A");
        final_cursor_row
    }

    fn finish_diff_render(
        &mut self,
        output: &mut dyn Write,
        new_lines: Vec<String>,
        cursor_pos: Option<CursorPos>,
        dimensions: RenderDimensions,
        diff_render: &DiffRender,
    ) -> std::io::Result<()> {
        self.log_diff_render(dimensions, &new_lines, diff_render);
        write_render_output(
            output,
            &diff_render.buffer,
            self.capabilities.ansi.synchronized_output,
        )?;

        let cursor_row = new_lines.len().saturating_sub(1);
        let max_lines_rendered = self.max_lines_rendered.max(new_lines.len());
        let previous_viewport_top = diff_render.viewport.previous_top.max(
            diff_render
                .final_cursor_row
                .saturating_sub(dimensions.height - 1),
        );
        let hardware_cursor_row = self.position_hardware_cursor(
            output,
            cursor_pos,
            new_lines.len(),
            diff_render.final_cursor_row,
        )?;

        self.previous_lines = new_lines;
        self.previous_kitty_image_ids = collect_kitty_image_ids(&self.previous_lines);
        self.cursor_row = cursor_row;
        self.hardware_cursor_row = hardware_cursor_row;
        self.max_lines_rendered = max_lines_rendered;
        self.previous_viewport_top = previous_viewport_top;
        self.previous_width = dimensions.width;
        self.previous_height = dimensions.height_u16;
        Ok(())
    }

    fn previous_buffer_length(&self, height: usize) -> usize {
        if self.previous_height > 0 {
            self.previous_viewport_top + usize::from(self.previous_height)
        } else {
            height
        }
    }

    fn changed_range(&self, new_lines: &[String]) -> Option<ChangeRange> {
        let (mut first_changed, mut last_changed) =
            raw_changed_range(&self.previous_lines, new_lines);
        let appended_lines = new_lines.len() > self.previous_lines.len();
        if appended_lines {
            if first_changed.is_none() {
                first_changed = Some(self.previous_lines.len());
            }
            last_changed = Some(new_lines.len().saturating_sub(1));
        }

        let (Some(first), Some(last)) = (first_changed, last_changed) else {
            return None;
        };
        let (first, last) = self.expand_changed_range_for_kitty_images(first, last, new_lines);
        let append_start = appended_lines && first == self.previous_lines.len() && first > 0;
        Some(ChangeRange {
            first,
            last,
            append_start,
        })
    }

    fn render_deleted_tail(
        &mut self,
        output: &mut dyn Write,
        dimensions: RenderDimensions,
        new_lines: &[String],
        change_range: ChangeRange,
        viewport: ViewportState,
    ) -> std::io::Result<bool> {
        if self.previous_lines.len() <= new_lines.len() {
            return Ok(true);
        }

        let target_row = new_lines.len().saturating_sub(1);
        let extra_lines = self.previous_lines.len() - new_lines.len();
        if target_row < viewport.previous_top || extra_lines > dimensions.height {
            return Ok(false);
        }

        let mut buffer = String::new();
        buffer.push_str("\x1b[?2026h");
        buffer.push_str(&self.delete_changed_kitty_images(change_range.first, change_range.last));
        push_vertical_move(&mut buffer, viewport.line_diff(target_row));
        buffer.push('\r');
        push_deleted_tail_clears(&mut buffer, extra_lines, !new_lines.is_empty());
        buffer.push_str("\x1b[?2026l");

        if debug_log_enabled() {
            let _ = write_output_log(self.debug_frame_id, "deleted-lines", &buffer);
        }
        write_render_output(output, &buffer, true)?;
        Ok(true)
    }

    fn finish_deleted_tail(
        &mut self,
        output: &mut dyn Write,
        new_lines: Vec<String>,
        cursor_pos: Option<CursorPos>,
        dimensions: RenderDimensions,
        previous_viewport_top: usize,
    ) -> std::io::Result<()> {
        let cursor_row = new_lines.len().saturating_sub(1);
        let hardware_cursor_row =
            self.position_hardware_cursor(output, cursor_pos, new_lines.len(), cursor_row)?;
        self.previous_lines = new_lines;
        self.previous_kitty_image_ids = collect_kitty_image_ids(&self.previous_lines);
        self.cursor_row = cursor_row;
        self.hardware_cursor_row = hardware_cursor_row;
        self.previous_width = dimensions.width;
        self.previous_height = dimensions.height_u16;
        self.previous_viewport_top = previous_viewport_top;
        Ok(())
    }

    /// Full render: optionally clear screen/scrollback, then write the full
    /// rendered frame.
    #[allow(clippy::too_many_arguments)]
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
                self.debug_frame_id,
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
                let _ = write!(buffer, "\x1b[{}A", image_reserved_rows - 1);
                buffer.push_str(line);
                let _ = write!(buffer, "\x1b[{}B", image_reserved_rows - 1);
                i += image_reserved_rows;
                continue;
            }
            buffer.push_str(line);
            i += 1;
        }
        buffer.push_str("\x1b[?2026l");
        if debug_log_enabled() {
            let _ = write_output_log(
                self.debug_frame_id,
                &format!("full-render-{clear}"),
                &buffer,
            );
        }
        write_render_output(output, &buffer, true)?;

        let cursor_row = new_lines.len().saturating_sub(1);
        let max_lines_rendered = if clear {
            new_lines.len()
        } else {
            self.max_lines_rendered.max(new_lines.len())
        };
        let buffer_length = height.max(new_lines.len());
        let previous_viewport_top = buffer_length.saturating_sub(height);
        let hardware_cursor_row =
            self.position_hardware_cursor(output, cursor_pos, new_lines.len(), cursor_row)?;
        self.previous_lines = new_lines.to_vec();
        self.previous_kitty_image_ids = collect_kitty_image_ids(&self.previous_lines);
        self.cursor_row = cursor_row;
        self.hardware_cursor_row = hardware_cursor_row;
        self.max_lines_rendered = max_lines_rendered;
        self.previous_viewport_top = previous_viewport_top;
        self.previous_width = width;
        self.previous_height = height_u16;
        Ok(())
    }

    /// Position the hardware cursor for IME candidate windows.
    fn position_hardware_cursor(
        &self,
        output: &mut dyn Write,
        cursor_pos: Option<CursorPos>,
        total_lines: usize,
        current_hardware_cursor_row: usize,
    ) -> std::io::Result<usize> {
        if cursor_pos.is_none() || total_lines == 0 {
            output.write_all(b"\x1b[?25l")?; // Hide cursor
            output.flush()?;
            return Ok(current_hardware_cursor_row);
        }
        let cursor_pos = cursor_pos.unwrap();

        // Clamp cursor position to valid range
        let target_row = cursor_pos.row.min(total_lines - 1);
        let target_col = cursor_pos.col;

        // Move cursor from current position to target
        let row_delta = target_row.cast_signed() - current_hardware_cursor_row.cast_signed();
        let mut buffer = String::new();
        if row_delta > 0 {
            let _ = write!(buffer, "\x1b[{row_delta}B"); // Move down
        } else if row_delta < 0 {
            let _ = write!(buffer, "\x1b[{}A", -row_delta); // Move up
        }
        // Move to absolute column (1-indexed)
        let _ = write!(buffer, "\x1b[{}G", target_col + 1);
        if self.show_hardware_cursor {
            buffer.push_str("\x1b[?25h");
        } else {
            buffer.push_str("\x1b[?25l");
        }

        if !buffer.is_empty() {
            output.write_all(buffer.as_bytes())?;
            output.flush()?;
        }

        Ok(target_row)
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

fn constrain_frame_lines(width: u16, lines: Vec<String>) -> ConstrainedFrameLines {
    let width = usize::from(width).max(1);
    let mut constrained = Vec::with_capacity(lines.len());
    let mut row_starts = Vec::with_capacity(lines.len());
    for mut line in lines {
        row_starts.push(constrained.len());
        if is_image_line(&line) {
            constrained.push(line);
            continue;
        }
        normalize_terminal_output(&mut line);
        if visible_width(&line) <= width {
            constrained.push(line);
            continue;
        }
        constrained.extend(
            wrap_width(&line, width)
                .into_iter()
                .map(|line| truncate_width(&line, width, "", false)),
        );
    }
    ConstrainedFrameLines {
        lines: constrained,
        row_starts,
    }
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

fn raw_changed_range(
    previous_lines: &[String],
    new_lines: &[String],
) -> (Option<usize>, Option<usize>) {
    let mut first_changed = None;
    let mut last_changed = None;
    let max_lines = new_lines.len().max(previous_lines.len());
    for index in 0..max_lines {
        let old_line = previous_lines.get(index).map_or("", String::as_str);
        let new_line = new_lines.get(index).map_or("", String::as_str);
        if old_line != new_line {
            first_changed.get_or_insert(index);
            last_changed = Some(index);
        }
    }
    (first_changed, last_changed)
}

fn push_deleted_tail_clears(buffer: &mut String, extra_lines: usize, has_content: bool) {
    let clear_start_offset = usize::from(has_content);
    push_deleted_tail_down(buffer, extra_lines, clear_start_offset);
    push_deleted_tail_clear_lines(buffer, extra_lines);
    push_deleted_tail_up(buffer, extra_lines, clear_start_offset);
}

fn push_deleted_tail_down(buffer: &mut String, extra_lines: usize, clear_start_offset: usize) {
    if extra_lines > 0 && clear_start_offset > 0 {
        let _ = write!(buffer, "\x1b[{clear_start_offset}B");
    }
}

fn push_deleted_tail_clear_lines(buffer: &mut String, extra_lines: usize) {
    for index in 0..extra_lines {
        buffer.push_str("\r\x1b[2K");
        if index < extra_lines - 1 {
            buffer.push_str("\x1b[1B");
        }
    }
}

fn push_deleted_tail_up(buffer: &mut String, extra_lines: usize, clear_start_offset: usize) {
    let move_back = extra_lines.saturating_sub(1) + clear_start_offset;
    if move_back > 0 {
        let _ = write!(buffer, "\x1b[{move_back}A");
    }
}

fn render_changed_lines(
    buffer: &mut String,
    new_lines: &[String],
    change_range: ChangeRange,
    viewport: ViewportState,
    dimensions: RenderDimensions,
) -> ChangedLinesRender {
    let render_end = change_range.render_end(new_lines.len());
    let mut index = change_range.first;
    while index <= render_end {
        if index > change_range.first {
            buffer.push_str("\r\n");
        }
        let line = &new_lines[index];
        let image_reserved_rows = reserved_render_rows(new_lines, index, render_end);
        if image_reserved_rows > 1 {
            if !image_block_fits(index, image_reserved_rows, viewport, dimensions.height) {
                return ChangedLinesRender::NeedsFullRender;
            }
            push_image_block(buffer, line, image_reserved_rows);
            index += image_reserved_rows;
            continue;
        }
        buffer.push_str("\x1b[2K");
        buffer.push_str(line);
        index += 1;
    }
    ChangedLinesRender::Rendered { render_end }
}

impl Drop for TuiRenderer {
    fn drop(&mut self) {
        self.leave();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingWriter {
        bytes_before_failure: usize,
    }

    impl FailingWriter {
        const fn after_bytes(bytes_before_failure: usize) -> Self {
            Self {
                bytes_before_failure,
            }
        }
    }

    impl Write for FailingWriter {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            if self.bytes_before_failure == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
            }
            let written = buffer.len().min(self.bytes_before_failure);
            self.bytes_before_failure -= written;
            Ok(written)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct FailOnWrite {
        successful_writes_remaining: usize,
    }

    impl FailOnWrite {
        const fn after_writes(successful_writes_remaining: usize) -> Self {
            Self {
                successful_writes_remaining,
            }
        }
    }

    impl Write for FailOnWrite {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            if self.successful_writes_remaining == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
            }
            self.successful_writes_remaining -= 1;
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct RecordingFailingWriter {
        bytes_before_failure: usize,
        failed_once: bool,
        output: Vec<u8>,
    }

    impl RecordingFailingWriter {
        const fn after_bytes(bytes_before_failure: usize) -> Self {
            Self {
                bytes_before_failure,
                failed_once: false,
                output: Vec::new(),
            }
        }
    }

    impl Write for RecordingFailingWriter {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            if self.bytes_before_failure == 0 && !self.failed_once {
                self.failed_once = true;
                return Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
            }
            if self.failed_once {
                self.output.extend_from_slice(buffer);
                return Ok(buffer.len());
            }
            let written = buffer.len().min(self.bytes_before_failure);
            self.output.extend_from_slice(&buffer[..written]);
            self.bytes_before_failure -= written;
            Ok(written)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    type RenderState = (
        Vec<String>,
        BTreeSet<u32>,
        usize,
        usize,
        usize,
        u16,
        u16,
        bool,
        usize,
        usize,
    );

    fn render_state(renderer: &TuiRenderer) -> RenderState {
        (
            renderer.previous_lines.clone(),
            renderer.previous_kitty_image_ids.clone(),
            renderer.viewport_top,
            renderer.previous_viewport_top,
            renderer.hardware_cursor_row,
            renderer.previous_width,
            renderer.previous_height,
            renderer.first_render,
            renderer.max_lines_rendered,
            renderer.cursor_row,
        )
    }

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
            force_full_redraw: false,
            first_render: line_count == 0,
            max_lines_rendered: line_count,
            cursor_row: line_count.saturating_sub(1),
            clear_on_shrink: false,
            show_hardware_cursor: true,
            capabilities: TerminalCapabilities::default(),
            debug_frame_id: 0,
            #[cfg(windows)]
            windows_input_mode: windows_input_mode::WindowsInputModeGuard::inactive(),
        }
    }

    #[test]
    fn render_write_failure_does_not_commit_frame_state() {
        let mut renderer = test_renderer(vec!["old".to_owned()]);
        let before = render_state(&renderer);
        let mut output = FailingWriter::after_bytes(2);

        let error = renderer
            .render_to_with_size(&mut output, 80, 24, vec!["new".to_owned()], None)
            .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        assert_eq!(render_state(&renderer), before);
        assert!(renderer.force_full_redraw);
    }

    #[test]
    fn cursor_write_failure_keeps_cached_frame_and_forces_full_retry() {
        let mut renderer = test_renderer(vec!["old".to_owned()]);
        let before = render_state(&renderer);
        let mut output = FailOnWrite::after_writes(1);

        let error = renderer
            .render_to_with_size(
                &mut output,
                80,
                24,
                vec!["new".to_owned()],
                Some(CursorPos { row: 0, col: 0 }),
            )
            .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        assert_eq!(render_state(&renderer), before);
        assert!(renderer.force_full_redraw);

        let mut retry_output = Vec::new();
        renderer
            .render_to_with_size(&mut retry_output, 80, 24, vec!["new".to_owned()], None)
            .unwrap();
        assert!(String::from_utf8_lossy(&retry_output).contains("\x1b[2J\x1b[H\x1b[3J"));
    }

    #[test]
    fn failed_synchronized_render_closes_terminal_output_mode() {
        let mut output = RecordingFailingWriter::after_bytes("\x1b[?2026h".len());

        let error = write_render_output(&mut output, "\x1b[?2026hframe", true).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        assert!(output.output.starts_with(b"\x1b[?2026h"));
        assert!(output.output.ends_with(SYNCHRONIZED_OUTPUT_END));
    }

    #[test]
    fn enter_output_leaves_mouse_wheel_to_terminal_scrollback() {
        let mut buf = Vec::new();
        write_enter_output(&mut buf, TerminalCapabilities::default()).unwrap();
        let output = String::from_utf8_lossy(&buf);
        assert!(
            !output.contains("\x1b[?1000h")
                && !output.contains("\x1b[?1002h")
                && !output.contains("\x1b[?1003h")
                && !output.contains("\x1b[?1006h")
                && !output.contains("\x1b[?1015h"),
            "enter output must not enable terminal mouse reporting: {output:?}"
        );
    }

    #[test]
    fn leave_output_does_not_touch_mouse_reporting() {
        let mut renderer = test_renderer(Vec::new());
        let mut buf = Vec::new();
        renderer.write_leave_output(&mut buf);
        write_leave_terminal_output(&mut buf, TerminalCapabilities::default()).unwrap();
        let output = String::from_utf8_lossy(&buf);
        assert!(
            !output.contains("\x1b[?1000l")
                && !output.contains("\x1b[?1002l")
                && !output.contains("\x1b[?1003l")
                && !output.contains("\x1b[?1006l")
                && !output.contains("\x1b[?1015l"),
            "leave output must not touch terminal mouse reporting: {output:?}"
        );
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
    fn renderer_wraps_oversized_lines_instead_of_crashing() {
        let mut renderer = test_renderer(Vec::new());
        let mut buf: Vec<u8> = Vec::new();
        renderer
            .render_to_with_size(
                &mut buf,
                20,
                10,
                vec![format!("\x1b[31m{}\x1b[0m", "abcdef".repeat(8))],
                None,
            )
            .expect("oversized line should render");

        assert!(
            renderer
                .previous_lines
                .iter()
                .all(|line| visible_width(line) <= 20),
            "renderer stored oversized lines: {:?}",
            renderer.previous_lines
        );
        let output = String::from_utf8_lossy(&buf);
        assert!(
            output.contains("abcdef"),
            "wrapped render should still include content: {output:?}"
        );
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
    fn explicit_cursor_row_tracks_lines_inserted_by_width_constraint() {
        let mut renderer = test_renderer(Vec::new());
        let mut buf = Vec::new();
        renderer
            .render_to_with_size(
                &mut buf,
                80,
                10,
                vec![
                    "x".repeat(81),
                    "│  > first prompt line                                                   │"
                        .to_owned(),
                    "│    second prompt line                                                  │"
                        .to_owned(),
                ],
                Some(CursorPos { row: 2, col: 8 }),
            )
            .unwrap();

        assert_eq!(
            renderer.hardware_cursor_row, 3,
            "cursor row should account for wrapped lines inserted before it"
        );
    }

    #[test]
    fn diff_render_skips_reserved_kitty_image_rows() {
        let mut renderer = test_renderer(vec![
            "\x1b_Gi=7,r=3;payload\x1b\\".to_owned(),
            String::new(),
            String::new(),
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
                    String::new(),
                    String::new(),
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
    fn hardware_cursor_visibility_defaults_to_visible_and_can_be_disabled() {
        assert!(hardware_cursor_enabled_from_env_value(None));
        assert!(hardware_cursor_enabled_from_env_value(Some("1")));
        assert!(hardware_cursor_enabled_from_env_value(Some("true")));
        assert!(!hardware_cursor_enabled_from_env_value(Some("0")));
        assert!(!hardware_cursor_enabled_from_env_value(Some("false")));
        assert!(!hardware_cursor_enabled_from_env_value(Some("off")));
    }

    #[test]
    fn cursor_position_shows_hardware_cursor_by_default() {
        let mut renderer = test_renderer(vec!["hello".to_owned()]);
        renderer.hardware_cursor_row = 0;
        let mut buf = Vec::new();

        renderer
            .position_hardware_cursor(&mut buf, Some(CursorPos { row: 0, col: 3 }), 1, 0)
            .unwrap();
        let output = String::from_utf8_lossy(&buf);

        assert!(output.contains("\x1b[4G"));
        assert!(output.contains("\x1b[?25h"));
        assert!(!output.contains("\x1b[?25l"));
    }

    #[test]
    fn cursor_position_hides_hardware_cursor_when_disabled() {
        let mut renderer = test_renderer(vec!["hello".to_owned()]);
        renderer.show_hardware_cursor = false;
        renderer.hardware_cursor_row = 0;
        let mut buf = Vec::new();

        renderer
            .position_hardware_cursor(&mut buf, Some(CursorPos { row: 0, col: 3 }), 1, 0)
            .unwrap();
        let output = String::from_utf8_lossy(&buf);

        assert!(output.contains("\x1b[4G"));
        assert!(output.contains("\x1b[?25l"));
        assert!(!output.contains("\x1b[?25h"));
    }

    #[test]
    fn leave_output_restores_hardware_cursor_visibility() {
        let mut renderer = test_renderer(vec!["hello".to_owned()]);
        renderer.show_hardware_cursor = false;
        let mut buf = Vec::new();

        renderer.write_leave_output(&mut buf);
        let output = String::from_utf8_lossy(&buf);

        assert!(output.contains("\x1b[?25h"));
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
