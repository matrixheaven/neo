use std::time::Duration;

use futures::StreamExt;
use neo_agent_core::harness::FakeHarness;
use neo_agent_core::multi_agent::{AgentId, AgentLifecycleState, AgentRunMode, MultiAgentRuntime};
use neo_agent_core::tools::{
    BackgroundTaskKind, BackgroundTaskManager, Tool, ToolContext, ToolFuture, ToolRegistry,
    ToolResult,
};
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, PermissionMode, ToolAccess,
    ToolExecutionMode,
};
use neo_ai::{AiStreamEvent, StopReason};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, oneshot};

#[tokio::test]
async fn background_manager_lists_delegate_tasks() {
    let runtime = MultiAgentRuntime::new();
    let agent = runtime.start_foreground_delegate_for_test("inspect task browser");
    let manager = BackgroundTaskManager::new();

    manager.start_delegate(agent.clone()).await;
    let snapshots = manager.list(false, 10).await;

    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].kind, BackgroundTaskKind::Delegate);
    assert_eq!(snapshots[0].task_id, agent.id.as_str());
    assert!(snapshots[0].delegate.is_some());
}

#[tokio::test]
async fn background_manager_lists_swarm_tasks() {
    use neo_agent_core::multi_agent::{
        AgentDisplayName, AgentPath, AgentRole, AgentSnapshot, SwarmChildSnapshot, SwarmSnapshot,
    };
    let _runtime = MultiAgentRuntime::new();
    let name = AgentDisplayName::new("Zeno");
    let child_agent = AgentSnapshot {
        id: AgentId::from_suffix_for_test("sw-0"),
        display_name: name.clone(),
        path: AgentPath::root_child(&name),
        role: AgentRole::Coder,
        mode: AgentRunMode::Background,
        state: AgentLifecycleState::Running,
        task: "item 0".to_owned(),
        tool_count: 0,
        token_count: 0,
        elapsed: Duration::ZERO,
        latest_text: None,
        activity: Vec::new(),
        outcome: None,
    };
    let swarm = SwarmSnapshot {
        swarm_id: "swarm-test".to_owned(),
        description: "test swarm".to_owned(),
        mode: AgentRunMode::Background,
        max_concurrency: 1,
        children: vec![SwarmChildSnapshot {
            item_index: 0,
            item: "check".to_owned(),
            agent: child_agent,
        }],
    };
    let manager = BackgroundTaskManager::new();
    manager.start_delegate_swarm(swarm).await;

    let snapshots = manager.list(false, 10).await;
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].kind, BackgroundTaskKind::DelegateSwarm);
    assert!(snapshots[0].swarm.is_some());
}

#[tokio::test]
async fn delegate_background_registers_task() {
    let harness = FakeHarness::from_turns([vec![
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::ToolCallStart {
            id: "tool_1".to_owned(),
            name: "Delegate".to_owned(),
        },
        AiStreamEvent::ToolCallArgsDelta {
            id: "tool_1".to_owned(),
            json_fragment: r#"{"task":"bg task","mode":"background"}"#.to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: "tool_1".to_owned(),
            arguments: json!({ "task": "bg task", "mode": "background" }),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::ToolUse,
            usage: None,
        },
    ]]);
    let tools = ToolRegistry::with_builtin_tools();
    let config = AgentConfig::for_model(harness.model())
        .with_tool_execution_mode(ToolExecutionMode::Sequential)
        .with_permission_mode(PermissionMode::Yolo);
    let runtime = AgentRuntime::with_tools(config, harness.client(), tools);
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run bg delegate"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    // The tool result should mention background mode.
    let tool_finished: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolExecutionFinished { result, .. } => Some(result),
            _ => None,
        })
        .collect();
    let delegate_result = tool_finished
        .iter()
        .find(|r| r.content.contains("kind: delegate"))
        .expect("should have a delegate result");
    assert!(
        delegate_result.content.contains("status: running"),
        "{}",
        delegate_result.content
    );

    // Details should include background mode.
    let details = delegate_result.details.as_ref().expect("details");
    assert_eq!(details["mode"], "background");
}

#[tokio::test]
async fn ctrl_b_detach_preserves_agent_id_and_registers_background_task() {
    let runtime = MultiAgentRuntime::new();
    let manager = BackgroundTaskManager::new();
    let running = runtime.start_foreground_delegate_for_test("detach me");

    let detached = runtime
        .detach_agent(&running.id)
        .expect("agent should detach");
    manager.start_delegate(detached.clone()).await;
    let tasks = manager.list(false, 10).await;

    assert_eq!(detached.id, running.id);
    assert_eq!(detached.mode, AgentRunMode::Background);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task_id, running.id.as_str());
}

#[tokio::test]
async fn list_delegates_reports_background_delegate() {
    use neo_agent_core::tools::ToolContext;
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_access(ToolAccess::all());
    let agent = ctx
        .multi_agent
        .start_foreground_delegate_for_test("inspect background registry");
    ctx.background_tasks.start_delegate(agent.clone()).await;

    let result = ToolRegistry::with_builtin_tools()
        .run(
            "ListDelegates",
            &ctx,
            serde_json::json!({ "include_completed": true }),
        )
        .await
        .expect("list should succeed");

    assert!(result.content.contains(agent.id.as_str()));
    assert!(result.content.contains("inspect background registry"));
}

#[tokio::test]
async fn wait_delegate_times_out_without_completion() {
    use neo_agent_core::tools::ToolContext;
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path()).unwrap();
    let agent = ctx
        .multi_agent
        .start_foreground_delegate_for_test("long running task");
    ctx.background_tasks.start_delegate(agent.clone()).await;

    let result = ToolRegistry::with_builtin_tools()
        .run(
            "WaitDelegate",
            &ctx,
            serde_json::json!({ "id": agent.id.as_str(), "timeout_ms": 1 }),
        )
        .await
        .expect("wait should return timeout result");

    assert!(result.content.contains("timed_out"));
}

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
    assert!(
        ctx.multi_agent.pending_mailbox("agent_missing").is_empty(),
        "unknown id must not create a mailbox"
    );
}

#[tokio::test]
async fn message_delegate_existing_background_agent_queues_message() {
    use neo_agent_core::tools::ToolContext;
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path()).unwrap();
    let agent = ctx.multi_agent.start_delegate(
        "receive updates",
        neo_agent_core::multi_agent::AgentRole::Coder,
        AgentRunMode::Background,
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
        .expect("message should queue");

    assert!(!result.is_error, "{}", result.content);
    let pending = ctx.multi_agent.pending_mailbox(agent.id.as_str());
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].text, "new facts");
}

#[tokio::test]
async fn message_delegate_message_ids_are_globally_unique_and_expose_pending_count() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path()).unwrap();
    let first = ctx.multi_agent.start_delegate(
        "first receiver",
        neo_agent_core::multi_agent::AgentRole::Coder,
        AgentRunMode::Background,
        neo_agent_core::multi_agent::AgentPathKind::Root,
    );
    let second = ctx.multi_agent.start_delegate(
        "second receiver",
        neo_agent_core::multi_agent::AgentRole::Coder,
        AgentRunMode::Background,
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
        .expect("first message should queue");
    let second_result = tools
        .run(
            "MessageDelegate",
            &ctx,
            serde_json::json!({ "id": second.id.as_str(), "message": "second facts" }),
        )
        .await
        .expect("second message should queue");

    let first_id = first_result.details.as_ref().unwrap()["message_id"]
        .as_str()
        .unwrap();
    let second_id = second_result.details.as_ref().unwrap()["message_id"]
        .as_str()
        .unwrap();
    assert_ne!(first_id, second_id);
    assert!(first_id.starts_with("msg_"));
    assert!(second_id.starts_with("msg_"));
    assert_eq!(
        first_result.details.as_ref().unwrap()["mailbox_pending_count"],
        1
    );

    let list = tools
        .run(
            "ListDelegates",
            &ctx,
            serde_json::json!({ "include_completed": true }),
        )
        .await
        .expect("list should succeed");
    assert!(
        list.content.contains("mailbox_pending: 1"),
        "{}",
        list.content
    );
}

#[tokio::test]
async fn message_delegate_delivers_to_running_background_delegate_as_live_steer() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "child_msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_block".to_owned(),
                name: "block_probe".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_block".to_owned(),
                arguments: json!({}),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "child_msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "saw live steer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
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
        message_result.content.contains("status: delivered"),
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
async fn task_stop_cancels_delegate_runtime_and_completion_cannot_overwrite_stopped() {
    use neo_agent_core::tools::ToolContext;
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path())
        .unwrap()
        .with_access(ToolAccess::all());
    let agent = ctx.multi_agent.start_delegate(
        "stop me",
        neo_agent_core::multi_agent::AgentRole::Coder,
        AgentRunMode::Background,
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
        .expect("TaskStop should stop delegate");

    assert!(
        result.content.contains("status: stopped"),
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
        neo_agent_core::tools::BackgroundTaskStatus::Stopped
    );
}

#[tokio::test]
async fn task_stop_cancels_delegate_swarm_children_and_late_completion_cannot_overwrite_stopped() {
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
        AgentRole::Coder,
        AgentRunMode::Background,
        AgentPathKind::SwarmChild(&swarm_id),
    );
    let second = ctx.multi_agent.start_delegate(
        "check beta",
        AgentRole::Coder,
        AgentRunMode::Background,
        AgentPathKind::SwarmChild(&swarm_id),
    );
    let swarm = SwarmSnapshot {
        swarm_id: swarm_id.clone(),
        description: "background swarm".to_owned(),
        mode: AgentRunMode::Background,
        max_concurrency: 1,
        children: vec![
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
        ],
    };
    ctx.multi_agent.register_swarm(swarm.clone());
    ctx.background_tasks.start_delegate_swarm(swarm).await;

    let result = ToolRegistry::with_builtin_tools()
        .run("TaskStop", &ctx, serde_json::json!({ "task_id": swarm_id }))
        .await
        .expect("TaskStop should stop delegate swarm");

    assert!(
        result.content.contains("status: stopped"),
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
        mode: AgentRunMode::Background,
        max_concurrency: 1,
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
    assert_eq!(stopped.status, BackgroundTaskStatus::Stopped);
}

struct BlockingProbeTool {
    started: Arc<Notify>,
    release: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

impl Tool for BlockingProbeTool {
    fn name(&self) -> &str {
        "block_probe"
    }

    fn description(&self) -> &str {
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
            self.started.notify_waiters();
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
