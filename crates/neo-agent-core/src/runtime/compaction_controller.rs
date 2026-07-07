use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::context_budget::ContextBudgetSnapshot;
use crate::compaction::projection::ProjectionMode;
use crate::events::CompactionReason;

pub struct CompactionController;

impl CompactionController {
    #[must_use]
    pub fn decide_before_model_call(
        snapshot: ContextBudgetSnapshot,
        pending_debt: Option<&DeferredCompaction>,
        manual_requested: bool,
    ) -> CompactionDecision {
        if let Some(debt) = pending_debt {
            return CompactionDecision::RunFullCompaction {
                snapshot,
                reason: debt.reason,
                urgency: debt.urgency,
            };
        }

        if manual_requested {
            return CompactionDecision::RunFullCompaction {
                snapshot,
                reason: CompactionReason::Manual,
                urgency: CompactionUrgency::Normal,
            };
        }

        if crosses_effective_max(&snapshot)
            || crosses_absolute_max(&snapshot)
            || crosses_reserved_headroom(&snapshot)
            || crosses_trigger(&snapshot)
        {
            return CompactionDecision::RunFullCompaction {
                snapshot,
                reason: CompactionReason::Threshold,
                urgency: CompactionUrgency::Normal,
            };
        }

        if snapshot.projection.enabled && snapshot.projection.mode != ProjectionMode::None {
            let plan = snapshot.projection;
            return CompactionDecision::UseProjectionOnly { snapshot, plan };
        }

        CompactionDecision::NoAction { snapshot }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum CompactionDecision {
    NoAction {
        snapshot: ContextBudgetSnapshot,
    },
    UseProjectionOnly {
        snapshot: ContextBudgetSnapshot,
        plan: crate::compaction::projection::ProjectionPlan,
    },
    RunFullCompaction {
        snapshot: ContextBudgetSnapshot,
        reason: CompactionReason,
        urgency: CompactionUrgency,
    },
    ForceAfterOverflow {
        snapshot: ContextBudgetSnapshot,
        observed_limit: usize,
    },
    StopWithContextError {
        snapshot: ContextBudgetSnapshot,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum CompactionUrgency {
    Normal,
    DeferredAfterToolGroup,
    UrgentBeforeNextModelCall,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredCompaction {
    pub reason: CompactionReason,
    pub urgency: CompactionUrgency,
    pub first_triggered_after_call_index: usize,
    pub projected_tokens_at_trigger: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolGroupBudgetState {
    pub turn: u32,
    pub total_calls: usize,
    pub completed_calls: usize,
    pub latest_snapshot: ContextBudgetSnapshot,
    pub deferred_compaction: Option<DeferredCompaction>,
}

impl ToolGroupBudgetState {
    #[must_use]
    pub const fn new(
        turn: u32,
        total_calls: usize,
        latest_snapshot: ContextBudgetSnapshot,
    ) -> Self {
        Self {
            turn,
            total_calls,
            completed_calls: 0,
            latest_snapshot,
            deferred_compaction: None,
        }
    }

    pub fn observe_completed_result(
        &mut self,
        call_index: usize,
        snapshot: ContextBudgetSnapshot,
    ) -> Option<DeferredCompaction> {
        self.completed_calls = self.completed_calls.saturating_add(1).min(self.total_calls);
        self.latest_snapshot = snapshot;
        let urgency = if crosses_effective_max(&self.latest_snapshot) {
            Some(CompactionUrgency::UrgentBeforeNextModelCall)
        } else if crosses_absolute_max(&self.latest_snapshot)
            || crosses_reserved_headroom(&self.latest_snapshot)
            || crosses_trigger(&self.latest_snapshot)
        {
            Some(CompactionUrgency::DeferredAfterToolGroup)
        } else {
            None
        };

        if let Some(urgency) = urgency {
            let debt = match &self.deferred_compaction {
                Some(existing)
                    if existing.urgency == CompactionUrgency::UrgentBeforeNextModelCall =>
                {
                    existing.clone()
                }
                Some(existing) if urgency == CompactionUrgency::DeferredAfterToolGroup => {
                    existing.clone()
                }
                _ => DeferredCompaction {
                    reason: CompactionReason::Threshold,
                    urgency,
                    first_triggered_after_call_index: call_index,
                    projected_tokens_at_trigger: self.latest_snapshot.projected_tokens,
                },
            };
            self.deferred_compaction = Some(debt.clone());
            return Some(debt);
        }

        self.deferred_compaction.clone()
    }
}

fn crosses_effective_max(snapshot: &ContextBudgetSnapshot) -> bool {
    snapshot
        .effective_max_context_tokens
        .is_some_and(|max| snapshot.projected_tokens >= max)
}

fn crosses_absolute_max(snapshot: &ContextBudgetSnapshot) -> bool {
    snapshot
        .absolute_max_tokens
        .is_some_and(|max| snapshot.projected_tokens >= max)
}

fn crosses_reserved_headroom(snapshot: &ContextBudgetSnapshot) -> bool {
    snapshot
        .effective_max_context_tokens
        .is_some_and(|max| snapshot.projected_tokens + snapshot.reserved_headroom_tokens >= max)
}

fn crosses_trigger(snapshot: &ContextBudgetSnapshot) -> bool {
    snapshot
        .trigger_tokens
        .is_some_and(|trigger| snapshot.projected_tokens >= trigger)
}

#[cfg(test)]
mod tests {
    use crate::compaction::projection::ProjectionPlan;
    use crate::events::{CompactionReason, ContextWindowSource};
    use crate::runtime::context_budget::ContextBudgetSnapshot;

    use super::*;

    fn test_snapshot(
        projected_tokens: usize,
        effective_max_context_tokens: Option<usize>,
        trigger_tokens: Option<usize>,
    ) -> ContextBudgetSnapshot {
        ContextBudgetSnapshot {
            turn: 1,
            durable_tokens: projected_tokens,
            fixed_overhead_tokens: 0,
            tool_schema_tokens: 0,
            raw_effective_tokens: projected_tokens,
            projected_tokens,
            max_context_tokens: effective_max_context_tokens,
            effective_max_context_tokens,
            absolute_max_tokens: None,
            trigger_tokens,
            reserved_headroom_tokens: 0,
            remaining_to_trigger: trigger_tokens
                .map(|tokens| tokens.saturating_sub(projected_tokens)),
            remaining_to_max: effective_max_context_tokens
                .map(|tokens| tokens.saturating_sub(projected_tokens)),
            source: ContextWindowSource::Configured,
            projection: ProjectionPlan::disabled(),
        }
    }

    #[test]
    fn decision_no_action_below_threshold() {
        let snapshot = test_snapshot(10_000, Some(100_000), Some(80_000));
        let decision = CompactionController::decide_before_model_call(snapshot, None, false);
        assert!(matches!(decision, CompactionDecision::NoAction { .. }));
    }

    #[test]
    fn decision_runs_full_compaction_at_ratio_threshold() {
        let snapshot = test_snapshot(80_000, Some(100_000), Some(80_000));
        let decision = CompactionController::decide_before_model_call(snapshot, None, false);
        assert!(matches!(
            decision,
            CompactionDecision::RunFullCompaction {
                reason: CompactionReason::Threshold,
                urgency: CompactionUrgency::Normal,
                ..
            }
        ));
    }

    #[test]
    fn decision_uses_deferred_tool_group_debt_before_next_model_call() {
        let snapshot = test_snapshot(10_000, Some(100_000), Some(80_000));
        let debt = DeferredCompaction {
            reason: CompactionReason::Threshold,
            urgency: CompactionUrgency::DeferredAfterToolGroup,
            first_triggered_after_call_index: 1,
            projected_tokens_at_trigger: 90_000,
        };

        let decision = CompactionController::decide_before_model_call(snapshot, Some(&debt), false);

        assert!(matches!(
            decision,
            CompactionDecision::RunFullCompaction {
                urgency: CompactionUrgency::DeferredAfterToolGroup,
                ..
            }
        ));
    }

    #[test]
    fn decision_runs_full_compaction_at_reserved_headroom() {
        let mut snapshot = test_snapshot(75_000, Some(100_000), Some(80_000));
        snapshot.reserved_headroom_tokens = 25_000;

        let decision = CompactionController::decide_before_model_call(snapshot, None, false);

        assert!(matches!(
            decision,
            CompactionDecision::RunFullCompaction {
                reason: CompactionReason::Threshold,
                urgency: CompactionUrgency::Normal,
                ..
            }
        ));
    }

    #[test]
    fn tool_group_records_deferred_debt_after_first_result_crosses_threshold() {
        let initial = test_snapshot(40_000, Some(100_000), Some(80_000));
        let mut group = ToolGroupBudgetState::new(7, 3, initial);
        let crossed = test_snapshot(82_000, Some(100_000), Some(80_000));

        let debt = group.observe_completed_result(0, crossed).expect("debt");

        assert_eq!(debt.first_triggered_after_call_index, 0);
        assert_eq!(group.completed_calls, 1);
        assert!(group.deferred_compaction.is_some());
    }
}
