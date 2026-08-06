use super::*;
use crate::AgentToolCall;
use crate::StopReason;

fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::user_text(text)
}

fn assistant_text(text: &str) -> AgentMessage {
    AgentMessage::assistant(vec![Content::text(text)], Vec::new(), StopReason::EndTurn)
}

fn assistant_with_tools(calls: Vec<AgentToolCall>) -> AgentMessage {
    AgentMessage::assistant(Vec::new(), calls, StopReason::ToolUse)
}

fn tool_result(id: &str) -> AgentMessage {
    AgentMessage::tool_result(id, "bash", vec![Content::text("ok")], false)
}

fn tool_call(id: &str) -> AgentToolCall {
    AgentToolCall {
        id: id.into(),
        name: "bash".into(),
        raw_arguments: serde_json::json!({"command": "ls"}).to_string().into(),
    }
}

#[test]
fn compute_compact_count_manual_finds_safe_boundary() {
    let messages = vec![
        user_msg("task 1"),
        assistant_text("done 1"),
        user_msg("task 2"),
        assistant_text("done 2"),
    ];
    let strategy = CompactionStrategy::default();
    let count = compute_compact_count(&messages, CompactionSource::Manual, &strategy, 0);
    // Manual should compact as much as possible: split after index 1 (assistant_text)
    assert_eq!(count, 2);
}

#[test]
fn compute_compact_count_auto_respects_max_recent() {
    let messages: Vec<AgentMessage> = (0..20)
        .map(|i| {
            if i % 2 == 0 {
                user_msg(&format!("msg {i}"))
            } else {
                assistant_text(&format!("reply {i}"))
            }
        })
        .collect();
    let strategy = CompactionStrategy {
        max_recent_messages: 4,
        ..CompactionStrategy::default()
    };
    let count = compute_compact_count(&messages, CompactionSource::Auto, &strategy, 0);
    // Should keep at most max_recent_messages (4), compact the rest
    assert!(count <= messages.len() - 3, "count={count}");
}

#[test]
fn compute_compact_count_auto_can_compact_closed_trailing_tool_group() {
    let messages = vec![
        user_msg("run tools"),
        assistant_with_tools(vec![tool_call("tc1"), tool_call("tc2")]),
        tool_result("tc1"),
        tool_result("tc2"),
    ];
    let strategy = CompactionStrategy {
        max_recent_messages: 1,
        ..CompactionStrategy::default()
    };

    let count = compute_compact_count(&messages, CompactionSource::Auto, &strategy, 0);

    assert_eq!(count, messages.len());
}

#[test]
fn compute_compact_count_returns_zero_for_tiny_history() {
    let messages = vec![user_msg("only message")];
    let strategy = CompactionStrategy::default();
    let count = compute_compact_count(&messages, CompactionSource::Manual, &strategy, 0);
    assert_eq!(count, 0);
}

#[test]
fn compute_compact_count_preserves_safe_boundaries() {
    let messages = vec![
        user_msg("run"),
        assistant_with_tools(vec![tool_call("tc1")]),
        tool_result("tc1"),
        user_msg("again"),
        assistant_with_tools(vec![tool_call("tc2")]),
        tool_result("tc2"),
        user_msg("done"),
    ];
    let strategy = CompactionStrategy::default();
    let count = compute_compact_count(&messages, CompactionSource::Manual, &strategy, 0);
    // The split must not orphan any tool result
    if count > 0 {
        let kept = &messages[count..];
        // If kept starts with a tool result, it's orphaned
        if let Some(AgentMessage::ToolResult { .. }) = kept.first() {
            panic!("compaction kept an orphaned tool result at start");
        }
    }
}

#[test]
fn compute_compact_count_manual_after_dropping_incomplete_trailing_tool_turn() {
    let messages = vec![
        user_msg("task 1"),
        assistant_text("done 1"),
        user_msg("task 2"),
        assistant_with_tools(vec![tool_call("tc1")]),
    ];
    let messages = crate::sanitize_tool_exchange_messages(&messages);
    let strategy = CompactionStrategy::default();
    let count = compute_compact_count(&messages, CompactionSource::Manual, &strategy, 0);
    // After dropping the unresolved trailing assistant, manual compaction can
    // safely compact the prefix up to the previous safe boundary.
    assert_eq!(count, 2);
}
