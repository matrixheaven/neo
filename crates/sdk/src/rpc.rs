use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RpcMessage {
    Request(RpcRequest),
    Response(RpcResponse),
    Notification(RpcNotification),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RpcRequest {
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

impl RpcRequest {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        method: impl Into<String>,
        params: serde_json::Value,
    ) -> Self {
        Self {
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RpcNotification {
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

impl RpcNotification {
    #[must_use]
    pub fn new(method: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RpcResponse {
    pub id: String,
    #[serde(flatten)]
    pub outcome: RpcOutcome,
}

impl RpcResponse {
    #[must_use]
    pub fn success(id: impl Into<String>, result: serde_json::Value) -> Self {
        Self {
            id: id.into(),
            outcome: RpcOutcome::Success { result },
        }
    }

    #[must_use]
    pub fn failure(id: impl Into<String>, error: RpcError) -> Self {
        Self {
            id: id.into(),
            outcome: RpcOutcome::Failure { error },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum RpcOutcome {
    Success { result: serde_json::Value },
    Failure { error: RpcError },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RpcError {
    pub code: RpcErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl RpcError {
    #[must_use]
    pub fn new(
        code: RpcErrorCode,
        message: impl Into<String>,
        data: Option<serde_json::Value>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            data,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RpcErrorCode {
    ParseError,
    InvalidRequest,
    MethodNotFound,
    InvalidParams,
    InternalError,
}

#[derive(Debug, thiserror::Error)]
pub enum RpcCodecError {
    #[error("JSONL frame is empty")]
    EmptyFrame,
    #[error("JSONL frame contains trailing content")]
    TrailingContent,
    #[error("failed to serialize RPC message: {0}")]
    Serialize(#[from] serde_json::Error),
}

pub struct JsonlCodec;

impl JsonlCodec {
    pub fn encode(message: &RpcMessage) -> Result<String, RpcCodecError> {
        let mut line = serde_json::to_string(message)?;
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
        Ok(serde_json::from_str(trimmed)?)
    }

    pub fn decode_stream(input: &str) -> Result<Vec<RpcMessage>, RpcCodecError> {
        input.lines().map(Self::decode_line).collect()
    }
}
