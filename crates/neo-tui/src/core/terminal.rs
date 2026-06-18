//! Terminal rendering is now handled entirely by [`crate::terminal::TuiRenderer`],
//! which implements the single-buffer differential render algorithm.
//!
//! The previous `TerminalRenderer` (a split committed/live-region model) was
//! removed: it could not track the hardware cursor across frames and caused the
//! prompt box to stack downward on every render tick. `TuiRenderer::render`
//! takes one flat `Vec<String>` (all screen lines) and diffs it against the
//! previous frame, so there is no committed/live distinction at the render
//! layer anymore.
