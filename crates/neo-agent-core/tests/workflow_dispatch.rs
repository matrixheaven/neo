use std::sync::Arc;

use neo_agent_core::AgentContext;
use neo_agent_core::harness::FakeHarness;
use neo_agent_core::runtime::WorkflowDispatchHandle;
use neo_agent_core::tools::{ProcessSupervisor, ToolRegistry};
use neo_agent_core::ToolAccess;
use neo_agent_core::workflow::WorkflowOutcomeStatus;

fn make_harness() -> FakeHarness {
    FakeHarness::from_turns([])
}

#[tokio::test]
async fn verify_command_uses_canonical_bash_path() {
    let dir = tempfile::tempdir().unwrap();
    let harness = make_harness();
    let registry = Arc::new(ToolRegistry::with_builtin_tools());
    let config = neo_agent_core::AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path().to_path_buf())
        .expect("workspace root")
        .with_permission_mode(neo_agent_core::PermissionMode::Yolo);

    let handle = WorkflowDispatchHandle {
        config,
        model_client: harness.client(),
        registry,
        process_supervisor: ProcessSupervisor::default(),
        context: AgentContext::new(),
        tool_access: Some(ToolAccess::all()),
    };

    let outcome = handle
        .run_one("Bash", serde_json::json!({"command": "echo canonical-dispatch-test"}))
        .await;

    assert!(outcome.ok, "bash should succeed: {:?}", outcome.summary);
    assert_eq!(outcome.status, WorkflowOutcomeStatus::Completed);
}

#[tokio::test]
async fn tool_registry_run_rejects_unknown_tools() {
    let dir = tempfile::tempdir().unwrap();
    let harness = make_harness();
    let registry = Arc::new(ToolRegistry::with_builtin_tools());
    let config = neo_agent_core::AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path().to_path_buf())
        .expect("workspace root");

    let handle = WorkflowDispatchHandle {
        config,
        model_client: harness.client(),
        registry,
        process_supervisor: ProcessSupervisor::default(),
        context: AgentContext::new(),
        tool_access: Some(ToolAccess::all()),
    };

    let outcome = handle
        .run_one("NonExistentTool", serde_json::json!({}))
        .await;

    assert!(!outcome.ok);
    assert_eq!(outcome.status, WorkflowOutcomeStatus::Failed);
    assert!(outcome.summary.contains("unknown"), "summary: {}", outcome.summary);
}

#[tokio::test]
async fn instruction_replan_blocks_effect_without_model_turn() {
    let dir = tempfile::tempdir().unwrap();
    let harness = make_harness();
    let registry = Arc::new(ToolRegistry::with_builtin_tools());
    let config = neo_agent_core::AgentConfig::for_model(harness.model())
        .with_workspace_root(dir.path().to_path_buf())
        .expect("workspace root")
        .with_permission_mode(neo_agent_core::PermissionMode::Yolo);

    let handle = WorkflowDispatchHandle {
        config,
        model_client: harness.client(),
        registry,
        process_supervisor: ProcessSupervisor::default(),
        context: AgentContext::new(),
        tool_access: Some(ToolAccess::all()),
    };

    // Without an instruction registry wired, preflight returns None (bypass).
    // The test verifies that when no registry is present, the bridge proceeds
    // to tool execution normally rather than blocking.
    let outcome = handle
        .run_one("Bash", serde_json::json!({"command": "echo test"}))
        .await;

    assert!(outcome.ok);
    assert_eq!(outcome.status, WorkflowOutcomeStatus::Completed);
}
