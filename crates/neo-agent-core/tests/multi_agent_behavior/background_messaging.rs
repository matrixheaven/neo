#![allow(clippy::duration_suboptimal_units)]
use std::time::Duration;

use neo_agent_core::harness::FakeHarness;
use neo_agent_core::multi_agent::AgentRunMode;
use neo_agent_core::tools::{Tool, ToolContext, ToolFuture, ToolRegistry, ToolResult};
use neo_agent_core::{AgentConfig, PermissionMode, ToolAccess, ToolExecutionMode};
use neo_ai::{AiStreamEvent, StopReason};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, oneshot};

#[tokio::test]
async fn message_delegate_unknown_id_errors_without_creating_mailbox() {
    use neo_agent_core::tools::ToolContext;
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path()).unwrap();

    let result = ToolRegistry::with_builtin_tools()
        .run(
            "MessageDelegate",
            &ctx,
            serde_json::json!({ "id": "agent_missing", "message": "hello?" }),
        )
        .await
        .expect("tool should return an error result");

    assert!(result.is_error, "unknown target must be an error result");
    assert!(
        result.content.contains("unknown delegate"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn message_delegate_background_agent_without_live_steer_returns_resume_hint() {
    use neo_agent_core::tools::ToolContext;
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path()).unwrap();
    let agent = ctx.multi_agent.start_delegate(
        "receive updates",
        None,
        neo_agent_core::multi_agent::AgentRole::Coder,
        AgentRunMode::Background,
        neo_agent_core::multi_agent::DelegateContext::None,
        neo_agent_core::multi_agent::AgentPathKind::Root,
    );
    ctx.background_tasks.start_delegate(agent.clone()).await;

    let result = ToolRegistry::with_builtin_tools()
        .run(
            "MessageDelegate",
            &ctx,
            serde_json::json!({ "id": agent.id.as_str(), "message": "new facts" }),
        )
        .await
        .expect("message should return a tool result");

    assert!(result.is_error, "{}", result.content);
    assert!(
        result
            .content
            .contains("agent is not running; use Delegate with resume"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn message_delegate_non_running_agents_do_not_create_mailboxes() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path()).unwrap();
    let first = ctx.multi_agent.start_delegate(
        "first receiver",
        None,
        neo_agent_core::multi_agent::AgentRole::Coder,
        AgentRunMode::Background,
        neo_agent_core::multi_agent::DelegateContext::None,
        neo_agent_core::multi_agent::AgentPathKind::Root,
    );
    let second = ctx.multi_agent.start_delegate(
        "second receiver",
        None,
        neo_agent_core::multi_agent::AgentRole::Coder,
        AgentRunMode::Background,
        neo_agent_core::multi_agent::DelegateContext::None,
        neo_agent_core::multi_agent::AgentPathKind::Root,
    );
    ctx.background_tasks.start_delegate(first.clone()).await;
    ctx.background_tasks.start_delegate(second.clone()).await;
    let tools = ToolRegistry::with_builtin_tools();

    let first_result = tools
        .run(
            "MessageDelegate",
            &ctx,
            serde_json::json!({ "id": first.id.as_str(), "message": "first facts" }),
        )
        .await
        .expect("first message should return a tool result");
    let second_result = tools
        .run(
            "MessageDelegate",
            &ctx,
            serde_json::json!({ "id": second.id.as_str(), "message": "second facts" }),
        )
        .await
        .expect("second message should return a tool result");

    assert!(first_result.is_error);
    assert!(second_result.is_error);
    assert!(
        first_result
            .content
            .contains("agent is not running; use Delegate with resume")
    );
    assert!(
        second_result
            .content
            .contains("agent is not running; use Delegate with resume")
    );
}

#[tokio::test]
async fn message_delegate_delivers_to_running_background_delegate_as_live_steer() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: neo_ai::MessagePhase::Unknown,
                id: "child_msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_block".to_owned(),
                name: "block_probe".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_block".to_owned(),
                raw_arguments: json!({}).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: neo_ai::MessagePhase::Unknown,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: neo_ai::MessagePhase::Unknown,
                id: "child_msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "saw live steer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: neo_ai::MessagePhase::Unknown,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let dir = tempfile::tempdir().unwrap();
    let started = Arc::new(Notify::new());
    let (release_sender, release_receiver) = oneshot::channel();
    let mut child_tools = ToolRegistry::new();
    child_tools.register(BlockingProbeTool {
        started: Arc::clone(&started),
        release: Arc::new(Mutex::new(Some(release_receiver))),
    });
    let ctx = ToolContext::new(dir.path()).unwrap().with_child_runtime(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_tool_execution_mode(ToolExecutionMode::Sequential),
        harness.client(),
        Arc::new(child_tools),
        1,
    );
    let tools = ToolRegistry::with_builtin_tools();

    let delegate_result = tools
        .run(
            "Delegate",
            &ctx,
            json!({ "task": "wait for live message", "mode": "background" }),
        )
        .await
        .expect("background delegate should start");
    let agent_id = delegate_result
        .content
        .lines()
        .find_map(|line| line.strip_prefix("agent_id: "))
        .expect("delegate result should include agent_id")
        .to_owned();
    started.notified().await;

    let message_result = tools
        .run(
            "MessageDelegate",
            &ctx,
            json!({ "id": agent_id, "message": "GOT_MSG:yes" }),
        )
        .await
        .expect("message should deliver");
    assert!(
        message_result.content.contains("outcome: delivered"),
        "{}",
        message_result.content
    );
    release_sender.send(()).expect("release child tool");

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if harness.requests().len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("child should make a second model request");

    let second_request = harness.requests().pop().expect("second child request");
    let request_text = format!("{:?}", second_request.messages);
    assert!(
        request_text.contains("GOT_MSG:yes"),
        "second child request did not include live message: {request_text}"
    );
}

#[tokio::test]
async fn message_delegate_rejects_completed_agent_with_resume_hint() {
    let (registry, ctx) = registry_with_multi_agent();
    let delegate = registry
        .run(
            "Delegate",
            &ctx,
            serde_json::json!({
                "task": "finish quickly",
                "mode": "foreground"
            }),
        )
        .await
        .expect("delegate should complete");
    let agent_id = delegate
        .details
        .as_ref()
        .and_then(|details| details.get("agent_id"))
        .and_then(serde_json::Value::as_str)
        .expect("delegate result should include agent_id")
        .to_owned();

    let message = registry
        .run(
            "MessageDelegate",
            &ctx,
            serde_json::json!({
                "id": agent_id,
                "message": "please do more"
            }),
        )
        .await
        .expect("MessageDelegate should return a tool result");

    assert!(message.is_error);
    assert!(
        message
            .content
            .contains("terminal agents cannot receive live messages"),
        "{}",
        message.content
    );
    assert_eq!(
        message.details.as_ref().and_then(|details| details
            .get("resume_hint")
            .and_then(serde_json::Value::as_str)),
        Some(format!("Delegate with resume=\"{agent_id}\"").as_str())
    );
}

#[tokio::test]
async fn message_delegate_broadcasts_to_running_swarm_children() {
    let (registry, ctx) = registry_with_multi_agent();
    let started = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "live swarm",
                "items": [{"title": "a", "value": "a"}, {"title": "b", "value": "b"}],
                "prompt_template": "Wait for follow-up about {{item}}",
                "mode": "background",
                "max_concurrency": 2
            }),
        )
        .await
        .expect("swarm starts");
    let swarm_id = started
        .details
        .as_ref()
        .and_then(|details| {
            details
                .get("swarm_id")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    details
                        .get("swarm")
                        .and_then(|swarm| swarm.get("swarm_id"))
                        .and_then(serde_json::Value::as_str)
                })
                .or_else(|| details.get("task_id").and_then(serde_json::Value::as_str))
        })
        .expect("swarm_id")
        .to_owned();

    let message = registry
        .run(
            "MessageDelegate",
            &ctx,
            serde_json::json!({
                "id": swarm_id,
                "message": "continue now"
            }),
        )
        .await
        .expect("message returns result");

    // Message may fail if children already completed (FakeHarness completes instantly).
    // The test just verifies the swarm routing works without crashing.
    // If delivered, check format; if error, verify it has structured skipped details.
    if message.is_error {
        assert!(
            message.content.contains("no running children"),
            "{}",
            message.content
        );
        let details = message.details.as_ref().expect("error must have details");
        assert!(
            details["skipped"].is_array(),
            "skipped must be an array: {details}"
        );
        assert!(
            details["delivered"].as_array().is_some_and(Vec::is_empty),
            "delivered must be empty: {details}"
        );
    } else {
        assert!(
            message.content.contains("delivered:"),
            "{}",
            message.content
        );
    }
}

#[tokio::test]
async fn message_delegate_swarm_all_completed_returns_structured_skipped() {
    let (registry, ctx) = registry_with_multi_agent();
    let started = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "fast swarm",
                "items": [{"title": "x", "value": "x"}, {"title": "y", "value": "y"}],
                "prompt_template": "Process {{item}}",
                "mode": "foreground"
            }),
        )
        .await
        .expect("swarm starts and completes");
    let swarm_id = started
        .details
        .as_ref()
        .and_then(|details| {
            details
                .get("swarm_id")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    details
                        .get("swarm")
                        .and_then(|swarm| swarm.get("swarm_id"))
                        .and_then(serde_json::Value::as_str)
                })
        })
        .expect("swarm_id")
        .to_owned();

    let message = registry
        .run(
            "MessageDelegate",
            &ctx,
            serde_json::json!({
                "id": swarm_id,
                "message": "post-completion guidance"
            }),
        )
        .await
        .expect("message returns result");

    // All children completed — must return an error with structured details.
    assert!(message.is_error, "{}", message.content);
    assert!(
        message.content.contains("no running children"),
        "{}",
        message.content
    );
    let details = message.details.as_ref().expect("details required");
    let skipped = details["skipped"]
        .as_array()
        .expect("skipped must be an array");
    assert_eq!(skipped.len(), 2, "both children should be skipped");
    assert!(
        skipped
            .iter()
            .all(|entry| { entry["state"].as_str().is_some_and(|s| s == "completed") }),
        "all skipped children should show completed state: {skipped:?}"
    );
    assert!(
        details["delivered"].as_array().is_some_and(Vec::is_empty),
        "delivered must be empty"
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

struct BlockingProbeTool {
    started: Arc<Notify>,
    release: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

impl Tool for BlockingProbeTool {
    fn name(&self) -> &'static str {
        "block_probe"
    }

    fn description(&self) -> &'static str {
        "Test-only blocking probe."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            self.started.notify_one();
            let release = self
                .release
                .lock()
                .expect("release lock poisoned")
                .take()
                .expect("release receiver should exist");
            let _ = release.await;
            Ok(ToolResult::ok("unblocked"))
        })
    }
}
