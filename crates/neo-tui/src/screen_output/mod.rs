//! Screen output rendering — differential frame-to-stdout renderer.
//!
//! This module contains the single-buffer differential renderer that takes
//! complete frames (`Vec<String>` with embedded ANSI codes) and writes only
//! the changed lines to stdout. It is NOT a terminal emulator — it never
//! reads stdin or parses user input. Input handling lives in `crate::input`.

mod debug_log;
mod kitty_image;

pub mod frame_differ;

pub use frame_differ::{CURSOR_MARKER, CursorPos, TuiRenderer};
