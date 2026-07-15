/// Cursor position for prompt editing inside the mutable live surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPos {
    pub row: usize,
    pub col: usize,
}

/// Zero-width marker used while composing prompt lines before cursor extraction.
pub const CURSOR_MARKER: &str = "\x1b_pi:c\x07";
