//! Rendering primitives: color, style, text layout, and component traits.
//!
//! This module consolidates foundational types and functions used across
//! the entire TUI: Color and Style value types, ANSI escape sequence
//! builders, text measurement/wrapping utilities, and the Component trait.

pub mod ansi_escape;
pub mod color;
pub mod component;
pub mod container;
pub mod line;
pub mod style;
pub mod text;
pub mod text_layout;

// Flat re-exports for ergonomic single-path imports
pub use ansi_escape::{bg_to_ansi, fg_to_ansi, paint, strip_ansi, style_to_ansi};
pub use color::Color;
pub use component::{Component, Expandable, Finalization, InputResult};
pub use container::{Container, GutterContainer};
pub use line::{Line, Span};
pub use style::{RESET, Rect, Style};
pub use text::Text;
pub use text_layout::{
    pad_to_width, truncate_to_width, truncate_width, visible_width, wrap_text, wrap_width,
    wrap_width_with_indices,
};

// Crate-visible re-exports
pub(crate) use ansi_escape::next_sequence;
pub(crate) use text_layout::{clip_plain_to_width, clip_visible_to_width, update_active_sgr};
