//! Typed workflow definitions and canonical content-revision hashing.
//!
//! Every source adapter (file pair, builtin bytes, dynamic `Workflow` tool
//! input) produces one [`ResolvedWorkflowDefinition`]. The content revision is SHA-256
//! over exact length-prefixed framing of the canonical manifest JSON and Lua
//! source bytes. Path, mtime, registry scope, and precedence are never hash
//! inputs. Schema compilation reuses the single host [`CompiledSchema`] owner.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::error::{WorkflowError, WorkflowErrorCode};
use super::journal::canonicalize_json;
use super::limits::WorkflowLimits;
use super::schema::{CompiledSchema, SchemaErrorCode};
use super::state::{
    WorkflowName, WorkflowPhase, WorkflowRevision, WorkflowSourceOrigin, validate_portable_name,
};

/// ASCII framing magic including the trailing NUL byte.
pub const DEFINITION_REVISION_PREFIX: &[u8] = b"neo-workflow-definition\0";

/// Fully typed manifest fields that participate in the content revision.
///
/// Keys are sorted by UTF-8 byte order when serialized to
/// `canonical_manifest_json`. Optional `input_schema` is omitted when absent.
/// `name` is not a hash input: the filename stem (or dynamic input name) is the
/// lookup key and lives on [`ResolvedWorkflowDefinition`] only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanonicalWorkflowManifest {
    pub display_name: String,
    pub description: String,
    pub phases: Vec<WorkflowPhase>,
    /// Lowercase SHA-256 of the exact Lua source bytes.
    pub source_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    pub output_schema: Value,
}

/// One resolved definition accepted by runtime creation.
///
/// Runtime does not rescan files, re-resolve precedence, or re-infer metadata.
#[derive(Clone)]
pub struct ResolvedWorkflowDefinition {
    pub name: WorkflowName,
    pub display_name: String,
    pub description: String,
    pub phases: Vec<WorkflowPhase>,
    pub input_schema: Option<Value>,
    pub output_schema: Value,
    pub compiled_input_schema: Option<CompiledSchema>,
    pub compiled_output_schema: CompiledSchema,
    /// Exact Lua source as UTF-8 text (bytes match the hashed source).
    pub lua_source: String,
    pub source_sha256: String,
    pub source_origin: WorkflowSourceOrigin,
    /// Display-only locator; never a trust or hash input.
    pub source_locator: Option<String>,
    pub revision: WorkflowRevision,
    /// Whitespace-free canonical manifest JSON bytes used for the revision frame.
    pub canonical_manifest_json: Vec<u8>,
}

impl std::fmt::Debug for ResolvedWorkflowDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedWorkflowDefinition")
            .field("name", &self.name)
            .field("display_name", &self.display_name)
            .field("description", &self.description)
            .field("phases", &self.phases)
            .field("input_schema", &self.input_schema)
            .field("output_schema", &self.output_schema)
            .field("source_sha256", &self.source_sha256)
            .field("source_origin", &self.source_origin)
            .field("source_locator", &self.source_locator)
            .field("revision", &self.revision)
            .field(
                "canonical_manifest_json_len",
                &self.canonical_manifest_json.len(),
            )
            .field("lua_source_len", &self.lua_source.len())
            .finish_non_exhaustive()
    }
}

/// Model-facing dynamic definition adapter (`Workflow` tool inline shape).
///
/// Unknown fields are rejected. Runtime limits, concurrency, budgets, output
/// parsing modes, and execution backends are not accepted.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicWorkflowDefinitionInput {
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    pub description: String,
    pub phases: Vec<WorkflowPhase>,
    /// Complete Lua source.
    pub script: String,
    #[serde(default)]
    pub input_schema: Option<Value>,
    /// Required final structured-output schema.
    pub output_schema: Value,
}

/// TOML file-backed manifest shape (paired with `<stem>.lua`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileWorkflowToml {
    /// Optional; when present must match the filename stem exactly.
    #[serde(default)]
    name: Option<String>,
    display_name: String,
    description: String,
    phases: Vec<WorkflowPhase>,
    source_sha256: String,
    #[serde(default)]
    input_schema: Option<toml::Value>,
    output_schema: toml::Value,
}

/// Compute the exact definition content revision.
///
/// Framing:
/// `ASCII "neo-workflow-definition\0" || u64be(manifest_len) || manifest ||
///  u64be(source_len) || source`.
#[must_use]
pub fn compute_definition_revision(
    canonical_manifest_json: &[u8],
    source_bytes: &[u8],
) -> WorkflowRevision {
    let frame = build_definition_revision_frame(canonical_manifest_json, source_bytes);
    WorkflowRevision::from_bytes(&frame)
}

/// Build the exact byte frame hashed for a definition revision.
#[must_use]
pub fn build_definition_revision_frame(
    canonical_manifest_json: &[u8],
    source_bytes: &[u8],
) -> Vec<u8> {
    let manifest_len = u64::try_from(canonical_manifest_json.len()).unwrap_or(u64::MAX);
    let source_len = u64::try_from(source_bytes.len()).unwrap_or(u64::MAX);
    let mut frame = Vec::with_capacity(
        DEFINITION_REVISION_PREFIX
            .len()
            .saturating_add(8)
            .saturating_add(canonical_manifest_json.len())
            .saturating_add(8)
            .saturating_add(source_bytes.len()),
    );
    frame.extend_from_slice(DEFINITION_REVISION_PREFIX);
    frame.extend_from_slice(&manifest_len.to_be_bytes());
    frame.extend_from_slice(canonical_manifest_json);
    frame.extend_from_slice(&source_len.to_be_bytes());
    frame.extend_from_slice(source_bytes);
    frame
}

/// Serialize a typed manifest to whitespace-free UTF-8-byte-sorted canonical JSON.
pub fn serialize_canonical_manifest(
    manifest: &CanonicalWorkflowManifest,
) -> Result<Vec<u8>, WorkflowError> {
    let value = serde_json::to_value(manifest).map_err(|err| {
        WorkflowError::coded(
            WorkflowErrorCode::InvalidManifest,
            format!("canonical manifest serialization failed: {err}"),
        )
    })?;
    let canonical = canonicalize_json(&value);
    serde_json::to_vec(&canonical).map_err(|err| {
        WorkflowError::coded(
            WorkflowErrorCode::InvalidManifest,
            format!("canonical manifest JSON encoding failed: {err}"),
        )
    })
}

/// Lowercase SHA-256 hex of exact source bytes.
#[must_use]
pub fn source_sha256_hex(source_bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(source_bytes))
}

/// Resolve a dynamic definition input into the single resolved form.
pub fn resolve_dynamic_definition(
    input: DynamicWorkflowDefinitionInput,
    limits: &WorkflowLimits,
) -> Result<ResolvedWorkflowDefinition, WorkflowError> {
    let name = WorkflowName::parse(input.name.trim())?;
    let display_name = input
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| name.as_str())
        .to_owned();
    let description = input.description.trim().to_owned();
    if description.is_empty() {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidDefinition,
            "description must not be empty",
        ));
    }

    let source_bytes = input.script.as_bytes();
    reject_source_limit(source_bytes, limits)?;
    if source_bytes.is_empty() || input.script.trim().is_empty() {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidDefinition,
            "script must not be empty",
        ));
    }

    let phases = validate_phases(&input.phases)?;
    let output_schema = input.output_schema;
    reject_schema_limit(&output_schema, "output_schema", limits)?;
    let source_sha256 = source_sha256_hex(source_bytes);
    let typed = CanonicalWorkflowManifest {
        display_name,
        description,
        phases,
        source_sha256: source_sha256.clone(),
        input_schema: input.input_schema,
        output_schema,
    };
    finish_resolved(
        name,
        typed,
        input.script,
        source_sha256,
        WorkflowSourceOrigin::Dynamic,
        None,
        limits,
    )
}

/// Resolve a paired `.lua` + `.workflow.toml` (file or builtin bytes).
///
/// `stem_name` is the filename stem and the canonical lookup name. Manifest
/// `source_sha256` must match the exact `source_bytes`. Origin must not be
/// `dynamic` (use [`resolve_dynamic_definition`]).
pub fn resolve_paired_definition(
    stem_name: &str,
    manifest_bytes: &[u8],
    source_bytes: &[u8],
    origin: WorkflowSourceOrigin,
    source_locator: Option<String>,
    limits: &WorkflowLimits,
) -> Result<ResolvedWorkflowDefinition, WorkflowError> {
    if matches!(origin, WorkflowSourceOrigin::Dynamic) {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidDefinition,
            "paired definitions cannot use dynamic origin",
        ));
    }

    reject_manifest_limit(manifest_bytes, limits)?;
    reject_source_limit(source_bytes, limits)?;
    if source_bytes.is_empty() {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidDefinition,
            "Lua source must not be empty",
        ));
    }

    let name = WorkflowName::parse(stem_name)?;
    let raw = decode_file_manifest(manifest_bytes)?;
    validate_manifest_name_against_stem(&raw, &name)?;
    let expected_sha = verify_source_sha256(&raw.source_sha256, source_bytes)?;
    let (display_name, description, phases, input_schema, output_schema) =
        materialize_file_manifest_fields(raw, limits)?;

    let lua_source = std::str::from_utf8(source_bytes)
        .map_err(|_| {
            WorkflowError::coded(
                WorkflowErrorCode::InvalidDefinition,
                "Lua source must be valid UTF-8",
            )
        })?
        .to_owned();

    let typed = CanonicalWorkflowManifest {
        display_name,
        description,
        phases,
        source_sha256: expected_sha.clone(),
        input_schema,
        output_schema,
    };
    finish_resolved(
        name,
        typed,
        lua_source,
        expected_sha,
        origin,
        source_locator,
        limits,
    )
}

fn decode_file_manifest(manifest_bytes: &[u8]) -> Result<FileWorkflowToml, WorkflowError> {
    let manifest_text = std::str::from_utf8(manifest_bytes).map_err(|_| {
        WorkflowError::coded(
            WorkflowErrorCode::InvalidManifest,
            "manifest must be valid UTF-8",
        )
    })?;
    let raw: FileWorkflowToml = toml::from_str(manifest_text).map_err(|err| {
        WorkflowError::coded(
            WorkflowErrorCode::InvalidManifest,
            format!("manifest TOML decode failed: {err}"),
        )
    })?;
    Ok(raw)
}

fn validate_manifest_name_against_stem(
    raw: &FileWorkflowToml,
    name: &WorkflowName,
) -> Result<(), WorkflowError> {
    let Some(manifest_name) = raw.name.as_deref() else {
        return Ok(());
    };
    validate_portable_name(manifest_name, "manifest name")?;
    if manifest_name != name.as_str() {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidManifest,
            format!(
                "manifest name {manifest_name:?} conflicts with filename stem {:?}",
                name.as_str()
            ),
        ));
    }
    Ok(())
}

fn verify_source_sha256(declared: &str, source_bytes: &[u8]) -> Result<String, WorkflowError> {
    let expected_sha = source_sha256_hex(source_bytes);
    let declared_sha = declared.trim().to_ascii_lowercase();
    WorkflowRevision::parse(&declared_sha).map_err(|_| {
        WorkflowError::coded(
            WorkflowErrorCode::InvalidManifest,
            "source_sha256 must be 64 lowercase hex characters",
        )
    })?;
    if declared_sha != expected_sha {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidManifest,
            format!(
                "source_sha256 mismatch: manifest declares {declared_sha}, source hashes to {expected_sha}"
            ),
        ));
    }
    Ok(expected_sha)
}

type MaterializedManifestFields = (String, String, Vec<WorkflowPhase>, Option<Value>, Value);

fn materialize_file_manifest_fields(
    raw: FileWorkflowToml,
    limits: &WorkflowLimits,
) -> Result<MaterializedManifestFields, WorkflowError> {
    let display_name = raw.display_name.trim().to_owned();
    if display_name.is_empty() {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidManifest,
            "display_name must not be empty",
        ));
    }
    let description = raw.description.trim().to_owned();
    if description.is_empty() {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidManifest,
            "description must not be empty",
        ));
    }
    let phases = validate_phases(&raw.phases)?;
    let output_schema = toml_value_to_json(raw.output_schema)?;
    reject_schema_limit(&output_schema, "output_schema", limits)?;
    let input_schema = match raw.input_schema {
        Some(schema) => {
            let json = toml_value_to_json(schema)?;
            reject_schema_limit(&json, "input_schema", limits)?;
            Some(json)
        }
        None => None,
    };
    Ok((
        display_name,
        description,
        phases,
        input_schema,
        output_schema,
    ))
}

fn finish_resolved(
    name: WorkflowName,
    mut typed: CanonicalWorkflowManifest,
    lua_source: String,
    source_sha256: String,
    source_origin: WorkflowSourceOrigin,
    source_locator: Option<String>,
    limits: &WorkflowLimits,
) -> Result<ResolvedWorkflowDefinition, WorkflowError> {
    let input_schema = typed.input_schema.get_or_insert_with(|| {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
        })
    });
    reject_schema_limit(input_schema, "input_schema", limits)?;
    let canonical_manifest_json = serialize_canonical_manifest(&typed)?;
    reject_manifest_limit(&canonical_manifest_json, limits)?;

    let compiled_output_schema = compile_output_schema(&typed.output_schema)?;
    let compiled_input_schema = match &typed.input_schema {
        Some(schema) => Some(compile_input_schema(schema)?),
        None => None,
    };

    let revision = compute_definition_revision(&canonical_manifest_json, lua_source.as_bytes());

    Ok(ResolvedWorkflowDefinition {
        name,
        display_name: typed.display_name,
        description: typed.description,
        phases: typed.phases,
        input_schema: typed.input_schema,
        output_schema: typed.output_schema,
        compiled_input_schema,
        compiled_output_schema,
        lua_source,
        source_sha256,
        source_origin,
        source_locator,
        revision,
        canonical_manifest_json,
    })
}

fn validate_phases(phases: &[WorkflowPhase]) -> Result<Vec<WorkflowPhase>, WorkflowError> {
    if phases.is_empty() {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidDefinition,
            "phases must contain at least one phase",
        ));
    }
    let mut seen = HashSet::with_capacity(phases.len());
    let mut out = Vec::with_capacity(phases.len());
    for phase in phases {
        let id = phase.id.trim();
        let description = phase.description.trim();
        if id.is_empty() || description.is_empty() {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidDefinition,
                "phase id and description must not be empty",
            ));
        }
        if !seen.insert(id.to_owned()) {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidDefinition,
                format!("duplicate phase id `{id}`"),
            ));
        }
        out.push(WorkflowPhase {
            id: id.to_owned(),
            description: description.to_owned(),
        });
    }
    Ok(out)
}

fn reject_source_limit(source_bytes: &[u8], limits: &WorkflowLimits) -> Result<(), WorkflowError> {
    let len = source_bytes.len() as u64;
    if len > limits.lua_source_bytes {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidDefinition,
            format!(
                "Lua source size {len} exceeds limit {}",
                limits.lua_source_bytes
            ),
        ));
    }
    Ok(())
}

fn reject_manifest_limit(
    manifest_bytes: &[u8],
    limits: &WorkflowLimits,
) -> Result<(), WorkflowError> {
    let len = manifest_bytes.len() as u64;
    if len > limits.manifest_bytes {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidManifest,
            format!(
                "manifest size {len} exceeds limit {}",
                limits.manifest_bytes
            ),
        ));
    }
    Ok(())
}

fn reject_schema_limit(
    schema: &Value,
    label: &str,
    limits: &WorkflowLimits,
) -> Result<(), WorkflowError> {
    // Schemas ride inside the manifest budget for file-backed definitions; the
    // same ceiling applies to dynamic schema documents before compile.
    let encoded = serde_json::to_vec(schema).map_err(|err| {
        WorkflowError::coded(
            WorkflowErrorCode::InvalidSchema,
            format!("{label} serialization failed: {err}"),
        )
    })?;
    let len = encoded.len() as u64;
    if len > limits.manifest_bytes {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidSchema,
            format!("{label} size {len} exceeds limit {}", limits.manifest_bytes),
        ));
    }
    Ok(())
}

fn compile_output_schema(schema: &Value) -> Result<CompiledSchema, WorkflowError> {
    CompiledSchema::compile(schema).map_err(|err| {
        let code = match err.code {
            SchemaErrorCode::InvalidSchema => WorkflowErrorCode::InvalidSchema,
            SchemaErrorCode::SchemaInvalid | SchemaErrorCode::StrictJsonFailed => {
                WorkflowErrorCode::InvalidSchema
            }
        };
        WorkflowError::coded(code, format!("output_schema compile failed: {err}"))
    })
}

fn compile_input_schema(schema: &Value) -> Result<CompiledSchema, WorkflowError> {
    CompiledSchema::compile(schema).map_err(|err| {
        WorkflowError::coded(
            WorkflowErrorCode::InputSchemaInvalid,
            format!("input_schema compile failed: {err}"),
        )
    })
}

fn toml_value_to_json(value: toml::Value) -> Result<Value, WorkflowError> {
    match value {
        toml::Value::String(s) => Ok(Value::String(s)),
        toml::Value::Integer(i) => Ok(Value::Number(i.into())),
        toml::Value::Float(f) => serde_json::Number::from_f64(f).map_or_else(
            || {
                Err(WorkflowError::coded(
                    WorkflowErrorCode::InvalidManifest,
                    "manifest contains non-finite float",
                ))
            },
            |n| Ok(Value::Number(n)),
        ),
        toml::Value::Boolean(b) => Ok(Value::Bool(b)),
        toml::Value::Datetime(dt) => Ok(Value::String(dt.to_string())),
        toml::Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(toml_value_to_json(item)?);
            }
            Ok(Value::Array(out))
        }
        toml::Value::Table(table) => {
            let mut map = serde_json::Map::with_capacity(table.len());
            for (key, item) in table {
                map.insert(key, toml_value_to_json(item)?);
            }
            Ok(Value::Object(map))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn framing_length_prefixes_prevent_field_boundary_collision() {
        let a = build_definition_revision_frame(b"x", b"yz");
        let b = build_definition_revision_frame(b"xy", b"z");
        assert_ne!(a, b);
        assert_ne!(
            compute_definition_revision(b"x", b"yz"),
            compute_definition_revision(b"xy", b"z")
        );
    }

    #[test]
    fn key_reorder_preserves_canonical_manifest_bytes() {
        let m1 = CanonicalWorkflowManifest {
            display_name: "Demo".into(),
            description: "d".into(),
            phases: vec![WorkflowPhase {
                id: "p".into(),
                description: "phase".into(),
            }],
            source_sha256: "a".repeat(64),
            input_schema: Some(json!({"type": "object", "properties": {"b": {}, "a": {}}})),
            output_schema: json!({"type": "object", "properties": {"z": {}, "a": {}}}),
        };
        // Same logical content — serialize path always sorts keys.
        let bytes1 = serialize_canonical_manifest(&m1).expect("ser");
        let parsed: Value = serde_json::from_slice(&bytes1).unwrap();
        let bytes2 = serde_json::to_vec(&canonicalize_json(&parsed)).unwrap();
        assert_eq!(bytes1, bytes2);
        // Nested object keys are sorted in the canonical form.
        let text = String::from_utf8(bytes1).unwrap();
        assert!(text.contains(r#""properties":{"a":{},"b":{}}"#) || text.contains("\"a\":{}"));
        let a_pos = text.find(r#""a":{}"#).expect("a key");
        let b_pos = text.find(r#""b":{}"#).expect("b key");
        assert!(a_pos < b_pos, "UTF-8 byte-sorted keys: a before b");
    }
}
