//! Final-result transport (design §26) and bounded TaskOutput views (design §35).
//!
//! Exactly one top-level Lua return is the sole final-result owner. Oversized
//! values become content-addressed artifact references; actual usage, child
//! refs, and terminal reason stay observable on the surrounding output surface.
//!
//! TaskOutput never loads or serializes a complete journal or artifact. Views
//! are summary / journal / result / artifacts / artifact_content; cursors bind
//! run, view, and query hash; the complete ToolResult is byte-capped.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use base64::Engine;
use sha2::{Digest, Sha256};

use super::artifacts::{
    ArtifactContentRange, ArtifactKind, ArtifactMetadata, ArtifactStore, ArtifactValue,
    serialize_artifact_bytes,
};
use super::error::{WorkflowError, WorkflowErrorCode};
use super::journal::{
    self, JournalEnvelope, JournalPage, JournalPayload, canonicalize_json, scan_journal_page,
};
use super::limits::WorkflowLimits;
use super::state::{
    WorkflowArtifactId, WorkflowChildRef, WorkflowFinalResultMetadata, WorkflowId,
    WorkflowRevision, WorkflowRunMetadata, WorkflowState,
};
use super::user_input::PendingUserInput;
use crate::AgentTokenUsage;

// ---------------------------------------------------------------------------
// Final-result transport (Task 7)
// ---------------------------------------------------------------------------

/// Inline JSON or content-addressed artifact reference for a final result body.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinalResultBody {
    Inline {
        value: serde_json::Value,
    },
    Artifact {
        artifact_id: WorkflowArtifactId,
        sha256: String,
        byte_len: u64,
        media_type: String,
        logical_name: String,
        version: u32,
    },
}

impl FinalResultBody {
    #[must_use]
    pub fn from_metadata(meta: &WorkflowFinalResultMetadata) -> Option<Self> {
        if let Some(value) = &meta.value {
            return Some(Self::Inline {
                value: value.clone(),
            });
        }
        // Artifact-only metadata is reconstructed by the runtime with store lookup.
        None
    }

    #[must_use]
    pub fn artifact_id(&self) -> Option<&WorkflowArtifactId> {
        match self {
            Self::Inline { .. } => None,
            Self::Artifact { artifact_id, .. } => Some(artifact_id),
        }
    }
}

/// Prepared decision: keep inline or stage as an artifact.
#[derive(Debug, Clone, PartialEq)]
pub enum PreparedFinalBody {
    Inline(serde_json::Value),
    NeedsArtifact {
        logical_name: String,
        kind: ArtifactKind,
        value: ArtifactValue,
        media_type: String,
        byte_len: u64,
    },
}

/// Canonical final result with always-observable diagnostics.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CanonicalFinalResult {
    pub body: FinalResultBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_revision: Option<WorkflowRevision>,
    /// Actual provider usage remains observable when the body is artifact-backed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_usage: Option<AgentTokenUsage>,
    /// Canonical child/task references stay inline.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_refs: Vec<WorkflowChildRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
}

impl CanonicalFinalResult {
    /// Journal metadata only — usage/child_refs/reason live on WorkflowOutput.
    #[must_use]
    pub fn to_journal_metadata(&self) -> WorkflowFinalResultMetadata {
        match &self.body {
            FinalResultBody::Inline { value } => WorkflowFinalResultMetadata {
                value: Some(value.clone()),
                artifact_id: None,
                schema_revision: self.schema_revision.clone(),
            },
            FinalResultBody::Artifact { artifact_id, .. } => WorkflowFinalResultMetadata {
                value: None,
                artifact_id: Some(artifact_id.clone()),
                schema_revision: self.schema_revision.clone(),
            },
        }
    }
}

/// Logical name reserved for oversized top-level final results.
pub const FINAL_RESULT_LOGICAL_NAME: &str = "final-result";

/// Serialize a JSON value to canonical (key-sorted) bytes.
pub fn serialize_canonical_json_bytes(value: &serde_json::Value) -> Result<Vec<u8>, WorkflowError> {
    let canonical = canonicalize_json(value);
    serde_json::to_vec(&canonical).map_err(|e| {
        WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            format!("canonical json serialization failed: {e}"),
        )
    })
}

/// Whether a serialized final result should move behind an artifact ref.
#[must_use]
pub fn final_result_exceeds_inline_budget(byte_len: u64, limits: &WorkflowLimits) -> bool {
    // Page-sized TaskOutput budget: keep small results inline; larger ones are
    // content-addressed so journal/result views stay bounded.
    byte_len > limits.task_output_page_bytes
}

/// Decide inline vs artifact staging for exactly one top-level return value.
///
/// Reports are never consulted and never become a synthetic final result.
pub fn prepare_final_body(
    value: serde_json::Value,
    limits: &WorkflowLimits,
) -> Result<PreparedFinalBody, WorkflowError> {
    let bytes = serialize_canonical_json_bytes(&value)?;
    let byte_len = u64::try_from(bytes.len())
        .map_err(|_| WorkflowError::Journal("final result size overflow".to_owned()))?;
    if byte_len > limits.artifact_record_bytes {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::ResourceLimited,
            format!(
                "final result size {byte_len} exceeds artifact_record_bytes {}",
                limits.artifact_record_bytes
            ),
        ));
    }
    if final_result_exceeds_inline_budget(byte_len, limits) {
        Ok(PreparedFinalBody::NeedsArtifact {
            logical_name: FINAL_RESULT_LOGICAL_NAME.to_owned(),
            kind: ArtifactKind::Json,
            value: ArtifactValue::Json(value),
            media_type: ArtifactKind::Json.default_media_type().to_owned(),
            byte_len,
        })
    } else {
        // Store the canonical form so journal bytes match content digests.
        let canonical: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
            WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                format!("canonical json round-trip failed: {e}"),
            )
        })?;
        Ok(PreparedFinalBody::Inline(canonical))
    }
}

/// Build a final-result body from a committed artifact.
#[must_use]
pub fn final_body_from_artifact(meta: &ArtifactMetadata) -> FinalResultBody {
    FinalResultBody::Artifact {
        artifact_id: meta.artifact_id.clone(),
        sha256: meta.sha256.clone(),
        byte_len: meta.byte_len,
        media_type: meta.media_type.clone(),
        logical_name: meta.logical_name.clone(),
        version: meta.version,
    }
}

/// Rebuild a [`CanonicalFinalResult`] from journal metadata + optional store lookup.
pub fn reconstruct_canonical_final_result(
    metadata: &WorkflowFinalResultMetadata,
    artifact: Option<&ArtifactMetadata>,
    actual_usage: Option<AgentTokenUsage>,
    child_refs: Vec<WorkflowChildRef>,
    terminal_reason: Option<String>,
) -> Result<CanonicalFinalResult, WorkflowError> {
    let body = if let Some(value) = &metadata.value {
        FinalResultBody::Inline {
            value: value.clone(),
        }
    } else if let Some(id) = &metadata.artifact_id {
        let meta = artifact.ok_or_else(|| {
            WorkflowError::coded(
                WorkflowErrorCode::ArtifactMissing,
                format!(
                    "final result references missing artifact {}",
                    id.as_content_sha256()
                ),
            )
        })?;
        if &meta.artifact_id != id {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::ArtifactCorrupt,
                "final result artifact metadata identity mismatch",
            ));
        }
        final_body_from_artifact(meta)
    } else {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::JournalCorrupt,
            "final result recorded without value or artifact reference",
        ));
    };
    Ok(CanonicalFinalResult {
        body,
        schema_revision: metadata.schema_revision.clone(),
        actual_usage,
        child_refs,
        terminal_reason,
    })
}

/// Validate that artifact staging bytes match the prepared decision (test/helpers).
pub fn prepared_artifact_bytes(
    prepared: &PreparedFinalBody,
) -> Result<Option<Vec<u8>>, WorkflowError> {
    match prepared {
        PreparedFinalBody::Inline(_) => Ok(None),
        PreparedFinalBody::NeedsArtifact { kind, value, .. } => {
            Ok(Some(serialize_artifact_bytes(*kind, value)?))
        }
    }
}

// ---------------------------------------------------------------------------
// TaskOutput views (Task 18)
// ---------------------------------------------------------------------------

/// Explicit TaskOutput view (design §35.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskOutputView {
    Summary,
    Journal,
    Result,
    Artifacts,
    ArtifactContent,
}

impl TaskOutputView {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Journal => "journal",
            Self::Result => "result",
            Self::Artifacts => "artifacts",
            Self::ArtifactContent => "artifact_content",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, WorkflowError> {
        match raw {
            "summary" => Ok(Self::Summary),
            "journal" => Ok(Self::Journal),
            "result" => Ok(Self::Result),
            "artifacts" => Ok(Self::Artifacts),
            "artifact_content" => Ok(Self::ArtifactContent),
            other => Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                format!("unsupported TaskOutput view `{other}`"),
            )),
        }
    }
}

/// Caller-supplied TaskOutput request after tool-schema parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskOutputRequest {
    pub view: TaskOutputView,
    pub cursor: Option<String>,
    /// Complete ToolResult budget (content + details), never only the page payload.
    pub max_output_bytes: u64,
    /// Required for `artifact_content` when no cursor is supplied.
    pub artifact_id: Option<WorkflowArtifactId>,
}

impl TaskOutputRequest {
    #[must_use]
    pub fn summary(max_output_bytes: u64) -> Self {
        Self {
            view: TaskOutputView::Summary,
            cursor: None,
            max_output_bytes: max_output_bytes.max(1),
            artifact_id: None,
        }
    }

    #[must_use]
    pub fn query_hash(&self) -> String {
        compute_query_hash(self.view, self.artifact_id.as_ref())
    }
}

/// Durable materials copied under the run lock; I/O happens after release.
#[derive(Debug, Clone)]
pub struct TaskOutputMaterials {
    pub run_id: WorkflowId,
    pub journal_path: PathBuf,
    pub journal_format_version: u32,
    pub metadata: WorkflowRunMetadata,
    pub state: WorkflowState,
    pub current_phase: Option<String>,
    pub human_handle: Option<String>,
    pub invocation_count: u64,
    pub failure_count: u64,
    pub actual_usage: Option<AgentTokenUsage>,
    pub inherited_usage: Option<AgentTokenUsage>,
    pub terminal_reason: Option<String>,
    pub latest_log_summary: Option<String>,
    pub latest_report_summary: Option<String>,
    /// Bounded report projections only (already truncated at write time).
    pub reports: Vec<serde_json::Value>,
    pub final_result: Option<CanonicalFinalResult>,
    pub artifacts: ArtifactStore,
    pub pending_user: Option<PendingUserInput>,
    pub started_child_count: u64,
    pub queued_child_count: u64,
    pub terminal_child_count: u64,
    pub admission_wait_reason: Option<String>,
}

/// One journal envelope projection that never silently mid-cuts a record.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct JournalRecordSummary {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub payload_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub record_bytes: u64,
    /// Full envelope when it fits the page budget; omitted when oversized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<JournalEnvelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oversized: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_bytes: Option<u64>,
}

/// Bounded summary view body (design §35.2) — never embeds full journal/artifacts.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WorkflowOutputSummary {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_handle: Option<String>,
    pub name: String,
    pub origin: String,
    pub revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lineage: Option<serde_json::Value>,
    pub state: WorkflowState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission_wait_reason: Option<String>,
    pub started_child_count: u64,
    pub queued_child_count: u64,
    pub terminal_child_count: u64,
    pub invocation_count: u64,
    pub failure_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_usage: Option<AgentTokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherited_usage: Option<AgentTokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_user: Option<PendingUserInputMeta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_result: Option<CanonicalFinalResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub latest_reports: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_metadata: Vec<ArtifactMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_log_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_next_cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts_next_cursor: Option<String>,
}

/// Actionable pending user request metadata exposed by ordinary TaskOutput views.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PendingUserInputMeta {
    pub request_id: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub answer_policy: String,
    pub answer_schema: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    pub next_action: String,
}

/// Canonical paged TaskOutput response (all views).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TaskOutputPage {
    pub view: TaskOutputView,
    pub run_id: String,
    pub kind: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seq: Option<u64>,
    pub has_more: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub returned_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<WorkflowOutputSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub journal: Vec<JournalRecordSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<CanonicalFinalResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_content: Option<ArtifactContentPage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_user: Option<PendingUserInputMeta>,
    /// Wire-compatible state field used by existing TaskOutput consumers.
    pub state: WorkflowState,
    pub failure_count: u64,
    pub invocation_count: u64,
}

/// One page of artifact bytes (never the complete multi-record payload by default).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ArtifactContentPage {
    pub artifact_id: WorkflowArtifactId,
    pub sha256: String,
    pub media_type: String,
    pub logical_name: String,
    pub version: u32,
    pub offset: u64,
    pub total_bytes: u64,
    /// UTF-8 text when valid; otherwise base64.
    pub encoding: String,
    pub content: String,
    pub content_bytes: u64,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct CursorPayload {
    v: u32,
    run_id: String,
    view: TaskOutputView,
    query_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    artifact_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    artifact_index: Option<usize>,
}

const CURSOR_VERSION: u32 = 1;
/// Headroom for text content + JSON envelope so the complete ToolResult stays under budget.
const TOOL_RESULT_OVERHEAD_RESERVE: u64 = 4_096;
/// Hard cap on journal records per page regardless of byte budget.
const MAX_JOURNAL_RECORDS_PER_PAGE: usize = 64;
const MAX_ARTIFACT_META_PER_PAGE: usize = 64;
const MAX_SUMMARY_REPORTS: usize = 8;

#[must_use]
pub fn compute_query_hash(
    view: TaskOutputView,
    artifact_id: Option<&WorkflowArtifactId>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(view.as_str().as_bytes());
    hasher.update(b"|");
    if let Some(id) = artifact_id {
        hasher.update(id.as_content_sha256().as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn encode_cursor(payload: &CursorPayload) -> Result<String, WorkflowError> {
    let bytes = serde_json::to_vec(payload).map_err(|e| {
        WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            format!("cursor encode failed: {e}"),
        )
    })?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

fn decode_cursor(raw: &str) -> Result<CursorPayload, WorkflowError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw.trim().as_bytes())
        .map_err(|e| {
            WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                format!("invalid TaskOutput cursor encoding: {e}"),
            )
        })?;
    serde_json::from_slice(&bytes).map_err(|e| {
        WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            format!("invalid TaskOutput cursor payload: {e}"),
        )
    })
}

/// Validate a cursor against the active request (run / view / query-bound).
fn bind_cursor(
    request: &TaskOutputRequest,
    run_id: &WorkflowId,
) -> Result<Option<CursorPayload>, WorkflowError> {
    let Some(raw) = request.cursor.as_deref() else {
        return Ok(None);
    };
    let cursor = decode_cursor(raw)?;
    if cursor.v != CURSOR_VERSION {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            format!(
                "unsupported TaskOutput cursor version {} (expected {CURSOR_VERSION})",
                cursor.v
            ),
        ));
    }
    if cursor.run_id != run_id.as_str() {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            format!(
                "TaskOutput cursor run_id `{}` does not match `{}`",
                cursor.run_id,
                run_id.as_str()
            ),
        ));
    }
    if cursor.view != request.view {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            format!(
                "TaskOutput cursor view `{}` does not match request view `{}`",
                cursor.view.as_str(),
                request.view.as_str()
            ),
        ));
    }
    let expected_hash = request.query_hash();
    if cursor.query_hash != expected_hash {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            "TaskOutput cursor query/filter does not match the request",
        ));
    }
    Ok(Some(cursor))
}

fn payload_type_name(payload: &JournalPayload) -> &'static str {
    match payload {
        JournalPayload::RunCreated { .. } => "run_created",
        JournalPayload::StateChanged { .. } => "state_changed",
        JournalPayload::InvocationStarted { .. } => "invocation_started",
        JournalPayload::InvocationFinished { .. } => "invocation_finished",
        JournalPayload::SwarmItemQueued { .. } => "swarm_item_queued",
        JournalPayload::SwarmItemStarted { .. } => "swarm_item_started",
        JournalPayload::SwarmItemFinished { .. } => "swarm_item_finished",
        JournalPayload::SchemaRepairStarted { .. } => "schema_repair_started",
        JournalPayload::SchemaRepairFinished { .. } => "schema_repair_finished",
        JournalPayload::UserInputRequested { .. } => "user_input_requested",
        JournalPayload::UserInputAnswered { .. } => "user_input_answered",
        JournalPayload::ArtifactCommitted { .. } => "artifact_committed",
        JournalPayload::FinalResultRecorded { .. } => "final_result_recorded",
        JournalPayload::LineageSeedImported { .. } => "lineage_seed_imported",
        JournalPayload::RecoveryActionApplied { .. } => "recovery_action_applied",
        JournalPayload::UsageRecorded { .. } => "usage_recorded",
        JournalPayload::ProvenanceRecorded { .. } => "provenance_recorded",
        JournalPayload::ChildQueued { .. } => "child_queued",
        JournalPayload::ChildStarted { .. } => "child_started",
        JournalPayload::ChildFinished { .. } => "child_finished",
    }
}

fn envelope_invocation_id(payload: &JournalPayload) -> Option<String> {
    match payload {
        JournalPayload::InvocationStarted { invocation_id, .. }
        | JournalPayload::InvocationFinished { invocation_id, .. }
        | JournalPayload::SwarmItemStarted { invocation_id, .. }
        | JournalPayload::SwarmItemFinished { invocation_id, .. }
        | JournalPayload::SchemaRepairStarted { invocation_id, .. }
        | JournalPayload::UsageRecorded {
            invocation_id: Some(invocation_id),
            ..
        }
        | JournalPayload::ProvenanceRecorded {
            invocation_id: Some(invocation_id),
            ..
        } => Some(invocation_id.clone()),
        JournalPayload::ChildQueued { .. }
        | JournalPayload::ChildStarted { .. }
        | JournalPayload::ChildFinished { .. } => None,
        _ => None,
    }
}

fn envelope_summary_text(payload: &JournalPayload) -> Option<String> {
    match payload {
        JournalPayload::StateChanged { reason, new, .. } => {
            Some(format!("{} ({reason})", new.as_str()))
        }
        JournalPayload::InvocationFinished { outcome, .. }
        | JournalPayload::SwarmItemFinished { outcome, .. } => Some(outcome.summary.clone()),
        JournalPayload::SchemaRepairFinished { summary, .. } => Some(summary.clone()),
        JournalPayload::FinalResultRecorded { .. } => Some("final_result".to_owned()),
        JournalPayload::RecoveryActionApplied { action, .. } => Some(action.clone()),
        _ => None,
    }
}

fn summarize_envelope(envelope: &JournalEnvelope, include_body: bool) -> JournalRecordSummary {
    let record_bytes = serde_json::to_vec(envelope).map_or(0, |b| b.len() as u64);
    JournalRecordSummary {
        seq: envelope.seq,
        timestamp_ms: envelope.timestamp_ms,
        payload_type: payload_type_name(&envelope.payload).to_owned(),
        invocation_id: envelope_invocation_id(&envelope.payload),
        summary: envelope_summary_text(&envelope.payload),
        record_bytes,
        envelope: if include_body {
            Some(envelope.clone())
        } else {
            None
        },
        oversized: if include_body { None } else { Some(true) },
        minimum_bytes: if include_body {
            None
        } else {
            Some(record_bytes)
        },
    }
}

fn page_budget(max_output_bytes: u64) -> u64 {
    max_output_bytes
        .saturating_sub(TOOL_RESULT_OVERHEAD_RESERVE)
        .max(256)
}

fn state_status_str(state: WorkflowState) -> &'static str {
    match state {
        WorkflowState::Queued | WorkflowState::Running => "running",
        WorkflowState::Pausing => "finishing_current_work",
        WorkflowState::AwaitingUser => "waiting_for_user",
        WorkflowState::Paused => "paused",
        WorkflowState::Completed => "completed",
        WorkflowState::Failed => "failed",
        WorkflowState::Cancelled => "cancelled",
        WorkflowState::ResourceLimited => "resource_limited",
    }
}

fn pending_meta(pending: &PendingUserInput) -> PendingUserInputMeta {
    PendingUserInputMeta {
        request_id: pending.request_id.clone(),
        prompt: pending.prompt.clone(),
        title: pending.title.clone(),
        answer_policy: pending.answer_policy.as_str().to_owned(),
        answer_schema: pending.answer_schema.clone(),
        default: pending.default.clone(),
        next_action: if pending
            .answer_policy
            .allows_actor(super::state::WorkflowActor::Model)
        {
            "TaskAnswer".to_owned()
        } else {
            "wait_for_human".to_owned()
        },
    }
}

fn journal_cursor(
    run_id: &WorkflowId,
    next_seq: u64,
    query_hash: &str,
) -> Result<String, WorkflowError> {
    encode_cursor(&CursorPayload {
        v: CURSOR_VERSION,
        run_id: run_id.as_str().to_owned(),
        view: TaskOutputView::Journal,
        query_hash: query_hash.to_owned(),
        next_seq: Some(next_seq),
        offset: None,
        artifact_id: None,
        artifact_index: None,
    })
}

fn artifacts_cursor(
    run_id: &WorkflowId,
    next_index: usize,
    query_hash: &str,
) -> Result<String, WorkflowError> {
    encode_cursor(&CursorPayload {
        v: CURSOR_VERSION,
        run_id: run_id.as_str().to_owned(),
        view: TaskOutputView::Artifacts,
        query_hash: query_hash.to_owned(),
        next_seq: None,
        offset: None,
        artifact_id: None,
        artifact_index: Some(next_index),
    })
}

fn artifact_content_cursor(
    run_id: &WorkflowId,
    artifact_id: &WorkflowArtifactId,
    next_offset: u64,
    query_hash: &str,
) -> Result<String, WorkflowError> {
    let artifact_json = serde_json::to_string(artifact_id).map_err(|e| {
        WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            format!("artifact id cursor encode failed: {e}"),
        )
    })?;
    encode_cursor(&CursorPayload {
        v: CURSOR_VERSION,
        run_id: run_id.as_str().to_owned(),
        view: TaskOutputView::ArtifactContent,
        query_hash: query_hash.to_owned(),
        next_seq: None,
        offset: Some(next_offset),
        artifact_id: Some(artifact_json),
        artifact_index: None,
    })
}

/// Build a bounded summary page from lock-free materials (no journal scan required).
pub fn build_summary_page(
    materials: &TaskOutputMaterials,
    request: &TaskOutputRequest,
) -> Result<TaskOutputPage, WorkflowError> {
    if request.view != TaskOutputView::Summary {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            "build_summary_page requires view=summary",
        ));
    }
    // Summary rejects non-empty cursors that bind other views.
    let _ = bind_cursor(request, &materials.run_id)?;

    let artifact_metadata = materials.artifacts.list_metadata().to_vec();
    let latest_reports: Vec<serde_json::Value> = materials
        .reports
        .iter()
        .rev()
        .take(MAX_SUMMARY_REPORTS)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    let journal_next_cursor = if materials.journal_format_version >= journal::JOURNAL_FORMAT_V2 {
        Some(journal_cursor(
            &materials.run_id,
            0,
            &query_hash_for_view(TaskOutputView::Journal, None),
        )?)
    } else {
        None
    };
    let artifacts_next_cursor = if artifact_metadata.is_empty() {
        None
    } else {
        Some(artifacts_cursor(
            &materials.run_id,
            0,
            &query_hash_for_view(TaskOutputView::Artifacts, None),
        )?)
    };

    let summary = WorkflowOutputSummary {
        run_id: materials.run_id.as_str().to_owned(),
        human_handle: materials.human_handle.clone(),
        name: materials.metadata.name.clone(),
        origin: materials.metadata.launch_source.clone(),
        revision: materials.metadata.script_sha256.clone(),
        lineage: materials.metadata.parent_run_id.as_ref().map(|parent| {
            serde_json::json!({
                "parent_run_id": parent.as_str(),
            })
        }),
        state: materials.state,
        current_phase: materials.current_phase.clone(),
        admission_wait_reason: materials.admission_wait_reason.clone(),
        started_child_count: materials.started_child_count,
        queued_child_count: materials.queued_child_count,
        terminal_child_count: materials.terminal_child_count,
        invocation_count: materials.invocation_count,
        failure_count: materials.failure_count,
        actual_usage: materials.actual_usage,
        inherited_usage: materials.inherited_usage,
        pending_user: materials.pending_user.as_ref().map(pending_meta),
        final_result: materials.final_result.clone(),
        terminal_reason: materials.terminal_reason.clone(),
        latest_reports,
        artifact_metadata,
        latest_log_summary: materials.latest_log_summary.clone(),
        journal_next_cursor,
        artifacts_next_cursor,
    };

    let returned_bytes = serde_json::to_vec(&summary).map_or(0, |b| b.len() as u64);

    Ok(TaskOutputPage {
        view: TaskOutputView::Summary,
        run_id: materials.run_id.as_str().to_owned(),
        kind: "workflow".to_owned(),
        status: state_status_str(materials.state).to_owned(),
        first_seq: None,
        last_seq: None,
        has_more: false,
        next_cursor: None,
        returned_bytes,
        summary: Some(summary),
        journal: Vec::new(),
        result: materials.final_result.clone(),
        artifacts: Vec::new(),
        artifact_content: None,
        pending_user: materials.pending_user.as_ref().map(pending_meta),
        state: materials.state,
        failure_count: materials.failure_count,
        invocation_count: materials.invocation_count,
    })
}

fn query_hash_for_view(view: TaskOutputView, artifact_id: Option<&WorkflowArtifactId>) -> String {
    compute_query_hash(view, artifact_id)
}

/// Page a V2 journal outside runtime locks.
pub fn page_journal_from_path(
    path: &Path,
    materials: &TaskOutputMaterials,
    request: &TaskOutputRequest,
) -> Result<TaskOutputPage, WorkflowError> {
    if request.view != TaskOutputView::Journal {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            "page_journal_from_path requires view=journal",
        ));
    }
    let cursor = bind_cursor(request, &materials.run_id)?;
    let from_seq = cursor.and_then(|c| c.next_seq).unwrap_or(0);
    let budget = page_budget(request.max_output_bytes);
    let query_hash = request.query_hash();

    if materials.journal_format_version < journal::JOURNAL_FORMAT_V2 {
        // V1 journals are inspectable via summary only; full-journal TaskOutput is retired.
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            "journal view requires a V2 journal; use summary for V1 read-only projections",
        ));
    }

    let raw_page = scan_journal_page(
        path,
        Some(&materials.run_id),
        from_seq,
        MAX_JOURNAL_RECORDS_PER_PAGE,
        budget,
    )?;

    let page = pack_journal_page(materials, &raw_page, budget, &query_hash)?;
    Ok(page)
}

fn pack_journal_page(
    materials: &TaskOutputMaterials,
    raw: &JournalPage,
    budget: u64,
    query_hash: &str,
) -> Result<TaskOutputPage, WorkflowError> {
    let mut journal = Vec::new();
    let mut returned_bytes = 0u64;
    let mut first_seq = None;
    let mut last_seq = None;
    let mut has_more = raw.has_more;
    let mut next_seq = raw.next_seq;

    for envelope in &raw.envelopes {
        let encoded_len = serde_json::to_vec(envelope).map_or(0, |b| b.len() as u64);
        if journal.is_empty() && encoded_len > budget {
            // Single record cannot fit: metadata only + explicit minimum size.
            let summary = summarize_envelope(envelope, false);
            returned_bytes =
                serde_json::to_vec(&summary).map_or(summary.record_bytes, |b| b.len() as u64);
            first_seq = Some(envelope.seq);
            last_seq = Some(envelope.seq);
            next_seq = envelope.seq.saturating_add(1);
            // More records may exist after this oversized one.
            has_more = true;
            journal.push(summary);
            break;
        }
        if !journal.is_empty() && returned_bytes.saturating_add(encoded_len) > budget {
            has_more = true;
            next_seq = envelope.seq;
            break;
        }
        let summary = summarize_envelope(envelope, true);
        let summary_bytes = serde_json::to_vec(&summary).map_or(encoded_len, |b| b.len() as u64);
        returned_bytes = returned_bytes.saturating_add(summary_bytes);
        if first_seq.is_none() {
            first_seq = Some(envelope.seq);
        }
        last_seq = Some(envelope.seq);
        next_seq = envelope.seq.saturating_add(1);
        journal.push(summary);
    }

    // If raw page said has_more but we consumed all its envelopes, keep raw flag.
    if raw.has_more && last_seq == raw.last_seq {
        has_more = true;
        next_seq = raw.next_seq;
    }

    let next_cursor = if has_more {
        Some(journal_cursor(&materials.run_id, next_seq, query_hash)?)
    } else {
        None
    };

    Ok(TaskOutputPage {
        view: TaskOutputView::Journal,
        run_id: materials.run_id.as_str().to_owned(),
        kind: "workflow".to_owned(),
        status: state_status_str(materials.state).to_owned(),
        first_seq,
        last_seq,
        has_more,
        next_cursor,
        returned_bytes,
        summary: None,
        journal,
        result: None,
        artifacts: Vec::new(),
        artifact_content: None,
        pending_user: materials.pending_user.as_ref().map(pending_meta),
        state: materials.state,
        failure_count: materials.failure_count,
        invocation_count: materials.invocation_count,
    })
}

/// Result view: bounded final result (inline or artifact ref), never full artifact bytes.
pub fn build_result_page(
    materials: &TaskOutputMaterials,
    request: &TaskOutputRequest,
) -> Result<TaskOutputPage, WorkflowError> {
    if request.view != TaskOutputView::Result {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            "build_result_page requires view=result",
        ));
    }
    let _ = bind_cursor(request, &materials.run_id)?;
    let result = materials.final_result.clone();
    let returned_bytes = result
        .as_ref()
        .and_then(|r| serde_json::to_vec(r).ok())
        .map_or(0, |b| b.len() as u64);
    if returned_bytes > page_budget(request.max_output_bytes) {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::ResourceLimited,
            format!(
                "result view requires at least {returned_bytes} bytes; increase max_output_bytes or read via artifact_content"
            ),
        ));
    }
    Ok(TaskOutputPage {
        view: TaskOutputView::Result,
        run_id: materials.run_id.as_str().to_owned(),
        kind: "workflow".to_owned(),
        status: state_status_str(materials.state).to_owned(),
        first_seq: None,
        last_seq: None,
        has_more: false,
        next_cursor: None,
        returned_bytes,
        summary: None,
        journal: Vec::new(),
        result,
        artifacts: Vec::new(),
        artifact_content: None,
        pending_user: materials.pending_user.as_ref().map(pending_meta),
        state: materials.state,
        failure_count: materials.failure_count,
        invocation_count: materials.invocation_count,
    })
}

/// Artifacts metadata page (no payload bytes).
pub fn build_artifacts_page(
    materials: &TaskOutputMaterials,
    request: &TaskOutputRequest,
) -> Result<TaskOutputPage, WorkflowError> {
    if request.view != TaskOutputView::Artifacts {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            "build_artifacts_page requires view=artifacts",
        ));
    }
    let cursor = bind_cursor(request, &materials.run_id)?;
    let start = cursor.and_then(|c| c.artifact_index).unwrap_or(0);
    let all = materials.artifacts.list_metadata();
    let budget = page_budget(request.max_output_bytes);
    let query_hash = request.query_hash();

    let mut artifacts = Vec::new();
    let mut returned_bytes = 0u64;
    let mut idx = start;
    while idx < all.len() && artifacts.len() < MAX_ARTIFACT_META_PER_PAGE {
        let meta = &all[idx];
        let meta_bytes = serde_json::to_vec(meta).map_or(128, |b| b.len() as u64);
        if !artifacts.is_empty() && returned_bytes.saturating_add(meta_bytes) > budget {
            break;
        }
        if artifacts.is_empty() && meta_bytes > budget {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::ResourceLimited,
                format!(
                    "artifact metadata entry requires at least {meta_bytes} bytes; increase max_output_bytes"
                ),
            ));
        }
        returned_bytes = returned_bytes.saturating_add(meta_bytes);
        artifacts.push(meta.clone());
        idx += 1;
    }
    let has_more = idx < all.len();
    let next_cursor = if has_more {
        Some(artifacts_cursor(&materials.run_id, idx, &query_hash)?)
    } else {
        None
    };

    Ok(TaskOutputPage {
        view: TaskOutputView::Artifacts,
        run_id: materials.run_id.as_str().to_owned(),
        kind: "workflow".to_owned(),
        status: state_status_str(materials.state).to_owned(),
        first_seq: None,
        last_seq: None,
        has_more,
        next_cursor,
        returned_bytes,
        summary: None,
        journal: Vec::new(),
        result: None,
        artifacts,
        artifact_content: None,
        pending_user: materials.pending_user.as_ref().map(pending_meta),
        state: materials.state,
        failure_count: materials.failure_count,
        invocation_count: materials.invocation_count,
    })
}

/// Artifact content page via range read (caller supplies store after lock release).
pub fn build_artifact_content_page(
    materials: &TaskOutputMaterials,
    request: &TaskOutputRequest,
) -> Result<TaskOutputPage, WorkflowError> {
    if request.view != TaskOutputView::ArtifactContent {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            "build_artifact_content_page requires view=artifact_content",
        ));
    }
    let cursor = bind_cursor(request, &materials.run_id)?;
    let artifact_id = resolve_artifact_id(request, materials, cursor.as_ref())?;
    let offset = cursor.and_then(|c| c.offset).unwrap_or(0);
    let budget = page_budget(request.max_output_bytes);
    let query_hash = request.query_hash();

    let range: ArtifactContentRange =
        materials
            .artifacts
            .read_range(&artifact_id, offset, budget)?;
    let content_bytes = range.bytes.len() as u64;
    let (encoding, content) = match std::str::from_utf8(&range.bytes) {
        Ok(text) => ("utf-8".to_owned(), text.to_owned()),
        Err(_) => (
            "base64".to_owned(),
            base64::engine::general_purpose::STANDARD.encode(&range.bytes),
        ),
    };
    let next_offset = offset.saturating_add(content_bytes);
    let has_more = range.has_more;
    let next_cursor = if has_more {
        Some(artifact_content_cursor(
            &materials.run_id,
            &artifact_id,
            next_offset,
            &query_hash,
        )?)
    } else {
        None
    };

    let page = ArtifactContentPage {
        artifact_id: range.metadata.artifact_id.clone(),
        sha256: range.metadata.sha256.clone(),
        media_type: range.metadata.media_type.clone(),
        logical_name: range.metadata.logical_name.clone(),
        version: range.metadata.version,
        offset: range.offset,
        total_bytes: range.metadata.byte_len,
        encoding,
        content,
        content_bytes,
        has_more,
    };
    let returned_bytes = serde_json::to_vec(&page).map_or(content_bytes, |b| b.len() as u64);

    Ok(TaskOutputPage {
        view: TaskOutputView::ArtifactContent,
        run_id: materials.run_id.as_str().to_owned(),
        kind: "workflow".to_owned(),
        status: state_status_str(materials.state).to_owned(),
        first_seq: None,
        last_seq: None,
        has_more,
        next_cursor,
        returned_bytes,
        summary: None,
        journal: Vec::new(),
        result: None,
        artifacts: Vec::new(),
        artifact_content: Some(page),
        pending_user: materials.pending_user.as_ref().map(pending_meta),
        state: materials.state,
        failure_count: materials.failure_count,
        invocation_count: materials.invocation_count,
    })
}

fn resolve_artifact_id(
    request: &TaskOutputRequest,
    materials: &TaskOutputMaterials,
    cursor: Option<&CursorPayload>,
) -> Result<WorkflowArtifactId, WorkflowError> {
    if let Some(id) = &request.artifact_id {
        return Ok(id.clone());
    }
    if let Some(cursor) = cursor
        && let Some(raw) = &cursor.artifact_id
    {
        // Prefer full JSON identity; fall back to sha-only against this run.
        if let Ok(id) = serde_json::from_str::<WorkflowArtifactId>(raw) {
            return Ok(id);
        }
        return WorkflowArtifactId::new(materials.run_id.clone(), raw.clone()).map_err(|e| {
            WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                format!("cursor artifact_id invalid: {e}"),
            )
        });
    }
    Err(WorkflowError::coded(
        WorkflowErrorCode::InvalidInput,
        "artifact_content view requires artifact_id (or a cursor that binds one)",
    ))
}

/// Render a page into a ToolResult that respects the complete byte cap.
pub fn page_to_tool_result(
    page: &TaskOutputPage,
    max_output_bytes: u64,
) -> Result<(String, serde_json::Value), WorkflowError> {
    let details = serde_json::to_value(page).map_err(|e| {
        WorkflowError::coded(
            WorkflowErrorCode::InvalidInput,
            format!("TaskOutput details serialization failed: {e}"),
        )
    })?;
    let content = format_page_content(page);
    let total = measure_tool_result_bytes(&content, &details);
    if total as u64 > max_output_bytes {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::ResourceLimited,
            format!(
                "TaskOutput complete ToolResult is {total} bytes which exceeds max_output_bytes {max_output_bytes}"
            ),
        ));
    }
    Ok((content, details))
}

#[must_use]
pub fn measure_tool_result_bytes(content: &str, details: &serde_json::Value) -> usize {
    let details_len = serde_json::to_vec(details).map_or(0, |b| b.len());
    content.len().saturating_add(details_len)
}

fn format_page_content(page: &TaskOutputPage) -> String {
    let mut content = match page.view {
        TaskOutputView::Summary => format!(
            "task_id: {}\nkind: workflow\nstatus: {}\nview: summary\ninvocations: {}\nfailures: {}",
            page.run_id, page.status, page.invocation_count, page.failure_count
        ),
        TaskOutputView::Journal => format!(
            "task_id: {}\nkind: workflow\nstatus: {}\nview: journal\nfirst_seq: {:?}\nlast_seq: {:?}\nhas_more: {}\nreturned_bytes: {}\nrecords: {}",
            page.run_id,
            page.status,
            page.first_seq,
            page.last_seq,
            page.has_more,
            page.returned_bytes,
            page.journal.len()
        ),
        TaskOutputView::Result => format!(
            "task_id: {}\nkind: workflow\nstatus: {}\nview: result\nhas_result: {}",
            page.run_id,
            page.status,
            page.result.is_some()
        ),
        TaskOutputView::Artifacts => format!(
            "task_id: {}\nkind: workflow\nstatus: {}\nview: artifacts\ncount: {}\nhas_more: {}",
            page.run_id,
            page.status,
            page.artifacts.len(),
            page.has_more
        ),
        TaskOutputView::ArtifactContent => {
            let (offset, bytes, more) = page
                .artifact_content
                .as_ref()
                .map_or((0, 0, false), |c| (c.offset, c.content_bytes, c.has_more));
            format!(
                "task_id: {}\nkind: workflow\nstatus: {}\nview: artifact_content\noffset: {}\ncontent_bytes: {}\nhas_more: {}",
                page.run_id, page.status, offset, bytes, more
            )
        }
    };
    if let Some(pending) = &page.pending_user {
        let _ = write!(
            content,
            "\npending_request_id: {}\nprompt: {}\nanswer_policy: {}\nanswer_schema: {}",
            pending.request_id, pending.prompt, pending.answer_policy, pending.answer_schema
        );
        if let Some(default) = &pending.default {
            let _ = write!(content, "\ndefault_answer: {default}");
        }
        if pending.next_action == "TaskAnswer" {
            let _ = write!(
                content,
                "\nnext_action: TaskAnswer(task_id=\"{}\", request_id=\"{}\", answer=<JSON matching answer_schema>)",
                page.run_id, pending.request_id
            );
        } else {
            let _ = write!(content, "\nnext_action: {}", pending.next_action);
        }
    }
    content
}

/// Dispatch a view after materials were collected (I/O-safe / lock-free).
pub fn render_task_output_page(
    materials: &TaskOutputMaterials,
    request: &TaskOutputRequest,
) -> Result<TaskOutputPage, WorkflowError> {
    let mut page = match request.view {
        TaskOutputView::Summary => build_summary_page(materials, request)?,
        TaskOutputView::Journal => {
            page_journal_from_path(&materials.journal_path, materials, request)?
        }
        TaskOutputView::Result => build_result_page(materials, request)?,
        TaskOutputView::Artifacts => build_artifacts_page(materials, request)?,
        TaskOutputView::ArtifactContent => build_artifact_content_page(materials, request)?,
    };
    shrink_page_to_tool_result_cap(&mut page, request)?;
    Ok(page)
}

/// Drop trailing journal/artifact entries until the complete ToolResult fits.
fn shrink_page_to_tool_result_cap(
    page: &mut TaskOutputPage,
    request: &TaskOutputRequest,
) -> Result<(), WorkflowError> {
    let max = request.max_output_bytes.max(1);
    for _ in 0..256 {
        let (content, details) = {
            let content = format_page_content(page);
            let details = serde_json::to_value(&*page).map_err(|e| {
                WorkflowError::coded(
                    WorkflowErrorCode::InvalidInput,
                    format!("TaskOutput details serialization failed: {e}"),
                )
            })?;
            (content, details)
        };
        let total = measure_tool_result_bytes(&content, &details) as u64;
        if total <= max {
            page.returned_bytes = page
                .returned_bytes
                .min(total.saturating_sub(content.len() as u64));
            return Ok(());
        }
        match page.view {
            TaskOutputView::Journal if page.journal.len() > 1 => {
                let removed = page.journal.pop().expect("len > 1");
                page.has_more = true;
                page.last_seq = page.journal.last().map(|r| r.seq);
                page.first_seq = page.journal.first().map(|r| r.seq);
                page.next_cursor = Some(journal_cursor(
                    &WorkflowId::from_existing(page.run_id.clone()),
                    removed.seq,
                    &compute_query_hash(TaskOutputView::Journal, None),
                )?);
                page.returned_bytes = page
                    .journal
                    .iter()
                    .map(|r| serde_json::to_vec(r).map_or(r.record_bytes, |b| b.len() as u64))
                    .sum();
            }
            TaskOutputView::Artifacts if page.artifacts.len() > 1 => {
                page.artifacts.pop();
                page.has_more = true;
                page.next_cursor = Some(artifacts_cursor(
                    &WorkflowId::from_existing(page.run_id.clone()),
                    // Cannot recover exact index; client restarts artifacts paging.
                    0,
                    &compute_query_hash(TaskOutputView::Artifacts, None),
                )?);
            }
            TaskOutputView::Journal if page.journal.len() == 1 => {
                // Keep metadata-only for the single oversized record.
                if let Some(rec) = page.journal.first_mut() {
                    if rec.envelope.is_none() && rec.oversized == Some(true) {
                        // Already metadata-only; truncate summary text.
                        if let Some(summary) = rec.summary.as_mut() {
                            const MAX: usize = 128;
                            if summary.len() > MAX {
                                summary.truncate(MAX);
                                summary.push('…');
                            }
                        }
                        rec.record_bytes = 0;
                    } else {
                        rec.envelope = None;
                        rec.oversized = Some(true);
                        rec.minimum_bytes = Some(rec.record_bytes);
                        if let Some(summary) = rec.summary.as_mut() {
                            const MAX: usize = 128;
                            if summary.len() > MAX {
                                summary.truncate(MAX);
                                summary.push('…');
                            }
                        }
                    }
                }
                let only = page.journal[0].seq;
                page.first_seq = Some(only);
                page.last_seq = Some(only);
                page.returned_bytes =
                    serde_json::to_vec(&page.journal[0]).map_or(64, |b| b.len() as u64);
            }
            _ => {
                return Err(WorkflowError::coded(
                    WorkflowErrorCode::ResourceLimited,
                    format!(
                        "TaskOutput complete ToolResult is {total} bytes which exceeds max_output_bytes {max}"
                    ),
                ));
            }
        }
    }
    Err(WorkflowError::coded(
        WorkflowErrorCode::ResourceLimited,
        format!("TaskOutput could not shrink under max_output_bytes {max}"),
    ))
}
