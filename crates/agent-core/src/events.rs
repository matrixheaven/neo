use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use std::path::PathBuf;

use crate::{AgentMessage, AgentToolCall, PermissionOperation, ToolResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    MaxTurns,
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
    ApprovalRequested {
        turn: u32,
        id: String,
        operation: PermissionOperation,
        subject: String,
        arguments: serde_json::Value,
    },
    ShellCommandStarted {
        turn: u32,
        id: String,
        command: String,
        cwd: PathBuf,
    },
    ShellCommandFinished {
        turn: u32,
        id: String,
        exit_code: Option<i32>,
        stdout: String,
        stderr: String,
        truncated: bool,
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
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum QueueKind {
    Steering,
    FollowUp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompactionSummary {
    pub summary: String,
    pub tokens_before: usize,
    pub first_kept_message_index: usize,
}
