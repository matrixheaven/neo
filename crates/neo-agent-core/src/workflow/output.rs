//! Canonical final-result transport and related result types (design §26).
//!
//! Exactly one top-level Lua return is the sole final-result owner. Oversized
//! values become content-addressed artifact references; actual usage, child
//! refs, and terminal reason stay observable on the surrounding output surface.

use super::artifacts::{ArtifactKind, ArtifactMetadata, ArtifactValue, serialize_artifact_bytes};
use super::error::{WorkflowError, WorkflowErrorCode};
use super::journal::canonicalize_json;
use super::limits::WorkflowLimits;
use super::state::{
    WorkflowArtifactId, WorkflowChildRef, WorkflowFinalResultMetadata, WorkflowRevision,
};
use crate::AgentTokenUsage;

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
