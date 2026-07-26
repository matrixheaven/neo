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
                output_schema: None,
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

fn child_schema_doc() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "ok": { "type": "boolean" }
        },
        "required": ["ok"],
        "additionalProperties": false
    })
}

fn text_turn(text: &str, usage: Option<(u32, u32)>) -> Vec<neo_ai::AiStreamEvent> {
    use neo_ai::{AiStreamEvent, StopReason, TokenUsage};
    vec![
        AiStreamEvent::MessageStart {
            id: format!("msg_{text}"),
        },
        AiStreamEvent::TextDelta {
            text: text.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::EndTurn,
            usage: usage.map(|(input_tokens, output_tokens)| TokenUsage {
                input_tokens,
                output_tokens,
                input_cache_read_tokens: 0,
                input_cache_write_tokens: 0,
            }),
        },
    ]
}

fn tool_attempt_turn() -> Vec<neo_ai::AiStreamEvent> {
    use neo_ai::{AiStreamEvent, StopReason};
    vec![
        AiStreamEvent::MessageStart {
            id: "repair_tool".to_owned(),
        },
        AiStreamEvent::ToolCallStart {
            id: "call_1".to_owned(),
            name: "Bash".to_owned(),
        },
        AiStreamEvent::ToolCallArgsDelta {
            id: "call_1".to_owned(),
            json_fragment: r#"{"command":"echo no"}"#.to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: "call_1".to_owned(),
            raw_arguments: r#"{"command":"echo no"}"#.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::ToolUse,
            usage: Some(neo_ai::TokenUsage {
                input_tokens: 3,
                output_tokens: 4,
                input_cache_read_tokens: 0,
                input_cache_write_tokens: 0,
            }),
        },
    ]
}

async fn running_workflow_handle(
    dir: &std::path::Path,
) -> neo_agent_core::workflow::WorkflowHandle {
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = runtime
        .create_run(
            dir,
            neo_agent_core::workflow::WorkflowLaunchRequest {
                name: "child-schema".to_owned(),
                description: "child schema repair".to_owned(),
                phases: Vec::new(),
                script: String::new(),
                args: json!({}),
                launch_source: "test".to_owned(),
                parent_run_id: None,
                output_schema: None,
            },
        )
        .await
        .expect("create run");
    handle
        .enter_running_for_direct_execution()
        .await
        .expect("enter running");
    handle
}

/// Invalid first child output journals repair-start, runs exactly one tools-disabled
/// corrective model call, then journals repair-finish with aggregated usage.
#[tokio::test]
async fn child_schema_invalid_output_gets_exactly_one_tools_disabled_repair() {
    use neo_agent_core::multi_agent::{
        AgentRunMode, ChildRuntimeDeps, DelegateContext, DelegateRequest, MultiAgentRuntime,
    };
    use neo_agent_core::workflow::journal::{JournalPayload, collect_journal_v2};
    use neo_agent_core::workflow::{WorkflowInvocationKind, WorkflowOutcomeStatus};

    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path();
    let handle = running_workflow_handle(session_dir).await;

    let harness = FakeHarness::from_turns([
        text_turn(r#"{"ok":"nope"}"#, Some((10, 20))),
        text_turn(r#"{"ok":true}"#, Some((5, 7))),
    ]);
    let multi = MultiAgentRuntime::new().with_session_directory(session_dir.to_path_buf());
    let deps = ChildRuntimeDeps::new(
        neo_agent_core::AgentConfig::for_model(harness.model())
            .with_workspace_root(session_dir.to_path_buf())
            .expect("workspace"),
        harness.client(),
        std::sync::Arc::new(ToolRegistry::new()),
    );
    let request = DelegateRequest {
        task: "return structured ok".to_owned(),
        resume: None,
        title: None,
        role: None,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::None,
        output_schema: Some(child_schema_doc()),
    };
    let first = multi
        .run_child_turn(deps.clone(), &request, AgentRunMode::Foreground)
        .await
        .expect("first child turn");
    assert_eq!(
        harness.requests().len(),
        1,
        "only original child turn so far"
    );

    let schema = CompiledSchema::compile(&child_schema_doc()).expect("schema");

    let accepted = handle
        .invoke(
            0,
            WorkflowInvocationKind::Delegate,
            json!({"task": "return structured ok", "output_schema": child_schema_doc()}),
            true,
            {
                let multi = multi.clone();
                let deps = deps.clone();
                let agent_id = first.snapshot.id.clone();
                let first = first.clone();
                let schema = schema.clone();
                let handle = handle.clone();
                move |ctx| async move {
                    let accepted = handle
                        .accept_child_structured_output_with_repair(
                            &multi,
                            deps,
                            neo_agent_core::workflow::ChildSchemaRepairRequest {
                                invocation_id: &ctx.invocation_id,
                                agent_id: &agent_id,
                                schema: &schema,
                                first_output: &first,
                            },
                        )
                        .await
                        .expect("accept with repair");
                    neo_agent_core::workflow::WorkflowInvocationOutcome {
                        ok: accepted.ok,
                        status: if accepted.ok {
                            WorkflowOutcomeStatus::Completed
                        } else {
                            WorkflowOutcomeStatus::Failed
                        },
                        summary: accepted.summary.clone(),
                        interruption: None,
                        details: json!({
                            "structured_output": accepted.value,
                            "schema_repair_attempted": accepted.repair_attempted,
                            "first_raw": accepted.first_raw,
                            "repair_raw": accepted.repair_raw,
                            "actual_usage": accepted.actual_usage,
                        }),
                        actual_usage: accepted.actual_usage,
                        child_refs: vec![],
                    }
                }
            },
        )
        .await
        .expect("invoke");

    assert!(accepted.ok, "{accepted:?}");
    assert_eq!(accepted.details["structured_output"], json!({"ok": true}));
    assert_eq!(accepted.details["schema_repair_attempted"], json!(true));

    let requests = harness.requests();
    assert_eq!(
        requests.len(),
        2,
        "expected original + one repair: {requests:?}"
    );
    assert!(
        requests[1].tools.is_empty(),
        "repair turn must advertise no tools: {:?}",
        requests[1].tools
    );

    let usage = accepted.actual_usage.expect("usage");
    assert_eq!(usage.input_tokens, 15);
    assert_eq!(usage.output_tokens, 27);

    let run_dir = neo_agent_core::workflow::run_dir(session_dir, &handle.run_id);
    let envelopes =
        collect_journal_v2(&run_dir.join("journal.jsonl"), Some(&handle.run_id)).expect("journal");
    let kinds: Vec<&str> = envelopes
        .iter()
        .map(|e| match &e.payload {
            JournalPayload::SchemaRepairStarted { .. } => "schema_repair_started",
            JournalPayload::SchemaRepairFinished { ok, .. } => {
                if *ok {
                    "schema_repair_finished_ok"
                } else {
                    "schema_repair_finished_err"
                }
            }
            _ => "other",
        })
        .collect();
    assert!(kinds.contains(&"schema_repair_started"), "{kinds:?}");
    assert!(kinds.contains(&"schema_repair_finished_ok"), "{kinds:?}");
    let repair_starts = kinds
        .iter()
        .filter(|k| **k == "schema_repair_started")
        .count();
    assert_eq!(repair_starts, 1, "exactly one repair start: {kinds:?}");
}

/// A tool call during the repair turn fails with schema_repair_tool_forbidden and
/// never starts a second repair continuation.
#[tokio::test]
async fn schema_repair_tool_attempt_is_forbidden() {
    use neo_agent_core::multi_agent::{
        AgentRunMode, ChildRuntimeDeps, DelegateContext, DelegateRequest, MultiAgentRuntime,
    };
    use neo_agent_core::workflow::journal::{JournalPayload, collect_journal_v2};
    use neo_agent_core::workflow::{
        WorkflowErrorCode, WorkflowInvocationKind, WorkflowOutcomeStatus,
    };

    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path();
    let handle = running_workflow_handle(session_dir).await;

    let harness = FakeHarness::from_turns([
        text_turn("not-json-at-all", Some((1, 1))),
        tool_attempt_turn(),
    ]);
    let multi = MultiAgentRuntime::new().with_session_directory(session_dir.to_path_buf());
    let deps = ChildRuntimeDeps::new(
        neo_agent_core::AgentConfig::for_model(harness.model())
            .with_workspace_root(session_dir.to_path_buf())
            .expect("workspace"),
        harness.client(),
        std::sync::Arc::new(ToolRegistry::new()),
    );
    let request = DelegateRequest {
        task: "return structured ok".to_owned(),
        resume: None,
        title: None,
        role: None,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::None,
        output_schema: Some(child_schema_doc()),
    };
    let first = multi
        .run_child_turn(deps.clone(), &request, AgentRunMode::Foreground)
        .await
        .expect("first child");
    let schema = CompiledSchema::compile(&child_schema_doc()).expect("schema");

    let outcome = handle
        .invoke(
            0,
            WorkflowInvocationKind::Delegate,
            json!({"task": "x"}),
            true,
            {
                let multi = multi.clone();
                let deps = deps.clone();
                let agent_id = first.snapshot.id.clone();
                let first = first.clone();
                let schema = schema.clone();
                let handle = handle.clone();
                move |ctx| async move {
                    let accepted = handle
                        .accept_child_structured_output_with_repair(
                            &multi,
                            deps,
                            neo_agent_core::workflow::ChildSchemaRepairRequest {
                                invocation_id: &ctx.invocation_id,
                                agent_id: &agent_id,
                                schema: &schema,
                                first_output: &first,
                            },
                        )
                        .await
                        .expect("accept");
                    neo_agent_core::workflow::WorkflowInvocationOutcome {
                        ok: accepted.ok,
                        status: WorkflowOutcomeStatus::Failed,
                        summary: accepted.summary.clone(),
                        interruption: None,
                        details: json!({
                            "schema_error_code": accepted.error_code.map(|c| c.as_str()),
                            "repair_attempted": accepted.repair_attempted,
                        }),
                        actual_usage: accepted.actual_usage,
                        child_refs: vec![],
                    }
                }
            },
        )
        .await
        .expect("invoke");

    assert!(!outcome.ok);
    assert_eq!(
        outcome.details["schema_error_code"],
        json!(WorkflowErrorCode::SchemaRepairToolForbidden.as_str())
    );
    assert_eq!(
        harness.requests().len(),
        2,
        "one original + one failed repair"
    );
    assert!(harness.requests()[1].tools.is_empty());

    let run_dir = neo_agent_core::workflow::run_dir(session_dir, &handle.run_id);
    let envelopes =
        collect_journal_v2(&run_dir.join("journal.jsonl"), Some(&handle.run_id)).expect("journal");
    let finished_err = envelopes.iter().any(|e| {
        matches!(
            &e.payload,
            JournalPayload::SchemaRepairFinished { ok: false, summary, .. }
            if summary.contains("schema_repair_tool_forbidden")
        )
    });
    assert!(finished_err, "{envelopes:?}");
    let starts = envelopes
        .iter()
        .filter(|e| matches!(e.payload, JournalPayload::SchemaRepairStarted { .. }))
        .count();
    assert_eq!(starts, 1);
}

/// Crash after SchemaRepairStarted must never re-dispatch the corrective model effect.
#[tokio::test]
async fn crash_during_repair_never_repeats_model_effect() {
    use neo_agent_core::multi_agent::{
        AgentRunMode, ChildRuntimeDeps, DelegateContext, DelegateRequest, MultiAgentRuntime,
    };
    use neo_agent_core::workflow::WorkflowErrorCode;

    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path();
    let handle = running_workflow_handle(session_dir).await;

    let harness = FakeHarness::from_turns([text_turn("not-json", Some((2, 2)))]);
    let multi = MultiAgentRuntime::new().with_session_directory(session_dir.to_path_buf());
    let deps = ChildRuntimeDeps::new(
        neo_agent_core::AgentConfig::for_model(harness.model())
            .with_workspace_root(session_dir.to_path_buf())
            .expect("workspace"),
        harness.client(),
        std::sync::Arc::new(ToolRegistry::new()),
    );
    let request = DelegateRequest {
        task: "return structured ok".to_owned(),
        resume: None,
        title: None,
        role: None,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::None,
        output_schema: Some(child_schema_doc()),
    };
    let first = multi
        .run_child_turn(deps.clone(), &request, AgentRunMode::Foreground)
        .await
        .expect("first child");
    assert_eq!(harness.requests().len(), 1);

    let schema = CompiledSchema::compile(&child_schema_doc()).expect("schema");

    let repair_id = handle
        .start_schema_repair("inv_crash_repair")
        .await
        .expect("repair start");
    assert!(!repair_id.is_empty());
    assert_eq!(
        harness.requests().len(),
        1,
        "start_schema_repair must not call the model"
    );

    let accepted = handle
        .accept_child_structured_output_with_repair(
            &multi,
            deps,
            neo_agent_core::workflow::ChildSchemaRepairRequest {
                invocation_id: "inv_crash_repair",
                agent_id: &first.snapshot.id,
                schema: &schema,
                first_output: &first,
            },
        )
        .await
        .expect("accept after crash");
    assert!(!accepted.ok);
    assert_eq!(
        accepted.error_code,
        Some(WorkflowErrorCode::InterruptedHostExit)
    );
    assert_eq!(
        harness.requests().len(),
        1,
        "crash recovery must not re-dispatch repair model effect"
    );

    let err = handle
        .start_schema_repair("inv_crash_repair")
        .await
        .expect_err("duplicate start");
    assert_eq!(err.code(), WorkflowErrorCode::InterruptedHostExit);
    assert_eq!(harness.requests().len(), 1);
}
