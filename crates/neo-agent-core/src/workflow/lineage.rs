//! Verified linked-run checkpoints and lineage seed import (design §34).
//!
//! Linked creation never depends on mutable parent files after seed import:
//! the completed invocation prefix and referenced artifact bytes are copied
//! into the new run's journal/artifact store under verified hashes.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::AgentTokenUsage;
use crate::workflow::artifacts::{ArtifactKind, ArtifactStore, ArtifactValue, artifacts_dir};
use crate::workflow::error::{WorkflowError, WorkflowErrorCode};
use crate::workflow::journal::{
    self, JournalEnvelope, JournalPayload, JournalRecord, find_incomplete_invocations,
    find_incomplete_invocations_v2,
};
use crate::workflow::limits::WorkflowLimits;
use crate::workflow::state::{
    WorkflowCheckpoint, WorkflowId, WorkflowInvocationKind, WorkflowInvocationOutcome,
    WorkflowLineageMetadata, WorkflowRunMetadata,
};

/// One completed host-call pair imported as lineage seed (not charged to actual usage).
#[derive(Debug, Clone, PartialEq)]
pub struct LineageSeedInvocation {
    pub invocation_id: String,
    pub call_index: u64,
    pub kind: WorkflowInvocationKind,
    pub canonical_input_hash: String,
    pub canonical_input: Option<serde_json::Value>,
    pub outcome: WorkflowInvocationOutcome,
    pub source_run_id: WorkflowId,
}

/// Artifact bytes verified from the parent and ready to content-address into the child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedArtifactRef {
    pub sha256: String,
    pub byte_len: u64,
    pub media_type: Option<String>,
    pub logical_name: Option<String>,
    pub bytes: Vec<u8>,
}

/// Verified completed prefix ready for journal seed import.
#[derive(Debug, Clone)]
pub struct VerifiedPrefix {
    pub checkpoint: WorkflowCheckpoint,
    pub seed_invocations: Vec<LineageSeedInvocation>,
    pub artifacts: Vec<SeedArtifactRef>,
    pub inherited_usage: Option<AgentTokenUsage>,
    pub lineage: WorkflowLineageMetadata,
}

fn add_usage(total: Option<AgentTokenUsage>, usage: AgentTokenUsage) -> AgentTokenUsage {
    let total = total.unwrap_or(AgentTokenUsage {
        input_tokens: 0,
        output_tokens: 0,
        input_cache_read_tokens: 0,
        input_cache_write_tokens: 0,
    });
    total.saturating_add(usage)
}

fn sum_seed_usage(entries: &[LineageSeedInvocation]) -> Option<AgentTokenUsage> {
    entries
        .iter()
        .fold(None, |acc, entry| match entry.outcome.actual_usage {
            Some(usage) => Some(add_usage(acc, usage)),
            None => acc,
        })
}

/// Running SHA-256 over the durable journal prefix through `through_seq` (inclusive).
pub fn compute_prefix_digest_v1(
    records: &[JournalRecord],
    through_seq: u64,
) -> Result<String, WorkflowError> {
    let mut hasher = Sha256::new();
    let mut saw = false;
    for record in records {
        if record.seq() > through_seq {
            break;
        }
        saw = true;
        let line = serde_json::to_string(record).map_err(|e| {
            WorkflowError::coded(
                WorkflowErrorCode::JournalCorrupt,
                format!("failed to serialize V1 record for prefix digest: {e}"),
            )
        })?;
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    if !saw {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::LineageMismatch,
            format!("no journal records at or before sequence {through_seq}"),
        ));
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Running SHA-256 over the durable V2 journal prefix through `through_seq` (inclusive).
pub fn compute_prefix_digest_v2(
    envelopes: &[JournalEnvelope],
    through_seq: u64,
) -> Result<String, WorkflowError> {
    let mut hasher = Sha256::new();
    let mut saw = false;
    for envelope in envelopes {
        if envelope.seq > through_seq {
            break;
        }
        saw = true;
        let line = serde_json::to_string(envelope).map_err(|e| {
            WorkflowError::coded(
                WorkflowErrorCode::JournalCorrupt,
                format!("failed to serialize V2 envelope for prefix digest: {e}"),
            )
        })?;
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    if !saw {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::LineageMismatch,
            format!("no journal envelopes at or before sequence {through_seq}"),
        ));
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Latest sequence with a complete (no incomplete host-call) journal prefix.
pub fn latest_eligible_sequence_v1(records: &[JournalRecord]) -> Result<u64, WorkflowError> {
    if records.is_empty() {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::LineageMismatch,
            "parent journal is empty; no eligible checkpoint",
        ));
    }
    let mut best: Option<u64> = None;
    for record in records {
        let seq = record.seq();
        let prefix: Vec<_> = records.iter().filter(|r| r.seq() <= seq).cloned().collect();
        if find_incomplete_invocations(&prefix).is_empty() {
            best = Some(seq);
        }
    }
    best.ok_or_else(|| {
        WorkflowError::coded(
            WorkflowErrorCode::LineageMismatch,
            "parent has no eligible checkpoint without incomplete effects",
        )
    })
}

/// Latest sequence with a complete (no incomplete host-call) V2 journal prefix.
pub fn latest_eligible_sequence_v2(envelopes: &[JournalEnvelope]) -> Result<u64, WorkflowError> {
    if envelopes.is_empty() {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::LineageMismatch,
            "parent journal is empty; no eligible checkpoint",
        ));
    }
    let mut best: Option<u64> = None;
    for envelope in envelopes {
        let seq = envelope.seq;
        let prefix: Vec<_> = envelopes.iter().filter(|e| e.seq <= seq).cloned().collect();
        if find_incomplete_invocations_v2(&prefix).is_empty() {
            best = Some(seq);
        }
    }
    best.ok_or_else(|| {
        WorkflowError::coded(
            WorkflowErrorCode::LineageMismatch,
            "parent has no eligible checkpoint without incomplete effects",
        )
    })
}

fn ensure_complete_prefix_v1(
    records: &[JournalRecord],
    through_seq: u64,
) -> Result<(), WorkflowError> {
    let prefix: Vec<_> = records
        .iter()
        .filter(|r| r.seq() <= through_seq)
        .cloned()
        .collect();
    if prefix.is_empty() || !prefix.iter().any(|r| r.seq() == through_seq) {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::LineageMismatch,
            format!("checkpoint sequence {through_seq} is not present in parent journal"),
        ));
    }
    let incomplete = find_incomplete_invocations(&prefix);
    if !incomplete.is_empty() {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::LineageMismatch,
            format!(
                "checkpoint sequence {through_seq} includes incomplete invocation {}",
                incomplete[0].invocation_id
            ),
        ));
    }
    Ok(())
}

fn ensure_complete_prefix_v2(
    envelopes: &[JournalEnvelope],
    through_seq: u64,
) -> Result<(), WorkflowError> {
    let prefix: Vec<_> = envelopes
        .iter()
        .filter(|e| e.seq <= through_seq)
        .cloned()
        .collect();
    if prefix.is_empty() || !prefix.iter().any(|e| e.seq == through_seq) {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::LineageMismatch,
            format!("checkpoint sequence {through_seq} is not present in parent journal"),
        ));
    }
    let incomplete = find_incomplete_invocations_v2(&prefix);
    if !incomplete.is_empty() {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::LineageMismatch,
            format!(
                "checkpoint sequence {through_seq} includes incomplete invocation {}",
                incomplete[0].invocation_id
            ),
        ));
    }
    Ok(())
}

/// Build a verified V1 prefix for linked import. Never includes incomplete effects.
pub fn extract_verified_prefix_v1(
    parent_meta: &WorkflowRunMetadata,
    records: &[JournalRecord],
    requested: Option<&WorkflowCheckpoint>,
    link_reason: &str,
) -> Result<VerifiedPrefix, WorkflowError> {
    let through_seq = match requested {
        Some(cp) => {
            if cp.run_id != parent_meta.run_id {
                return Err(WorkflowError::coded(
                    WorkflowErrorCode::LineageMismatch,
                    "checkpoint run id does not match parent",
                ));
            }
            ensure_complete_prefix_v1(records, cp.sequence)?;
            let digest = compute_prefix_digest_v1(records, cp.sequence)?;
            if digest != cp.prefix_digest {
                return Err(WorkflowError::coded(
                    WorkflowErrorCode::LineageMismatch,
                    "checkpoint prefix_digest does not match verified parent prefix",
                ));
            }
            cp.sequence
        }
        None => latest_eligible_sequence_v1(records)?,
    };

    ensure_complete_prefix_v1(records, through_seq)?;
    let digest = compute_prefix_digest_v1(records, through_seq)?;
    let checkpoint = WorkflowCheckpoint::new(parent_meta.run_id.clone(), through_seq, digest)?;

    let finished: HashMap<&str, &WorkflowInvocationOutcome> = records
        .iter()
        .filter_map(|r| match r {
            JournalRecord::InvocationFinished {
                invocation_id,
                outcome,
                seq,
                ..
            } if *seq <= through_seq => Some((invocation_id.as_str(), outcome)),
            _ => None,
        })
        .collect();

    let mut seed_invocations = Vec::new();
    for record in records {
        if record.seq() > through_seq {
            break;
        }
        if let JournalRecord::InvocationStarted {
            invocation_id,
            call_index,
            kind,
            canonical_input,
            canonical_input_hash,
            ..
        } = record
        {
            let Some(outcome) = finished.get(invocation_id.as_str()) else {
                return Err(WorkflowError::coded(
                    WorkflowErrorCode::LineageMismatch,
                    format!("incomplete invocation {invocation_id} in verified prefix"),
                ));
            };
            seed_invocations.push(LineageSeedInvocation {
                invocation_id: invocation_id.clone(),
                call_index: *call_index,
                kind: *kind,
                canonical_input_hash: canonical_input_hash.clone(),
                canonical_input: Some(canonical_input.clone()),
                outcome: (*outcome).clone(),
                source_run_id: parent_meta.run_id.clone(),
            });
        }
    }

    let inherited_usage = sum_seed_usage(&seed_invocations);
    Ok(VerifiedPrefix {
        checkpoint: checkpoint.clone(),
        seed_invocations,
        artifacts: Vec::new(),
        inherited_usage,
        lineage: WorkflowLineageMetadata {
            parent_run_id: Some(parent_meta.run_id.clone()),
            parent_checkpoint: Some(checkpoint),
            link_reason: Some(link_reason.to_owned()),
        },
    })
}

/// Build a verified V2 prefix for linked import, including referenced artifacts.
pub fn extract_verified_prefix_v2(
    parent_meta: &WorkflowRunMetadata,
    parent_run_dir: &Path,
    envelopes: &[JournalEnvelope],
    requested: Option<&WorkflowCheckpoint>,
    link_reason: &str,
) -> Result<VerifiedPrefix, WorkflowError> {
    let through_seq = match requested {
        Some(cp) => {
            if cp.run_id != parent_meta.run_id {
                return Err(WorkflowError::coded(
                    WorkflowErrorCode::LineageMismatch,
                    "checkpoint run id does not match parent",
                ));
            }
            ensure_complete_prefix_v2(envelopes, cp.sequence)?;
            let digest = compute_prefix_digest_v2(envelopes, cp.sequence)?;
            if digest != cp.prefix_digest {
                return Err(WorkflowError::coded(
                    WorkflowErrorCode::LineageMismatch,
                    "checkpoint prefix_digest does not match verified parent prefix",
                ));
            }
            cp.sequence
        }
        None => latest_eligible_sequence_v2(envelopes)?,
    };

    ensure_complete_prefix_v2(envelopes, through_seq)?;
    let digest = compute_prefix_digest_v2(envelopes, through_seq)?;
    let checkpoint = WorkflowCheckpoint::new(parent_meta.run_id.clone(), through_seq, digest)?;

    let finished: HashMap<&str, &WorkflowInvocationOutcome> = envelopes
        .iter()
        .filter_map(|e| {
            if e.seq > through_seq {
                return None;
            }
            match &e.payload {
                JournalPayload::InvocationFinished {
                    invocation_id,
                    outcome,
                } => Some((invocation_id.as_str(), outcome)),
                _ => None,
            }
        })
        .collect();

    let mut seed_invocations = Vec::new();
    for envelope in envelopes {
        if envelope.seq > through_seq {
            break;
        }
        if let JournalPayload::InvocationStarted {
            invocation_id,
            call_index,
            kind,
            canonical_input,
        } = &envelope.payload
        {
            let Some(outcome) = finished.get(invocation_id.as_str()) else {
                return Err(WorkflowError::coded(
                    WorkflowErrorCode::LineageMismatch,
                    format!("incomplete invocation {invocation_id} in verified prefix"),
                ));
            };
            let hash = envelope.canonical_input_hash.clone().unwrap_or_else(|| {
                canonical_input
                    .as_ref()
                    .map(journal::canonical_input_hash)
                    .unwrap_or_default()
            });
            seed_invocations.push(LineageSeedInvocation {
                invocation_id: invocation_id.clone(),
                call_index: *call_index,
                kind: *kind,
                canonical_input_hash: hash,
                canonical_input: canonical_input.clone(),
                outcome: (*outcome).clone(),
                source_run_id: parent_meta.run_id.clone(),
            });
        }
    }

    let artifacts = load_verified_artifacts(parent_run_dir, envelopes, through_seq)?;
    let inherited_usage = sum_seed_usage(&seed_invocations);
    Ok(VerifiedPrefix {
        checkpoint: checkpoint.clone(),
        seed_invocations,
        artifacts,
        inherited_usage,
        lineage: WorkflowLineageMetadata {
            parent_run_id: Some(parent_meta.run_id.clone()),
            parent_checkpoint: Some(checkpoint),
            link_reason: Some(link_reason.to_owned()),
        },
    })
}

fn load_verified_artifacts(
    parent_run_dir: &Path,
    envelopes: &[JournalEnvelope],
    through_seq: u64,
) -> Result<Vec<SeedArtifactRef>, WorkflowError> {
    let dir = artifacts_dir(parent_run_dir);
    let mut out = Vec::new();
    for envelope in envelopes {
        if envelope.seq > through_seq {
            break;
        }
        let JournalPayload::ArtifactCommitted {
            sha256,
            byte_len,
            media_type,
            logical_name,
            ..
        } = &envelope.payload
        else {
            continue;
        };
        let path = dir.join(sha256);
        let bytes = std::fs::read(&path).map_err(|e| {
            WorkflowError::coded(
                WorkflowErrorCode::ArtifactMissing,
                format!("parent artifact {sha256} missing for lineage import: {e}"),
            )
        })?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        if digest != *sha256 {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::ArtifactCorrupt,
                format!("parent artifact {sha256} content digest mismatch"),
            ));
        }
        if bytes.len() as u64 != *byte_len {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::ArtifactCorrupt,
                format!(
                    "parent artifact {sha256} size mismatch: expected {byte_len}, got {}",
                    bytes.len()
                ),
            ));
        }
        out.push(SeedArtifactRef {
            sha256: sha256.clone(),
            byte_len: *byte_len,
            media_type: media_type.clone(),
            logical_name: logical_name.clone(),
            bytes,
        });
    }
    Ok(out)
}

/// Stage verified seed artifact bytes into the child store (content-addressed by hash).
pub fn import_seed_artifact(
    store: &ArtifactStore,
    limits: &WorkflowLimits,
    artifact: &SeedArtifactRef,
) -> Result<crate::workflow::artifacts::StagedArtifact, WorkflowError> {
    let logical_name = artifact
        .logical_name
        .clone()
        .unwrap_or_else(|| format!("seed-{}", &artifact.sha256[..12.min(artifact.sha256.len())]));
    let (kind, value) = artifact_value_from_bytes(&artifact.bytes, artifact.media_type.as_deref())?;
    let staged = store.stage(
        limits,
        &logical_name,
        kind,
        &value,
        artifact.media_type.as_deref(),
    )?;
    if staged.sha256 != artifact.sha256 {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::ArtifactCorrupt,
            format!(
                "seed artifact hash changed on import: expected {}, got {}",
                artifact.sha256, staged.sha256
            ),
        ));
    }
    if staged.byte_len != artifact.byte_len {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::ArtifactCorrupt,
            "seed artifact byte length changed on import",
        ));
    }
    Ok(staged)
}

fn artifact_value_from_bytes(
    bytes: &[u8],
    media_type: Option<&str>,
) -> Result<(ArtifactKind, ArtifactValue), WorkflowError> {
    let prefer_json = media_type.is_some_and(|m| m.contains("json"));
    if prefer_json || serde_json::from_slice::<serde_json::Value>(bytes).is_ok() {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) {
            return Ok((ArtifactKind::Json, ArtifactValue::Json(value)));
        }
    }
    let text = std::str::from_utf8(bytes).map_err(|e| {
        WorkflowError::coded(
            WorkflowErrorCode::ArtifactCorrupt,
            format!("seed artifact is not UTF-8 text: {e}"),
        )
    })?;
    Ok((ArtifactKind::Text, ArtifactValue::Text(text.to_owned())))
}

/// Split usage so seed finishes never charge the new run's actual totals.
#[must_use]
pub fn split_usage_for_seed(
    envelopes: &[JournalEnvelope],
    seed_invocation_ids: &HashSet<String>,
) -> (Option<AgentTokenUsage>, Option<AgentTokenUsage>) {
    let mut inherited = None;
    let mut actual = None;
    for envelope in envelopes {
        match &envelope.payload {
            JournalPayload::InvocationFinished {
                invocation_id,
                outcome:
                    WorkflowInvocationOutcome {
                        actual_usage: Some(usage),
                        ..
                    },
            } => {
                if seed_invocation_ids.contains(invocation_id) {
                    inherited = Some(add_usage(inherited, *usage));
                } else {
                    actual = Some(add_usage(actual, *usage));
                }
            }
            JournalPayload::UsageRecorded {
                usage,
                invocation_id,
            } => {
                let is_seed = invocation_id
                    .as_ref()
                    .is_some_and(|id| seed_invocation_ids.contains(id));
                if is_seed {
                    inherited = Some(add_usage(inherited, *usage));
                } else {
                    actual = Some(add_usage(actual, *usage));
                }
            }
            _ => {}
        }
    }
    (inherited, actual)
}

/// Invocation IDs belonging to the contiguous seed block after `LineageSeedImported`.
#[must_use]
pub fn seed_invocation_ids_from_journal(envelopes: &[JournalEnvelope]) -> HashSet<String> {
    let mut ids = HashSet::new();
    let mut after_seed = false;
    for envelope in envelopes {
        match &envelope.payload {
            JournalPayload::LineageSeedImported { .. } => {
                after_seed = true;
            }
            JournalPayload::InvocationStarted { invocation_id, .. }
            | JournalPayload::InvocationFinished { invocation_id, .. }
                if after_seed =>
            {
                ids.insert(invocation_id.clone());
            }
            JournalPayload::ArtifactCommitted { .. } if after_seed => {}
            _ if after_seed => {
                break;
            }
            _ => {}
        }
    }
    ids
}

/// Number of completed seed host-call pairs after `LineageSeedImported`.
#[must_use]
pub fn seed_pair_count_from_journal(envelopes: &[JournalEnvelope]) -> usize {
    let mut after_seed = false;
    let mut started = HashSet::new();
    let mut finished = HashSet::new();
    for envelope in envelopes {
        match &envelope.payload {
            JournalPayload::LineageSeedImported { .. } => after_seed = true,
            JournalPayload::InvocationStarted { invocation_id, .. } if after_seed => {
                started.insert(invocation_id.clone());
            }
            JournalPayload::InvocationFinished { invocation_id, .. } if after_seed => {
                finished.insert(invocation_id.clone());
            }
            JournalPayload::ArtifactCommitted { .. } if after_seed => {}
            _ if after_seed => break,
            _ => {}
        }
    }
    started.intersection(&finished).count()
}
