use std::io::Write;
use std::time::Instant;

use crate::terminal_capabilities::TerminalCapabilities;

use super::LiveRenderer;
use super::terminal_modes::TerminalModeGuard;
use super::types::CursorPos;

const SYNCHRONIZED_OUTPUT_START: &str = "\x1b[?2026h";
const SYNCHRONIZED_OUTPUT_END: &[u8] = b"\x1b[?2026l";
const RESET_SCROLL_REGION: &[u8] = b"\x1b[r";

/// A single bounded fullscreen frame: the visible document slice plus fitted
/// chrome, the logical cursor, and the next animation deadline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalFrame {
    pub lines: Vec<String>,
    pub cursor: Option<CursorPos>,
    pub next_animation_deadline: Option<Instant>,
}

impl TerminalFrame {
    #[must_use]
    pub const fn new(lines: Vec<String>, cursor: Option<CursorPos>) -> Self {
        Self {
            lines,
            cursor,
            next_animation_deadline: None,
        }
    }

    #[must_use]
    pub const fn with_animation_deadline(
        lines: Vec<String>,
        cursor: Option<CursorPos>,
        next_animation_deadline: Option<Instant>,
    ) -> Self {
        Self {
            lines,
            cursor,
            next_animation_deadline,
        }
    }
}

/// Absolute fullscreen geometry owned solely by [`FullscreenTerminal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FullscreenGeometry {
    width: u16,
    height: u16,
    generation: u64,
}

/// The single fullscreen terminal owner.
///
/// Owns the alternate-screen lifecycle (entered once at startup, restored on
/// leave/error/panic/suspend), the bounded live renderer at origin zero, and
/// synchronized-output transactions with write-failure rollback. There is no
/// native history insertion and no second surface: every frame is one bounded
/// document slice written at the top of the alternate screen.
#[derive(Debug)]
pub struct FullscreenTerminal {
    synchronized_output: bool,
    geometry: FullscreenGeometry,
    live: LiveRenderer,
    modes: Option<TerminalModeGuard>,
}

impl FullscreenTerminal {
    #[must_use]
    pub fn new(width: u16, height: u16, capabilities: TerminalCapabilities) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        Self {
            synchronized_output: capabilities.ansi.synchronized_output,
            geometry: FullscreenGeometry {
                width,
                height,
                generation: 0,
            },
            live: LiveRenderer::new(width, height),
            modes: None,
        }
    }

    pub fn enter(
        width: u16,
        height: u16,
        capabilities: TerminalCapabilities,
    ) -> std::io::Result<Self> {
        let modes = TerminalModeGuard::enter(capabilities)?;
        let mut terminal = Self::new(width, height, capabilities);
        terminal.modes = Some(modes);
        Ok(terminal)
    }

    /// Test constructor. Starts at the fullscreen origin `(0, 0)`.
    #[must_use]
    pub fn for_test(width: u16, height: u16) -> Self {
        Self::for_test_with_cursor(width, height, 0, 0)
    }

    /// Test constructor with an explicit zero-based cursor. The cursor is
    /// accepted for signature compatibility; fullscreen geometry always starts
    /// at the alternate-screen origin.
    #[must_use]
    pub fn for_test_with_cursor(
        width: u16,
        height: u16,
        _cursor_col: u16,
        _cursor_row: u16,
    ) -> Self {
        Self::new(width, height, TerminalCapabilities::default())
    }

    pub fn render_to(
        &mut self,
        output: &mut dyn Write,
        frame: &TerminalFrame,
    ) -> std::io::Result<()> {
        let mut next_live = self.live.clone();
        let mut live_bytes = Vec::new();
        if let Err(error) =
            next_live.render_to(&mut live_bytes, 0, frame.lines.clone(), frame.cursor)
        {
            let _ = output.write_all(RESET_SCROLL_REGION);
            let _ = output.flush();
            return Err(error);
        }

        if live_bytes.is_empty() {
            return Ok(());
        }

        let transaction = if self.synchronized_output {
            format!(
                "{SYNCHRONIZED_OUTPUT_START}{}{}",
                String::from_utf8_lossy(&live_bytes),
                String::from_utf8_lossy(SYNCHRONIZED_OUTPUT_END)
            )
            .into_bytes()
        } else {
            live_bytes
        };
        if let Err(error) = output.write_all(&transaction).and_then(|()| output.flush()) {
            let _ = output.write_all(RESET_SCROLL_REGION);
            if self.synchronized_output {
                let _ = output.write_all(SYNCHRONIZED_OUTPUT_END);
            }
            let _ = output.flush();
            // Renderer state was cloned; a failed transaction leaves it
            // unchanged so the next render_to retries the same frame.
            return Err(error);
        }

        // Commit the renderer cache only after a successful flush.
        self.live = next_live;
        Ok(())
    }

    /// Resize with a cursor observation tagged by size generation.
    ///
    /// Stale or out-of-bounds observations fail closed with `InvalidData`.
    /// Same generation and size is a no-op so steady-state frames do not
    /// recompute geometry from a stale cursor snapshot.
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

        self.geometry = FullscreenGeometry {
            width,
            height,
            generation,
        };
        self.live.resize(width, height);
        Ok(())
    }

    /// Apply a resize without generation checks (test helper).
    pub fn resize_for_test(&mut self, width: u16, height: u16) {
        let generation = self.geometry.generation.saturating_add(1);
        let _ = self.resize(width, height, 0, 0, generation);
    }

    /// Clear the live surface and leave the fullscreen modes so the suspended
    /// process returns to the shell's normal screen.
    pub fn suspend_prepare(&mut self, output: &mut dyn Write) -> std::io::Result<()> {
        let result = self.clear_live_to(output, false);
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
        // suspend_prepare already cleared the live state; the next frame must
        // repaint the fresh alternate screen from scratch.
        let deletes = self.live.reset();
        if !deletes.is_empty() {
            return Err(std::io::Error::other(
                "live renderer resume saw pending kitty deletes; suspend_prepare must clear live first",
            ));
        }
        self.resize(width, height, cursor_col, cursor_row, generation)?;
        Ok(())
    }

    pub fn leave(&mut self, output: &mut dyn Write) -> std::io::Result<()> {
        let show_cursor = self.modes.is_none();
        let result = self.clear_live_to(output, show_cursor);
        if let Some(modes) = &mut self.modes {
            modes.leave();
        }
        result
    }

    fn clear_live_to(&mut self, output: &mut dyn Write, show_cursor: bool) -> std::io::Result<()> {
        let mut next_live = self.live.clone();
        let mut transaction = next_live.clear_at_origin(0);
        // Park the cursor at the fullscreen origin before restoring modes.
        transaction.push_str(&String::from_utf8_lossy(RESET_SCROLL_REGION));
        transaction.push_str("\x1b[1;1H");
        if show_cursor {
            transaction.push_str("\x1b[?25h");
        }
        output.write_all(transaction.as_bytes())?;
        output.flush()?;
        self.live = next_live;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use super::*;

    #[test]
    fn resize_invalidates_live_cache() {
        let mut terminal = FullscreenTerminal::for_test(80, 12);
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
    fn failed_transaction_rolls_back_and_retries_same_frame() {
        let mut terminal = FullscreenTerminal::for_test(80, 12);
        let frame = TerminalFrame::new(vec!["surface".to_owned()], None);

        let mut failing = FailOnceAfterBytes::new(1);
        let result = terminal.render_to(&mut failing, &frame);
        assert!(result.is_err());

        let mut retry = Vec::new();
        terminal
            .render_to(&mut retry, &frame)
            .expect("retry must repaint the same frame");
        assert!(String::from_utf8(retry).unwrap().contains("surface"));
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
