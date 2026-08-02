//! Deterministic workflow fixture harness (Task 22 / design §39.2).

use std::path::PathBuf;

use neo_agent_core::workflow::journal::{JournalPayload, collect_journal};
use neo_agent_core::workflow::{
    FixtureExecutionMode, WorkflowLimits, WorkflowSourceOrigin, WorkflowState, load_fixture,
    parse_fixture, resolve_paired_definition, run_fixture, run_fixture_retained, source_sha256_hex,
};
use serde_json::json;

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workflows")
}

fn resolve_script(
    name: &str,
    display: &str,
    description: &str,
    script: &str,
) -> neo_agent_core::workflow::ResolvedWorkflowDefinition {
    let source_sha = source_sha256_hex(script.as_bytes());
    let toml = format!(
        r#"
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
    resolve_paired_definition(
        name,
        toml.as_bytes(),
        script.as_bytes(),
        WorkflowSourceOrigin::User,
        Some(format!("fixture://{name}")),
        &WorkflowLimits::default(),
    )
    .unwrap_or_else(|err| panic!("resolve {name}: {err}"))
}

fn resolve_script_with_schema(
    name: &str,
    display: &str,
    description: &str,
    script: &str,
    output_schema_toml: &str,
) -> neo_agent_core::workflow::ResolvedWorkflowDefinition {
    let source_sha = source_sha256_hex(script.as_bytes());
    let toml = format!(
        r#"
display_name = "{display}"
description = "{description}"
source_sha256 = "{source_sha}"

[[phases]]
id = "run"
description = "execute"

{output_schema_toml}
"#
    );
    resolve_paired_definition(
        name,
        toml.as_bytes(),
        script.as_bytes(),
        WorkflowSourceOrigin::User,
        Some(format!("fixture://{name}")),
        &WorkflowLimits::default(),
    )
    .unwrap_or_else(|err| panic!("resolve {name}: {err}"))
}

/// Pure local Lua + real journal; no model/shell/MCP effects.
#[tokio::test]
async fn deterministic_fixture_runs_real_lua_and_journal_without_external_effects() {
    let script = r#"
neo.phase("run")
neo.log("hello")
neo.report({ step = "done" })
return { ok = true, label = neo.args.label }
"#;
    let definition = resolve_script_with_schema(
        "pure-local",
        "Pure Local",
        "no external effects",
        script,
        r#"
[output_schema]
type = "object"
additionalProperties = false
required = ["ok", "label"]

[output_schema.properties.ok]
type = "boolean"

[output_schema.properties.label]
type = "string"
"#,
    );

    let fixture = load_fixture(&fixture_root().join("pure_local.json")).expect("load fixture");
    let report = run_fixture(&definition, &fixture, WorkflowLimits::default())
        .await
        .expect("run fixture");

    assert!(report.ok, "fixture diagnostics: {:?}", report.diagnostics);
    assert_eq!(
        report.final_result,
        Some(json!({"ok": true, "label": "fixture"}))
    );
    assert!(
        report.invocation_kinds.iter().any(|k| k == "phase"),
        "{:?}",
        report.invocation_kinds
    );
    assert!(
        report.invocation_kinds.iter().any(|k| k == "log"),
        "{:?}",
        report.invocation_kinds
    );
    assert!(
        report.invocation_kinds.iter().any(|k| k == "report"),
        "{:?}",
        report.invocation_kinds
    );
    assert_eq!(report.state, WorkflowState::Completed.as_str());
}

/// Child schema repair: exactly one non-executing corrective model turn.
#[tokio::test]
async fn deterministic_fixture_records_one_non_executing_child_schema_repair() {
    let script = r#"
local outcome = neo.delegate({
  task = "return structured ok",
  output_schema = {
    type = "object",
    properties = { ok = { type = "boolean" } },
    required = { "ok" },
    additionalProperties = false,
  },
})
neo.verify(outcome.ok, "delegate must succeed after one repair")
return { ok = true }
"#;
    let definition = resolve_script(
        "schema-repair",
        "Schema Repair",
        "one non-executing repair",
        script,
    );
    let fixture =
        load_fixture(&fixture_root().join("child_schema_repair.json")).expect("load fixture");

    let (report, _session, _runtime) =
        run_fixture_retained(&definition, &fixture, WorkflowLimits::default())
            .await
            .expect("run fixture");

    assert!(report.ok, "fixture diagnostics: {:?}", report.diagnostics);
    assert_eq!(
        report.schema_repair_starts, 1,
        "exactly one schema repair start"
    );
    assert!(
        report.invocation_kinds.iter().any(|k| k == "delegate"),
        "{:?}",
        report.invocation_kinds
    );

    let envelopes = collect_journal(
        &report.journal_path,
        None,
        WorkflowLimits::default().journal_record_bytes,
        WorkflowLimits::default().journal_total_bytes,
    )
    .expect("journal readable after run");
    let repair_starts = envelopes
        .iter()
        .filter(|e| matches!(e.payload, JournalPayload::SchemaRepairStarted { .. }))
        .count();
    let repair_ok = envelopes.iter().any(|e| {
        matches!(
            &e.payload,
            JournalPayload::SchemaRepairFinished { ok: true, .. }
        )
    });
    assert_eq!(repair_starts, 1);
    assert!(repair_ok, "repair must finish ok: {envelopes:?}");
}

/// Await-user answer + real artifact commit survive completion (and rehydrate).
#[tokio::test]
async fn deterministic_fixture_replays_await_user_answer_and_artifact() {
    let script = r#"
neo.phase("run")
local answer = neo.await_user({
  prompt = "Continue?",
  answer_schema = {
    type = "object",
    properties = { ok = { type = "boolean" } },
    required = { "ok" },
    additionalProperties = false,
  },
  answer_policy = "human",
})
neo.verify(answer.ok, "user must approve")
return { ok = true }
"#;
    let definition = resolve_script(
        "await-artifact",
        "Await Artifact",
        "await user and artifact",
        script,
    );
    let fixture =
        load_fixture(&fixture_root().join("await_user_artifact.json")).expect("load fixture");

    let (report, session, runtime) =
        run_fixture_retained(&definition, &fixture, WorkflowLimits::default())
            .await
            .expect("run fixture");

    assert!(report.ok, "fixture diagnostics: {:?}", report.diagnostics);
    assert_eq!(report.final_result, Some(json!({"ok": true})));
    assert_eq!(report.state, WorkflowState::Completed.as_str());

    let envelopes = collect_journal(
        &report.journal_path,
        None,
        WorkflowLimits::default().journal_record_bytes,
        WorkflowLimits::default().journal_total_bytes,
    )
    .expect("journal");
    assert!(
        envelopes
            .iter()
            .any(|e| matches!(e.payload, JournalPayload::UserInputAnswered { .. })),
        "answer must be journaled"
    );
    assert!(
        envelopes
            .iter()
            .any(|e| matches!(e.payload, JournalPayload::ArtifactCommitted { .. })),
        "artifact must be journaled"
    );

    // Rehydrate projection still sees completed + artifacts (replay path).
    let runtime2 = neo_agent_core::workflow::WorkflowRuntime::new(WorkflowLimits::default());
    runtime2
        .bind_runner(|_h, _m, _s| async move {
            panic!("rehydrate must not auto-start workers for completed runs");
        })
        .expect("bind");
    let handles = runtime2.rehydrate(session.path()).await.expect("rehydrate");
    assert_eq!(handles.len(), 1);
    assert_eq!(handles[0].snapshot().await.state, WorkflowState::Completed);
    let output = handles[0].output().await.expect("output");
    assert!(
        output.artifacts.iter().any(|a| a.logical_name == "plan"),
        "artifact visible after rehydrate: {:?}",
        output.artifacts
    );
    let _ = runtime; // original runtime kept until end of test
}

/// Live execution is not a fixture mode — unknown live fields are rejected.
#[test]
fn live_execution_is_not_a_fixture_mode() {
    assert!(!FixtureExecutionMode::Deterministic.supports_live());
    assert!(!FixtureExecutionMode::default().supports_live());

    let err = parse_fixture(r#"{"live": true}"#).expect_err("live field must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown field") || msg.contains("live") || msg.contains("fixture parse"),
        "unexpected error: {msg}"
    );

    let err = parse_fixture(r#"{"mode":"live"}"#).expect_err("mode=live must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("live") || msg.contains("fixture parse") || msg.contains("unknown variant"),
        "unexpected error: {msg}"
    );

    // Deterministic empty fixture is accepted.
    let ok = parse_fixture("{}").expect("empty deterministic fixture");
    assert!(!ok.mode.supports_live());
    assert_eq!(ok.mode, FixtureExecutionMode::Deterministic);
}
