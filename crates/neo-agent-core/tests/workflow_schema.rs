//! Host schema validation tests for workflow structured outputs.

use neo_agent_core::AgentContext;
use neo_agent_core::harness::FakeHarness;
use neo_agent_core::runtime::WorkflowDispatchHandle;
use neo_agent_core::tools::{ProcessSupervisor, ToolRegistry};
use neo_agent_core::workflow::{
    CompiledSchema, LuaWorkflowRunner, SchemaErrorCode, StructuredOutputSource, WorkflowErrorCode,
    WorkflowLimits, WorkflowRuntime, accept_structured_output, attach_response_format_hint,
    parse_strict_json_value, validate_final_lua_result,
};
use neo_ai::RequestOptions;
use serde_json::json;

#[test]
fn exact_json_succeeds_and_prose_or_fences_fail() {
    let ok = parse_strict_json_value(r#"{"ok":true}"#).expect("exact JSON");
    assert_eq!(ok, json!({"ok": true}));

    for bad in [
        "```json\n{\"ok\":true}\n```",
        "sure: {\"ok\":true}",
        "{\"ok\":true}{\"ok\":false}",
        "not json",
        "",
        "  \n\t  ",
    ] {
        let err = parse_strict_json_value(bad).expect_err("must fail strict parse");
        assert_eq!(
            err.code,
            SchemaErrorCode::StrictJsonFailed,
            "input={bad:?} err={err}"
        );
    }
}

#[test]
fn provider_native_and_text_fallback_share_host_validation() {
    let schema_doc = json!({
        "type": "object",
        "properties": {
            "ok": { "type": "boolean" }
        },
        "required": ["ok"],
        "additionalProperties": false
    });
    let schema = CompiledSchema::compile(&schema_doc).expect("compile schema");

    let native = accept_structured_output(
        &schema,
        StructuredOutputSource::ProviderNative(json!({ "ok": true })),
    )
    .expect("provider-native valid value");
    let text = accept_structured_output(
        &schema,
        StructuredOutputSource::AssistantText(r#"{"ok":true}"#.to_owned()),
    )
    .expect("text fallback valid value");
    assert_eq!(native, text);
    assert_eq!(native, json!({ "ok": true }));

    // Both invalid paths fail with the same host error code.
    let bad_native = accept_structured_output(
        &schema,
        StructuredOutputSource::ProviderNative(json!({ "ok": "nope" })),
    )
    .expect_err("type mismatch must fail host validation");
    let bad_text = accept_structured_output(
        &schema,
        StructuredOutputSource::AssistantText(r#"{"ok":"nope"}"#.to_owned()),
    )
    .expect_err("type mismatch must fail host validation");
    assert_eq!(bad_native.code, SchemaErrorCode::SchemaInvalid);
    assert_eq!(bad_text.code, SchemaErrorCode::SchemaInvalid);

    // Text fallback is strict: fences / prose never become accepted JSON.
    let fenced = accept_structured_output(
        &schema,
        StructuredOutputSource::AssistantText("```json\n{\"ok\":true}\n```".to_owned()),
    )
    .expect_err("markdown fences must fail");
    assert_eq!(fenced.code, SchemaErrorCode::StrictJsonFailed);

    let prose = accept_structured_output(
        &schema,
        StructuredOutputSource::AssistantText("sure: {\"ok\":true}".to_owned()),
    )
    .expect_err("prose must fail");
    assert_eq!(prose.code, SchemaErrorCode::StrictJsonFailed);

    // Composition seam attaches a provider-neutral hint without bypassing host
    // validation: wire acceptance is not simulated; host still rejects bad values.
    let mut options = RequestOptions::default();
    attach_response_format_hint(&mut options, "child_output", schema_doc.clone(), true);
    let hint = options
        .response_format
        .expect("response format hint attached");
    assert_eq!(hint.name, "child_output");
    assert!(hint.strict);
    assert_eq!(hint.schema, schema_doc);

    // Even with a hint present, invalid text still fails the same host path.
    let still_invalid = accept_structured_output(
        &schema,
        StructuredOutputSource::AssistantText("not json".to_owned()),
    )
    .expect_err("hint does not skip host validation");
    assert_eq!(still_invalid.code, SchemaErrorCode::StrictJsonFailed);

    // Strict parse helper rejects multiple top-level values.
    let multi = parse_strict_json_value("{\"ok\":true}{\"ok\":false}")
        .expect_err("multiple values must fail");
    assert_eq!(multi.code, SchemaErrorCode::StrictJsonFailed);
}

/// Final Lua returns are host-validated only — no child session, no repair model call.
#[tokio::test]
async fn invalid_final_lua_result_fails_without_hidden_model_repair() {
    let schema_doc = json!({
        "type": "object",
        "properties": {
            "ok": { "type": "boolean" }
        },
        "required": ["ok"],
        "additionalProperties": false
    });
    let schema = CompiledSchema::compile(&schema_doc).expect("compile schema");

    // Pure host path: typed failure, no model dependency at all.
    let pure = validate_final_lua_result(&schema, &json!({ "ok": "nope" }))
        .expect_err("invalid final result");
    assert_eq!(pure.code, SchemaErrorCode::SchemaInvalid);
    assert!(
        pure.message.contains("schema_invalid_final_result"),
        "{}",
        pure.message
    );

    // Integration: empty FakeHarness records any model call. Invalid final
    // return must fail schema validation without consuming a model turn.
    let dir = tempfile::tempdir().unwrap();
    let harness = FakeHarness::from_turns([]);
    let config = neo_agent_core::AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path().to_path_buf())
        .expect("workspace root")
        .with_permission_mode(neo_agent_core::PermissionMode::Yolo);
    let dispatch = WorkflowDispatchHandle {
        config,
        model_client: harness.client(),
        registry: std::sync::Arc::new(ToolRegistry::with_builtin_tools()),
        process_supervisor: ProcessSupervisor::default(),
        context: AgentContext::new(),
    };
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = runtime
        .create_run(
            dir.path(),
            neo_agent_core::workflow::WorkflowLaunchRequest {
                name: "final-schema".to_owned(),
                description: "final schema fail".to_owned(),
                phases: Vec::new(),
                script: String::new(),
                args: json!({}),
                launch_source: "test".to_owned(),
                parent_run_id: None,
            },
        )
        .await
        .expect("create run");
    handle
        .enter_running_for_direct_execution()
        .await
        .expect("enter running");
    let runner = LuaWorkflowRunner::new(dispatch, handle.clone(), WorkflowLimits::default())
        .with_final_schema(schema, None);

    let error = runner
        .execute(r#"return { ok = "nope" }"#, json!({}))
        .await
        .expect_err("invalid final schema");
    assert_eq!(error.code(), WorkflowErrorCode::SchemaInvalid, "{error}");
    assert!(
        error.to_string().contains("schema_invalid_final_result"),
        "{error}"
    );
    assert!(
        harness.requests().is_empty(),
        "final schema failure must not call the model: {:?}",
        harness.requests()
    );
    let output = handle.output().await.expect("output");
    assert!(
        output.final_result.is_none(),
        "invalid final must not be persisted"
    );
}
