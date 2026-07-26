//! Pure workflow definition validation (design §39.1).
//!
//! Routes through the existing definition registry, schema compiler, and Lua
//! compile owners. Never creates runs, mutates the registry, or executes host
//! effects.

use serde::Serialize;
use serde_json::{Value, json};

use super::definition::{
    ResolvedWorkflowDefinition, compute_definition_revision, resolve_paired_definition,
    source_sha256_hex,
};
use super::error::WorkflowError;
use super::launch::compile_lua_source;
use super::limits::WorkflowLimits;
use super::registry::{BuiltinWorkflowDefinition, WorkflowDefinitionRegistry};
use super::schema::CompiledSchema;
use super::state::{WorkflowRevision, WorkflowSourceOrigin};

/// Severity of one check diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckSeverity {
    Error,
    Warning,
    /// Static analysis that is not fail-closed (Lua names may be dynamic).
    Advisory,
}

impl CheckSeverity {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Advisory => "advisory",
        }
    }
}

/// One validation finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckDiagnostic {
    pub severity: CheckSeverity,
    pub code: String,
    pub message: String,
}

/// Pure check report. `ok` is true only when no error-severity diagnostics exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkflowCheckReport {
    pub ok: bool,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_locator: Option<String>,
    pub diagnostics: Vec<CheckDiagnostic>,
}

impl WorkflowCheckReport {
    /// Stable JSON value for CLI / scripting.
    #[must_use]
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| {
            json!({
                "ok": false,
                "name": self.name,
                "diagnostics": [{
                    "severity": "error",
                    "code": "report_serialize_failed",
                    "message": "failed to serialize check report",
                }],
            })
        })
    }

    fn from_load_error(target: &str, error: &WorkflowError) -> Self {
        Self {
            ok: false,
            name: target.to_owned(),
            revision: None,
            source_origin: None,
            source_locator: None,
            diagnostics: vec![CheckDiagnostic {
                severity: CheckSeverity::Error,
                code: error.code().as_str().to_owned(),
                message: error.to_string(),
            }],
        }
    }

    fn finish(mut self) -> Self {
        self.ok = self
            .diagnostics
            .iter()
            .all(|diag| diag.severity != CheckSeverity::Error);
        self
    }
}

/// Validate an already-resolved definition without creating a run.
///
/// Covers: Lua syntax, phase uniqueness (already enforced at resolve), schema
/// recompile, source/manifest limits (already enforced), forbidden static API
/// names (advisory), revision recomputation, and source_sha256 consistency.
#[must_use]
pub fn check_definition(definition: &ResolvedWorkflowDefinition) -> WorkflowCheckReport {
    let mut diagnostics = Vec::new();

    // Lua syntax via the sole compile owner.
    if let Err(error) = compile_lua_source(&definition.lua_source) {
        diagnostics.push(CheckDiagnostic {
            severity: CheckSeverity::Error,
            code: error.code().as_str().to_owned(),
            message: error.to_string(),
        });
    }

    // Phase uniqueness (defensive re-check; resolve already rejects duplicates).
    let mut seen_phases = std::collections::HashSet::new();
    for phase in &definition.phases {
        if !seen_phases.insert(phase.id.as_str()) {
            diagnostics.push(CheckDiagnostic {
                severity: CheckSeverity::Error,
                code: "duplicate_phase_id".to_owned(),
                message: format!("duplicate phase id `{}`", phase.id),
            });
        }
    }
    if definition.phases.is_empty() {
        diagnostics.push(CheckDiagnostic {
            severity: CheckSeverity::Error,
            code: "empty_phases".to_owned(),
            message: "phases must contain at least one phase".to_owned(),
        });
    }

    // Schema recompilation through the host owner.
    if let Err(error) = CompiledSchema::compile(&definition.output_schema) {
        diagnostics.push(CheckDiagnostic {
            severity: CheckSeverity::Error,
            code: "invalid_schema".to_owned(),
            message: format!("output_schema compile failed: {error}"),
        });
    }
    if let Some(input) = &definition.input_schema
        && let Err(error) = CompiledSchema::compile(input)
    {
        diagnostics.push(CheckDiagnostic {
            severity: CheckSeverity::Error,
            code: "input_schema_invalid".to_owned(),
            message: format!("input_schema compile failed: {error}"),
        });
    }

    // Source SHA must match exact Lua bytes.
    let source_sha = source_sha256_hex(definition.lua_source.as_bytes());
    if source_sha != definition.source_sha256 {
        diagnostics.push(CheckDiagnostic {
            severity: CheckSeverity::Error,
            code: "source_sha256_mismatch".to_owned(),
            message: format!(
                "source_sha256 mismatch: stored {}, source hashes to {source_sha}",
                definition.source_sha256
            ),
        });
    }

    // Content revision must recompute exactly.
    let recomputed = compute_definition_revision(
        &definition.canonical_manifest_json,
        definition.lua_source.as_bytes(),
    );
    if recomputed.as_str() != definition.revision.as_str() {
        diagnostics.push(CheckDiagnostic {
            severity: CheckSeverity::Error,
            code: "revision_mismatch".to_owned(),
            message: format!(
                "stored revision {} does not match recomputed {}",
                definition.revision.as_str(),
                recomputed.as_str()
            ),
        });
    }

    // Builtin consistency: origin/locator shape.
    if definition.source_origin == WorkflowSourceOrigin::Builtin {
        match definition.source_locator.as_deref() {
            Some(locator) if locator.starts_with("builtin://") => {}
            Some(locator) => diagnostics.push(CheckDiagnostic {
                severity: CheckSeverity::Error,
                code: "builtin_locator_invalid".to_owned(),
                message: format!(
                    "builtin definition locator must start with builtin://, got {locator:?}"
                ),
            }),
            None => diagnostics.push(CheckDiagnostic {
                severity: CheckSeverity::Warning,
                code: "builtin_locator_missing".to_owned(),
                message: "builtin definition has no source_locator".to_owned(),
            }),
        }
    }

    // Forbidden static API names — advisory only (dynamic names exist at runtime).
    diagnostics.extend(scan_forbidden_static_names(&definition.lua_source));

    WorkflowCheckReport {
        ok: false,
        name: definition.name.as_str().to_owned(),
        revision: Some(definition.revision.as_str().to_owned()),
        source_origin: Some(definition.source_origin.as_str().to_owned()),
        source_locator: definition.source_locator.clone(),
        diagnostics,
    }
    .finish()
}

/// Resolve a paired definition from bytes, then check it. Does not write files or create runs.
#[must_use]
pub fn check_paired_bytes(
    stem_name: &str,
    manifest_bytes: &[u8],
    source_bytes: &[u8],
    origin: WorkflowSourceOrigin,
    source_locator: Option<String>,
    limits: &WorkflowLimits,
) -> WorkflowCheckReport {
    match resolve_paired_definition(
        stem_name,
        manifest_bytes,
        source_bytes,
        origin,
        source_locator,
        limits,
    ) {
        Ok(definition) => check_definition(&definition),
        Err(error) => WorkflowCheckReport::from_load_error(stem_name, &error),
    }
}

/// Resolve by effective registry name and check. Read-only.
#[must_use]
pub fn check_registry_name(
    registry: &WorkflowDefinitionRegistry,
    name: &str,
) -> WorkflowCheckReport {
    match registry.resolve(name) {
        Ok(definition) => check_definition(&definition),
        Err(error) => WorkflowCheckReport::from_load_error(name, &error),
    }
}

/// Stable revision vectors for host-supplied builtins (Task 22 / design §39.1).
///
/// Each builtin is resolved through the same paired path used at runtime and the
/// content revision is recomputed. Returns `(name, revision)` for every ready
/// builtin; invalid builtins surface as check errors.
pub fn builtin_manifest_revision_vectors(
    builtins: &[BuiltinWorkflowDefinition],
    limits: &WorkflowLimits,
) -> Result<Vec<(String, WorkflowRevision)>, WorkflowError> {
    let mut out = Vec::with_capacity(builtins.len());
    for builtin in builtins {
        let definition = resolve_paired_definition(
            &builtin.name,
            &builtin.manifest_bytes,
            &builtin.source_bytes,
            WorkflowSourceOrigin::Builtin,
            Some(format!("builtin://{}", builtin.name.trim())),
            limits,
        )?;
        let report = check_definition(&definition);
        if !report.ok {
            let first = report
                .diagnostics
                .iter()
                .find(|d| d.severity == CheckSeverity::Error)
                .map(|d| d.message.as_str())
                .unwrap_or("builtin check failed");
            return Err(WorkflowError::coded(
                super::error::WorkflowErrorCode::InvalidDefinition,
                format!("builtin `{}`: {first}", builtin.name),
            ));
        }
        out.push((definition.name.as_str().to_owned(), definition.revision));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Substrings that must not appear as static host/sandbox escapes.
///
/// Advisory: runtime enforcement remains authoritative (design §39.1).
const FORBIDDEN_STATIC_NAMES: &[(&str, &str)] = &[
    ("require(", "require"),
    ("dofile(", "dofile"),
    ("loadfile(", "loadfile"),
    ("loadstring(", "loadstring"),
    ("getfenv(", "getfenv"),
    ("setfenv(", "setfenv"),
    ("collectgarbage(", "collectgarbage"),
    ("os.", "os"),
    ("io.", "io"),
    ("debug.", "debug"),
    ("package.", "package"),
    ("jit.", "jit"),
];

fn scan_forbidden_static_names(source: &str) -> Vec<CheckDiagnostic> {
    let mut out = Vec::new();
    for (needle, name) in FORBIDDEN_STATIC_NAMES {
        if source.contains(needle) {
            out.push(CheckDiagnostic {
                severity: CheckSeverity::Advisory,
                code: "forbidden_static_name".to_owned(),
                message: format!(
                    "source contains static reference to `{name}` ({needle}); runtime sandbox remains authoritative"
                ),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_source_scan_is_clean() {
        assert!(scan_forbidden_static_names("return { ok = true }").is_empty());
    }

    #[test]
    fn require_is_advisory() {
        let hits = scan_forbidden_static_names(r#"local x = require("foo")"#);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].severity, CheckSeverity::Advisory);
        assert_eq!(hits[0].code, "forbidden_static_name");
    }

    #[test]
    fn report_json_shape_is_stable() {
        let report = WorkflowCheckReport {
            ok: true,
            name: "demo".to_owned(),
            revision: Some("a".repeat(64)),
            source_origin: Some("user".to_owned()),
            source_locator: None,
            diagnostics: vec![],
        };
        let value = report.to_json();
        assert_eq!(value["ok"], json!(true));
        assert_eq!(value["name"], json!("demo"));
        assert!(value["diagnostics"].as_array().unwrap().is_empty());
    }
}
