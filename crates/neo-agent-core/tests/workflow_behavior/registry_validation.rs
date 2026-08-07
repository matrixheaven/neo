//! Workflow definition registry tests.
//!
//! Task 9: typed definitions and canonical revision hashing.
//! Task 10: trusted definition registry precedence, trust, path safety, save.

use neo_agent_core::workflow::{
    CanonicalWorkflowManifest, DEFINITION_REVISION_PREFIX, DynamicWorkflowDefinitionInput,
    WorkflowErrorCode, WorkflowLimits, WorkflowPhase, WorkflowSourceOrigin,
    build_definition_revision_frame, compute_definition_revision, resolve_dynamic_definition,
    resolve_paired_definition, serialize_canonical_manifest, source_sha256_hex,
};
use serde_json::{Map, Value, json};

pub(crate) fn minimal_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "ok": { "type": "boolean" }
        },
        "required": ["ok"],
        "additionalProperties": false
    })
}

fn minimal_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
    })
}

fn sample_script() -> &'static str {
    "return { ok = true }\n"
}

fn golden_manifest(source_sha256: &str) -> CanonicalWorkflowManifest {
    CanonicalWorkflowManifest {
        display_name: "Golden Demo".to_owned(),
        description: "stable revision fixture".to_owned(),
        phases: vec![WorkflowPhase {
            id: "run".to_owned(),
            description: "execute".to_owned(),
        }],
        source_sha256: source_sha256.to_owned(),
        input_schema: Some(minimal_input_schema()),
        output_schema: minimal_output_schema(),
    }
}

/// Golden vector: fixed definition bytes → fixed revision across platforms.
#[test]
fn definition_revision_golden_vectors_are_stable() {
    let script = sample_script();
    let source_sha = source_sha256_hex(script.as_bytes());

    let manifest = golden_manifest(&source_sha);
    let canonical = serialize_canonical_manifest(&manifest).expect("canonical manifest");
    let canonical_text = std::str::from_utf8(&canonical).expect("utf-8");
    assert!(
        !canonical_text.contains('\n'),
        "canonical JSON must be single-line: {canonical_text}"
    );
    // serde_json::to_vec adds no space after ':' or ',' outside strings.
    assert!(
        !canonical_text.contains(": ") && !canonical_text.contains(", "),
        "canonical JSON must omit insignificant whitespace: {canonical_text}"
    );

    let frame = build_definition_revision_frame(&canonical, script.as_bytes());
    assert_eq!(
        &frame[..DEFINITION_REVISION_PREFIX.len()],
        DEFINITION_REVISION_PREFIX
    );
    let prefix_len = DEFINITION_REVISION_PREFIX.len();
    let manifest_len = u64::from_be_bytes(frame[prefix_len..prefix_len + 8].try_into().unwrap());
    assert_eq!(manifest_len as usize, canonical.len());

    let revision = compute_definition_revision(&canonical, script.as_bytes());
    assert_eq!(revision.as_str().len(), 64);

    // Dynamic and paired adapters share one ResolvedWorkflowDefinition shape
    // and the same content revision for identical logical content.
    let dynamic = resolve_dynamic_definition(
        DynamicWorkflowDefinitionInput {
            name: "golden-demo".to_owned(),
            display_name: Some("Golden Demo".to_owned()),
            description: "stable revision fixture".to_owned(),
            phases: vec![WorkflowPhase {
                id: "run".to_owned(),
                description: "execute".to_owned(),
            }],
            script: script.to_owned(),
            input_schema: None,
            output_schema: minimal_output_schema(),
        },
        &WorkflowLimits::default(),
    )
    .expect("dynamic resolve");
    assert_eq!(dynamic.revision.as_str(), revision.as_str());
    assert_eq!(dynamic.source_origin, WorkflowSourceOrigin::Dynamic);
    assert_eq!(dynamic.source_sha256, source_sha);
    assert_eq!(dynamic.canonical_manifest_json, canonical);
    assert_eq!(dynamic.name.as_str(), "golden-demo");

    let toml = format!(
        r#"
display_name = "Golden Demo"
description = "stable revision fixture"
source_sha256 = "{source_sha}"

[[phases]]
id = "run"
description = "execute"

[output_schema]
type = "object"
additionalProperties = false
required = ["ok"]

[output_schema.properties.ok]
type = "boolean"

[input_schema]
type = "object"
additionalProperties = false
"#
    );
    let paired = resolve_paired_definition(
        "golden-demo",
        toml.as_bytes(),
        script.as_bytes(),
        WorkflowSourceOrigin::Builtin,
        Some("builtin://golden-demo".to_owned()),
        &WorkflowLimits::default(),
    )
    .expect("paired resolve");
    assert_eq!(paired.revision.as_str(), revision.as_str());
    assert_eq!(paired.source_origin, WorkflowSourceOrigin::Builtin);
    assert_eq!(paired.canonical_manifest_json, canonical);
    assert_eq!(
        paired.source_locator.as_deref(),
        Some("builtin://golden-demo")
    );

    // Pin absolute goldens so cross-platform drift fails closed.
    assert_eq!(source_sha, GOLDEN_SOURCE_SHA256, "source sha drift");
    assert_eq!(
        canonical_text, GOLDEN_CANONICAL_MANIFEST_JSON,
        "canonical manifest drift"
    );
    assert_eq!(
        revision.as_str(),
        GOLDEN_DEFINITION_REVISION,
        "definition revision drift"
    );
}

// Pinned after fixture resolve. Update only when the golden fixture changes.
const GOLDEN_SOURCE_SHA256: &str =
    "0467d5837e47b9b59fa85b2914df8bc62206b88545943869b0a659a9b617b821";
const GOLDEN_CANONICAL_MANIFEST_JSON: &str = r#"{"description":"stable revision fixture","display_name":"Golden Demo","input_schema":{"additionalProperties":false,"type":"object"},"output_schema":{"additionalProperties":false,"properties":{"ok":{"type":"boolean"}},"required":["ok"],"type":"object"},"phases":[{"description":"execute","id":"run"}],"source_sha256":"0467d5837e47b9b59fa85b2914df8bc62206b88545943869b0a659a9b617b821"}"#;
const GOLDEN_DEFINITION_REVISION: &str =
    "f70f9fd64ac0649b982b9cf3a92d0d62c4b614d1db35b97d99245daeec4661f6";

/// Object-key reorder preserves revision; length-prefix framing prevents
/// field-boundary collisions from producing the same digest.
#[test]
fn definition_revision_preserves_object_order_rules_and_field_boundaries() {
    let script = sample_script();
    let source_sha = source_sha256_hex(script.as_bytes());

    // Build maps with deliberate opposite insertion order.
    let mut props_z_first = Map::new();
    props_z_first.insert("z".to_owned(), json!({ "type": "string" }));
    props_z_first.insert("a".to_owned(), json!({ "type": "number" }));
    let mut root_z_first = Map::new();
    root_z_first.insert("type".to_owned(), json!("object"));
    root_z_first.insert("properties".to_owned(), Value::Object(props_z_first));
    root_z_first.insert("required".to_owned(), json!(["a", "z"]));
    root_z_first.insert("additionalProperties".to_owned(), json!(false));
    let schema_z_first = Value::Object(root_z_first);

    let mut props_a_first = Map::new();
    props_a_first.insert("a".to_owned(), json!({ "type": "number" }));
    props_a_first.insert("z".to_owned(), json!({ "type": "string" }));
    let mut root_a_first = Map::new();
    root_a_first.insert("additionalProperties".to_owned(), json!(false));
    root_a_first.insert("required".to_owned(), json!(["a", "z"]));
    root_a_first.insert("properties".to_owned(), Value::Object(props_a_first));
    root_a_first.insert("type".to_owned(), json!("object"));
    let schema_a_first = Value::Object(root_a_first);

    // Workspace serde_json uses BTreeMap maps (no preserve_order), so raw
    // serialize already sorts keys. Still build opposite insertion orders and
    // prove the canonical path is order-independent either way.
    let mut manifest_a = golden_manifest(&source_sha);
    manifest_a.output_schema = schema_z_first;
    let mut manifest_b = golden_manifest(&source_sha);
    manifest_b.output_schema = schema_a_first;

    let bytes_a = serialize_canonical_manifest(&manifest_a).expect("ser a");
    let bytes_b = serialize_canonical_manifest(&manifest_b).expect("ser b");
    assert_eq!(
        bytes_a, bytes_b,
        "UTF-8 byte-sorted object keys must yield identical canonical manifest JSON"
    );
    assert_eq!(
        compute_definition_revision(&bytes_a, script.as_bytes()),
        compute_definition_revision(&bytes_b, script.as_bytes())
    );

    // Nested key order inside the canonical bytes is sorted (a before z).
    let text = std::str::from_utf8(&bytes_a).unwrap();
    let a_idx = text.find(r#""a":{"type":"number"}"#).expect("a property");
    let z_idx = text.find(r#""z":{"type":"string"}"#).expect("z property");
    assert!(a_idx < z_idx, "nested properties sorted by UTF-8 key bytes");

    // Field-boundary counterexample: without length prefixes, concat(m||s) can
    // collide; with u64be lengths, revisions differ.
    let rev_boundary_a = compute_definition_revision(b"x", b"yz");
    let rev_boundary_b = compute_definition_revision(b"xy", b"z");
    assert_ne!(
        rev_boundary_a, rev_boundary_b,
        "length-prefixed framing must separate field boundaries"
    );
    assert_ne!(
        build_definition_revision_frame(b"x", b"yz"),
        build_definition_revision_frame(b"xy", b"z")
    );

    // Arrays retain order (phases order is significant).
    let mut phases_swapped = golden_manifest(&source_sha);
    phases_swapped.phases = vec![
        WorkflowPhase {
            id: "b".to_owned(),
            description: "second".to_owned(),
        },
        WorkflowPhase {
            id: "a".to_owned(),
            description: "first".to_owned(),
        },
    ];
    let mut phases_original = golden_manifest(&source_sha);
    phases_original.phases = vec![
        WorkflowPhase {
            id: "a".to_owned(),
            description: "first".to_owned(),
        },
        WorkflowPhase {
            id: "b".to_owned(),
            description: "second".to_owned(),
        },
    ];
    let swapped = serialize_canonical_manifest(&phases_swapped).unwrap();
    let original = serialize_canonical_manifest(&phases_original).unwrap();
    assert_ne!(
        swapped, original,
        "array order (phases) must affect the revision inputs"
    );
    assert_ne!(
        compute_definition_revision(&swapped, script.as_bytes()),
        compute_definition_revision(&original, script.as_bytes())
    );
}

/// Dynamic definitions require a final output_schema; unknown fields rejected.
#[test]
fn dynamic_definition_requires_final_output_schema() {
    // Missing output_schema fails serde decode (field required on the adapter).
    let missing = json!({
        "name": "needs-schema",
        "description": "missing output schema",
        "phases": [{ "id": "p", "description": "p" }],
        "script": "return {}"
    });
    let err = serde_json::from_value::<DynamicWorkflowDefinitionInput>(missing)
        .expect_err("output_schema is required");
    assert!(
        err.to_string().contains("output_schema"),
        "error should name missing field: {err}"
    );

    // Unknown fields rejected at the dynamic adapter boundary.
    let unknown = json!({
        "name": "x",
        "description": "d",
        "phases": [{ "id": "p", "description": "p" }],
        "script": "return {}",
        "output_schema": { "type": "object" },
        "token_budget": 1
    });
    assert!(
        serde_json::from_value::<DynamicWorkflowDefinitionInput>(unknown).is_err(),
        "unknown fields must be rejected"
    );

    // Empty phases rejected after decode.
    let no_phases = DynamicWorkflowDefinitionInput {
        name: "ok-name".to_owned(),
        display_name: None,
        description: "d".to_owned(),
        phases: vec![],
        script: "return 1".to_owned(),
        input_schema: None,
        output_schema: minimal_output_schema(),
    };
    let err = resolve_dynamic_definition(no_phases, &WorkflowLimits::default())
        .expect_err("empty phases");
    assert_eq!(err.code(), WorkflowErrorCode::InvalidDefinition);

    // Invalid schema document fails compile.
    let bad_schema = DynamicWorkflowDefinitionInput {
        name: "bad-schema".to_owned(),
        display_name: None,
        description: "invalid schema doc".to_owned(),
        phases: vec![WorkflowPhase {
            id: "p".to_owned(),
            description: "p".to_owned(),
        }],
        script: "return { ok = true }".to_owned(),
        input_schema: None,
        // jsonschema Draft 2020-12 rejects non-string/array `type` values that
        // are numbers; also reject remote $ref via our DenyRemoteRetriever —
        // use an unresolvable absolute $ref so build fails closed.
        output_schema: json!({
            "$ref": "https://example.invalid/schemas/never-fetched.json"
        }),
    };
    let err = resolve_dynamic_definition(bad_schema, &WorkflowLimits::default())
        .expect_err("remote ref schema must fail compile");
    assert_eq!(err.code(), WorkflowErrorCode::InvalidSchema);

    // Valid dynamic with output_schema succeeds and compiles.
    let ok = resolve_dynamic_definition(
        DynamicWorkflowDefinitionInput {
            name: "with-schema".to_owned(),
            display_name: None,
            description: "has schema".to_owned(),
            phases: vec![WorkflowPhase {
                id: "p".to_owned(),
                description: "p".to_owned(),
            }],
            script: "return { ok = true }".to_owned(),
            input_schema: None,
            output_schema: minimal_output_schema(),
        },
        &WorkflowLimits::default(),
    )
    .expect("valid dynamic definition");
    assert!(ok.compiled_output_schema.schema().is_object());
    assert_eq!(ok.display_name, "with-schema");
}

#[test]
fn omitted_input_schema_is_strictly_no_arguments() {
    let definition = resolve_dynamic_definition(
        DynamicWorkflowDefinitionInput {
            name: "no-arguments".to_owned(),
            display_name: None,
            description: "accepts no arguments".to_owned(),
            phases: vec![WorkflowPhase {
                id: "run".to_owned(),
                description: "run".to_owned(),
            }],
            script: sample_script().to_owned(),
            input_schema: None,
            output_schema: minimal_output_schema(),
        },
        &WorkflowLimits::default(),
    )
    .expect("dynamic definition");

    let expected = json!({
        "type": "object",
        "additionalProperties": false,
    });
    assert_eq!(definition.input_schema.as_ref(), Some(&expected));
    let input_schema = definition
        .compiled_input_schema
        .as_ref()
        .expect("normalized input schema compiles");
    assert!(input_schema.validate_instance(&json!({})).is_ok());
    assert!(
        input_schema
            .validate_instance(&json!({"unexpected": true}))
            .is_err()
    );
}

#[test]
fn paired_definition_without_input_schema_is_strictly_no_arguments() {
    let script = sample_script();
    let manifest = format!(
        r#"
display_name = "No Arguments"
description = "accepts no arguments"
source_sha256 = "{}"

[[phases]]
id = "run"
description = "run"

[output_schema]
type = "object"
additionalProperties = false
"#,
        source_sha256_hex(script.as_bytes())
    );

    let definition = resolve_paired_definition(
        "no-arguments",
        manifest.as_bytes(),
        script.as_bytes(),
        WorkflowSourceOrigin::User,
        None,
        &WorkflowLimits::default(),
    )
    .expect("paired definition");

    let input_schema = definition
        .compiled_input_schema
        .as_ref()
        .expect("normalized input schema compiles");
    assert!(input_schema.validate_instance(&json!({})).is_ok());
    assert!(
        input_schema
            .validate_instance(&json!({"unexpected": true}))
            .is_err()
    );
}

// ---------------------------------------------------------------------------
// Task 10 helpers
// ---------------------------------------------------------------------------
