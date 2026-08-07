#![allow(clippy::duration_suboptimal_units)]

use neo_agent_core::harness::FakeHarness;
use neo_agent_core::tools::{ToolContext, ToolRegistry};
use neo_agent_core::{AgentConfig, PermissionMode, ToolAccess, ToolExecutionMode};
use neo_ai::{AiStreamEvent, StopReason};
use std::sync::Arc;

#[tokio::test]
async fn interrupt_delegate_marks_running_agent_cancelled() {
    use neo_agent_core::tools::ToolContext;
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path()).unwrap();
    let agent = ctx
        .multi_agent
        .start_foreground_delegate_for_test("cancel me");
    ctx.background_tasks.start_delegate(agent.clone()).await;

    let result = ToolRegistry::with_builtin_tools()
        .run(
            "InterruptDelegate",
            &ctx,
            serde_json::json!({ "id": agent.id.as_str() }),
        )
        .await
        .expect("interrupt should succeed");

    assert!(result.content.contains("cancelled"));
}

#[tokio::test]
async fn interrupt_delegate_rejects_completed_agent_without_mutating_state() {
    let (registry, ctx) = registry_with_multi_agent();
    let delegate = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "return exactly done",
                "mode": "foreground"
            }),
        )
        .await
        .expect("foreground delegate should complete");
    let agent_id = delegate
        .details
        .as_ref()
        .and_then(|details| details.get("agent_id"))
        .and_then(serde_json::Value::as_str)
        .expect("delegate result should include agent_id")
        .to_owned();

    let interrupted = registry
        .run(
            "InterruptDelegate",
            &ctx,
            serde_json::json!({ "id": agent_id }),
        )
        .await
        .expect("interrupt should return a tool result");

    assert!(interrupted.is_error);
    assert!(
        interrupted.content.contains("already completed"),
        "{}",
        interrupted.content
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
        waited.content.contains("status: completed"),
        "{}",
        waited.content
    );
    assert!(
        !waited.content.contains("mailbox_pending"),
        "{}",
        waited.content
    );
}

#[tokio::test]
async fn interrupt_delegate_stops_background_child_stream() {
    let registry = ToolRegistry::with_builtin_tools();
    let ctx = blocking_child_ctx();

    let started = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "slow background child",
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

    // Give the background worker time to start streaming.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let interrupt = registry
        .run(
            "InterruptDelegate",
            &ctx,
            serde_json::json!({ "id": agent_id }),
        )
        .await
        .expect("interrupt should succeed");

    assert!(
        interrupt.content.contains("status: cancelled"),
        "{}",
        interrupt.content
    );

    let waited = registry
        .run(
            "WaitDelegate",
            &ctx,
            serde_json::json!({ "ids": [agent_id], "timeout_ms": 5000 }),
        )
        .await
        .expect("wait should return result");

    assert!(
        waited.content.contains("status: cancelled"),
        "{}",
        waited.content
    );
    assert!(
        !waited.content.contains("should not arrive"),
        "{}",
        waited.content
    );
}

#[tokio::test]
async fn interrupt_delegate_stops_running_swarm_children() {
    let registry = ToolRegistry::with_builtin_tools();
    let ctx = blocking_child_ctx();

    let started = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "two slow children",
                "prompt_template": "process {{item}}",
                "items": [
                    { "title": "a", "value": "alpha" },
                    { "title": "b", "value": "beta" }
                ],
                "max_concurrency": 2,
                "mode": "background"
            }),
        )
        .await
        .expect("background swarm should start");
    let swarm_id = started
        .details
        .as_ref()
        .and_then(|details| {
            details
                .get("swarm")
                .and_then(|swarm| swarm.get("swarm_id"))
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            started
                .details
                .as_ref()
                .and_then(|details| details.get("task_id").and_then(serde_json::Value::as_str))
        })
        .expect("swarm_id")
        .to_owned();

    // Give the background swarm children time to start streaming.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let interrupt = registry
        .run(
            "InterruptDelegate",
            &ctx,
            serde_json::json!({ "id": swarm_id }),
        )
        .await
        .expect("interrupt should succeed");

    assert!(
        interrupt.content.contains("status: cancelled"),
        "{}",
        interrupt.content
    );

    let waited = registry
        .run(
            "WaitDelegate",
            &ctx,
            serde_json::json!({ "ids": [swarm_id], "timeout_ms": 10000 }),
        )
        .await
        .expect("wait should return result");

    assert!(
        waited.content.contains("status: cancelled"),
        "{}",
        waited.content
    );
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
