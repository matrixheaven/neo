//! Ordinary registry built-ins (Task 23 / design §40).

use std::path::PathBuf;

use neo_agent_core::workflow::{
    WorkflowDefinitionRegistry, WorkflowLimits, WorkflowListScope, WorkflowSourceOrigin,
    builtin_manifest_revision_vectors, builtin_workflow_definition, builtin_workflow_definitions,
    check_definition, load_fixture, resolve_builtin_definition, run_builtin_fixture,
    run_fixture_retained, source_sha256_hex,
};
use serde_json::json;
use tempfile::tempdir;

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workflows")
}

fn read_only_tools() -> [&'static str; 5] {
    ["Read", "List", "Grep", "Find", "Glob"]
}

/// All three built-ins resolve and validate through the public registry/check path.
#[test]
fn all_builtin_definitions_validate_through_public_registry() {
    let builtins = builtin_workflow_definitions();
    assert_eq!(
        builtins.len(),
        3,
        "deep-research, code-review, large-refactor"
    );
    let names: Vec<&str> = builtins.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["code-review", "deep-research", "large-refactor"]
    );

    let limits = WorkflowLimits::default();
    let vectors = builtin_manifest_revision_vectors(&builtins, &limits).expect("revision vectors");
    assert_eq!(vectors.len(), 3);

    let dir = tempdir().expect("tempdir");
    let registry = WorkflowDefinitionRegistry::with_builtin_definitions(
        dir.path().join("neo_home"),
        dir.path().join("workspace"),
        false,
        limits.clone(),
    );

    let listed = registry
        .list(WorkflowListScope::Builtin)
        .expect("list builtins");
    assert_eq!(listed.len(), 3);
    for summary in &listed {
        assert_eq!(summary.source_origin, WorkflowSourceOrigin::Builtin);
        assert!(
            summary
                .source_locator
                .as_deref()
                .is_some_and(|l| l.starts_with("builtin://")),
            "locator {:?}",
            summary.source_locator
        );
        let resolved = registry
            .resolve(summary.name.as_str())
            .expect("resolve effective");
        assert_eq!(resolved.source_origin, WorkflowSourceOrigin::Builtin);
        assert_eq!(resolved.revision.as_str(), summary.revision.as_str());

        let report = check_definition(&resolved);
        assert!(
            report.ok,
            "{} check failed: {:?}",
            summary.name.as_str(),
            report.diagnostics
        );
    }

    // Embedded source_sha256 must match exact included Lua bytes.
    for def in &builtins {
        let expected = source_sha256_hex(&def.source_bytes);
        let manifest = std::str::from_utf8(&def.manifest_bytes).expect("utf8");
        assert!(
            manifest.contains(&expected),
            "{} manifest missing source hash {expected}",
            def.name
        );
        // No privileged host surface in source.
        let source = std::str::from_utf8(&def.source_bytes).expect("utf8");
        for banned in [
            "require(", "io.", "os.", "package.", "debug.", "dofile", "loadfile", "jit.",
        ] {
            assert!(
                !source.contains(banned),
                "{} must not reference privileged static name {banned}",
                def.name
            );
        }
    }
}

/// Deep research exercises plan → heterogeneous children → verify → structured report.
#[tokio::test]
async fn deep_research_builtin_fixture() {
    let limits = WorkflowLimits::default();
    let definition = resolve_builtin_definition("deep-research", &limits).expect("resolve");
    assert_eq!(definition.name.as_str(), "deep-research");
    assert_eq!(definition.source_origin, WorkflowSourceOrigin::Builtin);

    let source = &definition.lua_source;
    assert!(source.contains("role = \"explorer\""));
    assert!(source.contains("role = \"reviewer\""));
    assert!(source.contains("role = \"planner\""));
    assert!(source.contains("primary_sources"));
    assert!(source.contains("counterpoints"));
    assert!(source.contains("finding_schema") || source.contains("\"claim\""));

    let fixture = load_fixture(&fixture_root().join("deep_research.json")).expect("fixture");
    let (report, _session, _runtime) = run_fixture_retained(&definition, &fixture, limits)
        .await
        .expect("run");

    assert!(report.ok, "diagnostics: {:?}", report.diagnostics);
    assert_eq!(report.state, "completed");
    let result = report.final_result.expect("final result");
    assert_eq!(result["ok"], json!(true));
    assert_eq!(
        result["question"],
        json!("How does Neo workflow revision hashing work?")
    );
    assert!(result["report"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(result["findings"].as_array().is_some());
    assert!(result["plan"].is_object());
    assert!(
        result["artifacts"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v == "research_plan")),
        "artifacts field: {:?}",
        result["artifacts"]
    );
    assert!(
        report.invocation_kinds.iter().any(|k| k == "delegate"),
        "expected delegate children: {:?}",
        report.invocation_kinds
    );
    assert!(
        report.invocation_kinds.iter().any(|k| k == "report"),
        "expected research plan report: {:?}",
        report.invocation_kinds
    );
}

/// Code review is read-only (tool_allow ceiling) and findings-first in final output.
#[tokio::test]
async fn code_review_builtin_is_read_only_and_findings_first() {
    let limits = WorkflowLimits::default();
    let definition = resolve_builtin_definition("code-review", &limits).expect("resolve");
    let source = &definition.lua_source;

    // Static contract: every review child uses the read-only ceiling.
    for tool in read_only_tools() {
        assert!(
            source.contains(&format!("\"{tool}\"")),
            "missing read-only tool {tool}"
        );
    }
    for banned in ["Write", "Edit", "Bash", "Terminal"] {
        // tool_allow must not grant mutation tools; incidental task prose may mention them.
        // Enforce via the READ_ONLY_TOOLS table and explicit tool_allow assignments.
        assert!(
            !source.contains(&format!("\"{banned}\"")),
            "code-review must not allow mutation tool {banned}"
        );
    }
    assert!(source.contains("READ_ONLY_TOOLS") || source.contains("tool_allow"));
    assert!(source.contains("read_only = true") || source.contains("read_only=true"));

    // Final schema is findings-first: findings is required and listed first.
    let schema = definition.compiled_output_schema.schema();
    let required = schema
        .get("required")
        .and_then(|v| v.as_array())
        .expect("required");
    assert_eq!(
        required.first().and_then(|v| v.as_str()),
        Some("findings"),
        "findings-first final schema, got {required:?}"
    );

    let fixture = load_fixture(&fixture_root().join("code_review.json")).expect("fixture");
    let report = run_builtin_fixture("code-review", &fixture, limits)
        .await
        .expect("run");

    assert!(report.ok, "diagnostics: {:?}", report.diagnostics);
    let result = report.final_result.expect("final");
    // Findings-first object: findings present and non-empty from fixtures.
    let findings = result["findings"].as_array().expect("findings array");
    assert!(!findings.is_empty(), "expected fixture findings");
    assert_eq!(result["ok"], json!(true));
    assert_eq!(result["read_only"], json!(true));
    assert_eq!(result["scope"], json!("crates/neo-agent-core/src/workflow"));
    // Structured finding fields.
    let first = &findings[0];
    for key in ["severity", "path", "line", "evidence", "test_gap"] {
        assert!(first.get(key).is_some(), "missing {key} in {first}");
    }
}

/// Large refactor defaults mutations to isolated worktrees and awaits explicit merge.
#[tokio::test]
async fn large_refactor_builtin_requires_explicit_merge_decision() {
    let limits = WorkflowLimits::default();
    let definition = resolve_builtin_definition("large-refactor", &limits).expect("resolve");
    let source = &definition.lua_source;

    assert!(
        source.contains("worktree = \"isolated\""),
        "mutation slices must default to isolated worktrees"
    );
    assert!(
        source.contains("neo.await_user"),
        "must await explicit human merge/retirement"
    );
    assert!(
        source.contains("merge") && source.contains("retire_worktrees"),
        "answer schema must cover merge and retirement"
    );
    assert!(
        source.contains("auto_merge = false") || source.contains("auto_merge=false"),
        "must never auto-merge"
    );
    assert!(
        !source.contains("auto_merge = true") && !source.contains("auto_merge=true"),
        "must never enable auto-merge"
    );

    let fixture = load_fixture(&fixture_root().join("large_refactor.json")).expect("fixture");
    let (report, _session, _runtime) = run_fixture_retained(&definition, &fixture, limits)
        .await
        .expect("run");

    assert!(report.ok, "diagnostics: {:?}", report.diagnostics);
    assert_eq!(report.state, "completed");
    let result = report.final_result.expect("final");
    assert_eq!(result["ok"], json!(true));
    // Explicit human decision from fixture: merge false, retire false.
    assert_eq!(result["merge"], json!(false));
    assert_eq!(result["retire_worktrees"], json!(false));
    let risks = result["unresolved_risks"]
        .as_array()
        .expect("unresolved_risks");
    assert!(
        risks
            .iter()
            .any(|r| r.as_str().is_some_and(|s| s.contains("merge"))),
        "unresolved risks should mention unapproved merge: {risks:?}"
    );
    assert!(
        risks.iter().any(|r| r
            .as_str()
            .is_some_and(|s| s.contains("worktree") || s.contains("retire"))),
        "unresolved risks should mention worktree retention: {risks:?}"
    );
    let lineage = result["lineage"].as_object().expect("lineage");
    assert_eq!(lineage.get("auto_merge"), Some(&json!(false)));
    assert_eq!(lineage.get("auto_delete_worktrees"), Some(&json!(false)));
    assert_eq!(lineage.get("worktree_policy"), Some(&json!("isolated")));
    assert!(
        report.invocation_kinds.iter().any(|k| k == "delegate"),
        "expected isolated slice delegates: {:?}",
        report.invocation_kinds
    );
}

#[test]
fn builtin_lookup_is_exact_name() {
    assert!(builtin_workflow_definition("deep-research").is_some());
    assert!(builtin_workflow_definition("code-review").is_some());
    assert!(builtin_workflow_definition("large-refactor").is_some());
    assert!(builtin_workflow_definition("Deep-Research").is_none());
    assert!(builtin_workflow_definition("missing").is_none());
}
