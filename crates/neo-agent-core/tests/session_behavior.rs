//! Session behavior: JSONL append and torn/corrupt recovery, schema
//! compatibility reads, session state, session tree metadata, and
//! instruction registry.

#[path = "session_behavior/instructions.rs"]
mod instructions;
#[path = "session_behavior/instructions_admission.rs"]
mod instructions_admission;
#[path = "session_behavior/jsonl_append.rs"]
mod jsonl_append;
#[path = "session_behavior/jsonl_recovery.rs"]
mod jsonl_recovery;
#[path = "session_behavior/schema_compatibility.rs"]
mod schema_compatibility;
#[path = "session_behavior/state.rs"]
mod state;
#[path = "session_behavior/tree.rs"]
mod tree;
