//! Production read-only recovery resolver (design §19).
//!
//! Bound at the workflow dispatch composition root. Queries canonical terminal
//! child/task stores only; never dispatches, waits, resumes, retries, or mutates
//! multi-agent / background-task state.

use std::sync::Arc;

use crate::multi_agent::{
    AgentId, AgentLifecycleState, AgentRunMode, AgentSnapshot, MultiAgentRuntime, SwarmSnapshot,
};
use crate::tools::BackgroundTaskManager;
use crate::workflow::journal::IncompleteInvocation;
use crate::workflow::recovery::{EffectReconciliation, reconcile_incomplete_effect};
use crate::workflow::{
    WorkflowChildRef, WorkflowInvocationKind, WorkflowInvocationOutcome, WorkflowOutcomeStatus,
};

use super::workflow_dispatch::{WorkflowDispatchResolver, WorkflowDispatchSnapshot};

/// Resolve one incomplete invocation against live terminal child/task stores.
///
/// Returns [`None`] when zero, conflicting, or unknown results are found — the
/// runtime classifies that as `interrupted(host_exit)`. Never starts children,
/// opens model turns, or writes any store.
pub async fn resolve_proven_terminal_outcome(
    resolver: &WorkflowDispatchResolver,
    invocation: Arc<IncompleteInvocation>,
) -> Option<WorkflowInvocationOutcome> {
    match reconcile_from_dispatch(resolver, invocation.as_ref()).await {
        EffectReconciliation::AdoptProven { outcome } => Some(outcome),
        EffectReconciliation::InterruptHostExit | EffectReconciliation::Conflict { .. } => None,
    }
}

async fn reconcile_from_dispatch(
    resolver: &WorkflowDispatchResolver,
    invocation: &IncompleteInvocation,
) -> EffectReconciliation {
    let snapshots = match resolver.bound_snapshots() {
        Ok(snapshots) => snapshots,
        // Unbound / poisoned: treat as unknown — interrupt, never dispatch.
        Err(_) => return EffectReconciliation::InterruptHostExit,
    };
    if snapshots.is_empty() {
        return EffectReconciliation::InterruptHostExit;
    }

    let mut proven: Vec<WorkflowInvocationOutcome> = Vec::new();
    let mut conflict = false;
    let mut conflict_detail = String::new();

    for snapshot in &snapshots {
        match lookup_in_snapshot(snapshot, invocation).await {
            LookupResult::None => {}
            LookupResult::Proven(outcome) => {
                if proven
                    .iter()
                    .any(|existing| !outcomes_equivalent(existing, &outcome))
                {
                    conflict = true;
                    conflict_detail = format!(
                        "conflicting terminal results for invocation {}",
                        invocation.invocation_id
                    );
                }
                proven.push(outcome);
            }
            LookupResult::Conflict(detail) => {
                conflict = true;
                conflict_detail = detail;
            }
        }
    }

    let single = if conflict {
        None
    } else if proven.len() == 1 {
        proven.pop()
    } else if proven.len() > 1 {
        // Multiple identical proven results still count as one decision.
        if proven
            .windows(2)
            .all(|pair| outcomes_equivalent(&pair[0], &pair[1]))
        {
            proven.pop()
        } else {
            conflict = true;
            conflict_detail = format!(
                "conflicting terminal results for invocation {}",
                invocation.invocation_id
            );
            None
        }
    } else {
        None
    };

    reconcile_incomplete_effect(single, conflict, conflict_detail)
}

enum LookupResult {
    None,
    Proven(WorkflowInvocationOutcome),
    Conflict(String),
}

async fn lookup_in_snapshot(
    snapshot: &WorkflowDispatchSnapshot,
    invocation: &IncompleteInvocation,
) -> LookupResult {
    match invocation.kind {
        WorkflowInvocationKind::Delegate => {
            lookup_delegate(
                &snapshot.config.multi_agent,
                &snapshot.config.background_tasks,
                invocation,
            )
            .await
        }
        WorkflowInvocationKind::Swarm => {
            lookup_swarm(
                &snapshot.config.multi_agent,
                &snapshot.config.background_tasks,
                invocation,
            )
            .await
        }
        // Host-local effects have no external terminal store; missing finish is
        // always host_exit interruption (never re-executed).
        WorkflowInvocationKind::Phase
        | WorkflowInvocationKind::Log
        | WorkflowInvocationKind::Verify
        | WorkflowInvocationKind::VerifyCommand
        | WorkflowInvocationKind::Report
        | WorkflowInvocationKind::Fail
        // Generic tools are not auto-retried; incomplete ends as host-exit.
        | WorkflowInvocationKind::Tool => LookupResult::None,
    }
}

async fn lookup_delegate(
    multi_agent: &MultiAgentRuntime,
    background_tasks: &BackgroundTaskManager,
    invocation: &IncompleteInvocation,
) -> LookupResult {
    let mut candidates = Vec::new();

    let agent_id = AgentId::from_existing(&invocation.invocation_id);
    if let Some(agent) = multi_agent.snapshot(&agent_id)
        && agent.state.is_terminal()
        && let Some(outcome) = agent_to_outcome(&agent)
    {
        candidates.push(outcome);
    }

    // Non-blocking enumeration only (never wait for running tasks).
    for task in background_tasks.list(false, 10_000).await {
        if task.task_id != invocation.invocation_id || task.status.is_active() {
            continue;
        }
        if let Some(agent) = task.delegate.as_ref()
            && agent.state.is_terminal()
            && let Some(outcome) = agent_to_outcome(agent)
        {
            candidates.push(outcome);
        }
    }

    collapse_candidates(candidates, &invocation.invocation_id)
}

async fn lookup_swarm(
    multi_agent: &MultiAgentRuntime,
    background_tasks: &BackgroundTaskManager,
    invocation: &IncompleteInvocation,
) -> LookupResult {
    let mut candidates = Vec::new();

    if let Some(swarm) = multi_agent.swarm_snapshot(&invocation.invocation_id)
        && swarm.state.is_terminal()
        && let Some(outcome) = swarm_to_outcome(&swarm)
    {
        candidates.push(outcome);
    }

    for task in background_tasks.list(false, 10_000).await {
        if task.task_id != invocation.invocation_id || task.status.is_active() {
            continue;
        }
        if let Some(swarm) = task.swarm.as_ref()
            && swarm.state.is_terminal()
            && let Some(outcome) = swarm_to_outcome(swarm)
        {
            candidates.push(outcome);
        }
    }

    collapse_candidates(candidates, &invocation.invocation_id)
}

fn collapse_candidates(
    candidates: Vec<WorkflowInvocationOutcome>,
    invocation_id: &str,
) -> LookupResult {
    if candidates.is_empty() {
        return LookupResult::None;
    }
    let first = &candidates[0];
    if candidates
        .iter()
        .skip(1)
        .any(|other| !outcomes_equivalent(first, other))
    {
        return LookupResult::Conflict(format!(
            "conflicting terminal store results for invocation {invocation_id}"
        ));
    }
    LookupResult::Proven(candidates.into_iter().next().expect("non-empty"))
}

fn agent_to_outcome(agent: &AgentSnapshot) -> Option<WorkflowInvocationOutcome> {
    if agent.mode == AgentRunMode::Background {
        // Background mode is nonterminal for workflow finish adoption.
        return None;
    }
    let status = lifecycle_to_outcome_status(agent.state)?;
    let summary = agent
        .outcome
        .as_ref()
        .map(|outcome| outcome.summary.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("delegate {}", agent.state.as_str()));
    Some(WorkflowInvocationOutcome {
        ok: status == WorkflowOutcomeStatus::Completed,
        status,
        summary,
        interruption: None,
        details: serde_json::json!({
            "kind": "delegate",
            "agent_id": agent.id.as_str(),
            "status": agent.state.as_str(),
            "mode": match agent.mode {
                AgentRunMode::Foreground => "foreground",
                AgentRunMode::Background => "background",
            },
            "side_effect_occurred": true,
            "recovery_adopted": true,
        }),
        actual_usage: None,
        child_refs: vec![WorkflowChildRef {
            kind: "delegate".to_owned(),
            id: agent.id.as_str().to_owned(),
        }],
    })
}

fn swarm_to_outcome(swarm: &SwarmSnapshot) -> Option<WorkflowInvocationOutcome> {
    if swarm.mode == AgentRunMode::Background {
        return None;
    }
    if swarm
        .children
        .iter()
        .any(|child| !child.agent.state.is_terminal())
    {
        return None;
    }
    let status = lifecycle_to_outcome_status(swarm.state)?;
    let mut child_refs = Vec::with_capacity(swarm.children.len() + 1);
    child_refs.push(WorkflowChildRef {
        kind: "delegate_swarm".to_owned(),
        id: swarm.swarm_id.clone(),
    });
    for child in &swarm.children {
        child_refs.push(WorkflowChildRef {
            kind: "delegate".to_owned(),
            id: child.agent.id.as_str().to_owned(),
        });
    }
    Some(WorkflowInvocationOutcome {
        ok: status == WorkflowOutcomeStatus::Completed,
        status,
        summary: format!("swarm {}", swarm.state.as_str()),
        interruption: None,
        details: serde_json::json!({
            "kind": "delegate_swarm",
            "swarm_id": swarm.swarm_id,
            "status": swarm.state.as_str(),
            "mode": match swarm.mode {
                AgentRunMode::Foreground => "foreground",
                AgentRunMode::Background => "background",
            },
            "side_effect_occurred": true,
            "recovery_adopted": true,
        }),
        actual_usage: None,
        child_refs,
    })
}

fn lifecycle_to_outcome_status(state: AgentLifecycleState) -> Option<WorkflowOutcomeStatus> {
    match state {
        AgentLifecycleState::Completed => Some(WorkflowOutcomeStatus::Completed),
        AgentLifecycleState::Failed | AgentLifecycleState::TimedOut => {
            Some(WorkflowOutcomeStatus::Failed)
        }
        AgentLifecycleState::Cancelled => Some(WorkflowOutcomeStatus::Cancelled),
        AgentLifecycleState::Interrupted => Some(WorkflowOutcomeStatus::Interrupted),
        AgentLifecycleState::Queued | AgentLifecycleState::Running => None,
    }
}

fn outcomes_equivalent(
    left: &WorkflowInvocationOutcome,
    right: &WorkflowInvocationOutcome,
) -> bool {
    left.status == right.status
        && left.ok == right.ok
        && left.child_refs == right.child_refs
        && left.summary == right.summary
}
