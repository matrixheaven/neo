use std::sync::Arc;

use neo_agent_core::harness::FakeHarness;
use neo_agent_core::tools::{Tool, ToolContext, ToolRegistry};
use neo_agent_core::workflow::{LuaWorkflowRunner, WorkflowState};
use neo_agent_core::{AgentConfig, PermissionMode, ToolAccess, ToolExecutionMode};
use neo_ai::{AiError, AiStreamEvent, StopReason};
use serde_json::json;

#[test]
fn lua_workflow_runner_executes_basic_script() {
    let runner = LuaWorkflowRunner::default();

    runner
        .run_script("local x = 1 + 1")
        .expect("script should run");
}

#[test]
fn lua_workflow_runner_reports_lua_errors() {
    let runner = LuaWorkflowRunner::default();

    let err = runner
        .run_script("error('boom')")
        .expect_err("script should fail");

    assert!(err.to_string().contains("boom"));
}

#[test]
fn lua_workflow_exposes_neo_report() {
    let runner = LuaWorkflowRunner::default();

    runner
        .run_script("neo.report({ ok = true })")
        .expect("report should run");
}

#[test]
fn lua_workflow_exposes_neo_fail() {
    let runner = LuaWorkflowRunner::default();

    let err = runner
        .run_script("neo.fail('not good')")
        .expect_err("fail should error");

    assert!(err.to_string().contains("not good"));
}

#[test]
fn lua_workflow_runner_does_not_stub_runtime_host_apis() {
    let runner = LuaWorkflowRunner::default();
    let err = runner
        .run_script(
            r#"
            local audit = neo.swarm({ description = "audit", items = {"a"}, prompt_template = "{{item}}" })
            assert(audit:has_failures() == false)
            local fix = neo.delegate({ task = "fix issue" })
            neo.verify(true, "cargo nextest run passes")
            neo.report({ audit = audit:summary(), fix = fix:summary() })
            "#,
        )
        .expect_err("recorder-only runner should not expose runtime APIs");

    assert!(err.to_string().contains("swarm"));
}

#[tokio::test]
async fn run_workflow_tool_executes_lua() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path()).unwrap();
    let tool = neo_agent_core::tools::RunWorkflowTool;

    let result = tool
        .execute(
            &ctx,
            json!({
                "title": "test workflow",
                "script": "neo.report({ status = 'ok' })"
            }),
        )
        .await
        .expect("execute should succeed");

    assert!(!result.is_error);
    assert!(result.content.contains("completed"));
    assert!(result.content.contains("steps: 1"));
}

#[tokio::test]
async fn run_workflow_returns_reports_and_top_level_result() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path()).unwrap();
    let tool = neo_agent_core::tools::RunWorkflowTool;

    let result = tool
        .execute(
            &ctx,
            json!({
                "title": "reporting workflow",
                "script": r#"
                    neo.report("first report")
                    neo.report({ second = "report" })
                    return { answer = 42 }
                "#
            }),
        )
        .await
        .expect("execute should succeed");

    assert!(!result.is_error, "{}", result.content);
    assert!(result.content.contains("reports:"), "{}", result.content);
    assert!(
        result.content.contains("first report"),
        "{}",
        result.content
    );
    assert!(result.content.contains("result:"), "{}", result.content);
    let details = result.details.as_ref().expect("workflow details");
    assert_eq!(details["reports"][0]["value"], "first report");
    assert_eq!(details["reports"][1]["value"]["second"], "report");
    assert_eq!(details["result"]["answer"], 42);
}

#[tokio::test]
async fn run_workflow_tool_reports_failure() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path()).unwrap();
    let tool = neo_agent_core::tools::RunWorkflowTool;

    let result = tool
        .execute(
            &ctx,
            json!({
                "title": "failing workflow",
                "script": "neo.fail('deliberate failure')"
            }),
        )
        .await
        .expect("execute should succeed");

    assert!(result.is_error);
    assert!(result.content.contains("failed"));
    assert!(result.content.contains("deliberate failure"));
}

#[tokio::test]
async fn run_workflow_delegate_handle_tostring_is_agent_id() {
    let dir = tempfile::tempdir().unwrap();
    let harness = FakeHarness::from_turns([child_text_turn("child inspected workflow")]);
    let ctx = workflow_ctx(dir.path(), &harness).with_access(ToolAccess::all());
    let tool = neo_agent_core::tools::RunWorkflowTool;

    let result = tool
        .execute(
            &ctx,
            json!({
                "title": "delegate handle workflow",
                "script": r#"
                    local child = neo.delegate({ task = "inspect workflow context" })
                    neo.report({ id = child:id(), tostring = tostring(child) })
                "#,
            }),
        )
        .await
        .expect("workflow should execute");

    assert!(!result.is_error, "{}", result.content);
    let reports = result.details.as_ref().unwrap()["reports"]
        .as_array()
        .expect("reports");
    let report = &reports[0]["value"];
    let id = report["id"].as_str().expect("id string");
    assert!(id.starts_with("agent_"), "{id}");
    assert_eq!(report["tostring"], id);
}

#[tokio::test]
async fn run_workflow_tool_registered_in_builtin_tools() {
    let specs = ToolRegistry::with_builtin_tools()
        .specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();

    assert!(specs.iter().any(|name| name == "RunWorkflow"));
}

#[tokio::test]
async fn run_workflow_delegate_runs_child_model_turn_and_returns_summary() {
    let dir = tempfile::tempdir().unwrap();
    let harness = FakeHarness::from_turns([child_text_turn("child inspected workflow")]);
    let ctx = workflow_ctx(dir.path(), &harness).with_access(ToolAccess::all());
    let tool = neo_agent_core::tools::RunWorkflowTool;

    let result = tool
        .execute(
            &ctx,
            json!({
                "title": "delegate workflow",
                "script": r#"
                    local child = neo.delegate({
                        task = "inspect workflow context",
                        role = "reviewer",
                        context = "inherit"
                    })
                    neo.report({ summary = child:summary() })
                "#,
            }),
        )
        .await
        .expect("workflow should execute");

    assert!(!result.is_error, "{}", result.content);
    assert_eq!(
        harness.requests().len(),
        1,
        "delegate should consume one child model turn"
    );
    assert!(result.content.contains("completed"));
    assert!(result.content.contains("steps: 2"));
    assert!(!result.content.contains("Foreground delegate completed."));

    let details = result.details.as_ref().expect("workflow details");
    let steps = details["steps"].as_array().expect("steps array");
    assert_eq!(steps[0]["state"], json!(WorkflowState::Completed));
    assert_eq!(steps[0]["summary"], "child inspected workflow");
    assert_eq!(steps[0]["details"]["summary"], "child inspected workflow");
    assert_eq!(steps[0]["details"]["role"], "reviewer");
}

#[tokio::test]
async fn run_workflow_verify_command_uses_bash_tool_success_and_failure() {
    let dir = tempfile::tempdir().unwrap();
    let harness = FakeHarness::from_turns([]);
    let ctx = workflow_ctx(dir.path(), &harness).with_access(ToolAccess::all());
    let tool = neo_agent_core::tools::RunWorkflowTool;

    let success = tool
        .execute(
            &ctx,
            json!({
                "title": "verify success",
                "script": r#"
                    local ok = neo.verify_command("printf workflow-verify-ok")
                    assert(ok == true)
                "#,
            }),
        )
        .await
        .expect("success workflow should execute");
    assert!(!success.is_error, "{}", success.content);
    let success_steps = success.details.as_ref().unwrap()["steps"]
        .as_array()
        .expect("success steps");
    assert_eq!(success_steps[0]["state"], json!(WorkflowState::Completed));
    assert_eq!(success_steps[0]["details"]["exit_code"], 0);
    assert_eq!(success_steps[0]["details"]["stdout"], "workflow-verify-ok");

    let failure = tool
        .execute(
            &ctx,
            json!({
                "title": "verify failure",
                "script": r#"
                    local ok = neo.verify_command("printf workflow-verify-fail; exit 7")
                    assert(ok == false)
                "#,
            }),
        )
        .await
        .expect("failed verify should be represented as workflow result");
    assert!(failure.is_error);
    let failure_steps = failure.details.as_ref().unwrap()["steps"]
        .as_array()
        .expect("failure steps");
    assert_eq!(failure_steps[0]["state"], json!(WorkflowState::Failed));
    assert_eq!(failure_steps[0]["details"]["exit_code"], 7);
    assert_eq!(
        failure_steps[0]["details"]["stdout"],
        "workflow-verify-fail"
    );
}

#[tokio::test]
async fn run_workflow_swarm_has_failures_reflects_child_failure() {
    let dir = tempfile::tempdir().unwrap();
    let harness = FakeHarness::from_result_turns([
        child_text_turn("alpha ok").into_iter().map(Ok).collect(),
        vec![Err(AiError::Protocol {
            message: "beta failed".to_owned(),
        })],
    ]);
    let ctx = workflow_ctx(dir.path(), &harness).with_access(ToolAccess::all());
    let tool = neo_agent_core::tools::RunWorkflowTool;

    let result = tool
        .execute(
            &ctx,
            json!({
                "title": "swarm workflow",
                "script": r#"
                    local swarm = neo.swarm({
                        description = "audit modules",
                        items = {{title="alpha",value="alpha"},{title="beta",value="beta"}},
                        prompt_template = "Audit {{item}}",
                        max_concurrency = 2
                    })
                    assert(swarm:has_failures() == true)
                    neo.report({ summary = swarm:summary() })
                "#,
            }),
        )
        .await
        .expect("workflow should execute");

    assert!(result.is_error);
    assert_eq!(harness.requests().len(), 2);
    let details = result.details.as_ref().expect("workflow details");
    let steps = details["steps"].as_array().expect("steps array");
    assert_eq!(steps[0]["state"], json!(WorkflowState::Failed));
    assert_eq!(steps[0]["has_failures"], true);
    assert_eq!(steps[0]["details"]["items"][0]["summary"], "alpha ok");
    assert_eq!(steps[0]["details"]["items"][1]["status"], "failed");
}

fn workflow_ctx(path: &std::path::Path, harness: &FakeHarness) -> ToolContext {
    let registry = Arc::new(ToolRegistry::with_builtin_tools());
    let config = AgentConfig::for_model(harness.model())
        .with_tool_execution_mode(ToolExecutionMode::Sequential)
        .with_permission_mode(PermissionMode::Yolo);
    ToolContext::new(path)
        .unwrap()
        .with_child_runtime(config, harness.client(), registry, 1)
}

fn workflow_tool_context_with_fake_model() -> (ToolContext, Arc<FakeHarness>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = Arc::new(FakeHarness::from_turns([child_text_turn(
        "child completed workflow",
    )]));
    let ctx = workflow_ctx(dir.path(), &harness).with_access(ToolAccess::all());
    (ctx, harness)
}

fn workflow_tool_context_denying_bash() -> (ToolContext, Arc<FakeHarness>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = Arc::new(FakeHarness::from_turns([]));
    let registry = Arc::new(ToolRegistry::with_builtin_tools());
    let config = AgentConfig::for_model(harness.model())
        .with_tool_execution_mode(ToolExecutionMode::Sequential)
        .with_permission_mode(PermissionMode::Yolo);
    let ctx = ToolContext::new(dir.path())
        .expect("tool context")
        .with_access(ToolAccess {
            file_read: true,
            file_write: false,
            shell: false,
            tool: true,
            user_question: false,
        })
        .with_child_runtime(config, harness.client(), registry, 1);
    (ctx, harness)
}

fn child_text_turn(text: &str) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            id: format!("msg_{text}"),
        },
        AiStreamEvent::TextDelta {
            text: text.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]
}

#[tokio::test]
async fn workflow_can_return_delegate_handle_as_table() {
    let tool = neo_agent_core::tools::RunWorkflowTool;
    let (ctx, _model) = workflow_tool_context_with_fake_model();

    let result = tool
        .execute(
            &ctx,
            json!({
                "title": "delegate return",
                "script": "return neo.delegate({ task = 'inspect one file', mode = 'foreground' })"
            }),
        )
        .await
        .expect("workflow should run");

    assert!(!result.is_error, "{}", result.content);
    let details = result.details.as_ref().expect("details");
    let returned = details.get("result").expect("result");
    assert_eq!(
        returned.get("kind").and_then(serde_json::Value::as_str),
        Some("delegate")
    );
    assert!(
        returned
            .get("agent_id")
            .and_then(serde_json::Value::as_str)
            .is_some()
    );
    assert!(
        returned
            .get("summary")
            .and_then(serde_json::Value::as_str)
            .is_some()
    );
}

#[tokio::test]
async fn workflow_swarm_handle_exposes_items_and_serializes() {
    let tool = neo_agent_core::tools::RunWorkflowTool;
    let harness = Arc::new(FakeHarness::from_turns([
        child_text_turn("core ok"),
        child_text_turn("tui ok"),
    ]));
    let dir = tempfile::tempdir().unwrap();
    let ctx = workflow_ctx(dir.path(), &harness).with_access(ToolAccess::all());

    let result = tool
        .execute(
            &ctx,
            json!({
                "title": "swarm return",
                "script": r#"
                    local s = neo.swarm({
                        description = "audit",
                        items = { {title="core",value="core"}, {title="tui",value="tui"} },
                        prompt_template = "Audit {{item}}",
                        mode = "foreground"
                    })
                    local items = s:items()
                    return { id = s:id(), status = s:status(), summary = s:summary(), items = items, table = s:to_table() }
                "#
            }),
        )
        .await
        .expect("workflow should run");

    assert!(!result.is_error, "{}", result.content);
    let returned = result.details.as_ref().unwrap().get("result").unwrap();
    assert_eq!(
        returned
            .get("items")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        returned.get("status").and_then(serde_json::Value::as_str),
        Some("completed")
    );
    assert_eq!(
        returned
            .pointer("/table/kind")
            .and_then(serde_json::Value::as_str),
        Some("swarm")
    );
}

#[tokio::test]
async fn workflow_verify_is_boolean_assertion() {
    let tool = neo_agent_core::tools::RunWorkflowTool;
    let (ctx, _model) = workflow_tool_context_with_fake_model();

    let ok = tool
        .execute(
            &ctx,
            json!({
                "title": "verify assertion ok",
                "script": "neo.verify(true, 'should pass'); return 'ok'"
            }),
        )
        .await
        .expect("workflow should run");
    assert!(!ok.is_error, "{}", ok.content);

    let failed = tool
        .execute(
            &ctx,
            json!({
                "title": "verify assertion fail",
                "script": "neo.verify(false, 'expected three completed children')"
            }),
        )
        .await
        .expect("workflow returns failure result");
    assert!(failed.is_error);
    assert!(
        failed.content.contains("expected three completed children"),
        "{}",
        failed.content
    );
}

#[tokio::test]
async fn workflow_verify_command_reports_bash_permission_denial_clearly() {
    let tool = neo_agent_core::tools::RunWorkflowTool;
    let (ctx, _model) = workflow_tool_context_denying_bash();

    let result = tool
        .execute(
            &ctx,
            json!({
                "title": "verify command denied",
                "script": "return neo.verify_command('printf denied', 'verify failed')"
            }),
        )
        .await
        .expect("workflow returns failure result");

    assert!(result.is_error);
    assert!(
        result
            .content
            .contains("verify_command denied by Bash permission policy"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn run_workflow_output_includes_report_values() {
    let tool = neo_agent_core::tools::RunWorkflowTool;
    let (ctx, _model) = workflow_tool_context_with_fake_model();

    let result = tool
        .execute(
            &ctx,
            json!({
                "title": "reports",
                "script": r#"
                    neo.report("first report")
                    neo.report({ completed = 3 })
                    return "done"
                "#
            }),
        )
        .await
        .expect("workflow should run");

    assert!(!result.is_error, "{}", result.content);
    assert!(
        result.content.contains("first report"),
        "{}",
        result.content
    );
    assert!(result.content.contains("completed"), "{}", result.content);
    let reports = result
        .details
        .as_ref()
        .and_then(|details| details.get("reports"))
        .and_then(serde_json::Value::as_array)
        .expect("reports array");
    assert_eq!(reports.len(), 2);
}

#[tokio::test]
async fn workflow_lua_errors_do_not_expose_rust_source_paths() {
    let tool = neo_agent_core::tools::RunWorkflowTool;
    let (ctx, _model) = workflow_tool_context_with_fake_model();

    let result = tool
        .execute(
            &ctx,
            json!({
                "title": "bad lua",
                "script": "error('plain workflow failure')"
            }),
        )
        .await
        .expect("workflow returns failure result");

    assert!(result.is_error);
    assert!(
        result.content.contains("plain workflow failure"),
        "{}",
        result.content
    );
    assert!(
        !result.content.contains("crates/neo-agent-core/src"),
        "{}",
        result.content
    );
}
