//! Context bridge between the session instruction registry and the runtime
//! request path.
//!
//! Owns the dynamic instruction budget, the pre-compaction admission
//! decision for pending epochs, epoch application, and byte-exact
//! rehydration of pinned instruction content after full compaction. The
//! bridge is stateless: every method derives from the current config and
//! context, so the turn loop can call it before each provider request.

use super::config::AgentConfig;
use super::context::AgentContext;
use super::context_budget::{ContextBudgetEstimator, ContextBudgetSnapshot};
use super::{estimate_message_tokens, estimate_messages_tokens};
use crate::compaction::projection::ProjectionPlan;
use crate::instructions::{
    InstructionBudget, InstructionEpochData, InstructionError, InstructionFingerprint,
    InstructionRegistry, InstructionScopeKind,
};
use crate::{AgentMessage, Content};

/// How to admit one pending instruction epoch without ever injecting a
/// large epoch and immediately summarizing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingEpochAdmission {
    /// The pending epoch fits the current request safely; apply it now.
    Admit,
    /// The pending epoch would cross the existing compaction thresholds:
    /// run full compaction of ordinary history first, rehydrate the current
    /// instruction chain, then re-snapshot and admit. If capacity still
    /// does not fit, admission falls back to deterministic whole-bundle
    /// omission or the typed context-overflow error.
    CompactFirst,
}

/// Stateless bridge between the session instruction registry and per-agent
/// request context.
pub struct InstructionContextBridge;

impl InstructionContextBridge {
    /// Dynamic instruction budget for the next request: nominal
    /// `max(65_536, effective_max_tokens / 8)` where the effective window
    /// includes the observed-overflow correction, clamped to the safely
    /// available request capacity (fixed overhead, tool schemas, reserved
    /// output headroom, and current ordinary context).
    #[must_use]
    pub fn budget(config: &AgentConfig, context: &AgentContext) -> InstructionBudget {
        let snapshot = Self::snapshot(config, context);
        Self::budget_from_snapshot(&snapshot)
    }

    /// Budget from an already-computed request snapshot.
    #[must_use]
    pub(crate) fn budget_from_snapshot(snapshot: &ContextBudgetSnapshot) -> InstructionBudget {
        let effective_max = snapshot
            .effective_max_context_tokens
            .map(|tokens| u64::try_from(tokens).unwrap_or(u64::MAX));
        let safe = snapshot
            .safely_available_instruction_tokens()
            .unwrap_or(u64::MAX);
        InstructionBudget::from_context(effective_max, safe)
    }

    /// Decides how to admit one pending epoch. Pinned instruction content is
    /// projection-immune, so the decision uses the raw (unprojected) request
    /// size. When the epoch would cross the existing compaction thresholds
    /// (trigger ratio, reserved headroom, effective or absolute cap), the
    /// caller must compact ordinary history FIRST and admit afterwards —
    /// never inject a large epoch and immediately summarize it.
    #[must_use]
    pub fn prepare_pending_epoch(
        config: &AgentConfig,
        context: &AgentContext,
        epoch: &InstructionEpochData,
    ) -> PendingEpochAdmission {
        let snapshot = Self::snapshot(config, context);
        let pending_tokens = epoch.model_content.as_deref().map_or(0, |content| {
            estimate_message_tokens(&AgentMessage::Instruction {
                generation: epoch.generation,
                content: vec![Content::text(content)],
            })
        });
        if pending_tokens == 0 {
            return PendingEpochAdmission::Admit;
        }
        let projected = snapshot.projected_tokens.saturating_add(pending_tokens);
        let crosses_trigger = snapshot
            .trigger_tokens
            .is_some_and(|trigger| projected >= trigger);
        let crosses_effective = snapshot
            .effective_max_context_tokens
            .is_some_and(|max| projected >= max);
        let crosses_absolute = snapshot
            .absolute_max_tokens
            .is_some_and(|max| projected >= max);
        let crosses_reserved = snapshot
            .effective_max_context_tokens
            .is_some_and(|max| projected.saturating_add(snapshot.reserved_headroom_tokens) >= max);
        if crosses_trigger || crosses_effective || crosses_absolute || crosses_reserved {
            PendingEpochAdmission::CompactFirst
        } else {
            PendingEpochAdmission::Admit
        }
    }

    /// Applies one admitted epoch: pins its model content as an
    /// [`AgentMessage::Instruction`], updates agent-local visibility, and
    /// records the decision fingerprint so an unchanged re-probe proceeds
    /// silently.
    ///
    /// Callers that route the epoch through an `EventEmitter` as
    /// `AgentEvent::InstructionEpoch` must NOT also call this (the emitter
    /// performs the same conversion); they record the fingerprint via
    /// [`AgentContext::instruction_state_mut`] instead.
    pub fn apply_epoch(
        context: &mut AgentContext,
        epoch: &InstructionEpochData,
        fingerprint: &InstructionFingerprint,
    ) {
        context.apply_instruction_epoch(epoch);
        context.instruction_state_mut().last_epoch_fingerprint = Some(fingerprint.hash.clone());
    }

    /// Rehydrates pinned instruction content after a full compaction:
    /// strips every pinned instruction message, re-pins the exact current
    /// global + initial workspace baseline + current/most-recent nested
    /// scope chain from registry state, and retains visited sibling metadata
    /// without pinning their bodies.
    ///
    /// Full compaction already establishes a new provider prefix, so exact
    /// rehydration creates no additional cache regression. The
    /// already-current scope keeps its decision fingerprint and
    /// `most_recent_scope`, so continuing in it proceeds silently (no new
    /// epoch, no card), while re-entering a visited sibling emits one
    /// `Reactivated` epoch. This is an invisible context repair: it emits no
    /// event and persists nothing beyond the agent-local state update.
    ///
    /// Returns `true` when current chain content was re-pinned.
    ///
    /// # Errors
    /// Returns the typed [`InstructionError`] when the registry cannot
    /// refresh the current chain; callers must surface it rather than
    /// rehydrate partial rules.
    pub async fn rehydrate_after_compaction(
        registry: &InstructionRegistry,
        context: &mut AgentContext,
    ) -> Result<bool, InstructionError> {
        // Drop every pinned instruction body (stale revisions included)
        // before re-pinning the exact current chain.
        context
            .messages
            .retain(|message| !matches!(message, AgentMessage::Instruction { .. }));
        context.estimated_tokens = estimate_messages_tokens(&context.messages);

        if context.instruction_state().visible_generation == 0 {
            // No instruction state was ever admitted; baseline establishment
            // owns the first epoch, not rehydration.
            return Ok(false);
        }
        let most_recent_scope = context.instruction_state().most_recent_scope.clone();
        let snapshot = registry
            .rehydration_snapshot(most_recent_scope.as_deref())
            .await?;

        let repinned = snapshot.model_content.is_some();
        if let Some(model_content) = snapshot.model_content {
            let generation = context.instruction_state().visible_generation;
            context.append_message(AgentMessage::Instruction {
                generation,
                content: vec![Content::text(model_content)],
            });
        }

        // The rehydrated baseline (global, ancestors, workspace root) stays
        // active. The nested chain is pinned for continuity and kept in
        // `most_recent_scope` with its fingerprint, but contracts out of the
        // active set so re-entering a visited sibling is a visible
        // `Reactivated` epoch. Sibling bodies stay unpinned; their revisions
        // survive in `visible_revisions` as metadata.
        let state = context.instruction_state_mut();
        state.active_scopes = snapshot
            .chain
            .iter()
            .filter(|candidate| candidate.kind != InstructionScopeKind::Nested)
            .map(|candidate| candidate.scope_dir.clone())
            .collect();
        state.visible_revisions = snapshot
            .visited
            .iter()
            .map(|metadata| (metadata.display_path.clone(), metadata.revision.clone()))
            .collect();
        Ok(repinned)
    }

    fn snapshot(config: &AgentConfig, context: &AgentContext) -> ContextBudgetSnapshot {
        ContextBudgetEstimator::snapshot(config, context, ProjectionPlan::disabled())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::fake_model;
    use crate::instructions::{
        InstructionEpochOutcome, InstructionScopeData, InstructionScopeKind,
    };
    use crate::{AgentMessage, CompactionSettings};

    fn epoch_with_content(model_content: Option<String>) -> InstructionEpochData {
        InstructionEpochData {
            agent_id: "main".to_owned(),
            generation: 3,
            outcome: InstructionEpochOutcome::Activated,
            scopes: vec![InstructionScopeData {
                display_path: std::path::PathBuf::from("/workspace"),
                kind: InstructionScopeKind::WorkspaceRoot,
                revision: Some("rev".to_owned()),
                token_estimate: 12,
            }],
            selected_bundles: Vec::new(),
            ignored_bundles: Vec::new(),
            replacements: Vec::new(),
            failure: None,
            deferred_tool_ids: Vec::new(),
            model_content,
        }
    }

    #[test]
    fn pending_epoch_crossing_threshold_compacts_before_admission() {
        let mut config = AgentConfig::for_model(fake_model()).with_compaction(CompactionSettings {
            reserved_context_tokens: 1_000,
            ..CompactionSettings::new(usize::MAX, 4)
        });
        config.model.capabilities.max_context_tokens = Some(10_000);
        let mut context = AgentContext::new();
        // ~5_000 tokens of ordinary history.
        context.append_message(AgentMessage::user_text("x".repeat(20_000)));

        // A ~4_000-token pending epoch crosses the 8_000-token small-window
        // trigger: compact ordinary history first, never summarize the epoch.
        let large = epoch_with_content(Some("y".repeat(16_000)));
        assert_eq!(
            InstructionContextBridge::prepare_pending_epoch(&config, &context, &large),
            PendingEpochAdmission::CompactFirst
        );

        // A small epoch fits below the trigger and the reserved headroom.
        let small = epoch_with_content(Some("z".repeat(400)));
        assert_eq!(
            InstructionContextBridge::prepare_pending_epoch(&config, &context, &small),
            PendingEpochAdmission::Admit
        );

        // An epoch without model content (e.g. a removal) pins nothing and
        // never triggers compaction.
        let empty = epoch_with_content(None);
        assert_eq!(
            InstructionContextBridge::prepare_pending_epoch(&config, &context, &empty),
            PendingEpochAdmission::Admit
        );
    }
}
