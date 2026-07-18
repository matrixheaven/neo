//! Request-time and summary-time projection of large historical tool results.
//!
//! Projection is an ephemeral context-budgeting transform. It operates on owned
//! message copies for model inputs and never mutates durable agent history.

use crate::runtime::estimate_messages_tokens;
use crate::{AgentMessage, Content};

/// Projection mode for large historical tool result content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionMode {
    /// Projection disabled by mode.
    None,
    /// Projection applied to a normal model request.
    Request,
    /// Projection applied to messages fed into summary generation.
    SummaryInput,
}

/// Projection plan for old, large tool results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectionPlan {
    /// Whether projection should run.
    pub enabled: bool,
    /// First message index that must remain verbatim.
    pub cutoff_index: usize,
    /// Minimum estimated tool-result tokens before content is omitted.
    pub min_tool_result_tokens: usize,
    /// Number of newest messages to keep verbatim regardless of cutoff.
    pub keep_recent_messages: usize,
    /// Projection mode.
    pub mode: ProjectionMode,
}

impl ProjectionPlan {
    /// Return a disabled projection plan.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            enabled: false,
            cutoff_index: 0,
            min_tool_result_tokens: usize::MAX,
            keep_recent_messages: usize::MAX,
            mode: ProjectionMode::None,
        }
    }
}

/// Result of projecting messages for an ephemeral model input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionResult {
    /// Projected message copy.
    pub messages: Vec<AgentMessage>,
    /// Estimated tokens removed from omitted tool results.
    pub omitted_tokens: usize,
    /// Estimated token count of the projected message copy.
    pub projected_tokens: usize,
}

/// Project messages for a normal model request.
#[must_use]
pub fn project_for_request(messages: &[AgentMessage], plan: &ProjectionPlan) -> ProjectionResult {
    project_messages(messages, plan, ProjectionMode::Request)
}

/// Project messages for summary generation input.
#[must_use]
pub fn project_for_summary(messages: &[AgentMessage], plan: &ProjectionPlan) -> ProjectionResult {
    project_messages(messages, plan, ProjectionMode::SummaryInput)
}

fn project_messages(
    messages: &[AgentMessage],
    plan: &ProjectionPlan,
    mode: ProjectionMode,
) -> ProjectionResult {
    if !plan.enabled || plan.mode == ProjectionMode::None || plan.mode != mode {
        return unchanged(messages);
    }

    let recent_start = messages.len().saturating_sub(plan.keep_recent_messages);
    let cutoff = plan.cutoff_index.min(messages.len());
    let mut omitted_tokens = 0;
    let projected = messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            if index >= cutoff || index >= recent_start {
                return message.clone();
            }
            project_tool_result(
                message,
                plan.min_tool_result_tokens,
                mode,
                &mut omitted_tokens,
            )
        })
        .collect::<Vec<_>>();
    let projected_tokens = estimate_messages_tokens(&projected);

    ProjectionResult {
        messages: projected,
        omitted_tokens,
        projected_tokens,
    }
}

fn unchanged(messages: &[AgentMessage]) -> ProjectionResult {
    let messages = messages.to_vec();
    let projected_tokens = estimate_messages_tokens(&messages);
    ProjectionResult {
        messages,
        omitted_tokens: 0,
        projected_tokens,
    }
}

fn project_tool_result(
    message: &AgentMessage,
    min_tool_result_tokens: usize,
    mode: ProjectionMode,
    omitted_tokens: &mut usize,
) -> AgentMessage {
    let AgentMessage::ToolResult {
        tool_call_id,
        tool_name,
        content,
        is_error,
    } = message
    else {
        return message.clone();
    };

    let content_tokens = estimate_content_tokens(content);
    if content_tokens < min_tool_result_tokens {
        return message.clone();
    }

    let marker = omission_marker(mode, tool_name, content_tokens);
    let replacement_tokens = marker.len().div_ceil(4);
    *omitted_tokens += content_tokens.saturating_sub(replacement_tokens);
    AgentMessage::tool_result(
        tool_call_id.clone(),
        tool_name.clone(),
        vec![Content::text(marker)],
        *is_error,
    )
}

fn omission_marker(mode: ProjectionMode, tool_name: &str, content_tokens: usize) -> String {
    match mode {
        ProjectionMode::None => unreachable!("disabled projection must not build markers"),
        ProjectionMode::Request => {
            format!("[tool result omitted: tool={tool_name}, approx_tokens={content_tokens}]")
        }
        ProjectionMode::SummaryInput => format!("[omitted {tool_name} {content_tokens}t]"),
    }
}

fn estimate_content_tokens(content: &[Content]) -> usize {
    content
        .iter()
        .map(|part| match part {
            Content::Text { text } | Content::Thinking { text, .. } => text.len().div_ceil(4),
            Content::Image { .. } => 4_800,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentMessage, Content, StopReason};

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
    fn micro_projection_never_changes_instruction_messages() {
        let instruction = AgentMessage::Instruction {
            generation: 7,
            content: vec![Content::text("pinned rules ".repeat(4_000))],
        };
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
            // ...but the pinned instruction message passes through byte-for-byte.
            assert_eq!(result.messages[1], instruction, "{mode:?}");
        }
    }
}
