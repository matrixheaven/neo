use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::error::{WorkflowError, WorkflowErrorCode};
use crate::AgentTokenUsage;

/// Portable workflow/registry name grammar: `[a-z0-9][a-z0-9_-]{0,63}`.
pub const WORKFLOW_NAME_MAX_LEN: usize = 64;

/// Durable machine identity for one workflow run.
///
/// Canonical name is [`WorkflowRunId`]; `WorkflowId` is the durable newtype
/// storage type (same struct) kept for existing call sites.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowId(pub String);

/// Canonical public name for [`WorkflowId`] (same type).
pub type WorkflowRunId = WorkflowId;

impl WorkflowId {
    /// Generate a machine identity (`wf_` + UUID simple hex).
    #[must_use]
    pub fn generate() -> Self {
        Self(format!("wf_{}", Uuid::new_v4().as_simple()))
    }

    /// Wrap an already validated stored identity without reparsing it.
    #[must_use]
    pub fn from_existing(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Parse a machine identity: raw UUID or `wf_<32-lowercase-hex>`.
    pub fn parse(raw: &str) -> Result<Self, WorkflowError> {
        if let Some(simple) = raw.strip_prefix("wf_") {
            if is_uuid_simple(simple) {
                return Ok(Self(raw.to_owned()));
            }
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                format!("invalid workflow run id {raw:?}: expected wf_<uuid-simple>"),
            ));
        }
        if let Ok(parsed) = Uuid::parse_str(raw)
            && parsed.to_string() == raw
        {
            return Ok(Self(format!("wf_{}", parsed.as_simple())));
        }
        if is_uuid_simple(raw) {
            return Ok(Self(format!("wf_{raw}")));
        }
        Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            format!("invalid workflow run id {raw:?}: expected UUID machine key"),
        ))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WorkflowId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn is_uuid_simple(s: &str) -> bool {
    s.len() == 32
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Human-readable session-local handle (design: WorkflowHandle), e.g. `review-2`.
///
/// Stable after creation and stored in `run.json`. Never used as a journal key.
/// Named `WorkflowHumanHandle` at the type level to avoid colliding with the
/// live runtime control handle exported as `workflow::WorkflowHandle`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowHumanHandle(String);

impl WorkflowHumanHandle {
    /// Parse a portable human handle using the same grammar as [`WorkflowName`].
    pub fn parse(raw: &str) -> Result<Self, WorkflowError> {
        validate_portable_name(raw, "workflow handle")?;
        Ok(Self(raw.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WorkflowHumanHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Validated registry / definition name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowName(String);

impl WorkflowName {
    /// Enforce portable grammar `[a-z0-9][a-z0-9_-]{0,63}` (case-sensitive).
    pub fn parse(raw: &str) -> Result<Self, WorkflowError> {
        validate_portable_name(raw, "workflow name")?;
        Ok(Self(raw.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WorkflowName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Lowercase SHA-256 content hash identifying a definition revision.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowRevision(String);

impl WorkflowRevision {
    /// Hash arbitrary bytes into a lowercase hex SHA-256 revision.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        use sha2::{Digest, Sha256};
        Self(format!("{:x}", Sha256::digest(bytes)))
    }

    /// Accept only a 64-character lowercase hex digest.
    pub fn parse(raw: &str) -> Result<Self, WorkflowError> {
        if raw.len() == 64
            && raw
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Ok(Self(raw.to_owned()));
        }
        Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            format!("invalid workflow revision {raw:?}: expected 64 lowercase hex chars"),
        ))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WorkflowRevision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Deterministic host-call invocation identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowInvocationId(String);

impl WorkflowInvocationId {
    #[must_use]
    pub fn from_existing(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn generate() -> Self {
        Self(format!("inv_{}", Uuid::new_v4().as_simple()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WorkflowInvocationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Durable AwaitingUser request identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowRequestId(String);

impl WorkflowRequestId {
    #[must_use]
    pub fn from_existing(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn generate() -> Self {
        Self(format!("req_{}", Uuid::new_v4().as_simple()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WorkflowRequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Content-addressed artifact identity associated with a run.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowArtifactId {
    pub run_id: WorkflowRunId,
    /// Lowercase SHA-256 of artifact bytes.
    pub content_sha256: String,
}

impl WorkflowArtifactId {
    pub fn new(
        run_id: WorkflowRunId,
        content_sha256: impl Into<String>,
    ) -> Result<Self, WorkflowError> {
        let content_sha256 = content_sha256.into();
        let rev = WorkflowRevision::parse(&content_sha256)?;
        Ok(Self {
            run_id,
            content_sha256: rev.0,
        })
    }

    #[must_use]
    pub fn as_content_sha256(&self) -> &str {
        &self.content_sha256
    }
}

/// Validate the portable name grammar used for workflow names and handles.
///
/// Grammar: `[a-z0-9][a-z0-9_-]{0,63}` — case-sensitive, no Unicode.
pub fn validate_portable_name(raw: &str, kind: &str) -> Result<(), WorkflowError> {
    if raw.is_empty() {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            format!("{kind} must not be empty"),
        ));
    }
    if raw.len() > WORKFLOW_NAME_MAX_LEN {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            format!("{kind} {raw:?} exceeds {WORKFLOW_NAME_MAX_LEN} characters"),
        ));
    }
    let mut chars = raw.chars();
    let Some(first) = chars.next() else {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            format!("{kind} must not be empty"),
        ));
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            format!("invalid {kind} {raw:?}: must match [a-z0-9][a-z0-9_-]{{0,63}}"),
        ));
    }
    if !chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '_' | '-')) {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            format!("invalid {kind} {raw:?}: must match [a-z0-9][a-z0-9_-]{{0,63}}"),
        ));
    }
    Ok(())
}

/// Canonical workflow lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowState {
    /// Admitted/created; waiting for a worker permit.
    Queued,
    Running,
    /// Durable human/model input required; independent of `Paused`.
    AwaitingUser,
    /// Pause requested; currently-approved children may finish, but no new
    /// children will be started. Transitions to `Paused` once all children
    /// have drained.
    Pausing,
    Paused,
    Completed,
    Failed,
    Cancelled,
    ResourceLimited,
}

impl WorkflowState {
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::ResourceLimited
        )
    }

    /// Whether ordinary resume may transition this state without a typed answer.
    #[must_use]
    pub fn allows_ordinary_resume(self) -> bool {
        matches!(self, Self::Paused | Self::Pausing)
    }

    /// Whether rehydration projects this state as paused(host_exit) until resume.
    #[must_use]
    pub fn rehydrates_as_paused_host_exit(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Pausing)
    }

    /// Snake_case wire/label form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::AwaitingUser => "awaiting_user",
            Self::Pausing => "pausing",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::ResourceLimited => "resource_limited",
        }
    }

    /// Whether `from -> to` is an allowed transition.
    #[must_use]
    pub fn can_transition_to(self, to: Self) -> bool {
        use WorkflowState::{
            AwaitingUser, Cancelled, Completed, Failed, Paused, Pausing, Queued, ResourceLimited,
            Running,
        };
        if self.is_terminal() {
            return false;
        }
        matches!(
            (self, to),
            (
                Queued,
                Running | Paused | Cancelled | Failed | ResourceLimited
            ) | (
                Running,
                AwaitingUser | Pausing | Paused | Completed | Failed | Cancelled | ResourceLimited
            ) | (Pausing, Paused | Cancelled | Failed | ResourceLimited)
                | (AwaitingUser, Queued | Cancelled | ResourceLimited)
                | (Paused, Queued | Cancelled)
        )
    }

    /// Reject illegal and terminal self-transitions with a stable error.
    pub fn require_transition_to(self, to: Self) -> Result<(), WorkflowError> {
        if self == to {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidOperation,
                format!("workflow already in state {}", self.as_str()),
            ));
        }
        if self.is_terminal() {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidOperation,
                format!(
                    "terminal workflow state {} is immutable; cannot transition to {}",
                    self.as_str(),
                    to.as_str()
                ),
            ));
        }
        if !self.can_transition_to(to) {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidOperation,
                format!(
                    "illegal workflow transition {} -> {}",
                    self.as_str(),
                    to.as_str()
                ),
            ));
        }
        Ok(())
    }

    /// Explicit transition table entries as `(from, to)` pairs.
    #[must_use]
    pub fn allowed_transitions() -> &'static [(Self, Self)] {
        use WorkflowState::{
            AwaitingUser, Cancelled, Completed, Failed, Paused, Pausing, Queued, ResourceLimited,
            Running,
        };
        &[
            (Queued, Running),
            (Queued, Paused),
            (Queued, Cancelled),
            (Queued, Failed),
            (Queued, ResourceLimited),
            (Running, AwaitingUser),
            (Running, Pausing),
            (Running, Paused),
            (Running, Completed),
            (Running, Failed),
            (Running, Cancelled),
            (Running, ResourceLimited),
            (Pausing, Paused),
            (Pausing, Cancelled),
            (Pausing, Failed),
            (Pausing, ResourceLimited),
            (AwaitingUser, Queued),
            (AwaitingUser, Cancelled),
            (AwaitingUser, ResourceLimited),
            (Paused, Queued),
            (Paused, Cancelled),
        ]
    }

    /// All lifecycle variants (for exhaustive illegal-transition scans).
    #[must_use]
    pub fn all_states() -> &'static [Self] {
        use WorkflowState::{
            AwaitingUser, Cancelled, Completed, Failed, Paused, Pausing, Queued, ResourceLimited,
            Running,
        };
        &[
            Queued,
            Running,
            AwaitingUser,
            Pausing,
            Paused,
            Completed,
            Failed,
            Cancelled,
            ResourceLimited,
        ]
    }
}

/// Definition source origin captured at launch (pinned on the run).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowSourceOrigin {
    Builtin,
    User,
    Project,
    Dynamic,
}

impl WorkflowSourceOrigin {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::User => "user",
            Self::Project => "project",
            Self::Dynamic => "dynamic",
        }
    }
}

/// Pinned definition source snapshot metadata for a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowPinnedSource {
    pub origin: WorkflowSourceOrigin,
    pub name: WorkflowName,
    pub revision: WorkflowRevision,
    /// Display-only locator; never a trust or hash input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_locator: Option<String>,
    /// Exact Lua source bytes as UTF-8 text.
    pub lua_source: String,
    /// Verified lowercase SHA-256 of `lua_source` bytes.
    pub source_sha256: String,
}

/// Final-result ownership metadata (top-level Lua return).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowFinalResultMetadata {
    /// Inline JSON value when small enough to keep in metadata/journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    /// Artifact indirection when the final result is oversized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<WorkflowArtifactId>,
    /// Definition revision whose `output_schema` validated this result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_revision: Option<WorkflowRevision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowActor {
    Human,
    Model,
    Runtime,
}

/// Typed execution origin attached to approvals, tool events, and task projections
/// that originate from a workflow run (design §36).
///
/// Metadata only — does not duplicate full workflow state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowExecutionOrigin {
    pub run_id: WorkflowId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_handle: Option<String>,
    pub definition_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swarm_item_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowInvocationKind {
    Phase,
    Log,
    Delegate,
    Swarm,
    Verify,
    VerifyCommand,
    Report,
    Fail,
    /// Generic host-dispatched tool via `neo.tool` / canonical ToolRegistry.
    Tool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowOutcomeStatus {
    Completed,
    Failed,
    Denied,
    Cancelled,
    ResourceLimited,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowInterruptionReason {
    InstructionReplanRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowPhase {
    pub id: String,
    pub description: String,
}

/// Durable metadata for a workflow run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowRunMetadata {
    pub run_id: WorkflowRunId,
    pub name: String,
    pub description: String,
    pub phases: Vec<WorkflowPhase>,
    pub script: String,
    pub script_sha256: String,
    #[serde(default = "default_args")]
    pub args: serde_json::Value,
    pub launch_source: String,
    /// Pinned final result schema for the production runner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<serde_json::Value>,
    /// Optional user-facing display name pinned at creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Optional input schema JSON pinned at creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<serde_json::Value>,
    /// Pinned definition origin captured at durable creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition_origin: Option<WorkflowSourceOrigin>,
    /// Whether this run was created from an inline (unsaved) definition.
    #[serde(default)]
    pub inline_unsaved: bool,
}

fn default_args() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowChildRef {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowInvocationOutcome {
    pub ok: bool,
    pub status: WorkflowOutcomeStatus,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interruption: Option<WorkflowInterruptionReason>,
    #[serde(default = "default_details")]
    pub details: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_usage: Option<AgentTokenUsage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_refs: Vec<WorkflowChildRef>,
}

fn default_details() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowSnapshot {
    pub id: WorkflowRunId,
    pub title: String,
    pub state: WorkflowState,
    #[serde(default)]
    pub current_phase: Option<String>,
    #[serde(default)]
    pub projection_sequence: Option<u64>,
    #[serde(default)]
    pub recovery_failure: bool,
    #[serde(default)]
    pub started_at_ms: Option<u64>,
    #[serde(default)]
    pub updated_at_ms: Option<u64>,
    #[serde(default)]
    pub invocation_count: u64,
    #[serde(default)]
    pub failure_count: u64,
    #[serde(default)]
    pub actual_usage: Option<AgentTokenUsage>,
    #[serde(default)]
    pub latest_log_summary: Option<String>,
    #[serde(default)]
    pub latest_report_summary: Option<String>,
    #[serde(default)]
    pub terminal_reason: Option<String>,
    /// Pinned display name for notifications and the Operator.
    #[serde(default)]
    pub display_name: String,
    /// Pinned purpose/description for notifications and the Operator.
    #[serde(default)]
    pub purpose: String,
}
