#![allow(clippy::duration_suboptimal_units)]
use std::time::Duration;

use neo_agent_core::harness::FakeHarness;
use neo_agent_core::multi_agent::{
    AgentDisplayName, AgentId, AgentLifecycleState, AgentPath, AgentRole, AgentRunMode,
    AgentSnapshot, SwarmAggregate,
};
use neo_agent_core::tools::{BackgroundTaskManager, ToolContext, ToolRegistry};
use neo_agent_core::{AgentConfig, PermissionMode, ToolAccess, ToolExecutionMode};
use neo_ai::{AiStreamEvent, StopReason};
use std::sync::Arc;

#[tokio::test]
async fn task_stop_cancels_delegate_runtime_and_completion_cannot_overwrite_cancelled() {
    use neo_agent_core::tools::ToolContext;
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_access(ToolAccess::all());
    let agent = ctx.multi_agent.start_delegate(
        "stop me",
        None,
        neo_agent_core::multi_agent::AgentRole::Coder,
        AgentRunMode::Background,
        neo_agent_core::multi_agent::DelegateContext::None,
        neo_agent_core::multi_agent::AgentPathKind::Root,
    );
    ctx.background_tasks.start_delegate(agent.clone()).await;

    let result = ToolRegistry::with_builtin_tools()
        .run(
            "TaskStop",
            &ctx,
            serde_json::json!({ "task_id": agent.id.as_str() }),
        )
        .await
        .expect("TaskStop should cancel delegate");

    assert!(
        result.content.contains("status: cancelled"),
        "{}",
        result.content
    );
    let runtime_snapshot = ctx
        .multi_agent
        .snapshot(&agent.id)
        .expect("agent remains tracked");
    assert_eq!(runtime_snapshot.state, AgentLifecycleState::Cancelled);

    let completed = ctx
        .multi_agent
        .complete_delegate_for_test(&agent.id, "late completion");
    ctx.background_tasks
        .complete_delegate(agent.id.as_str(), completed)
        .await;
    let stopped = ctx
        .background_tasks
        .snapshot(agent.id.as_str())
        .await
        .expect("task snapshot");
    assert_eq!(
        stopped.status,
        neo_agent_core::tools::BackgroundTaskStatus::Cancelled
    );
}

#[tokio::test]
async fn task_stop_cancels_delegate_swarm_children_and_late_completion_cannot_overwrite_cancelled()
{
    use neo_agent_core::multi_agent::{
        AgentPathKind, AgentRole, SwarmChildSnapshot, SwarmSnapshot,
    };
    use neo_agent_core::tools::{BackgroundTaskStatus, ToolContext};
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_access(ToolAccess::all());
    let swarm_id = ctx.multi_agent.new_swarm_id();
    let first = ctx.multi_agent.start_delegate(
        "check alpha",
        None,
        AgentRole::Coder,
        AgentRunMode::Background,
        neo_agent_core::multi_agent::DelegateContext::None,
        AgentPathKind::SwarmChild(&swarm_id),
    );
    let second = ctx.multi_agent.start_delegate(
        "check beta",
        None,
        AgentRole::Coder,
        AgentRunMode::Background,
        neo_agent_core::multi_agent::DelegateContext::None,
        AgentPathKind::SwarmChild(&swarm_id),
    );
    let children = vec![
        SwarmChildSnapshot {
            item_index: 0,
            item: "alpha".to_owned(),
            agent: first.clone(),
        },
        SwarmChildSnapshot {
            item_index: 1,
            item: "beta".to_owned(),
            agent: second.clone(),
        },
    ];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|c| c.agent.state));
    let swarm = SwarmSnapshot {
        swarm_id: swarm_id.clone(),
        description: "background swarm".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Background,
        state: AgentLifecycleState::Running,
        max_concurrency: 1,
        aggregate,
        children,
    };
    ctx.multi_agent.register_swarm(swarm.clone());
    ctx.background_tasks.start_delegate_swarm(swarm).await;

    let result = ToolRegistry::with_builtin_tools()
        .run("TaskStop", &ctx, serde_json::json!({ "task_id": swarm_id }))
        .await
        .expect("TaskStop should cancel delegate swarm");

    assert!(
        result.content.contains("status: cancelled"),
        "{}",
        result.content
    );
    assert_eq!(
        ctx.multi_agent.snapshot(&first.id).unwrap().state,
        AgentLifecycleState::Cancelled
    );
    assert_eq!(
        ctx.multi_agent.snapshot(&second.id).unwrap().state,
        AgentLifecycleState::Cancelled
    );

    let completed_swarm = SwarmSnapshot {
        swarm_id: swarm_id.clone(),
        description: "late completion".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Background,
        state: AgentLifecycleState::Completed,
        max_concurrency: 1,
        aggregate: SwarmAggregate::default(),
        children: Vec::new(),
    };
    ctx.background_tasks
        .complete_delegate_swarm(&swarm_id, completed_swarm)
        .await;
    let stopped = ctx
        .background_tasks
        .snapshot(&swarm_id)
        .await
        .expect("swarm task snapshot");
    assert_eq!(stopped.status, BackgroundTaskStatus::Cancelled);
}

#[tokio::test]
async fn task_stop_completed_delegate_returns_already_completed_error() {
    let (registry, ctx) = registry_with_multi_agent();
    let delegate = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "return exactly finished",
                "mode": "background"
            }),
        )
        .await
        .expect("background delegate should start");
    let agent_id = delegate
        .details
        .as_ref()
        .and_then(|details| details.get("agent_id"))
        .and_then(serde_json::Value::as_str)
        .expect("delegate result should include agent_id")
        .to_owned();

    registry
        .run(
            "WaitDelegate",
            &ctx,
            serde_json::json!({ "ids": [agent_id], "timeout_ms": 5000 }),
        )
        .await
        .expect("delegate should complete");

    let stopped = registry
        .run("TaskStop", &ctx, serde_json::json!({ "task_id": agent_id }))
        .await
        .expect("TaskStop should return a tool result");

    assert!(stopped.is_error);
    assert!(
        stopped.content.contains("already completed"),
        "{}",
        stopped.content
    );

    let waited = registry
        .run(
            "WaitDelegate",
            &ctx,
            serde_json::json!({ "ids": [agent_id], "timeout_ms": 1 }),
        )
        .await
        .expect("completed delegate remains queryable");
    assert!(
        waited.content.contains("\"status\":\"completed\""),
        "{}",
        waited.content
    );
}

#[tokio::test]
async fn task_stop_running_delegate_returns_cancelled_not_stopped() {
    let manager = BackgroundTaskManager::new();
    let snapshot = running_agent_snapshot("agent_task_stop_running");
    manager.start_delegate(snapshot).await;

    let result = manager
        .stop("agent_task_stop_running", "user requested stop", 2048)
        .await
        .expect("running delegate should be cancellable");

    assert!(!result.is_error);
    assert!(
        result.content.contains("status: cancelled"),
        "{}",
        result.content
    );
    assert!(
        !result.content.contains("status: stopped"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn task_stop_cancelled_delegate_returns_already_cancelled_error() {
    let manager = BackgroundTaskManager::new();
    let mut snapshot = running_agent_snapshot("agent_task_stop_cancelled");
    snapshot.state = AgentLifecycleState::Cancelled;
    manager.start_delegate(snapshot).await;

    let result = manager
        .stop("agent_task_stop_cancelled", "user requested stop", 2048)
        .await
        .expect("stop should return a tool result");

    assert!(result.is_error);
    assert!(
        result.content.contains("already cancelled"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn task_stop_stops_background_delegate_stream_before_finalizing_record() {
    let registry = ToolRegistry::with_builtin_tools();
    let ctx = blocking_child_ctx();

    let started = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "slow task-stop child",
                "mode": "background"
            }),
        )
        .await
        .expect("background delegate should start");
    let agent_id = started
        .details
        .as_ref()
        .and_then(|details| details.get("agent_id"))
        .and_then(serde_json::Value::as_str)
        .expect("agent_id in details")
        .to_owned();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let stopped = registry
        .run("TaskStop", &ctx, serde_json::json!({ "task_id": agent_id }))
        .await
        .expect("task stop should succeed");

    assert!(
        stopped.content.contains("status: cancelled"),
        "{}",
        stopped.content
    );
    assert!(
        stopped.content.contains("summary: Cancelled by user."),
        "{}",
        stopped.content
    );
    let details = stopped.details.as_ref().expect("details");
    assert_eq!(details["agent_id"], agent_id);
    assert_eq!(details["status"], "cancelled");

    let waited = registry
        .run(
            "WaitDelegate",
            &ctx,
            serde_json::json!({ "ids": [agent_id], "timeout_ms": 5000 }),
        )
        .await
        .expect("wait should return result");
    assert!(
        waited.content.contains("\"status\":\"cancelled\""),
        "{}",
        waited.content
    );
    assert!(
        !waited.content.contains("should not arrive"),
        "{}",
        waited.content
    );
}

fn running_agent_snapshot(id: &str) -> AgentSnapshot {
    AgentSnapshot {
        id: AgentId::from_suffix_for_test(id.trim_start_matches("agent_")),
        display_name: AgentDisplayName::new("Gauss"),
        path: AgentPath::root_child(&AgentDisplayName::new("Gauss")),
        role: AgentRole::Coder,
        mode: AgentRunMode::Background,
        context: neo_agent_core::multi_agent::DelegateContext::None,
        state: AgentLifecycleState::Running,
        task: "long running delegate".to_owned(),
        task_title: "long running delegate".to_owned(),
        created_at_ms: 1,
        updated_at_ms: 1,
        started_at_ms: Some(1),
        terminal_at_ms: None,
        detached_from_foreground: true,
        terminal_reason: None,
        run_count: 1,
        live_messages_received: 0,
        previous_status: None,
        terminal_status_history: Vec::new(),
        resumed_from: None,
        tool_count: 0,
        token_count: 0,
        input_token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: Duration::from_secs(0),
        latest_text: None,
        activity: Vec::new(),
        prior_messages: Vec::new(),
        outcome: None,
    }
}

fn registry_with_multi_agent() -> (ToolRegistry, ToolContext) {
    let turn_done = vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "msg_x".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "done".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ];
    let harness = FakeHarness::from_turns((0..10).map(|_| turn_done.clone()));
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_access(ToolAccess::all())
        .with_child_runtime(
            AgentConfig::for_model(harness.model())
                .with_permission_mode(PermissionMode::Yolo)
                .with_tool_execution_mode(ToolExecutionMode::Sequential),
            harness.client(),
            Arc::new(ToolRegistry::new()),
            1,
        );
    (ToolRegistry::with_builtin_tools(), ctx)
}

/// A child model that emits `MessageStart` then blocks for a long time. Every
/// call to `stream_chat` returns the same blocking stream, so it works for
/// both single Delegate and multi-child `DelegateSwarm` tests. The stream is
/// cancelled when the consumer drops it (via `CancellationToken`).
struct BlockingChildModel;

impl neo_ai::ModelClient for BlockingChildModel {
    fn stream_chat(
        &self,
        _request: neo_ai::ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<neo_ai::AiStreamEvent, neo_ai::AiError>> {
        use futures::StreamExt;
        futures::stream::unfold(false, |mut sent_start| async move {
            if !sent_start {
                sent_start = true;
                return Some((
                    Ok(neo_ai::AiStreamEvent::MessageStart {
                        phase: neo_ai::MessagePhase::Unknown,
                        id: "blocking-child".to_owned(),
                    }),
                    sent_start,
                ));
            }
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            Some((
                Ok(neo_ai::AiStreamEvent::TextDelta {
                    text: "should not arrive".to_owned(),
                }),
                sent_start,
            ))
        })
        .boxed()
    }
}

fn blocking_child_ctx() -> ToolContext {
    use neo_agent_core::tools::ToolContext;
    let dir = tempfile::tempdir().unwrap();
    ToolContext::new(dir.path())
        .unwrap()
        .with_access(ToolAccess::all())
        .with_child_runtime(
            AgentConfig::for_model(neo_agent_core::harness::fake_model())
                .with_permission_mode(PermissionMode::Yolo)
                .with_tool_execution_mode(ToolExecutionMode::Sequential),
            Arc::new(BlockingChildModel),
            Arc::new(ToolRegistry::new()),
            4,
        )
}
