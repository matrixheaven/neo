use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkflowError {
    #[error("lua error: {0}")]
    Lua(String),
    #[error("workflow failed: {0}")]
    Failed(String),
    #[error("host API error: {0}")]
    Host(String),
    #[error("journal error: {0}")]
    Journal(String),
    #[error("journal total size limit exceeded")]
    JournalTotalLimitExceeded,
    #[error("invalid workflow input: {0}")]
    InvalidInput(String),
    #[error("resource limited: {0}")]
    ResourceLimited(String),
    #[error("run not found: {0}")]
    NotFound(String),
}
