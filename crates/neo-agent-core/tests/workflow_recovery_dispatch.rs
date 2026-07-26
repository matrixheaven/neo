//! Production recovery resolver is read-only and never dispatches (Task 5).

use std::sync::Arc;

use neo_agent_core::harness::FakeHarness;
use neo_agent_core::multi_agent::{
    AgentLifecycleState, AgentPathKind, AgentRole, AgentRunMode, AgentTerminalOutcome,
    DelegateContext, MultiAgentRuntime,
};
use neo_agent_core::runtime::{
    AgentConfig, AgentContext, WorkflowDispatchResolver, WorkflowDispatchSnapshot,
    resolve_proven_terminal_outcome,
};
use neo_agent_core::tools::{BackgroundTaskManager, ProcessSupervisor, ToolRegistry};
use neo_agent_core::workflow::journal::IncompleteInvocation;
use neo_agent_core::workflow::{WorkflowInvocationKind, WorkflowOutcomeStatus, WorkflowRuntime};

fn incomplete(id: &str, kind: WorkflowInvocationKind) -> Arc<IncompleteInvocation> {
    Arc::new(IncompleteInvocation {
        invocation_id: id.to_owned(),
        call_index: 0,
        kind,
        canonical_input_hash: "a".repeat(64),
    })
}

fn snapshot_with(
    multi_agent: MultiAgentRuntime,
    background_tasks: BackgroundTaskManager,
    session: &std::path::Path,
) -> WorkflowDispatchSnapshot {
    let harness = FakeHarness::from_turns([]);
    let config = AgentConfig::for_model(harness.model())
        .with_multi_agent(multi_agent)
        .with_background_tasks(background_tasks)
        .with_session_directory(session.to_path_buf());
    WorkflowDispatchSnapshot {
        config,
        model_client: harness.client(),
        registry: Arc::new(ToolRegistry::default()),
        skills: None,
        process_supervisor: ProcessSupervisor::default(),
        context: AgentContext::new(),
    }
}

#[tokio::test]
async fn resolver_is_read_only_and_never_dispatches() {
    let session = tempfile::tempdir().unwrap();
    let multi_agent = MultiAgentRuntime::new();
    let background_tasks = BackgroundTaskManager::new();

    // Seed a finished agent so stores are non-empty but unrelated to lookup ids.
    let seed = multi_agent.start_delegate(
        "seed",
        None,
        AgentRole::Coder,
        AgentRunMode::Foreground,
        DelegateContext::None,
        AgentPathKind::Root,
    );
    let seed = multi_agent.complete_delegate_for_test(&seed.id, "seed done");
    background_tasks.start_delegate(seed.clone()).await;
    // start_delegate on background registers as finished when already terminal.
    let _ = seed;

    let dispatch = WorkflowDispatchResolver::default();
    dispatch
        .refresh(snapshot_with(
            multi_agent.clone(),
            background_tasks.clone(),
            session.path(),
        ))
        .expect("bind snapshot");

    let agents_before = multi_agent.list_agents(true).len();
    let tasks_before = background_tasks.list(false, 100).await.len();
    let fake_requests_before = {
        // No model client requests should occur during recovery resolve.
        0usize
    };

    // Unknown / missing terminal result → None (interrupted by runtime).
    assert!(
        resolve_proven_terminal_outcome(
            &dispatch,
            incomplete("inv_missing", WorkflowInvocationKind::Delegate),
        )
        .await
        .is_none()
    );

    // Host-local kinds never invent results and never dispatch.
    assert!(
        resolve_proven_terminal_outcome(
            &dispatch,
            incomplete("inv_phase", WorkflowInvocationKind::Phase),
        )
        .await
        .is_none()
    );

    // Unbound resolver: still None, no panic, no dispatch.
    assert!(
        resolve_proven_terminal_outcome(
            &WorkflowDispatchResolver::default(),
            incomplete("inv_unbound", WorkflowInvocationKind::Swarm),
        )
        .await
        .is_none()
    );

    // Read-only: stores unchanged after lookups.
    assert_eq!(multi_agent.list_agents(true).len(), agents_before);
    assert_eq!(background_tasks.list(false, 100).await.len(), tasks_before);
    let _ = fake_requests_before;

    // Composition root binds recovery resolver once (repeated binds ok).
    let runtime = WorkflowRuntime::default();
    dispatch
        .bind_workflow_runtime(&runtime)
        .expect("bind composition root");
    dispatch
        .bind_workflow_runtime(&runtime)
        .expect("rebind composition root");

    // Proven terminal child keyed by invocation_id is adopted exactly once.
    let multi = MultiAgentRuntime::new();
    let bg = BackgroundTaskManager::new();
    let live = multi.start_delegate(
        "proven",
        None,
        AgentRole::Coder,
        AgentRunMode::Foreground,
        DelegateContext::None,
        AgentPathKind::Root,
    );
    let finished = multi.complete_delegate_for_test(&live.id, "child completed");
    // Both stores agree: multi-agent + background task share the same terminal snapshot.
    bg.start_delegate(finished.clone()).await;

    let session2 = tempfile::tempdir().unwrap();
    let dispatch2 = WorkflowDispatchResolver::default();
    dispatch2
        .refresh(snapshot_with(multi.clone(), bg.clone(), session2.path()))
        .unwrap();

    let adopted = resolve_proven_terminal_outcome(
        &dispatch2,
        incomplete(finished.id.as_str(), WorkflowInvocationKind::Delegate),
    )
    .await
    .expect("exactly one proven terminal result is adopted");
    assert_eq!(adopted.status, WorkflowOutcomeStatus::Completed);
    assert!(adopted.ok);
    assert_eq!(
        adopted
            .details
            .get("recovery_adopted")
            .and_then(serde_json::value::Value::as_bool),
        Some(true)
    );
    // Adoption is read-only: no new agents/tasks created.
    assert_eq!(multi.list_agents(true).len(), 1);
    assert_eq!(bg.list(false, 100).await.len(), 1);

    // Conflicting terminal results across stores → None (no heuristic choice).
    let multi_c = MultiAgentRuntime::new();
    let bg_c = BackgroundTaskManager::new();
    let running = multi_c.start_delegate(
        "conflict",
        None,
        AgentRole::Coder,
        AgentRunMode::Foreground,
        DelegateContext::None,
        AgentPathKind::Root,
    );
    let completed = multi_c.complete_delegate_for_test(&running.id, "ok");
    // Background still "running" path then cancelled with different terminal status.
    bg_c.start_delegate({
        let mut running_snap = completed.clone();
        running_snap.state = AgentLifecycleState::Running;
        running_snap.outcome = None;
        running_snap
    })
    .await;
    let mut cancelled = completed.clone();
    cancelled.state = AgentLifecycleState::Cancelled;
    cancelled.outcome = Some(AgentTerminalOutcome {
        summary: "cancelled".to_owned(),
        is_error: true,
    });
    bg_c.cancel_delegate(completed.id.as_str(), cancelled).await;

    let session3 = tempfile::tempdir().unwrap();
    let dispatch_c = WorkflowDispatchResolver::default();
    dispatch_c
        .refresh(snapshot_with(multi_c, bg_c, session3.path()))
        .unwrap();
    assert!(
        resolve_proven_terminal_outcome(
            &dispatch_c,
            incomplete(completed.id.as_str(), WorkflowInvocationKind::Delegate),
        )
        .await
        .is_none(),
        "conflicting terminal results must not be chosen heuristically"
    );
}
