use super::*;
use crate::AgentMessage;
use crate::Content;
use crate::StopReason;

#[test]
fn request_projection_truncates_old_large_tool_results() {
    let messages = vec![
        AgentMessage::user_text("start"),
        AgentMessage::tool_result(
            "old_call",
            "Read",
            vec![Content::text("x".repeat(8_000))],
            false,
        ),
        AgentMessage::tool_result(
            "new_call",
            "Read",
            vec![Content::text("y".repeat(8_000))],
            false,
        ),
    ];
    let plan = ProjectionPlan {
        enabled: true,
        cutoff_index: 2,
        min_tool_result_tokens: 100,
        keep_recent_messages: 1,
        snip_enabled: false,
        snip_min_tokens: 0,
        snip_keep_recent: 0,
        mode: ProjectionMode::Request,
    };

    let result = project_for_request(&messages, &plan);

    assert_eq!(messages[1].text().len(), 8_000);
    assert!(result.messages[1].text().contains("[tool result omitted"));
    assert_eq!(result.messages[2].text().len(), 8_000);
    assert!(result.omitted_tokens > 1_000);
    assert!(result.projected_tokens < crate::runtime::estimate_messages_tokens(&messages));
}

#[test]
fn projection_never_changes_user_or_assistant_messages() {
    let assistant = AgentMessage::assistant(
        vec![Content::text("assistant payload")],
        Vec::new(),
        StopReason::EndTurn,
    );
    let user = AgentMessage::user_text("user payload");
    let messages = vec![user.clone(), assistant.clone()];
    let plan = ProjectionPlan {
        enabled: true,
        cutoff_index: messages.len(),
        min_tool_result_tokens: 1,
        keep_recent_messages: 0,
        snip_enabled: false,
        snip_min_tokens: 0,
        snip_keep_recent: 0,
        mode: ProjectionMode::Request,
    };

    let result = project_for_request(&messages, &plan);

    assert_eq!(result.messages, messages);
}

#[test]
fn summary_projection_can_be_more_aggressive_than_request_projection() {
    let messages = vec![
        AgentMessage::tool_result("a", "Read", vec![Content::text("a".repeat(4_000))], false),
        AgentMessage::tool_result("b", "Read", vec![Content::text("b".repeat(4_000))], false),
    ];
    let request_plan = ProjectionPlan {
        enabled: true,
        cutoff_index: 1,
        min_tool_result_tokens: 100,
        keep_recent_messages: 1,
        snip_enabled: false,
        snip_min_tokens: 0,
        snip_keep_recent: 0,
        mode: ProjectionMode::Request,
    };
    let summary_plan = ProjectionPlan {
        mode: ProjectionMode::SummaryInput,
        keep_recent_messages: 0,
        ..request_plan
    };

    let request = project_for_request(&messages, &request_plan);
    let summary = project_for_summary(&messages, &summary_plan);

    assert!(summary.omitted_tokens > request.omitted_tokens);
    assert!(summary.projected_tokens < request.projected_tokens);
}

#[test]
fn micro_projection_never_changes_instruction_injections() {
    let instruction =
        AgentMessage::injection_text("pinned rules ".repeat(4_000), "instruction_epoch");
    let messages = vec![
        AgentMessage::tool_result(
            "old_call",
            "Read",
            vec![Content::text("x".repeat(8_000))],
            false,
        ),
        instruction.clone(),
        AgentMessage::tool_result(
            "new_call",
            "Read",
            vec![Content::text("y".repeat(8_000))],
            false,
        ),
    ];

    for mode in [ProjectionMode::Request, ProjectionMode::SummaryInput] {
        let plan = ProjectionPlan {
            enabled: true,
            cutoff_index: messages.len(),
            min_tool_result_tokens: 100,
            keep_recent_messages: 0,
            snip_enabled: false,
            snip_min_tokens: 0,
            snip_keep_recent: 0,
            mode,
        };
        let result = match mode {
            ProjectionMode::Request => project_for_request(&messages, &plan),
            ProjectionMode::SummaryInput => project_for_summary(&messages, &plan),
            ProjectionMode::None => unreachable!("test only exercises active modes"),
        };

        // The large tool results around the epoch are projected...
        assert!(result.messages[0].text().contains("omitted"), "{mode:?}");
        assert!(result.messages[2].text().contains("omitted"), "{mode:?}");
        assert!(result.omitted_tokens > 0, "{mode:?}");
        // ...but the instruction injection passes through byte-for-byte.
        assert_eq!(result.messages[1], instruction, "{mode:?}");
    }
}
