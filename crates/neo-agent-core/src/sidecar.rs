//! Sidecar dialog support for `/btw`-style side questions.
//!
//! A sidecar is a lightweight temporary conversation that inherits the parent
//! conversation history but disables tool use. It appends a system reminder
//! telling the model to answer with text only and provides a deny-all hook
//! that can be installed as a `BeforeToolCallHook`.

use crate::{AgentMessage, AgentToolCall, ToolResult, sanitize_tool_exchange_messages};

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
    let mut messages = sanitize_tool_exchange_messages(parent);
    messages.push(AgentMessage::system_text(SIDE_QUESTION_SYSTEM_REMINDER));
    messages
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
    use crate::StopReason;

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

    #[test]
    fn btw_sidecar_inherits_projected_parent_history_without_mutating_parent() {
        let parent = vec![
            AgentMessage::user_text("first"),
            AgentMessage::assistant(
                [crate::Content::text("second")],
                Vec::new(),
                StopReason::EndTurn,
            ),
        ];
        let parent_clone = parent.clone();

        let projected = sidecar_projected_messages(&parent);

        assert_eq!(parent, parent_clone, "parent must not be mutated");
        let texts: Vec<String> = projected.iter().map(AgentMessage::text).collect();
        assert!(texts.iter().any(|t| t == "first"));
        assert!(texts.iter().any(|t| t == "second"));
        assert!(texts.iter().any(|t| t.contains("side-channel")));
    }

    #[test]
    fn btw_sidecar_trims_incomplete_trailing_tool_exchange() {
        let parent = vec![
            AgentMessage::user_text("run a tool"),
            AgentMessage::Assistant {
                content: vec![crate::Content::text("ok")],
                tool_calls: vec![AgentToolCall {
                    id: "t1".to_owned(),
                    name: "bash".to_owned(),
                    arguments: serde_json::json!({"command": "echo hi"}),
                }],
                stop_reason: StopReason::ToolUse,
            },
        ];

        let projected = sidecar_projected_messages(&parent);

        // The open assistant tool-call turn should be removed, leaving only the
        // user message and the side reminder.
        assert!(
            projected.iter().all(|message| !matches!(
                message,
                AgentMessage::Assistant {
                    stop_reason: StopReason::ToolUse,
                    ..
                }
            )),
            "incomplete trailing tool turn must be trimmed"
        );
        assert!(
            projected
                .iter()
                .any(|message| message.text() == "run a tool")
        );
        assert!(
            projected
                .iter()
                .any(|message| message.text().contains("side-channel"))
        );
    }

    #[test]
    fn btw_sidecar_appends_side_question_system_reminder() {
        let parent = vec![AgentMessage::user_text("hello")];

        let projected = sidecar_projected_messages(&parent);

        let reminder_idx = projected
            .iter()
            .position(|message| message.text().contains("side-channel"))
            .expect("reminder should be present");
        assert_eq!(
            reminder_idx,
            projected.len() - 1,
            "reminder must be the last message"
        );
    }
}
