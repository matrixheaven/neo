use std::fmt::Write as _;
use std::io::Write;
use std::time::Instant;

use crate::terminal_capabilities::TerminalCapabilities;
use crate::transcript::FinalizedBlock;

use super::LiveRenderer;
use super::terminal_modes::{
    TerminalModeGuard, write_disable_mouse_capture, write_enable_mouse_capture,
    write_enter_review_output, write_leave_review_output,
};
use super::types::CursorPos;

const SYNCHRONIZED_OUTPUT_START: &str = "\x1b[?2026h";
const SYNCHRONIZED_OUTPUT_END: &[u8] = b"\x1b[?2026l";
const RESET_SCROLL_REGION: &[u8] = b"\x1b[r";

#[derive(Debug, Clone)]
pub struct TerminalFrame {
    pub history: Vec<FinalizedBlock>,
    pub live: Vec<String>,
    pub cursor: Option<CursorPos>,
    pub review_surface: bool,
    pub mouse_capture: bool,
    pub next_animation_deadline: Option<Instant>,
}

impl TerminalFrame {
    #[must_use]
    pub const fn new(
        history: Vec<FinalizedBlock>,
        live: Vec<String>,
        cursor: Option<CursorPos>,
    ) -> Self {
        Self {
            history,
            live,
            cursor,
            review_surface: false,
            mouse_capture: false,
            next_animation_deadline: None,
        }
    }

    #[must_use]
    pub fn with_surface(
        history: Vec<FinalizedBlock>,
        live: Vec<String>,
        cursor: Option<CursorPos>,
        review_surface: bool,
        next_animation_deadline: Option<Instant>,
    ) -> Self {
        Self {
            history: if review_surface { Vec::new() } else { history },
            live,
            cursor,
            review_surface,
            mouse_capture: false,
            next_animation_deadline,
        }
    }

    #[must_use]
    pub const fn with_mouse_capture(mut self, mouse_capture: bool) -> Self {
        self.mouse_capture = mouse_capture;
        self
    }

    #[must_use]
    pub const fn with_animation_deadline(
        history: Vec<FinalizedBlock>,
        live: Vec<String>,
        cursor: Option<CursorPos>,
        next_animation_deadline: Option<Instant>,
    ) -> Self {
        Self {
            history,
            live,
            cursor,
            review_surface: false,
            mouse_capture: false,
            next_animation_deadline,
        }
    }
}

/// Absolute normal-screen geometry owned solely by [`InlineTerminal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NormalScreenGeometry {
    width: u16,
    height: u16,
    generation: u64,
    /// Zero-based top row of the mutable live viewport.
    live_top: u16,
    /// Absolute hardware cursor (column, row), zero-based.
    cursor_col: u16,
    cursor_row: u16,
}

#[derive(Debug)]
pub struct InlineTerminal {
    synchronized_output: bool,
    geometry: NormalScreenGeometry,
    live: LiveRenderer,
    saved_normal_live: Option<LiveRenderer>,
    saved_normal_geometry: Option<NormalScreenGeometry>,
    modes: Option<TerminalModeGuard>,
    review_surface: bool,
    mouse_capture: bool,
}

impl InlineTerminal {
    #[must_use]
    pub fn new(
        width: u16,
        height: u16,
        capabilities: TerminalCapabilities,
        cursor_col: u16,
        cursor_row: u16,
    ) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let cursor_col = cursor_col.min(width.saturating_sub(1));
        let cursor_row = cursor_row.min(height.saturating_sub(1));
        Self {
            synchronized_output: capabilities.ansi.synchronized_output,
            geometry: NormalScreenGeometry {
                width,
                height,
                generation: 0,
                live_top: cursor_row,
                cursor_col,
                cursor_row,
            },
            live: LiveRenderer::new(width, height),
            saved_normal_live: None,
            saved_normal_geometry: None,
            modes: None,
            review_surface: false,
            mouse_capture: false,
        }
    }

    pub fn enter(
        width: u16,
        height: u16,
        capabilities: TerminalCapabilities,
        cursor_col: u16,
        cursor_row: u16,
    ) -> std::io::Result<Self> {
        let modes = TerminalModeGuard::enter(capabilities)?;
        let mut terminal = Self::new(width, height, capabilities, cursor_col, cursor_row);
        terminal.modes = Some(modes);
        Ok(terminal)
    }

    /// Test constructor. Starts at absolute cursor `(0, 0)`.
    /// Shell-seeded harnesses should use [`Self::for_test_with_cursor`].
    #[must_use]
    pub fn for_test(width: u16, height: u16) -> Self {
        Self::for_test_with_cursor(width, height, 0, 0)
    }

    /// Test constructor with an explicit zero-based absolute cursor.
    #[must_use]
    pub fn for_test_with_cursor(width: u16, height: u16, cursor_col: u16, cursor_row: u16) -> Self {
        Self::new(
            width,
            height,
            TerminalCapabilities::default(),
            cursor_col,
            cursor_row,
        )
    }

    pub fn render_to(
        &mut self,
        output: &mut dyn Write,
        frame: &TerminalFrame,
    ) -> std::io::Result<()> {
        let entering_review = frame.review_surface && !self.review_surface;
        let leaving_review = !frame.review_surface && self.review_surface;
        let mut transaction = Vec::new();
        // Keep the primary live anchor while the alternate review screen owns the renderer.
        let saved_normal_live = entering_review.then(|| self.live.clone());
        let saved_normal_geometry = entering_review.then_some(self.geometry);

        append_surface_transition(
            &mut transaction,
            &mut self.modes,
            self.review_surface,
            self.mouse_capture,
            frame.review_surface,
            frame.mouse_capture,
        )?;

        let history_lines = if frame.review_surface {
            Vec::new()
        } else {
            flatten_history(&frame.history)
        };

        let mut next_live = if leaving_review {
            self.saved_normal_live
                .clone()
                .unwrap_or_else(|| self.live.clone())
        } else {
            self.live.clone()
        };
        let mut next_geometry = if leaving_review {
            self.saved_normal_geometry.unwrap_or(self.geometry)
        } else {
            self.geometry
        };
        // Origin that currently owns mutable rows on the physical screen.
        let previous_live_top = next_geometry.live_top;
        let previous_live_rows = next_live.previous_line_count();

        if entering_review {
            next_live.reset();
            // Alternate screen starts at the top-left of a fresh buffer.
            next_geometry.live_top = 0;
            next_geometry.cursor_col = 0;
            next_geometry.cursor_row = 0;
        }

        // Target live top for the incoming frame (before history insertion moves
        // content). Used to detect origin moves that require erasing the old
        // absolute viewport.
        let projected_live_top =
            projected_live_top(&next_geometry, frame.live.len()).unwrap_or(next_geometry.live_top);

        // Erase previously drawn live-owned rows at their absolute origin before
        // any scroll that could carry them into native scrollback.
        if previous_live_rows > 0
            && (!history_lines.is_empty()
                || leaving_review
                || projected_live_top != previous_live_top)
        {
            transaction.extend_from_slice(next_live.clear_at_origin(previous_live_top).as_bytes());
        }

        if !history_lines.is_empty() {
            append_protected_history(&mut transaction, &mut next_geometry, &history_lines);
        }

        // Reconcile live height: shrink clears released rows; grow makes room above.
        if let Err(error) =
            reconcile_live_viewport(&mut transaction, &mut next_geometry, frame.live.len())
        {
            let _ = output.write_all(RESET_SCROLL_REGION);
            let _ = output.flush();
            return Err(error);
        }

        let mut live_bytes = Vec::new();
        if let Err(error) = next_live.render_to(
            &mut live_bytes,
            next_geometry.live_top,
            frame.live.clone(),
            frame.cursor,
        ) {
            let _ = output.write_all(RESET_SCROLL_REGION);
            let _ = output.flush();
            return Err(error);
        }
        transaction.extend_from_slice(&live_bytes);

        // Track absolute hardware cursor after the live draw.
        let live_len = u16::try_from(frame.live.len()).unwrap_or(u16::MAX);
        if let Some(cursor) = frame.cursor {
            next_geometry.cursor_row = next_geometry
                .live_top
                .saturating_add(u16::try_from(cursor.row).unwrap_or(u16::MAX));
            next_geometry.cursor_col = u16::try_from(cursor.col).unwrap_or(u16::MAX);
        } else if live_len > 0 {
            next_geometry.cursor_row = next_geometry.live_top.saturating_add(live_len - 1);
            next_geometry.cursor_col = 0;
        }

        if transaction.is_empty() {
            return Ok(());
        }

        let transaction = if self.synchronized_output {
            format!(
                "{SYNCHRONIZED_OUTPUT_START}{}{}",
                String::from_utf8_lossy(&transaction),
                String::from_utf8_lossy(SYNCHRONIZED_OUTPUT_END)
            )
            .into_bytes()
        } else {
            transaction
        };
        if let Err(error) = output.write_all(&transaction).and_then(|()| output.flush()) {
            let _ = output.write_all(RESET_SCROLL_REGION);
            if self.synchronized_output {
                let _ = output.write_all(SYNCHRONIZED_OUTPUT_END);
            }
            let _ = output.flush();
            recover_surface_transition(
                &mut self.modes,
                output,
                self.review_surface,
                self.mouse_capture,
                frame.review_surface,
                frame.mouse_capture,
            );
            return Err(error);
        }

        // Commit geometry and renderer cache only after a successful flush.
        self.live = next_live;
        self.geometry = next_geometry;
        if entering_review {
            self.review_surface = true;
            self.saved_normal_live = saved_normal_live;
            self.saved_normal_geometry = saved_normal_geometry;
            if let Some(modes) = &mut self.modes {
                modes.set_review_active(true);
            }
        } else if leaving_review {
            self.review_surface = false;
            self.saved_normal_live = None;
            self.saved_normal_geometry = None;
            if let Some(modes) = &mut self.modes {
                modes.set_review_active(false);
            }
        }
        self.mouse_capture = frame.mouse_capture;
        if let Some(modes) = &mut self.modes {
            modes.set_mouse_capture_active(frame.mouse_capture);
        }
        Ok(())
    }

    /// Resize with a cursor observation tagged by size generation.
    ///
    /// Stale or out-of-bounds observations fail closed with `InvalidData`.
    /// Same generation and size is a no-op so steady-state frames do not
    /// recompute the absolute live viewport from a stale cursor snapshot.
    pub fn resize(
        &mut self,
        width: u16,
        height: u16,
        cursor_col: u16,
        cursor_row: u16,
        generation: u64,
    ) -> std::io::Result<()> {
        let width = width.max(1);
        let height = height.max(1);
        if generation < self.geometry.generation {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "stale geometry generation {generation} < {}",
                    self.geometry.generation
                ),
            ));
        }
        if generation == self.geometry.generation {
            if width == self.geometry.width && height == self.geometry.height {
                return Ok(());
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "geometry generation {generation} already observed for {}x{}, not {width}x{height}",
                    self.geometry.width, self.geometry.height
                ),
            ));
        }
        if cursor_col >= width || cursor_row >= height {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("cursor ({cursor_col},{cursor_row}) outside screen {width}x{height}"),
            ));
        }

        let live_height = u16::try_from(self.live.previous_line_count().min(usize::from(height)))
            .unwrap_or(u16::MAX);
        let cursor_offset = self
            .geometry
            .cursor_row
            .saturating_sub(self.geometry.live_top);
        // Keep the observed hardware cursor on the same logical live row. The
        // composer cursor is not necessarily on the final live row because the
        // footer and completion rows may follow it.
        let live_top = if live_height == 0 {
            cursor_row.min(height.saturating_sub(1))
        } else {
            let max_top = height.saturating_sub(live_height);
            cursor_row
                .saturating_sub(cursor_offset.min(live_height.saturating_sub(1)))
                .min(max_top)
        };

        self.geometry = NormalScreenGeometry {
            width,
            height,
            generation,
            live_top,
            cursor_col,
            cursor_row,
        };
        self.live.resize(width, height);
        if let Some(saved) = &mut self.saved_normal_live {
            saved.resize(width, height);
        }
        if let Some(saved_geo) = &mut self.saved_normal_geometry {
            saved_geo.width = width;
            saved_geo.height = height;
            saved_geo.generation = generation;
        }
        Ok(())
    }

    /// Apply a resize without generation checks (test helper).
    pub fn resize_for_test(&mut self, width: u16, height: u16) {
        let generation = self.geometry.generation.saturating_add(1);
        let cursor_row = self.geometry.cursor_row.min(height.saturating_sub(1));
        let cursor_col = self.geometry.cursor_col.min(width.saturating_sub(1));
        let _ = self.resize(width, height, cursor_col, cursor_row, generation);
    }

    pub fn suspend_prepare(&mut self, output: &mut dyn Write) -> std::io::Result<()> {
        let was_review = self.review_surface;
        if self.review_surface {
            let mut transition = Vec::new();
            append_surface_transition(
                &mut transition,
                &mut self.modes,
                self.review_surface,
                self.mouse_capture,
                false,
                false,
            )?;
            if let Some(saved) = self.saved_normal_live.as_ref() {
                let mut saved = saved.clone();
                let live_top = self
                    .saved_normal_geometry
                    .map_or(self.geometry.live_top, |geo| geo.live_top);
                transition.extend_from_slice(saved.clear_at_origin(live_top).as_bytes());
            }
            transition.extend_from_slice(RESET_SCROLL_REGION);
            if let Err(error) = output.write_all(&transition).and_then(|()| output.flush()) {
                let _ = output.write_all(RESET_SCROLL_REGION);
                recover_surface_transition(
                    &mut self.modes,
                    output,
                    self.review_surface,
                    self.mouse_capture,
                    false,
                    false,
                );
                return Err(error);
            }
            self.saved_normal_live = None;
            self.saved_normal_geometry = None;
            if let Some(modes) = &mut self.modes {
                modes.set_review_active(false);
                modes.set_mouse_capture_active(false);
            }
            self.review_surface = false;
            self.mouse_capture = false;
            self.live.reset();
        }
        let result = if was_review {
            Ok(())
        } else {
            self.clear_live_to(output, false)
        };
        if let Some(modes) = &mut self.modes {
            modes.leave();
        }
        result
    }

    pub fn resume(
        &mut self,
        width: u16,
        height: u16,
        cursor_col: u16,
        cursor_row: u16,
        generation: u64,
    ) -> std::io::Result<()> {
        if let Some(modes) = &mut self.modes {
            modes.resume()?;
        }
        self.live.reset();
        self.resize(width, height, cursor_col, cursor_row, generation)?;
        Ok(())
    }

    /// Resume without a fresh observation (test helper defaults generation bump).
    pub fn resume_for_test(&mut self) -> std::io::Result<()> {
        let generation = self.geometry.generation.saturating_add(1);
        self.resume(
            self.geometry.width,
            self.geometry.height,
            self.geometry.cursor_col,
            self.geometry.cursor_row,
            generation,
        )
    }

    pub fn leave(&mut self, output: &mut dyn Write) -> std::io::Result<()> {
        if self.review_surface {
            let mut transition = Vec::new();
            append_surface_transition(
                &mut transition,
                &mut self.modes,
                self.review_surface,
                self.mouse_capture,
                false,
                false,
            )?;
            if let Some(saved) = self.saved_normal_live.as_ref() {
                let mut saved = saved.clone();
                let live_top = self
                    .saved_normal_geometry
                    .map_or(self.geometry.live_top, |geo| geo.live_top);
                transition.extend_from_slice(saved.clear_at_origin(live_top).as_bytes());
            }
            transition.extend_from_slice(RESET_SCROLL_REGION);
            if let Err(error) = output.write_all(&transition).and_then(|()| output.flush()) {
                let _ = output.write_all(RESET_SCROLL_REGION);
                recover_surface_transition(
                    &mut self.modes,
                    output,
                    self.review_surface,
                    self.mouse_capture,
                    false,
                    false,
                );
                return Err(error);
            }
            self.saved_normal_live = None;
            self.saved_normal_geometry = None;
            if let Some(modes) = &mut self.modes {
                modes.set_review_active(false);
                modes.set_mouse_capture_active(false);
            }
            self.review_surface = false;
            self.mouse_capture = false;
            self.live.reset();
            if let Some(modes) = &mut self.modes {
                modes.leave();
            } else {
                output.write_all(b"\x1b[?25h")?;
                output.flush()?;
            }
            return Ok(());
        }
        let show_cursor = self.modes.is_none();
        let result = self.clear_live_to(output, show_cursor);
        if let Some(modes) = &mut self.modes {
            modes.leave();
        }
        result
    }

    fn clear_live_to(&mut self, output: &mut dyn Write, show_cursor: bool) -> std::io::Result<()> {
        let previous_live_rows = u16::try_from(self.live.previous_line_count()).unwrap_or(u16::MAX);
        let mut next_live = self.live.clone();
        let mut transaction = next_live.clear_at_origin(self.geometry.live_top);
        // After clearing the live viewport, park the cursor on the first cleared
        // row (immediately below finalized history). If the live zone reached
        // the bottom of the screen, step one row past the last history line by
        // emitting a final CRLF so the shell prompt lands below Neo output.
        let mut cursor_row = self
            .geometry
            .live_top
            .min(self.geometry.height.saturating_sub(1));
        if previous_live_rows > 0
            && self.geometry.live_top.saturating_add(previous_live_rows) >= self.geometry.height
        {
            // Live occupied the bottom of the screen; scroll one line so the
            // cursor rests below the last finalized Neo row.
            let _ = write!(
                transaction,
                "\x1b[{};1H\r\n",
                u32::from(self.geometry.height)
            );
            cursor_row = self.geometry.height.saturating_sub(1);
        }
        // Reset margins first — some terminals (and the vt100 harness) home the
        // cursor when applying CSI r — then restore the absolute leave cursor.
        transaction.push_str(&String::from_utf8_lossy(RESET_SCROLL_REGION));
        let ansi_row = u32::from(cursor_row).saturating_add(1);
        let _ = write!(transaction, "\x1b[{ansi_row};1H");
        if show_cursor {
            transaction.push_str("\x1b[?25h");
        }
        output.write_all(transaction.as_bytes())?;
        output.flush()?;
        self.live = next_live;
        self.geometry.cursor_row = cursor_row;
        self.geometry.cursor_col = 0;
        self.geometry.live_top = cursor_row;
        Ok(())
    }
}

/// Promote finalized history at the cleared live origin.
///
/// Prior live rows are blank, so full-screen scrolling preserves native
/// scrollback without carrying mutable chrome into it.
fn append_protected_history(
    transaction: &mut Vec<u8>,
    geometry: &mut NormalScreenGeometry,
    lines: &[String],
) {
    if lines.is_empty() {
        return;
    }

    let mut body = String::new();
    let region_bottom = geometry.height.max(1);
    let start_row = geometry.live_top.min(geometry.height.saturating_sub(1));
    let ansi_start_row = u32::from(start_row).saturating_add(1);
    let _ = write!(body, "\x1b[1;{region_bottom}r");
    let _ = write!(body, "\x1b[{ansi_start_row};1H");
    let mut cursor_row = start_row;
    for line in lines {
        body.push_str("\r\x1b[2K");
        body.push_str(line);
        body.push_str("\r\n");
        cursor_row = cursor_row
            .saturating_add(1)
            .min(geometry.height.saturating_sub(1));
    }
    body.push_str(&String::from_utf8_lossy(RESET_SCROLL_REGION));
    transaction.extend_from_slice(body.as_bytes());
    geometry.live_top = cursor_row;
    geometry.cursor_row = cursor_row;
    geometry.cursor_col = 0;
}

fn projected_live_top(geometry: &NormalScreenGeometry, live_len: usize) -> std::io::Result<u16> {
    let live_len = u16::try_from(live_len).unwrap_or(u16::MAX);
    if live_len > geometry.height {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "live frame has {live_len} rows but terminal height is {}",
                geometry.height
            ),
        ));
    }
    if live_len == 0 {
        return Ok(geometry.live_top.min(geometry.height.saturating_sub(1)));
    }
    let max_top = geometry.height.saturating_sub(live_len);
    Ok(geometry.live_top.min(max_top))
}

/// Grow or shrink the live viewport without scrolling populated live rows.
///
/// The caller clears populated live rows before any origin move, so only the
/// minimum required full-screen scroll is needed here.
fn reconcile_live_viewport(
    transaction: &mut Vec<u8>,
    geometry: &mut NormalScreenGeometry,
    live_len: usize,
) -> std::io::Result<()> {
    let live_len = u16::try_from(live_len).unwrap_or(u16::MAX);
    if live_len > geometry.height {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "live frame has {live_len} rows but terminal height is {}",
                geometry.height
            ),
        ));
    }
    if live_len == 0 {
        return Ok(());
    }

    let max_top = geometry.height.saturating_sub(live_len);
    if geometry.live_top <= max_top {
        return Ok(());
    }

    // Need more room above the bottom of the screen.
    let need = geometry.live_top - max_top;
    let bottom = geometry.height;
    let mut body = String::from_utf8_lossy(RESET_SCROLL_REGION).into_owned();
    let _ = write!(body, "\x1b[{bottom};1H");
    for _ in 0..need {
        body.push_str("\r\n");
    }
    transaction.extend_from_slice(body.as_bytes());
    geometry.live_top = max_top;
    Ok(())
}

fn append_surface_transition(
    transaction: &mut Vec<u8>,
    modes: &mut Option<TerminalModeGuard>,
    from_review: bool,
    from_mouse_capture: bool,
    to_review: bool,
    to_mouse_capture: bool,
) -> std::io::Result<()> {
    if from_review && !to_review && from_mouse_capture {
        write_disable_mouse_capture(transaction)?;
    }
    if from_review != to_review {
        if let Some(modes) = modes {
            if to_review {
                modes.enter_review(transaction)?;
            } else {
                modes.leave_review(transaction)?;
            }
        } else if to_review {
            write_enter_review_output(transaction)?;
        } else {
            write_leave_review_output(transaction)?;
        }
    }
    if from_mouse_capture != to_mouse_capture && !(from_review && !to_review) {
        if to_mouse_capture {
            write_enable_mouse_capture(transaction)?;
        } else {
            write_disable_mouse_capture(transaction)?;
        }
    }
    Ok(())
}

fn recover_surface_transition(
    modes: &mut Option<TerminalModeGuard>,
    output: &mut dyn Write,
    from_review: bool,
    from_mouse_capture: bool,
    to_review: bool,
    to_mouse_capture: bool,
) {
    let mut rollback = Vec::new();
    let mut no_modes = None;
    let _ = append_surface_transition(
        &mut rollback,
        &mut no_modes,
        to_review,
        to_mouse_capture,
        from_review,
        from_mouse_capture,
    );
    let _ = output.write_all(&rollback).and_then(|()| output.flush());
    if let Some(modes) = modes {
        // Keep the guard aligned with the pre-transition state so the next
        // frame can retry the transition on the same writer.
        modes.set_review_active(from_review);
        modes.set_mouse_capture_active(from_mouse_capture);
    }
}

fn flatten_history(blocks: &[FinalizedBlock]) -> Vec<String> {
    let mut lines = Vec::new();
    for block in blocks {
        if block.separator_before && !block.lines.is_empty() {
            lines.push(String::new());
        }
        lines.extend(block.lines.iter().cloned());
    }
    lines
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use super::*;
    use crate::screen_output::terminal_modes::TerminalModeGuard;

    #[test]
    fn resize_invalidates_live_cache() {
        let mut terminal = InlineTerminal::for_test(80, 12);
        terminal
            .live
            .render_to(&mut Vec::new(), 0, vec!["live".to_owned()], None)
            .expect("initial live frame");

        terminal
            .resize(50, 8, 0, 0, 1)
            .expect("resize with fresh generation");

        let mut redraw = Vec::new();
        terminal
            .live
            .render_to(&mut redraw, 0, vec!["live".to_owned()], None)
            .expect("live redraw after resize");
        assert!(String::from_utf8(redraw).unwrap().contains("live"));
    }

    #[test]
    fn resize_preserves_the_logical_cursor_row_inside_live_viewport() {
        let mut terminal = InlineTerminal::for_test_with_cursor(80, 24, 0, 5);
        terminal
            .render_to(
                &mut Vec::new(),
                &TerminalFrame::new(
                    Vec::new(),
                    vec![
                        "todo".to_owned(),
                        "composer".to_owned(),
                        "footer".to_owned(),
                    ],
                    Some(CursorPos { row: 1, col: 0 }),
                ),
            )
            .expect("initial frame");

        terminal
            .resize(80, 20, 0, 8, 1)
            .expect("height resize with observed cursor");

        assert_eq!(terminal.geometry.live_top, 7);
        assert_eq!(terminal.geometry.cursor_row, 8);
    }

    #[test]
    fn failed_review_transitions_roll_back_on_same_writer_and_retry() {
        let mut terminal = InlineTerminal::for_test(80, 12);
        terminal.modes = Some(TerminalModeGuard::for_test());
        let review =
            TerminalFrame::with_surface(Vec::new(), vec!["review".to_owned()], None, true, None);
        let normal =
            TerminalFrame::with_surface(Vec::new(), vec!["normal".to_owned()], None, false, None);

        let mut enter_failure = FailOnceAfterBytes::new(1);
        let enter_result = terminal.render_to(&mut enter_failure, &review);
        let enter_rollback_output = String::from_utf8(enter_failure.output);
        let enter_failure_surface = terminal.review_surface;
        let enter_failure_guard_review = terminal
            .modes
            .as_ref()
            .expect("test mode guard")
            .review_active_for_test();

        let mut enter_retry = Vec::new();
        let enter_retry_result = terminal.render_to(&mut enter_retry, &review);
        let enter_retry_output = String::from_utf8(enter_retry);

        let mut leave_failure = FailOnceAfterBytes::new(1);
        let leave_result = terminal.render_to(&mut leave_failure, &normal);
        let leave_rollback_output = String::from_utf8(leave_failure.output);
        let leave_failure_surface = terminal.review_surface;
        let (guard_active, leave_failure_guard_review) = {
            let guard = terminal.modes.as_ref().expect("test mode guard");
            (guard.active_for_test(), guard.review_active_for_test())
        };

        let mut leave_retry = Vec::new();
        let leave_retry_result = terminal.render_to(&mut leave_retry, &normal);
        let leave_retry_output = String::from_utf8(leave_retry);
        let final_surface = terminal.review_surface;
        let final_guard_review = terminal
            .modes
            .as_ref()
            .expect("test mode guard")
            .review_active_for_test();
        terminal
            .modes
            .as_mut()
            .expect("test mode guard")
            .disarm_for_test();

        assert!(enter_result.is_err());
        assert!(!enter_failure_surface);
        assert!(!enter_failure_guard_review);
        assert!(
            enter_rollback_output
                .expect("enter rollback output")
                .contains("?1049l")
        );
        assert!(enter_retry_result.is_ok());
        assert!(
            enter_retry_output
                .expect("enter retry output")
                .contains("?1049h")
        );

        assert!(leave_result.is_err());
        assert!(leave_failure_surface);
        assert!(guard_active);
        assert!(leave_failure_guard_review);
        assert!(
            leave_rollback_output
                .expect("leave rollback output")
                .contains("?1049h")
        );
        assert!(leave_retry_result.is_ok());
        assert!(
            leave_retry_output
                .expect("leave retry output")
                .contains("?1049l")
        );
        assert!(!final_surface);
        assert!(!final_guard_review);
    }

    #[test]
    fn task_review_mouse_capture_leaves_before_alternate_screen() {
        let mut terminal = InlineTerminal::for_test(80, 12);
        terminal.modes = Some(TerminalModeGuard::for_test());
        let task =
            TerminalFrame::with_surface(Vec::new(), vec!["task".to_owned()], None, true, None)
                .with_mouse_capture(true);
        let normal =
            TerminalFrame::with_surface(Vec::new(), vec!["normal".to_owned()], None, false, None);

        let mut enter = Vec::new();
        terminal
            .render_to(&mut enter, &task)
            .expect("enter task review");
        let mut leave = Vec::new();
        terminal
            .render_to(&mut leave, &normal)
            .expect("leave task review");
        let leave = String::from_utf8(leave).expect("leave is UTF-8");

        assert!(leave.contains("?1000l"));
        assert!(leave.contains("?1049l"));
        assert!(leave.find("?1000l") < leave.find("?1049l"));
    }

    struct FailOnceAfterBytes {
        output: Vec<u8>,
        remaining: usize,
        failed: bool,
    }

    impl FailOnceAfterBytes {
        const fn new(remaining: usize) -> Self {
            Self {
                output: Vec::new(),
                remaining,
                failed: false,
            }
        }
    }

    impl Write for FailOnceAfterBytes {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.failed {
                self.output.extend_from_slice(bytes);
                return Ok(bytes.len());
            }
            if self.remaining == 0 {
                self.failed = true;
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "injected failure",
                ));
            }
            let written = bytes.len().min(self.remaining);
            self.output.extend_from_slice(&bytes[..written]);
            self.remaining -= written;
            Ok(written)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
