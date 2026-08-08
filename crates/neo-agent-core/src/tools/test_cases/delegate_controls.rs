//! Delegate controls behavior (moved from `delegate_controls.rs`).

use super::*;
use serde_json::json;

fn test_context() -> ToolContext {
    let dir = tempfile::tempdir().expect("temp dir");
    ToolContext::new(dir.path()).expect("tool context")
}

#[test]
fn delegate_list_optional_fields_are_bounded_and_single_line() {
    let mut detail = String::new();
    append_delegate_list_field(
        &mut detail,
        "summary",
        &format!("first line\n{}", "x".repeat(MAX_DELEGATE_LIST_FIELD_CHARS)),
    );
    assert_eq!(detail.lines().count(), 1);
    assert!(detail.contains("first line "));
    assert!(detail.ends_with("...\n"));

    let max_bytes = DELEGATE_LIST_CONTENT_TRUNCATION_SUFFIX.len() + 8;
    let capped = cap_delegate_list_content("x".repeat(128), max_bytes);
    assert!(capped.len() <= max_bytes);
    assert!(capped.ends_with(DELEGATE_LIST_CONTENT_TRUNCATION_SUFFIX));
}

#[tokio::test]
async fn wait_delegate_validates_ids_and_returns_unknown_targets() {
    let ctx = test_context();
    let tool = WaitDelegateTool;

    for (input, expected) in [
        (json!({"ids": []}), "at least one"),
        (
            json!({"ids": ["agent_same", "agent_same"]}),
            "duplicate target",
        ),
    ] {
        let result = tool
            .execute(&ctx, input)
            .await
            .expect("validation should return a tool result");
        assert!(result.is_error);
        assert!(result.content.contains(expected), "{}", result.content);
    }

    let completed = ctx
        .multi_agent
        .start_foreground_delegate_for_test("completed target");
    let _ = ctx
        .multi_agent
        .complete_delegate_for_test(&completed.id, "done");
    let result = tool
        .execute(
            &ctx,
            json!({"ids": [completed.id.as_str(), "agent_missing"]}),
        )
        .await
        .expect("unknown targets should return immediately");
    let details = result.details.expect("wait details");
    assert_eq!(details["outcome"], "not_found");
    assert_eq!(details["aggregate"]["terminal"], 1);
    assert_eq!(details["aggregate"]["not_found"], 1);
    assert_eq!(details["items"][0]["status"], "completed");
    assert_eq!(details["items"][1]["status"], "not_found");
}

#[tokio::test]
async fn list_delegates_empty_steps_follow_state_filter() {
    let ctx = test_context();
    let tool = ListDelegatesTool;

    let result = tool
        .execute(
            &ctx,
            json!({
                "include_completed": true,
                "state": "cancelled"
            }),
        )
        .await
        .expect("list result");

    assert!(!result.is_error);
    assert!(result.content.contains("No delegates found."));
    assert!(result.content.contains("No cancelled delegates found"));
    assert!(!result.content.contains("Pass include_completed=true"));
    assert_eq!(
        result.details.as_ref().unwrap()["query"]["state"],
        "cancelled"
    );
    assert_eq!(result.details.as_ref().unwrap()["include_completed"], true);
}

#[tokio::test]
async fn list_delegates_default_empty_steps_explain_active_default() {
    let ctx = test_context();
    let tool = ListDelegatesTool;

    let result = tool.execute(&ctx, json!({})).await.expect("list result");

    assert!(!result.is_error);
    assert!(result.content.contains("No active delegates found."));
    assert!(result.content.contains("Pass include_completed=true"));
}

#[tokio::test]
async fn list_delegates_rejects_zero_limit() {
    let ctx = test_context();
    let tool = ListDelegatesTool;

    let err = tool
        .execute(&ctx, json!({ "limit": 0 }))
        .await
        .expect_err("zero limit should be invalid input");

    match err {
        ToolError::InvalidInput { tool, message } => {
            assert_eq!(tool, "ListDelegates");
            assert!(message.contains("limit must be >= 1"));
        }
        other => panic!("expected invalid input, got {other:?}"),
    }
}

#[tokio::test]
async fn interrupt_delegate_unknown_id_uses_delegate_error() {
    let ctx = test_context();
    let tool = InterruptDelegateTool;

    let result = tool
        .execute(&ctx, json!({ "id": "agent_missing" }))
        .await
        .expect("tool should return result");

    assert!(result.is_error);
    assert_eq!(result.content, "unknown delegate target `agent_missing`");
    assert!(!result.content.contains("TaskStop"));
    assert!(!result.content.contains("background task"));
    assert_eq!(result.details.as_ref().unwrap()["kind"], "delegate_target");
    assert_eq!(result.details.as_ref().unwrap()["outcome"], "not_found");
}

#[tokio::test]
async fn terminal_delegate_errors_are_action_specific() {
    let ctx = test_context();
    let agent = ctx
        .multi_agent
        .start_foreground_delegate_for_test("calculate 2 + 2");
    let _ = ctx
        .multi_agent
        .complete_delegate_for_test(&agent.id, "The answer is 4.");

    let message_result = MessageDelegateTool
        .execute(
            &ctx,
            json!({ "id": agent.id.as_str(), "message": "another question" }),
        )
        .await
        .expect("message result");
    assert!(message_result.is_error);
    assert!(
        message_result
            .content
            .contains("cannot receive live messages")
    );
    assert!(!message_result.content.contains("be interrupted"));
    assert_eq!(
        message_result.details.as_ref().unwrap()["action"],
        "message"
    );

    let interrupt_result = InterruptDelegateTool
        .execute(&ctx, json!({ "id": agent.id.as_str() }))
        .await
        .expect("interrupt result");
    assert!(interrupt_result.is_error);
    assert!(interrupt_result.content.contains("cannot be interrupted"));
    assert!(!interrupt_result.content.contains("live messages"));
    assert_eq!(
        interrupt_result.details.as_ref().unwrap()["action"],
        "interrupt"
    );
}

#[tokio::test]
async fn list_delegates_any_run_state_finds_resumed_cancelled_agent() {
    let ctx = test_context();
    let agent = ctx
        .multi_agent
        .start_foreground_delegate_for_test("first run");
    let cancelled = ctx
        .multi_agent
        .cancel_agent_by_id(agent.id.as_str())
        .expect("agent cancelled");
    assert_eq!(cancelled.state, AgentLifecycleState::Cancelled);

    ctx.multi_agent
        .start_resume_delegate(
            agent.id.as_str(),
            &crate::multi_agent::DelegateRequest {
                task: "second run".to_owned(),
                resume: Some(agent.id.as_str().to_owned()),
                title: None,
                role: None,
                mode: crate::multi_agent::AgentRunMode::Foreground,
                context: crate::multi_agent::DelegateContext::None,
                output_schema: None,
            },
        )
        .expect("resume starts");
    let _ = ctx
        .multi_agent
        .complete_delegate_for_test(&agent.id, "second run done");

    let result = ListDelegatesTool
        .execute(
            &ctx,
            json!({
                "include_completed": true,
                "state": "cancelled",
                "state_scope": "any_run"
            }),
        )
        .await
        .expect("list result");

    assert!(!result.is_error);
    assert!(result.content.contains(agent.id.as_str()));
    let details = result.details.as_ref().unwrap();
    assert_eq!(details["query"]["state"], "cancelled");
    assert_eq!(details["query"]["state_scope"], "any_run");
    assert_eq!(details["delegates"][0]["current_status"], "completed");
    assert_eq!(
        details["delegates"][0]["terminal_status_history"][0],
        "cancelled"
    );
}

#[tokio::test]
async fn list_delegates_current_state_does_not_match_resumed_cancelled_agent() {
    let ctx = test_context();
    let agent = ctx
        .multi_agent
        .start_foreground_delegate_for_test("first run");
    ctx.multi_agent
        .cancel_agent_by_id(agent.id.as_str())
        .expect("agent cancelled");
    ctx.multi_agent
        .start_resume_delegate(
            agent.id.as_str(),
            &crate::multi_agent::DelegateRequest {
                task: "second run".to_owned(),
                resume: Some(agent.id.as_str().to_owned()),
                title: None,
                role: None,
                mode: crate::multi_agent::AgentRunMode::Foreground,
                context: crate::multi_agent::DelegateContext::None,
                output_schema: None,
            },
        )
        .expect("resume starts");
    let _ = ctx
        .multi_agent
        .complete_delegate_for_test(&agent.id, "second run done");

    let result = ListDelegatesTool
        .execute(
            &ctx,
            json!({
                "include_completed": true,
                "state": "cancelled"
            }),
        )
        .await
        .expect("list result");

    assert!(!result.is_error);
    assert!(result.content.contains("No cancelled delegates found"));
}

#[tokio::test]
async fn list_delegates_any_run_history_preserves_repeated_terminal_states() {
    let ctx = test_context();
    let agent = ctx
        .multi_agent
        .start_foreground_delegate_for_test("first run");
    let _ = ctx
        .multi_agent
        .complete_delegate_for_test(&agent.id, "first run done");

    ctx.multi_agent
        .start_resume_delegate(
            agent.id.as_str(),
            &crate::multi_agent::DelegateRequest {
                task: "second run".to_owned(),
                resume: Some(agent.id.as_str().to_owned()),
                title: None,
                role: None,
                mode: crate::multi_agent::AgentRunMode::Foreground,
                context: crate::multi_agent::DelegateContext::None,
                output_schema: None,
            },
        )
        .expect("second run starts");
    let _ = ctx
        .multi_agent
        .complete_delegate_for_test(&agent.id, "second run done");
    ctx.multi_agent
        .start_resume_delegate(
            agent.id.as_str(),
            &crate::multi_agent::DelegateRequest {
                task: "third run".to_owned(),
                resume: Some(agent.id.as_str().to_owned()),
                title: None,
                role: None,
                mode: crate::multi_agent::AgentRunMode::Foreground,
                context: crate::multi_agent::DelegateContext::None,
                output_schema: None,
            },
        )
        .expect("third run starts");

    let result = ListDelegatesTool
        .execute(
            &ctx,
            json!({
                "state": "completed",
                "state_scope": "any_run"
            }),
        )
        .await
        .expect("list result");

    assert!(!result.is_error);
    let history = &result.details.as_ref().unwrap()["delegates"][0]["terminal_status_history"];
    assert_eq!(history, &json!(["completed", "completed"]));
}

#[tokio::test]
async fn delegate_control_results_strip_live_queue_metadata() {
    let ctx = test_context();
    let agent = ctx
        .multi_agent
        .start_foreground_delegate_for_test("queued command");
    let started_at = std::time::Instant::now();
    let _ = ctx.multi_agent.apply_child_event(
        &agent.id,
        started_at,
        &crate::AgentEvent::ToolExecutionQueued {
            turn: 1,
            id: "bash-queued".to_owned(),
            name: "Bash".to_owned(),
            arguments: json!({"command": "cargo test"}),
            workflow_origin: None,
        },
    );
    let _ = ctx.multi_agent.apply_child_event(
        &agent.id,
        started_at,
        &crate::AgentEvent::ToolExecutionQueueUpdated {
            turn: 1,
            id: "bash-queued".to_owned(),
            position: 2,
            waiting_ms: 18_000,
        },
    );
    let interrupted = InterruptDelegateTool
        .execute(&ctx, json!({"id": agent.id.as_str()}))
        .await
        .expect("interrupt queued agent");
    assert_queue_metadata_cleared(
        &interrupted.details.as_ref().expect("interrupt details")["agent"]["activity"][0]["kind"]["phase"],
    );

    let listed = ListDelegatesTool
        .execute(
            &ctx,
            json!({
                "include_completed": true,
                "include": ["activity"]
            }),
        )
        .await
        .expect("list delegates");
    assert_queue_metadata_cleared(
        &listed.details.as_ref().expect("list details")["delegates"][0]["activity_tail"][0]["kind"]
            ["phase"],
    );

    let waited = WaitDelegateTool
        .execute(&ctx, json!({"ids": [agent.id.as_str()], "timeout_ms": 1}))
        .await
        .expect("wait delegate");
    assert_queue_metadata_cleared(
        &waited.details.as_ref().expect("wait details")["items"][0]["activity_tail"][0]["kind"]["phase"],
    );
}

fn assert_queue_metadata_cleared(phase: &serde_json::Value) {
    assert_eq!(phase["queued"]["position"], serde_json::Value::Null);
    assert_eq!(phase["queued"]["queued_at_ms"], 0);
}
