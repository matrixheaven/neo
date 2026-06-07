use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentMessage, AgentToolCall};

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
    TurnStarted {
        turn: u32,
    },
    MessageStarted {
        turn: u32,
        id: String,
    },
    TextDelta {
        turn: u32,
        text: String,
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
    MessageAppended {
        message: AgentMessage,
    },
    TurnFinished {
        turn: u32,
        stop_reason: StopReason,
    },
    Error {
        turn: u32,
        message: String,
    },
}
