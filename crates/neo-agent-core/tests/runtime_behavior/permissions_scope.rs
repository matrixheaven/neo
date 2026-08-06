use super::fake_harness::final_done_turn;
use super::permissions::count_approval_requests;
use super::permissions::permit_once;
use super::permissions::select_action;
use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, ApprovalAction,
    ApprovalRequest, ApprovalResponse, PermissionMode, ToolRegistry, harness::FakeHarness,
};
use neo_ai::{AiStreamEvent, MessagePhase};
use serde_json::json;
use std::sync::{Arc, Mutex};

pub(crate) fn permit_for_session(request: &ApprovalRequest) -> ApprovalResponse {
    let action = request
        .options
        .iter()
        .find_map(|option| match &option.action {
            ApprovalAction::PermitForSession { .. } => Some(option.action.clone()),
            _ => None,
        })
        .expect("PermitForSession option");
    select_action(request, action)
}

pub(crate) fn permit_for_prefix(request: &ApprovalRequest) -> ApprovalResponse {
    let action = request
        .options
        .iter()
        .find_map(|option| match &option.action {
            ApprovalAction::PermitForPrefix { .. } => Some(option.action.clone()),
            _ => None,
        })
        .expect("PermitForPrefix option");
    select_action(request, action)
}

#[tokio::test]
async fn allow_for_session_does_not_persist_prefix_rule() {
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
    let config = AgentConfig::for_model(harness.model())
        .with_permission_mode(PermissionMode::Ask)
        .with_workspace_root(workspace.path())
        .expect("workspace root")
        .with_approval_handler(permit_for_session);
    let prefix_store = Arc::clone(&config.prefix_approval_rules);
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("python script"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ApprovalRequested { request }
            if request.options.iter().any(|option| matches!(
                &option.action,
                ApprovalAction::PermitForPrefix { rule }
                    if rule.prefix == vec!["python".to_owned()]
            ))
    )));
    assert!(
        prefix_store
            .lock()
            .expect("prefix store")
            .prefix_rules
            .is_empty(),
        "AllowForSession must not persist prefix approval rules"
    );
}

#[tokio::test]
async fn allow_for_prefix_persists_prefix_rule() {
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
    let config = AgentConfig::for_model(harness.model())
        .with_permission_mode(PermissionMode::Ask)
        .with_workspace_root(workspace.path())
        .expect("workspace root")
        .with_approval_handler(permit_for_prefix);
    let prefix_store = Arc::clone(&config.prefix_approval_rules);
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("python script"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert_eq!(
        prefix_store
            .lock()
            .expect("prefix store")
            .prefix_rules
            .iter()
            .map(|rule| rule.prefix.clone())
            .collect::<Vec<_>>(),
        vec![vec!["python".to_owned()]]
    );
}

#[tokio::test]
async fn layer3_safe_command_auto_approved() {
    // `cat README.md` is a known-safe command — it should not prompt at all.
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
                raw_arguments: json!({ "command": "cat README.md" }).to_string(),
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
                    permit_once(request)
                }
            }),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("cat readme"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");
    assert_eq!(
        count_approval_requests(&events),
        0,
        "known-safe commands like `cat` must be auto-approved without prompt"
    );
}

#[tokio::test]
async fn layer3_dangerous_command_forces_prompt_no_scope() {
    // `rm -rf /tmp/x` is dangerous — it must prompt and offer NO session scope.
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
                raw_arguments: json!({ "command": "rm -rf /tmp/x" }).to_string(),
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
            .expect("workspace root")
            .with_approval_handler(permit_once),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("rm"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");
    assert_eq!(
        count_approval_requests(&events),
        1,
        "dangerous commands must prompt"
    );
    // The approval event must offer NO session option (so it can't be cached).
    let has_scope = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ApprovalRequested { request }
                if request.options.iter().any(|option| {
                    matches!(option.action, ApprovalAction::PermitForSession { .. })
                })
        )
    });
    assert!(
        !has_scope,
        "dangerous commands must not offer a reusable session scope"
    );
}
