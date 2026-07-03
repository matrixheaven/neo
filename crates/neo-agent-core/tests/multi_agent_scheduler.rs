use std::time::Duration;

use neo_agent_core::multi_agent::{
    AgentLifecycleState, MultiAgentRuntime, SwarmProgressInput, SwarmScheduler,
    SwarmSchedulerConfig, estimate_swarm_progress,
};
use neo_agent_core::tools::{ToolContext, ToolRegistry};
use tempfile::tempdir;

// --- Live message tests ---

#[tokio::test]
async fn message_delegate_non_live_agent_returns_resume_hint() {
    let dir = tempdir().unwrap();
    let ctx = ToolContext::new(dir.path()).unwrap();
    let agent = ctx
        .multi_agent
        .start_foreground_delegate_for_test("background task");
    ctx.background_tasks.start_delegate(agent.clone()).await;

    let result = ToolRegistry::with_builtin_tools()
        .run(
            "MessageDelegate",
            &ctx,
            serde_json::json!({ "id": agent.id.as_str(), "message": "check the error" }),
        )
        .await
        .expect("message should return a tool result");

    assert!(result.is_error);
    assert!(
        result
            .content
            .contains("agent is not running; use Delegate with resume")
    );
}

// --- Scheduler tests ---

#[test]
fn swarm_scheduler_reduces_concurrency_on_rate_limit() {
    let mut scheduler = SwarmScheduler::new(SwarmSchedulerConfig::default());

    scheduler.record_rate_limit();

    assert_eq!(scheduler.effective_concurrency(), 3);
}

#[test]
fn swarm_scheduler_recover_restores_concurrency() {
    let mut scheduler = SwarmScheduler::new(SwarmSchedulerConfig::default());
    scheduler.record_rate_limit();
    assert_eq!(scheduler.effective_concurrency(), 3);

    scheduler.record_recovery();
    assert_eq!(scheduler.effective_concurrency(), 4);
}

#[test]
fn swarm_scheduler_retry_delay_grows_exponentially() {
    let scheduler = SwarmScheduler::new(SwarmSchedulerConfig::default());

    assert!(scheduler.retry_delay(2) > scheduler.retry_delay(1));
}

// --- Partial resume tests ---

#[test]
fn partial_swarm_resume_skips_completed_items() {
    let runtime = MultiAgentRuntime::new();
    let swarm_id = runtime.create_swarm_for_test(vec![
        ("done", AgentLifecycleState::Completed),
        ("failed", AgentLifecycleState::Failed),
        ("queued", AgentLifecycleState::Queued),
    ]);

    let resumable = runtime.resumable_swarm_items(&swarm_id);

    assert_eq!(resumable, vec![1, 2]);
}

// --- Progress estimator tests ---

#[test]
fn progress_estimate_never_claims_completion_while_items_are_active() {
    let progress = estimate_swarm_progress(&SwarmProgressInput {
        total: 4,
        completed: 3,
        failed: 0,
        running: 1,
        queued: 0,
        suspended: 0,
        median_completed_duration: Some(Duration::from_secs(10)),
        running_durations: vec![Duration::from_secs(100)],
    });

    assert!(progress < 1.0);
    assert!(progress <= 0.95);
}

#[test]
fn progress_estimate_returns_full_when_all_terminal() {
    let progress = estimate_swarm_progress(&SwarmProgressInput {
        total: 3,
        completed: 2,
        failed: 1,
        running: 0,
        queued: 0,
        suspended: 0,
        median_completed_duration: None,
        running_durations: vec![],
    });

    assert!((progress - 1.0).abs() < f32::EPSILON);
}

#[test]
fn progress_estimate_increases_with_running_duration() {
    let early = estimate_swarm_progress(&SwarmProgressInput {
        total: 4,
        completed: 0,
        failed: 0,
        running: 1,
        queued: 3,
        suspended: 0,
        median_completed_duration: Some(Duration::from_secs(60)),
        running_durations: vec![Duration::from_secs(5)],
    });

    let late = estimate_swarm_progress(&SwarmProgressInput {
        total: 4,
        completed: 0,
        failed: 0,
        running: 1,
        queued: 3,
        suspended: 0,
        median_completed_duration: Some(Duration::from_secs(60)),
        running_durations: vec![Duration::from_secs(50)],
    });

    assert!(late > early);
}
