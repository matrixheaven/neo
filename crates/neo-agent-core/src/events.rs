use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use std::path::PathBuf;

use crate::multi_agent::{AgentSnapshot, SwarmSnapshot};
use crate::{AgentMessage, AgentToolCall, PermissionOperation, ShellCommandOutcome, ToolResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ShellCommandOrigin {
    ModelBashTool,
    UserShellMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentTokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(default)]
    pub input_cache_read_tokens: u32,
    #[serde(default)]
    pub input_cache_write_tokens: u32,
}

impl From<neo_ai::TokenUsage> for AgentTokenUsage {
    fn from(value: neo_ai::TokenUsage) -> Self {
        Self {
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            input_cache_read_tokens: value.input_cache_read_tokens,
            input_cache_write_tokens: value.input_cache_write_tokens,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Cancelled,
    Error,
}

impl From<neo_ai::StopReason> for StopReason {
    fn from(value: neo_ai::StopReason) -> Self {
        match value {
            neo_ai::StopReason::EndTurn => Self::EndTurn,
            neo_ai::StopReason::ToolUse => Self::ToolUse,
            neo_ai::StopReason::MaxTokens => Self::MaxTokens,
            neo_ai::StopReason::Cancelled => Self::Cancelled,
            neo_ai::StopReason::Error => Self::Error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum AgentEvent {
    RunStarted {
        turn: u32,
    },
    TurnStarted {
        turn: u32,
    },
    MessageStarted {
        turn: u32,
        id: String,
    },
    MessageFinished {
        turn: u32,
        id: String,
        stop_reason: StopReason,
    },
    TextDelta {
        turn: u32,
        text: String,
    },
    ThinkingStarted {
        turn: u32,
        id: String,
    },
    ThinkingDelta {
        turn: u32,
        text: String,
    },
    ThinkingFinished {
        turn: u32,
        signature: Option<String>,
        redacted: bool,
    },
    ToolCallStarted {
        turn: u32,
        id: String,
        name: String,
    },
    ToolCallArgumentsDelta {
        turn: u32,
        id: String,
        json_fragment: String,
    },
    ToolCallFinished {
        turn: u32,
        tool_call: AgentToolCall,
    },
    ToolExecutionStarted {
        turn: u32,
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    ToolExecutionFinished {
        turn: u32,
        id: String,
        name: String,
        result: ToolResult,
    },
    ToolExecutionUpdate {
        turn: u32,
        id: String,
        name: String,
        partial_result: ToolResult,
    },
    SkillActivated {
        turn: u32,
        name: String,
        #[serde(default)]
        body: String,
    },
    GoalStarted {
        turn: u32,
        objective: String,
    },
    GoalPaused {
        turn: u32,
        objective: String,
    },
    GoalResumed {
        turn: u32,
        objective: String,
    },
    GoalBlocked {
        turn: u32,
        objective: String,
        reason: String,
    },
    GoalFinished {
        turn: u32,
        objective: String,
        outcome: String,
    },
    ApprovalRequested {
        turn: u32,
        id: String,
        operation: PermissionOperation,
        subject: String,
        arguments: serde_json::Value,
        /// Reusable session scope (Layer 1). `None` for review transitions and
        /// scope-ineligible prompts. `#[serde(default)]` keeps old JSONL working.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_scope: Option<crate::permissions::SessionApprovalScope>,
        /// Proposed persistent prefix rule (Layer 2). `None` when no prefix
        /// option should be offered.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prefix_rule: Option<crate::permissions::PrefixApprovalRule>,
    },
    ShellCommandStarted {
        turn: u32,
        id: String,
        command: String,
        cwd: PathBuf,
        origin: ShellCommandOrigin,
    },
    ShellCommandFinished {
        turn: u32,
        id: String,
        exit_code: Option<i32>,
        stdout: String,
        stderr: String,
        truncated: bool,
        origin: ShellCommandOrigin,
        outcome: ShellCommandOutcome,
    },
    TerminalSessionStarted {
        turn: u32,
        id: String,
        handle: String,
        command: String,
        cwd: PathBuf,
        cols: u16,
        rows: u16,
    },
    TerminalSessionOutput {
        turn: u32,
        id: String,
        handle: String,
        output: String,
        truncated: bool,
    },
    TerminalSessionFinished {
        turn: u32,
        id: String,
        handle: String,
        status: String,
        exit_code: Option<i32>,
    },
    TokenUsage {
        turn: u32,
        usage: AgentTokenUsage,
    },
    ContextWindowUpdated {
        turn: u32,
        used_tokens: u32,
    },
    SteeringQueued {
        message: AgentMessage,
    },
    FollowUpQueued {
        message: AgentMessage,
    },
    QueueDrained {
        kind: QueueKind,
        count: usize,
    },
    CompactionStarted {
        reason: CompactionReason,
        tokens_before: usize,
        message_count: usize,
    },
    CompactionProgress {
        phase: CompactionPhase,
        percent: u8,
    },
    CompactionApplied {
        summary: CompactionSummary,
    },
    MessageAppended {
        message: AgentMessage,
    },
    TurnFinished {
        turn: u32,
        stop_reason: StopReason,
    },
    RunFinished {
        turn: u32,
        stop_reason: StopReason,
    },
    Error {
        turn: u32,
        message: String,
        /// Stable error code (e.g. `"provider.rate_limit"`). `None` for old sessions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        /// Retry-After hint in seconds, if the provider included one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after: Option<u64>,
    },
    /// Plan mode was entered — read-only exploration plus plan file writes.
    PlanModeEntered {
        turn: u32,
        id: String,
    },
    /// Plan mode was exited — normal tool access restored.
    PlanModeExited {
        turn: u32,
        id: String,
    },
    /// Plan-mode active state changed (for TUI replay / status updates).
    PlanUpdated {
        turn: u32,
        enabled: bool,
    },
    /// Structured todo list was updated (for persistence + TUI panel).
    TodoUpdated {
        turn: u32,
        todos: Vec<TodoEventData>,
    },
    /// `AskUser` question request (reverse-RPC from tool to host).
    QuestionRequested {
        turn: u32,
        id: String,
        questions: Vec<QuestionEventData>,
    },
    DelegateStarted {
        turn: u32,
        agent: AgentSnapshot,
    },
    DelegateUpdated {
        turn: u32,
        agent: AgentSnapshot,
    },
    DelegateFinished {
        turn: u32,
        agent: AgentSnapshot,
    },
    DelegateSwarmStarted {
        turn: u32,
        swarm: SwarmSnapshot,
    },
    DelegateSwarmUpdated {
        turn: u32,
        swarm: SwarmSnapshot,
    },
    DelegateSwarmFinished {
        turn: u32,
        swarm: SwarmSnapshot,
    },
    WorkflowStarted {
        turn: u32,
        workflow: crate::workflow::WorkflowSnapshot,
    },
    WorkflowUpdated {
        turn: u32,
        workflow: crate::workflow::WorkflowSnapshot,
    },
    WorkflowFinished {
        turn: u32,
        workflow: crate::workflow::WorkflowSnapshot,
    },
}

// ---------------------------------------------------------------------------
// Value types for new events
// ---------------------------------------------------------------------------

/// Serializable representation of a single todo item, used in
/// [`AgentEvent::TodoUpdated`]. Kept in `events.rs` so that persistence
/// does not depend on the `tools` module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TodoEventData {
    /// Short, human-readable description of the task.
    pub title: String,
    /// Current status: `"pending"`, `"in_progress"`, or `"done"`.
    pub status: String,
}

/// Serializable representation of a single question in an `AskUser` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionEventData {
    /// The question text (should end with `?`).
    pub question: String,
    /// Optional short header displayed above the question (max ~12 chars).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    /// Optional longer body / context for the question.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Available options the user can choose from.
    pub options: Vec<QuestionOptionData>,
    /// Whether the user may select multiple options.
    pub multi_select: bool,
}

/// Serializable representation of a single option in a question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionOptionData {
    /// Short label shown as the choice.
    pub label: String,
    /// Optional description explaining the option.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum QueueKind {
    Steering,
    FollowUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum CompactionReason {
    Threshold,
    Manual,
}

/// Whether compaction was triggered by the user (`/compact`) or automatically
/// by the threshold strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum CompactionSource {
    Manual,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum CompactionPhase {
    Estimating,
    SelectingBoundary,
    Summarizing,
    Applying,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompactionSummary {
    pub summary: String,
    pub tokens_before: usize,
    /// Estimated token count *after* compaction (summary + retained messages).
    pub tokens_after: usize,
    pub first_kept_message_index: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todo_event_data_serializes() {
        let data = TodoEventData {
            title: "Task".into(),
            status: "in_progress".into(),
        };
        let json = serde_json::to_string(&data).expect("serialize");
        assert!(json.contains("\"title\":\"Task\""));
        assert!(json.contains("\"status\":\"in_progress\""));
        let back: TodoEventData = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(data, back);
    }

    #[test]
    fn question_event_data_serializes() {
        let data = QuestionEventData {
            question: "Which?".into(),
            header: Some("Choice".into()),
            body: None,
            options: vec![QuestionOptionData {
                label: "A".into(),
                description: Some("desc".into()),
            }],
            multi_select: false,
        };
        let json = serde_json::to_string(&data).expect("serialize");
        assert!(json.contains("\"question\":\"Which?\""));
        assert!(json.contains("\"multi_select\":false"));
        // body is None and should be skipped.
        assert!(!json.contains("\"body\""));
        let back: QuestionEventData = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(data, back);
    }

    #[test]
    fn plan_mode_entered_serializes() {
        let event = AgentEvent::PlanModeEntered {
            turn: 3,
            id: "p1".into(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"PlanModeEntered\""));
        assert!(json.contains("\"id\":\"p1\""));
        let back: AgentEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, back);
    }

    #[test]
    fn plan_mode_exited_serializes() {
        let event = AgentEvent::PlanModeExited {
            turn: 5,
            id: "p1".into(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"PlanModeExited\""));
        let back: AgentEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, back);
    }

    #[test]
    fn plan_updated_serializes() {
        let event = AgentEvent::PlanUpdated {
            turn: 2,
            enabled: true,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"PlanUpdated\""));
        assert!(json.contains("\"enabled\":true"));
        let back: AgentEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, back);
    }

    #[test]
    fn todo_updated_serializes() {
        let event = AgentEvent::TodoUpdated {
            turn: 2,
            todos: vec![
                TodoEventData {
                    title: "A".into(),
                    status: "done".into(),
                },
                TodoEventData {
                    title: "B".into(),
                    status: "pending".into(),
                },
            ],
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"TodoUpdated\""));
        assert!(json.contains("\"todos\""));
        let back: AgentEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, back);
    }

    #[test]
    fn question_requested_serializes() {
        let event = AgentEvent::QuestionRequested {
            turn: 1,
            id: "q-123".into(),
            questions: vec![QuestionEventData {
                question: "Test?".into(),
                header: None,
                body: None,
                options: vec![
                    QuestionOptionData {
                        label: "Yes".into(),
                        description: None,
                    },
                    QuestionOptionData {
                        label: "No".into(),
                        description: None,
                    },
                ],
                multi_select: false,
            }],
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"QuestionRequested\""));
        assert!(json.contains("\"q-123\""));
        let back: AgentEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, back);
    }

    #[test]
    fn error_with_code_serializes() {
        let event = AgentEvent::Error {
            turn: 1,
            message: "rate limited".into(),
            code: Some("provider.rate_limit".into()),
            retry_after: Some(30),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"code\":\"provider.rate_limit\""));
        assert!(json.contains("\"retry_after\":30"));
        let back: AgentEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, back);
    }

    #[test]
    fn error_without_code_backward_compatible() {
        // Old JSONL format without code/retry_after
        let json = r#"{"Error":{"turn":1,"message":"old format"}}"#;
        let event: AgentEvent = serde_json::from_str(json).expect("deserialize");
        match event {
            AgentEvent::Error {
                turn,
                message,
                code,
                retry_after,
            } => {
                assert_eq!(turn, 1);
                assert_eq!(message, "old format");
                assert_eq!(code, None);
                assert_eq!(retry_after, None);
            }
            _ => panic!("expected Error variant"),
        }
    }
}
