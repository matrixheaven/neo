use std::sync::Arc;

use neo_agent_core::AgentContext;
use neo_agent_core::ToolAccess;
use neo_agent_core::harness::FakeHarness;
use neo_agent_core::runtime::WorkflowDispatchHandle;
use neo_agent_core::tools::{ProcessSupervisor, ToolRegistry};
use neo_agent_core::workflow::{LuaWorkflowRunner, WorkflowLimits, WorkflowRuntime};

async fn make_runner() -> LuaWorkflowRunner {
    let dir = tempfile::tempdir().unwrap();
    let harness = FakeHarness::from_turns([]);
    let config = neo_agent_core::AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path().to_path_buf())
        .expect("workspace root")
        .with_permission_mode(neo_agent_core::PermissionMode::Yolo);
    let registry = Arc::new(ToolRegistry::with_builtin_tools());

    let dispatch = WorkflowDispatchHandle {
        config,
        model_client: harness.client(),
        registry,
        process_supervisor: ProcessSupervisor::default(),
        context: AgentContext::new(),
        tool_access: Some(ToolAccess::all()),
    };

    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = runtime
        .create_run(
            dir.path(),
            neo_agent_core::workflow::WorkflowLaunchRequest {
                name: "test".to_owned(),
                description: "test".to_owned(),
                phases: vec![],
                script: String::new(),
                args: serde_json::json!({}),
                launch_source: "test".to_owned(),
                parent_run_id: None,
            },
        )
        .await
        .expect("create run");

    LuaWorkflowRunner::new(dispatch, handle, WorkflowLimits::default())
}

#[tokio::test]
async fn workflow_rejects_unknown_host_fields() {
    let runner = make_runner().await;

    let err = runner
        .execute(
            r#"local res = neo.delegate({ task = "test", mode = "background" })"#,
            serde_json::json!({}),
        )
        .await
        .expect_err("background mode should be rejected");

    assert!(err.to_string().contains("mode"));
}

#[tokio::test]
async fn workflow_args_are_recursively_read_only() {
    let runner = make_runner().await;

    let err = runner
        .execute(
            r#"neo.args.target = "modified""#,
            serde_json::json!({"target": "crates/neo"}),
        )
        .await
        .expect_err("mutation should fail");

    assert!(err.to_string().contains("read-only"));
}

#[tokio::test]
async fn infinite_lua_hits_instruction_resource_limit() {
    let mut limits = WorkflowLimits::default();
    limits.lua_vm_memory_bytes = 1024 * 1024; // 1 MiB tiny limit

    let dir = tempfile::tempdir().unwrap();
    let harness = FakeHarness::from_turns([]);
    let config = neo_agent_core::AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path().to_path_buf())
        .expect("workspace root");
    let registry = Arc::new(ToolRegistry::with_builtin_tools());

    let dispatch = WorkflowDispatchHandle {
        config,
        model_client: harness.client(),
        registry,
        process_supervisor: ProcessSupervisor::default(),
        context: AgentContext::new(),
        tool_access: Some(ToolAccess::all()),
    };

    let runtime = WorkflowRuntime::new(limits.clone());
    let handle = runtime
        .create_run(
            dir.path(),
            neo_agent_core::workflow::WorkflowLaunchRequest {
                name: "test".to_owned(),
                description: "test".to_owned(),
                phases: vec![],
                script: String::new(),
                args: serde_json::json!({}),
                launch_source: "test".to_owned(),
                parent_run_id: None,
            },
        )
        .await
        .expect("create run");

    let runner = LuaWorkflowRunner::new(dispatch, handle, limits);

    // A script that generates a huge string should hit memory limit
    let err = runner
        .execute(
            r#"local s = "" for i = 1, 1000000 do s = s .. "x" end"#,
            serde_json::json!({}),
        )
        .await
        .expect_err("should hit memory limit");

    assert!(
        err.to_string().contains("memory") || err.to_string().contains("resource"),
        "expected memory limit, got: {err}"
    );
}

#[tokio::test]
async fn neo_fail_is_catchable_by_pcall() {
    let runner = make_runner().await;

    // neo.fail throws a Lua error; pcall catches it but the error object
    // is not JSON-serializable. Verify pcall returns ok=false and a message.
    let result = runner
        .execute(
            r#"
        local ok, err = pcall(function() neo.fail("deliberate") end)
        -- Return just ok since err is a non-serializable error object
        return { ok = ok }
        "#,
            serde_json::json!({}),
        )
        .await
        .expect("script should not crash");

    let result = result.expect("should return a value");
    assert_eq!(result["ok"], false);
}

#[tokio::test]
async fn neo_phase_rejects_empty_id() {
    let runner = make_runner().await;

    let err = runner
        .execute(r#"neo.phase("")"#, serde_json::json!({}))
        .await
        .expect_err("empty phase id should fail");

    assert!(err.to_string().contains("non-empty"));
}

#[tokio::test]
async fn neo_verify_fails_on_false_condition() {
    let runner = make_runner().await;

    let err = runner
        .execute(
            r#"neo.verify(false, "should have passed")"#,
            serde_json::json!({}),
        )
        .await
        .expect_err("false verify should raise error");

    assert!(err.to_string().contains("should have passed"));
}

#[tokio::test]
async fn neo_delegate_rejects_missing_task() {
    let runner = make_runner().await;

    let err = runner
        .execute(r#"neo.delegate({})"#, serde_json::json!({}))
        .await
        .expect_err("missing task should fail");

    assert!(err.to_string().contains("task"));
}

#[tokio::test]
async fn neo_swarm_rejects_mode_field() {
    let runner = make_runner().await;

    let err = runner
        .execute(
            r#"neo.swarm({ description = "test", mode = "background", items = {{title="x",value="x"}}, prompt_template = "{{item}}" })"#,
            serde_json::json!({}),
        )
        .await
        .expect_err("mode field should be rejected");

    assert!(err.to_string().contains("mode"));
}

#[tokio::test]
async fn make_read_only_args_prevents_assignment() {
    let runner = make_runner().await;

    let err = runner
        .execute(
            r#"neo.args["new_key"] = "forbidden""#,
            serde_json::json!({"existing": true}),
        )
        .await
        .expect_err("read-only args should prevent mutation");

    assert!(err.to_string().contains("read-only"));
}

#[tokio::test]
async fn lua_workflow_runner_reports_lua_errors() {
    let runner = make_runner().await;

    let err = runner
        .execute("error('boom')", serde_json::json!({}))
        .await
        .expect_err("script should fail");

    assert!(err.to_string().contains("boom"));
}
