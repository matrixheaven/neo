//! Agent runtime — turn loop, tool dispatch, permissions, compaction.
//!
//! This module is being progressively split from `legacy.rs` into
//! focused submodules. During the transition, `legacy.rs` contains
//! all the code; submodules are extracted one domain at a time.

mod legacy;

// Re-export all public items from `legacy.rs` so that the existing
// `pub use runtime::*;` in `lib.rs` continues to resolve without
// logic or call-site changes during the split.
pub use legacy::*;

// Submodules will be added here as code is extracted from legacy.rs.
