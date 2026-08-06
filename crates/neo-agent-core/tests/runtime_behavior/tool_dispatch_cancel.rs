use super::fake_harness::EchoTool;
use super::fake_harness::RecordingEchoTool;
use super::fake_harness::echo_tool_harness;
use super::fake_harness::final_done_turn;
use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, PermissionMode, StopReason,
    Tool, ToolContext, ToolExecutionMode, ToolFuture, ToolRegistry, ToolResult,
    harness::FakeHarness,
};
use neo_ai::{AiStreamEvent, MessagePhase};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, oneshot};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn runtime_cancels_in_flight_tool_execution_and_finishes_run() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "never".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({}).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(NeverTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    let mut stream = runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text("call never"),
        cancel.clone(),
    );
    let mut events = Vec::new();

    loop {
        let event = timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("tool start should arrive promptly")
            .expect("event before cancellation")
            .expect("event should be ok");
        let should_cancel = matches!(
            event,
            AgentEvent::ToolExecutionStarted { ref id, .. } if id == "tool_1"
        );
        events.push(event);
        if should_cancel {
            cancel.cancel();
            break;
        }
    }
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("cancelled tool run should finish promptly")
    {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "never".to_owned(),
        result: ToolResult {
            content: "tool execution cancelled".to_owned(),
            is_error: true,
            details: Some(serde_json::json!({"kind": "cancelled", "side_effect_occurred": false})),
            terminate: false,
        },
        workflow_origin: None,
        output_ref: None,
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Cancelled,
        })
    );
    assert_eq!(context.messages().len(), 2);
}

#[tokio::test]
async fn runtime_cancels_while_waiting_for_async_before_tool_hook() {
    let harness = echo_tool_harness("should not execute");
    let (hook_wait_sender, hook_wait_receiver) = oneshot::channel::<()>();
    let hook_wait_receiver = Arc::new(Mutex::new(Some(hook_wait_receiver)));
    let hook_started = Arc::new(Notify::new());
    let hook_started_for_hook = Arc::clone(&hook_started);
    let hook_wait_receiver_for_hook = Arc::clone(&hook_wait_receiver);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Sequential)
            .with_permission_mode(PermissionMode::Yolo)
            .with_async_before_tool_call(move |_call, _cancel| {
                let started = hook_started_for_hook.clone();
                let receiver = hook_wait_receiver_for_hook.clone();
                async move {
                    started.notify_one();
                    let wait = receiver
                        .lock()
                        .expect("receiver lock poisoned")
                        .take()
                        .expect("hook wait receiver should be present");
                    let _ = wait.await;
                    None
                }
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    // The before-hook runs during batch authorization, before any
    // ToolExecutionStarted: cancel as soon as the hook starts, then drain.
    let mut stream = runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text("call echo"),
        cancel.clone(),
    );
    let mut events = Vec::new();
    timeout(Duration::from_millis(250), hook_started.notified())
        .await
        .expect("async hook should start promptly");
    cancel.cancel();
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("cancelled async hook should finish promptly")
    {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);
    drop(hook_wait_sender);

    assert_async_hook_cancelled_cleanly(&events, &context);
    assert!(executed.lock().expect("executed lock poisoned").is_empty());
}

#[tokio::test]
async fn runtime_cancels_while_waiting_for_async_after_tool_hook() {
    let harness = echo_tool_harness("executed");
    let (hook_wait_sender, hook_wait_receiver) = oneshot::channel::<()>();
    let hook_wait_receiver = Arc::new(Mutex::new(Some(hook_wait_receiver)));
    let hook_started = Arc::new(Notify::new());
    let hook_started_for_hook = Arc::clone(&hook_started);
    let hook_wait_receiver_for_hook = Arc::clone(&hook_wait_receiver);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Sequential)
            .with_permission_mode(PermissionMode::Yolo)
            .with_async_after_tool_call(move |_call, result, _cancel| {
                let started = hook_started_for_hook.clone();
                let receiver = hook_wait_receiver_for_hook.clone();
                async move {
                    started.notify_one();
                    let wait = receiver
                        .lock()
                        .expect("receiver lock poisoned")
                        .take()
                        .expect("hook wait receiver should be present");
                    let _ = wait.await;
                    result
                }
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    let events =
        cancel_after_async_tool_hook_starts(&runtime, &mut context, cancel, &hook_started).await;
    drop(hook_wait_sender);

    assert_async_hook_cancelled_cleanly(&events, &context);
    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["executed".to_owned()]
    );
}

async fn cancel_after_async_tool_hook_starts(
    runtime: &AgentRuntime,
    context: &mut AgentContext,
    cancel: CancellationToken,
    hook_started: &Notify,
) -> Vec<AgentEvent> {
    let mut stream = runtime.run_turn_with_cancel(
        context,
        AgentMessage::user_text("call echo"),
        cancel.clone(),
    );
    let mut events = Vec::new();

    loop {
        let event = timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("tool start should arrive promptly")
            .expect("event before cancellation")
            .expect("event should be ok");
        let should_cancel = matches!(
            event,
            AgentEvent::ToolExecutionStarted { ref id, .. } if id == "tool_1"
        );
        events.push(event);
        if should_cancel {
            break;
        }
    }
    timeout(Duration::from_millis(250), hook_started.notified())
        .await
        .expect("async hook should start promptly");
    cancel.cancel();
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("cancelled async hook should finish promptly")
    {
        events.push(event.expect("event should be ok"));
    }
    events
}

fn assert_async_hook_cancelled_cleanly(events: &[AgentEvent], context: &AgentContext) {
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult {
            content: "tool execution cancelled".to_owned(),
            is_error: true,
            details: Some(serde_json::json!({"kind": "cancelled", "side_effect_occurred": false})),
            terminate: false,
        },
        workflow_origin: None,
        output_ref: None,
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Cancelled,
        })
    );
    assert_eq!(context.messages().len(), 2);
}

#[tokio::test]
async fn runtime_parallel_cancellation_finishes_all_started_tool_wrappers() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "text": "fast" }).to_string(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "never".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                raw_arguments: json!({}).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool);
    tools.register(NeverTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Parallel)
            .with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    let mut stream = runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text("call parallel tools"),
        cancel.clone(),
    );
    let mut events = Vec::new();

    loop {
        let event = timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("tool starts should arrive promptly")
            .expect("event before cancellation")
            .expect("event should be ok");
        let should_cancel = matches!(
            event,
            AgentEvent::ToolExecutionStarted { ref id, .. } if id == "tool_2"
        );
        events.push(event);
        if should_cancel {
            cancel.cancel();
            break;
        }
    }
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("cancelled parallel tool run should finish promptly")
    {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::ok("fast"),
        workflow_origin: None,
        output_ref: None,
    }));
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_2".to_owned(),
        name: "never".to_owned(),
        result: ToolResult {
            content: "tool execution cancelled".to_owned(),
            is_error: true,
            details: Some(serde_json::json!({"kind": "cancelled", "side_effect_occurred": false})),
            terminate: false,
        },
        workflow_origin: None,
        output_ref: None,
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Cancelled,
        })
    );
    assert_eq!(context.messages().len(), 2);
}

#[tokio::test]
async fn runtime_parallel_cancellation_does_not_start_later_tool_calls() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "text": "first" }).to_string(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "never".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                raw_arguments: json!({}).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let cancel = CancellationToken::new();
    let cancel_from_hook = cancel.clone();
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool);
    tools.register(NeverTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Parallel)
            .with_before_tool_call(move |call| {
                if call.id.as_ref() == "tool_1" {
                    cancel_from_hook.cancel();
                    Some(ToolResult::ok("first"))
                } else {
                    None
                }
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn_with_cancel(
            &mut context,
            AgentMessage::user_text("call parallel tools"),
            cancel,
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::ok("first"),
        workflow_origin: None,
        output_ref: None,
    }));
    assert!(!events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ToolExecutionStarted { id, .. } if id == "tool_2"
        )
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Cancelled,
        })
    );
    assert_eq!(context.messages().len(), 2);
}

struct NeverTool;

impl Tool for NeverTool {
    fn name(&self) -> &'static str {
        "never"
    }

    fn description(&self) -> &'static str {
        "Never completes."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(std::future::pending())
    }
}
