//! Append-only terminal history and bounded live-surface rendering.

pub mod inline_terminal;
mod kitty_image;
pub mod live_renderer;
mod terminal_modes;
mod types;

pub use inline_terminal::{InlineTerminal, TerminalFrame};
pub use live_renderer::LiveRenderer;
pub use types::{CURSOR_MARKER, CursorPos};
