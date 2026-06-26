//! Style, Rect, and the RESET constant for terminal rendering.

use super::color::Color;

/// Reset all attributes to terminal default.
pub const RESET: &str = "\x1b[0m";

/// A rectangular region used for layout calculations.
///
/// A simple rectangle used for layout calculations.
/// from external dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    #[must_use]
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    #[must_use]
    pub const fn bottom(&self) -> u16 {
        self.y + self.height
    }

    #[must_use]
    pub const fn right(&self) -> u16 {
        self.x + self.width
    }

    #[must_use]
    pub const fn area(&self) -> u32 {
        self.width as u32 * self.height as u32
    }
}

/// A text style for rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub blink: bool,
    pub reversed: bool,
    pub crossed_out: bool,
}

impl Style {
    #[must_use]
    pub fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    #[must_use]
    pub fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    #[must_use]
    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    #[must_use]
    pub fn dim(mut self) -> Self {
        self.dim = true;
        self
    }

    #[must_use]
    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    #[must_use]
    pub fn underline(mut self) -> Self {
        self.underline = true;
        self
    }

    #[must_use]
    pub fn crossed_out(mut self) -> Self {
        self.crossed_out = true;
        self
    }

    /// Is this the default (empty) style?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Style::default()
    }
}
