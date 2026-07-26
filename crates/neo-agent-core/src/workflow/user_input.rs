//! Durable AwaitingUser request/answer types (design §29).
//!
//! The runtime owns request validation and answer acceptance. The TUI is not an
//! answer owner. Answers are persisted in the journal — this surface must not
//! request secrets or passwords.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::error::{WorkflowError, WorkflowErrorCode};
use super::schema::CompiledSchema;
use super::state::WorkflowActor;

/// Who may answer a durable user-input request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UserAnswerPolicy {
    /// Only a human may answer (default).
    #[default]
    Human,
    /// A human or a model may answer.
    HumanOrModel,
}

impl UserAnswerPolicy {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::HumanOrModel => "human_or_model",
        }
    }

    /// Parse the Lua/host string form.
    pub fn parse(raw: &str) -> Result<Self, WorkflowError> {
        match raw.trim() {
            "" | "human" => Ok(Self::Human),
            "human_or_model" => Ok(Self::HumanOrModel),
            other => Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                format!("answer_policy must be \"human\" or \"human_or_model\", got {other:?}"),
            )),
        }
    }

    /// Whether `actor` is allowed to submit an answer under this policy.
    #[must_use]
    pub fn allows_actor(self, actor: WorkflowActor) -> bool {
        matches!(
            (self, actor),
            (_, WorkflowActor::Human) | (Self::HumanOrModel, WorkflowActor::Model)
        )
    }
}

/// Host-facing await_user input after Lua decode (pre-effect validation).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AwaitUserInput {
    pub prompt: String,
    pub answer_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_policy: Option<UserAnswerPolicy>,
}

impl AwaitUserInput {
    /// Compile schema and validate optional default before any durable effect.
    pub fn prepare(&self) -> Result<PreparedUserInputRequest, WorkflowError> {
        let prompt = self.prompt.trim();
        if prompt.is_empty() {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                "await_user prompt must be non-empty",
            ));
        }
        if let Some(title) = self.title.as_deref()
            && title.trim().is_empty()
        {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                "await_user title must be non-empty when present",
            ));
        }
        let compiled = CompiledSchema::compile(&self.answer_schema).map_err(|err| {
            WorkflowError::coded(
                WorkflowErrorCode::InvalidSchema,
                format!("await_user answer_schema: {err}"),
            )
        })?;
        if let Some(default) = &self.default {
            compiled.validate_instance(default).map_err(|err| {
                WorkflowError::coded(
                    WorkflowErrorCode::InvalidUserAnswer,
                    format!("await_user default failed answer_schema: {err}"),
                )
            })?;
        }
        Ok(PreparedUserInputRequest {
            prompt: prompt.to_owned(),
            answer_schema: self.answer_schema.clone(),
            default: self.default.clone(),
            title: self
                .title
                .as_ref()
                .map(|t| t.trim().to_owned())
                .filter(|t| !t.is_empty()),
            answer_policy: self.answer_policy.unwrap_or_default(),
            compiled,
        })
    }
}

/// Schema-compiled request ready for durable append.
#[derive(Clone)]
pub struct PreparedUserInputRequest {
    pub prompt: String,
    pub answer_schema: Value,
    pub default: Option<Value>,
    pub title: Option<String>,
    pub answer_policy: UserAnswerPolicy,
    pub compiled: CompiledSchema,
}

/// Rehydrated open (or historical) user-input request projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingUserInput {
    pub request_id: String,
    pub prompt: String,
    pub answer_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub answer_policy: UserAnswerPolicy,
    /// Journaled answer when the request has been answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Value>,
}

impl PendingUserInput {
    /// Validate a candidate answer against this request's policy and schema.
    pub fn validate_answer(
        &self,
        value: &Value,
        actor: WorkflowActor,
    ) -> Result<(), WorkflowError> {
        if !self.answer_policy.allows_actor(actor) {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidUserAnswer,
                format!(
                    "answer_policy {} rejects actor {:?}",
                    self.answer_policy.as_str(),
                    actor
                ),
            ));
        }
        let compiled = CompiledSchema::compile(&self.answer_schema).map_err(|err| {
            WorkflowError::coded(
                WorkflowErrorCode::InvalidSchema,
                format!("await_user answer_schema: {err}"),
            )
        })?;
        compiled.validate_instance(value).map_err(|err| {
            WorkflowError::coded(
                WorkflowErrorCode::InvalidUserAnswer,
                format!("answer failed request schema: {err}"),
            )
        })?;
        Ok(())
    }
}

/// Deterministic request identity from host call index (design §29.2).
#[must_use]
pub fn request_id_for_call_index(call_index: u64) -> String {
    format!("req_c{call_index}")
}
