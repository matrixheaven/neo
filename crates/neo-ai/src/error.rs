use thiserror::Error;

#[derive(Debug, Error)]
pub enum AiError {
    #[error("provider configuration error: {0}")]
    Configuration(String),
    #[error("provider stream error: {0}")]
    Stream(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("request was cancelled")]
    Cancelled,
}
