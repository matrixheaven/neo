use thiserror::Error;

use crate::{ToolError, compaction};

#[derive(Debug, Error)]
pub enum AgentRuntimeError {
    #[error("model stream failed: {0}")]
    Model(#[from] neo_ai::AiError),
    #[error("tool execution failed: {0}")]
    Tool(#[from] ToolError),
    #[error("runtime I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("compaction failed: {0}")]
    Compaction(#[from] compaction::CompactionError),
    #[error("turn cancelled")]
    Cancelled,
}
