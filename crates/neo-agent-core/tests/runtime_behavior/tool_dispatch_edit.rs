use super::compaction::end_turn_events;
use super::fake_harness::EchoTool;
use super::fake_harness::final_done_turn;
use super::fake_harness::tool_call_turn;
use super::permissions::permit_once;
use super::permissions_scope::permit_for_session;
use super::plan_and_goal::set_config_permission_mode;
use super::tool_dispatch::edit_arguments;
use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, ApprovalAction,
    ApprovalPresentation, PermissionMode, PermissionOperation, Tool, ToolContext, ToolFuture,
    ToolRegistry, ToolResult,
    harness::{FakeHarness, fake_model},
    runtime::WorkflowDispatchHandle,
    session::{main_agent_plans_dir, workspace_sessions_dir},
    workflow::WorkflowInvocationContext,
};
use neo_ai::{AiStreamEvent, MessagePhase};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::timeout;

struct StoreWorkflowDispatchHandleTool {
    slot: Arc<Mutex<Option<WorkflowDispatchHandle>>>,
}

impl Tool for StoreWorkflowDispatchHandleTool {
    fn name(&self) -> &'static str {
        "StoreWorkflowDispatchHandle"
    }

    fn description(&self) -> &'static str {
        "Stores a workflow dispatch handle for a later natural turn."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let handle = WorkflowDispatchHandle {
                config: ctx.child_config.clone().expect("runtime config"),
                model_client: ctx.child_model.clone().expect("runtime model"),
                registry: ctx.child_tools.clone().expect("runtime tools"),
                process_supervisor: ctx.process_supervisor.clone(),
                context: AgentContext::new(),
            };
            *self.slot.lock().expect("dispatch slot") = Some(handle);
            Ok(ToolResult::ok("stored"))
        })
    }
}

struct InvokeStoredWorkflowDispatchHandleTool {
    slot: Arc<Mutex<Option<WorkflowDispatchHandle>>>,
}

impl Tool for InvokeStoredWorkflowDispatchHandleTool {
    fn name(&self) -> &'static str {
        "InvokeStoredWorkflowDispatchHandle"
    }

    fn description(&self) -> &'static str {
        "Invokes a stored workflow dispatch handle."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let handle = self
                .slot
                .lock()
                .expect("dispatch slot")
                .clone()
                .expect("stored workflow dispatch handle");
            let outcome = handle
                .run_one(
                    WorkflowInvocationContext {
                        invocation_id: "nested_turn_two".to_owned(),
                        cancel_token: ctx.cancel_token.clone(),
                    },
                    "NestedWorkflowEcho",
                    json!({}),
                )
                .await;
            if outcome.is_completed() {
                Ok(ToolResult::ok(outcome.summary))
            } else {
                Ok(ToolResult::error(outcome.summary))
            }
        })
    }
}

struct NestedWorkflowEchoTool;

impl Tool for NestedWorkflowEchoTool {
    fn name(&self) -> &'static str {
        "NestedWorkflowEcho"
    }

    fn description(&self) -> &'static str {
        "Completes one nested canonical workflow call."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async { Ok(ToolResult::ok("nested complete")) })
    }
}

#[tokio::test]
async fn runtime_invalid_tool_arguments_return_model_visible_error() {
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
                raw_arguments: r#"{"text":"neo"#.into(),
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
                text: "retrying".to_owned(),
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
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call echo"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    // 1. No ToolExecutionStarted — execution never begins for invalid args.
    assert!(
        !events.iter().any(
            |event| matches!(event, AgentEvent::ToolExecutionStarted { name, .. } if name == "echo")
        ),
        "invalid tool arguments must not start execution"
    );

    // 2. A ToolExecutionFinished with an error result is emitted.
    let error_event = events.iter().find(|event| {
        matches!(
            event,
            AgentEvent::ToolExecutionFinished { name, result, .. }
                if name == "echo" && result.is_error
        )
    });
    let error_event = error_event.expect("expected a ToolExecutionFinished error event");
    if let AgentEvent::ToolExecutionFinished { result, .. } = error_event {
        assert!(
            result.content.contains("Tool arguments were invalid JSON"),
            "error content should mention invalid JSON, got: {}",
            result.content
        );
    }

    // 3. The model gets a second turn (error is fed back).
    assert_eq!(harness.requests().len(), 2);

    // 4. The second request's messages end with a ToolResult containing the error.
    let requests = harness.requests();
    let last_message = requests[1].messages.last();
    assert!(
        matches!(
            last_message,
            Some(neo_ai::ChatMessage::ToolResult { content, is_error, .. })
                if *is_error
                    && content.iter().any(|part| matches!(part,
                        neo_ai::ContentPart::Text { text } if text.contains("invalid JSON")
                    ))
        ),
        "second request should end with an error ToolResult"
    );
}

#[tokio::test]
async fn runtime_edit_approval_uses_verified_single_file_projection() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::create_dir_all(workspace.path().join("src")).expect("mkdir");
    std::fs::write(workspace.path().join("src/a.txt"), "aaa\n").expect("a");
    let args = edit_arguments("src/a.txt", "aaa", "AAA");
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[("edit_1", "Edit", args)]),
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace")
            .with_approval_handler(|request| {
                assert_eq!(request.operation, PermissionOperation::FileWrite);
                match &request.presentation {
                    ApprovalPresentation::Edit { title, edit } => {
                        assert_eq!(title, "Edit 1 files?");
                        assert_eq!(edit.files, 1);
                        assert_eq!(edit.replacements, 1);
                        assert_eq!(edit.changes.len(), 1);
                    }
                    other => panic!("expected Edit presentation, got {other:?}"),
                }
                assert!(request.options.iter().any(|option| matches!(
                    &option.action,
                    ApprovalAction::PermitForSession { scope }
                        if scope.keys.len() == 1
                )));
                permit_once(request)
            }),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("edit files"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn");

    let finished = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionFinished {
                id, name, result, ..
            } if id == "edit_1" && name == "Edit" => Some(result),
            _ => None,
        })
        .expect("finished edit");
    assert!(!finished.is_error, "{}", finished.content);
    let details = finished.details.as_ref().expect("details");
    assert_eq!(details["status"], "committed");
    assert_eq!(details["files"], 1);
    let success_diff = details["changes"][0]["diff"].as_str().expect("diff");
    let approval = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ApprovalRequested { request } => Some(request),
            _ => None,
        })
        .expect("approval");
    match &approval.presentation {
        ApprovalPresentation::Edit { edit, .. } => {
            assert_eq!(edit.changes[0].diff, success_diff);
        }
        other => panic!("expected Edit approval, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("src/a.txt")).expect("a"),
        "AAA\n"
    );
}

#[tokio::test]
async fn runtime_edit_stale_after_approval_writes_nothing() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("stale.txt"), "before\n").expect("seed");
    let args = edit_arguments("stale.txt", "before", "after");
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[("edit_stale", "Edit", args)]),
        final_done_turn(),
    ]);
    let path = workspace.path().join("stale.txt");
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace")
            .with_approval_handler({
                let path = path.clone();
                move |request| {
                    // Mutate the file after the verified preparation/approval projection.
                    std::fs::write(&path, "changed externally\n").expect("stale write");
                    permit_once(request)
                }
            }),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("edit stale"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn");

    let finished = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionFinished { id, result, .. } if id == "edit_stale" => {
                Some(result)
            }
            _ => None,
        })
        .expect("finished");
    assert!(finished.is_error);
    let details = finished.details.as_ref().expect("details");
    assert_eq!(details["status"], "stale");
    assert!(
        !events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionUpdate {
                partial_result,
                ..
            } if partial_result.details.as_ref().is_some_and(|d| d.get("kind") == Some(&json!("edit_progress")))
        )),
        "no commit progress after stale"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        "changed externally\n"
    );
}

#[tokio::test]
async fn runtime_edit_emits_prepared_then_progress_update() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::create_dir_all(workspace.path().join("src")).expect("mkdir");
    std::fs::write(workspace.path().join("src/a.txt"), "aaa\n").expect("a");
    let args = edit_arguments("src/a.txt", "aaa", "AAA");
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[("edit_prog", "Edit", args)]),
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_workspace_root(workspace.path())
            .expect("workspace"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("edit"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn");

    let edit_events: Vec<_> = events
        .iter()
        .filter(|event| match event {
            AgentEvent::ToolExecutionStarted { id, .. }
            | AgentEvent::ToolExecutionUpdate { id, .. }
            | AgentEvent::ToolExecutionFinished { id, .. } => id == "edit_prog",
            _ => false,
        })
        .collect();
    assert!(
        matches!(
            edit_events.first(),
            Some(AgentEvent::ToolExecutionStarted { .. })
        ),
        "started first: {edit_events:?}"
    );
    let kinds: Vec<_> = edit_events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolExecutionUpdate { partial_result, .. } => partial_result
                .details
                .as_ref()
                .and_then(|d| d.get("kind"))
                .and_then(|k| k.as_str())
                .map(str::to_owned),
            _ => None,
        })
        .collect();
    assert_eq!(kinds.first().map(String::as_str), Some("edit_prepared"));
    assert!(kinds.iter().any(|k| k == "edit_progress"));
    assert!(
        matches!(
            edit_events.last(),
            Some(AgentEvent::ToolExecutionFinished { .. })
        ),
        "finished last"
    );
}

fn write_arguments(path: &str, content: &str) -> serde_json::Value {
    json!({ "path": path, "content": content })
}

#[tokio::test]
async fn runtime_write_approval_uses_verified_single_file_projection() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("existing.txt"), "old\n").expect("seed");
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[
            (
                "write_existing",
                "Write",
                write_arguments("existing.txt", "new content\n"),
            ),
            (
                "write_created",
                "Write",
                write_arguments("created.txt", "fresh\n"),
            ),
        ]),
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace")
            .with_approval_handler(|request| {
                assert_eq!(request.operation, PermissionOperation::FileWrite);
                match &request.presentation {
                    ApprovalPresentation::Write { title, write } => {
                        assert_eq!(title, "Write 1 files?");
                        assert_eq!(write.files, 1);
                        assert_eq!(write.created + write.overwritten, 1);
                        assert_eq!(write.changes.len(), 1);
                    }
                    other => panic!("expected Write presentation, got {other:?}"),
                }
                assert!(request.options.iter().any(|option| matches!(
                    &option.action,
                    ApprovalAction::PermitForSession { scope }
                        if scope.keys.len() == 1
                )));
                permit_once(request)
            }),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("write files"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn");

    for id in ["write_existing", "write_created"] {
        let finished = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ToolExecutionFinished {
                    id: event_id,
                    name,
                    result,
                    ..
                } if event_id == id && name == "Write" => Some(result),
                _ => None,
            })
            .expect("finished write");
        assert!(!finished.is_error, "{}", finished.content);
        assert_eq!(finished.details.as_ref().expect("details")["files"], 1);
    }
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("existing.txt")).expect("read"),
        "new content\n"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("created.txt")).expect("read"),
        "fresh\n"
    );
}

#[tokio::test]
async fn runtime_write_stale_existing_and_appeared_target_install_nothing() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("stale.txt"), "before\n").expect("seed");
    let args = write_arguments("stale.txt", "planned overwrite\n");
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[("write_stale", "Write", args)]),
        final_done_turn(),
    ]);
    let path = workspace.path().join("stale.txt");
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace")
            .with_approval_handler({
                let path = path.clone();
                move |request| {
                    std::fs::write(&path, "changed externally\n").expect("stale");
                    permit_once(request)
                }
            }),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("write stale"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn");

    let finished = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionFinished { id, result, .. } if id == "write_stale" => {
                Some(result)
            }
            _ => None,
        })
        .expect("finished");
    assert!(finished.is_error);
    let details = finished.details.as_ref().expect("details");
    assert_eq!(details["status"], "stale");
    assert!(
        !events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionUpdate {
                partial_result,
                ..
            } if partial_result.details.as_ref().is_some_and(|d| d.get("kind") == Some(&json!("write_progress")))
        )),
        "no commit progress after stale"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        "changed externally\n"
    );
    let workspace = tempfile::tempdir().expect("appeared workspace");
    let appeared = workspace.path().join("appeared.txt");
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[(
            "write_appeared",
            "Write",
            write_arguments("appeared.txt", "planned create\n"),
        )]),
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace")
            .with_approval_handler({
                let appeared = appeared.clone();
                move |request| {
                    std::fs::write(&appeared, "appeared externally\n").expect("appeared");
                    permit_once(request)
                }
            }),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("write appeared"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn");
    let finished = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionFinished { id, result, .. } if id == "write_appeared" => {
                Some(result)
            }
            _ => None,
        })
        .expect("finished");
    assert!(finished.is_error);
    assert_eq!(
        finished.details.as_ref().expect("details")["status"],
        "stale"
    );
    assert_eq!(
        std::fs::read_to_string(&appeared).expect("read"),
        "appeared externally\n"
    );
}

#[tokio::test]
async fn runtime_multiple_write_calls_each_emit_prepared_and_progress_updates() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[
            ("write_a", "Write", write_arguments("a.txt", "aaa\n")),
            ("write_b", "Write", write_arguments("b.txt", "bbb\n")),
        ]),
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_workspace_root(workspace.path())
            .expect("workspace"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("write"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn");

    for id in ["write_a", "write_b"] {
        let kinds = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolExecutionUpdate {
                    id: event_id,
                    partial_result,
                    ..
                } if event_id == id => partial_result
                    .details
                    .as_ref()
                    .and_then(|details| details.get("kind"))
                    .and_then(|kind| kind.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(kinds, vec!["write_prepared", "write_progress"]);
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionFinished { id: event_id, result, .. }
                if event_id == id && !result.is_error
        )));
    }
}

#[tokio::test]
async fn runtime_plan_mode_allows_only_single_active_plan_write_target() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace.path().canonicalize().expect("workspace");
    let plans_dir = main_agent_plans_dir(&workspace_sessions_dir(
        &home.path().join("sessions"),
        &workspace_root,
    ));
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root.clone());
    set_config_permission_mode(&mut config, PermissionMode::Yolo);
    let plan_path = {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let data = pm.enter(&plans_dir, true).expect("enter plan mode");
        std::fs::write(&data.path, "draft\n").expect("seed plan");
        data.path
    };
    let plan_mode = Arc::clone(&config.plan_mode);
    let extra = workspace_root.join("extra.txt");

    let harness = FakeHarness::from_turns([
        tool_call_turn(&[(
            "write_single",
            "Write",
            json!({ "path": plan_path, "content": "# Final plan\n" }),
        )]),
        tool_call_turn(&[
            (
                "write_plan_again",
                "Write",
                json!({ "path": plan_path, "content": "# Changed\n" }),
            ),
            (
                "write_extra",
                "Write",
                json!({ "path": extra, "content": "should not appear\n" }),
            ),
        ]),
        final_done_turn(),
    ]);
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("plan writes"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn");

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionFinished { id, result, .. }
                if id == "write_single" && !result.is_error
        )),
        "single plan-file write succeeds"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionFinished { id, result, .. }
                if id == "write_extra" && result.is_error && result.content.contains("plan mode")
        )),
        "non-plan target denied by plan guard"
    );
    assert!(!extra.exists(), "extra file must not be created");
    assert_eq!(
        std::fs::read_to_string(&plan_path).expect("read plan"),
        "# Changed\n"
    );
    assert!(plan_mode.read().expect("lock").is_active());
}

#[tokio::test]
async fn runtime_write_session_scope_is_bound_to_one_prepared_target() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[("w1", "Write", write_arguments("a.txt", "a1\n"))]),
        tool_call_turn(&[("w2", "Write", write_arguments("a.txt", "a2\n"))]),
        tool_call_turn(&[("w3", "Write", write_arguments("c.txt", "c3\n"))]),
        final_done_turn(),
    ]);
    let approval_count = Arc::new(Mutex::new(0usize));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace")
            .with_approval_handler({
                let count = Arc::clone(&approval_count);
                move |request| {
                    *count.lock().expect("count") += 1;
                    permit_for_session(request)
                }
            }),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("session scope"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn");

    assert_eq!(
        *approval_count.lock().expect("count"),
        2,
        "turn 2 reuses the a.txt grant; turn 3 introduces c.txt"
    );
    for id in ["w1", "w2", "w3"] {
        assert!(
            events.iter().any(|event| matches!(
                event,
                AgentEvent::ToolExecutionFinished { id: eid, result, .. }
                    if eid == id && !result.is_error
            )),
            "{id} committed"
        );
    }
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("a.txt")).expect("a"),
        "a2\n"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("c.txt")).expect("c"),
        "c3\n"
    );
}

#[tokio::test]
async fn stored_workflow_handle_routes_nested_events_only_to_the_active_turn() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[("store_dispatch", "StoreWorkflowDispatchHandle", json!({}))]),
        end_turn_events("stored"),
        tool_call_turn(&[(
            "invoke_dispatch",
            "InvokeStoredWorkflowDispatchHandle",
            json!({}),
        )]),
        end_turn_events("invoked"),
    ]);
    let slot = Arc::new(Mutex::new(None));
    let mut registry = ToolRegistry::new();
    registry.register(StoreWorkflowDispatchHandleTool {
        slot: Arc::clone(&slot),
    });
    registry.register(InvokeStoredWorkflowDispatchHandleTool {
        slot: Arc::clone(&slot),
    });
    registry.register(NestedWorkflowEchoTool);
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(workspace.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo);
    let runtime = AgentRuntime::with_tools(config, harness.client(), registry);
    let mut context = AgentContext::new();

    let turn_one = timeout(
        Duration::from_secs(5),
        runtime
            .run_turn(&mut context, AgentMessage::user_text("store handle"))
            .collect::<Vec<_>>(),
    )
    .await
    .expect("turn one stream closes while handle remains stored")
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .expect("turn one succeeds");
    assert!(slot.lock().expect("dispatch slot").is_some());
    assert!(!turn_one.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionStarted { id, .. }
            | AgentEvent::ToolExecutionFinished { id, .. }
            if id == "nested_turn_two"
    )));

    let turn_two = timeout(
        Duration::from_secs(5),
        runtime
            .run_turn(&mut context, AgentMessage::user_text("invoke handle"))
            .collect::<Vec<_>>(),
    )
    .await
    .expect("turn two stream closes after nested completion")
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .expect("turn two succeeds");
    let active_turn = turn_two
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionStarted { turn, id, .. } if id == "invoke_dispatch" => {
                Some(*turn)
            }
            _ => None,
        })
        .expect("turn two outer tool start");
    assert!(turn_two.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionStarted { turn, id, name, .. }
            if *turn == active_turn
                && id == "nested_turn_two"
                && name == "NestedWorkflowEcho"
    )));
    assert!(turn_two.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished { turn, id, name, .. }
            if *turn == active_turn
                && id == "nested_turn_two"
                && name == "NestedWorkflowEcho"
    )));
}
