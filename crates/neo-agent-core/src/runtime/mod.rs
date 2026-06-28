//! Agent runtime — turn loop, tool dispatch, permissions, compaction.
//!
//! This module is being progressively split from `legacy.rs` into
//! focused submodules. During the transition, `legacy.rs` contains
//! all the code; submodules are extracted one domain at a time.

mod chat_request;
mod config;
mod context;
mod events;
mod image_blobs;
mod legacy;
mod plan_orchestration;
mod queue;
mod skill_dispatch;
mod stream_aggregator;
mod tokens;

// Re-export all public items from `legacy.rs` so that the existing
// `pub use runtime::*;` in `lib.rs` continues to resolve without
// logic or call-site changes during the split.
pub use config::*;
pub use context::*;
pub use legacy::*;
pub use queue::*;

// Submodules will be added here as code is extracted from legacy.rs.
