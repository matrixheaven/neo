//! Rust port of the pi-tui terminal core.
//!
//! Keep this module boring and mechanical: app-specific transcript/chrome code
//! belongs above it.

pub mod tui;

pub use tui::{CURSOR_MARKER, CursorPos, TuiRenderer};
