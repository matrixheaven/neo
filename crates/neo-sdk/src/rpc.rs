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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RpcSessionRecord {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_user_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub children: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RpcSessionsListResult {
    pub sessions: Vec<RpcSessionRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RpcSessionGetResult {
    #[serde(flatten)]
    pub record: RpcSessionRecord,
    pub path: String,
    pub messages: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RpcSessionExportHtmlResult {
    pub session_id: String,
    pub html: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RpcCommandRecord {
    pub name: String,
    pub kind: RpcCommandKind,
    pub template: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    pub location: String,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RpcCommandKind {
    PromptTemplate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RpcCommandsResult {
    pub commands: Vec<RpcCommandRecord>,
}

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
