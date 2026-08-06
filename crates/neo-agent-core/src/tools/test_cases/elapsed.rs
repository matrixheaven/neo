use super::*;
use crate::workflow::WorkflowId;
use crate::workflow::WorkflowSnapshot;
use crate::workflow::WorkflowState;

#[tokio::test]
async fn delegate_and_swarm_elapsed_use_frozen_persisted_timestamps_after_reregistration() {
    use crate::multi_agent::{
        AgentPathKind, AgentRole, AgentRunMode, DelegateContext, MultiAgentRuntime, SwarmAggregate,
        SwarmChildSnapshot, SwarmSnapshot,
    };

    let runtime = MultiAgentRuntime::new();
    let mut agent = runtime.start_delegate(
        "review",
        None,
        AgentRole::Reviewer,
        AgentRunMode::Background,
        DelegateContext::None,
        AgentPathKind::Root,
    );
    agent.state = crate::multi_agent::AgentLifecycleState::Completed;
    agent.started_at_ms = Some(100);
    agent.terminal_at_ms = Some(650);
    agent.updated_at_ms = 650;

    let manager = BackgroundTaskManager::new();
    let task_id = manager.start_delegate(agent.clone()).await;
    assert_eq!(
        manager
            .snapshot(&task_id)
            .await
            .expect("delegate snapshot")
            .elapsed,
        Duration::from_millis(550)
    );
    manager.start_delegate(agent.clone()).await;
    assert_eq!(
        manager
            .snapshot(&task_id)
            .await
            .expect("delegate reload")
            .elapsed,
        Duration::from_millis(550)
    );

    let swarm = SwarmSnapshot {
        swarm_id: "swarm-elapsed".to_owned(),
        description: "review swarm".to_owned(),
        role: AgentRole::Reviewer,
        mode: AgentRunMode::Background,
        state: crate::multi_agent::AgentLifecycleState::Completed,
        max_concurrency: 1,
        aggregate: SwarmAggregate::from_states([agent.state]),
        children: vec![SwarmChildSnapshot {
            item_index: 0,
            item: "review".to_owned(),
            agent,
        }],
    };
    let swarm_id = manager.start_delegate_swarm(swarm.clone()).await;
    assert_eq!(
        manager
            .snapshot(&swarm_id)
            .await
            .expect("swarm snapshot")
            .elapsed,
        Duration::from_millis(550)
    );
    manager.start_delegate_swarm(swarm).await;
    assert_eq!(
        manager
            .snapshot(&swarm_id)
            .await
            .expect("swarm reload")
            .elapsed,
        Duration::from_millis(550)
    );
}

#[tokio::test]
async fn terminal_elapsed_freezes_for_completed_background_tasks() {
    let manager = BackgroundTaskManager::new();
    manager
        .start_question("elapsed-question".to_owned(), "Pick one".to_owned())
        .await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    manager
        .complete_question("elapsed-question", vec!["answer".to_owned()])
        .await;
    let first = manager
        .snapshot("elapsed-question")
        .await
        .expect("completed snapshot")
        .elapsed;
    tokio::time::sleep(Duration::from_millis(10)).await;
    let second = manager
        .snapshot("elapsed-question")
        .await
        .expect("completed snapshot")
        .elapsed;

    assert_eq!(first, second);
}

#[test]
fn terminal_workflow_elapsed_uses_its_durable_timestamps() {
    let snapshot = WorkflowSnapshot {
        id: WorkflowId("workflow-elapsed".to_owned()),
        title: "workflow elapsed".to_owned(),
        state: WorkflowState::Completed,
        current_phase: None,
        projection_sequence: None,
        recovery_failure: false,
        started_at_ms: Some(100),
        updated_at_ms: Some(650),
        invocation_count: 0,
        failure_count: 0,
        actual_usage: None,
        latest_log_summary: None,
        latest_report_summary: None,
        terminal_reason: None,
        display_name: "workflow elapsed".to_owned(),
        purpose: "test".to_owned(),
    };

    assert_eq!(workflow_elapsed(&snapshot), Duration::from_millis(550));
}
