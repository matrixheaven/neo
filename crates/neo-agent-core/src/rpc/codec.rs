use super::{RpcError, RpcErrorCode, RpcMessage, RpcResponse};

#[derive(Debug, thiserror::Error)]
pub enum RpcCodecError {
    #[error("JSONL frame is empty")]
    EmptyFrame,
    #[error("JSONL frame contains trailing content")]
    TrailingContent,
    #[error("failed to decode JSONL frame on line {line}: {source}")]
    Line {
        line: usize,
        source: Box<RpcCodecError>,
    },
    #[error("failed to serialize RPC message: {0}")]
    Serialize(serde_json::Error),
    #[error("failed to deserialize RPC message: {0}")]
    Deserialize(serde_json::Error),
}

impl RpcCodecError {
    #[must_use]
    pub fn to_response(&self, id: impl Into<String>) -> RpcResponse {
        RpcResponse::failure(id, RpcError::new(self.error_code(), self.to_string(), None))
    }

    #[must_use]
    pub fn error_code(&self) -> RpcErrorCode {
        match self {
            Self::EmptyFrame | Self::TrailingContent | Self::Deserialize(_) | Self::Line { .. } => {
                RpcErrorCode::ParseError
            }
            Self::Serialize(_) => RpcErrorCode::InternalError,
        }
    }
}

pub struct JsonlCodec;

impl JsonlCodec {
    pub fn encode(message: &RpcMessage) -> Result<String, RpcCodecError> {
        let mut line = serde_json::to_string(message).map_err(RpcCodecError::Serialize)?;
        line.push('\n');
        Ok(line)
    }

    pub fn encode_many<'a>(
        messages: impl IntoIterator<Item = &'a RpcMessage>,
    ) -> Result<String, RpcCodecError> {
        let mut output = String::new();
        for message in messages {
            output.push_str(&Self::encode(message)?);
        }
        Ok(output)
    }

    pub fn decode_line(line: &str) -> Result<RpcMessage, RpcCodecError> {
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        if trimmed.trim().is_empty() {
            return Err(RpcCodecError::EmptyFrame);
        }
        if trimmed.contains('\n') || trimmed.contains('\r') {
            return Err(RpcCodecError::TrailingContent);
        }
        serde_json::from_str(trimmed).map_err(RpcCodecError::Deserialize)
    }

    pub fn decode_stream(input: &str) -> Result<Vec<RpcMessage>, RpcCodecError> {
        input
            .lines()
            .enumerate()
            .map(|(index, line)| {
                Self::decode_line(line).map_err(|source| RpcCodecError::Line {
                    line: index + 1,
                    source: Box::new(source),
                })
            })
            .collect()
    }
}
