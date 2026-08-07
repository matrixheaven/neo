//! Pure workflow definition check (Task 22 / design §39.1).

use std::fs;
use std::path::PathBuf;

use neo_agent_core::workflow::{
    BuiltinWorkflowDefinition, WorkflowDefinitionRegistry, WorkflowDefinitionRegistryConfig,
    WorkflowLimits, WorkflowListScope, WorkflowSourceOrigin, builtin_manifest_revision_vectors,
    check_definition, check_paired_bytes, check_registry_name, compute_definition_revision,
    resolve_paired_definition, source_sha256_hex,
};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workflows")
}

fn paired_toml(display: &str, description: &str, source_sha: &str, phases: &str) -> String {
    format!(
        r#"
display_name = "{display}"
description = "{description}"
source_sha256 = "{source_sha}"

{phases}

[output_schema]
type = "object"
additionalProperties = false
required = ["ok"]

[output_schema.properties.ok]
type = "boolean"
"#
    )
}

fn phase_block() -> &'static str {
    r#"
[[phases]]
id = "run"
description = "execute"
"#
}

/// Invalid definitions fail closed and never create a run directory.
#[test]
fn workflow_check_rejects_invalid_definition_without_creating_run() {
    let dir = tempfile::tempdir().expect("tempdir");

    // Invalid Lua syntax fails check after successful resolve of schemas/limits.
    let script = "this is not valid lua !!!";
    let source_sha = source_sha256_hex(script.as_bytes());
    let toml = paired_toml("Bad", "invalid lua", &source_sha, phase_block());

    let report = check_paired_bytes(
        "bad-demo",
        toml.as_bytes(),
        script.as_bytes(),
        WorkflowSourceOrigin::User,
        Some(
            dir.path()
                .join("bad-demo.workflow.toml")
                .display()
                .to_string(),
        ),
        &WorkflowLimits::default(),
    );
    assert!(!report.ok, "invalid lua must fail check: {report:?}");
    assert!(
        report.diagnostics.iter().any(|d| {
            d.code == "lua_compile_failed" || d.message.to_lowercase().contains("lua compile")
        }),
        "expected lua compile diagnostic: {:?}",
        report.diagnostics
    );

    // Duplicate phase ids rejected at resolve (load_failed path).
    let script_ok = "return { ok = true }\n";
    let source_sha = source_sha256_hex(script_ok.as_bytes());
    let dup_phases = r#"
[[phases]]
id = "run"
description = "first"
[[phases]]
id = "run"
description = "dup"
"#;
    let toml = paired_toml("Dup", "duplicate phases", &source_sha, dup_phases);
    let report = check_paired_bytes(
        "dup-demo",
        toml.as_bytes(),
        script_ok.as_bytes(),
        WorkflowSourceOrigin::User,
        None,
        &WorkflowLimits::default(),
    );
    assert!(!report.ok, "duplicate phases must fail: {report:?}");

    // Forbidden static name is advisory only — definition otherwise valid stays ok.
    let script_adv = "local x = require(\"nope\")\nreturn { ok = true }\n";
    let source_sha = source_sha256_hex(script_adv.as_bytes());
    let toml = paired_toml("Adv", "advisory require", &source_sha, phase_block());
    let report = check_paired_bytes(
        "adv-demo",
        toml.as_bytes(),
        script_adv.as_bytes(),
        WorkflowSourceOrigin::User,
        None,
        &WorkflowLimits::default(),
    );
    assert!(
        report.ok,
        "advisory forbidden names must not fail closed: {report:?}"
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == "forbidden_static_name"),
        "expected advisory diagnostic: {:?}",
        report.diagnostics
    );

    // No workflow run dirs created under the temp root.
    let mut stack = vec![dir.path().to_path_buf()];
    while let Some(path) = stack.pop() {
        if !path.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&path).unwrap_or_else(|_| fs::read_dir(dir.path()).unwrap()) {
            let entry = entry.unwrap();
            let p = entry.path();
            if p.is_dir() {
                if p.file_name().and_then(|n| n.to_str()) == Some("workflows") {
                    assert!(
                        fs::read_dir(&p).unwrap().next().is_none(),
                        "check must not create workflow runs: {}",
                        p.display()
                    );
                }
                stack.push(p);
            }
        }
    }

    // Registry path also pure.
    let neo_home = dir.path().join("neo_home");
    let workspace = dir.path().join("workspace");
    fs::create_dir_all(neo_home.join("workflows")).unwrap();
    fs::create_dir_all(&workspace).unwrap();
    let registry = WorkflowDefinitionRegistry::new(WorkflowDefinitionRegistryConfig {
        neo_home,
        workspace,
        project_trusted: false,
        limits: WorkflowLimits::default(),
        builtins: Vec::new(),
    });
    let missing = check_registry_name(&registry, "does-not-exist");
    assert!(!missing.ok);
    assert!(!missing.diagnostics.is_empty());

    assert!(
        fixture_root().join("pure_local.json").is_file(),
        "fixture pure_local.json must exist"
    );
}

/// Builtin paired manifests produce stable content-revision vectors via check.
#[test]
fn builtin_manifest_revision_vectors_are_stable() {
    let script = "return { ok = true }\n";
    let source_sha = source_sha256_hex(script.as_bytes());
    let toml = paired_toml(
        "Golden Builtin",
        "stable revision fixture",
        &source_sha,
        phase_block(),
    );
    let builtins = vec![BuiltinWorkflowDefinition {
        name: "golden-builtin".to_owned(),
        manifest_bytes: toml.into_bytes(),
        source_bytes: script.as_bytes().to_vec(),
    }];

    let limits = WorkflowLimits::default();
    let vectors = builtin_manifest_revision_vectors(&builtins, &limits).expect("vectors");
    assert_eq!(vectors.len(), 1);
    assert_eq!(vectors[0].0, "golden-builtin");

    let resolved = resolve_paired_definition(
        "golden-builtin",
        &builtins[0].manifest_bytes,
        &builtins[0].source_bytes,
        WorkflowSourceOrigin::Builtin,
        Some("builtin://golden-builtin".to_owned()),
        &limits,
    )
    .expect("resolve");
    let recomputed = compute_definition_revision(
        &resolved.canonical_manifest_json,
        resolved.lua_source.as_bytes(),
    );
    assert_eq!(vectors[0].1.as_str(), recomputed.as_str());
    assert_eq!(vectors[0].1.as_str(), resolved.revision.as_str());
    assert_eq!(vectors[0].1.as_str().len(), 64);

    let report = check_definition(&resolved);
    assert!(report.ok, "{report:?}");
    assert_eq!(report.revision.as_deref(), Some(recomputed.as_str()));
    assert_eq!(report.source_origin.as_deref(), Some("builtin"));

    // Same bytes → same revision across a second resolve (stable vector).
    let again = builtin_manifest_revision_vectors(&builtins, &limits).expect("again");
    assert_eq!(again, vectors);

    // Registry list of builtins is consistent with check.
    let dir = tempfile::tempdir().unwrap();
    let registry = WorkflowDefinitionRegistry::new(WorkflowDefinitionRegistryConfig {
        neo_home: dir.path().join("home"),
        workspace: dir.path().join("ws"),
        project_trusted: false,
        limits: limits.clone(),
        builtins: builtins.clone(),
    });
    let listed = registry
        .list(WorkflowListScope::Builtin)
        .expect("list builtins");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].revision.as_str(), vectors[0].1.as_str());
}
