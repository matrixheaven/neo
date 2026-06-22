//! Sidecar dialog support for `/btw`-style side questions.
//!
//! A sidecar is a lightweight temporary conversation that inherits the parent
//! conversation history but disables tool use. It appends a system reminder
//! telling the model to answer with text only and provides a deny-all hook
//! that can be installed as a `BeforeToolCallHook`.

use crate::{AgentMessage, AgentToolCall, StopReason, ToolResult};

/// System reminder appended to sidecar projections.
///
/// This text is injected as a system message after the inherited parent
/// history so the side model knows it must not modify the main conversation,
/// queue, goal, plan, files, or workspace, and that all tool calls are
/// disabled.
pub const SIDE_QUESTION_SYSTEM_REMINDER: &str = "This is a side-channel conversation with the user.\n\
    You are a lightweight temporary instance answering a side question.\n\
    Do not modify the main conversation, queue, goal, plan, files, or workspace.\n\
    Tool definitions may be present only for prompt-cache stability.\n\
    All tool calls are disabled and will be rejected.\n\
    Answer with text only.";

/// Project a parent's message history into a sidecar context.
///
/// The parent history is cloned, incomplete trailing assistant/tool exchanges
/// are trimmed, and the side reminder system message is appended. The parent
/// slice is never mutated.
#[must_use]
pub fn sidecar_projected_messages(parent: &[AgentMessage]) -> Vec<AgentMessage> {
    let mut messages = drop_incomplete_trailing_tool_turn(parent.to_vec());
    messages.push(AgentMessage::system_text(SIDE_QUESTION_SYSTEM_REMINDER));
    messages
}

/// Drop an incomplete trailing assistant tool-call turn from a message list.
///
/// If the last assistant message stopped for tool use and not all of its tool
/// calls have matching `ToolResult` messages after it, that assistant turn and
/// any following tool results are removed. This keeps the sidecar projection
/// from presenting a tool call that the side model can never answer.
#[must_use]
pub fn drop_incomplete_trailing_tool_turn(messages: Vec<AgentMessage>) -> Vec<AgentMessage> {
    let Some(assistant_index) = messages.iter().rposition(|message| {
        matches!(
            message,
            AgentMessage::Assistant {
                tool_calls,
                stop_reason: StopReason::ToolUse,
                ..
            } if !tool_calls.is_empty()
        )
    }) else {
        return messages;
    };

    if messages[assistant_index + 1..].iter().any(|message| {
        matches!(
            message,
            AgentMessage::User { .. } | AgentMessage::Assistant { .. }
        )
    }) {
        return messages;
    }

    let AgentMessage::Assistant { tool_calls, .. } = &messages[assistant_index] else {
        return messages;
    };
    let mut missing_tool_result_ids = tool_calls
        .iter()
        .map(|tool_call| tool_call.id.as_str())
        .collect::<Vec<_>>();
    for message in &messages[assistant_index + 1..] {
        let AgentMessage::ToolResult { tool_call_id, .. } = message else {
            continue;
        };
        if let Some(index) = missing_tool_result_ids
            .iter()
            .position(|id| *id == tool_call_id)
        {
            missing_tool_result_ids.remove(index);
        }
    }
    if missing_tool_result_ids.is_empty() {
        messages
    } else {
        messages[..assistant_index].to_vec()
    }
}

/// Deny-all hook suitable for use as a `BeforeToolCallHook` in a sidecar.
///
/// Returns an error `ToolResult` for every call, ensuring no tool executes.
#[must_use]
pub fn deny_sidecar_tool_call(_call: &AgentToolCall) -> Option<ToolResult> {
    Some(ToolResult::error(
        "Tool calls are disabled for side questions. Answer with text only.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_sidecar_tool_call_returns_error() {
        let call = AgentToolCall {
            id: "t1".to_owned(),
            name: "bash".to_owned(),
            arguments: serde_json::json!({}),
        };
        let result = deny_sidecar_tool_call(&call).expect("hook should return a result");
        assert!(result.is_error);
        assert_eq!(
            result.content,
            "Tool calls are disabled for side questions. Answer with text only."
        );
    }
}
