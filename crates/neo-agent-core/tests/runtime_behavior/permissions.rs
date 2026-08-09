use super::fake_harness::EchoTool;
use super::fake_harness::RecordingEchoTool;
use super::fake_harness::echo_tool_harness;
use super::fake_harness::final_done_turn;
use super::fake_harness::tool_call_turn;
use super::permissions_scope::permit_for_session;
use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, AgentRuntimeError,
    ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest, ApprovalResponse,
    Content, PermissionMode, PermissionOperation, SessionApprovalKey, SessionApprovalScope,
    StopReason, ToolExecutionMode, ToolRegistry, ToolResult, harness::FakeHarness,
};
use neo_ai::{AiStreamEvent, MessagePhase};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

pub(crate) fn select_action(request: &ApprovalRequest, action: ApprovalAction) -> ApprovalResponse {
    assert!(
        request.options.iter().any(|option| option.action == action),
        "action {action:?} not offered in {:?}",
        request
            .options
            .iter()
            .map(|option| &option.action)
            .collect::<Vec<_>>()
    );
    ApprovalResponse::Selected {
        request_id: request.id.clone(),
        action,
        feedback: None,
    }
}

pub(crate) fn permit_once(request: &ApprovalRequest) -> ApprovalResponse {
    select_action(request, ApprovalAction::PermitOnce)
}

fn reject_action(request: &ApprovalRequest) -> ApprovalResponse {
    select_action(request, ApprovalAction::Reject)
}

#[tokio::test]
async fn runtime_emits_approval_request_for_ask_permission_and_skips_tool_execution() {
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
                raw_arguments: json!({ "text": "needs approval" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Ask),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call tool"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    assert!(events.contains(&AgentEvent::ApprovalRequested {
        request: echo_tool_approval_request("tool_1", ""),
    }));
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult {
            content: "approval required for tool: echo".to_owned(),
            media: Vec::new(),
            is_error: true,
            details: Some(serde_json::json!({"kind": "permission", "decision": "required", "operation": "tool", "subject": "echo", "side_effect_occurred": false})),
            terminate: false,
        },
        workflow_origin: None,
        output_ref: None,
    }));
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result(
            "tool_1",
            "echo",
            vec![Content::text("approval required for tool: echo")],
            true
        )
    );
}

fn assert_tool_was_executed(executed: &[String], should_execute: bool) {
    let was_executed = !executed.is_empty();
    assert_eq!(
        was_executed, should_execute,
        "expected should_execute={should_execute}, executed list: {executed:?}"
    );
}

pub(crate) fn echo_tool_session_scope(workspace: impl Into<String>) -> SessionApprovalScope {
    SessionApprovalScope {
        keys: vec![SessionApprovalKey::Tool {
            workspace: workspace.into(),
            name: "echo".to_owned(),
        }],
        label: "Approve this tool for this session".to_owned(),
        detail: "Tool: echo".to_owned(),
    }
}

pub(crate) fn echo_tool_options(workspace: impl Into<String>) -> Vec<ApprovalOption> {
    let scope = echo_tool_session_scope(workspace);
    vec![
        ApprovalOption {
            label: "Approve once".to_owned(),
            description: None,
            action: ApprovalAction::PermitOnce,
        },
        ApprovalOption {
            label: scope.label.clone(),
            description: Some(scope.detail.clone()),
            action: ApprovalAction::PermitForSession { scope },
        },
        ApprovalOption {
            label: "Reject".to_owned(),
            description: None,
            action: ApprovalAction::Reject,
        },
    ]
}

pub(crate) fn echo_tool_approval_request(
    id: &str,
    workspace: impl Into<String>,
) -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: id.to_owned(),
        operation: PermissionOperation::Tool,
        presentation: ApprovalPresentation::Tool {
            title: "Run tool?".to_owned(),
            details: vec!["tool: echo".to_owned()],
        },
        options: echo_tool_options(workspace),
        workflow_origin: None,
    }
}

#[tokio::test]
async fn runtime_executes_ask_permission_tool_after_approval_hook_allows_it() {
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
                raw_arguments: json!({ "text": "approved" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_approval_handler(|request| {
                assert_eq!(request.operation, PermissionOperation::Tool);
                permit_once(request)
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call tool"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("approved tool loop should succeed");

    assert!(events.contains(&AgentEvent::ApprovalRequested {
        request: echo_tool_approval_request("tool_1", ""),
    }));
    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["approved".to_owned()]
    );
    assert_tool_was_executed(&executed.lock().expect("lock poisoned"), true);
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::ok("approved"),
        workflow_origin: None,
        output_ref: None,
    }));
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result("tool_1", "echo", vec![Content::text("approved")], false)
    );
}

#[tokio::test]
async fn runtime_skips_ask_permission_tool_after_approval_hook_denies_it() {
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
                raw_arguments: json!({ "text": "denied" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_approval_handler(|request| {
                assert_eq!(request.operation, PermissionOperation::Tool);
                reject_action(request)
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call tool"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("denied tool loop should succeed");

    assert!(events.contains(&AgentEvent::ApprovalRequested {
        request: echo_tool_approval_request("tool_1", ""),
    }));
    assert!(executed.lock().expect("executed lock poisoned").is_empty());
    assert_tool_was_executed(&executed.lock().expect("lock poisoned"), false);
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult {
            content: "approval denied for tool: echo".to_owned(),
            media: Vec::new(),
            is_error: true,
            details: Some(serde_json::json!({
                "kind": "permission",
                "decision": "denied",
                "operation": "tool",
                "subject": "echo",
                "side_effect_occurred": false,
            })),
            terminate: false,
        },
        workflow_origin: None,
        output_ref: None,
    }));
}

pub(crate) struct AsyncEchoRuntime {
    pub(crate) runtime: AgentRuntime,
    pub(crate) executed: Arc<Mutex<Vec<String>>>,
    pub(crate) decision_sender: oneshot::Sender<ApprovalResponse>,
    pub(crate) observed_requests: Arc<Mutex<Vec<ApprovalRequest>>>,
}

fn async_echo_runtime(harness: &FakeHarness) -> AsyncEchoRuntime {
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let (decision_sender, decision_receiver) = oneshot::channel();
    let decision_receiver = Arc::new(Mutex::new(Some(decision_receiver)));
    let observed_requests = Arc::new(Mutex::new(Vec::new()));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_async_approval_handler({
                let decision_receiver = Arc::clone(&decision_receiver);
                let observed_requests = Arc::clone(&observed_requests);
                move |request| {
                    observed_requests
                        .lock()
                        .expect("observed requests lock poisoned")
                        .push(request.clone());
                    let decision_receiver = take_decision_receiver(&decision_receiver);
                    async move {
                        decision_receiver
                            .await
                            .expect("approval decision should be sent")
                    }
                }
            }),
        harness.client(),
        tools,
    );

    AsyncEchoRuntime {
        runtime,
        executed,
        decision_sender,
        observed_requests,
    }
}

fn take_decision_receiver(
    receiver: &Arc<Mutex<Option<oneshot::Receiver<ApprovalResponse>>>>,
) -> oneshot::Receiver<ApprovalResponse> {
    receiver
        .lock()
        .expect("decision receiver lock poisoned")
        .take()
        .expect("single approval decision receiver")
}

async fn collect_until_approval<S>(stream: &mut S, events: &mut Vec<AgentEvent>)
where
    S: futures::Stream<Item = Result<AgentEvent, AgentRuntimeError>> + Unpin,
{
    loop {
        let event = timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("event before approval request")
            .expect("stream should not end before approval request")
            .expect("event should be ok");
        let approval_requested = matches!(event, AgentEvent::ApprovalRequested { .. });
        events.push(event);
        if approval_requested {
            break;
        }
    }
}

async fn assert_waits_for_approval_decision<S>(stream: &mut S, action: &str)
where
    S: futures::Stream<Item = Result<AgentEvent, AgentRuntimeError>> + Unpin,
{
    assert!(
        timeout(Duration::from_millis(50), stream.next())
            .await
            .is_err(),
        "runtime should wait for the async approval decision before {action}"
    );
}

#[tokio::test]
async fn runtime_executes_ask_permission_tool_after_async_approval_wait_allows_it() {
    let harness = echo_tool_harness("async approved");
    let AsyncEchoRuntime {
        runtime,
        executed,
        decision_sender,
        observed_requests,
    } = async_echo_runtime(&harness);
    let mut context = AgentContext::new();

    let mut stream = runtime.run_turn(&mut context, AgentMessage::user_text("call tool"));
    let mut events = Vec::new();
    collect_until_approval(&mut stream, &mut events).await;

    assert_eq!(
        *observed_requests
            .lock()
            .expect("observed requests lock poisoned"),
        vec![echo_tool_approval_request("tool_1", "")]
    );
    assert!(events.contains(&AgentEvent::ApprovalRequested {
        request: echo_tool_approval_request("tool_1", ""),
    }));
    assert!(executed.lock().expect("executed lock poisoned").is_empty());
    assert_waits_for_approval_decision(&mut stream, "executing").await;

    decision_sender
        .send(ApprovalResponse::Selected {
            request_id: "tool_1".to_owned(),
            action: ApprovalAction::PermitOnce,
            feedback: None,
        })
        .expect("send allow decision");
    while let Some(event) = stream.next().await {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["async approved".to_owned()]
    );
    assert_tool_was_executed(&executed.lock().expect("lock poisoned"), true);
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::ok("async approved"),
        workflow_origin: None,
        output_ref: None,
    }));
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result(
            "tool_1",
            "echo",
            vec![Content::text("async approved")],
            false
        )
    );
}

#[tokio::test]
async fn runtime_skips_ask_permission_tool_after_async_approval_wait_denies_it() {
    let harness = echo_tool_harness("async denied");
    let AsyncEchoRuntime {
        runtime,
        executed,
        decision_sender,
        ..
    } = async_echo_runtime(&harness);
    let mut context = AgentContext::new();

    let mut stream = runtime.run_turn(&mut context, AgentMessage::user_text("call tool"));
    let mut events = Vec::new();
    collect_until_approval(&mut stream, &mut events).await;

    assert!(events.contains(&AgentEvent::ApprovalRequested {
        request: echo_tool_approval_request("tool_1", ""),
    }));
    assert!(executed.lock().expect("executed lock poisoned").is_empty());
    assert_tool_was_executed(&executed.lock().expect("lock poisoned"), false);
    assert_waits_for_approval_decision(&mut stream, "denying").await;

    decision_sender
        .send(ApprovalResponse::Selected {
            request_id: "tool_1".to_owned(),
            action: ApprovalAction::Reject,
            feedback: None,
        })
        .expect("send deny decision");
    while let Some(event) = stream.next().await {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert!(executed.lock().expect("executed lock poisoned").is_empty());
    assert_tool_was_executed(&executed.lock().expect("lock poisoned"), false);
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult {
            content: "approval denied for tool: echo".to_owned(),
            media: Vec::new(),
            is_error: true,
            details: Some(serde_json::json!({
                "kind": "permission",
                "decision": "denied",
                "operation": "tool",
                "subject": "echo",
                "side_effect_occurred": false,
            })),
            terminate: false,
        },
        workflow_origin: None,
        output_ref: None,
    }));
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result(
            "tool_1",
            "echo",
            vec![Content::text("approval denied for tool: echo")],
            true
        )
    );
}

#[tokio::test]
async fn runtime_cancels_while_waiting_for_async_approval_decision() {
    let harness = echo_tool_harness("async cancelled");
    let AsyncEchoRuntime {
        runtime,
        executed,
        decision_sender: _decision_sender,
        ..
    } = async_echo_runtime(&harness);
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();

    let mut stream = runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text("call approval-gated tool"),
        cancel.clone(),
    );
    let mut events = Vec::new();
    collect_until_approval(&mut stream, &mut events).await;

    assert!(events.contains(&AgentEvent::ApprovalRequested {
        request: echo_tool_approval_request("tool_1", ""),
    }));
    assert!(executed.lock().expect("executed lock poisoned").is_empty());

    cancel.cancel();
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("cancelled approval wait should finish promptly")
    {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult {
            content: "tool execution cancelled".to_owned(),
            media: Vec::new(),
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
    assert!(executed.lock().expect("executed lock poisoned").is_empty());
}

#[tokio::test]
async fn runtime_edit_approval_interrupt_reports_structured_zero_write_cancellation() {
    let workspace = tempfile::tempdir().expect("workspace");
    let path = workspace.path().join("file.txt");
    std::fs::write(&path, "before\n").expect("seed file");
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[(
            "edit_cancel",
            "Edit",
            json!({ "path": "file.txt", "old": "before", "new": "after" }),
        )]),
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace root")
            .with_async_approval_handler(|_request| async {
                futures::future::pending::<ApprovalResponse>().await
            }),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    let mut stream = runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text("edit after approval"),
        cancel.clone(),
    );
    let mut events = Vec::new();
    collect_until_approval(&mut stream, &mut events).await;

    cancel.cancel();
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("cancelled approval wait should finish promptly")
    {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    let result = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionFinished { id, result, .. } if id == "edit_cancel" => {
                Some(result)
            }
            _ => None,
        })
        .expect("finished Edit");
    let details = result.details.as_ref().expect("details");
    assert!(result.is_error);
    assert_eq!(details["kind"], "edit");
    assert_eq!(details["status"], "cancelled");
    assert_eq!(details["cause"], "cancelled");
    assert_eq!(details["changes"][0]["status"], "not_attempted");
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionStarted { id, .. } if id == "edit_cancel"
    )));
    assert_eq!(
        std::fs::read_to_string(path).expect("read file"),
        "before\n"
    );
}

#[tokio::test]
async fn parallel_mode_serializes_ask_approval_batches() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = parallel_write_and_glob_harness();
    let (decision_sender, decision_receiver) = oneshot::channel();
    let decision_receiver = Arc::new(Mutex::new(Some(decision_receiver)));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Parallel)
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace config")
            .with_async_approval_handler({
                let decision_receiver = Arc::clone(&decision_receiver);
                move |request| {
                    assert_eq!(request.operation, PermissionOperation::FileWrite);
                    let decision_receiver = take_decision_receiver(&decision_receiver);
                    async move {
                        decision_receiver
                            .await
                            .expect("approval decision should be sent")
                    }
                }
            }),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let mut stream = runtime.run_turn(&mut context, AgentMessage::user_text("call tools"));
    let mut events = Vec::new();
    collect_until_approval(&mut stream, &mut events).await;

    assert!(
        timeout(Duration::from_millis(250), stream.next())
            .await
            .is_err(),
        "later tools in an approval-gated batch must wait for the active approval"
    );
    assert!(
        !workspace.path().join("approved.txt").exists(),
        "approval-gated write should still be pending"
    );

    decision_sender
        .send(ApprovalResponse::Selected {
            request_id: "tool_1".to_owned(),
            action: ApprovalAction::PermitOnce,
            feedback: None,
        })
        .expect("send allow decision");
    while let Some(event) = stream.next().await {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert_eq!(
        std::fs::read_to_string(workspace.path().join("approved.txt")).expect("written file"),
        "ok"
    );
    assert!(context.messages().iter().any(|message| matches!(
        message,
        AgentMessage::ToolResult {
            tool_call_id,
            tool_name,
            is_error,
            ..
        } if tool_call_id.as_ref() == "tool_1"
            && tool_name.as_ref() == "Write"
            && !is_error
    )));
    assert!(context.messages().iter().any(|message| matches!(
        message,
        AgentMessage::ToolResult {
            tool_call_id,
            tool_name,
            content,
            is_error,
        } if tool_call_id.as_ref() == "tool_2"
            && tool_name.as_ref() == "Glob"
            && content
                .iter()
                .any(|part| matches!(part, Content::Text { text } if text.contains("Found")))
            && !is_error
    )));
}

fn parallel_write_and_glob_harness() -> FakeHarness {
    FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Write".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "path": "approved.txt", "content": "ok" }).to_string(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "Glob".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                raw_arguments: json!({ "pattern": "*" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ])
}

#[tokio::test]
async fn runtime_approval_handler_allows_file_write_tool_permission() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Write".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "path": "approved.txt", "content": "ok" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace config")
            .with_approval_handler(|request| {
                assert_eq!(request.operation, PermissionOperation::FileWrite);
                permit_once(request)
            }),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("write file"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("approved write should succeed");

    // Write now derives a reusable FileWrite scope (Layer 1). Use matches!
    // because the workspace path is dynamic (tempdir).
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ApprovalRequested { request }
            if request.id == "tool_1"
                && request.operation == PermissionOperation::FileWrite
                && request.options.iter().any(|option| matches!(
                    &option.action,
                    ApprovalAction::PermitForSession { scope }
                        if scope.label == "Approve writes to these 1 files for this session"
                            && scope.keys.len() == 1
                ))
    )));
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("approved.txt")).expect("written file"),
        "ok"
    );
}

#[tokio::test]
async fn runtime_session_approval_persists_for_same_tool() {
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
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                raw_arguments: json!({ "text": "second" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let approval_count = Arc::new(Mutex::new(0));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_approval_handler({
                let count = Arc::clone(&approval_count);
                move |request| {
                    *count.lock().expect("count lock poisoned") += 1;
                    permit_for_session(request)
                }
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("call echo twice"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    assert_eq!(
        *approval_count.lock().expect("count lock poisoned"),
        1,
        "AllowForSession should approve the same named tool for the rest of the session"
    );
    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["first".to_owned(), "second".to_owned()]
    );
}

pub(crate) fn count_approval_requests(events: &[AgentEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, AgentEvent::ApprovalRequested { .. }))
        .count()
}

pub(crate) fn first_approval_request(events: &[AgentEvent]) -> &ApprovalRequest {
    events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ApprovalRequested { request } => Some(request),
            _ => None,
        })
        .expect("expected ApprovalRequested")
}

pub(crate) async fn collect_approval_request_for_tool(
    name: &str,
    raw_arguments: serde_json::Value,
    workspace: &std::path::Path,
) -> ApprovalRequest {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: name.to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: raw_arguments.to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace)
            .expect("workspace root")
            .with_approval_handler(permit_once),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve me"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");
    first_approval_request(&events).clone()
}

#[tokio::test]
async fn approval_requests_only_offer_runtime_supported_actions() {
    let workspace = tempfile::tempdir().expect("workspace");

    let background = collect_approval_request_for_tool(
        "Bash",
        json!({ "command": "sleep 1", "run_in_background": true }),
        workspace.path(),
    )
    .await;
    assert_eq!(
        background
            .options
            .iter()
            .map(|option| &option.action)
            .collect::<Vec<_>>(),
        vec![&ApprovalAction::PermitOnce, &ApprovalAction::Reject],
    );
    assert!(matches!(
        background.presentation,
        ApprovalPresentation::Command { .. }
    ));

    let foreground = collect_approval_request_for_tool(
        "Bash",
        json!({ "command": "python script.py" }),
        workspace.path(),
    )
    .await;
    assert!(matches!(
        foreground.options.as_slice(),
        [
            ApprovalOption {
                action: ApprovalAction::PermitOnce,
                ..
            },
            ApprovalOption {
                action: ApprovalAction::PermitForSession { .. },
                ..
            },
            ApprovalOption {
                action: ApprovalAction::PermitForPrefix { .. },
                ..
            },
            ApprovalOption {
                action: ApprovalAction::Reject,
                ..
            },
        ]
    ));
    assert!(matches!(
        foreground.presentation,
        ApprovalPresentation::Command { .. }
    ));

    let write = collect_approval_request_for_tool(
        "Write",
        json!({ "path": "approved.txt", "content": "ok" }),
        workspace.path(),
    )
    .await;
    assert!(matches!(
        write.options.as_slice(),
        [
            ApprovalOption {
                action: ApprovalAction::PermitOnce,
                ..
            },
            ApprovalOption {
                action: ApprovalAction::PermitForSession { .. },
                ..
            },
            ApprovalOption {
                action: ApprovalAction::Reject,
                ..
            },
        ]
    ));
    assert!(matches!(
        write.presentation,
        ApprovalPresentation::Write { .. }
    ));
}

#[tokio::test]
async fn layer1_bash_session_approval_exact_command_only() {
    // Approving `git status` must NOT cover `git log`. Core regression test.
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "command": "git status" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                raw_arguments: json!({ "command": "python script.py" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let approval_count = Arc::new(Mutex::new(0));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace root")
            .with_approval_handler({
                let count = Arc::clone(&approval_count);
                move |request| {
                    *count.lock().expect("count lock poisoned") += 1;
                    permit_for_session(request)
                }
            }),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("git status then git log"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");
    // `git status` is auto-approved by Layer 3 (safe git subcommand), so only
    // `git log --oneline -20` reaches the handler. This proves the safe-command
    // path + that different commands don't share approval.
    assert!(
        count_approval_requests(&events) <= 1,
        "git status (safe) auto-approves; git log is a different command and must not inherit"
    );
}
