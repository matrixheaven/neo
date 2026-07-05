//! Unified terminal capability contract used by the renderer and image picker.

pub use crate::terminal_image::TerminalImageCapabilities;

/// ANSI/control-sequence capabilities the TUI renderer needs.
///
/// These are independent on/off features; a bitmask or enum set would add
/// complexity without improving safety, so we keep plain bools.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnsiCapabilities {
    /// Terminal supports basic ANSI color sequences.
    pub color: bool,
    /// Terminal supports cursor-addressing escape sequences.
    pub cursor_addressing: bool,
    /// Terminal can enable bracketed paste.
    pub bracketed_paste: bool,
    /// Terminal supports Kitty-style keyboard enhancement.
    pub kitty_keyboard: bool,
    /// Terminal supports synchronized output (ESC[?2026h/l).
    pub synchronized_output: bool,
}

/// Combined terminal capability contract.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalCapabilities {
    pub ansi: AnsiCapabilities,
    pub image: TerminalImageCapabilities,
}

impl TerminalCapabilities {
    /// Minimum capabilities required to enter the interactive TUI. Cursor
    /// addressing is the hard requirement; color is optional so `NO_COLOR`
    /// users still get a TUI.
    #[must_use]
    pub const fn can_run_tui(self) -> bool {
        self.ansi.cursor_addressing
    }
}
