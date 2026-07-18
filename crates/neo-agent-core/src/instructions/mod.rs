//! Pure path-scoped AGENTS.md instruction engine and shared DTOs.
//!
//! This module is dependency-free with respect to runtime, session, TUI,
//! and binary types: it consumes only `Path`/`PathBuf` values and the
//! crate's token estimator. Scope discovery walks only the primary
//! workspace-to-target directory chains; imports expand only standalone
//! single-`@` lines outside fenced code and must canonicalize inside the
//! primary workspace or `$NEO_HOME`.

mod registry;
mod resolver;
mod types;

pub use registry::{
    AdmissionCandidate, AdmissionSelection, InstructionAdmission, InstructionRegistry,
    RehydrationSnapshot,
};
pub use resolver::{
    FilesystemSourceIo, InstructionBundle, InstructionResolver, ResolvedScopes, SourceIo,
    SourceMetadata, find_agents_file, read_source_stable, select_agents_file_name, sha256_hex,
};
pub use types::{
    AgentInstructionState, IgnoredInstructionBundle, InstructionBudget, InstructionBundleMetadata,
    InstructionEpochData, InstructionEpochOutcome, InstructionError, InstructionFailure,
    InstructionFailureKind, InstructionFingerprint, InstructionInheritance,
    InstructionOmissionReason, InstructionPreflightDecision, InstructionReconcileKind,
    InstructionReconcileRequest, InstructionRegistryConfig, InstructionReplacement,
    InstructionScopeData, InstructionScopeKind, MAX_GRAPH_BYTES, MAX_GRAPH_SOURCES,
    MAX_IMPORT_DEPTH, MAX_SOURCE_BYTES, MIN_NOMINAL_BUDGET,
};
