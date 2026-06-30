use std::time::Duration;

use neo_agent_core::multi_agent::{
    AgentLifecycleState, DelegateMailbox, MultiAgentRuntime, SwarmProgressInput, SwarmScheduler,
    SwarmSchedulerConfig, estimate_swarm_progress,
};
use neo_agent_core::tools::{ToolContext, ToolRegistry};
use tempfile::tempdir;

// --- Mailbox tests ---

#[test]
fn delegate_mailbox_tracks_pending_delivery() {
    let mut mailbox = DelegateMailbox::default();
    let message = mailbox.push("check the failed test".to_owned());

    assert_eq!(mailbox.pending().len(), 1);
    mailbox.mark_delivered(&message.id);
    assert!(mailbox.pending().is_empty());
}

#[tokio::test]
async fn message_delegate_queues_mailbox_message() {
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
        .expect("message should succeed");

    assert!(result.content.contains("status: queued"));
    assert!(result.content.contains("message_id:"));

    let pending = ctx.multi_agent.pending_mailbox(agent.id.as_str());
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].text, "check the error");
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
    let progress = estimate_swarm_progress(SwarmProgressInput {
        total: 4,
        completed: 3,
        failed: 0,
        running: 1,
        queued: 0,
        suspended: 0,
        median_completed_duration: Some(Duration::from_secs(10)),
        longest_running_duration: Duration::from_secs(100),
    });

    assert!(progress < 1.0);
    assert!(progress <= 0.95);
}

#[test]
fn progress_estimate_returns_full_when_all_terminal() {
    let progress = estimate_swarm_progress(SwarmProgressInput {
        total: 3,
        completed: 2,
        failed: 1,
        running: 0,
        queued: 0,
        suspended: 0,
        median_completed_duration: None,
        longest_running_duration: Duration::ZERO,
    });

    assert!((progress - 1.0).abs() < f32::EPSILON);
}

#[test]
fn progress_estimate_increases_with_running_duration() {
    let early = estimate_swarm_progress(SwarmProgressInput {
        total: 4,
        completed: 0,
        failed: 0,
        running: 1,
        queued: 3,
        suspended: 0,
        median_completed_duration: Some(Duration::from_secs(60)),
        longest_running_duration: Duration::from_secs(5),
    });

    let late = estimate_swarm_progress(SwarmProgressInput {
        total: 4,
        completed: 0,
        failed: 0,
        running: 1,
        queued: 3,
        suspended: 0,
        median_completed_duration: Some(Duration::from_secs(60)),
        longest_running_duration: Duration::from_secs(50),
    });

    assert!(late > early);
}
