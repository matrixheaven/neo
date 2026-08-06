use super::*;
use crate::harness::fake_model;
use serde_json::json;
use std::sync::Arc;

fn workflow_prepared(arguments: serde_json::Value) -> (AgentToolCall, PreparedToolCall) {
    let call = AgentToolCall {
        id: "call-workflow".into(),
        name: "Workflow".into(),
        raw_arguments: arguments.to_string().into(),
    };
    let action = crate::tools::workflow::prepare_action(&arguments)
        .expect("valid workflow permission test input");
    let prepared = PreparedToolCall {
        id: call.id.to_string(),
        name: call.name.to_string(),
        raw_arguments: call.raw_arguments.to_string(),
        arguments,
        warning: None,
        approval: None,
        execution: PreparedExecution::Workflow(Arc::new(action)),
    };
    (call, prepared)
}

fn workflow_config(mode: PermissionMode) -> AgentConfig {
    AgentConfig::for_model(fake_model()).with_permission_mode(mode)
}

fn inline_run_input() -> serde_json::Value {
    json!({
        "action": "run_inline",
        "name": "perm-test",
        "description": "Permission routing test",
        "phases": [{"id": "work", "description": "Do the work"}],
        "script": "neo.phase('work')\nreturn {}",
        "input_schema": {"type": "object"},
        "output_schema": {"type": "object"}
    })
}

fn save_input() -> serde_json::Value {
    let mut input = inline_run_input();
    input["action"] = json!("save");
    input["scope"] = json!("user");
    input
}

#[test]
fn workflow_read_and_validate_actions_run_directly_in_ask_and_plan_mode() {
    for arguments in [
        json!({"action": "list"}),
        json!({"action": "show", "name": "perm-test"}),
        json!({"action": "validate_saved", "name": "perm-test"}),
        {
            let mut input = inline_run_input();
            input["action"] = json!("validate_inline");
            input
        },
    ] {
        for mode in [
            PermissionMode::Ask,
            PermissionMode::Auto,
            PermissionMode::Yolo,
        ] {
            let config = workflow_config(mode);
            let (call, prepared) = workflow_prepared(arguments.clone());
            let preparation = permission_preparation_for_mode(&config, &call, &prepared);
            assert!(
                matches!(preparation, PermissionPreparation::Run(_)),
                "read/validate must run directly in {mode:?}: {arguments}"
            );
        }
        let config = workflow_config(PermissionMode::Ask);
        config
            .plan_mode
            .write()
            .expect("plan mode lock")
            .enter_in_memory();
        let (call, prepared) = workflow_prepared(arguments.clone());
        let preparation = permission_preparation_for_mode(&config, &call, &prepared);
        assert!(
            matches!(preparation, PermissionPreparation::Run(_)),
            "read/validate must run directly in plan mode: {arguments}"
        );
    }
}

#[test]
fn workflow_save_asks_with_typed_save_review_in_ask_mode() {
    let config = workflow_config(PermissionMode::Ask);
    let (call, prepared) = workflow_prepared(save_input());
    let preparation = permission_preparation_for_mode(&config, &call, &prepared);
    assert!(
        matches!(
            preparation,
            PermissionPreparation::Ask {
                operation: PermissionOperation::WorkflowSave,
                ..
            }
        ),
        "save must open the typed save review in Ask mode"
    );
}

#[test]
fn workflow_run_asks_with_typed_launch_review_in_ask_mode() {
    let config = workflow_config(PermissionMode::Ask);
    let (call, prepared) = workflow_prepared(inline_run_input());
    let preparation = permission_preparation_for_mode(&config, &call, &prepared);
    assert!(
        matches!(
            preparation,
            PermissionPreparation::Ask {
                operation: PermissionOperation::WorkflowLaunch,
                ..
            }
        ),
        "run must open the typed launch review in Ask mode"
    );
}

#[test]
fn workflow_save_and_run_execute_directly_in_auto_and_yolo() {
    for mode in [PermissionMode::Auto, PermissionMode::Yolo] {
        for arguments in [save_input(), inline_run_input()] {
            let config = workflow_config(mode);
            let (call, prepared) = workflow_prepared(arguments.clone());
            let preparation = permission_preparation_for_mode(&config, &call, &prepared);
            assert!(
                matches!(preparation, PermissionPreparation::Run(_)),
                "save/run must execute directly in {mode:?}: {arguments}"
            );
        }
    }
}

#[test]
fn workflow_save_and_run_are_denied_in_plan_mode() {
    for arguments in [save_input(), inline_run_input()] {
        let config = workflow_config(PermissionMode::Ask);
        config
            .plan_mode
            .write()
            .expect("plan mode lock")
            .enter_in_memory();
        let (call, prepared) = workflow_prepared(arguments.clone());
        let preparation = permission_preparation_for_mode(&config, &call, &prepared);
        assert!(
            matches!(preparation, PermissionPreparation::Deny(_)),
            "save/run must be denied in plan mode: {arguments}"
        );
    }
}
