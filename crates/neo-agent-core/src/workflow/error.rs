use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkflowError {
    #[error("lua error: {0}")]
    Lua(String),
    #[error("workflow failed: {0}")]
    Failed(String),
    #[error("host API error: {0}")]
    Host(String),
}
