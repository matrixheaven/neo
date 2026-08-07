//! Host schema validation tests for workflow structured outputs.

use neo_agent_core::AgentContext;
use neo_agent_core::harness::FakeHarness;
use neo_agent_core::runtime::WorkflowDispatchHandle;
use neo_agent_core::tools::{ProcessSupervisor, ToolRegistry};
use neo_agent_core::workflow::{
    CompiledSchema, LuaWorkflowRunner, SchemaErrorCode, StructuredOutputSource, WorkflowLimits,
    WorkflowRuntime, accept_structured_output, attach_response_format_hint,
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

/// Final Lua returns are persisted exactly as returned even when they do not
/// match a declared output schema — a mismatch is a projection diagnostic, not
/// a failure, and never consumes a model turn.
#[tokio::test]
async fn final_lua_result_schema_mismatch_is_persisted() {
    let schema_doc = json!({
        "type": "object",
        "properties": {
            "ok": { "type": "boolean" }
        },
        "required": ["ok"],
        "additionalProperties": false
    });
    let schema = CompiledSchema::compile(&schema_doc).expect("compile schema");

    // The pure diagnostic path still describes the mismatch without rejecting.
    let pure = validate_final_lua_result(&schema, &json!({ "ok": "nope" }))
        .expect_err("invalid final result diagnostic");
    assert_eq!(pure.code, SchemaErrorCode::SchemaInvalid);
    assert!(
        pure.message.contains("schema_invalid_final_result"),
        "{}",
        pure.message
    );

    // Integration: empty FakeHarness records any model call. A schema-mismatched
    // final return must persist without consuming a model turn.
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
                description: "final schema projection".to_owned(),
                phases: Vec::new(),
                script: String::new(),
                args: json!({}),
                launch_source: "test".to_owned(),
                output_schema: None,
                display_name: None,
                input_schema: None,
                definition_origin: None,
                inline_unsaved: false,
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

    let result = runner
        .execute(r#"return { ok = "nope" }"#, json!({}))
        .await
        .expect("schema mismatch must not fail the run");
    assert_eq!(result, json!({ "ok": "nope" }));
    assert!(
        harness.requests().is_empty(),
        "final schema projection must not call the model: {:?}",
        harness.requests()
    );
    let output = handle.output().await.expect("output");
    let persisted = output
        .final_result
        .and_then(|meta| match meta.body {
            neo_agent_core::workflow::FinalResultBody::Inline { value } => Some(value),
            _ => None,
        })
        .expect("mismatched final result must still be persisted");
    assert_eq!(persisted, json!({ "ok": "nope" }));
}

/// Final-result schema failures must identify the failing instance path and a
/// bounded, Unicode-safe preview of the actual node — never the complete root.
#[test]
fn final_lua_schema_error_includes_path_and_bounded_actual() {
    let schema_doc = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string", "minLength": 3, "maxLength": 3 }
        },
        "required": ["name"],
        "additionalProperties": false
    });
    let schema = CompiledSchema::compile(&schema_doc).expect("compile schema");

    // Nested path plus the exact short value preview.
    let short = json!({ "name": "a" });
    let short_err = validate_final_lua_result(&schema, &short).expect_err("short name");
    assert_eq!(short_err.code, SchemaErrorCode::SchemaInvalid);
    assert!(
        short_err
            .message
            .contains("schema_invalid_final_result at /name"),
        "{}",
        short_err.message
    );
    assert!(
        short_err.message.contains("actual=\"a\""),
        "{}",
        short_err.message
    );

    // A serialized value at exactly 160 characters stays intact.
    let exact = json!({ "name": "汉".repeat(158) });
    let exact_err = validate_final_lua_result(&schema, &exact).expect_err("exact boundary");
    let exact_actual = exact_err
        .message
        .split_once("actual=")
        .map(|(_, tail)| tail)
        .expect("preview must be present");
    assert_eq!(exact_actual.chars().count(), 160, "{exact_actual}");
    assert_eq!(
        exact_actual,
        serde_json::to_string(&exact["name"]).expect("serialize exact value")
    );
    assert!(!exact_actual.ends_with('…'), "{exact_actual}");

    // Long Unicode value: the preview is cut safely at 160 characters and ends
    // with the Neo ellipsis; the complete long root object never leaks.
    let long_name = "汉".repeat(200);
    let long = json!({
        "name": long_name,
        "marker": "SECRET_ROOT_MARKER",
    });
    let long_err = validate_final_lua_result(&schema, &long).expect_err("long name");
    assert_eq!(long_err.code, SchemaErrorCode::SchemaInvalid);
    assert!(
        long_err
            .message
            .contains("schema_invalid_final_result at /name"),
        "{}",
        long_err.message
    );
    let actual = long_err
        .message
        .split_once("actual=")
        .map(|(_, tail)| tail)
        .expect("preview must be present");
    assert_eq!(actual.chars().count(), 160, "{}", actual);
    assert!(
        actual.ends_with('…'),
        "preview must end with the Neo ellipsis: {actual}"
    );
    assert!(
        actual.chars().filter(|c| *c == '汉').count() < 160,
        "preview must actually be cut: {actual}"
    );
    assert!(
        !long_err.message.contains("SECRET_ROOT_MARKER"),
        "complete long root object must not leak into the error: {}",
        long_err.message
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
            phase: neo_ai::MessagePhase::Unknown,
            id: format!("msg_{text}"),
        },
        AiStreamEvent::TextDelta {
            text: text.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
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

fn todo_turn_with_usage(
    id: &str,
    input_tokens: u32,
    output_tokens: u32,
) -> Vec<neo_ai::AiStreamEvent> {
    use neo_ai::{AiStreamEvent, StopReason, TokenUsage};
    vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: format!("message_{id}"),
        },
        AiStreamEvent::ToolCallStart {
            id: id.to_owned(),
            name: "TodoList".to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: id.to_owned(),
            raw_arguments: "{}".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::ToolUse,
            usage: Some(TokenUsage {
                input_tokens,
                output_tokens,
                input_cache_read_tokens: 0,
                input_cache_write_tokens: 0,
            }),
        },
    ]
}

fn error_turn_with_usage(
    message: &str,
    input_tokens: u32,
    output_tokens: u32,
) -> Vec<neo_ai::AiStreamEvent> {
    use neo_ai::{AiStreamEvent, StopReason, TokenUsage};
    vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "usage_before_failure".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: message.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::Error,
            usage: Some(TokenUsage {
                input_tokens,
                output_tokens,
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
                output_schema: None,
                display_name: None,
                input_schema: None,
                definition_origin: None,
                inline_unsaved: false,
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

/// A child whose text does not match its output schema stays `Completed`: only
/// the structured projection is unavailable, usage is preserved, and exactly one
/// model turn is consumed.
#[tokio::test]
async fn child_projection_mismatch_keeps_completed_status() {
    use neo_agent_core::workflow::{WorkflowInvocationKind, WorkflowOutcomeStatus};

    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = runtime
        .create_run(
            session_dir,
            neo_agent_core::workflow::WorkflowLaunchRequest {
                name: "delegate-schema-projection".to_owned(),
                description: "delegate schema projection".to_owned(),
                phases: Vec::new(),
                script: String::new(),
                args: json!({}),
                launch_source: "test".to_owned(),
                output_schema: None,
                display_name: None,
                input_schema: None,
                definition_origin: None,
                inline_unsaved: false,
            },
        )
        .await
        .expect("create run");
    handle
        .enter_running_for_direct_execution()
        .await
        .expect("enter running");

    let harness = FakeHarness::from_turns([text_turn(r#"{"ok":"nope"}"#, Some((10, 20)))]);
    let mut config = neo_agent_core::AgentConfig::for_model(harness.model());
    config.max_retries = 0;
    config = config
        .with_permission_mode(neo_agent_core::PermissionMode::Yolo)
        .with_workflow_runtime(runtime);
    let registry = std::sync::Arc::new(ToolRegistry::with_builtin_tools());
    config.tools = registry.specs();
    let input = json!({
        "task": "return structured ok",
        "output_schema": child_schema_doc(),
    });
    let dispatch = WorkflowDispatchHandle {
        config,
        model_client: harness.client(),
        registry,
        process_supervisor: ProcessSupervisor::default(),
        context: AgentContext::new(),
    };
    let origin = handle.execution_origin(None).await;
    let outcome = handle
        .invoke(
            0,
            WorkflowInvocationKind::Delegate,
            input.clone(),
            true,
            move |invocation| async move {
                dispatch
                    .run_one_with_origin(invocation, "Delegate", input, Some(origin))
                    .await
            },
        )
        .await
        .expect("invoke");

    assert!(outcome.is_completed(), "{outcome:?}");
    assert_eq!(outcome.status, WorkflowOutcomeStatus::Completed);
    assert!(
        outcome.details.get("structured_output").is_none(),
        "mismatch must not produce structured output: {outcome:?}"
    );
    assert!(
        outcome.details["projection_error"]
            .as_str()
            .is_some_and(|s| s.contains("structured projection unavailable")),
        "{outcome:?}"
    );
    assert_eq!(outcome.child_refs.len(), 1, "{outcome:?}");
    assert_eq!(outcome.child_refs[0].kind, "delegate");

    let requests = harness.requests();
    assert_eq!(
        requests.len(),
        1,
        "projection mismatch must not start a repair turn: {requests:?}"
    );
    let usage = outcome.actual_usage.expect("usage");
    assert_eq!(usage.input_tokens, 10);
    assert_eq!(usage.output_tokens, 20);
}

/// A projection mismatch writes no schema-repair journal records.
#[tokio::test]
async fn child_projection_mismatch_does_not_start_repair() {
    use neo_agent_core::workflow::journal::{JournalPayload, collect_journal};
    use neo_agent_core::workflow::{WorkflowInvocationKind, WorkflowOutcomeStatus};

    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = runtime
        .create_run(
            session_dir,
            neo_agent_core::workflow::WorkflowLaunchRequest {
                name: "delegate-schema-projection".to_owned(),
                description: "delegate schema projection".to_owned(),
                phases: Vec::new(),
                script: String::new(),
                args: json!({}),
                launch_source: "test".to_owned(),
                output_schema: None,
                display_name: None,
                input_schema: None,
                definition_origin: None,
                inline_unsaved: false,
            },
        )
        .await
        .expect("create run");
    handle
        .enter_running_for_direct_execution()
        .await
        .expect("enter running");

    let harness = FakeHarness::from_turns([text_turn("prose instead of json", Some((10, 20)))]);
    let mut config = neo_agent_core::AgentConfig::for_model(harness.model());
    config.max_retries = 0;
    config = config
        .with_permission_mode(neo_agent_core::PermissionMode::Yolo)
        .with_workflow_runtime(runtime);
    let registry = std::sync::Arc::new(ToolRegistry::with_builtin_tools());
    config.tools = registry.specs();
    let input = json!({
        "task": "return structured ok",
        "output_schema": child_schema_doc(),
    });
    let dispatch = WorkflowDispatchHandle {
        config,
        model_client: harness.client(),
        registry,
        process_supervisor: ProcessSupervisor::default(),
        context: AgentContext::new(),
    };
    let origin = handle.execution_origin(None).await;
    let outcome = handle
        .invoke(
            0,
            WorkflowInvocationKind::Delegate,
            input.clone(),
            true,
            move |invocation| async move {
                dispatch
                    .run_one_with_origin(invocation, "Delegate", input, Some(origin))
                    .await
            },
        )
        .await
        .expect("invoke");

    assert!(outcome.is_completed(), "{outcome:?}");
    assert_eq!(outcome.status, WorkflowOutcomeStatus::Completed);

    let run_dir = neo_agent_core::workflow::run_dir(session_dir, &handle.run_id);
    let envelopes = collect_journal(
        &run_dir.join("journal.jsonl"),
        Some(&handle.run_id),
        WorkflowLimits::default().journal_record_bytes,
        WorkflowLimits::default().journal_total_bytes,
    )
    .expect("journal");
    let repair_records = envelopes
        .iter()
        .filter(|e| {
            matches!(
                e.payload,
                JournalPayload::SchemaRepairStarted { .. }
                    | JournalPayload::SchemaRepairFinished { .. }
            )
        })
        .count();
    assert_eq!(
        repair_records, 0,
        "projection mismatch must write no schema-repair journal records: {envelopes:?}"
    );
}

/// A mixed swarm preserves per-item state: a failed child keeps its original
/// error, a projection-unavailable child stays completed, a schema-valid child
/// keeps its structured output, and observed usage is aggregated — all without
/// any repair turn.
#[tokio::test]
async fn swarm_mixed_results_preserve_usage_and_partial_state() {
    use neo_agent_core::multi_agent::{
        AgentRole, ChildPlan, ChildRuntimeDeps, ChildWorktreePolicy, DelegateContext,
        MultiAgentRuntime,
    };
    use neo_agent_core::workflow::journal::{JournalPayload, collect_journal};
    use neo_agent_core::workflow::{SwarmBatchRequest, WorkflowOutcomeStatus};

    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path();
    let handle = running_workflow_handle(session_dir).await;
    let harness = FakeHarness::from_turns([
        error_turn_with_usage("response_format unsupported on compatible endpoint", 17, 19),
        text_turn("prose instead of json", Some((10, 20))),
        text_turn(r#"{"ok":true}"#, Some((5, 7))),
    ]);
    let mut config = neo_agent_core::AgentConfig::for_model(harness.model());
    config.max_retries = 0;
    config = config.with_permission_mode(neo_agent_core::PermissionMode::Yolo);
    let multi = MultiAgentRuntime::new().with_session_directory(session_dir.to_path_buf());
    let deps = ChildRuntimeDeps::new(
        config
            .with_workspace_root(session_dir.to_path_buf())
            .expect("workspace"),
        harness.client(),
        std::sync::Arc::new(ToolRegistry::new()),
    );
    let plans = ["item-a", "item-b", "item-c"]
        .into_iter()
        .map(|item_id| ChildPlan {
            item_id: item_id.to_owned(),
            item_label: item_id.to_owned(),
            task: "return structured ok".to_owned(),
            title: None,
            resume: None,
            role: None,
            model: None,
            provider: None,
            context: DelegateContext::None,
            worktree: ChildWorktreePolicy::Shared,
            tool_allow: None,
            output_schema: Some(child_schema_doc()),
        })
        .collect();
    let outcome = handle
        .invoke_swarm_batch(
            SwarmBatchRequest {
                call_index: 0,
                canonical_input: json!({
                    "description": "mixed swarm",
                    "items": [{
                        "task": "return structured ok",
                        "output_schema": child_schema_doc(),
                    }],
                }),
                description: "mixed swarm".to_owned(),
                role: AgentRole::Coder,
                max_concurrency: 1,
                plans,
            },
            multi,
            deps,
        )
        .await
        .expect("swarm batch");

    // One real child failure keeps the aggregate bounded and partial; the other
    // items remain visible with their own state.
    assert!(!outcome.is_completed(), "{outcome:?}");
    assert_eq!(outcome.status, WorkflowOutcomeStatus::Failed, "{outcome:?}");
    assert!(
        outcome.summary.contains("failed 1/3"),
        "bounded partial summary: {}",
        outcome.summary
    );
    let items = outcome
        .details
        .get("items")
        .and_then(serde_json::Value::as_array)
        .expect("ordered item details");
    assert_eq!(items.len(), 3, "{items:?}");
    assert_eq!(items[0]["item_id"], json!("item-a"));
    assert_eq!(items[0]["status"], json!("failed"));
    assert!(
        items[0]["summary"]
            .as_str()
            .is_some_and(|s| s.contains("response_format unsupported")),
        "original provider error must survive: {items:?}"
    );
    assert_eq!(items[1]["item_id"], json!("item-b"));
    assert_eq!(items[1]["status"], json!("completed"));
    assert!(
        items[1].get("structured_output").is_none(),
        "prose must not produce structured output: {items:?}"
    );
    assert!(
        items[1]["projection_error"]
            .as_str()
            .is_some_and(|s| s.contains("structured projection unavailable")),
        "{items:?}"
    );
    assert_eq!(items[2]["item_id"], json!("item-c"));
    assert_eq!(items[2]["status"], json!("completed"));
    assert_eq!(items[2]["structured_output"], json!({"ok": true}));

    let usage = outcome.actual_usage.expect("usage");
    assert_eq!(usage.input_tokens, 32);
    assert_eq!(usage.output_tokens, 46);
    assert_eq!(
        harness.requests().len(),
        3,
        "one turn per child, no repair: {:?}",
        harness.requests()
    );

    let run_dir = neo_agent_core::workflow::run_dir(session_dir, &handle.run_id);
    let envelopes = collect_journal(
        &run_dir.join("journal.jsonl"),
        Some(&handle.run_id),
        WorkflowLimits::default().journal_record_bytes,
        WorkflowLimits::default().journal_total_bytes,
    )
    .expect("journal");
    let repair_records = envelopes
        .iter()
        .filter(|envelope| {
            matches!(
                envelope.payload,
                JournalPayload::SchemaRepairStarted { .. }
                    | JournalPayload::SchemaRepairFinished { .. }
            )
        })
        .count();
    assert_eq!(repair_records, 0, "{envelopes:?}");
}

/// A non-model runtime failure keeps prior events, usage, and its child
/// reference unchanged, with no projection attempt and no repair request.
#[tokio::test]
async fn failed_child_preserves_original_error_without_projection() {
    use neo_agent_core::workflow::journal::{JournalPayload, collect_journal};
    use neo_agent_core::workflow::{
        WorkflowInvocationKind, WorkflowLaunchRequest, WorkflowOutcomeStatus,
    };
    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = runtime
        .create_run(
            session_dir,
            WorkflowLaunchRequest {
                name: "delegate-runtime".to_owned(),
                description: "delegate runtime failure".to_owned(),
                phases: Vec::new(),
                script: String::new(),
                args: json!({}),
                launch_source: "test".to_owned(),
                output_schema: None,
                display_name: None,
                input_schema: None,
                definition_origin: None,
                inline_unsaved: false,
            },
        )
        .await
        .expect("create run");
    handle
        .enter_running_for_direct_execution()
        .await
        .expect("enter running");

    let harness = FakeHarness::from_turns([
        todo_turn_with_usage("remove_workspace", 11, 13),
        todo_turn_with_usage("fail_context", 17, 19),
    ]);
    let workspace = session_dir.join("runtime-workspace");
    std::fs::create_dir(&workspace).expect("create workspace");
    let workspace_to_remove = workspace.clone();
    let mut config = neo_agent_core::AgentConfig::for_model(harness.model());
    config.max_retries = 0;
    config = config
        .with_permission_mode(neo_agent_core::PermissionMode::Yolo)
        .with_workflow_runtime(runtime)
        .with_workspace_root(&workspace)
        .expect("workspace")
        .with_async_before_tool_call(move |call, _| {
            let workspace = workspace_to_remove.clone();
            async move {
                if call.name.as_ref() == "TodoList" && workspace.exists() {
                    std::fs::remove_dir(&workspace).expect("remove empty workspace");
                }
                None
            }
        });
    // Foreground Delegate path through normal workflow dispatch: the tool runs
    // the child turn and then applies the output schema.
    let input = json!({
        "task": "return structured ok",
        "output_schema": child_schema_doc(),
    });
    let dispatch = WorkflowDispatchHandle {
        config,
        model_client: harness.client(),
        registry: std::sync::Arc::new(ToolRegistry::with_builtin_tools()),
        process_supervisor: ProcessSupervisor::default(),
        context: AgentContext::new(),
    };
    let origin = handle.execution_origin(None).await;
    let outcome = handle
        .invoke(
            0,
            WorkflowInvocationKind::Delegate,
            input.clone(),
            true,
            move |invocation| async move {
                dispatch
                    .run_one_with_origin(invocation, "Delegate", input, Some(origin))
                    .await
            },
        )
        .await
        .expect("invoke");

    assert!(!outcome.is_completed(), "{outcome:?}");
    assert_eq!(outcome.status, WorkflowOutcomeStatus::Failed, "{outcome:?}");
    assert!(
        outcome.summary.contains("tool execution failed"),
        "original runtime error must survive in summary: {}",
        outcome.summary
    );
    assert_eq!(
        harness.requests().len(),
        2,
        "failed child must not trigger a repair request: {:?}",
        harness.requests()
    );
    let usage = outcome.actual_usage.expect("usage before runtime failure");
    assert_eq!(usage.input_tokens, 28);
    assert_eq!(usage.output_tokens, 32);
    assert_eq!(outcome.child_refs.len(), 1, "{outcome:?}");
    assert_eq!(outcome.child_refs[0].kind, "delegate");
    assert!(!outcome.child_refs[0].id.is_empty());

    let run_dir = neo_agent_core::workflow::run_dir(session_dir, &handle.run_id);
    let envelopes = collect_journal(
        &run_dir.join("journal.jsonl"),
        Some(&handle.run_id),
        WorkflowLimits::default().journal_record_bytes,
        WorkflowLimits::default().journal_total_bytes,
    )
    .expect("journal");
    let kinds: Vec<&str> = envelopes
        .iter()
        .map(|e| match &e.payload {
            JournalPayload::SchemaRepairStarted { .. } => "schema_repair_started",
            JournalPayload::SchemaRepairFinished { .. } => "schema_repair_finished",
            _ => "other",
        })
        .collect();
    assert!(
        !kinds.contains(&"schema_repair_started") && !kinds.contains(&"schema_repair_finished"),
        "failed child must write no schema-repair journal records: {kinds:?}"
    );
    let serialized = outcome.details.to_string();
    assert!(
        !serialized.contains("strict_json_failed") && !serialized.contains("schema_error"),
        "no schema-failure replacement in details: {serialized}"
    );
    assert!(
        !outcome.summary.contains("strict_json_failed"),
        "no schema-failure replacement in summary: {}",
        outcome.summary
    );
}

/// A direct workflow swarm child with a provider-reported error stop keeps
/// observed usage and skips schema repair through the real swarm consumer.
#[tokio::test]
async fn workflow_swarm_failure_skips_schema_repair_and_preserves_error() {
    use neo_agent_core::multi_agent::{
        AgentRole, ChildPlan, ChildRuntimeDeps, ChildWorktreePolicy, DelegateContext,
        MultiAgentRuntime,
    };
    use neo_agent_core::workflow::journal::{JournalPayload, collect_journal};
    use neo_agent_core::workflow::{SwarmBatchRequest, WorkflowOutcomeStatus};
    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path();
    let handle = running_workflow_handle(session_dir).await;

    let harness = FakeHarness::from_turns([error_turn_with_usage(
        "response_format unsupported on compatible endpoint",
        17,
        19,
    )]);
    let mut config = neo_agent_core::AgentConfig::for_model(harness.model());
    config.max_retries = 0;
    config = config.with_permission_mode(neo_agent_core::PermissionMode::Yolo);
    let multi = MultiAgentRuntime::new().with_session_directory(session_dir.to_path_buf());
    let deps = ChildRuntimeDeps::new(
        config
            .with_workspace_root(session_dir.to_path_buf())
            .expect("workspace"),
        harness.client(),
        std::sync::Arc::new(ToolRegistry::with_builtin_tools()),
    );

    let plan = ChildPlan {
        item_id: "item-a".to_owned(),
        item_label: "a".to_owned(),
        task: "return structured ok".to_owned(),
        title: None,
        resume: None,
        role: None,
        model: None,
        provider: None,
        context: DelegateContext::None,
        worktree: ChildWorktreePolicy::Shared,
        tool_allow: None,
        output_schema: Some(child_schema_doc()),
    };
    let request = SwarmBatchRequest {
        call_index: 0,
        canonical_input: json!({
            "description": "protocol failure swarm",
            "items": [{
                "task": "return structured ok",
                "output_schema": child_schema_doc(),
            }],
        }),
        description: "protocol failure swarm".to_owned(),
        role: AgentRole::Coder,
        max_concurrency: 1,
        plans: vec![plan],
    };
    let outcome = handle
        .invoke_swarm_batch(request, multi, deps)
        .await
        .expect("swarm batch");

    assert!(!outcome.is_completed(), "{outcome:?}");
    assert_eq!(outcome.status, WorkflowOutcomeStatus::Failed, "{outcome:?}");
    assert_eq!(
        harness.requests().len(),
        1,
        "failed swarm child must not trigger a repair request: {:?}",
        harness.requests()
    );
    let usage = outcome.actual_usage.expect("usage before provider failure");
    assert_eq!(usage.input_tokens, 17);
    assert_eq!(usage.output_tokens, 19);
    let items = outcome
        .details
        .get("items")
        .and_then(serde_json::Value::as_array)
        .expect("ordered item details");
    assert_eq!(items.len(), 1, "{items:?}");
    let first = &items[0];
    assert_eq!(first["item_id"], json!("item-a"));
    assert_eq!(first["status"], json!("failed"));
    assert!(
        first["summary"]
            .as_str()
            .is_some_and(|s| s.contains("response_format unsupported")),
        "original provider error must survive in item details: {first}"
    );
    let serialized = outcome.details.to_string();
    assert!(
        !serialized.contains("strict_json_failed") && !serialized.contains("schema_error"),
        "no schema-failure replacement in details: {serialized}"
    );

    let run_dir = neo_agent_core::workflow::run_dir(session_dir, &handle.run_id);
    let envelopes = collect_journal(
        &run_dir.join("journal.jsonl"),
        Some(&handle.run_id),
        WorkflowLimits::default().journal_record_bytes,
        WorkflowLimits::default().journal_total_bytes,
    )
    .expect("journal");
    let kinds: Vec<&str> = envelopes
        .iter()
        .map(|e| match &e.payload {
            JournalPayload::SchemaRepairStarted { .. } => "schema_repair_started",
            JournalPayload::SchemaRepairFinished { .. } => "schema_repair_finished",
            _ => "other",
        })
        .collect();
    assert!(
        !kinds.contains(&"schema_repair_started") && !kinds.contains(&"schema_repair_finished"),
        "failed swarm child must write no schema-repair journal records: {kinds:?}"
    );
}
