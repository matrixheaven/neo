//! Shared data types for the path-scoped AGENTS.md instruction engine.
//!
//! The engine consumes only `Path`/`PathBuf` values and the crate's token
//! estimator; it never touches runtime, session, TUI, or binary types.
//! Epoch and metadata structs carry display-safe paths, revision and
//! fingerprint strings, and counts only — never source bodies. Expanded
//! content lives exclusively in [`InstructionEpochData::model_content`]
//! and in in-memory [`crate::instructions::InstructionBundle`] values.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Maximum recursive `@path` import depth below the root `AGENTS.md`.
pub const MAX_IMPORT_DEPTH: u32 = 5;
/// Maximum number of sources (root plus imports) in one import graph.
pub const MAX_GRAPH_SOURCES: u32 = 32;
/// Maximum byte length of one instruction source.
pub const MAX_SOURCE_BYTES: u64 = 1024 * 1024;
/// Maximum combined byte length of one complete import graph.
pub const MAX_GRAPH_BYTES: u64 = 8 * 1024 * 1024;
/// Floor of the nominal instruction token budget.
pub const MIN_NOMINAL_BUDGET: u64 = 65_536;

/// Semantic outcome of one instruction epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionEpochOutcome {
    Ready,
    Activated,
    Updated,
    Removed,
    Reactivated,
    PartiallyLoaded,
    Blocked,
}

/// Configuration for [`crate::instructions::InstructionRegistry`].
#[derive(Debug, Clone)]
pub struct InstructionRegistryConfig {
    pub primary_workspace: PathBuf,
    pub neo_home: Option<PathBuf>,
    pub project_trusted: bool,
}

/// One append-only instruction epoch. This is the single persisted source
/// for model content and transcript metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionEpochData {
    pub agent_id: String,
    pub generation: u64,
    pub outcome: InstructionEpochOutcome,
    pub scopes: Vec<InstructionScopeData>,
    pub selected_bundles: Vec<InstructionBundleMetadata>,
    pub ignored_bundles: Vec<IgnoredInstructionBundle>,
    pub replacements: Vec<InstructionReplacement>,
    pub failure: Option<InstructionFailure>,
    pub deferred_tool_ids: Vec<String>,
    // Persisted once in this event and consumed only by model-context projection.
    pub model_content: Option<String>,
}

/// Agent-local view of which instruction state the model has seen.
///
/// Scope paths are canonical absolute paths; home redaction for display is
/// applied later by the transcript projection, not by the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AgentInstructionState {
    pub visible_generation: u64,
    pub visible_revisions: BTreeMap<PathBuf, String>,
    pub active_scopes: Vec<PathBuf>,
    pub most_recent_scope: Option<PathBuf>,
    pub last_epoch_fingerprint: Option<String>,
}

impl AgentInstructionState {
    /// Applies one emitted epoch so the next reconciliation compares against
    /// the state the model has actually seen. `Proceed` decisions carry no
    /// epoch; callers record `fingerprint.hash` themselves in that case.
    pub fn apply_epoch(
        &mut self,
        epoch: &InstructionEpochData,
        fingerprint: &InstructionFingerprint,
    ) {
        self.visible_generation = epoch.generation;
        self.last_epoch_fingerprint = Some(fingerprint.hash.clone());
        self.active_scopes = epoch
            .scopes
            .iter()
            .map(|scope| scope.display_path.clone())
            .collect();
        self.most_recent_scope = epoch.scopes.last().map(|scope| scope.display_path.clone());
        if epoch.outcome != InstructionEpochOutcome::Blocked {
            // Blocked content never becomes model-visible; keep prior revisions.
            self.visible_revisions = epoch
                .selected_bundles
                .iter()
                .map(|bundle| (bundle.display_path.clone(), bundle.revision.clone()))
                .collect();
        }
    }
}

/// The single decision returned by preflight reconciliation.
pub enum InstructionPreflightDecision {
    Proceed {
        fingerprint: InstructionFingerprint,
    },
    Defer {
        epoch: InstructionEpochData,
        fingerprint: InstructionFingerprint,
    },
    Block {
        epoch: InstructionEpochData,
        fingerprint: InstructionFingerprint,
    },
}

/// One frozen reconciliation request.
pub struct InstructionReconcileRequest {
    pub agent_id: String,
    pub kind: InstructionReconcileKind,
    pub target_directories: Vec<PathBuf>,
    pub budget: InstructionBudget,
    pub deferred_tool_ids: Vec<String>,
}

/// Why reconciliation runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionReconcileKind {
    /// Baseline before the first model request of an agent.
    Baseline,
    /// Preflight before a tool batch.
    ToolPreflight,
}

/// How a child agent inherits instruction context from its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionInheritance {
    /// Child seeds the parent's currently applicable nested scopes.
    FullContext,
    /// Child receives only the global/workspace baseline.
    Summary,
}

/// Which filesystem layer a scope belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionScopeKind {
    /// `$NEO_HOME/AGENTS.md`.
    Global,
    /// Trusted ancestor directory above the primary workspace.
    Ancestor,
    /// Primary workspace root.
    WorkspaceRoot,
    /// Nested directory inside the primary workspace.
    Nested,
}

/// Display-safe metadata about one discovered scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionScopeData {
    pub display_path: PathBuf,
    pub kind: InstructionScopeKind,
    pub revision: Option<String>,
    pub token_estimate: u64,
}

/// Display-safe metadata about one admitted bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionBundleMetadata {
    pub display_path: PathBuf,
    pub revision: String,
    pub token_estimate: u64,
    pub byte_size: u64,
    pub source_count: u32,
    pub import_count: u32,
}

/// A complete bundle omitted from the model context as a whole unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IgnoredInstructionBundle {
    pub display_path: PathBuf,
    pub revision: String,
    pub token_estimate: u64,
    pub reason: InstructionOmissionReason,
}

/// Why a complete bundle was omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionOmissionReason {
    OverBudget,
}

/// One bundle revision replacement (previous revision -> new revision).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionReplacement {
    pub display_path: PathBuf,
    pub previous_revision: String,
    pub new_revision: String,
}

/// Typed failure kinds for blocked instruction bundles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionFailureKind {
    MissingImport,
    UnreadableSource,
    InvalidEncoding,
    IncludeCycle,
    LimitExceeded,
    UntrustedImport,
    AmbiguousAgentsFile,
    UnstableSource,
}

impl InstructionFailureKind {
    /// Human-readable reason used in compact failure notices.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::MissingImport => "missing import",
            Self::UnreadableSource => "unreadable source",
            Self::InvalidEncoding => "invalid encoding",
            Self::IncludeCycle => "include cycle",
            Self::LimitExceeded => "instruction limit exceeded",
            Self::UntrustedImport => "untrusted import",
            Self::AmbiguousAgentsFile => "ambiguous AGENTS.md",
            Self::UnstableSource => "unstable source",
        }
    }
}

/// One blocked instruction state: display-safe path, typed kind, and
/// display-safe detail (paths and limit numbers only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionFailure {
    pub display_path: PathBuf,
    pub kind: InstructionFailureKind,
    pub fingerprint: String,
    /// Full display text of the underlying error. Distinguishes blocked
    /// states of one kind (e.g. two different limit violations) and names
    /// the failing source when `display_path` is empty (`LimitExceeded`,
    /// I/O errors). Contains only paths and limit numbers — never bodies.
    pub detail: String,
}

/// Dynamic instruction budget derived from the model window and the tokens
/// safely available in the current request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionBudget {
    pub nominal: u64,
    pub actual: u64,
}

impl InstructionBudget {
    /// Computes `max(65_536, effective_max_tokens / 8)` clamped to the safely
    /// available request capacity.
    #[must_use]
    pub fn from_context(effective_max_tokens: Option<u64>, safely_available_tokens: u64) -> Self {
        let nominal =
            effective_max_tokens.map_or(MIN_NOMINAL_BUDGET, |max| MIN_NOMINAL_BUDGET.max(max / 8));
        let actual = nominal.min(safely_available_tokens);
        Self { nominal, actual }
    }
}

/// Opaque identity of one frozen reconciliation, carrying the inputs needed
/// to re-verify the same frozen generation during fingerprint recheck.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionFingerprint {
    pub hash: String,
    pub agent_id: String,
    pub target_directories: Vec<PathBuf>,
    pub budget: InstructionBudget,
    pub deferred_tool_ids: Vec<String>,
}

/// Typed engine error. Each variant maps to exactly one
/// [`InstructionFailureKind`]; the engine never panics on filesystem state.
#[derive(Debug, thiserror::Error)]
pub enum InstructionError {
    #[error("multiple case-folded AGENTS.md variants in `{directory}`: {}", candidates.iter().map(|c| format!("`{}`", c.display())).collect::<Vec<_>>().join(", "))]
    AmbiguousAgentsFile {
        directory: PathBuf,
        candidates: Vec<PathBuf>,
    },
    #[error("import `{path}` not found")]
    MissingImport { path: PathBuf },
    #[error("source `{path}` is unreadable: {reason}")]
    UnreadableSource { path: PathBuf, reason: String },
    #[error("source `{path}` is not valid UTF-8")]
    InvalidEncoding { path: PathBuf },
    #[error("import cycle reaches `{path}`")]
    IncludeCycle { path: PathBuf },
    #[error("instruction limit exceeded: {0}")]
    LimitExceeded(String),
    #[error("import `{path}` canonicalizes outside the primary workspace and $NEO_HOME")]
    UntrustedImport { path: PathBuf },
    #[error("source `{path}` changed repeatedly while being read")]
    UnstableSource { path: PathBuf },
    #[error("instruction I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl InstructionError {
    /// Maps the error to its typed failure kind.
    #[must_use]
    pub fn failure_kind(&self) -> InstructionFailureKind {
        match self {
            Self::AmbiguousAgentsFile { .. } => InstructionFailureKind::AmbiguousAgentsFile,
            Self::MissingImport { .. } => InstructionFailureKind::MissingImport,
            Self::UnreadableSource { .. } | Self::Io(_) => InstructionFailureKind::UnreadableSource,
            Self::InvalidEncoding { .. } => InstructionFailureKind::InvalidEncoding,
            Self::IncludeCycle { .. } => InstructionFailureKind::IncludeCycle,
            Self::LimitExceeded(_) => InstructionFailureKind::LimitExceeded,
            Self::UntrustedImport { .. } => InstructionFailureKind::UntrustedImport,
            Self::UnstableSource { .. } => InstructionFailureKind::UnstableSource,
        }
    }

    /// Display-safe path most relevant to the failure, when one exists.
    #[must_use]
    pub fn failure_path(&self) -> PathBuf {
        match self {
            Self::AmbiguousAgentsFile { directory, .. } => directory.clone(),
            Self::MissingImport { path }
            | Self::UnreadableSource { path, .. }
            | Self::InvalidEncoding { path }
            | Self::IncludeCycle { path }
            | Self::UntrustedImport { path }
            | Self::UnstableSource { path } => path.clone(),
            Self::LimitExceeded(_) | Self::Io(_) => PathBuf::new(),
        }
    }

    /// Full distinguishing identity of one blocked state: display-safe path,
    /// typed kind, and the error's display text (paths and limit numbers
    /// only). Two blocked states of the same kind but with different
    /// underlying sources or limits never share an identity, so fingerprint
    /// comparisons always surface the changed state as a fresh epoch.
    #[must_use]
    pub fn failure_identity(&self) -> String {
        format!(
            "{}|{}|{}",
            self.failure_path().display(),
            self.failure_kind().describe(),
            self
        )
    }
}
