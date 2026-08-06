use super::*;
use crate::harness::fake_model;
use crate::workspace_policy::WorkspaceAccessPolicy;
use crate::workspace_policy::WorkspaceAccessRoot;
use crate::workspace_policy::WorkspaceAccessRootKind;
use serde_json::json;
use std::sync::Arc;
use std::sync::RwLock;

#[test]
fn plan_mode_denies_write_to_added_write_root() {
    let primary = tempfile::tempdir().expect("primary tempdir");
    let added = tempfile::tempdir().expect("added tempdir");
    let policy = WorkspaceAccessPolicy::with_roots(
        primary.path(),
        [WorkspaceAccessRoot {
            path: added.path().to_path_buf(),
            kind: WorkspaceAccessRootKind::Added,
            read: true,
            write: true,
        }],
    )
    .expect("workspace policy");
    let config = AgentConfig::for_model(fake_model())
        .with_workspace_root(primary.path())
        .expect("workspace root")
        .with_workspace_policy(Arc::new(RwLock::new(Some(policy))))
        .with_permission_mode(PermissionMode::Ask);
    config
        .plan_mode
        .write()
        .expect("plan mode lock")
        .enter_in_memory();
    let blocked_path = added.path().join("blocked.txt");
    let arguments = json!({
        "path": blocked_path.display().to_string(),
        "content": "blocked",
    });
    let call = AgentToolCall {
        id: "call-write-added-root".into(),
        name: "Write".into(),
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

    let preparation = permission_preparation_for_mode(&config, &call, &prepared);

    assert!(matches!(preparation, PermissionPreparation::Deny(_)));
}

#[test]
fn sleep_is_default_approved() {
    let call = AgentToolCall {
        id: "call-sleep".into(),
        name: "Sleep".into(),
        raw_arguments: r#"{"duration_seconds":1,"reason":"wait"}"#.into(),
    };
    assert!(is_default_approved_tool(&call));
}
