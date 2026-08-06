use super::compaction::end_turn_events;
use super::compaction::text_turn_events;
use super::context::finished_tool_results;
use super::fake_harness::EchoTool;
use super::fake_harness::RecordingEchoTool;
use super::fake_harness::echo_tool_harness;
use super::fake_harness::final_done_turn;
use super::fake_harness::tool_call_turn;
use super::permissions::echo_tool_approval_request;
use super::permissions::permit_once;
use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, ApprovalAction,
    ApprovalPresentation, ApprovalRequest, AskUserTool, Content, PermissionMode,
    PermissionOperation, ShellCommandOrigin, ShellCommandOutcome, SkillInvocationOutcome,
    SkillInvocationSource, Tool, ToolContext, ToolFuture, ToolRegistry, ToolResult,
    harness::FakeHarness, skills::SkillStore,
};
use neo_ai::{AiStreamEvent, MessagePhase};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;

pub(crate) fn two_echo_tool_turns() -> FakeHarness {
    FakeHarness::from_turns([
        tool_call_turn(&[("tool_1", "echo", json!({ "text": "first" }))]),
        tool_call_turn(&[("tool_2", "echo", json!({ "text": "second" }))]),
        text_turn_events("msg_3", "done"),
    ])
}

#[tokio::test]
async fn live_permission_switch_to_auto_skips_approval_for_later_tool_calls() {
    // One turn with two model ToolUse round-trips. The first echo requires
    // approval in Ask mode. The approval handler switches the shared live mode
    // to Auto while granting this first call; the second echo must therefore
    // run WITHOUT a second ApprovalRequested event.
    let harness = two_echo_tool_turns();
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let live_mode = Arc::new(std::sync::RwLock::new(PermissionMode::Ask));
    let live_for_handler = Arc::clone(&live_mode);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_live_permission_mode(Arc::clone(&live_mode))
            .with_approval_handler(move |request| {
                // Flip the live mode to Auto before returning so the second tool
                // call is prepared under Auto and must not request approval again.
                if let Ok(mut mode) = live_for_handler.write() {
                    *mode = PermissionMode::Auto;
                }
                permit_once(request)
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call tools"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("live-switch tool loop should succeed");

    let first_approval = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ApprovalRequested { request } if request.id == "tool_1"
        )
    });
    let second_approval = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ApprovalRequested { request } if request.id == "tool_2"
        )
    });
    assert!(
        first_approval,
        "first call should request approval under Ask"
    );
    assert!(
        !second_approval,
        "second call should NOT request approval after live switch to Auto"
    );
    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["first".to_owned(), "second".to_owned()]
    );
    assert_eq!(*live_mode.read().unwrap(), PermissionMode::Auto);
}

#[tokio::test]
async fn live_permission_switch_to_ask_requests_approval_for_later_tool_calls() {
    // Inverse of the above: start Auto (no approval), flip live mode to Ask
    // mid-turn via the async after-tool hook, and the second generic tool call
    // must request approval.
    let harness = two_echo_tool_turns();
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let live_mode = Arc::new(std::sync::RwLock::new(PermissionMode::Auto));
    let live_for_hook = Arc::clone(&live_mode);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Auto)
            .with_live_permission_mode(Arc::clone(&live_mode))
            .with_async_after_tool_call(move |_call, result, _cancel| {
                let live = Arc::clone(&live_for_hook);
                async move {
                    if let Ok(mut mode) = live.write() {
                        *mode = PermissionMode::Ask;
                    }
                    result
                }
            })
            .with_approval_handler(permit_once),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call tools"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("live-switch tool loop should succeed");

    let first_approval = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ApprovalRequested { request } if request.id == "tool_1"
        )
    });
    let second_approval = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ApprovalRequested { request } if request.id == "tool_2"
        )
    });
    assert!(
        !first_approval,
        "first call should NOT request approval under Auto"
    );
    assert!(
        second_approval,
        "second call should request approval after live switch to Ask"
    );
    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["first".to_owned(), "second".to_owned()]
    );
}

#[tokio::test]
async fn ask_mode_ask_user_question_dispatches_without_approval() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "AskUserQuestion".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({
                    "questions": [{
                        "question": "Which language?",
                        "options": [
                            { "label": "Rust" },
                            { "label": "TypeScript" }
                        ]
                    }]
                })
                .to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let (question_tx, mut question_rx) = mpsc::unbounded_channel();
    let mut tools = ToolRegistry::new();
    tools.register(neo_agent_core::AskUserTool::new(question_tx));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Ask),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let stream = runtime.run_turn(&mut context, AgentMessage::user_text("ask user"));
    let pending = timeout(Duration::from_millis(250), question_rx.recv())
        .await
        .expect("ask mode should dispatch AskUserQuestion to the host")
        .expect("question should be pending");
    assert_eq!(pending.questions[0].question, "Which language?");

    pending
        .response_tx
        .send(neo_agent_core::QuestionResponse {
            answers: vec!["Rust".to_owned()],
        })
        .expect("send question response");
    let events = stream
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    assert!(
        events
            .iter()
            .all(|event| !matches!(event, AgentEvent::ApprovalRequested { .. })),
        "AskUserQuestion must not be wrapped in the approval dialog"
    );
}

#[tokio::test]
async fn ask_mode_skill_tool_runs_without_approval() {
    let skills_dir = tempfile::tempdir().expect("skills dir");
    std::fs::write(
        skills_dir.path().join("SKILL.md"),
        r"---
name: review
description: Review the current change.
---
Review the current change carefully.
",
    )
    .expect("write skill");
    let skill_store = SkillStore::load(&[], &[skills_dir.path().to_path_buf()], Vec::new());

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Skill".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({"skill": "review"}).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools_and_skills(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Ask),
        harness.client(),
        ToolRegistry::new(),
        skill_store,
    );
    let mut context = AgentContext::new();

    let events = timeout(
        Duration::from_secs(2),
        runtime
            .run_turn(&mut context, AgentMessage::user_text("use review skill"))
            .collect::<Vec<_>>(),
    )
    .await
    .expect("skill turn should finish")
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .expect("skill tool should run");

    assert!(
        events
            .iter()
            .all(|event| !matches!(event, AgentEvent::ApprovalRequested { .. })),
        "Skill must not be wrapped in the approval dialog"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionFinished { name, result, .. }
                if name == "Skill"
                    && !result.is_error
                    && result.content.contains("Review the current change carefully.")
        )),
        "Skill should execute successfully; events: {events:#?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::SkillInvocation {
                names,
                source: SkillInvocationSource::Auto,
                outcome: SkillInvocationOutcome::Activated,
                body,
            } if names == &["review".to_owned()] && body.is_empty()
        )),
        "Skill should emit one semantic activation event; events: {events:#?}"
    );
}

#[tokio::test]
async fn runtime_yolo_mode_auto_approves_custom_tool() {
    let harness = echo_tool_harness("yolo approved");
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
        .expect("tool loop should succeed");

    assert!(
        events
            .iter()
            .all(|event| !matches!(event, AgentEvent::ApprovalRequested { .. })),
        "yolo mode should not request approvals"
    );
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result(
            "tool_1",
            "echo",
            vec![Content::text("yolo approved")],
            false
        )
    );
}

#[tokio::test]
async fn runtime_auto_mode_denies_ask_user_question() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "AskUserQuestion".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({
                    "questions": [{
                        "question": "Continue?",
                        "options": [{ "label": "Yes" }, { "label": "No" }]
                    }]
                })
                .to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let (question_tx, mut question_rx) = mpsc::unbounded_channel();
    let mut tools = ToolRegistry::new();
    tools.register(AskUserTool::new(question_tx));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Auto),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("ask user"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "AskUserQuestion".to_owned(),
        result: ToolResult::error(
            "AskUserQuestion is disabled while auto permission mode is active"
        ),
        workflow_origin: None,
        output_ref: None,
    }));
    assert!(
        question_rx.try_recv().is_err(),
        "no question should be dispatched in auto mode"
    );
}

#[tokio::test]
async fn runtime_ask_mode_read_runs_and_custom_tool_asks() {
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_key = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace")
        .display()
        .to_string();
    std::fs::write(workspace.path().join("file.txt"), "hello").expect("seed file");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Read".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "path": "file.txt" }).to_string(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                raw_arguments: json!({ "text": "needs approval" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let mut tools = ToolRegistry::with_builtin_tools();
    tools.register(EchoTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("read and call"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionFinished {
                id,
                name,
                result,
                ..
            } if id == "tool_1" && name == "Read" && result.content.contains("hello")
        )),
        "Read should run without approval in ask mode"
    );
    assert!(events.contains(&AgentEvent::ApprovalRequested {
        request: echo_tool_approval_request("tool_2", workspace_key),
    }));
}

#[tokio::test]
async fn ask_mode_asks_for_bash() {
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
                raw_arguments: json!({ "command": "mkdir test_dir" }).to_string(),
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
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run bash"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    // `mkdir test_dir` is NOT a known-safe command (mkdir isn't in the safe
    // list), so it must prompt. Use matches! because the scope carries a
    // dynamic workspace path.
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ApprovalRequested { request }
            if request.id == "tool_1"
                && request.operation == PermissionOperation::Shell
                && matches!(
                    &request.presentation,
                    ApprovalPresentation::Command { command, .. }
                        if command == "mkdir test_dir"
                )
                && request.options.iter().any(|option| {
                    matches!(option.action, ApprovalAction::PermitForSession { .. })
                })
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            id,
            name,
            result,
            ..
        } if id == "tool_1" && name == "Bash" && result.content.contains("approval required")
    )));
}

#[tokio::test]
async fn auto_mode_approves_bash_without_approval() {
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
                raw_arguments: json!({ "command": "printf auto-ok" }).to_string(),
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
            .with_permission_mode(PermissionMode::Auto)
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run bash"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::ApprovalRequested { .. })),
        "auto mode should not request bash approval"
    );
    assert!(events.contains(&AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        exit_code: Some(0),
        signal: None,
        stdout: "auto-ok".to_owned(),
        stderr: String::new(),
        truncated: false,
        origin: ShellCommandOrigin::ModelBashTool,
        outcome: ShellCommandOutcome::Completed,
        output_ref: None,
    }));
}

#[tokio::test]
async fn yolo_mode_approves_write_without_approval() {
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
                raw_arguments: json!({ "path": "yolo.txt", "content": "yolo" }).to_string(),
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
            .with_permission_mode(PermissionMode::Yolo)
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
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
        .expect("turn should succeed");

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::ApprovalRequested { .. })),
        "yolo mode should not request write approval"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("yolo.txt")).expect("written file"),
        "yolo"
    );
}

#[derive(Clone)]
struct ThemeDraftProbe {
    executed: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl Tool for ThemeDraftProbe {
    fn name(&self) -> &'static str {
        "ThemeDraft"
    }

    fn description(&self) -> &'static str {
        "probe ThemeDraft"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        let executed = Arc::clone(&self.executed);
        Box::pin(async move {
            executed.lock().expect("executed").push(input);
            Ok(ToolResult::ok("probe ok"))
        })
    }
}

fn theme_draft_probe_runtime(
    harness: &FakeHarness,
    mode: PermissionMode,
) -> (AgentRuntime, ThemeDraftProbe) {
    let probe = ThemeDraftProbe {
        executed: Arc::new(Mutex::new(Vec::new())),
    };
    let mut tools = ToolRegistry::new();
    tools.register(probe.clone());
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(mode),
        harness.client(),
        tools,
    );
    (runtime, probe)
}

async fn run_theme_draft_turn(
    runtime: AgentRuntime,
    arguments: serde_json::Value,
) -> Vec<AgentEvent> {
    let mut context = AgentContext::new();
    runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text(format!(
                "theme draft call: {}",
                serde_json::to_string(&arguments).expect("json")
            )),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("theme draft turn should succeed")
}

fn theme_draft_approval_events(events: &[AgentEvent]) -> Vec<&ApprovalRequest> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ApprovalRequested { request } => Some(request),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn theme_draft_save_requires_typed_theme_save_approval_with_no_session_grant() {
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[(
            "call_td",
            "ThemeDraft",
            json!({"action": "save", "draft_id": "draft-0001", "overwrite": false}),
        )]),
        end_turn_events("done"),
    ]);
    let (runtime, probe) = theme_draft_probe_runtime(&harness, PermissionMode::Ask);
    let events = run_theme_draft_turn(runtime, json!({"action": "save"})).await;

    let approvals = theme_draft_approval_events(&events);
    assert_eq!(
        approvals.len(),
        1,
        "exactly one approval request: {events:?}"
    );
    let request = approvals[0];
    assert_eq!(request.operation, PermissionOperation::ThemeSave);
    assert_eq!(request.id, "call_td");
    let offered = request
        .options
        .iter()
        .map(|option| &option.action)
        .collect::<Vec<_>>();
    assert!(
        offered
            .iter()
            .any(|action| **action == ApprovalAction::PermitOnce),
        "Approve once must be offered: {offered:?}"
    );
    assert!(
        !offered
            .iter()
            .any(|action| matches!(action, ApprovalAction::PermitForSession { .. })),
        "ThemeSave must never offer a session-wide grant: {offered:?}"
    );

    // Without a handler the save is terminal with the typed permission error
    // and the probe never executed.
    assert_eq!(probe.executed.lock().unwrap().len(), 0);
    let finished = finished_tool_results(&events, "call_td");
    assert_eq!(finished.len(), 1);
    assert!(finished[0].is_error);
    assert!(
        finished[0]
            .content
            .contains("approval required for theme save")
    );
}

#[tokio::test]
async fn theme_draft_preview_runs_without_any_approval_in_ask_mode() {
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[(
            "call_td",
            "ThemeDraft",
            json!({"action": "preview", "name": "Aurora Night"}),
        )]),
        end_turn_events("done"),
    ]);
    let (runtime, probe) = theme_draft_probe_runtime(&harness, PermissionMode::Ask);
    let events = run_theme_draft_turn(runtime, json!({"action": "preview"})).await;

    assert!(
        theme_draft_approval_events(&events).is_empty(),
        "preview must not require approval: {events:?}"
    );
    let executed = probe.executed.lock().unwrap();
    assert_eq!(executed.len(), 1, "preview must reach execution");
    assert_eq!(executed[0]["action"], "preview");
}

#[tokio::test]
async fn theme_draft_save_executes_directly_in_auto_mode() {
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[(
            "call_td",
            "ThemeDraft",
            json!({"action": "save", "draft_id": "draft-0001"}),
        )]),
        end_turn_events("done"),
    ]);
    let (runtime, probe) = theme_draft_probe_runtime(&harness, PermissionMode::Auto);
    let events = run_theme_draft_turn(runtime, json!({"action": "save"})).await;

    assert!(
        theme_draft_approval_events(&events).is_empty(),
        "auto mode must not prompt for ThemeDraft save: {events:?}"
    );
    assert_eq!(probe.executed.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn theme_draft_save_is_denied_in_plan_mode_while_preview_runs() {
    for (action, should_run) in [("save", false), ("preview", true)] {
        let harness = FakeHarness::from_turns([
            tool_call_turn(&[("call_td", "ThemeDraft", json!({"action": action}))]),
            end_turn_events("done"),
        ]);
        let (runtime, probe) = theme_draft_probe_runtime(&harness, PermissionMode::Ask);
        runtime
            .config()
            .plan_mode
            .write()
            .expect("plan mode lock")
            .enter_in_memory();
        let events = run_theme_draft_turn(runtime, json!({"action": action})).await;

        assert_eq!(
            probe.executed.lock().unwrap().len(),
            usize::from(should_run),
            "action {action} execution mismatch: {events:?}"
        );
        if should_run {
            assert!(
                theme_draft_approval_events(&events).is_empty(),
                "preview must not prompt in plan mode: {events:?}"
            );
        } else {
            let finished = finished_tool_results(&events, "call_td");
            assert_eq!(finished.len(), 1, "{events:?}");
            assert!(
                finished[0].content.contains("blocked by plan mode"),
                "save must be denied in plan mode: {}",
                finished[0].content
            );
        }
    }
}
