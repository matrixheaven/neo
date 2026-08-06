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
fn can_split_after_user_message_is_unsafe() {
    let messages = vec![user_msg("hello"), assistant_text("hi")];
    assert!(!can_split_after(&messages, 0));
}

#[test]
fn can_split_after_assistant_with_tool_calls_is_unsafe() {
    let messages = vec![
        user_msg("run ls"),
        assistant_with_tools(vec![tool_call("tc1")]),
    ];
    // index 1 is assistant with tool calls → unsafe
    assert!(!can_split_after(&messages, 1));
}

#[test]
fn can_split_after_tool_result_when_next_is_user_is_unsafe_due_to_open_exchange() {
    let messages = vec![
        user_msg("run ls"),
        assistant_with_tools(vec![tool_call("tc1"), tool_call("tc2")]),
        tool_result("tc1"),
        user_msg("done"),
    ];
    // index 2 is tool_result; prefix has assistant with 2 calls but only 1 result → open
    assert!(!can_split_after(&messages, 2));
}

#[test]
fn can_split_after_resolved_tool_result_is_safe() {
    let messages = vec![
        user_msg("run ls"),
        assistant_with_tools(vec![tool_call("tc1")]),
        tool_result("tc1"),
        user_msg("done"),
    ];
    // index 2 is tool_result; prefix has assistant with 1 call and 1 result → resolved → safe
    // BUT next message (index 3) is user, so the split after index 2 is safe
    assert!(can_split_after(&messages, 2));
}

#[test]
fn can_split_after_next_is_tool_result_is_unsafe() {
    let messages = vec![
        user_msg("run ls"),
        assistant_with_tools(vec![tool_call("tc1")]),
        tool_result("tc1"),
        tool_result("tc1b"), // would be orphaned if we split before it
    ];
    // index 2: next (index 3) is tool result → unsafe
    assert!(!can_split_after(&messages, 2));
}

#[test]
fn can_split_after_plain_assistant_is_safe() {
    let messages = vec![
        user_msg("hello"),
        assistant_text("hi there"),
        user_msg("bye"),
    ];
    // index 1: assistant without tool calls, next is user → safe
    assert!(can_split_after(&messages, 1));
}

#[test]
fn can_split_after_rejects_suffix_starting_with_unresolved_assistant_tool_calls() {
    // A previous exchange is fully resolved, but the next assistant has no results yet.
    let messages = vec![
        user_msg("run"),
        assistant_with_tools(vec![tool_call("tc0")]),
        tool_result("tc0"),
        assistant_with_tools(vec![tool_call("tc1")]),
    ];
    // Splitting after the resolved tc0 result would leave an orphan assistant with tool calls.
    assert!(!can_split_after(&messages, 2));
}

#[test]
fn can_split_after_allows_suffix_starting_with_resolved_assistant_tool_calls() {
    let messages = vec![
        user_msg("run"),
        assistant_with_tools(vec![tool_call("tc0")]),
        tool_result("tc0"),
        assistant_with_tools(vec![tool_call("tc1")]),
        tool_result("tc1"),
    ];
    // Splitting after tc0 result is fine because the next assistant already has its result.
    assert!(can_split_after(&messages, 2));
}

#[test]
fn can_split_after_rejects_partial_parallel_tool_results_in_suffix() {
    let messages = vec![
        user_msg("run"),
        assistant_with_tools(vec![tool_call("tc0")]),
        tool_result("tc0"),
        assistant_with_tools(vec![tool_call("tc1"), tool_call("tc2")]),
        tool_result("tc1"),
    ];
    // suffix would start with assistant that still needs tc2 result
    assert!(!can_split_after(&messages, 2));
}
