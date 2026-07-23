use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum WorkflowError {
    #[error("lua error: {0}")]
    Lua(String),
    #[error("workflow failed: {0}")]
    Failed(String),
    #[error("host API error: {0}")]
    Host(String),
    #[error("journal error: {0}")]
    Journal(String),
    #[error("journal record size {observed} exceeds limit {limit}")]
    JournalRecordLimitExceeded { observed: u64, limit: u64 },
    #[error("journal total size limit exceeded")]
    JournalTotalLimitExceeded,
    #[error("invalid workflow input: {0}")]
    InvalidInput(String),
    #[error("invalid_workflow_operation: {0}")]
    InvalidOperation(String),
    #[error("resource limited: {0}")]
    ResourceLimited(String),
    #[error("workflow paused: {0}")]
    Paused(String),
    #[error("workflow cancelled: {0}")]
    Cancelled(String),
    #[error("run not found: {0}")]
    NotFound(String),
}
