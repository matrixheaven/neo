use std::collections::BTreeSet;

use super::*;
use crate::ToolRegistry;
use crate::workflow::{
    WorkflowDefinitionRegistryConfig, WorkflowLimits, WorkflowSourceOrigin, source_sha256_hex,
};

fn inline_input(action: &str) -> Value {
    json!({
        "action": action,
        "name": "adapter-test",
        "description": "Exercise the unified Workflow adapter.",
        "phases": [{"id": "work", "description": "Run the workflow"}],
        "script": "return { ok = true }",
        "input_schema": {"type": "object"},
        "output_schema": {"type": "object"}
    })
}

fn test_context(
    workspace: &tempfile::TempDir,
    neo_home: &tempfile::TempDir,
    session: &tempfile::TempDir,
) -> ToolContext {
    let runtime = crate::workflow::WorkflowRuntime::default();
    runtime
        .bind_runner(|_handle, _metadata, _session_dir| async move { Ok(()) })
        .expect("bind runner");
    let registry = WorkflowDefinitionRegistry::new(WorkflowDefinitionRegistryConfig {
        neo_home: neo_home.path().to_path_buf(),
        workspace: workspace.path().to_path_buf(),
        project_trusted: true,
        limits: WorkflowLimits::default(),
        builtins: Vec::new(),
    });
    ToolContext::new(workspace.path())
        .expect("context")
        .with_workflow_runtime(runtime)
        .with_workflow_definitions(registry)
        .with_agent_session_context(session.path(), "main")
}

#[test]
fn workflow_schema_is_flat_and_exact() {
    let schema = WorkflowTool.input_schema();
    let properties = schema["properties"]
        .as_object()
        .expect("workflow schema properties");
    let names = properties
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();

    assert_eq!(
        names,
        BTreeSet::from([
            "action",
            "args",
            "cursor",
            "description",
            "input_schema",
            "limit",
            "name",
            "output_schema",
            "phases",
            "replace",
            "scope",
            "script",
        ])
    );
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["required"], json!(["action"]));
    for field in ["args", "input_schema", "output_schema"] {
        assert_eq!(
            schema["properties"][field]["type"], "object",
            "{field} must be model-visible as an object"
        );
    }

    let error = prepare_action(&json!({"action": "list", "limits": {"token_cap": 1}}))
        .expect_err("model-supplied limits were accepted");
    assert!(
        error.message.contains("unknown field `limits`"),
        "{error:?}"
    );
}

#[test]
fn action_matrix_rejects_saved_inline_mixtures_without_side_effects() {
    let mut input = inline_input("run_saved");
    input["args"] = json!({});

    let error = prepare_action(&input).expect_err("saved/inline mixture was accepted");
    assert_eq!(error.action, Some(WorkflowAction::RunSaved));
    assert_eq!(error.field, Some("description"));

    let result = input_error_result(error);
    let details = result.details.expect("structured details");
    assert_eq!(details["ok"], false);
    assert_eq!(details["action"], "run_saved");
    assert_eq!(details["error"]["code"], "workflow_input_invalid");
    assert_eq!(details["error"]["side_effect_occurred"], false);
    assert!(details["error"]["expected_shape"].is_object());
}

#[test]
fn all_actions_accept_their_canonical_shapes_and_treat_null_as_absent() {
    let mut save = inline_input("save");
    save["scope"] = json!("user");
    let cases = [
        json!({"action": "list"}),
        json!({"action": "show", "name": "adapter-test"}),
        inline_input("validate_inline"),
        json!({"action": "validate_saved", "name": "adapter-test"}),
        save,
        inline_input("run_inline"),
        json!({"action": "run_saved", "name": "adapter-test", "args": null}),
    ];

    for input in cases {
        let action = input["action"].as_str().expect("action");
        let prepared = prepare_action(&input)
            .unwrap_or_else(|error| panic!("canonical {action} shape was rejected: {error:?}"));
        assert_eq!(prepared.action().as_str(), action);
    }

    let error = prepare_action(&json!({"action": "show", "name": null}))
        .expect_err("null required field was accepted");
    assert_eq!(error.field, Some("name"));
    assert_eq!(
        expected_shape(error.action)["required"],
        json!(["action", "name"])
    );
}

#[tokio::test]
async fn validate_inline_has_no_run_task_or_definition_side_effect() {
    let workspace = tempfile::tempdir().expect("workspace");
    let neo_home = tempfile::tempdir().expect("neo home");
    let session = tempfile::tempdir().expect("session");
    let ctx = test_context(&workspace, &neo_home, &session);

    let result = WorkflowTool
        .execute(&ctx, inline_input("validate_inline"))
        .await
        .expect("execute");

    assert!(!result.is_error, "{}", result.content);
    let details = result.details.expect("structured details");
    assert_eq!(details["ok"], true);
    assert_eq!(details["action"], "validate_inline");
    assert_eq!(details["status"], "valid");
    assert_eq!(details["validation"]["valid"], true);
    assert!(
        ctx.background_tasks.list(false, 100).await.is_empty(),
        "validation registered a task"
    );
    assert!(
        !WorkflowDefinitionRegistry::user_workflows_dir(neo_home.path()).exists(),
        "validation wrote a saved definition"
    );
}

#[tokio::test]
async fn save_uses_registry_hash_and_writes_resolvable_pair_without_launching() {
    let workspace = tempfile::tempdir().expect("workspace");
    let neo_home = tempfile::tempdir().expect("neo home");
    let session = tempfile::tempdir().expect("session");
    let ctx = test_context(&workspace, &neo_home, &session);
    let mut input = inline_input("save");
    input["scope"] = json!("user");

    let result = WorkflowTool
        .execute(&ctx, input)
        .await
        .expect("execute save");

    assert!(!result.is_error, "{}", result.content);
    let resolved = ctx
        .workflow_definitions
        .resolve("adapter-test")
        .expect("resolve saved definition");
    assert_eq!(resolved.source_origin, WorkflowSourceOrigin::User);
    assert_eq!(
        resolved.source_sha256,
        source_sha256_hex(b"return { ok = true }")
    );
    assert!(resolved.source_locator.is_some());
    assert!(
        ctx.background_tasks.list(false, 100).await.is_empty(),
        "save launched a task"
    );
    let details = result.details.expect("structured details");
    assert_eq!(details["action"], "save");
    assert_eq!(details["status"], "saved");
    assert_eq!(details["workflow"]["source_sha256"], resolved.source_sha256);
}

#[tokio::test]
async fn save_rejects_invalid_lua_before_writing() {
    let workspace = tempfile::tempdir().expect("workspace");
    let neo_home = tempfile::tempdir().expect("neo home");
    let session = tempfile::tempdir().expect("session");
    let ctx = test_context(&workspace, &neo_home, &session);
    let mut input = inline_input("save");
    input["scope"] = json!("user");
    input["script"] = json!("return function(");

    let result = WorkflowTool
        .execute(&ctx, input)
        .await
        .expect("execute invalid save");

    assert!(result.is_error);
    assert_eq!(
        result.details.as_ref().expect("details")["error"]["code"],
        "workflow_definition_invalid"
    );
    assert!(ctx.background_tasks.list(false, 100).await.is_empty());
    assert!(!WorkflowDefinitionRegistry::user_workflows_dir(neo_home.path()).exists());
}

#[tokio::test]
async fn invalid_run_args_are_input_errors_without_side_effects() {
    let workspace = tempfile::tempdir().expect("workspace");
    let neo_home = tempfile::tempdir().expect("neo home");
    let session = tempfile::tempdir().expect("session");
    let ctx = test_context(&workspace, &neo_home, &session);
    let mut input = inline_input("run_inline");
    input["input_schema"] = json!({
        "type": "object",
        "required": ["target"],
        "properties": {"target": {"type": "string"}}
    });
    input["args"] = json!({});

    let result = WorkflowTool
        .execute(&ctx, input)
        .await
        .expect("execute invalid args");

    assert!(result.is_error);
    let details = result.details.expect("details");
    assert_eq!(details["error"]["code"], "workflow_input_invalid");
    assert_eq!(details["error"]["field"], "args");
    assert_eq!(details["error"]["side_effect_occurred"], false);
    assert!(ctx.background_tasks.list(false, 100).await.is_empty());
}

#[tokio::test]
async fn launch_errors_report_side_effects_from_the_coordinator_stage() {
    let workspace = tempfile::tempdir().expect("workspace");
    let neo_home = tempfile::tempdir().expect("neo home");
    let session = tempfile::tempdir().expect("session");

    let mut limits = WorkflowLimits::default();
    limits.global_storage_bytes = 1;
    let blocked_runtime = crate::workflow::WorkflowRuntime::new(limits);
    let blocked_registry = WorkflowDefinitionRegistry::new(WorkflowDefinitionRegistryConfig {
        neo_home: neo_home.path().to_path_buf(),
        workspace: workspace.path().to_path_buf(),
        project_trusted: true,
        limits: blocked_runtime.limits(),
        builtins: Vec::new(),
    });
    let blocked = ToolContext::new(workspace.path())
        .expect("blocked context")
        .with_workflow_runtime(blocked_runtime)
        .with_workflow_definitions(blocked_registry)
        .with_agent_session_context(session.path(), "main");
    let before_create = WorkflowTool
        .execute(&blocked, inline_input("run_inline"))
        .await
        .expect("blocked launch")
        .details
        .expect("blocked details");
    assert_eq!(before_create["error"]["side_effect_occurred"], false);

    let unbound_runtime = crate::workflow::WorkflowRuntime::default();
    let unbound_registry = WorkflowDefinitionRegistry::new(WorkflowDefinitionRegistryConfig {
        neo_home: neo_home.path().to_path_buf(),
        workspace: workspace.path().to_path_buf(),
        project_trusted: true,
        limits: unbound_runtime.limits(),
        builtins: Vec::new(),
    });
    let unbound = ToolContext::new(workspace.path())
        .expect("unbound context")
        .with_workflow_runtime(unbound_runtime)
        .with_workflow_definitions(unbound_registry)
        .with_agent_session_context(session.path(), "main");
    let after_create = WorkflowTool
        .execute(&unbound, inline_input("run_inline"))
        .await
        .expect("unbound launch")
        .details
        .expect("unbound details");
    assert_eq!(after_create["error"]["side_effect_occurred"], true);
}

#[test]
fn non_save_conflicts_preserve_registry_error_without_replace_recovery() {
    let result = workflow_error_result(
        WorkflowAction::Show,
        WorkflowError::coded(
            WorkflowErrorCode::DefinitionConflict,
            "same-scope registry conflict",
        ),
    );
    let details = result.details.expect("details");

    assert_eq!(details["error"]["code"], "workflow_conflict");
    assert!(
        details["error"]["message"]
            .as_str()
            .expect("message")
            .contains("same-scope registry conflict")
    );
    assert_eq!(details["next_actions"], json!([]));
    assert!(!details.to_string().contains("replace"));
}

#[test]
fn only_partial_save_errors_report_a_save_side_effect() {
    let partial = workflow_save_error_result(
        WorkflowError::coded(
            WorkflowErrorCode::DefinitionSavePartial,
            "source saved; manifest failed",
        ),
        None,
    )
    .details
    .expect("partial details");
    assert_eq!(partial["error"]["side_effect_occurred"], true);

    let host = workflow_save_error_result(
        WorkflowError::coded(WorkflowErrorCode::Host, "save failed before commit"),
        None,
    )
    .details
    .expect("host details");
    assert_eq!(host["error"]["side_effect_occurred"], false);
}

#[tokio::test]
async fn saved_actions_list_show_validate_run_and_recover_from_conflict() {
    let workspace = tempfile::tempdir().expect("workspace");
    let neo_home = tempfile::tempdir().expect("neo home");
    let session = tempfile::tempdir().expect("session");
    let ctx = test_context(&workspace, &neo_home, &session);
    let mut save = inline_input("save");
    save["scope"] = json!("user");
    WorkflowTool
        .execute(&ctx, save.clone())
        .await
        .expect("save")
        .details
        .expect("save details");

    let mut project_save = save.clone();
    project_save["scope"] = json!("project");
    project_save["input_schema"] = json!({
        "type": "object",
        "required": ["project_only"],
        "properties": {"project_only": {"type": "boolean"}}
    });
    WorkflowTool
        .execute(&ctx, project_save)
        .await
        .expect("project save")
        .details
        .expect("project save details");

    let listed = WorkflowTool
        .execute(&ctx, json!({"action": "list", "scope": "user"}))
        .await
        .expect("list")
        .details
        .expect("list details");
    assert_eq!(listed["items"]["entries"][0]["name"], "adapter-test");
    assert_eq!(
        listed["items"]["entries"][0]["schema"]["input"]["property_count"],
        0
    );
    let project_listed = WorkflowTool
        .execute(&ctx, json!({"action": "list", "scope": "project"}))
        .await
        .expect("project list")
        .details
        .expect("project list details");
    assert_eq!(
        project_listed["items"]["entries"][0]["schema"]["input"]["property_count"],
        1
    );
    assert_eq!(
        project_listed["items"]["entries"][0]["schema"]["input"]["required"],
        json!(["project_only"])
    );

    let shown = WorkflowTool
        .execute(&ctx, json!({"action": "show", "name": "adapter-test"}))
        .await
        .expect("show")
        .details
        .expect("show details");
    assert_eq!(shown["workflow"]["script"], "return { ok = true }");

    let validated = WorkflowTool
        .execute(
            &ctx,
            json!({"action": "validate_saved", "name": "adapter-test"}),
        )
        .await
        .expect("validate saved")
        .details
        .expect("validate details");
    assert_eq!(validated["validation"]["valid"], true);

    let mut conflicting = save;
    conflicting["script"] = json!("return { ok = false }");
    let conflict = WorkflowTool
        .execute(&ctx, conflicting)
        .await
        .expect("conflicting save");
    assert!(conflict.is_error);
    let conflict = conflict.details.expect("conflict details");
    assert_eq!(conflict["error"]["code"], "workflow_conflict");
    assert_eq!(conflict["next_actions"][0]["arguments"]["replace"], true);
    assert!(
        conflict["error"]["message"]
            .as_str()
            .unwrap()
            .contains("replace")
    );
    prepare_action(&conflict["next_actions"][0]["arguments"])
        .expect("conflict recovery action must be directly executable");

    let running = WorkflowTool
        .execute(
            &ctx,
            json!({
                "action": "run_saved",
                "name": "adapter-test",
                "args": {"project_only": true}
            }),
        )
        .await
        .expect("run saved")
        .details
        .expect("run details");
    let task_id = running["task"]["task_id"].as_str().expect("task id");
    assert_eq!(running["next_actions"][0]["tool"], "TaskOutput");
    assert!(
        ctx.background_tasks
            .workflow_handle(task_id)
            .await
            .is_some()
    );
}

#[tokio::test]
async fn run_inline_returns_registered_task_and_task_output_next_action() {
    let workspace = tempfile::tempdir().expect("workspace");
    let neo_home = tempfile::tempdir().expect("neo home");
    let session = tempfile::tempdir().expect("session");
    let ctx = test_context(&workspace, &neo_home, &session);

    let result = WorkflowTool
        .execute(&ctx, inline_input("run_inline"))
        .await
        .expect("execute run");

    assert!(!result.is_error, "{}", result.content);
    let details = result.details.expect("structured details");
    let task_id = details["task"]["task_id"].as_str().expect("task id");
    assert_eq!(details["ok"], true);
    assert_eq!(details["action"], "run_inline");
    assert_eq!(details["status"], "running");
    assert_eq!(details["next_actions"][0]["tool"], "TaskOutput");
    assert_eq!(details["next_actions"][0]["arguments"]["task_id"], task_id);
    assert!(
        ctx.background_tasks
            .workflow_handle(task_id)
            .await
            .is_some()
    );
}

#[test]
fn workflow_is_root_only() {
    assert!(ToolRegistry::with_builtin_tools().contains("Workflow"));
    assert!(!ToolRegistry::with_builtin_tools().contains("RunWorkflow"));
    assert!(!ToolRegistry::with_builtin_child_tools().contains("Workflow"));
}
