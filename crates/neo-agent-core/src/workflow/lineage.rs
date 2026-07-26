//! Verified linked-run checkpoints and lineage seed import (design §34).
//!
//! Linked creation never depends on mutable parent files after seed import:
//! the completed invocation prefix and referenced artifact bytes are copied
//! into the new run's journal/artifact store under verified hashes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use neo_ai::ModelSpec;
use sha2::{Digest, Sha256};

use crate::AgentMessage;
use crate::AgentTokenUsage;
use crate::PermissionMode;
use crate::instructions::InstructionInheritance;
use crate::multi_agent::{ChildPlan, ChildWorktreePolicy, DelegateContext};
use crate::tools::ToolRegistry;
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
use crate::worktree::{
    IsolatedWorktree, WorktreeError, WorktreeLifecycleState, WorktreeManager,
    path_is_portable_components,
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

// ---------------------------------------------------------------------------
// Per-child isolation and capability ceiling (design §32 / Task 17)
// ---------------------------------------------------------------------------

/// Maximum characters for a host-generated child context summary.
pub const CHILD_CONTEXT_SUMMARY_MAX_CHARS: usize = 2_048;

/// Parent authority snapshot used when resolving a child's ceilings.
pub struct ParentChildAuthority {
    pub permission_mode: PermissionMode,
    pub model: ModelSpec,
    /// Registered model aliases (`alias` → resolved [`ModelSpec`]).
    pub model_aliases: BTreeMap<String, ModelSpec>,
    /// Registered provider ids allowed as overrides.
    pub provider_ids: HashSet<String>,
    /// Parent-available tools (already role/session filtered as appropriate).
    pub tools: ToolRegistry,
    /// Canonical workspace the parent is running in.
    pub workspace_root: PathBuf,
    /// Parent messages used for inherit/summary context materialization.
    pub parent_messages: Vec<AgentMessage>,
}

/// Explicit child isolation request lowered from a [`ChildPlan`] or neo.delegate.
#[derive(Debug, Clone, PartialEq)]
pub struct ChildIsolationRequest {
    pub item_id: String,
    pub context: DelegateContext,
    pub worktree: ChildWorktreePolicy,
    pub tool_allow: Option<Vec<String>>,
    pub model: Option<String>,
    pub provider: Option<String>,
    /// Optional child-requested permission; must not exceed parent.
    pub permission_mode: Option<PermissionMode>,
}

impl ChildIsolationRequest {
    /// Build from a canonical [`ChildPlan`] (no child permission field on plan).
    #[must_use]
    pub fn from_child_plan(plan: &ChildPlan) -> Self {
        Self {
            item_id: plan.item_id.clone(),
            context: plan.context,
            worktree: plan.worktree,
            tool_allow: plan.tool_allow.clone(),
            model: plan.model.clone(),
            provider: plan.provider.clone(),
            permission_mode: None,
        }
    }
}

/// Resolved context policy — maps to existing instruction/context owners only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedChildContext {
    pub mode: DelegateContext,
    pub instruction_inheritance: InstructionInheritance,
    /// Host-generated bounded summary for `summary` mode; never arbitrary hidden prompts.
    pub host_summary: Option<String>,
}

/// Shared or isolated worktree binding recorded in child provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedWorktreeBinding {
    Shared { workspace_root: PathBuf },
    Isolated { handle: IsolatedWorktree },
}

impl ResolvedWorktreeBinding {
    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        match self {
            Self::Shared { workspace_root } => workspace_root.as_path(),
            Self::Isolated { handle } => handle.path.as_path(),
        }
    }

    #[must_use]
    pub fn policy(&self) -> ChildWorktreePolicy {
        match self {
            Self::Shared { .. } => ChildWorktreePolicy::Shared,
            Self::Isolated { .. } => ChildWorktreePolicy::Isolated,
        }
    }

    #[must_use]
    pub fn is_portable(&self) -> bool {
        path_is_portable_components(self.workspace_root())
    }
}

/// Fully resolved child start binding. Failures occur before any child agent starts.
#[derive(Debug, Clone)]
pub struct ResolvedChildIsolation {
    pub context: ResolvedChildContext,
    pub worktree: ResolvedWorktreeBinding,
    pub permission_mode: PermissionMode,
    pub model: ModelSpec,
    pub effective_tool_names: Vec<String>,
    pub tool_allow: Option<Vec<String>>,
}

/// Rank permission modes so a child cannot escalate Ask → Auto/Yolo.
#[must_use]
pub const fn permission_rank(mode: PermissionMode) -> u8 {
    match mode {
        PermissionMode::Ask => 0,
        PermissionMode::Auto => 1,
        PermissionMode::Yolo => 2,
    }
}

/// Clamp or reject a child permission request against the parent ceiling.
///
/// Escalation is rejected with [`WorkflowErrorCode::PermissionDenied`].
/// When `requested` is `None`, the parent mode is inherited.
pub fn resolve_child_permission(
    parent: PermissionMode,
    requested: Option<PermissionMode>,
) -> Result<PermissionMode, WorkflowError> {
    let Some(child) = requested else {
        return Ok(parent);
    };
    if permission_rank(child) > permission_rank(parent) {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::PermissionDenied,
            format!(
                "child permission mode {} escalates beyond parent {}",
                child.label(),
                parent.label()
            ),
        ));
    }
    Ok(child)
}

/// Resolve model/provider aliases through a provided catalog (canonical registries).
///
/// Missing aliases fail explicitly — there is no silent parent fallback once a
/// child supplies a model or provider override.
pub fn resolve_child_model(
    parent: &ParentChildAuthority,
    model_alias: Option<&str>,
    provider_override: Option<&str>,
) -> Result<ModelSpec, WorkflowError> {
    let mut resolved = if let Some(alias) = model_alias.map(str::trim).filter(|s| !s.is_empty()) {
        parent.model_aliases.get(alias).cloned().ok_or_else(|| {
            WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                format!("unknown model alias `{alias}`"),
            )
        })?
    } else {
        parent.model.clone()
    };

    if let Some(provider) = provider_override.map(str::trim).filter(|s| !s.is_empty()) {
        if !parent.provider_ids.contains(provider) {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                format!("unknown provider override `{provider}`"),
            ));
        }
        // Compatible override: provider id must match a registered provider.
        // Model id stays as resolved; provider field is rewritten to the override.
        resolved.provider = neo_ai::ProviderId(provider.to_owned());
    }
    Ok(resolved)
}

/// Map context mode onto existing instruction inheritance + optional host summary.
///
/// Does not inject arbitrary hidden system prompts outside instruction ownership.
#[must_use]
pub fn resolve_child_context(
    mode: DelegateContext,
    parent_messages: &[AgentMessage],
) -> ResolvedChildContext {
    match mode {
        DelegateContext::Inherit => ResolvedChildContext {
            mode,
            instruction_inheritance: InstructionInheritance::FullContext,
            host_summary: None,
        },
        DelegateContext::Summary => ResolvedChildContext {
            mode,
            instruction_inheritance: InstructionInheritance::Summary,
            host_summary: Some(host_bounded_context_summary(parent_messages)),
        },
        DelegateContext::None => ResolvedChildContext {
            mode,
            instruction_inheritance: InstructionInheritance::Summary,
            host_summary: None,
        },
    }
}

/// Host-generated bounded summary from parent messages (existing context owner surface).
#[must_use]
pub fn host_bounded_context_summary(messages: &[AgentMessage]) -> String {
    let mut parts = Vec::new();
    for message in messages {
        let text = message.text();
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            parts.push(trimmed.to_owned());
        }
    }
    let joined = parts.join("\n");
    let mut out: String = joined
        .chars()
        .map(|ch| if ch.is_whitespace() { ' ' } else { ch })
        .take(CHILD_CONTEXT_SUMMARY_MAX_CHARS)
        .collect();
    if joined.chars().count() > CHILD_CONTEXT_SUMMARY_MAX_CHARS {
        out.push('…');
    }
    out.trim().to_owned()
}

/// Intersect parent tools with optional child `tool_allow` ceiling (exact names).
#[must_use]
pub fn resolve_child_tool_ceiling(
    parent_tools: &ToolRegistry,
    tool_allow: Option<&[String]>,
) -> ToolRegistry {
    parent_tools.for_workflow_child(tool_allow)
}

fn map_worktree_error(error: WorktreeError) -> WorkflowError {
    match error {
        WorktreeError::Unsupported { message } => {
            WorkflowError::coded(WorkflowErrorCode::InvalidInput, message)
        }
        WorktreeError::CreateFailed { message }
        | WorktreeError::CleanupFailed { message }
        | WorktreeError::Io { message } => WorkflowError::coded(WorkflowErrorCode::Host, message),
        WorktreeError::CleanupRefused { message } => {
            WorkflowError::coded(WorkflowErrorCode::InvalidOperation, message)
        }
    }
}

/// Resolve worktree policy. For `isolated`, fails before creation when unsupported.
///
/// Shared paths record the parent workspace. Isolated paths go through
/// [`WorktreeManager`] only — never ad-hoc shell strings. No auto-merge.
pub fn resolve_child_worktree(
    policy: ChildWorktreePolicy,
    parent_workspace: &Path,
    child_key: &str,
    manager: Option<&WorktreeManager>,
) -> Result<ResolvedWorktreeBinding, WorkflowError> {
    match policy {
        ChildWorktreePolicy::Shared => Ok(ResolvedWorktreeBinding::Shared {
            workspace_root: parent_workspace.to_path_buf(),
        }),
        ChildWorktreePolicy::Isolated => {
            let manager = manager.ok_or_else(|| {
                WorkflowError::coded(
                    WorkflowErrorCode::InvalidInput,
                    "isolated worktree unsupported: no worktree manager is configured",
                )
            })?;
            // Fail before child start when isolation is unsupported.
            manager
                .ensure_isolation_supported(parent_workspace)
                .map_err(map_worktree_error)?;
            let handle = manager
                .create_isolated(parent_workspace, child_key)
                .map_err(map_worktree_error)?;
            if !path_is_portable_components(&handle.path) {
                return Err(WorkflowError::coded(
                    WorkflowErrorCode::Host,
                    format!(
                        "isolated worktree path is not portable: {}",
                        handle.path.display()
                    ),
                ));
            }
            Ok(ResolvedWorktreeBinding::Isolated {
                handle: handle.mark_active(),
            })
        }
    }
}

/// Full pre-start resolution: context, model/provider, permission, tools, worktree.
///
/// On any failure no isolated worktree is left behind from this call when the
/// worktree step is what failed; earlier steps have no external effects.
pub fn resolve_child_isolation(
    parent: &ParentChildAuthority,
    request: &ChildIsolationRequest,
    worktree_manager: Option<&WorktreeManager>,
) -> Result<ResolvedChildIsolation, WorkflowError> {
    let context = resolve_child_context(request.context, &parent.parent_messages);
    let permission_mode =
        resolve_child_permission(parent.permission_mode, request.permission_mode)?;
    let model = resolve_child_model(
        parent,
        request.model.as_deref(),
        request.provider.as_deref(),
    )?;
    let tools = resolve_child_tool_ceiling(&parent.tools, request.tool_allow.as_deref());
    // Worktree last among policy checks that can create external state: still
    // runs before any child agent start, and fails closed on unsupported repos.
    let worktree = resolve_child_worktree(
        request.worktree,
        &parent.workspace_root,
        &request.item_id,
        worktree_manager,
    )?;
    Ok(ResolvedChildIsolation {
        context,
        worktree,
        permission_mode,
        model,
        effective_tool_names: tools.names(),
        tool_allow: request.tool_allow.clone(),
    })
}

/// Explicit cleanup helper for isolated bindings (never auto-invoked).
pub fn cleanup_isolated_worktree(
    manager: &WorktreeManager,
    binding: &mut ResolvedWorktreeBinding,
) -> Result<(), WorkflowError> {
    match binding {
        ResolvedWorktreeBinding::Shared { .. } => Ok(()),
        ResolvedWorktreeBinding::Isolated { handle } => {
            manager.cleanup_explicit(handle).map_err(map_worktree_error)
        }
    }
}

/// Provenance snapshot suitable for journal/details (no hidden authority).
#[must_use]
pub fn child_isolation_provenance(resolved: &ResolvedChildIsolation) -> serde_json::Value {
    let worktree = match &resolved.worktree {
        ResolvedWorktreeBinding::Shared { workspace_root } => serde_json::json!({
            "policy": "shared",
            "path": workspace_root.display().to_string(),
            "cleanup": "n/a",
        }),
        ResolvedWorktreeBinding::Isolated { handle } => serde_json::json!({
            "policy": "isolated",
            "path": handle.path.display().to_string(),
            "source": handle.source_workspace.display().to_string(),
            "state": match handle.state {
                WorktreeLifecycleState::Created => "created",
                WorktreeLifecycleState::Active => "active",
                WorktreeLifecycleState::Cleaned => "cleaned",
                WorktreeLifecycleState::DirtyRefusedCleanup => "dirty_refused_cleanup",
            },
            "dirty": handle.dirty,
            "cleanup": "explicit_only",
            "auto_merge": false,
        }),
    };
    serde_json::json!({
        "context_mode": resolved.context.mode.as_str(),
        "instruction_inheritance": match resolved.context.instruction_inheritance {
            InstructionInheritance::FullContext => "full_context",
            InstructionInheritance::Summary => "summary",
        },
        "host_summary_present": resolved.context.host_summary.is_some(),
        "permission_mode": resolved.permission_mode.label(),
        "model": {
            "provider": resolved.model.provider.0,
            "model": resolved.model.model,
        },
        "tool_allow": resolved.tool_allow,
        "effective_tools": resolved.effective_tool_names,
        "worktree": worktree,
    })
}
