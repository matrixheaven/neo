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
fn render_messages_to_text_includes_role_and_content() {
    let messages = vec![user_msg("hello world"), assistant_text("hi")];
    let text = render_messages_to_text(&messages);
    assert!(text.contains("message 1"));
    assert!(text.contains("role=user"));
    assert!(text.contains("hello world"));
    assert!(text.contains("message 2"));
    assert!(text.contains("role=assistant"));
    assert!(text.contains("hi"));
}

#[test]
fn render_messages_to_text_shows_tool_calls() {
    let messages = vec![assistant_with_tools(vec![tool_call("tc-1")])];
    let text = render_messages_to_text(&messages);
    assert!(text.contains("tool calls:"));
    assert!(text.contains("tc-1: bash"));
}

#[test]
fn render_messages_to_text_shows_tool_result_metadata() {
    let messages = vec![tool_result("tr-1")];
    let text = render_messages_to_text(&messages);
    assert!(text.contains("tool_call_id=tr-1"));
    assert!(text.contains("tool_name=bash"));
}
