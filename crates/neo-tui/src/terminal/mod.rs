//! Terminal rendering core.
//!
//! Keep this module boring and mechanical: app-specific transcript/chrome code
//! belongs above it.

pub mod renderer;

pub use renderer::{CURSOR_MARKER, CursorPos, TuiRenderer};
