//! Host JSON Schema validation for workflow structured outputs.
//!
//! Provider-native structured-output hints are optional wire metadata. Both the
//! provider-native path and the assistant-text fallback share this single host
//! validator. Definition-level input/output schema compilation in
//! [`super::definition`] also uses [`CompiledSchema`] — there is no second
//! schema engine. Neo never accepts provider wire acceptance as proof of validity.
//!
//! Final Lua returns are validated here too. A final-result schema failure never
//! triggers a model call: no child session owns the Lua return.

use std::error::Error as StdError;
use std::fmt;
use std::sync::Arc;

use jsonschema::{Draft, Retrieve, Uri, ValidationError, Validator};
use neo_ai::{RequestOptions, ResponseFormat};
use serde::Deserialize;
use serde_json::Value;

/// Origin of a candidate structured output value.
#[derive(Debug, Clone, PartialEq)]
pub enum StructuredOutputSource {
    /// Already-decoded provider-native structured value.
    ProviderNative(Value),
    /// Final assistant text. Must be exactly one JSON value (no fences/prose).
    AssistantText(String),
}

/// Host-side schema / strict-JSON failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaValidationError {
    pub code: SchemaErrorCode,
    pub message: String,
    pub instance_path: String,
    pub schema_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SchemaErrorCode {
    /// Schema document itself is invalid or uses unsupported remote refs.
    InvalidSchema,
    /// Text was not exactly one JSON value.
    StrictJsonFailed,
    /// Instance failed JSON Schema validation.
    SchemaInvalid,
}

impl SchemaErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidSchema => "invalid_schema",
            Self::StrictJsonFailed => "strict_json_failed",
            Self::SchemaInvalid => "schema_invalid",
        }
    }
}

impl fmt::Display for SchemaValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl StdError for SchemaValidationError {}

/// Compiled host schema. Remote `$ref` resolution is disabled.
#[derive(Clone)]
pub struct CompiledSchema {
    validator: Arc<Validator>,
    schema: Value,
}

impl fmt::Debug for CompiledSchema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledSchema")
            .field("schema", &self.schema)
            .finish_non_exhaustive()
    }
}

impl CompiledSchema {
    /// Compile a JSON Schema Draft 2020-12 document.
    ///
    /// Remote network/file `$ref` resolution is rejected. Internal JSON Pointer
    /// references within the supplied document remain allowed.
    pub fn compile(schema: &Value) -> Result<Self, SchemaValidationError> {
        let validator = jsonschema::options()
            .with_draft(Draft::Draft202012)
            .with_retriever(DenyRemoteRetriever)
            .build(schema)
            .map_err(|err| SchemaValidationError {
                code: SchemaErrorCode::InvalidSchema,
                message: err.to_string(),
                instance_path: String::new(),
                schema_path: validation_schema_path(&err),
            })?;
        Ok(Self {
            validator: Arc::new(validator),
            schema: schema.clone(),
        })
    }

    #[must_use]
    pub fn schema(&self) -> &Value {
        &self.schema
    }

    /// Validate an instance against this compiled schema.
    pub fn validate_instance(&self, instance: &Value) -> Result<(), SchemaValidationError> {
        let mut errors = self.validator.iter_errors(instance);
        if let Some(err) = errors.next() {
            return Err(SchemaValidationError {
                code: SchemaErrorCode::SchemaInvalid,
                message: err.to_string(),
                instance_path: err.instance_path().to_string(),
                schema_path: err.schema_path().to_string(),
            });
        }
        Ok(())
    }
}

/// Parse exactly one JSON value from text. No fence unwrapping, no prose scan.
pub fn parse_strict_json_value(text: &str) -> Result<Value, SchemaValidationError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(SchemaValidationError {
            code: SchemaErrorCode::StrictJsonFailed,
            message: "expected exactly one JSON value; got empty text".to_owned(),
            instance_path: String::new(),
            schema_path: String::new(),
        });
    }
    let mut deserializer = serde_json::Deserializer::from_str(trimmed);
    let value = Value::deserialize(&mut deserializer).map_err(|err| SchemaValidationError {
        code: SchemaErrorCode::StrictJsonFailed,
        message: format!("expected exactly one JSON value: {err}"),
        instance_path: String::new(),
        schema_path: String::new(),
    })?;
    // Reject trailing content after one complete value.
    match deserializer.end() {
        Ok(()) => Ok(value),
        Err(err) => Err(SchemaValidationError {
            code: SchemaErrorCode::StrictJsonFailed,
            message: format!("expected exactly one JSON value; trailing content: {err}"),
            instance_path: String::new(),
            schema_path: String::new(),
        }),
    }
}

/// Accept structured output from either provider-native or text fallback path.
/// Both routes share this single host validator.
pub fn accept_structured_output(
    schema: &CompiledSchema,
    source: StructuredOutputSource,
) -> Result<Value, SchemaValidationError> {
    let value = match source {
        StructuredOutputSource::ProviderNative(value) => value,
        StructuredOutputSource::AssistantText(text) => parse_strict_json_value(&text)?,
    };
    schema.validate_instance(&value)?;
    Ok(value)
}

/// Validate a final Lua-converted JSON value against the definition output schema.
///
/// This path never consults a model: there is no child session and no hidden
/// repair turn for top-level Lua returns. Callers map failures to
/// `failed(schema_invalid_final_result)`.
pub fn validate_final_lua_result(
    schema: &CompiledSchema,
    value: &Value,
) -> Result<(), SchemaValidationError> {
    schema.validate_instance(value).map_err(|mut err| {
        if err.code == SchemaErrorCode::SchemaInvalid {
            err.message = format!("schema_invalid_final_result: {}", err.message);
        }
        err
    })
}

/// Child-call composition seam: attach a provider-neutral response-format hint.
///
/// Unsupported providers omit the wire mapping; host validation still runs on
/// whatever assistant value is produced.
pub fn attach_response_format_hint(
    options: &mut RequestOptions,
    name: impl Into<String>,
    schema: Value,
    strict: bool,
) {
    options.response_format = Some(ResponseFormat {
        name: name.into(),
        schema,
        strict,
    });
}

struct DenyRemoteRetriever;

impl Retrieve for DenyRemoteRetriever {
    fn retrieve(&self, uri: &Uri<String>) -> Result<Value, Box<dyn StdError + Send + Sync>> {
        Err(format!("remote $ref resolution is disabled: {uri}").into())
    }
}

fn validation_schema_path(err: &ValidationError<'_>) -> String {
    err.schema_path().to_string()
}
