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

use crate::primitive::visible_width;
use crate::primitive::{truncate_width, wrap_width};

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

fn hardware_cursor_enabled_from_env_value(value: Option<&str>) -> bool {
    !matches!(
        value.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("0" | "false" | "off")
    )
}

const fn height_change_requires_clear(height_changed: bool, is_termux: bool) -> bool {
    height_changed && !is_termux
}

fn write_output_log(label: &str, buffer: &str) -> std::io::Result<()> {
    let mut file = create_debug_log_file(&format!("output-{label}"))?;
    file.write_all(buffer.as_bytes())?;
    file.flush()
}

fn create_debug_log_file(stem: &str) -> std::io::Result<fs::File> {
    let path = debug_log_path(stem)?;
    fs::File::create(path)
}

fn debug_log_path(stem: &str) -> std::io::Result<PathBuf> {
    let dir = PathBuf::from("/tmp/neo-tui-debug");
    fs::create_dir_all(&dir)?;
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    Ok(dir.join(format!("{stem}-{timestamp}.log")))
}

fn write_rendered_lines(
    file: &mut fs::File,
    heading: &str,
    lines: &[String],
) -> std::io::Result<()> {
    writeln!(file, "{heading}")?;
    for (index, line) in lines.iter().enumerate() {
        let width = visible_width(line);
        writeln!(file, "[{index}] (w={width}) {line}")?;
    }
    Ok(())
}

fn write_debug_log(
    label: &str,
    width: u16,
    height: usize,
    new_lines: &[String],
    previous_lines: &[String],
    extra: Option<&str>,
) -> std::io::Result<()> {
    let mut file = create_debug_log_file(label)?;
    write_debug_log_header(&mut file, label, width, height, new_lines, previous_lines)?;
    write_optional_debug_text(&mut file, extra)?;
    write_debug_log_lines(&mut file, new_lines, previous_lines)?;
    file.flush()
}

fn write_debug_log_lines(
    file: &mut fs::File,
    new_lines: &[String],
    previous_lines: &[String],
) -> std::io::Result<()> {
    write_rendered_lines(file, "=== new_lines ===", new_lines)?;
    write_rendered_lines(file, "=== previous_lines ===", previous_lines)
}

fn write_debug_log_header(
    file: &mut fs::File,
    label: &str,
    width: u16,
    height: usize,
    new_lines: &[String],
    previous_lines: &[String],
) -> std::io::Result<()> {
    writeln!(file, "[{label}] width={width} height={height}")?;
    writeln!(
        file,
        "new_lines.len()={} previous_lines.len()={}",
        new_lines.len(),
        previous_lines.len()
    )
}

fn write_optional_debug_text(file: &mut fs::File, extra: Option<&str>) -> std::io::Result<()> {
    if let Some(text) = extra {
        writeln!(file, "{text}")?;
    }
    Ok(())
}

fn write_width_crash_log(
    width: u16,
    new_lines: &[String],
    offender_idx: usize,
) -> std::io::Result<PathBuf> {
    let path = debug_log_path("width-crash")?;
    let mut file = fs::File::create(&path)?;
    write_width_crash_body(&mut file, width, new_lines, offender_idx)?;
    Ok(path)
}

fn write_width_crash_body(
    file: &mut fs::File,
    width: u16,
    new_lines: &[String],
    offender_idx: usize,
) -> std::io::Result<()> {
    write_width_crash_header(file, width, new_lines, offender_idx)?;
    write_rendered_lines(file, "=== All rendered lines ===", new_lines)?;
    file.flush()
}

fn write_width_crash_header(
    file: &mut fs::File,
    width: u16,
    new_lines: &[String],
    offender_idx: usize,
) -> std::io::Result<()> {
    writeln!(file, "Terminal width: {width}")?;
    writeln!(file, "Offending line index: {offender_idx}")?;
    writeln!(
        file,
        "Offending line visible width: {}",
        visible_width(&new_lines[offender_idx])
    )
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

#[derive(Clone, Copy)]
struct RenderDimensions {
    width: u16,
    height: usize,
    height_u16: u16,
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
struct ViewportState {
    previous_top: usize,
    top: usize,
    hardware_cursor_row: usize,
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
struct ChangeRange {
    first: usize,
    last: usize,
    append_start: bool,
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

struct DiffRender {
    buffer: String,
    change_range: ChangeRange,
    viewport: ViewportState,
    move_target_row: usize,
    render_end: usize,
    final_cursor_row: usize,
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
            show_hardware_cursor: hardware_cursor_enabled_from_env_value(
                env::var("NEO_HARDWARE_CURSOR").ok().as_deref(),
            ),
        })
    }

    /// Restore terminal state.
    pub fn leave(&mut self) {
        let mut output = stdout();
        self.write_leave_output(&mut output);
        let _ = output.flush();

        let _ = execute!(output, PopKeyboardEnhancementFlags, DisableBracketedPaste,);
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
        ) {
            return Ok(());
        }

        // Content shrank below the historical high-water mark. We only force a
        // viewport redraw when explicitly opted in (e.g. a closed overlay).
        if self.clear_on_shrink && new_lines.len() < self.max_lines_rendered {
            self.full_render_with_dimensions(output, true, new_lines_ref, dimensions, cursor_pos);
            return Ok(());
        }

        let change_range = self.changed_range(&new_lines);

        // No changes - but still need to update hardware cursor position if it moved
        let Some(change_range) = change_range else {
            self.position_hardware_cursor(output, cursor_pos, new_lines.len());
            self.previous_viewport_top = viewport.previous_top;
            self.previous_height = height_u16;
            self.previous_lines = new_lines;
            return Ok(());
        };

        // All changes are in deleted lines (nothing to render, just clear)
        if change_range.first >= new_lines.len() {
            if !self.render_deleted_tail(output, dimensions, new_lines_ref, change_range, viewport)
            {
                self.full_render_with_dimensions(
                    output,
                    true,
                    new_lines_ref,
                    dimensions,
                    cursor_pos,
                );
                return Ok(());
            }
            self.finish_deleted_tail(
                output,
                new_lines,
                cursor_pos,
                dimensions,
                viewport.previous_top,
            );
            return Ok(());
        }

        // Differential rendering can only touch what was actually visible.
        // If the first changed line is above the previous viewport, use a full
        // clear render.
        if change_range.first < viewport.previous_top {
            self.full_render_with_dimensions(output, true, new_lines_ref, dimensions, cursor_pos);
            return Ok(());
        }

        let Some(diff_render) =
            self.build_diff_render(&new_lines, change_range, viewport, dimensions)
        else {
            self.full_render_with_dimensions(output, true, new_lines_ref, dimensions, cursor_pos);
            return Ok(());
        };
        self.finish_diff_render(output, new_lines, cursor_pos, dimensions, &diff_render);
        Ok(())
    }

    fn try_early_full_render(
        &mut self,
        output: &mut dyn Write,
        new_lines: &[String],
        dimensions: RenderDimensions,
        cursor_pos: Option<CursorPos>,
        changes: RenderChangeFlags,
    ) -> bool {
        let Some(clear) = self.early_full_render_clear(changes) else {
            return false;
        };
        self.full_render_with_dimensions(output, clear, new_lines, dimensions, cursor_pos);
        self.first_render = false;
        true
    }

    fn full_render_with_dimensions(
        &mut self,
        output: &mut dyn Write,
        clear: bool,
        new_lines: &[String],
        dimensions: RenderDimensions,
        cursor_pos: Option<CursorPos>,
    ) {
        self.full_render(
            output,
            clear,
            new_lines,
            dimensions.height,
            dimensions.height_u16,
            dimensions.width,
            cursor_pos,
        );
    }

    fn early_full_render_clear(&self, changes: RenderChangeFlags) -> Option<bool> {
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

    fn log_render_start(&self, dimensions: RenderDimensions, new_lines: &[String]) {
        if !debug_log_enabled() {
            return;
        }

        let _ = write_debug_log(
            "render-start",
            dimensions.width,
            dimensions.height,
            new_lines,
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

    fn log_diff_render(
        &self,
        dimensions: RenderDimensions,
        new_lines: &[String],
        diff_render: &DiffRender,
    ) {
        if !debug_log_enabled() {
            return;
        }

        let _ = write_output_log("diff-render", &diff_render.buffer);
        let _ = write_debug_log(
            "diff-render",
            dimensions.width,
            dimensions.height,
            new_lines,
            &self.previous_lines,
            Some(&format!(
                "first_changed={} last_changed={} append_start={} prev_viewport_top={} viewport_top={} hardware_cursor_row={} move_target_row={move_target_row} render_end={render_end} final_cursor_row={final_cursor_row}",
                diff_render.change_range.first,
                diff_render.change_range.last,
                diff_render.change_range.append_start,
                diff_render.viewport.previous_top,
                diff_render.viewport.top,
                diff_render.viewport.hardware_cursor_row,
                move_target_row = diff_render.move_target_row,
                render_end = diff_render.render_end,
                final_cursor_row = diff_render.final_cursor_row
            )),
        );
    }

    fn build_diff_render(
        &self,
        new_lines: &[String],
        change_range: ChangeRange,
        mut viewport: ViewportState,
        dimensions: RenderDimensions,
    ) -> Option<DiffRender> {
        let mut buffer = String::with_capacity(4096);
        buffer.push_str("\x1b[?2026h");
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
        buffer.push_str("\x1b[?2026l");

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
    ) {
        self.log_diff_render(dimensions, &new_lines, diff_render);
        let _ = output.write_all(diff_render.buffer.as_bytes());
        let _ = output.flush();

        self.cursor_row = new_lines.len().saturating_sub(1);
        self.hardware_cursor_row = diff_render.final_cursor_row;
        self.max_lines_rendered = self.max_lines_rendered.max(new_lines.len());
        self.previous_viewport_top = diff_render.viewport.previous_top.max(
            diff_render
                .final_cursor_row
                .saturating_sub(dimensions.height - 1),
        );
        self.position_hardware_cursor(output, cursor_pos, new_lines.len());

        self.previous_lines = new_lines;
        self.previous_kitty_image_ids = collect_kitty_image_ids(&self.previous_lines);
        self.previous_width = dimensions.width;
        self.previous_height = dimensions.height_u16;
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
    ) -> bool {
        if self.previous_lines.len() <= new_lines.len() {
            return true;
        }

        let target_row = new_lines.len().saturating_sub(1);
        let extra_lines = self.previous_lines.len() - new_lines.len();
        if target_row < viewport.previous_top || extra_lines > dimensions.height {
            return false;
        }

        let mut buffer = String::new();
        buffer.push_str("\x1b[?2026h");
        buffer.push_str(&self.delete_changed_kitty_images(change_range.first, change_range.last));
        push_vertical_move(&mut buffer, viewport.line_diff(target_row));
        buffer.push('\r');
        push_deleted_tail_clears(&mut buffer, extra_lines, !new_lines.is_empty());
        buffer.push_str("\x1b[?2026l");

        if debug_log_enabled() {
            let _ = write_output_log("deleted-lines", &buffer);
        }
        let _ = output.write_all(buffer.as_bytes());
        let _ = output.flush();
        self.cursor_row = target_row;
        self.hardware_cursor_row = target_row;
        true
    }

    fn finish_deleted_tail(
        &mut self,
        output: &mut dyn Write,
        new_lines: Vec<String>,
        cursor_pos: Option<CursorPos>,
        dimensions: RenderDimensions,
        previous_viewport_top: usize,
    ) {
        self.position_hardware_cursor(output, cursor_pos, new_lines.len());
        self.previous_lines = new_lines;
        self.previous_kitty_image_ids = collect_kitty_image_ids(&self.previous_lines);
        self.previous_width = dimensions.width;
        self.previous_height = dimensions.height_u16;
        self.previous_viewport_top = previous_viewport_top;
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
    ) {
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
        self.position_hardware_cursor(output, cursor_pos, new_lines.len());
        self.previous_lines = new_lines.to_vec();
        self.previous_kitty_image_ids = collect_kitty_image_ids(&self.previous_lines);
        self.previous_width = width;
        self.previous_height = height_u16;
    }

    /// Position the hardware cursor for IME candidate windows.
    fn position_hardware_cursor(
        &mut self,
        output: &mut dyn Write,
        cursor_pos: Option<CursorPos>,
        total_lines: usize,
    ) {
        if cursor_pos.is_none() || total_lines == 0 {
            let _ = write!(output, "\x1b[?25l"); // Hide cursor
            let _ = output.flush();
            return;
        }
        let cursor_pos = cursor_pos.unwrap();

        // Clamp cursor position to valid range
        let target_row = cursor_pos.row.min(total_lines - 1);
        let target_col = cursor_pos.col;

        // Move cursor from current position to target
        let row_delta = target_row.cast_signed() - self.hardware_cursor_row.cast_signed();
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
            let _ = output.write_all(buffer.as_bytes());
            let _ = output.flush();
        }

        self.hardware_cursor_row = target_row;
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
        let _ = write!(buffer, "\x1b_Ga=d,d=I,i={id},q=2\x1b\\");
    }
    buffer
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

fn reserved_render_rows(lines: &[String], index: usize, render_end: usize) -> usize {
    if is_image_line(&lines[index]) {
        get_kitty_image_reserved_rows(lines, index, render_end)
    } else {
        1
    }
}

fn image_block_fits(
    index: usize,
    image_reserved_rows: usize,
    viewport: ViewportState,
    height: usize,
) -> bool {
    let image_start_screen_row = index.cast_signed() - viewport.top.cast_signed();
    image_start_screen_row >= 0
        && image_start_screen_row.cast_unsigned() + image_reserved_rows <= height
}

fn push_image_block(buffer: &mut String, line: &str, image_reserved_rows: usize) {
    buffer.push_str("\x1b[2K");
    for _ in 1..image_reserved_rows {
        buffer.push_str("\r\n\x1b[2K");
    }
    let _ = write!(buffer, "\x1b[{}A", image_reserved_rows - 1);
    buffer.push_str(line);
    let _ = write!(buffer, "\x1b[{}B", image_reserved_rows - 1);
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
            show_hardware_cursor: true,
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

        renderer.position_hardware_cursor(&mut buf, Some(CursorPos { row: 0, col: 3 }), 1);
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

        renderer.position_hardware_cursor(&mut buf, Some(CursorPos { row: 0, col: 3 }), 1);
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
