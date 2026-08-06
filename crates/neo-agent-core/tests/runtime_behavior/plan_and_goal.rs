use super::context::finished_tool_results;
use super::fake_harness::final_done_turn;
use super::fake_harness::run_turn_collect;
use super::fake_harness::tool_call_turn;
use super::permissions::permit_once;
use super::permissions::select_action;
use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, ApprovalAction,
    ApprovalPresentation, ApprovalRequest, ApprovalResponse, PermissionMode, PermissionOperation,
    ToolRegistry,
    harness::{FakeHarness, fake_model},
    session::{main_agent_plans_dir, workspace_sessions_dir},
};
use neo_ai::{AiStreamEvent, MessagePhase};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

fn first_offered_action(request: &ApprovalRequest) -> ApprovalResponse {
    let action = request
        .options
        .first()
        .map(|option| option.action.clone())
        .expect("approval options");
    select_action(request, action)
}

fn approve_plan(request: &ApprovalRequest) -> ApprovalResponse {
    let action = request
        .options
        .iter()
        .find_map(|option| match &option.action {
            ApprovalAction::ApprovePlan { .. } => Some(option.action.clone()),
            _ => None,
        })
        .expect("ApprovePlan option");
    select_action(request, action)
}

fn approve_plan_with_label(request: &ApprovalRequest, label: &str) -> ApprovalResponse {
    let action = request
        .options
        .iter()
        .find_map(|option| match &option.action {
            ApprovalAction::ApprovePlan {
                selection: Some(selection),
            } if selection.label == label => Some(option.action.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("ApprovePlan selection {label:?}"));
    select_action(request, action)
}

fn reject_plan(request: &ApprovalRequest) -> ApprovalResponse {
    select_action(request, ApprovalAction::RejectPlan)
}

pub(crate) fn set_config_permission_mode(config: &mut AgentConfig, mode: PermissionMode) {
    config.permission_mode = mode;
    if let Ok(mut live) = config.live_permission_mode.write() {
        *live = mode;
    }
}

#[tokio::test]
async fn enter_plan_mode_continues_model_loop_after_mode_switch() {
    let home = tempfile::tempdir().expect("home dir");
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "EnterPlanMode".to_owned(),
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
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "continuing plan".to_owned(),
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
            .with_home_dir(home.path())
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = timeout(
        Duration::from_secs(2),
        runtime
            .run_turn(&mut context, AgentMessage::user_text("make a plan"))
            .collect::<Vec<_>>(),
    )
    .await
    .expect("plan-mode turn should finish")
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .expect("turn should continue after entering plan mode");

    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::PlanModeEntered { .. })),
        "EnterPlanMode should still emit the plan-mode side effect"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::TextDelta { text, .. } if text == "continuing plan"
        )),
        "the model loop should continue after EnterPlanMode"
    );
    assert_eq!(
        harness.requests().len(),
        2,
        "EnterPlanMode should not stop the agent loop"
    );
}

#[tokio::test]
async fn runtime_ask_mode_reviews_exit_plan_mode_with_non_empty_plan() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir = main_agent_plans_dir(&workspace_sessions_dir(
        &home.path().join("sessions"),
        &workspace_root,
    ));
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Ask);
    {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let data = pm.enter(&plans_dir, true).expect("enter plan mode");
        std::fs::write(&data.path, "do the thing").expect("write plan");
    }

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "plan_summary": "Ready to execute" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let config = config.with_approval_handler(|request| {
        assert_eq!(request.operation, PermissionOperation::PlanTransition);
        approve_plan(request)
    });
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve plan"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ApprovalRequested { request }
            if request.id == "tool_1"
                && request.operation == PermissionOperation::PlanTransition
                && matches!(
                    request.presentation,
                    ApprovalPresentation::Plan { .. }
                )
                && matches!(
                    request.options.first().map(|option| &option.action),
                    Some(ApprovalAction::ApprovePlan { selection: None })
                )
                && request.options.iter().any(|option| {
                    matches!(option.action, ApprovalAction::RejectPlan)
                })
                && !request.options.iter().any(|option| {
                    matches!(option.action, ApprovalAction::PermitForSession { .. })
                })
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::PlanModeExited { turn, .. } if *turn == 1
    )));
}

#[tokio::test]
async fn exit_plan_mode_continues_loop_after_approval() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir = main_agent_plans_dir(&workspace_sessions_dir(
        &home.path().join("sessions"),
        &workspace_root,
    ));
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Ask);
    {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let data = pm.enter(&plans_dir, true).expect("enter plan mode");
        std::fs::write(&data.path, "execute the plan").expect("write plan");
    }

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "plan_summary": "Ready to execute" }).to_string(),
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
                text: "starting work".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let config = config.with_approval_handler(|request| {
        assert_eq!(request.operation, PermissionOperation::PlanTransition);
        approve_plan(request)
    });
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve plan"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::PlanModeExited { turn, .. } if *turn == 1
        )),
        "ExitPlanMode should still flip plan mode off"
    );
    assert!(
        events.iter().any(
            |event| matches!(event, AgentEvent::TextDelta { text, .. } if text == "starting work")
        ),
        "the model loop should continue after an approved ExitPlanMode"
    );
    assert_eq!(
        harness.requests().len(),
        2,
        "an approved ExitPlanMode must not stop the agent loop"
    );
}

#[tokio::test]
async fn exit_plan_mode_plan_selection_label_prefixes_tool_result() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir = main_agent_plans_dir(&workspace_sessions_dir(
        &home.path().join("sessions"),
        &workspace_root,
    ));
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Ask);
    {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let data = pm.enter(&plans_dir, true).expect("enter plan mode");
        std::fs::write(&data.path, "ship feature X").expect("write plan");
    }

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({
                    "plan_summary": "Two approaches available",
                    "options": [
                        {"label": "Option A", "description": "fast"},
                        {"label": "Option B", "description": "safe"}
                    ]
                })
                .to_string(),
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
                text: "running option a".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let config = config.with_approval_handler(|request| {
        assert_eq!(request.operation, PermissionOperation::PlanTransition);
        approve_plan_with_label(request, "Option A")
    });
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let _events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve option A"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    // The selected-approach prefix must reach the next model turn. The harness
    // records every ChatRequest, so turn 2's messages must contain the prefix
    // in the ExitPlanMode tool result that was appended to the context.
    let requests = harness.requests();
    assert_eq!(
        requests.len(),
        2,
        "an approved ExitPlanMode should continue into a second model turn"
    );
    let turn2 = &requests[1];
    let turn2_text = serde_json::to_string(turn2).unwrap_or_default();
    assert!(
        turn2_text.contains("Selected approach: Option A"),
        "turn 2 request must carry the selected-approach prefix; got: {turn2_text}"
    );
    assert!(
        turn2_text.contains("Execute ONLY the selected approach"),
        "turn 2 request must carry the execute-only instruction"
    );
}

#[tokio::test]
async fn exit_plan_mode_generic_approval_has_no_selected_approach() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let mut config = AgentConfig::for_model(fake_model());
    setup_active_plan(&mut config, &home, &workspace, "generic plan body");
    set_config_permission_mode(&mut config, PermissionMode::Ask);

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "plan_summary": "Ready to execute" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let config = config.with_approval_handler(|request| {
        assert!(matches!(
            request.options.first().map(|option| &option.action),
            Some(ApprovalAction::ApprovePlan { selection: None })
        ));
        approve_plan(request)
    });
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve plan"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let plan_request = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ApprovalRequested { request }
                if request.operation == PermissionOperation::PlanTransition =>
            {
                Some(request)
            }
            _ => None,
        })
        .expect("plan approval request");
    assert!(matches!(
        plan_request.options.first().map(|option| &option.action),
        Some(ApprovalAction::ApprovePlan { selection: None })
    ));
    let finished = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionFinished { name, result, .. } if name == "ExitPlanMode" => {
                Some(result)
            }
            _ => None,
        })
        .expect("ExitPlanMode finished");
    assert!(
        !finished.content.contains("Selected approach:"),
        "generic approve must not fabricate a selected approach"
    );
    if let Some(details) = finished.details.as_ref() {
        assert!(
            details.get("plan_selected_label").is_none(),
            "generic approve must not set plan_selected_label"
        );
    }
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::PlanModeExited { turn, .. } if *turn == 1
    )));
}

#[tokio::test]
async fn exit_plan_mode_typed_selection_reaches_tool_result() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let mut config = AgentConfig::for_model(fake_model());
    setup_active_plan(&mut config, &home, &workspace, "choose carefully");
    set_config_permission_mode(&mut config, PermissionMode::Ask);

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({
                    "plan_summary": "Two approaches",
                    "options": [
                        {"label": "Fast path", "description": "ship sooner"},
                        {"label": "Safe path", "description": "more checks"}
                    ]
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
    let config =
        config.with_approval_handler(|request| approve_plan_with_label(request, "Safe path"));
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("pick safe"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    // Re-emitted finished event after decoration carries selection details.
    let selected_plan_result = events
        .iter()
        .rev()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionFinished { name, result, .. }
                if name == "ExitPlanMode" && !result.is_error =>
            {
                Some(result)
            }
            _ => None,
        })
        .expect("selected ExitPlanMode result");
    assert!(
        selected_plan_result
            .content
            .contains("Selected approach: Safe path"),
        "content prefix missing: {}",
        selected_plan_result.content
    );
    assert!(matches!(
        selected_plan_result.details.as_ref(),
        Some(details) if details["plan_selected_label"] == "Safe path"
    ));
}

#[tokio::test]
async fn runtime_allow_for_session_does_not_cache_exit_plan_mode() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir = main_agent_plans_dir(&workspace_sessions_dir(
        &home.path().join("sessions"),
        &workspace_root,
    ));

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "plan_summary": "Ready to execute" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);

    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Ask);
    {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let data = pm.enter(&plans_dir, true).expect("enter plan mode");
        std::fs::write(&data.path, "do the thing").expect("write plan");
    }
    let session_approvals = Arc::clone(&config.session_approvals);
    let config = config.with_approval_handler(|request| {
        assert_eq!(request.operation, PermissionOperation::PlanTransition);
        // Pretend the (now-removed) "Approve for this session" option was chosen.
        // Session option is not offered for plan/goal transitions.
        first_offered_action(request)
    });

    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("approve plan"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    // The decisive assertion: no approval key must be cached, otherwise every
    // future exit-plan review would be silently auto-approved for the session.
    let cached = session_approvals.lock().expect("session approvals lock");
    assert!(
        cached.is_empty(),
        "ExitPlanMode must not cache any session approval key (got {cached:?}); \
         AllowForSession must be treated as AllowOnce for plan/goal transitions"
    );
}

#[tokio::test]
async fn runtime_ask_mode_exit_plan_mode_reject_keeps_plan_active() {
    // RejectPlan must deny with "approval denied" and leave plan mode active.
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir = main_agent_plans_dir(&workspace_sessions_dir(
        &home.path().join("sessions"),
        &workspace_root,
    ));
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Ask);
    {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let data = pm.enter(&plans_dir, true).expect("enter plan mode");
        std::fs::write(&data.path, "do the thing").expect("write plan");
    }
    let plan_mode = Arc::clone(&config.plan_mode);

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "plan_summary": "Ready to execute" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let config = config.with_approval_handler(move |request| {
        if request.operation == PermissionOperation::PlanTransition {
            reject_plan(request)
        } else {
            permit_once(request)
        }
    });
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("revise plan"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::PlanModeExited { .. })),
        "plan mode should remain active after RejectPlan"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            id,
            name,
            result,
            ..
        } if id == "tool_1"
            && name == "ExitPlanMode"
            && result.content.contains("approval denied")
    )));
    assert!(plan_mode.read().expect("plan mode lock").is_active());
}

#[tokio::test]
async fn runtime_plan_mode_guard_denies_write_outside_plan_file() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir = main_agent_plans_dir(&workspace_sessions_dir(
        &home.path().join("sessions"),
        &workspace_root,
    ));
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Yolo);
    {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let _data = pm.enter(&plans_dir, true).expect("enter plan mode");
    }
    let plan_mode = Arc::clone(&config.plan_mode);

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
                raw_arguments: json!({ "path": "other.txt", "content": "x" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("write while planning"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            id,
            name,
            result,
            ..
        } if id == "tool_1" && name == "Write" && result.is_error && result.content.contains("plan mode")
    )));
    assert!(
        plan_mode.read().expect("plan mode lock").is_active(),
        "plan mode should stay active after a blocked write"
    );
}

#[tokio::test]
async fn runtime_plan_mode_allows_writing_active_plan_file_outside_workspace() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir = main_agent_plans_dir(&workspace_sessions_dir(
        &home.path().join("sessions"),
        &workspace_root,
    ));
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Yolo);
    let plan_path = {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        pm.enter(&plans_dir, true).expect("enter plan mode").path
    };

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
                raw_arguments: json!({
                    "path": plan_path,
                    "content": "# Plan\n\nUse Write, not Bash."
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
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("write plan file"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            id,
            name,
            result,
            ..
        } if id == "tool_1" && name == "Write" && !result.is_error
    )));
    assert_eq!(
        std::fs::read_to_string(&plan_path).expect("read plan"),
        "# Plan\n\nUse Write, not Bash."
    );
}

#[tokio::test]
async fn runtime_plan_mode_allows_editing_active_plan_file_outside_workspace() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir = main_agent_plans_dir(&workspace_sessions_dir(
        &home.path().join("sessions"),
        &workspace_root,
    ));
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Yolo);
    let plan_path = {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let data = pm.enter(&plans_dir, true).expect("enter plan mode");
        std::fs::write(&data.path, "# Plan\n\nDraft.").expect("seed plan");
        data.path
    };

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Edit".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({
                    "path": plan_path,
                    "old": "Draft.",
                    "new": "Finalized."
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
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("edit plan file"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            id,
            name,
            result,
            ..
        } if id == "tool_1" && name == "Edit" && !result.is_error
    )));
    assert_eq!(
        std::fs::read_to_string(&plan_path).expect("read plan"),
        "# Plan\n\nFinalized."
    );
}

#[tokio::test]
async fn runtime_plan_mode_checks_each_edit_call_independently() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace.path().canonicalize().expect("workspace");
    let plans_dir = main_agent_plans_dir(&workspace_sessions_dir(
        &home.path().join("sessions"),
        &workspace_root,
    ));
    let other = workspace.path().join("other.txt");
    std::fs::write(&other, "other\n").expect("other file");
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Yolo);
    let plan_path = {
        let mut plan_mode = config.plan_mode.write().expect("plan mode lock");
        let data = plan_mode.enter(&plans_dir, true).expect("enter plan mode");
        std::fs::write(&data.path, "plan\n").expect("seed plan");
        data.path
    };
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[
            (
                "edit_plan",
                "Edit",
                json!({ "path": plan_path, "old": "plan", "new": "PLAN" }),
            ),
            (
                "edit_other",
                "Edit",
                json!({ "path": other, "old": "other", "new": "OTHER" }),
            ),
        ]),
        final_done_turn(),
    ]);
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = run_turn_collect(&runtime, &mut context, "edit plan files").await;

    let plan_result = finished_tool_results(&events, "edit_plan")
        .into_iter()
        .next()
        .expect("finished plan Edit");
    assert!(!plan_result.is_error);
    let result = finished_tool_results(&events, "edit_other")
        .into_iter()
        .next()
        .expect("finished other Edit");
    assert!(result.is_error);
    assert!(result.content.contains("Plan mode"), "{}", result.content);
    assert_eq!(std::fs::read_to_string(&plan_path).expect("plan"), "PLAN\n");
    assert_eq!(std::fs::read_to_string(other).expect("other"), "other\n");
}

fn setup_active_plan(
    config: &mut AgentConfig,
    home: &tempfile::TempDir,
    workspace: &tempfile::TempDir,
    content: &str,
) {
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir = main_agent_plans_dir(&workspace_sessions_dir(
        &home.path().join("sessions"),
        &workspace_root,
    ));
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    let mut pm = config.plan_mode.write().expect("plan mode lock");
    let data = pm.enter(&plans_dir, true).expect("enter plan mode");
    std::fs::write(&data.path, content).expect("write plan");
}

#[tokio::test]
async fn auto_exit_plan_mode_does_not_request_review() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let mut config = AgentConfig::for_model(fake_model());
    setup_active_plan(&mut config, &home, &workspace, "do the thing");
    set_config_permission_mode(&mut config, PermissionMode::Auto);

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "plan_summary": "Ready" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve plan"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::ApprovalRequested { .. })),
        "auto mode should not review ExitPlanMode"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::PlanModeExited { turn, .. } if *turn == 1
    )));
}

#[tokio::test]
async fn yolo_exit_plan_mode_with_non_empty_plan_requests_review() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let mut config = AgentConfig::for_model(fake_model());
    setup_active_plan(&mut config, &home, &workspace, "do the thing");
    set_config_permission_mode(&mut config, PermissionMode::Yolo);

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "plan_summary": "Ready" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let config = config.with_approval_handler(|request| {
        assert_eq!(request.operation, PermissionOperation::PlanTransition);
        approve_plan(request)
    });
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve plan"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ApprovalRequested { request }
            if request.id == "tool_1"
                && request.operation == PermissionOperation::PlanTransition
                && matches!(
                    request.options.first().map(|option| &option.action),
                    Some(ApprovalAction::ApprovePlan { selection: None })
                )
                && !request.options.iter().any(|option| {
                    matches!(option.action, ApprovalAction::PermitForSession { .. })
                })
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::PlanModeExited { turn, .. } if *turn == 1
    )));
}

#[test]
fn skill_context_is_injected_before_user_message() {
    let mut context = AgentContext::new();
    context.set_skill_context(AgentMessage::system_text("skill body".to_owned()));

    let skill_context = context.take_skill_context();
    assert!(skill_context.is_some());
    context.append_message(skill_context.unwrap());
    context.append_message(AgentMessage::user_text("user prompt".to_owned()));

    let messages: Vec<_> = context.messages().iter().collect();
    assert_eq!(messages.len(), 2);
    assert!(matches!(messages[0], AgentMessage::System { .. }));
    assert!(matches!(messages[1], AgentMessage::User { .. }));
}
