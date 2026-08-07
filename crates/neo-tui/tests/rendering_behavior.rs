//! Primitive rendering behavior: markdown, thinking blocks, prompt
//! primitives, diff models, terminal frames, and core components.

#[path = "rendering_behavior/core_components.rs"]
mod core_components;
#[path = "rendering_behavior/diff_model.rs"]
mod diff_model;
#[path = "rendering_behavior/markdown_rendering.rs"]
mod markdown_rendering;
#[path = "rendering_behavior/primitives.rs"]
mod primitives;
#[path = "rendering_behavior/primitives_prompt.rs"]
mod primitives_prompt;
#[path = "rendering_behavior/primitives_wrap.rs"]
mod primitives_wrap;
#[path = "rendering_behavior/terminal_frame.rs"]
mod terminal_frame;
#[path = "rendering_behavior/thinking_blocks.rs"]
mod thinking_blocks;
