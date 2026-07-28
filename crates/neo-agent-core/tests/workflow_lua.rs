use std::sync::{Arc, Mutex};

use neo_agent_core::AgentContext;
use neo_agent_core::harness::FakeHarness;
use neo_agent_core::runtime::WorkflowDispatchHandle;
use neo_agent_core::tools::{
    ProcessSupervisor, Tool, ToolContext, ToolFuture, ToolRegistry, ToolResult,
};
use neo_agent_core::workflow::journal::{JournalPayload, collect_journal_v2, run_dir};
use neo_agent_core::workflow::{
    LuaWorkflowRunner, WorkflowActor, WorkflowHandle, WorkflowInvocationKind, WorkflowLimits,
    WorkflowPhase, WorkflowRuntime,
};

struct RunnerFixture {
    session_dir: tempfile::TempDir,
    runner: LuaWorkflowRunner,
    handle: WorkflowHandle,
}

async fn make_runner() -> RunnerFixture {
    make_runner_with(WorkflowLimits::default(), Vec::new()).await
}

async fn make_runner_with(limits: WorkflowLimits, phases: Vec<WorkflowPhase>) -> RunnerFixture {
    make_runner_with_registry(limits, phases, ToolRegistry::with_builtin_tools()).await
}

async fn make_runner_with_registry(
    limits: WorkflowLimits,
    phases: Vec<WorkflowPhase>,
    registry: ToolRegistry,
) -> RunnerFixture {
    make_runner_with_config(limits, phases, registry, |config| config).await
}

async fn make_runner_with_config(
    limits: WorkflowLimits,
    phases: Vec<WorkflowPhase>,
    registry: ToolRegistry,
    configure: impl FnOnce(neo_agent_core::AgentConfig) -> neo_agent_core::AgentConfig,
) -> RunnerFixture {
    let dir = tempfile::tempdir().unwrap();
    let harness = FakeHarness::from_turns([]);
    let config = configure(
        neo_agent_core::AgentConfig::for_model(harness.model())
            .with_workspace_root(dir.path().to_path_buf())
            .expect("workspace root")
            .with_permission_mode(neo_agent_core::PermissionMode::Yolo),
    );
    let registry = Arc::new(registry);
    let dispatch = WorkflowDispatchHandle {
        config,
        model_client: harness.client(),
        registry,
        process_supervisor: ProcessSupervisor::default(),
        context: AgentContext::new(),
    };
    let runtime = WorkflowRuntime::new(limits.clone());
    let handle = runtime
        .create_run(
            dir.path(),
            neo_agent_core::workflow::WorkflowLaunchRequest {
                name: "test".to_owned(),
                description: "test".to_owned(),
                phases,
                script: String::new(),
                args: serde_json::json!({}),
                launch_source: "test".to_owned(),
                parent_run_id: None,
                output_schema: None,},
        )
        .await
        .expect("create run");
    handle
        .enter_running_for_direct_execution()
        .await
        .expect("enter running for direct Lua execution");
    let runner = LuaWorkflowRunner::new(dispatch, handle.clone(), limits);
    RunnerFixture {
        session_dir: dir,
        runner,
        handle,
    }
}

fn journal_path(fixture: &RunnerFixture) -> std::path::PathBuf {
    run_dir(fixture.session_dir.path(), &fixture.handle.run_id).join("journal.jsonl")
}

fn v2_started_kinds(fixture: &RunnerFixture) -> Vec<WorkflowInvocationKind> {
    let envelopes = collect_journal_v2(&journal_path(fixture), Some(&fixture.handle.run_id))
        .expect("collect v2 journal");
    envelopes
        .into_iter()
        .filter_map(|envelope| match envelope.payload {
            JournalPayload::InvocationStarted { kind, .. } => Some(kind),
            _ => None,
        })
        .collect()
}

fn v2_started_inputs(fixture: &RunnerFixture) -> Vec<serde_json::Value> {
    let envelopes = collect_journal_v2(&journal_path(fixture), Some(&fixture.handle.run_id))
        .expect("collect v2 journal");
    envelopes
        .into_iter()
        .filter_map(|envelope| match envelope.payload {
            JournalPayload::InvocationStarted {
                canonical_input: Some(input),
                ..
            } => Some(input),
            _ => None,
        })
        .collect()
}

fn v2_finished_summaries(fixture: &RunnerFixture) -> Vec<String> {
    let envelopes = collect_journal_v2(&journal_path(fixture), Some(&fixture.handle.run_id))
        .expect("collect v2 journal");
    envelopes
        .into_iter()
        .filter_map(|envelope| match envelope.payload {
            JournalPayload::InvocationFinished { outcome, .. } => Some(outcome.summary),
            _ => None,
        })
        .collect()
}

struct RecordingTool {
    name: &'static str,
    observed: Arc<Mutex<Option<serde_json::Value>>>,
    result: ToolResult,
}

impl Tool for RecordingTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        "record workflow swarm input"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        *self.observed.lock().expect("recording lock") = Some(input);
        let result = self.result.clone();
        Box::pin(async move { Ok(result) })
    }
}

#[tokio::test]
async fn workflow_rejects_unknown_host_fields() {
    let fixture = make_runner().await;
    for script in [
        r#"neo.delegate({ task = "test", mode = "background" })"#,
        r#"neo.delegate({ task = "test", prompt = "alias" })"#,
        r#"neo.swarm({ description = "test", max_concurrency = 2 })"#,
        r#"neo.verify_command({ command = "true", timeout_secs = 1 })"#,
    ] {
        let error = fixture
            .runner
            .execute(script, serde_json::json!({}))
            .await
            .expect_err("unknown host field should be rejected");
        assert!(error.to_string().contains("unknown field"), "{error}");
    }
}

#[tokio::test]
async fn semantic_validation_precedes_durable_invocation() {
    let fixture = make_runner().await;
    for script in [
        r#"neo.delegate({ task = "new", title = "   " })"#,
        r#"neo.delegate({ task = "resume", resume = "agent_123", role = "reviewer" })"#,
        r#"neo.swarm({ description = "bad", items = {{title="x", value="x"}}, prompt_template = "constant" })"#,
    ] {
        let error = fixture
            .runner
            .execute(script, serde_json::json!({}))
            .await
            .expect_err("canonical semantic validation");
        assert!(
            error.to_string().contains("invalid workflow input"),
            "{error}"
        );
    }
    assert!(
        v2_started_kinds(&fixture).is_empty(),
        "semantic failure must not journal InvocationStarted"
    );
}

#[tokio::test]
async fn workflow_args_are_recursively_read_only() {
    let fixture = make_runner().await;
    let error = fixture
        .runner
        .execute(
            r#"
            assert(#neo.args.nested.items == 1)
            local count = 0
            local iterator, state, key = pairs(neo.args.nested.items)
            assert(state == nil)
            local _, leaked_item = iterator(state, key)
            assert(leaked_item.name == "original")
            for _, item in pairs(neo.args.nested.items) do
                assert(item.name == "original")
                count = count + 1
            end
            assert(count == 1)
            leaked_item.name = "modified"
            "#,
            serde_json::json!({"nested": {"items": [{"name": "original"}]}}),
        )
        .await
        .expect_err("deep mutation should fail");

    assert!(
        error.to_string().contains("invalid_workflow_operation"),
        "{error}"
    );
}

#[tokio::test]
async fn infinite_lua_hits_instruction_resource_limit() {
    let limits = WorkflowLimits {
        pause_hook_interval: 10_000,
        max_uninterrupted_instructions: 20_000,
        ..WorkflowLimits::default()
    };
    let fixture = make_runner_with(limits, Vec::new()).await;
    let error = fixture
        .runner
        .execute("while true do end", serde_json::json!({}))
        .await
        .expect_err("infinite Lua should hit the instruction limit");

    assert!(
        matches!(
            error,
            neo_agent_core::workflow::WorkflowError::ResourceLimited(_)
        ),
        "{error}"
    );
}

#[tokio::test]
async fn lua_memory_limit_is_resource_limited() {
    let limits = WorkflowLimits {
        lua_vm_memory_bytes: 1024 * 1024,
        ..WorkflowLimits::default()
    };
    let fixture = make_runner_with(limits, Vec::new()).await;
    let error = fixture
        .runner
        .execute(
            r#"local values = {} for i = 1, 1000000 do values[i] = string.rep("x", 100) end"#,
            serde_json::json!({}),
        )
        .await
        .expect_err("Lua allocation should hit the VM memory limit");

    assert!(
        matches!(
            error,
            neo_agent_core::workflow::WorkflowError::ResourceLimited(_)
        ),
        "{error}"
    );
}

#[tokio::test]
async fn disabled_apis_are_unavailable_but_pcall_remains() {
    let fixture = make_runner().await;
    let result = fixture
        .runner
        .execute(
            r#"
            return {
                io = io == nil,
                os = os == nil,
                package = package == nil,
                require = require == nil,
                random = math.random == nil,
                randomseed = math.randomseed == nil,
                dofile = dofile == nil,
                loadfile = loadfile == nil,
                print = print == nil,
                rawset = rawset == nil,
                pcall = type(pcall) == "function",
                xpcall = type(xpcall) == "function",
                api_count = (function()
                    local allowed = {
                        phase=true, log=true, delegate=true, swarm=true,
                        verify=true, verify_command=true, report=true, fail=true,
                        tool=true, await_user=true,
                        json_array=true, json_object=true,
                    }
                    local count = 0
                    for name, value in pairs(neo) do
                        if type(value) == "function" then
                            assert(allowed[name], name)
                            count = count + 1
                        end
                    end
                    return count == 12
                end)(),
            }
            "#,
            serde_json::json!({}),
        )
        .await
        .expect("sandbox inspection");

    assert!(
        result
            .as_object()
            .unwrap()
            .values()
            .all(|value| value == true)
    );
}

#[tokio::test]
async fn neo_fail_is_terminal_even_when_pcall_catches_it() {
    let fixture = make_runner().await;
    let error = fixture
        .runner
        .execute(
            r#"
            pcall(function() neo.fail("deliberate") end)
            neo.delegate({ task = "must not dispatch" })
            "#,
            serde_json::json!({}),
        )
        .await
        .expect_err("neo.fail must remain terminal");

    assert!(
        matches!(error, neo_agent_core::workflow::WorkflowError::Failed(ref reason) if reason == "deliberate"),
        "{error}"
    );
    let kinds = v2_started_kinds(&fixture);
    assert!(
        kinds.contains(&WorkflowInvocationKind::Fail),
        "expected Fail invocation, got {kinds:?}"
    );
    assert!(
        !kinds.contains(&WorkflowInvocationKind::Delegate),
        "delegate must not run after fail: {kinds:?}"
    );

    let limits = WorkflowLimits {
        pause_hook_interval: 10_000,
        max_uninterrupted_instructions: 10_000,
        ..WorkflowLimits::default()
    };
    let fixture = make_runner_with(limits, Vec::new()).await;
    let error = fixture
        .runner
        .execute(
            r#"pcall(function() neo.fail("fatal-first") end) while true do end"#,
            serde_json::json!({}),
        )
        .await
        .expect_err("fatal must outrank instruction exhaustion");
    assert!(matches!(
        error,
        neo_agent_core::workflow::WorkflowError::Failed(ref reason) if reason == "fatal-first"
    ));
}

#[tokio::test]
async fn neo_verify_failure_is_a_catchable_outcome_table() {
    let fixture = make_runner().await;
    let result = fixture
        .runner
        .execute(
            r#"
            local ok, outcome = pcall(function()
                neo.verify(false, "should have passed")
            end)
            local top_mutable = pcall(function() outcome.status = "completed" end)
            local nested_mutable = pcall(function() outcome.details.message = "changed" end)
            return {
                caught = not ok,
                status = outcome.status,
                summary = outcome.summary,
                detail = outcome.details.message,
                immutable = not top_mutable and not nested_mutable,
            }
            "#,
            serde_json::json!({}),
        )
        .await
        .expect("verification failure should be catchable");

    assert_eq!(result["caught"], true);
    assert_eq!(result["status"], "failed");
    assert_eq!(result["summary"], "should have passed");
    assert_eq!(result["detail"], "should have passed");
    assert_eq!(result["immutable"], true);
}

#[tokio::test]
async fn local_host_operations_are_durable() {
    let fixture = make_runner_with(
        WorkflowLimits::default(),
        vec![WorkflowPhase {
            id: "build".to_owned(),
            description: "Build".to_owned(),
        }],
    )
    .await;
    fixture
        .runner
        .execute(
            r#"
            neo.phase("build")
            neo.log("started")
            neo.report({ result = "ok" })
            "#,
            serde_json::json!({}),
        )
        .await
        .expect("local host operations");

    let output = fixture.handle.output().await.expect("workflow output");
    assert_eq!(output.current_phase.as_deref(), Some("build"));
    assert_eq!(output.reports, vec![serde_json::json!({"result": "ok"})]);
    assert_eq!(
        v2_started_kinds(&fixture),
        [
            WorkflowInvocationKind::Phase,
            WorkflowInvocationKind::Log,
            WorkflowInvocationKind::Report,
        ]
    );
}

#[tokio::test]
async fn child_failure_outcome_returns_normally() {
    let observed = Arc::new(Mutex::new(None));
    let mut registry = ToolRegistry::new();
    registry.register(RecordingTool {
        name: "Delegate",
        observed: Arc::clone(&observed),
        result: ToolResult::error("child failed").with_details(serde_json::json!({
            "kind": "delegate",
            "agent_id": "agent_test",
            "status": "failed",
            "mode": "foreground",
            "actual_usage": {
                "input_tokens": 11,
                "output_tokens": 7,
                "input_cache_read_tokens": 3,
                "input_cache_write_tokens": 2
            }
        })),
    });
    let fixture = make_runner_with_registry(WorkflowLimits::default(), Vec::new(), registry).await;
    let result = fixture
        .runner
        .execute(
            r#"
            local outcome = neo.delegate({ task = "fail without crashing" })
            local top = pcall(function() outcome.agent_id = "changed" end)
            local usage = pcall(function() outcome.actual_usage.input_tokens = 0 end)
            local details = pcall(function() outcome.details.kind = "changed" end)
            return {
                ok = outcome.ok,
                status = outcome.status,
                agent_id = outcome.agent_id,
                input_tokens = outcome.actual_usage.input_tokens,
                immutable = not top and not usage and not details,
            }
            "#,
            serde_json::json!({}),
        )
        .await
        .expect("child failure is a normal host result");

    assert_eq!(result["ok"], false);
    assert_eq!(result["status"], "failed");
    assert_eq!(result["agent_id"], "agent_test");
    assert_eq!(result["input_tokens"], 11);
    assert_eq!(result["immutable"], true);
}

#[tokio::test]
async fn verify_command_failure_message_is_durable_and_script_visible() {
    let observed = Arc::new(Mutex::new(None));
    let hook_observed = Arc::clone(&observed);
    let fixture = make_runner_with_config(
        WorkflowLimits::default(),
        Vec::new(),
        ToolRegistry::with_builtin_tools(),
        move |config| {
            config.with_before_tool_call(move |call| {
                if call.name.as_ref() != "Bash" {
                    return None;
                }
                *hook_observed.lock().expect("recording lock") =
                    serde_json::from_str(&call.raw_arguments).ok();
                Some(
                    ToolResult::error("preset dispatch failure")
                        .with_details(serde_json::json!({"outcome": "resource_limited"})),
                )
            })
        },
    )
    .await;
    let result = fixture
        .runner
        .execute(
            r#"
            local ok, outcome = pcall(function()
                neo.verify_command({
                    command = "pwd",
                    cwd = ".",
                    failure_message = "custom failure"
                })
            end)
            return {
                caught = not ok,
                outcome_type = type(outcome),
                summary = outcome and outcome.summary,
            }
            "#,
            serde_json::json!({}),
        )
        .await
        .expect("catch command failure");
    assert_eq!(
        *observed.lock().expect("recording lock"),
        Some(serde_json::json!({
            "command": "pwd",
            "cwd": "."
        }))
    );
    assert_eq!(result["caught"], true, "{result}");
    assert_eq!(result["outcome_type"], "table", "{result}");
    assert_eq!(result["summary"], "custom failure", "{result}");
    let summaries = v2_finished_summaries(&fixture);
    assert!(
        summaries.iter().any(|s| s == "custom failure"),
        "expected custom failure summary, got {summaries:?}"
    );
}

#[tokio::test]
async fn swarm_concurrency_is_runtime_owned() {
    let observed = Arc::new(Mutex::new(None));
    let mut registry = ToolRegistry::new();
    registry.register(RecordingTool {
        name: "DelegateSwarm",
        observed: Arc::clone(&observed),
        result: ToolResult::error("recorded"),
    });
    let fixture = make_runner_with_registry(WorkflowLimits::default(), Vec::new(), registry).await;
    fixture
        .runner
        .execute(
            r#"return neo.swarm({ description = "one", items = {{title="x", value="x"}}, prompt_template = "do {{item}}" })"#,
            serde_json::json!({}),
        )
        .await
        .expect("swarm outcome");
    assert_eq!(
        observed.lock().expect("recording lock").as_ref().unwrap()["max_concurrency"],
        4
    );
    let inputs = v2_started_inputs(&fixture);
    assert!(
        inputs
            .iter()
            .any(|input| input.get("max_concurrency").is_none()
                && input.get("description").is_some()),
        "canonical swarm journal input must omit max_concurrency: {inputs:?}"
    );
}

#[tokio::test]
async fn pause_and_cancel_are_typed() {
    let paused = make_runner().await;
    paused
        .handle
        .pause(WorkflowActor::Human)
        .await
        .expect("pause");
    let error = paused
        .runner
        .execute("while true do end", serde_json::json!({}))
        .await
        .expect_err("paused");
    assert!(matches!(
        error,
        neo_agent_core::workflow::WorkflowError::Paused(_)
    ));

    let cancelled = make_runner().await;
    cancelled
        .handle
        .stop(WorkflowActor::Human)
        .await
        .expect("stop");
    let error = cancelled
        .runner
        .execute("while true do end", serde_json::json!({}))
        .await
        .expect_err("cancelled");
    assert!(matches!(
        error,
        neo_agent_core::workflow::WorkflowError::Cancelled(_)
    ));
}

#[tokio::test]
async fn neo_phase_rejects_unknown_id() {
    let fixture = make_runner().await;
    let error = fixture
        .runner
        .execute(r#"neo.phase("missing")"#, serde_json::json!({}))
        .await
        .expect_err("undeclared phase id should fail");

    assert!(error.to_string().contains("unknown phase id"), "{error}");
}

#[tokio::test]
async fn lua_workflow_runner_reports_lua_errors() {
    let fixture = make_runner().await;
    let error = fixture
        .runner
        .execute("error('boom')", serde_json::json!({}))
        .await
        .expect_err("script should fail");

    assert!(error.to_string().contains("boom"));
}

#[tokio::test]
async fn lua_return_conversion_preserves_empty_array_and_object_markers() {
    let fixture = make_runner().await;
    let result = fixture
        .runner
        .execute(
            r#"
            local empty_array = neo.json_array({})
            local empty_object = neo.json_object({})
            local top_mutable = pcall(function() empty_array[1] = "x" end)
            local obj_mutable = pcall(function() empty_object.a = 1 end)
            return {
                empty_array = empty_array,
                empty_object = empty_object,
                unmarked_empty = {},
                array = neo.json_array({10, 20}),
                object = neo.json_object({a = 1, b = 2}),
                markers_immutable = (not top_mutable) and (not obj_mutable),
            }
            "#,
            serde_json::json!({}),
        )
        .await
        .expect("marker conversion");

    assert_eq!(result["empty_array"], serde_json::json!([]));
    assert_eq!(result["empty_object"], serde_json::json!({}));
    assert_eq!(result["unmarked_empty"], serde_json::json!({}));
    assert_eq!(result["array"], serde_json::json!([10, 20]));
    assert_eq!(result["object"], serde_json::json!({"a": 1, "b": 2}));
    assert_eq!(result["markers_immutable"], true);

    let output = fixture.handle.output().await.expect("output");
    assert_eq!(
        output.final_result.as_ref().and_then(|r| match &r.body {
            neo_agent_core::workflow::FinalResultBody::Inline { value } => Some(value.clone()),
            _ => None,
        }),
        Some(result)
    );
}

#[tokio::test]
async fn lua_return_conversion_rejects_sparse_mixed_cyclic_and_non_finite_values() {
    let fixture = make_runner().await;

    for (label, script) in [
        (
            "sparse",
            r#"local t = {}; t[1] = "a"; t[3] = "c"; return t"#,
        ),
        ("mixed", r"return {1, a = 2}"),
        ("cyclic", r"local t = {}; t.self = t; return t"),
        ("nan", r"return (0/0)"),
        ("inf", r"return (1/0)"),
        (
            "json_array_with_object_keys",
            r"return neo.json_array({a = 1})",
        ),
        (
            "json_object_with_array_keys",
            r"return neo.json_object({1, 2})",
        ),
        ("multiple_returns", r"return 1, 2"),
    ] {
        let error = fixture
            .runner
            .execute(script, serde_json::json!({}))
            .await
            .expect_err(label);
        assert!(
            matches!(
                error,
                neo_agent_core::workflow::WorkflowError::InvalidInput(_)
            ) || error.to_string().contains("must return at most one")
                || error.to_string().contains("invalid workflow input")
                || error.to_string().contains("non-finite")
                || error.to_string().contains("cyclic")
                || error.to_string().contains("sparse")
                || error.to_string().contains("mixed")
                || error.to_string().contains("json_array")
                || error.to_string().contains("json_object"),
            "{label}: {error}"
        );
    }
}

#[tokio::test]
async fn workflow_host_denies_model_supplied_limits() {
    let fixture = make_runner().await;
    for script in [
        r#"neo.swarm({ description = "x", items = {{title="a", value="b"}}, prompt_template = "do {{item}}", max_concurrency = 99 })"#,
        r#"neo.delegate({ task = "t", token_cap = 100 })"#,
        r#"neo.delegate({ task = "t", timeout_secs = 5 })"#,
        r#"neo.delegate({ task = "t", max_active_vms = 2 })"#,
        r#"neo.verify_command({ command = "true", timeout_secs = 1 })"#,
        r#"neo.swarm({ description = "x", items = {{title="a", value="b"}}, prompt_template = "do {{item}}", token_cap = 1 })"#,
    ] {
        let error = fixture
            .runner
            .execute(script, serde_json::json!({}))
            .await
            .expect_err("model-supplied limit must be rejected");
        assert!(
            error.to_string().contains("unknown field")
                || error.to_string().contains("invalid workflow input"),
            "script={script} error={error}"
        );
    }

    // Host-owned concurrency is still applied; journal input omits the field.
    let observed = std::sync::Arc::new(std::sync::Mutex::new(None));
    let mut registry = ToolRegistry::new();
    registry.register(RecordingTool {
        name: "DelegateSwarm",
        observed: std::sync::Arc::clone(&observed),
        result: ToolResult::error("recorded"),
    });
    let fixture = make_runner_with_registry(WorkflowLimits::default(), Vec::new(), registry).await;
    fixture
        .runner
        .execute(
            r#"return neo.swarm({ description = "one", items = {{title="x", value="x"}}, prompt_template = "do {{item}}" })"#,
            serde_json::json!({}),
        )
        .await
        .expect("swarm without model concurrency");
    assert_eq!(
        observed.lock().expect("recording lock").as_ref().unwrap()["max_concurrency"],
        4
    );
}
