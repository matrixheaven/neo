use std::sync::Arc;

use neo_agent_core::harness::FakeHarness;
use neo_agent_core::tools::{ToolContext, ToolRegistry};
use neo_agent_core::{AgentConfig, PermissionMode, ToolExecutionMode};
use neo_ai::{AiStreamEvent, MessagePhase, StopReason};
use serde_json::{Value, json};

#[tokio::test]
async fn delegate_result_includes_complete_text_without_details() {
    let text = "result ".repeat(200);
    let (registry, ctx) = registry_for_text(&text, 64 * 1024);

    let result = registry
        .run("Delegate", &ctx, json!({"task": "return the full result"}))
        .await
        .expect("delegate should complete");
    let content: Value = serde_json::from_str(&result.content).expect("delegate result JSON");

    assert_eq!(content["kind"], "delegate_result");
    assert_eq!(content["result"]["mode"], "inline");
    assert_eq!(content["result"]["text"], text);
    assert_eq!(content["result"]["has_more"], false);
    assert!(
        result
            .details
            .as_ref()
            .and_then(|details| details.get("summary"))
            .and_then(Value::as_str)
            .is_some_and(|summary| summary.chars().count() <= 512),
        "details remains a bounded preview"
    );
}

#[tokio::test]
async fn oversized_delegate_result_returns_first_page_and_exact_next_action() {
    let text = "line with \"quotes\" and unicode \u{4e16}\u{754c}\n".repeat(300);
    let (registry, ctx) = registry_for_text(&text, 1024);

    let first = registry
        .run("Delegate", &ctx, json!({"task": "return the full result"}))
        .await
        .expect("delegate should complete");
    let mut page: Value = serde_json::from_str(&first.content).expect("delegate result JSON");
    assert_eq!(page["result"]["mode"], "page");

    let agent_id = page["target"]["id"].as_str().expect("agent id").to_owned();
    let mut reconstructed = String::new();
    loop {
        reconstructed.push_str(page["result"]["text"].as_str().expect("page text"));
        let Some(action) = page["next_actions"]
            .as_array()
            .and_then(|actions| actions.first())
        else {
            break;
        };
        assert_eq!(action["tool"], "TaskOutput");
        assert_eq!(action["arguments"]["task_id"], agent_id);
        assert_eq!(action["arguments"]["view"], "result");
        let next = registry
            .run("TaskOutput", &ctx, action["arguments"].clone())
            .await
            .expect("result page should be readable");
        page = serde_json::from_str(&next.content).expect("result page JSON");
    }

    assert_eq!(reconstructed, text);
}

fn registry_for_text(text: &str, max_output_bytes: usize) -> (ToolRegistry, ToolContext) {
    let harness = FakeHarness::from_turns([vec![
        AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: "child_message".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: text.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]]);
    let workspace = tempfile::tempdir().expect("workspace");
    let mut ctx = ToolContext::new(workspace.path())
        .expect("tool context")
        .with_child_runtime(
            AgentConfig::for_model(harness.model())
                .with_permission_mode(PermissionMode::Yolo)
                .with_tool_execution_mode(ToolExecutionMode::Sequential),
            harness.client(),
            Arc::new(ToolRegistry::new()),
            1,
        );
    ctx.max_output_bytes = max_output_bytes;
    (ToolRegistry::with_builtin_tools(), ctx)
}
