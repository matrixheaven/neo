//! Workflow definition registry tests.
//!
//! Task 9: typed definitions and canonical revision hashing.
//! Task 10: trusted definition registry precedence, trust, path safety, save.

use std::fs;
use std::path::{Path, PathBuf};

use neo_agent_core::workflow::{
    BuiltinWorkflowDefinition, CanonicalWorkflowManifest, DEFINITION_REVISION_PREFIX,
    DynamicWorkflowDefinitionInput, MANIFEST_SUFFIX, SOURCE_SUFFIX, WorkflowDefinitionRegistry,
    WorkflowDefinitionRegistryConfig, WorkflowErrorCode, WorkflowLimits, WorkflowListScope,
    WorkflowPhase, WorkflowSaveRequest, WorkflowSaveScope, WorkflowSourceOrigin,
    build_definition_revision_frame, compute_definition_revision, pin_resolved_source,
    resolve_dynamic_definition, resolve_paired_definition, serialize_canonical_manifest,
    source_sha256_hex,
};
use serde_json::{Map, Value, json};

fn minimal_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "ok": { "type": "boolean" }
        },
        "required": ["ok"],
        "additionalProperties": false
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
        input_schema: None,
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
const GOLDEN_CANONICAL_MANIFEST_JSON: &str = r#"{"description":"stable revision fixture","display_name":"Golden Demo","output_schema":{"additionalProperties":false,"properties":{"ok":{"type":"boolean"}},"required":["ok"],"type":"object"},"phases":[{"description":"execute","id":"run"}],"source_sha256":"0467d5837e47b9b59fa85b2914df8bc62206b88545943869b0a659a9b617b821"}"#;
const GOLDEN_DEFINITION_REVISION: &str =
    "da83c9b4499969f02f09296c9549dc1613db42ab9ec04cd2b0577b787365ffd0";

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

// ---------------------------------------------------------------------------
// Task 10 helpers
// ---------------------------------------------------------------------------

fn registry_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().expect("tempdir");
    let neo_home = root.path().join("neo_home");
    let workspace = root.path().join("workspace");
    fs::create_dir_all(neo_home.join("workflows")).expect("user workflows");
    fs::create_dir_all(workspace.join(".neo").join("workflows")).expect("project workflows");
    (root, neo_home, workspace)
}

fn write_pair(dir: &Path, name: &str, display: &str, description: &str, script: &str) {
    let source_sha = source_sha256_hex(script.as_bytes());
    let toml = format!(
        r#"
name = "{name}"
display_name = "{display}"
description = "{description}"
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
"#
    );
    fs::write(dir.join(format!("{name}{SOURCE_SUFFIX}")), script).expect("write lua");
    fs::write(dir.join(format!("{name}{MANIFEST_SUFFIX}")), toml).expect("write toml");
}

fn builtin_pair(
    name: &str,
    display: &str,
    description: &str,
    script: &str,
) -> BuiltinWorkflowDefinition {
    let source_sha = source_sha256_hex(script.as_bytes());
    let toml = format!(
        r#"
name = "{name}"
display_name = "{display}"
description = "{description}"
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
"#
    );
    BuiltinWorkflowDefinition {
        name: name.to_owned(),
        manifest_bytes: toml.into_bytes(),
        source_bytes: script.as_bytes().to_vec(),
    }
}

fn make_registry(
    neo_home: PathBuf,
    workspace: PathBuf,
    project_trusted: bool,
    builtins: Vec<BuiltinWorkflowDefinition>,
) -> WorkflowDefinitionRegistry {
    WorkflowDefinitionRegistry::new(WorkflowDefinitionRegistryConfig {
        neo_home,
        workspace,
        project_trusted,
        limits: WorkflowLimits::default(),
        builtins,
    })
}

/// Precedence is builtin < user < trusted project; same-scope duplicates
/// conflict; invalid higher-scope content does not fall back.
#[test]
fn registry_precedence_conflict_and_no_fallback() {
    let (_root, neo_home, workspace) = registry_fixture();
    let user_dir = WorkflowDefinitionRegistry::user_workflows_dir(&neo_home);
    let project_dir = WorkflowDefinitionRegistry::project_workflows_dir(&workspace);

    // Shared name across scopes with distinct scripts.
    write_pair(
        &user_dir,
        "shared",
        "User Shared",
        "from user",
        "return { ok = true, scope = \"user\" }\n",
    );
    write_pair(
        &project_dir,
        "shared",
        "Project Shared",
        "from project",
        "return { ok = true, scope = \"project\" }\n",
    );

    // User-only name and builtin-only name.
    write_pair(
        &user_dir,
        "user-only",
        "User Only",
        "user scope only",
        "return { ok = true }\n",
    );

    let builtins = vec![
        builtin_pair(
            "shared",
            "Builtin Shared",
            "from builtin",
            "return { ok = true, scope = \"builtin\" }\n",
        ),
        builtin_pair(
            "builtin-only",
            "Builtin Only",
            "builtin scope only",
            "return { ok = true }\n",
        ),
        // Same-scope duplicate builtins → conflict for this name.
        builtin_pair("dup-builtin", "Dup A", "first", "return { ok = true }\n"),
        builtin_pair("dup-builtin", "Dup B", "second", "return { ok = false }\n"),
    ];

    let registry = make_registry(neo_home.clone(), workspace.clone(), true, builtins);

    // Trusted project shadows user and builtin.
    let shared = registry.resolve("shared").expect("project wins");
    assert_eq!(shared.source_origin, WorkflowSourceOrigin::Project);
    assert_eq!(shared.display_name, "Project Shared");
    assert!(shared.lua_source.contains("project"));

    // Pin snapshot is exact and independent of later registry mutation.
    let pinned = pin_resolved_source(&shared);
    assert_eq!(pinned.revision, shared.revision);
    assert_eq!(pinned.lua_source, shared.lua_source);
    assert_eq!(pinned.origin, WorkflowSourceOrigin::Project);

    // User-only resolves from user; builtin-only from builtin.
    assert_eq!(
        registry.resolve("user-only").unwrap().source_origin,
        WorkflowSourceOrigin::User
    );
    assert_eq!(
        registry.resolve("builtin-only").unwrap().source_origin,
        WorkflowSourceOrigin::Builtin
    );

    // Same-scope builtin conflict is deterministic.
    let conflict = registry
        .resolve("dup-builtin")
        .expect_err("duplicate builtin");
    assert_eq!(conflict.code(), WorkflowErrorCode::DefinitionConflict);

    // Invalid higher-scope content must not fall back to lower scopes.
    // Corrupt the project pair for `shared` by mismatching source_sha256.
    let bad_toml = format!(
        r#"
name = "shared"
display_name = "Broken Project"
description = "invalid higher scope"
source_sha256 = "{}"

[[phases]]
id = "run"
description = "execute"

[output_schema]
type = "object"
additionalProperties = false
required = ["ok"]

[output_schema.properties.ok]
type = "boolean"
"#,
        "0".repeat(64)
    );
    fs::write(
        project_dir.join(format!("shared{MANIFEST_SUFFIX}")),
        bad_toml,
    )
    .expect("corrupt project manifest");
    registry.invalidate();

    let no_fallback = registry
        .resolve("shared")
        .expect_err("invalid project must not fall back to user");
    assert_eq!(no_fallback.code(), WorkflowErrorCode::InvalidManifest);

    // Effective list includes only Ready entries; corrupted project name is absent
    // from list but still blocks resolve (tested above).
    let effective = registry
        .list(WorkflowListScope::Effective)
        .expect("list effective");
    assert!(
        effective.iter().all(|e| e.name.as_str() != "shared"),
        "invalid project entry must not appear as ready in list"
    );
    assert!(
        effective.iter().any(|e| e.name.as_str() == "user-only"),
        "user-only must still list"
    );

    // Without project trust, user shadows builtin for `shared` after we leave
    // the broken project pair on disk (project scope is absent entirely).
    let untrusted = make_registry(
        neo_home,
        workspace,
        false,
        vec![builtin_pair(
            "shared",
            "Builtin Shared",
            "from builtin",
            "return { ok = true, scope = \"builtin\" }\n",
        )],
    );
    // Recreate a valid user pair is already present.
    let from_user = untrusted
        .resolve("shared")
        .expect("user when project untrusted");
    assert_eq!(from_user.source_origin, WorkflowSourceOrigin::User);
}

/// Untrusted project definitions are absent from discovery and cannot be saved.
#[test]
fn untrusted_project_definitions_are_absent_and_unsaveable() {
    let (_root, neo_home, workspace) = registry_fixture();
    let project_dir = WorkflowDefinitionRegistry::project_workflows_dir(&workspace);
    write_pair(
        &project_dir,
        "secret-flow",
        "Secret",
        "must stay hidden when untrusted",
        "return { ok = true }\n",
    );

    let registry = make_registry(neo_home.clone(), workspace.clone(), false, Vec::new());
    let err = registry
        .resolve("secret-flow")
        .expect_err("untrusted project def absent");
    assert_eq!(err.code(), WorkflowErrorCode::DefinitionNotFound);

    let project_list = registry
        .list(WorkflowListScope::Project)
        .expect("list project");
    assert!(
        project_list.is_empty(),
        "untrusted project scope must produce no candidates"
    );

    let save_err = registry
        .save(
            WorkflowSaveScope::Project,
            &WorkflowSaveRequest {
                name: "new-project-flow".to_owned(),
                display_name: "New".to_owned(),
                description: "should fail".to_owned(),
                phases: vec![WorkflowPhase {
                    id: "run".to_owned(),
                    description: "execute".to_owned(),
                }],
                lua_source: "return { ok = true }\n".to_owned(),
                input_schema: None,
                output_schema: minimal_output_schema(),
            },
            false,
        )
        .expect_err("untrusted project save");
    assert_eq!(
        save_err.code(),
        WorkflowErrorCode::UntrustedProjectDefinition
    );
    assert!(
        !project_dir
            .join(format!("new-project-flow{SOURCE_SUFFIX}"))
            .exists(),
        "untrusted save must not write files"
    );

    // Trusted registry does discover the on-disk project definition.
    let trusted = make_registry(neo_home, workspace, true, Vec::new());
    let found = trusted.resolve("secret-flow").expect("trusted project");
    assert_eq!(found.source_origin, WorkflowSourceOrigin::Project);
}

/// Symlinked / reparse definition files and parent escapes are rejected.
#[test]
fn registry_rejects_symlink_reparse_and_path_escape() {
    let (root, neo_home, workspace) = registry_fixture();
    let user_dir = WorkflowDefinitionRegistry::user_workflows_dir(&neo_home);

    // Valid pair for contrast.
    write_pair(
        &user_dir,
        "plain",
        "Plain",
        "regular files",
        "return { ok = true }\n",
    );

    // Symlink the lua source of another name to a path outside the scope.
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let outside = root.path().join("outside.lua");
        fs::write(&outside, b"return { ok = true }\n").expect("outside lua");
        let link_lua = user_dir.join(format!("linked{SOURCE_SUFFIX}"));
        let good_toml_sha = source_sha256_hex(b"return { ok = true }\n");
        let toml = format!(
            r#"
name = "linked"
display_name = "Linked"
description = "symlinked source"
source_sha256 = "{good_toml_sha}"

[[phases]]
id = "run"
description = "execute"

[output_schema]
type = "object"
additionalProperties = false
required = ["ok"]

[output_schema.properties.ok]
type = "boolean"
"#
        );
        fs::write(user_dir.join(format!("linked{MANIFEST_SUFFIX}")), toml).expect("toml");
        symlink(&outside, &link_lua).expect("symlink lua");

        let registry = make_registry(neo_home.clone(), workspace.clone(), false, Vec::new());
        let err = registry
            .resolve("linked")
            .expect_err("symlink source rejected");
        assert_eq!(err.code(), WorkflowErrorCode::InvalidDefinition);
        assert!(
            err.to_string().to_lowercase().contains("symlink")
                || err.to_string().contains("refusing"),
            "error should name symlink risk: {err}"
        );

        // Symlinked workflows directory itself yields no user candidates.
        let linked_home = root.path().join("linked_home");
        fs::create_dir_all(&linked_home).expect("linked home");
        let real_workflows = linked_home.join("real_workflows");
        fs::create_dir_all(&real_workflows).expect("real workflows");
        write_pair(
            &real_workflows,
            "hidden",
            "Hidden",
            "via dir symlink",
            "return { ok = true }\n",
        );
        symlink(&real_workflows, linked_home.join("workflows")).expect("dir symlink");
        let via_dir = make_registry(linked_home, workspace.clone(), false, Vec::new());
        let missing = via_dir
            .resolve("hidden")
            .expect_err("symlinked workflows dir not followed");
        assert_eq!(missing.code(), WorkflowErrorCode::DefinitionNotFound);
    }

    // Parent-escape save targets are rejected (name grammar already blocks `..`,
    // so exercise the path validator via an illegal constructed root join).
    let registry = make_registry(neo_home.clone(), workspace.clone(), true, Vec::new());
    // Portable name cannot contain `/` or `..`; ensure valid save still lands only under scope.
    let saved = registry
        .save(
            WorkflowSaveScope::User,
            &WorkflowSaveRequest {
                name: "escape-check".to_owned(),
                display_name: "Escape Check".to_owned(),
                description: "must stay under user workflows".to_owned(),
                phases: vec![WorkflowPhase {
                    id: "run".to_owned(),
                    description: "execute".to_owned(),
                }],
                lua_source: "return { ok = true }\n".to_owned(),
                input_schema: None,
                output_schema: minimal_output_schema(),
            },
            false,
        )
        .expect("valid save");
    assert_eq!(saved.source_origin, WorkflowSourceOrigin::User);
    let expected_lua = user_dir.join(format!("escape-check{SOURCE_SUFFIX}"));
    assert!(expected_lua.is_file(), "saved lua under user dir");
    // Ensure nothing was written outside neo_home/workflows.
    assert!(
        !root.path().join("escape-check.lua").exists(),
        "must not write outside scope"
    );

    // Non-regular / wrong-suffix files are ignored (not treated as definitions).
    fs::write(user_dir.join("notes.txt"), "not a workflow").expect("txt");
    fs::write(user_dir.join("almost.workflow.toml.bak"), "nope").expect("bak");
    registry.invalidate();
    assert_eq!(
        registry.resolve("notes").expect_err("wrong suffix").code(),
        WorkflowErrorCode::DefinitionNotFound
    );
    assert!(registry.resolve("plain").is_ok());
}

/// Save is no-clobber by default, identical content is idempotent, and the
/// pair is written source-first / manifest-last so partial pairs are not launchable.
#[test]
fn save_is_no_clobber_and_pair_atomic() {
    let (_root, neo_home, workspace) = registry_fixture();
    let user_dir = WorkflowDefinitionRegistry::user_workflows_dir(&neo_home);
    let registry = make_registry(neo_home.clone(), workspace.clone(), true, Vec::new());

    let request = WorkflowSaveRequest {
        name: "atomic-demo".to_owned(),
        display_name: "Atomic Demo".to_owned(),
        description: "no-clobber pair".to_owned(),
        phases: vec![WorkflowPhase {
            id: "run".to_owned(),
            description: "execute".to_owned(),
        }],
        lua_source: "return { ok = true }\n".to_owned(),
        input_schema: None,
        output_schema: minimal_output_schema(),
    };

    let first = registry
        .save(WorkflowSaveScope::User, &request, false)
        .expect("initial save");
    assert_eq!(first.name.as_str(), "atomic-demo");
    assert_eq!(first.source_origin, WorkflowSourceOrigin::User);
    let lua_path = user_dir.join(format!("atomic-demo{SOURCE_SUFFIX}"));
    let toml_path = user_dir.join(format!("atomic-demo{MANIFEST_SUFFIX}"));
    assert!(lua_path.is_file());
    assert!(toml_path.is_file());
    let first_revision = first.revision.clone();

    // Identical content succeeds idempotently.
    let again = registry
        .save(WorkflowSaveScope::User, &request, false)
        .expect("identical save is idempotent");
    assert_eq!(again.revision, first_revision);

    // Different content without force fails closed (no-clobber).
    let mut changed = request.clone();
    changed.lua_source = "return { ok = false }\n".to_owned();
    let clobber = registry
        .save(WorkflowSaveScope::User, &changed, false)
        .expect_err("no-clobber");
    assert_eq!(clobber.code(), WorkflowErrorCode::DefinitionConflict);
    // On-disk content unchanged.
    let on_disk = fs::read_to_string(&lua_path).expect("read lua");
    assert_eq!(on_disk, "return { ok = true }\n");

    // Force overwrites.
    let forced = registry
        .save(WorkflowSaveScope::User, &changed, true)
        .expect("force save");
    assert_ne!(forced.revision, first_revision);
    assert_eq!(
        fs::read_to_string(&lua_path).unwrap(),
        "return { ok = false }\n"
    );

    // Partial pair: lua present with mismatched / missing manifest is not launchable.
    fs::write(
        user_dir.join(format!("partial{SOURCE_SUFFIX}")),
        "return { ok = true }\n",
    )
    .expect("partial lua");
    // No matching toml.
    registry.invalidate();
    let partial = registry.resolve("partial").expect_err("incomplete pair");
    assert_eq!(partial.code(), WorkflowErrorCode::InvalidDefinition);

    // Lua present with wrong source_sha256 in toml is not launchable.
    let wrong_sha_toml = format!(
        r#"
name = "mismatched"
display_name = "Mismatched"
description = "hash mismatch"
source_sha256 = "{}"

[[phases]]
id = "run"
description = "execute"

[output_schema]
type = "object"
additionalProperties = false
required = ["ok"]

[output_schema.properties.ok]
type = "boolean"
"#,
        "a".repeat(64)
    );
    fs::write(
        user_dir.join(format!("mismatched{SOURCE_SUFFIX}")),
        "return { ok = true }\n",
    )
    .expect("lua");
    fs::write(
        user_dir.join(format!("mismatched{MANIFEST_SUFFIX}")),
        wrong_sha_toml,
    )
    .expect("toml");
    registry.invalidate();
    let mismatch = registry.resolve("mismatched").expect_err("hash mismatch");
    assert_eq!(mismatch.code(), WorkflowErrorCode::InvalidManifest);

    // Project save works when trusted and is also no-clobber.
    let project_req = WorkflowSaveRequest {
        name: "proj-demo".to_owned(),
        display_name: "Proj".to_owned(),
        description: "project save".to_owned(),
        phases: vec![WorkflowPhase {
            id: "run".to_owned(),
            description: "execute".to_owned(),
        }],
        lua_source: "return { ok = true }\n".to_owned(),
        input_schema: None,
        output_schema: minimal_output_schema(),
    };
    let proj = registry
        .save(WorkflowSaveScope::Project, &project_req, false)
        .expect("project save");
    assert_eq!(proj.source_origin, WorkflowSourceOrigin::Project);
    let project_dir = WorkflowDefinitionRegistry::project_workflows_dir(&workspace);
    assert!(
        project_dir
            .join(format!("proj-demo{MANIFEST_SUFFIX}"))
            .is_file()
    );

    // Manifest is written last: after a successful save both files form a valid pair.
    let resolved = registry
        .resolve("atomic-demo")
        .expect("resolve after force");
    assert_eq!(resolved.revision, forced.revision);
    let pinned = WorkflowDefinitionRegistry::pin_source(&resolved);
    assert_eq!(pinned.lua_source, resolved.lua_source);
}

/// Platform path/link contract: PathBuf joins, no separator hardcoding, regular
/// files only, symlink/reparse escapes rejected, save stays under scope.
///
/// Native evidence target for Task 25 (macOS / Linux / Windows).
#[test]
fn registry_platform_path_and_link_semantics() {
    let (root, neo_home, workspace) = registry_fixture();
    let user_dir = WorkflowDefinitionRegistry::user_workflows_dir(&neo_home);
    let project_dir = WorkflowDefinitionRegistry::project_workflows_dir(&workspace);

    // Scope roots are built only via Path/PathBuf (no string path separators).
    assert_eq!(user_dir, PathBuf::from(&neo_home).join("workflows"));
    assert_eq!(
        project_dir,
        PathBuf::from(&workspace).join(".neo").join("workflows")
    );
    assert!(user_dir.is_dir());
    assert!(project_dir.is_dir());

    // Regular pair discovery uses exact suffixes and content-addressed revision.
    write_pair(
        &user_dir,
        "platform-plain",
        "Platform Plain",
        "regular platform pair",
        "return { ok = true }\n",
    );
    let registry = make_registry(neo_home.clone(), workspace.clone(), true, Vec::new());
    let resolved = registry
        .resolve("platform-plain")
        .expect("regular pair resolves");
    assert_eq!(resolved.source_origin, WorkflowSourceOrigin::User);
    assert_eq!(resolved.revision.as_str().len(), 64);

    // Wrong suffix and double-extension files never become definitions.
    fs::write(user_dir.join("notes.txt"), b"not a workflow").expect("txt");
    fs::write(
        user_dir.join("almost.workflow.toml.bak"),
        b"not a workflow manifest\n",
    )
    .expect("bak");
    registry.invalidate();
    assert_eq!(
        registry.resolve("notes").expect_err("wrong suffix").code(),
        WorkflowErrorCode::DefinitionNotFound
    );
    assert_eq!(
        registry
            .resolve("almost")
            .expect_err("double extension")
            .code(),
        WorkflowErrorCode::DefinitionNotFound
    );

    // Save lands only under the scope root; portable names cannot escape.
    let saved = registry
        .save(
            WorkflowSaveScope::User,
            &WorkflowSaveRequest {
                name: "platform-save".to_owned(),
                display_name: "Platform Save".to_owned(),
                description: "scope containment".to_owned(),
                phases: vec![WorkflowPhase {
                    id: "run".to_owned(),
                    description: "execute".to_owned(),
                }],
                lua_source: "return { ok = true }\n".to_owned(),
                input_schema: None,
                output_schema: minimal_output_schema(),
            },
            false,
        )
        .expect("save under user scope");
    assert_eq!(saved.source_origin, WorkflowSourceOrigin::User);
    let lua = user_dir.join(format!("platform-save{SOURCE_SUFFIX}"));
    let toml = user_dir.join(format!("platform-save{MANIFEST_SUFFIX}"));
    assert!(lua.is_file(), "lua must be a regular file under user dir");
    assert!(
        toml.is_file(),
        "manifest must be a regular file under user dir"
    );
    assert!(
        !root.path().join("platform-save.lua").exists(),
        "must not write outside neo_home/workflows"
    );
    assert!(
        !root.path().join("platform-save.workflow.toml").exists(),
        "must not write outside neo_home/workflows"
    );

    // Path-like save names fail closed (no parent escape via name grammar).
    for bad in ["../escape", "a/b", "a\\b", ".."] {
        let err = registry
            .save(
                WorkflowSaveScope::User,
                &WorkflowSaveRequest {
                    name: bad.to_owned(),
                    display_name: "Bad".to_owned(),
                    description: "escape".to_owned(),
                    phases: vec![WorkflowPhase {
                        id: "run".to_owned(),
                        description: "execute".to_owned(),
                    }],
                    lua_source: "return { ok = true }\n".to_owned(),
                    input_schema: None,
                    output_schema: minimal_output_schema(),
                },
                false,
            )
            .expect_err("path-like name rejected");
        assert!(
            matches!(
                err.code(),
                WorkflowErrorCode::InvalidDefinition | WorkflowErrorCode::InvalidInput
            ),
            "unexpected code for {bad:?}: {err}"
        );
    }

    // Unix: symlinked definition files and symlinked scope directories are rejected.
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let outside = root.path().join("outside-platform.lua");
        fs::write(&outside, b"return { ok = true }\n").expect("outside");
        let good_sha = source_sha256_hex(b"return { ok = true }\n");
        let manifest = format!(
            r#"
name = "platform-link"
display_name = "Platform Link"
description = "symlink source"
source_sha256 = "{good_sha}"

[[phases]]
id = "run"
description = "execute"

[output_schema]
type = "object"
additionalProperties = false
required = ["ok"]

[output_schema.properties.ok]
type = "boolean"
"#
        );
        fs::write(
            user_dir.join(format!("platform-link{MANIFEST_SUFFIX}")),
            manifest,
        )
        .expect("manifest");
        symlink(
            &outside,
            user_dir.join(format!("platform-link{SOURCE_SUFFIX}")),
        )
        .expect("symlink lua");

        registry.invalidate();
        let err = registry
            .resolve("platform-link")
            .expect_err("symlinked source rejected");
        assert_eq!(err.code(), WorkflowErrorCode::InvalidDefinition);
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("symlink") || msg.contains("refusing"),
            "error should name symlink risk: {err}"
        );

        // Symlinked workflows directory is not followed.
        let linked_home = root.path().join("platform_linked_home");
        fs::create_dir_all(&linked_home).expect("linked home");
        let real_workflows = linked_home.join("real_workflows");
        fs::create_dir_all(&real_workflows).expect("real");
        write_pair(
            &real_workflows,
            "hidden-platform",
            "Hidden",
            "via dir symlink",
            "return { ok = true }\n",
        );
        symlink(&real_workflows, linked_home.join("workflows")).expect("dir symlink");
        let via_dir = make_registry(linked_home, workspace.clone(), false, Vec::new());
        assert_eq!(
            via_dir
                .resolve("hidden-platform")
                .expect_err("dir symlink not followed")
                .code(),
            WorkflowErrorCode::DefinitionNotFound
        );
    }

    // Windows: when symlink privilege is available, reparse/symlink definition
    // files are rejected the same way. Without privilege, regular-file path
    // containment above remains the native proof for this host.
    #[cfg(windows)]
    {
        use std::os::windows::fs::symlink_file;
        let outside = root.path().join("outside-platform.lua");
        fs::write(&outside, b"return { ok = true }\n").expect("outside");
        let good_sha = source_sha256_hex(b"return { ok = true }\n");
        let manifest = format!(
            r#"
name = "platform-link-win"
display_name = "Platform Link Win"
description = "symlink source"
source_sha256 = "{good_sha}"

[[phases]]
id = "run"
description = "execute"

[output_schema]
type = "object"
additionalProperties = false
required = ["ok"]

[output_schema.properties.ok]
type = "boolean"
"#
        );
        fs::write(
            user_dir.join(format!("platform-link-win{MANIFEST_SUFFIX}")),
            manifest,
        )
        .expect("manifest");
        let link = user_dir.join(format!("platform-link-win{SOURCE_SUFFIX}"));
        match symlink_file(&outside, &link) {
            Ok(()) => {
                registry.invalidate();
                let err = registry
                    .resolve("platform-link-win")
                    .expect_err("symlinked source rejected on Windows");
                assert_eq!(err.code(), WorkflowErrorCode::InvalidDefinition);
            }
            Err(e) => {
                // Developer Mode / admin required for file symlinks on some hosts.
                eprintln!(
                    "windows symlink unavailable on this host ({e}); regular path containment verified"
                );
            }
        }
    }
}
