use super::*;
use crate::harness::fake_model;
use serde_json::json;

fn workflow_config(mode: PermissionMode) -> AgentConfig {
    AgentConfig::for_model(fake_model()).with_permission_mode(mode)
}

fn theme_draft_prepared(arguments: serde_json::Value) -> (AgentToolCall, PreparedToolCall) {
    let call = AgentToolCall {
        id: "call-theme-draft".into(),
        name: "ThemeDraft".into(),
        raw_arguments: arguments.to_string().into(),
    };
    let prepared = PreparedToolCall {
        id: call.id.to_string(),
        name: call.name.to_string(),
        raw_arguments: call.raw_arguments.to_string(),
        arguments,
        warning: None,
        approval: None,
        execution: PreparedExecution::Direct,
    };
    (call, prepared)
}

#[test]
fn theme_draft_preview_runs_directly_in_every_mode_including_plan() {
    let preview = json!({
        "action": "preview",
        "name": "Aurora Night",
        "colors": {"brand": "#58a6ff"},
    });
    for mode in [
        PermissionMode::Ask,
        PermissionMode::Auto,
        PermissionMode::Yolo,
    ] {
        let config = workflow_config(mode);
        let (call, prepared) = theme_draft_prepared(preview.clone());
        let preparation = permission_preparation_for_mode(&config, &call, &prepared);
        assert!(
            matches!(preparation, PermissionPreparation::Run(_)),
            "preview must run directly in {mode:?}"
        );
    }
    let config = workflow_config(PermissionMode::Ask);
    config
        .plan_mode
        .write()
        .expect("plan mode lock")
        .enter_in_memory();
    let (call, prepared) = theme_draft_prepared(preview.clone());
    let preparation = permission_preparation_for_mode(&config, &call, &prepared);
    assert!(
        matches!(preparation, PermissionPreparation::Run(_)),
        "preview must run directly in plan mode"
    );
}

#[test]
fn theme_draft_preview_never_grants_file_write_access() {
    let preview = json!({
        "action": "preview",
        "name": "Aurora Night",
    });
    let config = workflow_config(PermissionMode::Ask);
    let (call, prepared) = theme_draft_prepared(preview);
    let preparation = permission_preparation_for_mode(&config, &call, &prepared);
    let PermissionPreparation::Run(access) = preparation else {
        panic!("preview must run: {preparation:?}");
    };
    assert!(access.tool, "preview carries the tool access grant");
    assert!(
        !access.file_write,
        "ThemeDraft must never grant generic file_write"
    );
}

#[test]
fn theme_draft_save_asks_with_typed_theme_save_operation_and_no_session_scope() {
    let config = workflow_config(PermissionMode::Ask);
    let (call, prepared) = theme_draft_prepared(json!({
        "action": "save",
        "draft_id": "draft-0001",
        "overwrite": false,
    }));
    let preparation = permission_preparation_for_mode(&config, &call, &prepared);
    assert!(
        matches!(
            preparation,
            PermissionPreparation::Ask {
                operation: PermissionOperation::ThemeSave,
                session_scope: None,
                ..
            }
        ),
        "save must open the typed ThemeSave review with no session scope in Ask mode"
    );
}

#[test]
fn theme_draft_save_runs_directly_in_auto_and_yolo() {
    for mode in [PermissionMode::Auto, PermissionMode::Yolo] {
        let config = workflow_config(mode);
        let (call, prepared) = theme_draft_prepared(json!({
            "action": "save",
            "draft_id": "draft-0001",
        }));
        let preparation = permission_preparation_for_mode(&config, &call, &prepared);
        assert!(
            matches!(preparation, PermissionPreparation::Run(_)),
            "save must execute directly in {mode:?}"
        );
    }
}

#[test]
fn theme_draft_save_is_denied_in_plan_mode() {
    let config = workflow_config(PermissionMode::Ask);
    config
        .plan_mode
        .write()
        .expect("plan mode lock")
        .enter_in_memory();
    let (call, prepared) = theme_draft_prepared(json!({
        "action": "save",
        "draft_id": "draft-0001",
    }));
    let preparation = permission_preparation_for_mode(&config, &call, &prepared);
    assert!(
        matches!(preparation, PermissionPreparation::Deny(_)),
        "save must be denied in plan mode"
    );
}

#[test]
fn cached_tool_session_approval_never_authorizes_theme_draft_save() {
    // A generic "approve this tool for the session" grant for a different
    // tool must not leak into ThemeDraft: the ThemeDraft branch returns
    // before cached approvals are consulted, and save never offers a scope.
    let config = workflow_config(PermissionMode::Ask);
    let mut approved = std::collections::HashSet::new();
    approved.insert(SessionApprovalKey::Tool {
        workspace: String::new(),
        name: "ThemeDraft".to_owned(),
    });
    *config.session_approvals.lock().expect("session approvals") = approved;
    let (call, prepared) = theme_draft_prepared(json!({
        "action": "save",
        "draft_id": "draft-0001",
    }));
    let preparation = permission_preparation_for_mode(&config, &call, &prepared);
    assert!(
        matches!(
            preparation,
            PermissionPreparation::Ask {
                operation: PermissionOperation::ThemeSave,
                ..
            }
        ),
        "a cached tool session approval must not authorize a ThemeDraft save"
    );
}
