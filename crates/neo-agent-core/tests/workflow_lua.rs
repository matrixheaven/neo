use std::sync::{Arc, Mutex};

use neo_agent_core::AgentContext;
use neo_agent_core::harness::FakeHarness;
use neo_agent_core::runtime::WorkflowDispatchHandle;
use neo_agent_core::tools::{
    ProcessSupervisor, Tool, ToolContext, ToolFuture, ToolRegistry, ToolResult,
};
use neo_agent_core::workflow::{
    LuaWorkflowRunner, WorkflowActor, WorkflowHandle, WorkflowInvocationKind, WorkflowLimits,
    WorkflowPhase, WorkflowRuntime,
};

struct RunnerFixture {
    _dir: tempfile::TempDir,
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
            },
        )
        .await
        .expect("create run");
    let runner = LuaWorkflowRunner::new(dispatch, handle.clone(), limits);
    RunnerFixture {
        _dir: dir,
        runner,
        handle,
    }
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
    let output = fixture.handle.output().await.expect("workflow output");
    assert!(!output.invocations.iter().any(|record| matches!(
        record,
        neo_agent_core::workflow::JournalRecord::InvocationStarted { .. }
    )));
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
                    }
                    local count = 0
                    for name, value in pairs(neo) do
                        if type(value) == "function" then
                            assert(allowed[name])
                            count = count + 1
                        end
                    end
                    return count == 8
                end)(),
            }
            "#,
            serde_json::json!({}),
        )
        .await
        .expect("sandbox inspection")
        .expect("table result");

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
    let output = fixture.handle.output().await.expect("workflow output");
    assert!(output.invocations.iter().any(|record| matches!(
        record,
        neo_agent_core::workflow::JournalRecord::InvocationStarted {
            kind: WorkflowInvocationKind::Fail,
            ..
        }
    )));
    assert!(!output.invocations.iter().any(|record| matches!(
        record,
        neo_agent_core::workflow::JournalRecord::InvocationStarted {
            kind: WorkflowInvocationKind::Delegate,
            ..
        }
    )));

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
        .expect("verification failure should be catchable")
        .expect("table result");

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
    let started = output
        .invocations
        .iter()
        .filter_map(|record| match record {
            neo_agent_core::workflow::JournalRecord::InvocationStarted { kind, .. } => Some(*kind),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        started,
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
        .expect("child failure is a normal host result")
        .expect("outcome table");

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
        .expect("catch command failure")
        .expect("result");
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
    let output = fixture.handle.output().await.expect("workflow output");
    assert!(output.invocations.iter().any(|record| matches!(
        record,
        neo_agent_core::workflow::JournalRecord::InvocationFinished { outcome, .. }
            if outcome.summary == "custom failure"
    )));
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
    let output = fixture.handle.output().await.expect("workflow output");
    assert!(output.invocations.iter().any(|record| matches!(
        record,
        neo_agent_core::workflow::JournalRecord::InvocationStarted { canonical_input, .. }
            if canonical_input.get("max_concurrency").is_none()
    )));
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
